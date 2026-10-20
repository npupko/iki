//! Copy text, synthesize Command-V into the focused app, then restore the clipboard.

use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, EventField, KeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use crate::decide::SYNTHETIC_USER_DATA;
use crate::InputError;

const KEY_ANSI_V: u16 = 9; // kVK_ANSI_V
const RESTORE_AFTER: Duration = Duration::from_millis(400);

/// Copy `text` to the clipboard and synthesize Command-V, then restore the previous clipboard
/// about 400 ms later. Empty or whitespace-only text is Ok and does nothing.
/// Must not activate this process. Safe to call off the main thread.
pub fn paste_text(text: &str) -> Result<(), InputError> {
    if text.trim().is_empty() {
        return Ok(());
    }

    let mut clipboard = Clipboard::new().map_err(InputError::Clipboard)?;
    let previous = match clipboard.get_text() {
        Ok(existing) if !existing.is_empty() => Some(existing),
        _ => None,
    };
    clipboard
        .set_text(text.to_string())
        .map_err(InputError::Clipboard)?;
    // NSPasteboard already owns the string; drop our handle before posting the key.
    drop(clipboard);

    if let Err(error) = post_command_v() {
        restore_clipboard(previous);
        return Err(error);
    }

    let _ = thread::Builder::new()
        .name("globe-input-clipboard-restore".to_string())
        .spawn(move || {
            thread::sleep(RESTORE_AFTER);
            restore_clipboard(previous);
        });
    Ok(())
}

fn restore_clipboard(previous: Option<String>) {
    let Ok(mut clipboard) = Clipboard::new() else {
        return;
    };
    let result = match previous {
        Some(text) => clipboard.set_text(text),
        None => clipboard.clear(),
    };
    let _ = result;
}

fn post_command_v() -> Result<(), InputError> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| InputError::EventSource)?;
    let command = CGEventFlags::CGEventFlagCommand;
    // Modifier key plus the Command flag. V is never posted without Command.
    post_key(&source, KeyCode::COMMAND, true, command)?;
    post_key(&source, KEY_ANSI_V, true, command)?;
    post_key(&source, KEY_ANSI_V, false, command)?;
    post_key(&source, KeyCode::COMMAND, false, CGEventFlags::empty())?;
    Ok(())
}

fn post_key(
    source: &CGEventSource,
    keycode: u16,
    key_down: bool,
    flags: CGEventFlags,
) -> Result<(), InputError> {
    let event = CGEvent::new_keyboard_event(source.clone(), keycode, key_down)
        .map_err(|_| InputError::KeyboardEvent)?;
    event.set_flags(flags);
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, SYNTHETIC_USER_DATA);
    // Annotated session delivers like a HID event without activating this process.
    event.post(CGEventTapLocation::AnnotatedSession);
    Ok(())
}
