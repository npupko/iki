//! Session event tap. `core_graphics::event::CGEventTap` cannot swallow events
//! (`None` forwards the original), so this installs `CGEventTapCreate` directly
//! and returns NULL to drop Globe / fn.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::mach_port::{CFMachPort, CFMachPortRef};
use core_foundation::runloop::{
    kCFRunLoopCommonModes, CFRunLoop, CFRunLoopSource, CFRunLoopSourceInvalidate,
    CFRunLoopSourceRef,
};
use core_foundation::string::{CFString, CFStringRef};

use crate::decide::{decide, HoldState, Incoming, TapAction};
use crate::{HoldEdge, InputError};

const TAP_SESSION: u32 = 1; // kCGSessionEventTap
const TAP_HEAD_INSERT: u32 = 0; // kCGHeadInsertEventTap
const TAP_OPTIONS_DEFAULT: u32 = 0; // kCGEventTapOptionDefault, not listen-only
const EVENT_KEY_DOWN: u32 = 10;
const EVENT_KEY_UP: u32 = 11;
const EVENT_FLAGS_CHANGED: u32 = 12;
const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
const EVENT_MASK: u64 =
    (1u64 << EVENT_KEY_DOWN) | (1u64 << EVENT_KEY_UP) | (1u64 << EVENT_FLAGS_CHANGED);
const FIELD_AUTOREPEAT: u32 = 8; // kCGKeyboardEventAutorepeat
const FIELD_KEYCODE: u32 = 9; // kCGKeyboardEventKeycode
const FIELD_USER_DATA: u32 = 42; // kCGEventSourceUserData

type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
}

type CGEventTapCallBack = unsafe extern "C" fn(
    proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
}

struct TapState {
    hold: HoldState,
    /// Taken out for the duration of `on_edge` so a nested tap callback cannot alias it.
    on_edge: Option<Box<dyn Fn(HoldEdge) + Send>>,
    port: AtomicPtr<c_void>,
}

pub struct HoldWatch {
    state: Box<TapState>,
    port: CFMachPort,
    source: CFRunLoopSource,
    run_loop: CFRunLoop,
}

impl HoldWatch {
    /// Install a session CGEvent tap on the CURRENT thread's run loop.
    /// The thread must already be (or be about to become) a CFRunLoop, which the AppKit main thread is.
    /// Do not call CFRunLoop::run yourself.
    /// `on_edge` is invoked on the tap thread (the main thread) and must be fast.
    pub fn install<F>(on_edge: F) -> Result<Self, InputError>
    where
        F: Fn(HoldEdge) + Send + 'static,
    {
        let state = Box::new(TapState {
            hold: HoldState::default(),
            on_edge: Some(Box::new(on_edge)),
            port: AtomicPtr::new(ptr::null_mut()),
        });
        // into_raw keeps the allocation stable for the C callback.
        let state_ptr = Box::into_raw(state);
        let port_ref = unsafe {
            CGEventTapCreate(
                TAP_SESSION,
                TAP_HEAD_INSERT,
                TAP_OPTIONS_DEFAULT,
                EVENT_MASK,
                tap_callback,
                state_ptr.cast(),
            )
        };
        if port_ref.is_null() {
            drop(unsafe { Box::from_raw(state_ptr) });
            prompt_for_accessibility();
            return Err(InputError::EventTapUnavailable);
        }
        unsafe {
            (*state_ptr).port.store(port_ref.cast(), Ordering::Release);
        }
        let state = unsafe { Box::from_raw(state_ptr) };
        let port = unsafe { CFMachPort::wrap_under_create_rule(port_ref) };
        let source = port
            .create_runloop_source(0)
            .map_err(|_| InputError::RunLoopSource)?;
        let run_loop = CFRunLoop::get_current();
        let mode = unsafe { kCFRunLoopCommonModes };
        run_loop.add_source(&source, mode);
        unsafe { CGEventTapEnable(port.as_concrete_TypeRef(), true) };
        Ok(Self {
            state,
            port,
            source,
            run_loop,
        })
    }

    /// Re-enable the tap after macOS disables it (timeout / user input).
    pub fn enable(&self) {
        unsafe { CGEventTapEnable(self.port.as_concrete_TypeRef(), true) }
    }
}

impl Drop for HoldWatch {
    fn drop(&mut self) {
        self.state.port.store(ptr::null_mut(), Ordering::Release);
        let mode = unsafe { kCFRunLoopCommonModes };
        unsafe {
            CGEventTapEnable(self.port.as_concrete_TypeRef(), false);
            self.run_loop.remove_source(&self.source, mode);
            invalidate_source(self.source.as_concrete_TypeRef());
        }
    }
}

fn invalidate_source(source: CFRunLoopSourceRef) {
    unsafe { CFRunLoopSourceInvalidate(source) }
}

fn prompt_for_accessibility() {
    unsafe {
        let key_ref = kAXTrustedCheckOptionPrompt;
        if key_ref.is_null() {
            return;
        }
        let key = CFString::wrap_under_get_rule(key_ref);
        let value = CFBoolean::true_value();
        let options: CFDictionary<CFString, CFBoolean> =
            CFDictionary::from_CFType_pairs(&[(key, value)]);
        let _trusted = AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
    }
}

unsafe extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    if user_info.is_null() {
        return event;
    }
    handle_tap(user_info.cast(), event_type, event)
}

fn handle_tap(state_ptr: *mut TapState, event_type: u32, event: CGEventRef) -> CGEventRef {
    if event_type == EVENT_TAP_DISABLED_BY_TIMEOUT || event_type == EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        unsafe {
            let state = &mut *state_ptr;
            let _ = decide(&mut state.hold, Incoming::TapDisabled);
            let port = state.port.load(Ordering::Acquire) as CFMachPortRef;
            if !port.is_null() {
                CGEventTapEnable(port, true);
            }
        }
        return event;
    }
    if event.is_null() {
        return event;
    }

    let incoming = match event_type {
        EVENT_KEY_DOWN => Incoming::KeyDown {
            keycode: unsafe { CGEventGetIntegerValueField(event, FIELD_KEYCODE) },
            autorepeat: unsafe { CGEventGetIntegerValueField(event, FIELD_AUTOREPEAT) } != 0,
            user_data: unsafe { CGEventGetIntegerValueField(event, FIELD_USER_DATA) },
        },
        EVENT_KEY_UP => Incoming::KeyUp {
            keycode: unsafe { CGEventGetIntegerValueField(event, FIELD_KEYCODE) },
            user_data: unsafe { CGEventGetIntegerValueField(event, FIELD_USER_DATA) },
        },
        EVENT_FLAGS_CHANGED => Incoming::FlagsChanged {
            flags: unsafe { CGEventGetFlags(event) },
            user_data: unsafe { CGEventGetIntegerValueField(event, FIELD_USER_DATA) },
        },
        _ => return event,
    };

    let (action, callback) = unsafe {
        let state = &mut *state_ptr;
        let (action, edge) = decide(&mut state.hold, incoming);
        let callback = edge.and_then(|edge| state.on_edge.take().map(|on_edge| (on_edge, edge)));
        (action, callback)
    };
    if let Some((on_edge, edge)) = callback {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_edge(edge)));
        unsafe {
            (*state_ptr).on_edge = Some(on_edge);
        }
    }
    match action {
        TapAction::Pass => event,
        TapAction::Drop => ptr::null_mut(),
    }
}
