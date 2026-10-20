//! Cycles the pill. Do not run this from `cargo check`; `app.run` blocks.

use std::thread;
use std::time::{Duration, Instant};

use iki_bar::{BarPhase, BarController};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

fn main() {
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let bar = BarController::install().expect("iki bar");
    thread::spawn(move || {
        let started = Instant::now();
        loop {
            let t = started.elapsed().as_secs_f64() % 12.0;
            let phase = if t < 1.2 {
                BarPhase::Hidden
            } else if t < 5.2 {
                let u = (t - 1.2) / 4.0;
                let level = ((u * std::f64::consts::TAU).sin() * 0.5 + 0.5) as f32;
                BarPhase::Recording { level }
            } else if t < 8.2 {
                BarPhase::Processing
            } else if t < 11.2 {
                BarPhase::Error {
                    title: "No audio received".into(),
                    body: "We're not hearing anything. There may be an issue with your mic or permissions.".into(),
                }
            } else {
                BarPhase::Hidden
            };
            bar.set_phase(phase);
            thread::sleep(Duration::from_millis(33));
        }
    });
    app.run();
}
