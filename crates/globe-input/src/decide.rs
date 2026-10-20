//! Pure Globe / fn hold policy. No event tap is installed here.

use crate::HoldEdge;

/// Apple Globe key (`NX_KEYTYPE_GLOBE` / virtual key 179).
pub const KEY_GLOBE: i64 = 179;
/// `kVK_Function`.
pub const KEY_FN: i64 = 63;
/// `kCGEventFlagMaskSecondaryFn`.
pub const FLAG_SECONDARY_FN: u64 = 0x0080_0000;
/// `kCGEventSourceUserData` value stamped on events this crate synthesizes.
#[allow(clippy::unusual_byte_groupings)]
pub const SYNTHETIC_USER_DATA: i64 = 0x6C0_BE;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HoldState {
    pub down: bool,
    /// Last observed secondary-fn flag. Used only to detect rising and falling edges.
    pub fn_flag: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapAction {
    Pass,
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Incoming {
    KeyDown {
        keycode: i64,
        autorepeat: bool,
        user_data: i64,
    },
    KeyUp {
        keycode: i64,
        user_data: i64,
    },
    FlagsChanged {
        flags: u64,
        user_data: i64,
    },
    TapDisabled,
}

/// Decide whether to swallow `event` and whether it edges the Globe/fn hold.
///
/// A second key-down while [`HoldState::down`] is set is autorepeat: drop it, no edge.
/// The `autorepeat` field is carried for the caller; the edge follows `state.down`.
pub fn decide(state: &mut HoldState, event: Incoming) -> (TapAction, Option<HoldEdge>) {
    match event {
        Incoming::KeyDown { user_data, .. }
        | Incoming::KeyUp { user_data, .. }
        | Incoming::FlagsChanged { user_data, .. }
            if user_data == SYNTHETIC_USER_DATA =>
        {
            (TapAction::Pass, None)
        }
        Incoming::TapDisabled => (TapAction::Pass, None),
        Incoming::KeyDown { keycode, .. } => key_transition(state, keycode, true),
        Incoming::KeyUp { keycode, .. } => key_transition(state, keycode, false),
        Incoming::FlagsChanged { flags, .. } => flags_transition(state, flags),
    }
}

fn is_hold_key(keycode: i64) -> bool {
    keycode == KEY_GLOBE || keycode == KEY_FN
}

fn key_transition(
    state: &mut HoldState,
    keycode: i64,
    down: bool,
) -> (TapAction, Option<HoldEdge>) {
    if !is_hold_key(keycode) {
        return (TapAction::Pass, None);
    }
    if down {
        if state.down {
            (TapAction::Drop, None)
        } else {
            state.down = true;
            (TapAction::Drop, Some(HoldEdge::Down))
        }
    } else if state.down {
        state.down = false;
        (TapAction::Drop, Some(HoldEdge::Up))
    } else {
        (TapAction::Drop, None)
    }
}

fn flags_transition(state: &mut HoldState, flags: u64) -> (TapAction, Option<HoldEdge>) {
    let fn_now = (flags & FLAG_SECONDARY_FN) != 0;
    let changed = fn_now != state.fn_flag;
    state.fn_flag = fn_now;
    if !changed {
        // Unrelated modifiers, or the fn bit already matched. Do not swallow Shift/Command.
        return (TapAction::Pass, None);
    }
    // The fn bit itself flipped. Drop it so Character Viewer / the emoji picker stays closed.
    if fn_now && !state.down {
        state.down = true;
        (TapAction::Drop, Some(HoldEdge::Down))
    } else if !fn_now && state.down {
        state.down = false;
        (TapAction::Drop, Some(HoldEdge::Up))
    } else {
        (TapAction::Drop, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn globe_down(autorepeat: bool) -> Incoming {
        Incoming::KeyDown {
            keycode: KEY_GLOBE,
            autorepeat,
            user_data: 0,
        }
    }

    fn globe_up() -> Incoming {
        Incoming::KeyUp {
            keycode: KEY_GLOBE,
            user_data: 0,
        }
    }

    fn fn_flags(on: bool) -> Incoming {
        Incoming::FlagsChanged {
            flags: if on { FLAG_SECONDARY_FN } else { 0 },
            user_data: 0,
        }
    }

    #[test]
    fn globe_down_emits_down_and_drops() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(&mut state, globe_down(false)),
            (TapAction::Drop, Some(HoldEdge::Down))
        );
        assert!(state.down);
        assert!(!state.fn_flag);
    }

    #[test]
    fn globe_autorepeat_drops_without_edge() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(&mut state, globe_down(false)),
            (TapAction::Drop, Some(HoldEdge::Down))
        );
        assert_eq!(
            decide(&mut state, globe_down(true)),
            (TapAction::Drop, None)
        );
        assert!(state.down);
    }

    #[test]
    fn globe_up_emits_up_and_drops() {
        let mut state = HoldState::default();
        let _ = decide(&mut state, globe_down(false));
        assert_eq!(
            decide(&mut state, globe_up()),
            (TapAction::Drop, Some(HoldEdge::Up))
        );
        assert!(!state.down);
    }

    #[test]
    fn fn_key_down_and_up() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(
                &mut state,
                Incoming::KeyDown {
                    keycode: KEY_FN,
                    autorepeat: false,
                    user_data: 0,
                }
            ),
            (TapAction::Drop, Some(HoldEdge::Down))
        );
        assert_eq!(
            decide(
                &mut state,
                Incoming::KeyUp {
                    keycode: KEY_FN,
                    user_data: 0,
                }
            ),
            (TapAction::Drop, Some(HoldEdge::Up))
        );
        assert!(!state.down);
    }

    #[test]
    fn fn_flag_rise_emits_down_and_drops() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(&mut state, fn_flags(true)),
            (TapAction::Drop, Some(HoldEdge::Down))
        );
        assert!(state.down);
        assert!(state.fn_flag);
    }

    #[test]
    fn fn_flag_fall_emits_up_and_drops() {
        let mut state = HoldState::default();
        let _ = decide(&mut state, fn_flags(true));
        assert_eq!(
            decide(&mut state, fn_flags(false)),
            (TapAction::Drop, Some(HoldEdge::Up))
        );
        assert!(!state.down);
        assert!(!state.fn_flag);
    }

    #[test]
    fn letter_key_passes_with_no_edge() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(
                &mut state,
                Incoming::KeyDown {
                    keycode: 0,
                    autorepeat: false,
                    user_data: 0,
                }
            ),
            (TapAction::Pass, None)
        );
        assert_eq!(
            decide(
                &mut state,
                Incoming::KeyUp {
                    keycode: 0,
                    user_data: 0,
                }
            ),
            (TapAction::Pass, None)
        );
        assert!(!state.down);
    }

    #[test]
    fn synthetic_user_data_passes() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(
                &mut state,
                Incoming::KeyDown {
                    keycode: KEY_GLOBE,
                    autorepeat: false,
                    user_data: SYNTHETIC_USER_DATA,
                }
            ),
            (TapAction::Pass, None)
        );
        assert_eq!(
            decide(
                &mut state,
                Incoming::FlagsChanged {
                    flags: FLAG_SECONDARY_FN,
                    user_data: SYNTHETIC_USER_DATA,
                }
            ),
            (TapAction::Pass, None)
        );
        assert!(!state.down);
        assert!(!state.fn_flag);
    }

    #[test]
    fn keydown_then_flags_does_not_emit_second_down() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(&mut state, globe_down(false)),
            (TapAction::Drop, Some(HoldEdge::Down))
        );
        assert_eq!(decide(&mut state, fn_flags(true)), (TapAction::Drop, None));
        assert!(state.down);
        assert!(state.fn_flag);
        // Release: one Up, whether the flag falls or the key goes up first.
        assert_eq!(
            decide(&mut state, fn_flags(false)),
            (TapAction::Drop, Some(HoldEdge::Up))
        );
        assert_eq!(decide(&mut state, globe_up()), (TapAction::Drop, None));
        assert!(!state.down);
    }

    #[test]
    fn flags_then_keydown_does_not_emit_second_down() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(&mut state, fn_flags(true)),
            (TapAction::Drop, Some(HoldEdge::Down))
        );
        assert_eq!(
            decide(&mut state, globe_down(false)),
            (TapAction::Drop, None)
        );
        assert!(state.down);
        assert_eq!(
            decide(&mut state, globe_up()),
            (TapAction::Drop, Some(HoldEdge::Up))
        );
        assert_eq!(decide(&mut state, fn_flags(false)), (TapAction::Drop, None));
        assert!(!state.down);
        assert!(!state.fn_flag);
    }

    #[test]
    fn unrelated_flags_pass() {
        let mut state = HoldState::default();
        assert_eq!(
            decide(
                &mut state,
                Incoming::FlagsChanged {
                    flags: 0x0010_0000,
                    user_data: 0,
                }
            ),
            (TapAction::Pass, None)
        );
        assert!(!state.down);
        assert!(!state.fn_flag);
    }

    #[test]
    fn tap_disabled_passes_without_edge_or_state_change() {
        let mut state = HoldState {
            down: true,
            fn_flag: true,
        };
        assert_eq!(
            decide(&mut state, Incoming::TapDisabled),
            (TapAction::Pass, None)
        );
        assert!(state.down);
        assert!(state.fn_flag);
    }

    #[test]
    fn globe_up_when_not_down_drops_without_edge() {
        let mut state = HoldState::default();
        assert_eq!(decide(&mut state, globe_up()), (TapAction::Drop, None));
        assert!(!state.down);
    }
}
