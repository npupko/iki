//! Floating recording pill for Iki.

use std::cell::{Cell, OnceCell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSBackingStoreType,
    NSBezierPath, NSColor, NSEvent, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSGraphicsContext, NSLineCapStyle, NSPanel, NSResponder, NSScreen, NSShadow,
    NSStatusWindowLevel, NSStringDrawing, NSView, NSWindow, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSAttributedStringKey, NSDictionary, NSPoint, NSPointInRect, NSRect, NSRunLoop,
    NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};

/// Window is larger than the pill so the shadow is not clipped.
/// The capsule itself is 92×36, close to the dictation bubble.
const WINDOW_W: f64 = 124.0;
const WINDOW_H: f64 = 68.0;
const WINDOW_W_MAX: f64 = 360.0;
const INSET: f64 = 16.0;
const TEXT_PAD: f64 = 16.0;
const FONT_SIZE: f64 = 13.0;
const MARGIN: f64 = 8.0;
const FRAME_DT: f64 = 1.0 / 60.0;

/// Delay in seconds, then height share. Ten bars, delays of 0.1s mirrored
/// around the center.
const BARS: [(f64, f64); 10] = [
    (0.0, 0.578),
    (0.1, 0.745),
    (0.2, 0.870),
    (0.3, 0.953),
    (0.4, 0.995),
    (-0.5, 0.995),
    (-0.4, 0.953),
    (-0.3, 0.870),
    (-0.2, 0.745),
    (-0.1, 0.578),
];
const BAR_W: f64 = 2.0;
const BAR_GAP: f64 = 2.0;
const BAR_MIN: f64 = 4.0;
const BAR_MAX: f64 = 26.0;
/// This MacBook mic's speech peaks around 0.05. Room tone sits near 0.01.
const LEVEL_FLOOR: f64 = 0.01;
const LEVEL_FULL: f64 = 0.055;

#[derive(Clone, Debug)]
pub enum BarPhase {
    Hidden,
    Recording { level: f32 },
    Processing,
    Error { message: String },
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
}

struct ViewIvars {
    slot: Arc<Mutex<BarPhase>>,
    level: Cell<f32>,
    mode: Cell<Mode>,
    raw_message: RefCell<String>,
    fitted: RefCell<String>,
    anim_t: Cell<f64>,
    shown: Cell<bool>,
    drag: Cell<Option<Drag>>,
    timer: OnceCell<Retained<NSTimer>>,
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
        let screen = placement_screen(mtm)?;
        let origin = initial_origin(mtm, &screen);
        let slot = Arc::new(Mutex::new(BarPhase::Hidden));
        let view = BarView::new(mtm, Arc::clone(&slot));
        let panel = BarPanel::create(
            mtm,
            NSRect::new(origin, NSSize::new(WINDOW_W, WINDOW_H)),
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
    panel.setLevel(NSStatusWindowLevel);
    panel.setHidesOnDeactivate(false);
    panel.setIgnoresMouseEvents(false);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
    panel.setCollectionBehavior(
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
            window.setFrameOrigin(NSPoint::new(
                drag.start_origin.x + (now.x - drag.start_mouse.x),
                drag.start_origin.y + (now.y - drag.start_mouse.y),
            ));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if self.ivars().drag.take().is_none() {
                return;
            }
            if let Some(window) = self.window() {
                save_origin(window.frame().origin);
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
            level: Cell::new(0.0),
            mode: Cell::new(Mode::Hidden),
            raw_message: RefCell::new(String::new()),
            fitted: RefCell::new(String::new()),
            anim_t: Cell::new(0.0),
            shown: Cell::new(false),
            drag: Cell::new(None),
            timer: OnceCell::new(),
        });
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(WINDOW_W, WINDOW_H));
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn latest_phase(&self) -> BarPhase {
        self.ivars()
            .slot
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn apply_phase(&self) {
        match self.latest_phase() {
            BarPhase::Hidden => {
                self.ivars().level.set(0.0);
                if self.ivars().mode.get() != Mode::Hidden {
                    self.ivars().mode.set(Mode::Hidden);
                    self.ivars().raw_message.borrow_mut().clear();
                    self.ivars().fitted.borrow_mut().clear();
                    if let Some(window) = self.window() {
                        window.orderOut(None);
                    }
                    self.ivars().shown.set(false);
                }
            }
            BarPhase::Recording { level } => {
                self.ivars().level.set(clamp_level(level));
                if self.ivars().mode.get() != Mode::Recording {
                    self.ivars().mode.set(Mode::Recording);
                    self.ivars().raw_message.borrow_mut().clear();
                    self.ivars().fitted.borrow_mut().clear();
                    self.present(WINDOW_W);
                }
                self.step_anim();
                self.setNeedsDisplay(true);
            }
            BarPhase::Processing => {
                if self.ivars().mode.get() != Mode::Processing {
                    self.ivars().mode.set(Mode::Processing);
                    self.ivars().raw_message.borrow_mut().clear();
                    self.ivars().fitted.borrow_mut().clear();
                    self.present(WINDOW_W);
                }
                self.step_anim();
                self.setNeedsDisplay(true);
            }
            BarPhase::Error { message } => {
                let changed = self.ivars().mode.get() != Mode::Error
                    || *self.ivars().raw_message.borrow() != message;
                if changed {
                    let font = NSFont::systemFontOfSize(FONT_SIZE);
                    let (fitted, width) = fit_line(&message, &font);
                    self.ivars().mode.set(Mode::Error);
                    *self.ivars().raw_message.borrow_mut() = message;
                    *self.ivars().fitted.borrow_mut() = fitted;
                    self.present(width);
                    self.setNeedsDisplay(true);
                }
            }
        }
    }

    fn present(&self, width: f64) {
        let Some(window) = self.window() else {
            return;
        };
        resize_centered(&window, width);
        if !self.ivars().shown.get() {
            window.orderFrontRegardless();
            self.ivars().shown.set(true);
        }
    }

    fn step_anim(&self) {
        let mut t = self.ivars().anim_t.get() + FRAME_DT;
        if t > 10_000.0 {
            t -= 10_000.0;
        }
        self.ivars().anim_t.set(t);
    }

    fn paint(&self) {
        let mode = self.ivars().mode.get();
        if mode == Mode::Hidden {
            return;
        }
        let bounds = self.bounds();
        let pill = pill_rect(bounds);
        let dark = is_dark(self);
        let fill = pill_fill(mode, dark);

        NSGraphicsContext::saveGraphicsState_class();
        let shadow = NSShadow::new();
        shadow.setShadowOffset(NSSize::new(0.0, -2.0));
        shadow.setShadowBlurRadius(14.0);
        shadow.setShadowColor(Some(&NSColor::colorWithWhite_alpha(0.0, 0.28)));
        shadow.set();
        let radius = pill.size.height / 2.0;
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(pill, radius, radius);
        fill.setFill();
        path.fill();
        NSGraphicsContext::restoreGraphicsState_class();

        match mode {
            Mode::Recording => {
                stroke_rim(&path);
                let left = cluster_left(pill, 0.0);
                paint_bars(
                    left,
                    pill.mid().y,
                    f64::from(self.ivars().level.get()),
                    self.ivars().anim_t.get(),
                    1.0,
                );
            }
            Mode::Processing => {
                let time = self.ivars().anim_t.get();
                paint_ring(pill, time);
                let left = cluster_left(pill, SPINNER_GAP + SPINNER_BOX);
                paint_bars(left, pill.mid().y, 0.03, time, 0.72);
                paint_spokes(
                    NSPoint::new(left + bar_row_width() + SPINNER_GAP + SPINNER_BOX / 2.0, pill.mid().y),
                    time,
                );
            }
            Mode::Error => paint_error(pill, &self.ivars().fitted.borrow(), dark),
            Mode::Hidden => {}
        }
    }
}

fn stroke_rim(path: &NSBezierPath) {
    path.setLineWidth(1.0);
    srgb(48.0 / 255.0, 48.0 / 255.0, 47.0 / 255.0, 1.0).setStroke();
    path.stroke();
}

fn pill_rect(bounds: NSRect) -> NSRect {
    NSRect::new(
        NSPoint::new(INSET, INSET),
        NSSize::new(
            (bounds.size.width - INSET * 2.0).max(1.0),
            (bounds.size.height - INSET * 2.0).max(1.0),
        ),
    )
}

fn pill_fill(mode: Mode, dark: bool) -> Retained<NSColor> {
    if mode == Mode::Error {
        if dark {
            return srgb(112.0 / 255.0, 28.0 / 255.0, 36.0 / 255.0, 0.94);
        }
        return srgb(186.0 / 255.0, 48.0 / 255.0, 58.0 / 255.0, 0.94);
    }
    // The dictation bubble stays black in both appearances.
    srgb(0.0, 0.0, 0.0, 0.94)
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

const SPINNER_BOX: f64 = 16.0;
const SPINNER_GAP: f64 = 8.0;

fn bar_row_width() -> f64 {
    BARS.len() as f64 * BAR_W + (BARS.len() as f64 - 1.0) * BAR_GAP
}

fn cluster_left(pill: NSRect, extra: f64) -> f64 {
    pill.mid().x - (bar_row_width() + extra) / 2.0
}

fn paint_bars(left: f64, mid_y: f64, level: f64, time: f64, alpha: f64) {
    NSColor::colorWithWhite_alpha(1.0, alpha).setFill();
    for (index, &(delay, height_scale)) in BARS.iter().enumerate() {
        let height = bar_height(level, height_scale, time - delay);
        let rect = NSRect::new(
            NSPoint::new(left + index as f64 * (BAR_W + BAR_GAP), mid_y - height / 2.0),
            NSSize::new(BAR_W, height),
        );
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 0.5, 0.5).fill();
    }
}

/// Shared audio scale, per-bar height share, then the 1s loop at that bar's delay.
fn bar_height(level: f64, height_scale: f64, local_time: f64) -> f64 {
    let audio_scale = 5.0 * voice_amount(level) * 0.45;
    let scale = audio_scale.max(1.0) * height_scale * wave_mul(local_time);
    let height = BAR_MIN + (BAR_MAX - BAR_MIN) * ((scale - 1.0) / 3.0);
    height.clamp(BAR_MIN, BAR_MAX)
}

/// Eight ticks on a 6 pt radius. Each fades from 0.88 to 0.15 over 1.1s, a step apart.
fn paint_spokes(center: NSPoint, time: f64) {
    for index in 0..8 {
        let delay = 0.1375 * -((7 - index) as f64);
        let t = (time - delay).rem_euclid(1.1) / 1.1;
        let alpha = 0.88 + (0.15 - 0.88) * t;
        let angle = index as f64 * std::f64::consts::FRAC_PI_4;
        let dir_x = angle.sin();
        let dir_y = angle.cos();
        let cx = center.x + dir_x * 6.0;
        let cy = center.y + dir_y * 6.0;
        let path = NSBezierPath::new();
        path.moveToPoint(NSPoint::new(cx - dir_x * 2.5, cy - dir_y * 2.5));
        path.lineToPoint(NSPoint::new(cx + dir_x * 2.5, cy + dir_y * 2.5));
        path.setLineWidth(2.0);
        path.setLineCapStyle(NSLineCapStyle::Round);
        srgb(163.0 / 255.0, 148.0 / 255.0, 199.0 / 255.0, alpha).setStroke();
        path.stroke();
    }
}

/// 1s ease-in-out loop: 1, 1.2, 1.5, 1.1, 1.3, back to 1.
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
            let eased = ease_in_out((t - t0) / span);
            return v0 + (v1 - v0) * eased;
        }
    }
    1.0
}

fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Maps this mic's speech into 0..1. A raw peak of 0.05 is already loud here.
fn voice_amount(level: f64) -> f64 {
    let span = (LEVEL_FULL - LEVEL_FLOOR).max(f64::EPSILON);
    let lifted = ((level - LEVEL_FLOOR) / span).clamp(0.0, 1.0);
    lifted.powf(0.55)
}

/// One-pixel rainbow ring. One turn takes 6 seconds.
fn paint_ring(pill: NSRect, time: f64) {
    NSGraphicsContext::saveGraphicsState_class();
    let radius = pill.size.height / 2.0;
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(pill, radius, radius).addClip();

    let mid = pill.mid();
    let reach = pill.size.width.hypot(pill.size.height);
    let spin = time.rem_euclid(6.0) / 6.0 * 360.0;
    let steps = 64.0;
    for index in 0..steps as i32 {
        let t0 = index as f64 / steps;
        let t1 = (index as f64 + 1.0) / steps;
        let (red, green, blue) = ring_color(t0);
        let path = NSBezierPath::new();
        path.moveToPoint(mid);
        path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
            mid,
            reach,
            spin + t0 * 360.0,
            spin + t1 * 360.0,
        );
        path.closePath();
        srgb(red, green, blue, 1.0).setFill();
        path.fill();
    }
    NSGraphicsContext::restoreGraphicsState_class();

    let inset = 1.5;
    let inner = NSRect::new(
        NSPoint::new(pill.origin.x + inset, pill.origin.y + inset),
        NSSize::new(
            (pill.size.width - inset * 2.0).max(1.0),
            (pill.size.height - inset * 2.0).max(1.0),
        ),
    );
    let inner_radius = inner.size.height / 2.0;
    srgb(0.0, 0.0, 0.0, 0.96).setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inner, inner_radius, inner_radius)
        .fill();
}

/// `#f0d7ff`, `#ffbcf2`, `#ff6c4c`, `#ffa946`, `#7232a6`, then back.
fn ring_color(t: f64) -> (f64, f64, f64) {
    const STOPS: [(f64, f64, f64); 6] = [
        (0.941, 0.843, 1.000),
        (1.000, 0.737, 0.949),
        (1.000, 0.424, 0.298),
        (1.000, 0.663, 0.275),
        (0.447, 0.196, 0.651),
        (0.941, 0.843, 1.000),
    ];
    let t = t.clamp(0.0, 1.0);
    let scaled = t * (STOPS.len() - 1) as f64;
    let index = (scaled.floor() as usize).min(STOPS.len() - 2);
    let frac = scaled - index as f64;
    let a = STOPS[index];
    let b = STOPS[index + 1];
    (
        a.0 + (b.0 - a.0) * frac,
        a.1 + (b.1 - a.1) * frac,
        a.2 + (b.2 - a.2) * frac,
    )
}

fn paint_error(pill: NSRect, message: &str, dark: bool) {
    if message.is_empty() {
        return;
    }
    let font = NSFont::systemFontOfSize(FONT_SIZE);
    let color = if dark {
        NSColor::colorWithWhite_alpha(1.0, 1.0)
    } else {
        NSColor::colorWithWhite_alpha(0.0, 1.0)
    };
    let attrs = text_attributes(&font, Some(&color));
    let string = NSString::from_str(message);
    let size = unsafe { string.sizeWithAttributes(Some(&attrs)) };
    let mid = pill.mid();
    let baseline = mid.y - (font.ascender() + font.descender()) / 2.0;
    unsafe {
        string.drawAtPoint_withAttributes(
            NSPoint::new(mid.x - size.width / 2.0, baseline),
            Some(&attrs),
        );
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

fn fit_line(message: &str, font: &NSFont) -> (String, f64) {
    let text = collapse_whitespace(message);
    if text.is_empty() {
        return (String::new(), WINDOW_W);
    }
    let max_text = WINDOW_W_MAX - INSET * 2.0 - TEXT_PAD * 2.0;
    if measure(&text, font) <= max_text {
        let width = (measure(&text, font) + INSET * 2.0 + TEXT_PAD * 2.0).clamp(WINDOW_W, WINDOW_W_MAX);
        return (text, width);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut best = "…".to_string();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let mut candidate: String = chars[..mid].iter().collect();
        while candidate
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
        {
            candidate.pop();
        }
        candidate.push('…');
        if measure(&candidate, font) <= max_text {
            best = candidate;
            lo = mid;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    (best, WINDOW_W_MAX)
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

fn resize_centered(window: &NSWindow, width: f64) {
    let frame = window.frame();
    if (frame.size.width - width).abs() < 0.5 && (frame.size.height - WINDOW_H).abs() < 0.5 {
        return;
    }
    let mid_x = frame.origin.x + frame.size.width / 2.0;
    let mid_y = frame.origin.y + frame.size.height / 2.0;
    window.setFrame_display(
        NSRect::new(
            NSPoint::new(mid_x - width / 2.0, mid_y - WINDOW_H / 2.0),
            NSSize::new(width, WINDOW_H),
        ),
        true,
    );
}

fn placement_screen(mtm: MainThreadMarker) -> Result<Retained<NSScreen>, BarError> {
    if let Some(screen) = screen_containing(mtm, NSEvent::mouseLocation()) {
        return Ok(screen);
    }
    if let Some(screen) = NSScreen::mainScreen(mtm) {
        return Ok(screen);
    }
    let screens = NSScreen::screens(mtm);
    if screens.count() == 0 {
        Err(BarError::NoScreen)
    } else {
        Ok(screens.objectAtIndex(0))
    }
}

fn initial_origin(mtm: MainThreadMarker, screen: &NSScreen) -> NSPoint {
    if let Some(saved) = load_origin() {
        if screen_containing(mtm, saved).is_some() {
            return saved;
        }
    }
    let visible = screen.visibleFrame();
    NSPoint::new(
        visible.origin.x + (visible.size.width - WINDOW_W) / 2.0,
        visible.origin.y + MARGIN,
    )
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

fn save_origin(origin: NSPoint) {
    let Some(path) = position_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let body = format!(
        "{{\"x\":{:.4},\"y\":{:.4}}}\n",
        origin.x, origin.y
    );
    let _ = fs::write(path, body);
}

fn load_origin() -> Option<NSPoint> {
    let text = fs::read_to_string(position_path()?).ok()?;
    let x = json_number(&text, "x")?;
    let y = json_number(&text, "y")?;
    Some(NSPoint::new(x, y))
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
