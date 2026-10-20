//! Hold the Globe key to dictate. Releasing it pastes the Soniox transcript.

mod dashboard;
mod history;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use iki_bar::{BarPhase, BarController};
use globe_input::{HoldEdge, HoldWatch};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2::msg_send;
use objc2::sel;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem, NSStatusBar,
    NSVariableStatusItemLength,
};
use objc2_foundation::NSString;
use soniox_audio::{DictationConfig, DictationHandle};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn main() {
    let mtm = MainThreadMarker::new().expect("Iki must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let _status = install_status_item(&app, mtm);

    let bar = BarController::install().unwrap_or_else(|error| {
        eprintln!("iki bar: {error}");
        std::process::exit(1);
    });

    let (tx, rx) = unbounded_channel();
    let watch = {
        let tx = tx.clone();
        HoldWatch::install(move |edge| {
            let _ = tx.send(edge);
        })
    };
    if let Err(error) = &watch {
        bar.set_phase(BarPhase::Error {
            message: error.to_string(),
        });
        eprintln!("globe key: {error}");
        if matches!(error, globe_input::InputError::EventTapUnavailable) {
            open_accessibility_settings();
        }
    }

    let api_key = load_api_key();
    if api_key.is_empty() {
        bar.set_phase(BarPhase::Error {
            message: "Set SONIOX_API_KEY".into(),
        });
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime.spawn(dictation_loop(rx, bar, api_key));

    let _watch = watch.ok();
    app.run();
}

fn install_status_item(
    app: &NSApplication,
    mtm: MainThreadMarker,
) -> objc2::rc::Retained<objc2_app_kit::NSStatusItem> {
    let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status.button(mtm) {
        let title = NSString::from_str("Iki");
        let _: () = unsafe { msg_send![&*button, setTitle: &*title] };
    }

    let menu = NSMenu::new(mtm);
    dashboard::add_menu_item(&menu, mtm);
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit Iki"),
            Some(sel!(terminate:)),
            &NSString::from_str(""),
        )
    };
    unsafe { quit.setTarget(Some(app)) };
    menu.addItem(&quit);
    status.setMenu(Some(&menu));
    let _keep_menu = Box::leak(Box::new(menu));
    status
}

fn load_api_key() -> String {
    if let Ok(key) = std::env::var("SONIOX_API_KEY") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return key;
        }
    }
    let from_file = std::fs::read_to_string(key_file())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !from_file.is_empty() {
        return from_file;
    }
    fnox_key().unwrap_or_default()
}

/// `fnox get SONIOX_API_KEY`. Finder launches have a short PATH, so also try the mise shim.
fn fnox_key() -> Option<String> {
    let fnox = fnox_binary()?;
    let output = Command::new(fnox)
        .args(["get", "SONIOX_API_KEY", "--non-interactive"])
        .env("FNOX_NON_INTERACTIVE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let key = String::from_utf8(output.stdout).ok()?;
    let key = key.trim().to_string();
    if key.is_empty() { None } else { Some(key) }
}

fn fnox_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("fnox");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var_os("HOME")?;
    let home = PathBuf::from(home);
    [
        home.join(".local/share/mise/shims/fnox"),
        home.join(".local/share/mise/installs/fnox/latest/.mise-bins/fnox"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn key_file() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/Application Support/Iki/soniox_api_key")
}

async fn dictation_loop(
    mut rx: UnboundedReceiver<HoldEdge>,
    bar: BarController,
    api_key: String,
) {
    'hold: loop {
        match rx.recv().await {
            Some(HoldEdge::Down) => {}
            Some(HoldEdge::Up) => continue,
            None => return,
        }

        if api_key.is_empty() {
            show_error(&bar, "Set SONIOX_API_KEY").await;
            continue;
        }

        play_dictation("start");
        bar.set_phase(BarPhase::Recording { level: 0.0 });

        let start = DictationHandle::start(DictationConfig {
            api_key: api_key.clone(),
        });
        tokio::pin!(start);
        let mut released = false;
        let handle = loop {
            tokio::select! {
                started = &mut start => break started,
                edge = rx.recv() => match edge {
                    Some(HoldEdge::Up) => released = true,
                    Some(HoldEdge::Down) => {}
                    None => return,
                },
            }
        };

        let mut handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                show_error(&bar, &error.to_string()).await;
                continue;
            }
        };

        // The mic is already playing. Read it now, instead of waiting for the socket.
        if !released {
            bar.set_phase(BarPhase::Recording { level: handle.rms() });
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(16)) => {
                        if let Some(error) = handle.connect_error() {
                            let _ = handle.finish().await;
                            play_dictation("stop");
                            show_error(&bar, &error.to_string()).await;
                            continue 'hold;
                        }
                        bar.set_phase(BarPhase::Recording { level: handle.rms() });
                    }
                    edge = rx.recv() => match edge {
                        Some(HoldEdge::Up) | None => break,
                        Some(HoldEdge::Down) => {}
                    },
                }
            }
        }

        if let Some(error) = handle.connect_error() {
            let _ = handle.finish().await;
            play_dictation("stop");
            show_error(&bar, &error.to_string()).await;
            continue;
        }

        play_dictation("stop");
        bar.set_phase(BarPhase::Processing);
        match handle.finish().await {
            Ok(text) => {
                let text = text.trim();
                if text.is_empty() {
                    bar.set_phase(BarPhase::Hidden);
                    continue;
                }
                history::record(text);
                match globe_input::paste_text(text) {
                    Ok(()) => {
                        bar.set_phase(BarPhase::Hidden);
                    }
                    Err(error) => show_error(&bar, &error.to_string()).await,
                }
            }
            Err(error) => show_error(&bar, &error.to_string()).await,
        }
    }
}

async fn show_error(bar: &BarController, message: &str) {
    bar.set_phase(BarPhase::Error {
        message: message.to_string(),
    });
    tokio::time::sleep(Duration::from_secs(4)).await;
    bar.set_phase(BarPhase::Hidden);
}

fn open_accessibility_settings() {
    let urls = [
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    ];
    for url in urls {
        if Command::new("open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
        {
            return;
        }
    }
}

/// `assets/sounds` is copied into the app bundle by `scripts/bundle.sh`.
fn play_dictation(which: &str) {
    let Some(path) = dictation_sound(which) else {
        return;
    };
    let _ = Command::new("afplay")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn dictation_sound(which: &str) -> Option<PathBuf> {
    let name = format!("dictation-{which}.wav");
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("../Resources/sounds").join(&name);
            if bundled.is_file() {
                return Some(bundled);
            }
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../assets/sounds")
        .join(name);
    dev.is_file().then_some(dev)
}
