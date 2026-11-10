//! Dictionary window. Vocabulary plus the language order sent to Soniox.

use std::cell::{Cell, OnceCell, RefCell};

use objc2::ffi::NSInteger;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSBackingStoreType, NSBezelStyle,
    NSBezierPath, NSBorderType, NSButton, NSButtonType, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSControlTextEditingDelegate, NSEvent, NSFont, NSLineBreakMode, NSMenu,
    NSMenuItem, NSPopUpButton, NSResponder, NSScrollView, NSTextField, NSTextFieldDelegate, NSView,
    NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSEdgeInsets, NSNotification, NSPoint, NSRect, NSSize, NSString};

use crate::dashboard::{self, draw_string, text_attrs_with};
use crate::dictionary::{self, Sort};
use crate::languages;

const WINDOW_W: f64 = 720.0;
const WINDOW_H: f64 = 720.0;
const PAD: f64 = 20.0;
const ROW_H: f64 = 34.0;
const LANG_ROW_H: f64 = 28.0;

struct ShellIvars {
    stats: OnceCell<Retained<NSTextField>>,
    language_label: OnceCell<Retained<NSTextField>>,
    language_note: OnceCell<Retained<NSTextField>>,
    language_popup: OnceCell<Retained<NSPopUpButton>>,
    language_add: OnceCell<Retained<NSButton>>,
    language_scroll: OnceCell<Retained<NSScrollView>>,
    language_rows: OnceCell<Retained<LanguageRows>>,
    addable: RefCell<Vec<String>>,
    shown_codes: RefCell<Vec<String>>,
    row_width: Cell<f64>,
    vocabulary_label: OnceCell<Retained<NSTextField>>,
    search: OnceCell<Retained<NSTextField>>,
    sort_popup: OnceCell<Retained<NSPopUpButton>>,
    word: OnceCell<Retained<NSTextField>>,
    spelling: OnceCell<Retained<NSTextField>>,
    correction: OnceCell<Retained<NSButton>>,
    submit_button: OnceCell<Retained<NSButton>>,
    cancel_button: OnceCell<Retained<NSButton>>,
    delete_button: OnceCell<Retained<NSButton>>,
    error: OnceCell<Retained<NSTextField>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    list: OnceCell<Retained<DictionaryList>>,
    editing: RefCell<Option<String>>,
    suppress: Cell<bool>,
    separator_y: Cell<f64>,
}

struct ListIvars {
    listed: RefCell<Vec<Listed>>,
    rows: RefCell<Vec<Row>>,
    query_empty: Cell<bool>,
    press: Cell<Option<NSPoint>>,
    width: Cell<f64>,
    /// Shell that owns this list.
    host: OnceCell<Weak<DictionaryShell>>,
}

struct Listed {
    phrase: String,
    label: String,
    starred: bool,
}

struct Row {
    phrase: String,
    label: String,
    starred: bool,
    y: f64,
    height: f64,
    empty: bool,
}

struct DictionaryWindow {
    window: Retained<NSWindow>,
    shell: Retained<DictionaryShell>,
}

std::thread_local! {
    static OPEN: RefCell<Option<DictionaryWindow>> = const { RefCell::new(None) };
}

pub fn show(mtm: MainThreadMarker) {
    OPEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(DictionaryWindow::create(mtm));
        }
        let window = slot.as_ref().unwrap();
        window.shell.refresh();
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        window.window.makeKeyAndOrderFront(None);
    });
}

impl DictionaryWindow {
    fn create(mtm: MainThreadMarker) -> Self {
        let shell = DictionaryShell::new(mtm);
        let stats = label("", mtm);
        let language_label = section_label("Languages", mtm);
        let language_note = label("Soniox prefers this list, in order.", mtm);
        let language_popup = popup(mtm);
        let language_add = push_button("Add", NSRect::ZERO, mtm);
        let language_rows = LanguageRows::new(mtm);
        let language_scroll = scroll_view(mtm);
        language_scroll.setDocumentView(Some(&*language_rows));

        let vocabulary_label = section_label("Vocabulary", mtm);
        let search = entry_field("Search your vocabulary", mtm);
        let sort_popup = popup(mtm);
        for title in ["Newest", "Oldest", "Starred first", "Alphabetical"] {
            sort_popup.addItemWithTitle(&NSString::from_str(title));
        }
        let word = entry_field("Add a new word", mtm);
        let spelling = entry_field("Correct spelling", mtm);
        spelling.setHidden(true);

        let correction = NSButton::initWithFrame(NSButton::alloc(mtm), NSRect::ZERO);
        correction.setButtonType(NSButtonType::Switch);
        correction.setTitle(&NSString::from_str("Correct a misspelling"));
        correction.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        correction.setState(NSControlStateValueOff);
        correction.setToolTip(Some(&NSString::from_str(
            "When this is on, Iki replaces the misspelling with the correct spelling after dictation.",
        )));

        let submit_button = push_button("Add word", NSRect::ZERO, mtm);
        let cancel_button = push_button("Cancel", NSRect::ZERO, mtm);
        cancel_button.setHidden(true);
        let delete_button = push_button("Delete", NSRect::ZERO, mtm);
        delete_button.setHidden(true);
        let error = label("", mtm);
        error.setTextColor(Some(&NSColor::systemRedColor()));

        let list = DictionaryList::new(mtm);
        let scroll = scroll_view(mtm);
        scroll.setDocumentView(Some(&*list));

        for view in [
            &*stats as &NSView,
            &*language_label,
            &*language_note,
            &*language_popup,
            &*language_add,
            &*language_scroll,
            &*vocabulary_label,
            &*search,
            &*sort_popup,
            &*word,
            &*spelling,
            &*correction,
            &*submit_button,
            &*cancel_button,
            &*delete_button,
            &*error,
            &*scroll,
        ] {
            let _: () = unsafe { msg_send![&*shell, addSubview: view] };
        }

        let ivars = shell.ivars();
        ivars.stats.set(stats).ok();
        ivars.language_label.set(language_label).ok();
        ivars.language_note.set(language_note).ok();
        ivars.language_popup.set(language_popup.clone()).ok();
        ivars.language_add.set(language_add.clone()).ok();
        ivars.language_scroll.set(language_scroll).ok();
        ivars.language_rows.set(language_rows).ok();
        ivars.vocabulary_label.set(vocabulary_label).ok();
        ivars.search.set(search.clone()).ok();
        ivars.sort_popup.set(sort_popup.clone()).ok();
        ivars.word.set(word.clone()).ok();
        ivars.spelling.set(spelling.clone()).ok();
        ivars.correction.set(correction.clone()).ok();
        ivars.submit_button.set(submit_button.clone()).ok();
        ivars.cancel_button.set(cancel_button.clone()).ok();
        ivars.delete_button.set(delete_button.clone()).ok();
        ivars.error.set(error).ok();
        ivars.scroll.set(scroll).ok();
        ivars.list.set(list.clone()).ok();
        list.set_host(&shell);

        wire(
            &*shell,
            &search,
            &sort_popup,
            &word,
            &spelling,
            &correction,
            &submit_button,
            &cancel_button,
            &delete_button,
            &language_add,
        );

        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
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
        window.setTitle(&NSString::from_str("Dictionary"));
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        window.setContentMinSize(NSSize::new(520.0, 560.0));
        window.setContentView(Some(&*shell));
        window.setDelegate(Some(ProtocolObject::from_ref(&*shell)));
        dashboard::center_on_mouse_screen(&window, mtm);
        Self { window, shell }
    }
}

fn wire(
    shell: &DictionaryShell,
    search: &NSTextField,
    sort_popup: &NSPopUpButton,
    word: &NSTextField,
    spelling: &NSTextField,
    correction: &NSButton,
    submit_button: &NSButton,
    cancel_button: &NSButton,
    delete_button: &NSButton,
    language_add: &NSButton,
) {
    unsafe {
        search.setDelegate(Some(ProtocolObject::from_ref(shell)));
        sort_popup.setTarget(Some(shell));
        sort_popup.setAction(Some(sel!(sortChanged:)));
        word.setTarget(Some(shell));
        word.setAction(Some(sel!(submitWord:)));
        spelling.setTarget(Some(shell));
        spelling.setAction(Some(sel!(submitWord:)));
        correction.setTarget(Some(shell));
        correction.setAction(Some(sel!(toggleCorrection:)));
        submit_button.setTarget(Some(shell));
        submit_button.setAction(Some(sel!(submitWord:)));
        cancel_button.setTarget(Some(shell));
        cancel_button.setAction(Some(sel!(cancelEdit:)));
        delete_button.setTarget(Some(shell));
        delete_button.setAction(Some(sel!(deleteWord:)));
        language_add.setTarget(Some(shell));
        language_add.setAction(Some(sel!(addLanguage:)));
    }
}

/// An editable field whose action fires on Return only. AppKit's default also
/// fires it when the field loses focus, which would submit a half-filled form.
fn entry_field(placeholder: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
    field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
    field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    if let Some(cell) = field.cell() {
        cell.setSendsActionOnEndEditing(false);
    }
    field
}

pub(crate) fn label(text: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    field.setMaximumNumberOfLines(1);
    let _: () = unsafe { msg_send![&*field, setLineBreakMode: NSLineBreakMode::ByTruncatingTail] };
    field
}

fn section_label(text: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = label(text, mtm);
    field.setTextColor(Some(&NSColor::labelColor()));
    field.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    field
}

fn popup(mtm: MainThreadMarker) -> Retained<NSPopUpButton> {
    let popup =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
    popup.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    popup
}

pub(crate) fn push_button(title: &str, frame: NSRect, mtm: MainThreadMarker) -> Retained<NSButton> {
    let button = NSButton::initWithFrame(NSButton::alloc(mtm), frame);
    button.setTitle(&NSString::from_str(title));
    button.setBezelStyle(NSBezelStyle::Push);
    button.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    button
}

fn scroll_view(mtm: MainThreadMarker) -> Retained<NSScrollView> {
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), NSRect::ZERO);
    scroll.setBorderType(NSBorderType::NoBorder);
    scroll.setDrawsBackground(false);
    scroll.setHasVerticalScroller(true);
    scroll.setHasHorizontalScroller(false);
    scroll.setAutohidesScrollers(true);
    scroll.setAutomaticallyAdjustsContentInsets(false);
    scroll.setContentInsets(NSEdgeInsets {
        top: 0.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    });
    scroll
}

fn clear_subviews(view: &NSView) {
    let subviews = view.subviews();
    let owned: Vec<_> = (0..subviews.count())
        .map(|index| subviews.objectAtIndex(index))
        .collect();
    drop(subviews);
    for subview in owned {
        subview.removeFromSuperview();
    }
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ShellIvars]
    #[name = "IkiDictionaryShell"]
    struct DictionaryShell;

    unsafe impl NSObjectProtocol for DictionaryShell {}

    unsafe impl NSWindowDelegate for DictionaryShell {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: &NSWindow) -> bool {
            sender.orderOut(None);
            false
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            self.layout(false);
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            self.refresh();
        }
    }

    unsafe impl NSControlTextEditingDelegate for DictionaryShell {
        #[unsafe(method(controlTextDidChange:))]
        #[allow(non_snake_case)]
        fn controlTextDidChange(&self, _obj: &NSNotification) {
            self.reload_list();
        }
    }

    unsafe impl NSTextFieldDelegate for DictionaryShell {}

    impl DictionaryShell {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            NSColor::windowBackgroundColor().setFill();
            NSBezierPath::fillRect(dirty);
            let y = self.ivars().separator_y.get();
            if y > 0.0 {
                NSColor::separatorColor().setFill();
                NSBezierPath::fillRect(NSRect::new(
                    NSPoint::new(0.0, y),
                    NSSize::new(self.bounds().size.width, 1.0),
                ));
            }
        }

        #[unsafe(method(sortChanged:))]
        fn sort_changed(&self, _sender: Option<&AnyObject>) {
            if self.ivars().suppress.get() {
                return;
            }
            let sort = match self.sort_popup().indexOfSelectedItem() {
                1 => Sort::Oldest,
                2 => Sort::StarredFirst,
                3 => Sort::Alphabetical,
                _ => Sort::Newest,
            };
            dictionary::set_sort(sort);
            self.reload_list();
        }

        #[unsafe(method(toggleCorrection:))]
        fn toggle_correction(&self, _sender: Option<&AnyObject>) {
            self.layout_children();
        }

        #[unsafe(method(submitWord:))]
        fn submit_word(&self, _sender: Option<&AnyObject>) {
            self.submit();
        }

        #[unsafe(method(cancelEdit:))]
        fn cancel_edit(&self, _sender: Option<&AnyObject>) {
            self.clear_form();
            self.layout_children();
        }

        #[unsafe(method(deleteWord:))]
        fn delete_word(&self, _sender: Option<&AnyObject>) {
            self.delete_editing();
        }

        #[unsafe(method(addLanguage:))]
        fn add_language(&self, _sender: Option<&AnyObject>) {
            let index = self.language_popup().indexOfSelectedItem();
            if index < 0 {
                return;
            }
            let code = {
                let addable = self.ivars().addable.borrow();
                addable.get(index as usize).cloned()
            };
            let Some(code) = code else {
                return;
            };
            if let Err(error) = languages::add(&code) {
                self.set_error(error.message());
                return;
            }
            self.set_error("");
            self.ivars().row_width.set(-1.0);
            self.layout_children();
        }

        #[unsafe(method(moveLanguageUp:))]
        fn move_language_up(&self, sender: Option<&NSButton>) {
            self.move_language(sender, true);
        }

        #[unsafe(method(moveLanguageDown:))]
        fn move_language_down(&self, sender: Option<&NSButton>) {
            self.move_language(sender, false);
        }

        #[unsafe(method(removeLanguage:))]
        fn remove_language(&self, sender: Option<&NSButton>) {
            let Some(sender) = sender else {
                return;
            };
            match languages::remove(sender.tag() as usize) {
                Ok(()) => {
                    self.set_error("");
                    self.ivars().row_width.set(-1.0);
                    self.layout_children();
                }
                Err(error) => self.set_error(error.message()),
            }
        }
    }
);

impl DictionaryShell {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ShellIvars {
            stats: OnceCell::new(),
            language_label: OnceCell::new(),
            language_note: OnceCell::new(),
            language_popup: OnceCell::new(),
            language_add: OnceCell::new(),
            language_scroll: OnceCell::new(),
            language_rows: OnceCell::new(),
            addable: RefCell::new(Vec::new()),
            shown_codes: RefCell::new(Vec::new()),
            row_width: Cell::new(-1.0),
            vocabulary_label: OnceCell::new(),
            search: OnceCell::new(),
            sort_popup: OnceCell::new(),
            word: OnceCell::new(),
            spelling: OnceCell::new(),
            correction: OnceCell::new(),
            submit_button: OnceCell::new(),
            cancel_button: OnceCell::new(),
            delete_button: OnceCell::new(),
            error: OnceCell::new(),
            scroll: OnceCell::new(),
            list: OnceCell::new(),
            editing: RefCell::new(None),
            suppress: Cell::new(false),
            separator_y: Cell::new(0.0),
        });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    fn stats(&self) -> &NSTextField {
        self.ivars().stats.get().expect("stats")
    }
    fn language_label(&self) -> &NSTextField {
        self.ivars().language_label.get().expect("language label")
    }
    fn language_note(&self) -> &NSTextField {
        self.ivars().language_note.get().expect("language note")
    }
    fn language_popup(&self) -> &NSPopUpButton {
        self.ivars().language_popup.get().expect("language popup")
    }
    fn language_add(&self) -> &NSButton {
        self.ivars().language_add.get().expect("language add")
    }
    fn language_scroll(&self) -> &NSScrollView {
        self.ivars().language_scroll.get().expect("language scroll")
    }
    fn language_rows(&self) -> &LanguageRows {
        self.ivars().language_rows.get().expect("language rows")
    }
    fn vocabulary_label(&self) -> &NSTextField {
        self.ivars()
            .vocabulary_label
            .get()
            .expect("vocabulary label")
    }
    fn search(&self) -> &NSTextField {
        self.ivars().search.get().expect("search")
    }
    fn sort_popup(&self) -> &NSPopUpButton {
        self.ivars().sort_popup.get().expect("sort")
    }
    fn word(&self) -> &NSTextField {
        self.ivars().word.get().expect("word")
    }
    fn spelling(&self) -> &NSTextField {
        self.ivars().spelling.get().expect("spelling")
    }
    fn correction(&self) -> &NSButton {
        self.ivars().correction.get().expect("correction")
    }
    fn submit_button(&self) -> &NSButton {
        self.ivars().submit_button.get().expect("submit")
    }
    fn cancel_button(&self) -> &NSButton {
        self.ivars().cancel_button.get().expect("cancel")
    }
    fn delete_button(&self) -> &NSButton {
        self.ivars().delete_button.get().expect("delete")
    }
    fn error(&self) -> &NSTextField {
        self.ivars().error.get().expect("error")
    }
    fn scroll(&self) -> &NSScrollView {
        self.ivars().scroll.get().expect("scroll")
    }
    fn list(&self) -> &DictionaryList {
        self.ivars().list.get().expect("list")
    }

    fn refresh(&self) {
        self.sync_sort_popup();
        self.layout_children();
    }

    fn sync_sort_popup(&self) {
        let index = match dictionary::sort() {
            Sort::Newest => 0,
            Sort::Oldest => 1,
            Sort::StarredFirst => 2,
            Sort::Alphabetical => 3,
        };
        self.ivars().suppress.set(true);
        self.sort_popup().selectItemAtIndex(index as NSInteger);
        self.ivars().suppress.set(false);
    }

    fn reload_list(&self) {
        let query = self.search().stringValue().to_string();
        let query_empty = query.trim().is_empty();
        let (total, entries) = dictionary::listing(&query);
        let line = if query_empty {
            count_line(total)
        } else {
            format!("{} of {}", entries.len(), count_line(total))
        };
        self.stats().setStringValue(&NSString::from_str(&line));
        let listed = entries
            .iter()
            .map(|entry| Listed {
                phrase: entry.phrase.clone(),
                label: entry.label(),
                starred: entry.starred,
            })
            .collect();
        self.list().set_listed(listed, query_empty);
        self.layout_list();
    }

    fn layout_list(&self) {
        let scroll = self.scroll();
        let list = self.list();
        let mut content_w = scroll.contentSize().width;
        if content_w < 1.0 {
            content_w = scroll.frame().size.width;
        }
        list.reflow(content_w, scroll.contentSize().height);
        let content_w2 = scroll.contentSize().width;
        if (content_w2 - content_w).abs() > 1.0 {
            list.reflow(content_w2, scroll.contentSize().height);
        }
    }

    fn layout_children(&self) {
        self.layout(true);
    }

    /// `reload` re-reads the language and vocabulary files. A resize only
    /// moves frames, so it reuses what is already shown.
    fn layout(&self, reload: bool) {
        if self.ivars().stats.get().is_none() {
            return;
        }
        let bounds = self.bounds();
        let width = bounds.size.width.max(0.0);
        let inner = (width - PAD * 2.0).max(0.0);
        let mut y = 16.0;
        self.stats().setFrame(rect(PAD, y, inner, 18.0));
        y += 26.0;
        self.language_label().setFrame(rect(PAD, y, inner, 18.0));
        y += 20.0;
        self.language_note().setFrame(rect(PAD, y, inner, 16.0));
        y += 24.0;

        let add_w = 72.0;
        let popup_w = (inner - add_w - 8.0).max(80.0);
        self.language_popup().setFrame(rect(PAD, y, popup_w, 26.0));
        self.language_add()
            .setFrame(rect(PAD + inner - add_w, y, add_w, 26.0));
        y += 34.0;

        let shown = self.ivars().shown_codes.borrow().clone();
        let codes = if reload || shown.is_empty() {
            languages::codes()
        } else {
            shown
        };
        let list_h = ((codes.len().max(1) as f64) * LANG_ROW_H).min(112.0);
        self.language_scroll().setFrame(rect(PAD, y, inner, list_h));
        let mut language_w = self.language_scroll().contentSize().width;
        if language_w < 1.0 {
            language_w = inner;
        }
        self.sync_languages(&codes, language_w);
        y += list_h + 14.0;
        self.ivars().separator_y.set(y);
        y += 12.0;

        self.vocabulary_label().setFrame(rect(PAD, y, inner, 18.0));
        y += 24.0;
        let sort_w = 168.0;
        let search_w = (inner - sort_w - 8.0).max(80.0);
        self.search().setFrame(rect(PAD, y, search_w, 24.0));
        self.sort_popup()
            .setFrame(rect(PAD + inner - sort_w, y, sort_w, 26.0));
        y += 34.0;

        let correcting = self.correction().state() == NSControlStateValueOn;
        self.apply_placeholders(correcting);
        if correcting {
            let half = ((inner - 16.0) / 2.0).max(40.0);
            self.word().setFrame(rect(PAD, y, half, 24.0));
            self.spelling().setHidden(false);
            self.spelling()
                .setFrame(rect(PAD + half + 16.0, y, half, 24.0));
        } else {
            self.word().setFrame(rect(PAD, y, inner, 24.0));
            self.spelling().setHidden(true);
        }
        y += 32.0;
        self.correction().setFrame(rect(PAD, y, inner, 22.0));
        y += 30.0;

        let editing = self.ivars().editing.borrow().is_some();
        let title = if editing { "Save changes" } else { "Add word" };
        self.submit_button().setTitle(&NSString::from_str(title));
        self.submit_button().setFrame(rect(PAD, y, 120.0, 26.0));
        self.cancel_button().setHidden(!editing);
        self.cancel_button()
            .setFrame(rect(PAD + 128.0, y, 84.0, 26.0));
        self.delete_button().setHidden(!editing);
        self.delete_button()
            .setFrame(rect(PAD + 220.0, y, 84.0, 26.0));
        y += 32.0;
        self.error().setFrame(rect(PAD, y, inner, 18.0));
        y += 24.0;

        let scroll_h = (bounds.size.height - y).max(0.0);
        self.scroll().setFrame(rect(0.0, y, width, scroll_h));
        if reload {
            self.reload_list();
        } else {
            self.layout_list();
        }
        self.setNeedsDisplay(true);
    }

    fn apply_placeholders(&self, correcting: bool) {
        if correcting {
            self.word()
                .setPlaceholderString(Some(&NSString::from_str("Misspelling")));
            self.spelling()
                .setPlaceholderString(Some(&NSString::from_str("Correct spelling")));
        } else {
            self.word()
                .setPlaceholderString(Some(&NSString::from_str("Add a new word")));
        }
    }

    fn sync_languages(&self, codes: &[String], width: f64) {
        let addable: Vec<String> = languages::catalog()
            .iter()
            .map(|(code, _)| (*code).to_string())
            .filter(|code| !codes.iter().any(|saved| saved == code))
            .collect();
        if *self.ivars().addable.borrow() != addable {
            *self.ivars().addable.borrow_mut() = addable.clone();
            let popup = self.language_popup();
            popup.removeAllItems();
            for code in &addable {
                popup.addItemWithTitle(&NSString::from_str(languages::name(code)));
            }
            let enabled = !addable.is_empty();
            popup.setEnabled(enabled);
            self.language_add().setEnabled(enabled);
        }
        self.rebuild_language_rows(codes, width);
    }

    fn rebuild_language_rows(&self, codes: &[String], width: f64) {
        let width = width.max(1.0);
        if *self.ivars().shown_codes.borrow() == codes
            && (self.ivars().row_width.get() - width).abs() < 0.5
        {
            return;
        }
        *self.ivars().shown_codes.borrow_mut() = codes.to_vec();
        self.ivars().row_width.set(width);
        let rows = self.language_rows();
        clear_subviews(rows);
        let mtm = MainThreadMarker::from(self);
        let last = codes.len().saturating_sub(1);
        for (index, code) in codes.iter().enumerate() {
            let y = index as f64 * LANG_ROW_H;
            let title = format!("{}. {}", index + 1, languages::name(code));
            let name = label(&title, mtm);
            name.setTextColor(Some(&NSColor::labelColor()));
            name.setFrame(rect(4.0, y + 4.0, (width - 196.0).max(40.0), 20.0));
            let up = small_button("Up", rect(width - 188.0, y + 2.0, 56.0, 24.0), mtm);
            let down = small_button("Down", rect(width - 128.0, y + 2.0, 64.0, 24.0), mtm);
            let remove = small_button("Remove", rect(width - 60.0, y + 2.0, 56.0, 24.0), mtm);
            up.setEnabled(index > 0);
            down.setEnabled(index < last);
            remove.setEnabled(codes.len() > 1);
            up.setTag(index as NSInteger);
            down.setTag(index as NSInteger);
            remove.setTag(index as NSInteger);
            unsafe {
                up.setTarget(Some(self));
                up.setAction(Some(sel!(moveLanguageUp:)));
                down.setTarget(Some(self));
                down.setAction(Some(sel!(moveLanguageDown:)));
                remove.setTarget(Some(self));
                remove.setAction(Some(sel!(removeLanguage:)));
            }
            add_subview(rows, &name);
            add_subview(rows, &up);
            add_subview(rows, &down);
            add_subview(rows, &remove);
        }
        let height = (codes.len().max(1) as f64) * LANG_ROW_H;
        rows.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(width, height)));
    }

    fn move_language(&self, sender: Option<&NSButton>, up: bool) {
        let Some(sender) = sender else {
            return;
        };
        let index = sender.tag() as usize;
        if up {
            languages::move_up(index);
        } else {
            languages::move_down(index);
        }
        self.ivars().row_width.set(-1.0);
        self.layout_children();
    }

    fn submit(&self) {
        let word = self.word().stringValue().to_string();
        let spelling = self.spelling().stringValue().to_string();
        let correcting = self.correction().state() == NSControlStateValueOn;
        let editing = self.ivars().editing.borrow().clone();
        let result = if let Some(original) = editing.as_deref() {
            let replacement = correcting.then_some(spelling.as_str());
            dictionary::update(original, &word, replacement)
        } else if correcting {
            dictionary::add_correction(&word, &spelling)
        } else {
            dictionary::add_word(&word)
        };
        match result {
            Ok(()) => {
                self.clear_form();
                self.layout_children();
            }
            Err(error) => self.set_error(error.message()),
        }
    }

    fn delete_editing(&self) {
        let Some(phrase) = self.ivars().editing.borrow().clone() else {
            return;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if !confirm_delete(&phrase, mtm) {
            return;
        }
        dictionary::remove(&phrase);
        self.clear_form();
        self.layout_children();
    }

    fn begin_edit(&self, phrase: &str) {
        let Some(entry) = dictionary::find(phrase) else {
            self.refresh();
            return;
        };
        *self.ivars().editing.borrow_mut() = Some(entry.phrase.clone());
        self.word()
            .setStringValue(&NSString::from_str(&entry.phrase));
        self.spelling().setStringValue(&NSString::from_str(
            entry.replacement.as_deref().unwrap_or(""),
        ));
        self.correction().setState(if entry.replacement.is_some() {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.set_error("");
        self.layout_children();
    }

    fn toggle_star(&self, phrase: &str) {
        dictionary::toggle_starred(phrase);
        self.reload_list();
    }

    fn clear_form(&self) {
        *self.ivars().editing.borrow_mut() = None;
        self.word().setStringValue(&NSString::from_str(""));
        self.spelling().setStringValue(&NSString::from_str(""));
        self.correction().setState(NSControlStateValueOff);
        self.set_error("");
    }

    fn set_error(&self, message: &str) {
        self.error().setStringValue(&NSString::from_str(message));
    }
}

fn confirm_delete(phrase: &str, mtm: MainThreadMarker) -> bool {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Delete from vocabulary?"));
    let info = format!("'{phrase}' will be deleted permanently.");
    alert.setInformativeText(&NSString::from_str(&info));
    alert.addButtonWithTitle(&NSString::from_str("Yes, delete it"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    alert.runModal() == NSAlertFirstButtonReturn
}

fn count_line(count: usize) -> String {
    if count == 1 {
        "1 word".to_string()
    } else {
        format!("{count} words")
    }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn small_button(title: &str, frame: NSRect, mtm: MainThreadMarker) -> Retained<NSButton> {
    let button = push_button(title, frame, mtm);
    button.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    button
}

fn add_subview(parent: &NSView, child: &NSView) {
    let _: () = unsafe { msg_send![parent, addSubview: child] };
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    #[name = "IkiLanguageRows"]
    struct LanguageRows;

    unsafe impl NSObjectProtocol for LanguageRows {}

    impl LanguageRows {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl LanguageRows {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ListIvars]
    #[name = "IkiDictionaryList"]
    struct DictionaryList;

    unsafe impl NSObjectProtocol for DictionaryList {}

    impl DictionaryList {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            self.ivars().press.set(Some(point));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let Some(start) = self.ivars().press.take() else {
                return;
            };
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            if (point.x - start.x).abs() > 6.0 || (point.y - start.y).abs() > 6.0 {
                return;
            }
            let hit = {
                let rows = self.ivars().rows.borrow();
                rows.iter().find_map(|row| {
                    if row.empty || point.y < row.y || point.y >= row.y + row.height {
                        return None;
                    }
                    Some((row.phrase.clone(), point.x < PAD + 28.0))
                })
            };
            let Some((phrase, star)) = hit else {
                return;
            };
            let Some(host) = self.host() else {
                return;
            };
            if star {
                host.toggle_star(&phrase);
            } else {
                host.begin_edit(&phrase);
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            NSColor::windowBackgroundColor().setFill();
            NSBezierPath::fillRect(dirty);
            let width = self.bounds().size.width;
            let body = NSFont::systemFontOfSize(13.0);
            let label = NSColor::labelColor();
            let secondary = NSColor::secondaryLabelColor();
            let body_attrs = text_attrs_with(&body, &label, NSLineBreakMode::ByTruncatingTail);
            let secondary_attrs =
                text_attrs_with(&body, &secondary, NSLineBreakMode::ByWordWrapping);
            let rows = self.ivars().rows.borrow();
            for row in rows.iter() {
                if row.y + row.height < dirty.origin.y || row.y > dirty.origin.y + dirty.size.height {
                    continue;
                }
                if row.empty {
                    draw_string(
                        &row.label,
                        rect(PAD, row.y + 8.0, (width - PAD * 2.0).max(1.0), row.height),
                        &secondary_attrs,
                    );
                    continue;
                }
                let star = if row.starred { "★" } else { "☆" };
                let star_attrs = if row.starred { &body_attrs } else { &secondary_attrs };
                draw_string(star, rect(PAD, row.y + 8.0, 22.0, 18.0), star_attrs);
                draw_string(
                    &row.label,
                    rect(PAD + 28.0, row.y + 8.0, (width - PAD * 2.0 - 28.0).max(1.0), 18.0),
                    &body_attrs,
                );
                NSColor::separatorColor().setFill();
                NSBezierPath::fillRect(rect(PAD, row.y + row.height - 1.0, (width - PAD * 2.0).max(0.0), 1.0));
            }
        }
    }
);

impl DictionaryList {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ListIvars {
            listed: RefCell::new(Vec::new()),
            rows: RefCell::new(Vec::new()),
            query_empty: Cell::new(true),
            press: Cell::new(None),
            width: Cell::new(-1.0),
            host: OnceCell::new(),
        });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    fn set_host(&self, host: &Retained<DictionaryShell>) {
        let _ = self.ivars().host.set(Weak::from_retained(host));
    }

    fn host(&self) -> Option<Retained<DictionaryShell>> {
        self.ivars().host.get()?.load()
    }

    fn set_listed(&self, listed: Vec<Listed>, query_empty: bool) {
        *self.ivars().listed.borrow_mut() = listed;
        self.ivars().query_empty.set(query_empty);
        self.ivars().width.set(-1.0);
    }

    fn reflow(&self, width: f64, min_height: f64) {
        let width = width.max(1.0);
        if (width - self.ivars().width.get()).abs() < 0.5 && self.ivars().width.get() > 0.0 {
            let needed = self.content_height().max(min_height);
            if (self.bounds().size.height - needed).abs() > 0.5 {
                self.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(width, needed)));
            }
            return;
        }
        let listed = self.ivars().listed.borrow();
        let mut rows = Vec::new();
        let mut y = 8.0;
        if listed.is_empty() {
            let message = if self.ivars().query_empty.get() {
                "Words you add will be recognized when you dictate."
            } else {
                "No words match that search."
            };
            rows.push(Row {
                phrase: String::new(),
                label: message.to_string(),
                starred: false,
                y,
                height: 48.0,
                empty: true,
            });
            y += 48.0;
        } else {
            for entry in listed.iter() {
                rows.push(Row {
                    phrase: entry.phrase.clone(),
                    label: entry.label.clone(),
                    starred: entry.starred,
                    y,
                    height: ROW_H,
                    empty: false,
                });
                y += ROW_H;
            }
        }
        y += 12.0;
        drop(listed);
        *self.ivars().rows.borrow_mut() = rows;
        self.ivars().width.set(width);
        let height = y.max(min_height);
        self.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(width, height)));
        self.setNeedsDisplay(true);
    }

    fn content_height(&self) -> f64 {
        self.ivars()
            .rows
            .borrow()
            .last()
            .map(|row| row.y + row.height + 12.0)
            .unwrap_or(16.0)
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IkiDictionaryItem"]
    struct DictionaryItem;

    unsafe impl NSObjectProtocol for DictionaryItem {}

    impl DictionaryItem {
        #[unsafe(method(showDictionary:))]
        fn show_dictionary(&self, _sender: Option<&AnyObject>) {
            if let Some(mtm) = MainThreadMarker::new() {
                show(mtm);
            }
        }
    }
);

fn menu_target(mtm: MainThreadMarker) -> Retained<DictionaryItem> {
    let this = DictionaryItem::alloc(mtm);
    unsafe { msg_send![this, init] }
}

pub fn add_menu_item(menu: &NSMenu, mtm: MainThreadMarker) {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Dictionary"),
            Some(sel!(showDictionary:)),
            &NSString::from_str(""),
        )
    };
    let target = menu_target(mtm);
    unsafe { item.setTarget(Some(&*target)) };
    menu.addItem(&item);
    std::mem::forget(target);
}
