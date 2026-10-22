//! Floating recording pill for Iki.
//!
//! An error is a notification card: a black card, a red
//! circle-exclamation, a short title, and one explanation. The recording
//! and processing pills stay 30 px tall.

use std::cell::{Cell, OnceCell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryGetValue, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFStringRef;
use core_graphics::geometry::CGRect;
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer,
    kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID,
};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSBackingStoreType,
    NSBezierPath, NSColor, NSEvent, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
    NSHapticFeedbackPerformer, NSLineCapStyle, NSPanel, NSResponder, NSScreen, NSStatusWindowLevel,
    NSStringDrawing, NSView, NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSAttributedStringKey, NSDictionary, NSPoint, NSPointInRect, NSRect, NSRunLoop,
    NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};

/// Bottom-dock hold-to-talk pill. Recording is 73×30, processing grows to 98×30.
/// The hidden resting sliver is 40×8; the open transition starts there.
const PILL_H: f64 = 30.0;
const WIDTH_RECORDING: f64 = 73.0;
const WIDTH_PROCESSING: f64 = 98.0;
const REST_W: f64 = 40.0;
const REST_H: f64 = 8.0;
const MARGIN: f64 = 8.0;
/// A drag snaps the pill onto the screen's centre line within this distance.
const SNAP_DISTANCE: f64 = 12.0;
/// Notification card: min 340, max 440, padding 20, radius 24.
const NOTICE_MIN_W: f64 = 340.0;
const NOTICE_MAX_W: f64 = 440.0;
const NOTICE_PAD: f64 = 20.0;
const NOTICE_RADIUS: f64 = 24.0;
const NOTICE_GAP: f64 = 12.0;
const NOTICE_ICON: f64 = 18.0;
const NOTICE_ICON_GAP: f64 = 8.0;
const NOTICE_FONT: f64 = 15.0;
const NOTICE_LINE: f64 = 20.0;
const FRAME_DT: f64 = 1.0 / 60.0;
/// `transition: all .1s cubic-bezier(0.05, 0.6, 0.4, 0.95)`.
const MORPH_SECS: f64 = 0.1;

const BAR_COUNT: usize = 10;
const BAR_W: f64 = 2.0;
const BAR_GAP: f64 = 2.0;
const BAR_BASE: f64 = 2.0;
const WAVE_PAD_X: f64 = 4.0;
const CLUSTER_GAP: f64 = 6.0;
const SPINNER_BOX: f64 = 16.0;
const SPINNER_RADIUS: f64 = 6.0;
/// `animation-delay: 0.1 * m[i]` for the ten-bar waveform.
const BAR_DELAYS: [f64; BAR_COUNT] = [0.0, 0.1, 0.2, 0.3, 0.4, -0.5, -0.4, -0.3, -0.2, -0.1];

#[derive(Clone, Debug)]
pub enum BarPhase {
    Hidden,
    Recording { level: f32 },
    Processing,
    Error { title: String, body: String },
}

#[derive(Debug)]
pub enum BarError {
    NotMainThread,
    NoScreen,
}

impl std::fmt::Display for BarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMainThread => {
                write!(f, "BarController::install must run on the main thread")
            }
            Self::NoScreen => write!(f, "no screen available to place the pill"),
        }
    }
}

impl std::error::Error for BarError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Hidden,
    Recording,
    Processing,
    Error,
}

#[derive(Clone, Copy)]
struct Drag {
    start_mouse: NSPoint,
    start_origin: NSPoint,
    snapped: bool,
}

struct ViewIvars {
    slot: Arc<Mutex<BarPhase>>,
    mode: Cell<Mode>,
    raw_title: RefCell<String>,
    raw_body: RefCell<String>,
    title_lines: RefCell<Vec<String>>,
    body_lines: RefCell<Vec<String>>,
    anim_origin: Cell<Option<Instant>>,
    last_tick: Cell<Option<Instant>>,
    shown: Cell<bool>,
    drag: Cell<Option<Drag>>,
    timer: OnceCell<Retained<NSTimer>>,
    /// Adaptive dBFS floor. Starts at 0 so the first real sample sets it.
    floor_db: Cell<f64>,
    /// Smoothed 0..=1 voice amount, before the ×5 audio scale.
    voice: Cell<f64>,
    cur_w: Cell<f64>,
    cur_h: Cell<f64>,
    from_w: Cell<f64>,
    from_h: Cell<f64>,
    to_w: Cell<f64>,
    to_h: Cell<f64>,
    /// 0 at the start of a size change, 1 when the pill has arrived.
    morph: Cell<f64>,
}

/// Handle to the overlay. Cheap to clone; the window stays on the main thread.
#[derive(Clone)]
pub struct BarController {
    slot: Arc<Mutex<BarPhase>>,
}

impl BarController {
    /// Call on the main thread. NSApplication must already exist (sharedApplication is fine)
    /// but this function must NOT call `NSApplication::run`.
    pub fn install() -> Result<Self, BarError> {
        let mtm = MainThreadMarker::new().ok_or(BarError::NotMainThread)?;
        let _app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let screen = focused_screen(mtm)?;
        let origin = origin_for_screen(mtm, &screen, (WIDTH_RECORDING, PILL_H));
        let slot = Arc::new(Mutex::new(BarPhase::Hidden));
        let view = BarView::new(mtm, Arc::clone(&slot));
        let panel = BarPanel::create(
            mtm,
            NSRect::new(origin, NSSize::new(WIDTH_RECORDING, PILL_H)),
        );
        configure_panel(&panel);
        panel.setContentView(Some(&*view));
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                FRAME_DT,
                &*view,
                sel!(tick:),
                None,
                true,
            )
        };
        // Common modes keep the poll running while the pill is dragged.
        let modes = unsafe { NSRunLoopCommonModes };
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, modes) };
        let _ = view.ivars().timer.set(timer);
        panel.orderOut(None);
        // The run loop retains the timer, the timer retains the view, and the
        // panel retains the view. Leak the panel so the overlay outlives `install`.
        std::mem::forget(panel);
        Ok(Self { slot })
    }

    /// Thread-safe. Coalesces to the latest phase. The view applies it on the main thread.
    pub fn set_phase(&self, phase: BarPhase) {
        *self.slot.lock().unwrap_or_else(|err| err.into_inner()) = phase;
    }
}

define_class!(
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IkiBarPanel"]
    struct BarPanel;

    unsafe impl NSObjectProtocol for BarPanel {}

    impl BarPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

impl BarPanel {
    fn create(mtm: MainThreadMarker, content: NSRect) -> Retained<Self> {
        let allocated = Self::alloc(mtm);
        // `defer` is a Rust keyword, so the safe NSPanel initializer is used.
        // Allocated<T> is pointer-sized; the instance's class is still this subclass.
        let allocated: Allocated<NSPanel> = unsafe { std::mem::transmute(allocated) };
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            allocated,
            content,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        match panel.downcast() {
            Ok(panel) => panel,
            Err(_) => panic!("iki bar panel class was not IkiBarPanel"),
        }
    }
}

fn configure_panel(panel: &BarPanel) {
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setHidesOnDeactivate(false);
    panel.setIgnoresMouseEvents(false);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
    float_over_everything(panel);
}

/// Puts the window above every app window, on every Space, including the
/// Space of a full-screen app. macOS does not always attach a hidden
/// all-Spaces window to a full-screen Space created while it was ordered out,
/// so this is re-applied before each show rather than set once.
fn float_over_everything(window: &NSWindow) {
    window.setLevel(NSStatusWindowLevel);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    #[name = "IkiBarView"]
    struct BarView;

    unsafe impl NSObjectProtocol for BarView {}

    impl BarView {
        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            false
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(needsPanelToBecomeKey))]
        fn needs_panel_to_become_key(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.paint();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let Some(window) = self.window() else {
                return;
            };
            self.ivars().drag.set(Some(Drag {
                start_mouse: NSEvent::mouseLocation(),
                start_origin: window.frame().origin,
                snapped: false,
            }));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            let Some(drag) = self.ivars().drag.get() else {
                return;
            };
            let Some(window) = self.window() else {
                return;
            };
            let now = NSEvent::mouseLocation();
            let mut origin = NSPoint::new(
                drag.start_origin.x + (now.x - drag.start_mouse.x),
                drag.start_origin.y + (now.y - drag.start_mouse.y),
            );
            let mut snapped = false;
            if let Some(screen) = self.clamp_screen(&window) {
                let visible = screen_rect(screen.visibleFrame());
                (origin.x, snapped) = snap_to_center(origin.x, window.frame().size.width, visible);
            }
            if snapped && !drag.snapped {
                NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                    NSHapticFeedbackPattern::Alignment,
                    NSHapticFeedbackPerformanceTime::Now,
                );
            }
            self.ivars().drag.set(Some(Drag { snapped, ..drag }));
            window.setFrameOrigin(origin);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if self.ivars().drag.take().is_none() {
                return;
            }
            // The pill may have morphed while the drag held its frame still.
            // Size it first, so the clamp and the saved origin see the final frame.
            self.apply_size(self.ivars().cur_w.get(), self.ivars().cur_h.get());
            if let Some(window) = self.window() {
                self.keep_on_screen(&window);
                save_position(window.frame());
            }
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.apply_phase();
        }
    }
);

impl BarView {
    fn new(mtm: MainThreadMarker, slot: Arc<Mutex<BarPhase>>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            slot,
            mode: Cell::new(Mode::Hidden),
            raw_title: RefCell::new(String::new()),
            raw_body: RefCell::new(String::new()),
            title_lines: RefCell::new(Vec::new()),
            body_lines: RefCell::new(Vec::new()),
            anim_origin: Cell::new(None),
            last_tick: Cell::new(None),
            shown: Cell::new(false),
            drag: Cell::new(None),
            timer: OnceCell::new(),
            floor_db: Cell::new(0.0),
            voice: Cell::new(0.0),
            cur_w: Cell::new(WIDTH_RECORDING),
            cur_h: Cell::new(PILL_H),
            from_w: Cell::new(WIDTH_RECORDING),
            from_h: Cell::new(PILL_H),
            to_w: Cell::new(WIDTH_RECORDING),
            to_h: Cell::new(PILL_H),
            morph: Cell::new(1.0),
        });
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH_RECORDING, PILL_H));
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn clear_notice(&self) {
        self.ivars().raw_title.borrow_mut().clear();
        self.ivars().raw_body.borrow_mut().clear();
        self.ivars().title_lines.borrow_mut().clear();
        self.ivars().body_lines.borrow_mut().clear();
    }

    fn latest_phase(&self) -> BarPhase {
        self.ivars()
            .slot
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn apply_phase(&self) {
        let dt = self.tick_dt();
        match self.latest_phase() {
            BarPhase::Hidden => {
                if self.ivars().mode.get() != Mode::Hidden {
                    self.ivars().mode.set(Mode::Hidden);
                    self.clear_notice();
                    if let Some(window) = self.window() {
                        window.orderOut(None);
                    }
                    self.ivars().shown.set(false);
                }
            }
            BarPhase::Recording { level } => {
                if self.ivars().mode.get() != Mode::Recording {
                    let from_hidden = !self.ivars().shown.get();
                    self.ivars().mode.set(Mode::Recording);
                    self.ivars().floor_db.set(0.0);
                    self.ivars().voice.set(0.0);
                    self.ivars().anim_origin.set(Some(Instant::now()));
                    self.clear_notice();
                    self.begin_size(WIDTH_RECORDING, PILL_H, from_hidden);
                    self.present();
                }
                self.ingest_level(level, dt);
                self.step_size(dt);
                self.setNeedsDisplay(true);
            }
            BarPhase::Processing => {
                if self.ivars().mode.get() != Mode::Processing {
                    let from_hidden = !self.ivars().shown.get();
                    self.ivars().mode.set(Mode::Processing);
                    self.ivars().anim_origin.set(Some(Instant::now()));
                    self.clear_notice();
                    self.begin_size(WIDTH_PROCESSING, PILL_H, from_hidden);
                    self.present();
                }
                self.step_size(dt);
                self.setNeedsDisplay(true);
            }
            BarPhase::Error { title, body } => {
                let changed = self.ivars().mode.get() != Mode::Error
                    || *self.ivars().raw_title.borrow() != title
                    || *self.ivars().raw_body.borrow() != body;
                if changed {
                    let font = NSFont::systemFontOfSize(NOTICE_FONT);
                    let laid = layout_notice(&title, &body, |text| measure(text, &font));
                    let from_hidden = !self.ivars().shown.get();
                    self.ivars().mode.set(Mode::Error);
                    *self.ivars().raw_title.borrow_mut() = title;
                    *self.ivars().raw_body.borrow_mut() = body;
                    *self.ivars().title_lines.borrow_mut() = laid.title_lines;
                    *self.ivars().body_lines.borrow_mut() = laid.body_lines;
                    self.begin_size(laid.width, laid.height, from_hidden);
                    self.present();
                }
                self.step_size(dt);
                self.setNeedsDisplay(true);
            }
        }
    }

    fn present(&self) {
        let Some(window) = self.window() else {
            return;
        };
        if !self.ivars().shown.get() {
            self.move_to_focused_screen(&window);
            float_over_everything(&window);
            window.orderFrontRegardless();
            self.ivars().shown.set(true);
        }
    }

    /// Move only when the pill is about to appear. A hold that is already
    /// showing stays on the screen where it started, including processing.
    fn move_to_focused_screen(&self, window: &NSWindow) {
        if self.ivars().drag.get().is_some() {
            return;
        }
        let mtm = self.mtm();
        let Ok(screen) = focused_screen(mtm) else {
            return;
        };
        let pill = (self.ivars().to_w.get(), self.ivars().to_h.get());
        let desired = origin_for_screen(mtm, &screen, pill);
        let frame = window.frame();
        window.setFrameOrigin(NSPoint::new(
            desired.x + (pill.0 - frame.size.width) / 2.0,
            desired.y + (pill.1 - frame.size.height) / 2.0,
        ));
    }

    /// The screen to keep the pill inside: the one showing most of it, or,
    /// for a frame dragged wholly off screen (which has no `screen()`), the
    /// display under the pointer.
    fn clamp_screen(&self, window: &NSWindow) -> Option<Retained<NSScreen>> {
        window
            .screen()
            .or_else(|| screen_containing(self.mtm(), NSEvent::mouseLocation()))
    }

    /// A drop can leave the card partly off the screen. Pull it back inside.
    fn keep_on_screen(&self, window: &NSWindow) {
        let Some(screen) = self.clamp_screen(window) else {
            return;
        };
        let visible = screen_rect(screen.visibleFrame());
        let frame = window.frame();
        let (x, y) = clamp_origin(
            frame.origin.x,
            frame.origin.y,
            visible,
            (frame.size.width, frame.size.height),
        );
        if (x - frame.origin.x).abs() >= 0.25 || (y - frame.origin.y).abs() >= 0.25 {
            window.setFrameOrigin(NSPoint::new(x, y));
        }
    }

    fn tick_dt(&self) -> f64 {
        let now = Instant::now();
        let dt = match self.ivars().last_tick.replace(Some(now)) {
            Some(then) => now.saturating_duration_since(then).as_secs_f64(),
            None => FRAME_DT,
        };
        dt.clamp(0.0, 0.1)
    }

    fn elapsed(&self) -> f64 {
        let now = Instant::now();
        let origin = self.ivars().anim_origin.get().unwrap_or_else(|| {
            self.ivars().anim_origin.set(Some(now));
            now
        });
        now.saturating_duration_since(origin).as_secs_f64()
    }

    fn ingest_level(&self, level: f32, dt: f64) {
        let mut floor = self.ivars().floor_db.get();
        let target = voice_target(f64::from(clamp_level(level)), &mut floor);
        self.ivars().floor_db.set(floor);
        let shown = smooth_shown(self.ivars().voice.get(), target, dt);
        self.ivars().voice.set(shown);
    }

    fn begin_size(&self, to_w: f64, to_h: f64, from_hidden: bool) {
        let (from_w, from_h) = if from_hidden {
            (REST_W, REST_H)
        } else {
            (self.ivars().cur_w.get(), self.ivars().cur_h.get())
        };
        self.ivars().from_w.set(from_w);
        self.ivars().from_h.set(from_h);
        self.ivars().to_w.set(to_w);
        self.ivars().to_h.set(to_h);
        self.ivars().morph.set(0.0);
        self.ivars().cur_w.set(from_w);
        self.ivars().cur_h.set(from_h);
        if from_hidden {
            self.apply_size(from_w, from_h);
        }
    }

    fn step_size(&self, dt: f64) {
        let morph = self.ivars().morph.get();
        let (width, height) = if morph >= 1.0 {
            (self.ivars().to_w.get(), self.ivars().to_h.get())
        } else {
            let next = (morph + dt / MORPH_SECS).min(1.0);
            self.ivars().morph.set(next);
            let eased = unit_bezier(0.05, 0.6, 0.4, 0.95, next);
            let width = lerp(self.ivars().from_w.get(), self.ivars().to_w.get(), eased);
            let height = lerp(self.ivars().from_h.get(), self.ivars().to_h.get(), eased);
            self.ivars().cur_w.set(width);
            self.ivars().cur_h.set(height);
            (width, height)
        };
        self.apply_size(width, height);
    }

    /// Grows from the center, kept inside the screen's visible frame so a
    /// notice card taller than the pill does not hang off the bottom edge.
    /// A drag in progress keeps the frame the pointer is holding.
    fn apply_size(&self, width: f64, height: f64) {
        if self.ivars().drag.get().is_some() {
            return;
        }
        let Some(window) = self.window() else {
            return;
        };
        let frame = window.frame();
        if (frame.size.width - width).abs() < 0.25 && (frame.size.height - height).abs() < 0.25 {
            return;
        }
        let mid_x = frame.origin.x + frame.size.width / 2.0;
        let mid_y = frame.origin.y + frame.size.height / 2.0;
        let mut origin = (mid_x - width / 2.0, mid_y - height / 2.0);
        if let Some(screen) = self.clamp_screen(&window) {
            let visible = screen_rect(screen.visibleFrame());
            origin = clamp_origin(origin.0, origin.1, visible, (width, height));
        }
        window.setFrame_display(
            NSRect::new(NSPoint::new(origin.0, origin.1), NSSize::new(width, height)),
            true,
        );
    }

    fn paint(&self) {
        let mode = self.ivars().mode.get();
        if mode == Mode::Hidden {
            return;
        }
        let pill = self.bounds();
        let radius = if mode == Mode::Error {
            NOTICE_RADIUS
        } else {
            (pill.size.height / 2.0).min(22.5)
        };
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(pill, radius, radius);
        srgb(0.0, 0.0, 0.0, 1.0).setFill();
        path.fill();
        stroke_border(pill, radius);

        match mode {
            Mode::Recording => {
                let time = self.elapsed();
                let scale = audio_scale(self.ivars().voice.get());
                paint_bars(pill, scale, time, true, false, 1.0);
            }
            Mode::Processing => {
                let time = self.elapsed();
                paint_bars(pill, 1.0, time, false, true, 0.4);
                paint_spinner(spinner_center(pill), time);
            }
            Mode::Error => {
                let dark = is_dark(self);
                paint_notice(
                    pill,
                    &self.ivars().title_lines.borrow(),
                    &self.ivars().body_lines.borrow(),
                    dark,
                );
            }
            Mode::Hidden => {}
        }
    }
}

fn stroke_border(pill: NSRect, radius: f64) {
    let inset = 0.5;
    let rect = NSRect::new(
        NSPoint::new(pill.origin.x + inset, pill.origin.y + inset),
        NSSize::new(
            (pill.size.width - inset * 2.0).max(1.0),
            (pill.size.height - inset * 2.0).max(1.0),
        ),
    );
    let radius = (radius - inset).max(0.0);
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
    path.setLineWidth(1.0);
    srgb(48.0 / 255.0, 48.0 / 255.0, 47.0 / 255.0, 1.0).setStroke();
    path.stroke();
}

fn srgb(red: f64, green: f64, blue: f64, alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, alpha)
}

fn is_dark(view: &BarView) -> bool {
    let appearance = view.effectiveAppearance();
    let aqua = unsafe { NSAppearanceNameAqua };
    let dark = unsafe { NSAppearanceNameDarkAqua };
    let names = NSArray::from_slice(&[aqua, dark]);
    match appearance.bestMatchFromAppearancesWithNames(&names) {
        Some(name) => name.isEqualToString(dark),
        None => false,
    }
}

fn bars_span() -> f64 {
    BAR_COUNT as f64 * BAR_W + (BAR_COUNT as f64 - 1.0) * BAR_GAP
}

fn cluster_width(with_spinner: bool) -> f64 {
    let wave = WAVE_PAD_X * 2.0 + bars_span();
    if with_spinner {
        wave + CLUSTER_GAP + SPINNER_BOX
    } else {
        wave
    }
}

fn paint_bars(
    pill: NSRect,
    audio_scale: f64,
    time: f64,
    animate: bool,
    with_spinner: bool,
    alpha: f64,
) {
    NSColor::colorWithWhite_alpha(1.0, alpha).setFill();
    let left = pill.origin.x + (pill.size.width - cluster_width(with_spinner)) / 2.0 + WAVE_PAD_X;
    let mid_y = pill.origin.y + pill.size.height / 2.0;
    for index in 0..BAR_COUNT {
        let height = if animate {
            BAR_BASE * audio_scale * height_share(index) * wave_mul(time - BAR_DELAYS[index])
        } else {
            BAR_BASE
        };
        let rect = NSRect::new(
            NSPoint::new(
                left + index as f64 * (BAR_W + BAR_GAP),
                mid_y - height / 2.0,
            ),
            NSSize::new(BAR_W, height),
        );
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 0.5, 0.5).fill();
    }
}

fn spinner_center(pill: NSRect) -> NSPoint {
    let left = pill.origin.x + (pill.size.width - cluster_width(true)) / 2.0;
    let wave = WAVE_PAD_X * 2.0 + bars_span();
    NSPoint::new(
        left + wave + CLUSTER_GAP + SPINNER_BOX / 2.0,
        pill.origin.y + pill.size.height / 2.0,
    )
}

/// Eight 2×5 capsules on a 6 px radius. Each fades from 0.88 to 0.15 over 1.1s.
fn paint_spinner(center: NSPoint, time: f64) {
    for index in 0..8 {
        let delay = 0.1375 * -((7 - index) as f64);
        let t = (time - delay).rem_euclid(1.1) / 1.1;
        let alpha = 0.88 + (0.15 - 0.88) * t;
        let theta = index as f64 * std::f64::consts::FRAC_PI_4;
        let path = capsule(center.x, center.y, theta.sin(), theta.cos());
        srgb(124.0 / 255.0, 154.0 / 255.0, 192.0 / 255.0, alpha).setFill();
        path.fill();
    }
}

fn capsule(cx: f64, cy: f64, dir_x: f64, dir_y: f64) -> Retained<NSBezierPath> {
    let radius = 1.0;
    let along = 1.5;
    let ox = cx + dir_x * SPINNER_RADIUS + dir_x * along;
    let oy = cy + dir_y * SPINNER_RADIUS + dir_y * along;
    let ix = cx + dir_x * SPINNER_RADIUS - dir_x * along;
    let iy = cy + dir_y * SPINNER_RADIUS - dir_y * along;
    let perp_x = -dir_y;
    let perp_y = dir_x;
    let path = NSBezierPath::new();
    let outer = atan2_deg(-perp_y, -perp_x);
    path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
        NSPoint::new(ox, oy),
        radius,
        outer,
        outer + 180.0,
    );
    let inner = atan2_deg(perp_y, perp_x);
    path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
        NSPoint::new(ix, iy),
        radius,
        inner,
        inner + 180.0,
    );
    path.closePath();
    path
}

fn atan2_deg(y: f64, x: f64) -> f64 {
    y.atan2(x).to_degrees()
}

/// Center bars are taller: `max(0, 1 - h²/48)` with `h = abs(4.5 - i)`.
fn height_share(index: usize) -> f64 {
    let mid = (BAR_COUNT - 1) as f64 / 2.0;
    let distance = (mid - index as f64).abs();
    (1.0 - distance * distance / 48.0).max(0.0)
}

/// 1s ease-in-out loop. Stops are 1, 1.2, 1.5, 1.1, 1.3, 1.
fn wave_mul(time: f64) -> f64 {
    let t = time.rem_euclid(1.0);
    const STOPS: [(f64, f64); 6] = [
        (0.0, 1.0),
        (0.2, 1.2),
        (0.4, 1.5),
        (0.8, 1.1),
        (0.9, 1.3),
        (1.0, 1.0),
    ];
    for pair in STOPS.windows(2) {
        let (t0, v0) = pair[0];
        let (t1, v1) = pair[1];
        if t <= t1 {
            let span = (t1 - t0).max(f64::EPSILON);
            let eased = unit_bezier(0.42, 0.0, 0.58, 1.0, (t - t0) / span);
            return v0 + (v1 - v0) * eased;
        }
    }
    1.0
}

/// `cubic-bezier` with endpoints fixed at (0, 0) and (1, 1). `x` is the time fraction.
fn unit_bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let mut t = x;
    for _ in 0..8 {
        let miss = bezier_component(t, x1, x2) - x;
        let slope = bezier_slope(t, x1, x2);
        if slope.abs() < 1e-6 {
            break;
        }
        t = (t - miss / slope).clamp(0.0, 1.0);
    }
    if (bezier_component(t, x1, x2) - x).abs() > 1e-4 {
        let mut lo = 0.0;
        let mut hi = 1.0;
        for _ in 0..24 {
            t = (lo + hi) * 0.5;
            if bezier_component(t, x1, x2) < x {
                lo = t;
            } else {
                hi = t;
            }
        }
    }
    bezier_component(t, y1, y2)
}

fn bezier_component(t: f64, a1: f64, a2: f64) -> f64 {
    let c = 1.0 - t;
    3.0 * c * c * t * a1 + 3.0 * c * t * t * a2 + t * t * t
}

fn bezier_slope(t: f64, a1: f64, a2: f64) -> f64 {
    let c = 1.0 - t;
    3.0 * c * c * a1 + 6.0 * c * t * (a2 - a1) + 3.0 * t * t * (1.0 - a2)
}

fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t
}

/// Adaptive dBFS normalizer. Silence must not move the floor, or room tone pegs the meter.
fn voice_target(linear: f64, floor_db: &mut f64) -> f64 {
    if !linear.is_finite() || linear <= 1e-6 {
        return 0.0;
    }
    let db = 20.0 * linear.log10();
    if db < *floor_db {
        *floor_db = db.max(-60.0);
    }
    ((db - *floor_db) / 20.0).clamp(0.0, 1.0)
}

/// One frame of `shown = 0.85*prev + 0.15*target`, scaled for the real tick, then centi-floored.
fn smooth_shown(previous: f64, target: f64, dt: f64) -> f64 {
    let keep = 0.85_f64.powf(dt / FRAME_DT);
    let shown = keep * previous + (1.0 - keep) * target;
    (shown * 100.0).floor() / 100.0
}

fn audio_scale(shown: f64) -> f64 {
    (5.0 * shown).max(1.0)
}

struct NoticeLayout {
    title_lines: Vec<String>,
    body_lines: Vec<String>,
    width: f64,
    height: f64,
}

/// Shrink to the text, then clamp to the 340–440 card.
/// A line that does not fit the maximum card wraps. The card stays 440 wide.
fn layout_notice(title: &str, body: &str, measure: impl Fn(&str) -> f64) -> NoticeLayout {
    let title = collapse_whitespace(title);
    let body = collapse_whitespace(body);
    let inner_max = NOTICE_MAX_W - NOTICE_PAD * 2.0;
    let title_max = (inner_max - NOTICE_ICON - NOTICE_ICON_GAP).max(1.0);
    let title_row = measure(&title) + NOTICE_ICON + NOTICE_ICON_GAP;
    let body_width = if body.is_empty() { 0.0 } else { measure(&body) };
    let natural = title_row.max(body_width) + NOTICE_PAD * 2.0;
    let (title_lines, body_lines, width) = if natural <= NOTICE_MAX_W {
        (
            vec![title],
            if body.is_empty() {
                Vec::new()
            } else {
                vec![body]
            },
            natural.max(NOTICE_MIN_W),
        )
    } else {
        (
            wrap_text(&title, title_max, &measure),
            wrap_text(&body, inner_max, &measure),
            NOTICE_MAX_W,
        )
    };
    let title_lines = if title_lines.is_empty() {
        vec![String::new()]
    } else {
        title_lines
    };
    let height = notice_height(title_lines.len(), body_lines.len());
    NoticeLayout {
        title_lines,
        body_lines,
        width,
        height,
    }
}

fn notice_height(title_lines: usize, body_lines: usize) -> f64 {
    let title_h = title_lines.max(1) as f64 * NOTICE_LINE;
    let body_h = if body_lines == 0 {
        0.0
    } else {
        NOTICE_GAP + body_lines as f64 * NOTICE_LINE
    };
    NOTICE_PAD + title_h + body_h + NOTICE_PAD
}

fn wrap_text(text: &str, max_width: f64, measure: &impl Fn(&str) -> f64) -> Vec<String> {
    let text = collapse_whitespace(text);
    if text.is_empty() {
        return Vec::new();
    }
    if measure(&text) <= max_width {
        return vec![text];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        if current.is_empty() {
            push_word(&mut lines, &mut current, word, max_width, measure);
            continue;
        }
        let candidate = format!("{current} {word}");
        if measure(&candidate) <= max_width {
            current = candidate;
        } else {
            lines.push(std::mem::take(&mut current));
            push_word(&mut lines, &mut current, word, max_width, measure);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn push_word(
    lines: &mut Vec<String>,
    current: &mut String,
    word: &str,
    max_width: f64,
    measure: &impl Fn(&str) -> f64,
) {
    if measure(word) <= max_width {
        current.push_str(word);
        return;
    }
    let mut piece = String::new();
    for ch in word.chars() {
        let next = format!("{piece}{ch}");
        if !piece.is_empty() && measure(&next) > max_width {
            lines.push(std::mem::take(&mut piece));
        }
        piece.push(ch);
    }
    *current = piece;
}

fn paint_notice(pill: NSRect, title_lines: &[String], body_lines: &[String], dark: bool) {
    let font = NSFont::systemFontOfSize(NOTICE_FONT);
    // Light-theme sand-50 and vast-300. The card stays black, so these stay readable.
    let title_color = srgb(252.0 / 255.0, 252.0 / 255.0, 251.0 / 255.0, 1.0);
    let body_color = srgb(179.0 / 255.0, 178.0 / 255.0, 173.0 / 255.0, 1.0);
    // destructive-500 in light, red-450 in dark.
    let icon_color = if dark {
        srgb(1.0, 87.0 / 255.0, 87.0 / 255.0, 1.0)
    } else {
        srgb(238.0 / 255.0, 106.0 / 255.0, 106.0 / 255.0, 1.0)
    };

    let title_count = title_lines.len().max(1);
    let title_block = title_count as f64 * NOTICE_LINE;
    let title_top = pill.origin.y + pill.size.height - NOTICE_PAD;
    let title_bottom = title_top - title_block;
    let icon_x = pill.origin.x + NOTICE_PAD;
    let icon_y = title_bottom + (title_block - NOTICE_ICON) / 2.0;
    paint_alert_icon(icon_x, icon_y, &icon_color);

    let text_x = icon_x + NOTICE_ICON + NOTICE_ICON_GAP;
    for (index, line) in title_lines.iter().enumerate() {
        let row_bottom = title_top - (index as f64 + 1.0) * NOTICE_LINE;
        draw_line(line, &font, &title_color, text_x, row_bottom);
    }
    let body_top = title_bottom - NOTICE_GAP;
    let body_x = pill.origin.x + NOTICE_PAD;
    for (index, line) in body_lines.iter().enumerate() {
        let row_bottom = body_top - (index as f64 + 1.0) * NOTICE_LINE;
        draw_line(line, &font, &body_color, body_x, row_bottom);
    }
}

/// 18 px circle-exclamation. Lucide's 24 px icon, stroke 2, scaled by 0.75.
fn paint_alert_icon(x: f64, y: f64, color: &NSColor) {
    color.setStroke();
    color.setFill();
    let stroke = 1.5;
    let cx = x + 9.0;
    let cy = y + 9.0;
    let radius = 7.5;
    let circle = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(cx - radius, cy - radius),
        NSSize::new(radius * 2.0, radius * 2.0),
    ));
    circle.setLineWidth(stroke);
    circle.stroke();

    let stem = NSBezierPath::new();
    stem.moveToPoint(NSPoint::new(cx, y + 12.0));
    stem.lineToPoint(NSPoint::new(cx, y + 9.0));
    stem.setLineWidth(stroke);
    stem.setLineCapStyle(NSLineCapStyle::Round);
    stem.stroke();

    let dot = stroke / 2.0;
    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(cx - dot, y + 6.0 - dot),
        NSSize::new(stroke, stroke),
    ))
    .fill();
}

fn draw_line(text: &str, font: &NSFont, color: &NSColor, x: f64, row_bottom: f64) {
    if text.is_empty() {
        return;
    }
    // `drawAtPoint` places the bottom of the line box, not the baseline, at
    // this point in an unflipped view. Center that box in the row.
    let box_bottom = row_bottom + (NOTICE_LINE - (font.ascender() - font.descender())) / 2.0;
    let attrs = text_attributes(font, Some(color));
    let string = NSString::from_str(text);
    unsafe {
        string.drawAtPoint_withAttributes(NSPoint::new(x, box_bottom), Some(&attrs));
    }
}

fn text_attributes(
    font: &NSFont,
    color: Option<&NSColor>,
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let font_key = unsafe { NSFontAttributeName };
    let font_obj: &AnyObject = font;
    if let Some(color) = color {
        let color_key = unsafe { NSForegroundColorAttributeName };
        let color_obj: &AnyObject = color;
        NSDictionary::from_slices(&[font_key, color_key], &[font_obj, color_obj])
    } else {
        NSDictionary::from_slices(&[font_key], &[font_obj])
    }
}

fn measure(text: &str, font: &NSFont) -> f64 {
    let string = NSString::from_str(text);
    let attrs = text_attributes(font, None);
    unsafe { string.sizeWithAttributes(Some(&attrs)).width }
}

fn collapse_whitespace(message: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in message.chars() {
        if ch.is_whitespace() {
            if !out.is_empty() {
                pending_space = true;
            }
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    out
}

fn clamp_level(level: f32) -> f32 {
    if level.is_finite() {
        level.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Screen the user is typing on.
///
/// `NSScreen.main` is the screen of the key window, but an LSUIElement app
/// can be given the menu-bar screen instead when displays do not have
/// separate Spaces (FB11506568). The frontmost app's top on-screen window
/// is the display that actually has the keyboard.
fn focused_screen(mtm: MainThreadMarker) -> Result<Retained<NSScreen>, BarError> {
    if let Some(screen) = screen_of_front_window(mtm) {
        return Ok(screen);
    }
    if let Some(screen) = NSScreen::mainScreen(mtm) {
        return Ok(screen);
    }
    if let Some(screen) = screen_containing(mtm, NSEvent::mouseLocation()) {
        return Ok(screen);
    }
    let screens = NSScreen::screens(mtm);
    if screens.count() == 0 {
        Err(BarError::NoScreen)
    } else {
        Ok(screens.objectAtIndex(0))
    }
}

fn screen_of_front_window(mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    let front = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let pid = i64::from(front.processIdentifier());
    if pid <= 0 {
        return None;
    }
    let screens = NSScreen::screens(mtm);
    if screens.count() == 0 {
        return None;
    }
    let primary_height = screens.objectAtIndex(0).frame().size.height;
    let window = front_window_bounds(pid, primary_height)?;
    let frames: Vec<ScreenRect> = (0..screens.count())
        .map(|index| screen_rect(screens.objectAtIndex(index).frame()))
        .collect();
    let index = screen_index_for_window(window, &frames)?;
    Some(screens.objectAtIndex(index))
}

/// First on-screen layer-0 window of `pid`, in front-to-back order.
/// Quartz bounds use the top-left of the primary display. `primary_height`
/// is `NSScreen.screens[0]`, the menu-bar display, whose height is the flip axis.
fn front_window_bounds(pid: i64, primary_height: f64) -> Option<ScreenRect> {
    let info = copy_window_info(kCGWindowListOptionOnScreenOnly, kCGNullWindowID)?;
    let layer_key = unsafe { kCGWindowLayer };
    let pid_key = unsafe { kCGWindowOwnerPID };
    let alpha_key = unsafe { kCGWindowAlpha };
    let bounds_key = unsafe { kCGWindowBounds };
    for index in 0..info.len() {
        let Some(item) = info.get(index) else {
            continue;
        };
        let dict = *item as CFDictionaryRef;
        if dict.is_null() {
            continue;
        }
        let Some(owner) = dict_i64(dict, pid_key) else {
            continue;
        };
        let Some(layer) = dict_i64(dict, layer_key) else {
            continue;
        };
        let alpha = dict_f64(dict, alpha_key).unwrap_or(1.0);
        let Some(bounds) = window_cg_rect(dict, bounds_key) else {
            continue;
        };
        if !is_focus_window(
            owner,
            pid,
            layer,
            alpha,
            bounds.size.width as f64,
            bounds.size.height as f64,
        ) {
            continue;
        }
        return Some(cg_window_to_appkit(
            bounds.origin.x as f64,
            bounds.origin.y as f64,
            bounds.size.width as f64,
            bounds.size.height as f64,
            primary_height,
        ));
    }
    None
}

fn dict_number(dict: CFDictionaryRef, key: CFStringRef) -> Option<CFNumber> {
    let value = unsafe { CFDictionaryGetValue(dict, key as *const std::ffi::c_void) };
    if value.is_null() {
        return None;
    }
    unsafe { CFType::wrap_under_get_rule(value) }.downcast()
}

fn dict_i64(dict: CFDictionaryRef, key: CFStringRef) -> Option<i64> {
    let number = dict_number(dict, key)?;
    number
        .to_i64()
        .or_else(|| number.to_f64().map(|value| value as i64))
}

fn dict_f64(dict: CFDictionaryRef, key: CFStringRef) -> Option<f64> {
    let number = dict_number(dict, key)?;
    number
        .to_f64()
        .or_else(|| number.to_i64().map(|value| value as f64))
}

fn window_cg_rect(dict: CFDictionaryRef, key: CFStringRef) -> Option<CGRect> {
    let value = unsafe { CFDictionaryGetValue(dict, key as *const std::ffi::c_void) };
    if value.is_null() {
        return None;
    }
    let bounds = unsafe { CFType::wrap_under_get_rule(value) }.downcast_into::<CFDictionary>()?;
    CGRect::from_dict_representation(&bounds)
}

/// Bottom-left of a pill of `pill` size on `screen`.
/// A saved drag is its centre's fraction across its old visible frame and an
/// inset from that frame's bottom, then clamped onto this screen. Saving the
/// centre keeps a centred pill centred as it changes width.
fn origin_for_screen(mtm: MainThreadMarker, screen: &NSScreen, pill: (f64, f64)) -> NSPoint {
    let visible = screen_rect(screen.visibleFrame());
    let point = if let Some(saved) = load_position() {
        if let Some(home) = screen_containing(mtm, saved) {
            let from = screen_rect(home.visibleFrame());
            map_position((saved.x, saved.y), from, visible, pill)
        } else {
            bottom_center(visible, pill)
        }
    } else {
        bottom_center(visible, pill)
    };
    NSPoint::new(point.0, point.1)
}

/// AppKit rectangle: origin is the bottom-left, y grows upward.
#[derive(Clone, Copy, Debug)]
struct ScreenRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn screen_rect(rect: NSRect) -> ScreenRect {
    ScreenRect {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

/// Quartz window bounds are top-left of the primary display, y downward.
fn cg_window_to_appkit(x: f64, y: f64, width: f64, height: f64, primary_height: f64) -> ScreenRect {
    ScreenRect {
        x,
        y: primary_height - (y + height),
        width,
        height,
    }
}

fn intersection_area(a: ScreenRect, b: ScreenRect) -> f64 {
    let left = a.x.max(b.x);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = a.y.max(b.y);
    let top = (a.y + a.height).min(b.y + b.height);
    (right - left).max(0.0) * (top - bottom).max(0.0)
}

fn screen_index_for_window(window: ScreenRect, screens: &[ScreenRect]) -> Option<usize> {
    let mut best_index = None;
    let mut best_area = 0.0;
    for (index, screen) in screens.iter().enumerate() {
        let area = intersection_area(window, *screen);
        if area > best_area {
            best_area = area;
            best_index = Some(index);
        }
    }
    best_index
}

const MIN_FOCUS_W: f64 = 40.0;
const MIN_FOCUS_H: f64 = 20.0;

fn is_focus_window(owner: i64, pid: i64, layer: i64, alpha: f64, width: f64, height: f64) -> bool {
    owner == pid
        && layer == 0
        && alpha > 0.05
        && width >= MIN_FOCUS_W
        && height >= MIN_FOCUS_H
        && width.is_finite()
        && height.is_finite()
}

/// `saved` is the pill's bottom-centre point.
fn map_position(
    saved: (f64, f64),
    from: ScreenRect,
    to: ScreenRect,
    pill: (f64, f64),
) -> (f64, f64) {
    let frac_x = if from.width > 1.0 {
        (saved.0 - from.x) / from.width
    } else {
        0.5
    };
    let inset_y = saved.1 - from.y;
    clamp_origin(
        to.x + frac_x * to.width - pill.0 / 2.0,
        to.y + inset_y,
        to,
        pill,
    )
}

/// Put the pill exactly on the visible frame's centre line when its centre
/// is within `SNAP_DISTANCE` of it. Returns the origin x and whether it snapped.
fn snap_to_center(x: f64, width: f64, visible: ScreenRect) -> (f64, bool) {
    let center = visible.x + visible.width / 2.0;
    if (x + width / 2.0 - center).abs() <= SNAP_DISTANCE {
        (center - width / 2.0, true)
    } else {
        (x, false)
    }
}

fn bottom_center(visible: ScreenRect, pill: (f64, f64)) -> (f64, f64) {
    clamp_origin(
        visible.x + (visible.width - pill.0) / 2.0,
        visible.y + MARGIN,
        visible,
        pill,
    )
}

fn clamp_origin(x: f64, y: f64, visible: ScreenRect, pill: (f64, f64)) -> (f64, f64) {
    let max_x = (visible.x + visible.width - pill.0).max(visible.x);
    let max_y = (visible.y + visible.height - pill.1).max(visible.y);
    (x.clamp(visible.x, max_x), y.clamp(visible.y, max_y))
}

fn screen_containing(mtm: MainThreadMarker, point: NSPoint) -> Option<Retained<NSScreen>> {
    let screens = NSScreen::screens(mtm);
    for index in 0..screens.count() {
        let screen = screens.objectAtIndex(index);
        if NSPointInRect(point, screen.frame()) {
            return Some(screen);
        }
    }
    None
}

fn position_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Iki")
            .join("bar-position.json"),
    )
}

/// Saves the bottom-centre point of `frame`.
fn save_position(frame: NSRect) {
    let Some(path) = position_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let center_x = frame.origin.x + frame.size.width / 2.0;
    let body = format!("{{\"cx\":{:.4},\"y\":{:.4}}}\n", center_x, frame.origin.y);
    let _ = fs::write(path, body);
}

/// The saved bottom-centre point. A file from before centres were saved holds
/// the recording pill's origin as `x`.
fn load_position() -> Option<NSPoint> {
    let text = fs::read_to_string(position_path()?).ok()?;
    let center_x = json_number(&text, "cx")
        .or_else(|| json_number(&text, "x").map(|x| x + WIDTH_RECORDING / 2.0))?;
    let y = json_number(&text, "y")?;
    Some(NSPoint::new(center_x, y))
}

fn json_number(text: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\"");
    let index = text.find(&needle)?;
    let rest = text[index + needle.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    parse_number(rest)
}

fn parse_number(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut end = 0;
    if bytes[0] == b'+' || bytes[0] == b'-' {
        end = 1;
    }
    let mut saw_digit = false;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        saw_digit = true;
        end += 1;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            saw_digit = true;
            end += 1;
        }
    }
    if !saw_digit {
        return None;
    }
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let marker = end;
        end += 1;
        if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
            end += 1;
        }
        let exp = end;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == exp {
            end = marker;
        }
    }
    text[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{
        audio_scale, bottom_center, cg_window_to_appkit, height_share, is_focus_window,
        layout_notice, map_position, screen_index_for_window, snap_to_center, unit_bezier,
        voice_target, wave_mul, ScreenRect, MARGIN, NOTICE_GAP, NOTICE_LINE, NOTICE_MAX_W,
        NOTICE_MIN_W, NOTICE_PAD,
    };

    fn chars(text: &str) -> f64 {
        text.chars().count() as f64 * 10.0
    }

    #[test]
    fn short_error_is_the_minimum_card() {
        let laid = layout_notice("No audio received", "Hi.", chars);
        assert_eq!(laid.width, NOTICE_MIN_W);
        assert_eq!(laid.title_lines, vec!["No audio received".to_string()]);
        assert_eq!(laid.body_lines, vec!["Hi.".to_string()]);
        assert_eq!(
            laid.height,
            NOTICE_PAD + NOTICE_LINE + NOTICE_GAP + NOTICE_LINE + NOTICE_PAD
        );
    }

    #[test]
    fn long_error_wraps_inside_the_maximum_card() {
        let body =
            "We're not hearing anything. There may be an issue with your mic or permissions.";
        let laid = layout_notice("No audio received", body, chars);
        assert_eq!(laid.width, NOTICE_MAX_W);
        assert!(laid.body_lines.len() >= 2, "{:?}", laid.body_lines);
        let inner = NOTICE_MAX_W - NOTICE_PAD * 2.0;
        for line in &laid.body_lines {
            assert!(chars(line) <= inner + 1e-6, "{line}");
        }
        assert_eq!(laid.body_lines.join(" "), body);
    }

    #[test]
    fn outer_bar_height_share_is_fixed() {
        let share = height_share(0);
        assert!((share - 0.578125).abs() < 1e-9, "{share}");
        assert!((height_share(9) - share).abs() < 1e-9);
    }

    #[test]
    fn morph_bezier_hits_the_endpoints() {
        assert_eq!(unit_bezier(0.05, 0.6, 0.4, 0.95, 0.0), 0.0);
        assert_eq!(unit_bezier(0.05, 0.6, 0.4, 0.95, 1.0), 1.0);
        let mid = unit_bezier(0.05, 0.6, 0.4, 0.95, 0.5);
        assert!((mid - 0.860146).abs() < 1e-4, "{mid}");
    }

    #[test]
    fn ease_in_out_midpoint_is_linear() {
        let mid = unit_bezier(0.42, 0.0, 0.58, 1.0, 0.5);
        assert!((mid - 0.5).abs() < 1e-6, "{mid}");
        let early = unit_bezier(0.42, 0.0, 0.58, 1.0, 0.25);
        assert!((early - 0.129162).abs() < 1e-4, "{early}");
    }

    #[test]
    fn wave_stops_match_the_keyframes() {
        let samples = [
            (0.0, 1.0),
            (0.2, 1.2),
            (0.4, 1.5),
            (0.8, 1.1),
            (0.9, 1.3),
            (1.0, 1.0),
        ];
        for (time, expected) in samples {
            let got = wave_mul(time);
            assert!((got - expected).abs() < 1e-9, "t={time} got={got}");
        }
    }

    #[test]
    fn silence_does_not_pin_the_floor() {
        let mut floor = 0.0;
        assert_eq!(voice_target(0.0, &mut floor), 0.0);
        assert_eq!(floor, 0.0);
        let quiet = 10f64.powf(-40.0 / 20.0);
        let _ = voice_target(quiet, &mut floor);
        assert!((floor - -40.0).abs() < 1e-6);
        assert_eq!(voice_target(0.0, &mut floor), 0.0);
        assert!((floor - -40.0).abs() < 1e-6);
    }

    #[test]
    fn twenty_db_above_the_floor_reaches_scale_five() {
        let mut floor = 0.0;
        let quiet = 10f64.powf(-40.0 / 20.0);
        let idle = voice_target(quiet, &mut floor);
        assert!(idle.abs() < 1e-6);
        let loud = 10f64.powf(-20.0 / 20.0);
        let target = voice_target(loud, &mut floor);
        assert!((target - 1.0).abs() < 1e-6, "{target}");
        assert!((audio_scale(target) - 5.0).abs() < 1e-9);
        assert!((floor - -40.0).abs() < 1e-6);
    }

    #[test]
    fn cg_bounds_flip_against_the_primary_display() {
        let frame = cg_window_to_appkit(40.0, 100.0, 200.0, 80.0, 1512.0);
        assert!((frame.x - 40.0).abs() < 1e-9);
        assert!((frame.y - (1512.0 - 180.0)).abs() < 1e-9);
        assert!((frame.width - 200.0).abs() < 1e-9);
        assert!((frame.height - 80.0).abs() < 1e-9);
    }

    #[test]
    fn window_below_the_primary_lands_on_the_lower_screen() {
        let window = cg_window_to_appkit(20.0, 900.0, 400.0, 300.0, 900.0);
        let screens = [
            ScreenRect {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            ScreenRect {
                x: 0.0,
                y: -1080.0,
                width: 1920.0,
                height: 1080.0,
            },
        ];
        assert_eq!(screen_index_for_window(window, &screens), Some(1));
    }

    #[test]
    fn spanned_window_belongs_to_the_screen_with_more_area() {
        let window = ScreenRect {
            x: 1400.0,
            y: 0.0,
            width: 400.0,
            height: 100.0,
        };
        let screens = [
            ScreenRect {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            ScreenRect {
                x: 1440.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
        ];
        assert_eq!(screen_index_for_window(window, &screens), Some(1));
    }

    #[test]
    fn focus_window_skips_the_status_pill_and_tiny_windows() {
        assert!(is_focus_window(10, 10, 0, 1.0, 800.0, 600.0));
        assert!(!is_focus_window(10, 11, 0, 1.0, 800.0, 600.0));
        assert!(!is_focus_window(10, 10, 25, 1.0, 73.0, 30.0));
        assert!(!is_focus_window(10, 10, 0, 0.0, 800.0, 600.0));
        assert!(!is_focus_window(10, 10, 0, 1.0, 8.0, 8.0));
    }

    #[test]
    fn dragged_centre_keeps_its_fraction_and_bottom_inset() {
        let from = ScreenRect {
            x: 0.0,
            y: 80.0,
            width: 1440.0,
            height: 800.0,
        };
        let to = ScreenRect {
            x: 1440.0,
            y: 0.0,
            width: 1920.0,
            height: 1050.0,
        };
        let (x, y) = map_position((360.0, 120.0), from, to, (73.0, 30.0));
        assert!((x - (1440.0 + 0.25 * 1920.0 - 36.5)).abs() < 1e-6, "{x}");
        assert!((y - 40.0).abs() < 1e-6, "{y}");
    }

    #[test]
    fn same_screen_mapping_centres_on_the_saved_point() {
        let frame = ScreenRect {
            x: 100.0,
            y: 40.0,
            width: 1400.0,
            height: 860.0,
        };
        let (x, y) = map_position((400.0, 48.0), frame, frame, (73.0, 30.0));
        assert!((x - (400.0 - 36.5)).abs() < 1e-6, "{x}");
        assert!((y - 48.0).abs() < 1e-6);
    }

    #[test]
    fn a_centred_pill_stays_centred_at_any_width_and_screen() {
        let from = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
        };
        let to = ScreenRect {
            x: 1440.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        for width in [40.0, 73.0, 98.0, 400.0] {
            let (x, _) = map_position((720.0, 8.0), from, to, (width, 30.0));
            assert!(
                (x + width / 2.0 - (1440.0 + 960.0)).abs() < 1e-6,
                "{width}: {x}"
            );
        }
    }

    #[test]
    fn mapped_origin_stays_inside_the_visible_frame() {
        let from = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 3000.0,
            height: 1800.0,
        };
        let to = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 500.0,
        };
        let (x, y) = map_position((2900.0, 1700.0), from, to, (73.0, 30.0));
        assert!(x >= 0.0 && x + 73.0 <= 800.0 + 1e-6, "{x}");
        assert!(y >= 0.0 && y + 30.0 <= 500.0 + 1e-6, "{y}");
    }

    #[test]
    fn a_drag_near_the_centre_line_snaps_onto_it() {
        let visible = ScreenRect {
            x: 100.0,
            y: 0.0,
            width: 1400.0,
            height: 900.0,
        };
        // Centre line is x = 800; a 73 wide pill centres at origin 763.5.
        assert_eq!(snap_to_center(763.5 + 12.0, 73.0, visible), (763.5, true));
        assert_eq!(snap_to_center(763.5 - 9.0, 73.0, visible), (763.5, true));
        assert_eq!(snap_to_center(763.5 + 12.5, 73.0, visible), (776.0, false));
        assert_eq!(snap_to_center(300.0, 73.0, visible), (300.0, false));
    }

    #[test]
    fn bottom_center_uses_the_margin() {
        let visible = ScreenRect {
            x: 10.0,
            y: 70.0,
            width: 1440.0,
            height: 800.0,
        };
        let (x, y) = bottom_center(visible, (73.0, 30.0));
        assert!((x - (10.0 + (1440.0 - 73.0) / 2.0)).abs() < 1e-9);
        assert!((y - (70.0 + MARGIN)).abs() < 1e-9);
    }
}
