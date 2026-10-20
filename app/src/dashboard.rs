//! Transcript window. Open with `show` on the main thread.

use std::cell::{Cell, OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBezierPath, NSBorderType, NSColor, NSEvent, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSLineBreakMode, NSMenu, NSMenuItem,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSPasteboard, NSPasteboardTypeString,
    NSResponder, NSScreen, NSScrollView, NSStringDrawingOptions, NSStringNSExtendedStringDrawing,
    NSTextField, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSAttributedStringKey, NSCalendar, NSCalendarUnit, NSDate, NSDateFormatter,
    NSDateFormatterStyle, NSDictionary, NSEdgeInsets, NSNotification, NSPoint, NSPointInRect,
    NSRect, NSSize, NSString,
};

use crate::history::{self, Entry};

const WINDOW_W: f64 = 720.0;
const WINDOW_H: f64 = 640.0;
const PAD_X: f64 = 20.0;
const STATS_TOP: f64 = 16.0;
const STATS_H: f64 = 20.0;
const SCROLL_TOP: f64 = 46.0;
const TIME_COL: f64 = 88.0;
const COL_GAP: f64 = 14.0;
const ROW_PAD_Y: f64 = 11.0;

struct ShellIvars {
    stats: OnceCell<Retained<NSTextField>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    list: OnceCell<Retained<TranscriptList>>,
}

struct ListIvars {
    items: RefCell<Vec<Item>>,
    blocks: RefCell<Vec<Block>>,
    stats: OnceCell<Retained<NSTextField>>,
    press: Cell<Option<NSPoint>>,
    width: Cell<f64>,
}

enum Item {
    Header(String),
    Row { time: String, text: String },
    Empty(String),
}

struct Block {
    y: f64,
    height: f64,
    kind: BlockKind,
}

enum BlockKind {
    Header(String),
    Row {
        time: String,
        text: String,
        rule: bool,
    },
    Empty(String),
}

struct Dashboard {
    window: Retained<NSWindow>,
    shell: Retained<DashboardShell>,
}

std::thread_local! {
    static OPEN: RefCell<Option<Dashboard>> = RefCell::new(None);
}

pub fn show(mtm: MainThreadMarker) {
    OPEN.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(Dashboard::create(mtm));
        }
        let dashboard = slot.as_ref().unwrap();
        dashboard.shell.reload();
        let app = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        dashboard.window.makeKeyAndOrderFront(None);
    });
}

impl Dashboard {
    fn create(mtm: MainThreadMarker) -> Self {
        let shell = DashboardShell::new(mtm);
        let stats = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        stats.setTextColor(Some(&NSColor::secondaryLabelColor()));
        stats.setDrawsBackground(false);
        stats.setBordered(false);
        stats.setBezeled(false);
        stats.setEditable(false);
        stats.setSelectable(false);
        stats.setMaximumNumberOfLines(1);
        let font = NSFont::systemFontOfSize(13.0);
        let _: () =
            unsafe { msg_send![&*stats, setLineBreakMode: NSLineBreakMode::ByTruncatingTail] };
        let _: () = unsafe { msg_send![&*stats, setFont: &*font] };

        let list = TranscriptList::new(mtm);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), NSRect::ZERO);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(true);
        scroll.setBackgroundColor(&NSColor::windowBackgroundColor());
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
        scroll.setDocumentView(Some(&*list));

        let _: () = unsafe { msg_send![&*shell, addSubview: &*stats] };
        let _: () = unsafe { msg_send![&*shell, addSubview: &*scroll] };
        let _ = shell.ivars().stats.set(stats.clone());
        let _ = shell.ivars().scroll.set(scroll);
        let _ = shell.ivars().list.set(list.clone());
        let _ = list.ivars().stats.set(stats);

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
        window.setTitle(&NSString::from_str("Iki"));
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        window.setContentMinSize(NSSize::new(420.0, 320.0));
        window.setContentView(Some(&*shell));
        window.setDelegate(Some(ProtocolObject::from_ref(&*shell)));
        center_on_mouse_screen(&window, mtm);
        Self { window, shell }
    }
}

pub(crate) fn center_on_mouse_screen(window: &NSWindow, mtm: MainThreadMarker) {
    let frame = window.frame();
    let Some(screen) = screen_under_mouse(mtm) else {
        return;
    };
    let visible = screen.visibleFrame();
    let x = if frame.size.width <= visible.size.width {
        visible.origin.x + (visible.size.width - frame.size.width) / 2.0
    } else {
        visible.origin.x
    };
    let y = if frame.size.height <= visible.size.height {
        visible.origin.y + (visible.size.height - frame.size.height) / 2.0
    } else {
        visible.origin.y
    };
    window.setFrame_display(NSRect::new(NSPoint::new(x, y), frame.size), false);
}

fn screen_under_mouse(mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    let point = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    for index in 0..screens.count() {
        let screen = screens.objectAtIndex(index);
        if NSPointInRect(point, screen.frame()) {
            return Some(screen);
        }
    }
    NSScreen::mainScreen(mtm).or_else(|| {
        if screens.count() == 0 {
            None
        } else {
            Some(screens.objectAtIndex(0))
        }
    })
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ShellIvars]
    #[name = "IkiDashboardShell"]
    struct DashboardShell;

    unsafe impl NSObjectProtocol for DashboardShell {}

    unsafe impl NSWindowDelegate for DashboardShell {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: &NSWindow) -> bool {
            sender.orderOut(None);
            false
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            self.layout_children();
        }
    }

    impl DashboardShell {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            NSColor::windowBackgroundColor().setFill();
            NSBezierPath::fillRect(dirty);
            let width = self.bounds().size.width;
            NSColor::separatorColor().setFill();
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(0.0, SCROLL_TOP - 1.0),
                NSSize::new(width, 1.0),
            ));
        }
    }
);

impl DashboardShell {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ShellIvars {
            stats: OnceCell::new(),
            scroll: OnceCell::new(),
            list: OnceCell::new(),
        });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    fn reload(&self) {
        let mut entries = history::load();
        entries.sort_by(|left, right| left.at_ms.cmp(&right.at_ms));
        let (count, words) = history::stats(&entries);
        if let Some(stats) = self.ivars().stats.get() {
            let line = format!("{count} transcripts · {words} words");
            let value = NSString::from_str(&line);
            let _: () = unsafe { msg_send![&*stats, setStringValue: &*value] };
        }
        if let Some(list) = self.ivars().list.get() {
            list.set_items(items_from(&entries));
        }
        self.layout_children();
        if let Some(list) = self.ivars().list.get() {
            list.scrollPoint(NSPoint::ZERO);
        }
    }

    fn layout_children(&self) {
        let Some(stats) = self.ivars().stats.get() else {
            return;
        };
        let Some(scroll) = self.ivars().scroll.get() else {
            return;
        };
        let Some(list) = self.ivars().list.get() else {
            return;
        };
        let bounds = self.bounds();
        let width = bounds.size.width.max(0.0);
        stats.setFrame(NSRect::new(
            NSPoint::new(PAD_X, STATS_TOP),
            NSSize::new((width - PAD_X * 2.0).max(0.0), STATS_H),
        ));
        let scroll_h = (bounds.size.height - SCROLL_TOP).max(0.0);
        scroll.setFrame(NSRect::new(
            NSPoint::new(0.0, SCROLL_TOP),
            NSSize::new(width, scroll_h),
        ));
        let mut content_w = scroll.contentSize().width;
        if content_w < 1.0 {
            content_w = width;
        }
        let visible_h = scroll.contentSize().height;
        list.reflow(content_w, visible_h);
        let content_w2 = scroll.contentSize().width;
        if (content_w2 - content_w).abs() > 1.0 {
            list.reflow(content_w2, scroll.contentSize().height);
        }
        self.setNeedsDisplay(true);
    }
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ListIvars]
    #[name = "IkiTranscriptList"]
    struct TranscriptList;

    unsafe impl NSObjectProtocol for TranscriptList {}

    impl TranscriptList {
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
            self.copy_at(point);
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            let width = self.bounds().size.width;
            NSColor::windowBackgroundColor().setFill();
            NSBezierPath::fillRect(dirty);
            let body_font = NSFont::systemFontOfSize(13.0);
            let time_font = NSFont::systemFontOfSize(12.0);
            let header_font = NSFont::boldSystemFontOfSize(12.0);
            let label = NSColor::labelColor();
            let secondary = NSColor::secondaryLabelColor();
            let body_attrs = text_attrs(&body_font, &label);
            let time_attrs = text_attrs(&time_font, &secondary);
            let header_attrs = text_attrs(&header_font, &secondary);
            let blocks = self.ivars().blocks.borrow();
            for block in blocks.iter() {
                if block.y + block.height < dirty.origin.y
                    || block.y > dirty.origin.y + dirty.size.height
                {
                    continue;
                }
                paint_block(
                    block,
                    width,
                    &body_attrs,
                    &time_attrs,
                    &header_attrs,
                );
            }
        }
    }
);

impl TranscriptList {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ListIvars {
            items: RefCell::new(Vec::new()),
            blocks: RefCell::new(Vec::new()),
            stats: OnceCell::new(),
            press: Cell::new(None),
            width: Cell::new(-1.0),
        });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    fn set_items(&self, items: Vec<Item>) {
        *self.ivars().items.borrow_mut() = items;
        self.ivars().width.set(-1.0);
    }

    fn reflow(&self, width: f64, min_height: f64) {
        let width = width.max(1.0);
        if (width - self.ivars().width.get()).abs() < 0.5 && self.ivars().width.get() > 0.0 {
            let height = self.bounds().size.height;
            let needed = self.content_height().max(min_height);
            if (height - needed).abs() > 0.5 {
                self.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(width, needed)));
            }
            return;
        }
        let body_font = NSFont::systemFontOfSize(13.0);
        let time_font = NSFont::systemFontOfSize(12.0);
        let label = NSColor::labelColor();
        let secondary = NSColor::secondaryLabelColor();
        let body_attrs = text_attrs(&body_font, &label);
        let time_attrs = text_attrs(&time_font, &secondary);
        let text_x = PAD_X + TIME_COL + COL_GAP;
        let text_w = (width - text_x - PAD_X).max(32.0);
        let items = self.ivars().items.borrow();
        let mut blocks = Vec::with_capacity(items.len());
        let mut y = 12.0;
        let mut index = 0;
        while index < items.len() {
            match &items[index] {
                Item::Empty(message) => {
                    let height = measure(message, text_w, &body_attrs).max(18.0) + 24.0;
                    blocks.push(Block {
                        y,
                        height,
                        kind: BlockKind::Empty(message.clone()),
                    });
                    y += height;
                    index += 1;
                }
                Item::Header(title) => {
                    blocks.push(Block {
                        y,
                        height: 32.0,
                        kind: BlockKind::Header(title.clone()),
                    });
                    y += 32.0;
                    index += 1;
                }
                Item::Row { .. } => {
                    let row_start = index;
                    while index < items.len() && matches!(items[index], Item::Row { .. }) {
                        index += 1;
                    }
                    for (offset, item) in items[row_start..index].iter().enumerate() {
                        let Item::Row { time, text } = item else {
                            continue;
                        };
                        let text_h = measure(text, text_w, &body_attrs).max(16.0);
                        let time_h = measure(time, TIME_COL, &time_attrs).max(16.0);
                        let height = text_h.max(time_h) + ROW_PAD_Y * 2.0;
                        let last = offset + 1 == index - row_start;
                        blocks.push(Block {
                            y,
                            height,
                            kind: BlockKind::Row {
                                time: time.clone(),
                                text: text.clone(),
                                rule: !last,
                            },
                        });
                        y += height;
                    }
                }
            }
        }
        y += 16.0;
        drop(items);
        *self.ivars().blocks.borrow_mut() = blocks;
        self.ivars().width.set(width);
        let height = y.max(min_height);
        self.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(width, height)));
        self.setNeedsDisplay(true);
    }

    fn content_height(&self) -> f64 {
        let blocks = self.ivars().blocks.borrow();
        blocks
            .last()
            .map(|block| block.y + block.height + 16.0)
            .unwrap_or(16.0)
    }

    fn copy_at(&self, point: NSPoint) {
        let text = {
            let blocks = self.ivars().blocks.borrow();
            let Some(block) = blocks
                .iter()
                .find(|block| point.y >= block.y && point.y < block.y + block.height)
            else {
                return;
            };
            match &block.kind {
                BlockKind::Row { text, .. } => text.clone(),
                _ => return,
            }
        };
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        let copied = board.setString_forType(&NSString::from_str(&text), unsafe {
            NSPasteboardTypeString
        });
        if copied {
            if let Some(stats) = self.ivars().stats.get() {
                let value = NSString::from_str("Copied");
                let _: () = unsafe { msg_send![&*stats, setStringValue: &*value] };
            }
        }
    }
}

fn paint_block(
    block: &Block,
    width: f64,
    body_attrs: &NSDictionary<NSAttributedStringKey, AnyObject>,
    time_attrs: &NSDictionary<NSAttributedStringKey, AnyObject>,
    header_attrs: &NSDictionary<NSAttributedStringKey, AnyObject>,
) {
    match &block.kind {
        BlockKind::Header(title) => {
            draw_string(
                title,
                NSRect::new(
                    NSPoint::new(PAD_X, block.y + 10.0),
                    NSSize::new((width - PAD_X * 2.0).max(1.0), 18.0),
                ),
                header_attrs,
            );
        }
        BlockKind::Empty(message) => {
            draw_string(
                message,
                NSRect::new(
                    NSPoint::new(PAD_X, block.y + 12.0),
                    NSSize::new((width - PAD_X * 2.0).max(1.0), block.height),
                ),
                time_attrs,
            );
        }
        BlockKind::Row { time, text, rule } => {
            let top = block.y + ROW_PAD_Y;
            let text_h = (block.height - ROW_PAD_Y * 2.0).max(1.0);
            draw_string(
                time,
                NSRect::new(NSPoint::new(PAD_X, top), NSSize::new(TIME_COL, text_h)),
                time_attrs,
            );
            let text_x = PAD_X + TIME_COL + COL_GAP;
            draw_string(
                text,
                NSRect::new(
                    NSPoint::new(text_x, top),
                    NSSize::new((width - text_x - PAD_X).max(1.0), text_h),
                ),
                body_attrs,
            );
            if *rule {
                NSColor::separatorColor().setFill();
                NSBezierPath::fillRect(NSRect::new(
                    NSPoint::new(PAD_X, block.y + block.height - 1.0),
                    NSSize::new((width - PAD_X * 2.0).max(0.0), 1.0),
                ));
            }
        }
    }
}

fn draw_string(text: &str, rect: NSRect, attrs: &NSDictionary<NSAttributedStringKey, AnyObject>) {
    let string = NSString::from_str(text);
    unsafe {
        string.drawWithRect_options_attributes_context(
            rect,
            NSStringDrawingOptions::UsesLineFragmentOrigin
                | NSStringDrawingOptions::UsesFontLeading,
            Some(attrs),
            None,
        );
    }
}

fn measure(text: &str, width: f64, attrs: &NSDictionary<NSAttributedStringKey, AnyObject>) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let string = NSString::from_str(text);
    let rect = unsafe {
        string.boundingRectWithSize_options_attributes_context(
            NSSize::new(width.max(1.0), 100_000.0),
            NSStringDrawingOptions::UsesLineFragmentOrigin
                | NSStringDrawingOptions::UsesFontLeading,
            Some(attrs),
            None,
        )
    };
    (rect.size.height + 1.0).ceil()
}

fn text_attrs(
    font: &NSFont,
    color: &NSColor,
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    let font_key = unsafe { NSFontAttributeName };
    let color_key = unsafe { NSForegroundColorAttributeName };
    let style_key = unsafe { NSParagraphStyleAttributeName };
    let font_obj: &AnyObject = font;
    let color_obj: &AnyObject = color;
    let style_obj: &AnyObject = &*paragraph;
    NSDictionary::from_slices(
        &[font_key, color_key, style_key],
        &[font_obj, color_obj, style_obj],
    )
}

fn items_from(entries: &[Entry]) -> Vec<Item> {
    if entries.is_empty() {
        return vec![Item::Empty(
            "Transcripts you dictate will show up here.".to_string(),
        )];
    }
    let calendar = NSCalendar::currentCalendar();
    let headers = NSDateFormatter::new();
    headers.setDateStyle(NSDateFormatterStyle::LongStyle);
    headers.setTimeStyle(NSDateFormatterStyle::NoStyle);
    headers.setDoesRelativeDateFormatting(true);
    let times = NSDateFormatter::new();
    times.setDateStyle(NSDateFormatterStyle::NoStyle);
    times.setTimeStyle(NSDateFormatterStyle::ShortStyle);

    let mut items = Vec::new();
    let mut index = entries.len();
    while index > 0 {
        let date = date_from_ms(entries[index - 1].at_ms);
        let day = day_key(&calendar, &date);
        let mut start = index - 1;
        while start > 0 {
            let previous = date_from_ms(entries[start - 1].at_ms);
            if day_key(&calendar, &previous) != day {
                break;
            }
            start -= 1;
        }
        items.push(Item::Header(header_title(&headers, &calendar, &date)));
        for entry in entries[start..index].iter().rev() {
            let when = date_from_ms(entry.at_ms);
            items.push(Item::Row {
                time: times.stringFromDate(&when).to_string(),
                text: entry.text.clone(),
            });
        }
        index = start;
    }
    items
}

fn date_from_ms(at_ms: u64) -> Retained<NSDate> {
    NSDate::dateWithTimeIntervalSince1970(at_ms as f64 / 1000.0)
}

fn day_key(calendar: &NSCalendar, date: &NSDate) -> (i64, i64, i64) {
    let units = NSCalendarUnit::Year | NSCalendarUnit::Month | NSCalendarUnit::Day;
    let parts = calendar.components_fromDate(units, date);
    (
        parts.year() as i64,
        parts.month() as i64,
        parts.day() as i64,
    )
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IkiTranscriptsItem"]
    struct TranscriptsItem;

    unsafe impl NSObjectProtocol for TranscriptsItem {}

    impl TranscriptsItem {
        #[unsafe(method(showTranscripts:))]
        fn show_transcripts(&self, _sender: Option<&AnyObject>) {
            if let Some(mtm) = MainThreadMarker::new() {
                show(mtm);
            }
        }
    }
);

fn menu_target(mtm: MainThreadMarker) -> Retained<TranscriptsItem> {
    let this = TranscriptsItem::alloc(mtm);
    unsafe { msg_send![this, init] }
}

pub fn add_menu_item(menu: &NSMenu, mtm: MainThreadMarker) {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Transcripts"),
            Some(sel!(showTranscripts:)),
            &NSString::from_str(""),
        )
    };
    let target = menu_target(mtm);
    unsafe { item.setTarget(Some(&*target)) };
    menu.addItem(&item);
    std::mem::forget(target);
}

fn header_title(formatter: &NSDateFormatter, calendar: &NSCalendar, date: &NSDate) -> String {
    if calendar.isDateInToday(date) {
        return "Today".to_string();
    }
    if calendar.isDateInYesterday(date) {
        return "Yesterday".to_string();
    }
    let title = formatter.stringFromDate(date).to_string();
    if title.is_empty() {
        let (year, month, day) = day_key(calendar, date);
        format!("{month:02}/{day:02}/{year}")
    } else {
        title
    }
}
