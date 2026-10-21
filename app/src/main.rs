//! Hold the Globe key to dictate. Releasing it pastes the Soniox transcript.

mod dashboard;
mod dictionary;
mod dictionary_ui;
mod history;
mod languages;
mod storage;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use iki_bar::{BarPhase, BarController};
use globe_input::{HoldEdge, HoldWatch};
use objc2::msg_send;
use objc2::sel;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSMenu, NSMenuItem, NSStatusBar,
    NSVariableStatusItemLength,
};
use objc2_foundation::NSString;
use soniox_audio::{DictationConfig, DictationError, DictationHandle};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

fn main() {
    let mtm = MainThreadMarker::new().expect("Iki must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    install_edit_menu(&app, mtm);
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
        let notice = notice_for_input(error);
        bar.set_phase(BarPhase::Error {
            title: notice.title,
            body: notice.body,
        });
        eprintln!("globe key: {error}");
        if matches!(error, globe_input::InputError::EventTapUnavailable) {
            open_accessibility_settings();
        }
    }

    let api_key = load_api_key();
    if api_key.is_empty() {
        let notice = missing_key_notice();
        bar.set_phase(BarPhase::Error {
            title: notice.title,
            body: notice.body,
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
    dictionary_ui::add_menu_item(&menu, mtm);
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

/// The app has no visible menu bar, but its main menu still supplies the
/// Cmd-X/C/V/A/Z key equivalents that text fields rely on.
fn install_edit_menu(app: &NSApplication, mtm: MainThreadMarker) {
    let main_menu = NSMenu::new(mtm);
    main_menu.addItem(&NSMenuItem::new(mtm));
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Edit"));
    for (title, action, key) in [
        ("Undo", sel!(undo:), "z"),
        ("Redo", sel!(redo:), "Z"),
        ("Cut", sel!(cut:), "x"),
        ("Copy", sel!(copy:), "c"),
        ("Paste", sel!(paste:), "v"),
        ("Select All", sel!(selectAll:), "a"),
    ] {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        edit.addItem(&item);
    }
    let edit_item = NSMenuItem::new(mtm);
    edit_item.setSubmenu(Some(&edit));
    main_menu.addItem(&edit_item);
    app.setMainMenu(Some(&main_menu));
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
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
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
    storage::app_support_file("soniox_api_key")
}

/// A release sooner than this is a tap. It may have no audio at all, which
/// Soniox rejects as "No audio received", so it ends the take silently.
const MIN_HOLD: Duration = Duration::from_millis(250);

/// Hold edges plus whether a press arrived that no take has used yet.
struct Keys {
    rx: UnboundedReceiver<HoldEdge>,
    pressed: bool,
}

impl Keys {
    async fn recv(&mut self) -> Option<HoldEdge> {
        let edge = self.rx.recv().await?;
        self.pressed = edge == HoldEdge::Down;
        Some(edge)
    }

    /// Drops the edges that queued while a take or notice was busy, keeping
    /// only whether the last of them was a press. `false` once the tap is gone.
    fn drain(&mut self) -> bool {
        loop {
            match self.rx.try_recv() {
                Ok(edge) => self.pressed = edge == HoldEdge::Down,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }
}

async fn dictation_loop(rx: UnboundedReceiver<HoldEdge>, bar: BarController, api_key: String) {
    let mut keys = Keys { rx, pressed: false };
    'hold: loop {
        // Presses made during the last take must not replay one by one.
        if !keys.drain() {
            return;
        }
        if !keys.pressed {
            match keys.recv().await {
                Some(HoldEdge::Down) => {}
                Some(HoldEdge::Up) => continue,
                None => return,
            }
        }
        // This take uses the press. A hold that outlasts the take, or the
        // notice it ends in, must not start another take by itself.
        keys.pressed = false;
        let mut pressed_at = Instant::now();

        if api_key.is_empty() {
            show_notice(&bar, missing_key_notice(), &mut keys).await;
            continue;
        }

        play_dictation("start");
        bar.set_phase(BarPhase::Recording { level: 0.0 });

        let start = DictationHandle::start(DictationConfig {
            api_key: api_key.clone(),
            terms: dictionary::recognition_terms(),
            languages: languages::codes(),
        });
        tokio::pin!(start);
        let mut held_for = None;
        let handle = loop {
            tokio::select! {
                started = &mut start => break started,
                edge = keys.recv() => match edge {
                    Some(HoldEdge::Up) => held_for = Some(pressed_at.elapsed()),
                    // Pressed again while the mic opens: this take is the new hold.
                    Some(HoldEdge::Down) => {
                        keys.pressed = false;
                        held_for = None;
                        pressed_at = Instant::now();
                    }
                    None => return,
                },
            }
        };

        let mut handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                show_dictation_error(&bar, error, &mut keys).await;
                continue;
            }
        };

        // The mic is already playing. Read it now, instead of waiting for the socket.
        if held_for.is_none() {
            bar.set_phase(BarPhase::Recording {
                level: handle.rms(),
            });
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(16)) => {
                        if let Some(error) = handle.connect_error() {
                            let _ = handle.finish().await;
                            play_dictation("stop");
                            show_dictation_error(&bar, error, &mut keys).await;
                            continue 'hold;
                        }
                        bar.set_phase(BarPhase::Recording { level: handle.rms() });
                    }
                    edge = keys.recv() => match edge {
                        Some(HoldEdge::Up) | None => {
                            held_for = Some(pressed_at.elapsed());
                            break;
                        }
                        Some(HoldEdge::Down) => {}
                    },
                }
            }
        }

        if held_for.is_some_and(|held| held < MIN_HOLD) {
            drop(handle);
            bar.set_phase(BarPhase::Hidden);
            continue;
        }

        if let Some(error) = handle.connect_error() {
            let _ = handle.finish().await;
            play_dictation("stop");
            show_dictation_error(&bar, error, &mut keys).await;
            continue;
        }

        play_dictation("stop");
        bar.set_phase(BarPhase::Processing);
        match handle.finish().await {
            Ok(text) => {
                let original = text.trim();
                if original.is_empty() {
                    bar.set_phase(BarPhase::Hidden);
                    continue;
                }
                let text = dictionary::process_transcript(original);
                history::record(&text);
                match globe_input::paste_text(&text) {
                    Ok(()) => {
                        bar.set_phase(BarPhase::Hidden);
                    }
                    Err(error) => {
                        eprintln!("iki: {error}");
                        show_notice(&bar, notice_for_input(&error), &mut keys).await;
                    }
                }
            }
            Err(error) => show_dictation_error(&bar, error, &mut keys).await,
        }
    }
}

struct Notice {
    title: String,
    body: String,
}

fn notice_for_dictation(error: &DictationError) -> Option<Notice> {
    Some(match error {
        DictationError::Cancelled => return None,
        DictationError::MissingApiKey => missing_key_notice(),
        DictationError::Microphone(_) => Notice {
            title: "Microphone isn't available".into(),
            body: "Check that it's connected, and that Iki is allowed to use it.".into(),
        },
        // The key travels in the handshake header, so a rejected key fails
        // the upgrade (HTTP 401/403) and arrives here as a network error.
        DictationError::Network(detail) if is_auth_failure(&detail.to_ascii_lowercase()) => {
            rejected_key_notice()
        }
        DictationError::Network(_) => Notice {
            title: "Can't reach Soniox".into(),
            body: "Check your connection and try again.".into(),
        },
        DictationError::Protocol(detail) => protocol_notice(detail),
    })
}

fn protocol_notice(detail: &str) -> Notice {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("no audio") {
        Notice {
            title: "No audio received".into(),
            body: "We're not hearing anything. There may be an issue with your mic or permissions."
                .into(),
        }
    } else if lower.contains("context is too long") {
        Notice {
            title: "Dictionary is too long".into(),
            body: "Star the names you use most, then try again.".into(),
        }
    } else if is_auth_failure(&lower) {
        rejected_key_notice()
    } else {
        Notice {
            title: "Transcription didn't finish".into(),
            body: "Try again in a moment.".into(),
        }
    }
}

fn is_auth_failure(detail: &str) -> bool {
    detail.contains("api key")
        || detail.contains("api_key")
        || detail.contains("unauthorized")
        || detail.contains("authentication")
        || detail.contains("forbidden")
        // A status code, not any number that contains the digits (`4013`, `14012 tokens`).
        || detail
            .split(|c: char| !c.is_ascii_digit())
            .any(|number| number == "401" || number == "403")
}

fn rejected_key_notice() -> Notice {
    Notice {
        title: "Soniox rejected the key".into(),
        body: "Check SONIOX_API_KEY, or the key in Application Support, then relaunch.".into(),
    }
}

fn notice_for_input(error: &globe_input::InputError) -> Notice {
    match error {
        globe_input::InputError::Clipboard(_)
        | globe_input::InputError::EventSource
        | globe_input::InputError::KeyboardEvent => Notice {
            title: "Couldn't paste".into(),
            body: "Click the field you want, then dictate again.".into(),
        },
        globe_input::InputError::EventTapUnavailable | globe_input::InputError::RunLoopSource => {
            Notice {
                title: "Allow Iki in Accessibility".into(),
                body: "Turn on Iki under Privacy & Security, Device Control and Data Access, then relaunch.".into(),
            }
        }
    }
}

fn missing_key_notice() -> Notice {
    Notice {
        title: "Add your Soniox key".into(),
        body: "Set SONIOX_API_KEY, or put the key in Iki's Application Support folder, then relaunch.".into(),
    }
}

async fn show_dictation_error(bar: &BarController, error: DictationError, keys: &mut Keys) {
    match notice_for_dictation(&error) {
        Some(notice) => {
            eprintln!("iki: {error}");
            show_notice(bar, notice, keys).await;
        }
        None => bar.set_phase(BarPhase::Hidden),
    }
}

/// Shows the notice for four seconds. Pressing the key dismisses it and
/// starts the next take, since `keys` then reports a pending press.
async fn show_notice(bar: &BarController, notice: Notice, keys: &mut Keys) {
    bar.set_phase(BarPhase::Error {
        title: notice.title,
        body: notice.body,
    });
    let shown = tokio::time::sleep(Duration::from_secs(4));
    tokio::pin!(shown);
    loop {
        tokio::select! {
            _ = &mut shown => break,
            edge = keys.recv() => match edge {
                Some(HoldEdge::Down) | None => break,
                Some(HoldEdge::Up) => {}
            },
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{notice_for_dictation, notice_for_input};
    use soniox_audio::DictationError;

    #[test]
    fn no_audio_is_a_plain_sentence() {
        let notice = notice_for_dictation(&DictationError::Protocol(
            "invalid_request: No audio received.".into(),
        ))
        .expect("shown");
        assert_eq!(notice.title, "No audio received");
        assert!(notice.body.contains("mic"));
        let shown = format!("{} {}", notice.title, notice.body).to_ascii_lowercase();
        assert!(!shown.contains("protocol"));
        assert!(!shown.contains("invalid_request"));
    }

    #[test]
    fn cancelled_dictation_has_no_notice() {
        assert!(notice_for_dictation(&DictationError::Cancelled).is_none());
    }

    #[test]
    fn context_limit_talks_about_the_dictionary() {
        let notice = notice_for_dictation(&DictationError::Protocol(
            "invalid_request: Context is too long: 9001 tokens, the maximum is 8000 tokens.".into(),
        ))
        .expect("shown");
        assert_eq!(notice.title, "Dictionary is too long");
        assert!(!notice.body.to_ascii_lowercase().contains("token"));
    }

    #[test]
    fn rejected_key_hides_the_status_code() {
        let notice = notice_for_dictation(&DictationError::Protocol(
            "authentication_error: invalid api key".into(),
        ))
        .expect("shown");
        assert_eq!(notice.title, "Soniox rejected the key");
        assert!(!notice.body.to_ascii_lowercase().contains("authentication"));
    }

    #[test]
    fn a_rejected_handshake_is_a_key_problem_not_a_connection_problem() {
        let notice = notice_for_dictation(&DictationError::Network(
            "HTTP error: 401 Unauthorized".into(),
        ))
        .expect("shown");
        assert_eq!(notice.title, "Soniox rejected the key");
        let notice = notice_for_dictation(&DictationError::Network(
            "IO error: Connection reset by peer".into(),
        ))
        .expect("shown");
        assert_eq!(notice.title, "Can't reach Soniox");
    }

    #[test]
    fn a_number_containing_401_is_not_an_auth_failure() {
        let notice = notice_for_dictation(&DictationError::Protocol(
            "internal_error: Cannot continue request (code 4013)".into(),
        ))
        .expect("shown");
        assert_eq!(notice.title, "Transcription didn't finish");
    }

    #[test]
    fn accessibility_notice_names_the_setting() {
        let notice = notice_for_input(&globe_input::InputError::EventTapUnavailable);
        assert_eq!(notice.title, "Allow Iki in Accessibility");
        assert!(notice.body.contains("Device Control and Data Access"));
    }
}
