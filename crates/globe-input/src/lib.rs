//! Globe / fn hold detection and paste-into-the-focused-app for macOS.
//!
//! Installing the event tap needs Accessibility and Input Monitoring permission.
//! `paste_text` posts Command-V to whatever is already focused and does not activate this process.

#[cfg(not(target_os = "macos"))]
compile_error!("globe-input supports macOS only");

#[cfg(target_os = "macos")]
mod decide;
#[cfg(target_os = "macos")]
mod paste;
#[cfg(target_os = "macos")]
mod tap;

#[cfg(target_os = "macos")]
pub use decide::{
    decide, HoldState, Incoming, TapAction, FLAG_SECONDARY_FN, KEY_FN, KEY_GLOBE,
    SYNTHETIC_USER_DATA,
};
#[cfg(target_os = "macos")]
pub use paste::paste_text;
#[cfg(target_os = "macos")]
pub use tap::HoldWatch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldEdge {
    Down,
    Up,
}

/// Failure from the event tap or the clipboard paste.
#[cfg(target_os = "macos")]
#[derive(Debug)]
pub enum InputError {
    /// `CGEventTapCreate` returned null.
    /// Enable Accessibility and Input Monitoring for this app.
    EventTapUnavailable,
    /// The text clipboard could not be read or written.
    Clipboard(arboard::Error),
    /// `CGEventSourceCreate` failed.
    EventSource,
    /// `CGEventCreateKeyboardEvent` failed.
    KeyboardEvent,
    /// The tap's run-loop source could not be created.
    RunLoopSource,
}

#[cfg(target_os = "macos")]
impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputError::EventTapUnavailable => {
                write!(f, "Allow Iki in Accessibility")
            }
            InputError::Clipboard(error) => write!(f, "clipboard error: {error}"),
            InputError::EventSource => write!(f, "could not create a CGEventSource"),
            InputError::KeyboardEvent => write!(f, "could not create a keyboard event"),
            InputError::RunLoopSource => {
                write!(f, "could not create a run loop source for the event tap")
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl std::error::Error for InputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InputError::Clipboard(error) => Some(error),
            _ => None,
        }
    }
}
