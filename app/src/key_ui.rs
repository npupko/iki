//! Soniox Key window. Checks a pasted key with Soniox, then saves it in the Keychain.

use std::cell::{Cell, OnceCell, RefCell};
use std::sync::OnceLock;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBezierPath, NSButton, NSColor,
    NSControlTextEditingDelegate, NSFont, NSLineBreakMode, NSMenu, NSMenuItem, NSResponder,
    NSSecureTextField, NSTextField, NSTextFieldDelegate, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSAttributedString, NSNotification, NSPoint, NSRect, NSSize, NSString, NSURL,
};

use crate::api_key::{self, Source, Verdict};
use crate::dashboard::{self, text_attrs_with};
use crate::dictionary_ui::{label, push_button};

const WINDOW_W: f64 = 460.0;
const WINDOW_H: f64 = 210.0;
const PAD: f64 = 20.0;
const CONSOLE_URL: &str = "https://console.soniox.com";

/// The runtime `verify` runs on. Set once at launch.
static RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

pub fn set_runtime(handle: tokio::runtime::Handle) {
    let _ = RUNTIME.set(handle);
}

struct ShellIvars {
    field: OnceCell<Retained<NSSecureTextField>>,
    status: OnceCell<Retained<NSTextField>>,
    remove: OnceCell<Retained<NSButton>>,
    save: OnceCell<Retained<NSButton>>,
    checking: Cell<bool>,
}

struct KeyWindow {
    window: Retained<NSWindow>,
    shell: Retained<KeyShell>,
}

std::thread_local! {
    static OPEN: RefCell<Option<KeyWindow>> = const { RefCell::new(None) };
}

pub fn show(mtm: MainThreadMarker) {
    OPEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(KeyWindow::create(mtm));
        }
        let window = slot.as_ref().unwrap();
        window.shell.reset();
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        window.window.makeKeyAndOrderFront(None);
        window.window.makeFirstResponder(Some(window.shell.field()));
    });
}

impl KeyWindow {
    fn create(mtm: MainThreadMarker) -> Self {
        let shell = KeyShell::new(mtm);
        let inner_w = WINDOW_W - 2.0 * PAD;

        let intro = label("Iki sends your audio to Soniox with this key.", mtm);
        intro.setTextColor(Some(&NSColor::labelColor()));
        intro.setFrame(rect(PAD, PAD, inner_w, 18.0));

        let field = NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(mtm),
            rect(PAD, 48.0, inner_w, 24.0),
        );
        field.setPlaceholderString(Some(&NSString::from_str("Paste your Soniox API key")));
        field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        // Return submits; leaving the field does not.
        if let Some(cell) = field.cell() {
            cell.setSendsActionOnEndEditing(false);
        }

        let status = label("", mtm);
        status.setFrame(rect(PAD, 82.0, inner_w, 18.0));

        let console =
            NSButton::initWithFrame(NSButton::alloc(mtm), rect(PAD, 106.0, inner_w, 20.0));
        console.setBordered(false);
        let attrs = text_attrs_with(
            &NSFont::systemFontOfSize(13.0),
            &NSColor::linkColor(),
            NSLineBreakMode::ByTruncatingTail,
        );
        let title = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str("Get a key in the Soniox Console"),
                Some(&attrs),
            )
        };
        console.setAttributedTitle(&title);
        console.sizeToFit();
        let fitted = console.frame().size;
        console.setFrame(rect(
            PAD - 2.0,
            106.0,
            fitted.width,
            fitted.height.max(20.0),
        ));

        let button_y = WINDOW_H - PAD - 32.0;
        let save = push_button(
            "Save",
            rect(WINDOW_W - PAD - 96.0, button_y, 96.0, 32.0),
            mtm,
        );
        save.setKeyEquivalent(&NSString::from_str("\r"));
        let remove = push_button(
            "Remove saved key",
            rect(WINDOW_W - PAD - 96.0 - 8.0 - 150.0, button_y, 150.0, 32.0),
            mtm,
        );

        for view in [
            &*intro as &NSView,
            &*field,
            &*status,
            &*console,
            &*remove,
            &*save,
        ] {
            let _: () = unsafe { msg_send![&*shell, addSubview: view] };
        }

        unsafe {
            field.setDelegate(Some(ProtocolObject::from_ref(&*shell)));
            field.setTarget(Some(&*shell));
            field.setAction(Some(sel!(saveKey:)));
            save.setTarget(Some(&*shell));
            save.setAction(Some(sel!(saveKey:)));
            remove.setTarget(Some(&*shell));
            remove.setAction(Some(sel!(removeKey:)));
            console.setTarget(Some(&*shell));
            console.setAction(Some(sel!(openConsole:)));
        }

        let ivars = shell.ivars();
        ivars.field.set(field).ok();
        ivars.status.set(status).ok();
        ivars.remove.set(remove).ok();
        ivars.save.set(save).ok();

        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(WINDOW_W, WINDOW_H)),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Soniox Key"));
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        window.setContentView(Some(&*shell));
        window.setDelegate(Some(ProtocolObject::from_ref(&*shell)));
        dashboard::center_on_mouse_screen(&window, mtm);
        Self { window, shell }
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

fn source_line(source: Option<Source>) -> &'static str {
    match source {
        Some(Source::Keychain) => "Using the key saved in Keychain.",
        Some(Source::Environment) => "Using SONIOX_API_KEY from the environment.",
        Some(Source::Fnox) => "Using the key from fnox.",
        None => "No key yet.",
    }
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ShellIvars]
    #[name = "IkiKeyShell"]
    struct KeyShell;

    unsafe impl NSObjectProtocol for KeyShell {}

    unsafe impl NSWindowDelegate for KeyShell {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: &NSWindow) -> bool {
            sender.orderOut(None);
            false
        }
    }

    unsafe impl NSControlTextEditingDelegate for KeyShell {
        #[unsafe(method(controlTextDidChange:))]
        #[allow(non_snake_case)]
        fn controlTextDidChange(&self, _obj: &NSNotification) {
            self.update_buttons();
        }
    }

    unsafe impl NSTextFieldDelegate for KeyShell {}

    impl KeyShell {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            NSColor::windowBackgroundColor().setFill();
            NSBezierPath::fillRect(dirty);
        }

        #[unsafe(method(saveKey:))]
        fn save_key(&self, _sender: Option<&AnyObject>) {
            self.start_check();
        }

        #[unsafe(method(removeKey:))]
        fn remove_key(&self, _sender: Option<&AnyObject>) {
            match api_key::remove() {
                Ok(()) => self.show_source(),
                Err(error) => {
                    eprintln!("iki: keychain: {error}");
                    self.set_status("Couldn't remove the key from Keychain.", true);
                }
            }
            self.update_buttons();
        }

        #[unsafe(method(openConsole:))]
        fn open_console(&self, _sender: Option<&AnyObject>) {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(CONSOLE_URL)) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        }
    }
);

impl KeyShell {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ShellIvars {
            field: OnceCell::new(),
            status: OnceCell::new(),
            remove: OnceCell::new(),
            save: OnceCell::new(),
            checking: Cell::new(false),
        });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    fn field(&self) -> &NSSecureTextField {
        self.ivars().field.get().expect("field")
    }
    fn status(&self) -> &NSTextField {
        self.ivars().status.get().expect("status")
    }
    fn remove(&self) -> &NSButton {
        self.ivars().remove.get().expect("remove")
    }
    fn save(&self) -> &NSButton {
        self.ivars().save.get().expect("save")
    }

    fn typed_key(&self) -> String {
        self.field().stringValue().to_string().trim().to_string()
    }

    /// The window opens empty. It never shows the stored key.
    fn reset(&self) {
        if self.ivars().checking.get() {
            return;
        }
        self.field().setStringValue(&NSString::from_str(""));
        self.show_source();
        self.update_buttons();
    }

    fn show_source(&self) {
        self.set_status(source_line(api_key::current_source()), false);
    }

    fn set_status(&self, text: &str, error: bool) {
        let status = self.status();
        status.setStringValue(&NSString::from_str(text));
        let color = if error {
            NSColor::systemRedColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        status.setTextColor(Some(&color));
    }

    fn update_buttons(&self) {
        let checking = self.ivars().checking.get();
        self.save()
            .setEnabled(!checking && !self.typed_key().is_empty());
        self.remove()
            .setHidden(api_key::current_source() != Some(Source::Keychain));
        self.remove().setEnabled(!checking);
        self.field().setEnabled(!checking);
    }

    fn start_check(&self) {
        if self.ivars().checking.get() {
            return;
        }
        let key = self.typed_key();
        if key.is_empty() {
            return;
        }
        let Some(runtime) = RUNTIME.get() else {
            self.set_status(
                "Couldn't reach Soniox. Check your connection and try again.",
                true,
            );
            return;
        };
        self.ivars().checking.set(true);
        self.set_status("Checking with Soniox…", false);
        self.update_buttons();
        runtime.spawn(async move {
            let verdict = api_key::verify(&key).await;
            DispatchQueue::main().exec_async(move || finish_check(key, verdict));
        });
    }

    fn finish(&self, key: &str, verdict: Verdict) {
        self.ivars().checking.set(false);
        match verdict {
            Verdict::Accepted => match api_key::save(key) {
                Ok(()) => {
                    self.field().setStringValue(&NSString::from_str(""));
                    self.set_status("Saved. Soniox accepted the key.", false);
                }
                Err(_) => self.set_status(
                    "Soniox accepted the key, but Keychain didn't save it.",
                    true,
                ),
            },
            Verdict::Rejected => self.set_status("Soniox rejected this key.", true),
            Verdict::Unreachable(detail) => {
                eprintln!("iki: soniox key check: {detail}");
                self.set_status(
                    "Couldn't reach Soniox. Check your connection and try again.",
                    true,
                );
            }
        }
        self.update_buttons();
    }
}

/// Runs on the main queue once Soniox has answered.
fn finish_check(key: String, verdict: Verdict) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    OPEN.with(|slot| {
        if let Some(window) = slot.borrow().as_ref() {
            window.shell.finish(&key, verdict);
        }
    });
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IkiKeyItem"]
    struct KeyItem;

    unsafe impl NSObjectProtocol for KeyItem {}

    impl KeyItem {
        #[unsafe(method(showKey:))]
        fn show_key(&self, _sender: Option<&AnyObject>) {
            if let Some(mtm) = MainThreadMarker::new() {
                show(mtm);
            }
        }
    }
);

fn menu_target(mtm: MainThreadMarker) -> Retained<KeyItem> {
    let this = KeyItem::alloc(mtm);
    unsafe { msg_send![this, init] }
}

pub fn add_menu_item(menu: &NSMenu, mtm: MainThreadMarker) {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Soniox Key…"),
            Some(sel!(showKey:)),
            &NSString::from_str(""),
        )
    };
    let target = menu_target(mtm);
    unsafe { item.setTarget(Some(&*target)) };
    menu.addItem(&item);
    std::mem::forget(target);
}

#[cfg(test)]
mod tests {
    use super::source_line;
    use crate::api_key::Source;

    #[test]
    fn the_status_line_names_where_the_key_comes_from() {
        assert_eq!(
            source_line(Some(Source::Keychain)),
            "Using the key saved in Keychain."
        );
        assert!(source_line(Some(Source::Environment)).contains("SONIOX_API_KEY"));
        assert!(source_line(Some(Source::Fnox)).contains("fnox"));
        assert_eq!(source_line(None), "No key yet.");
    }
}
