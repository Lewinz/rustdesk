// Client-side fullscreen keyboard grab for macOS.
//
// While the grab is active, a native CGEventTap (see macos.mm) forwards every
// key event into the current session and swallows it locally, so system
// shortcuts (Spotlight, Mission Control, ...) reach the remote host instead of
// this Mac. Cmd+Tab is the deliberate exception: macOS resolves it in
// WindowServer, so the local app switch wins and the grab is released when
// this window loses focus (the peer then receives the held-key releases).

use std::sync::Mutex;
use std::time::SystemTime;

use hbb_common::log;

use crate::{
    flutter::{sessions, FlutterHandler},
    flutter_ffi::SessionID,
    keyboard::client::process_event_with_session,
    ui_session_interface::Session,
};

extern "C" {
    fn MacOSKeyboardCaptureStart() -> bool;
    fn MacOSKeyboardCaptureStop();
    fn MacOSKeyboardCaptureRecordStart() -> bool;
    fn MacOSKeyboardCaptureRecordStop();
}

struct CaptureCtx {
    session_id: SessionID,
    keyboard_mode: String,
}

static CTX: Mutex<Option<CaptureCtx>> = Mutex::new(None);
static DOWN_KEYS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

// Shortcut-recording mode: the UI asks for a combo by starting the record
// tap; every key is swallowed and the combo is pushed to the UI channel.
static RECORDING: Mutex<Option<String>> = Mutex::new(None);

pub fn start_recording(channel: String) {
    if unsafe { !MacOSKeyboardCaptureRecordStart() } {
        log::error!("[macos_kb_capture] failed to start record tap");
        return;
    }
    *RECORDING.lock().unwrap() = Some(channel);
    log::debug!("[macos_kb_capture] recording started");
}

pub fn stop_recording() {
    if RECORDING.lock().unwrap().take().is_some() {
        unsafe { MacOSKeyboardCaptureRecordStop() };
        log::debug!("[macos_kb_capture] recording stopped");
    }
}

const CMD_FLAG: u64 = 1 << 20;
const ALT_FLAG: u64 = 1 << 19;
const CTRL_FLAG: u64 = 1 << 18;
const SHIFT_FLAG: u64 = 1 << 17;

// macOS virtual keycode -> combo token for the recordable main keys.
// Letters/digits are not contiguous in the ANSI keycode space, so map
// explicitly (this is the same table parse_combo uses in reverse).
const KEYCODE_TO_TOKEN: &[(i64, &str)] = &[
    (0, "a"), (1, "s"), (2, "d"), (3, "f"), (4, "h"), (5, "g"), (6, "z"),
    (7, "x"), (8, "c"), (9, "v"), (11, "b"), (12, "q"), (13, "w"), (14, "e"),
    (15, "r"), (16, "y"), (17, "t"), (18, "1"), (19, "2"), (20, "3"), (21, "4"),
    (22, "6"), (23, "5"), (25, "9"), (26, "7"), (28, "8"), (29, "0"), (31, "o"),
    (32, "u"), (34, "i"), (35, "p"), (37, "l"), (38, "j"), (40, "k"), (45, "n"),
    (46, "m"),
];

fn main_key_token(keycode: i64) -> Option<String> {
    let token = match keycode {
        48 => "tab",
        49 => "space",
        51 => "del",
        36 => "enter",
        126 => "up",
        125 => "down",
        123 => "left",
        124 => "right",
        115 => "home",
        119 => "end",
        116 => "pageup",
        121 => "pagedown",
        122 => "f1",
        120 => "f2",
        99 => "f3",
        118 => "f4",
        96 => "f5",
        97 => "f6",
        98 => "f7",
        100 => "f8",
        101 => "f9",
        109 => "f10",
        103 => "f11",
        111 => "f12",
        _ => {
            return KEYCODE_TO_TOKEN
                .iter()
                .find(|(code, _)| *code == keycode)
                .map(|(_, token)| token.to_string());
        }
    };
    Some(token.to_owned())
}

#[no_mangle]
pub extern "C" fn MacOSKeyboardCaptureRecordEvent(keycode: i64, down: bool, flags: u64) {
    if !down {
        return;
    }
    let Some(channel) = RECORDING.lock().unwrap().clone() else {
        return;
    };
    // Plain Esc cancels recording.
    if keycode == 53 && (flags & (CMD_FLAG | ALT_FLAG | CTRL_FLAG | SHIFT_FLAG)) == 0 {
        crate::flutter::push_global_event(&channel, r#"{"cancel":true}"#.to_owned());
        stop_recording();
        return;
    }
    let mut parts = Vec::new();
    if flags & CMD_FLAG != 0 {
        parts.push("cmd");
    }
    if flags & ALT_FLAG != 0 {
        parts.push("alt");
    }
    if flags & CTRL_FLAG != 0 {
        parts.push("ctrl");
    }
    if flags & SHIFT_FLAG != 0 {
        parts.push("shift");
    }
    let main = match main_key_token(keycode) {
        Some(t) => t,
        None => return, // modifier key itself or an unrecognized key
    };
    let combo = if parts.is_empty() {
        main
    } else {
        format!("{}+{}", parts.join("+"), main)
    };
    let msg = format!(r#"{{"combo":"{}"}}"#, combo);
    crate::flutter::push_global_event(&channel, msg);
    stop_recording();
}

// Whitelisted combos that pass through to the local system while the grab is
// active. Each entry is (modifier flags mask, keycode); an event matches when
// its flags contain every mask bit and its keycode equals the entry's.
static WHITELIST: Mutex<Vec<(u64, i64)>> = Mutex::new(Vec::new());

pub fn set_whitelist(combos: &[String]) {
    let parsed: Vec<(u64, i64)> = combos.iter().filter_map(|c| parse_combo(c)).collect();
    if parsed.len() != combos.len() {
        log::warn!(
            "[macos_kb_capture] {} whitelist entry(ies) could not be parsed",
            combos.len() - parsed.len()
        );
    }
    *WHITELIST.lock().unwrap() = parsed;
    log::debug!(
        "[macos_kb_capture] whitelist updated: {} combo(s)",
        WHITELIST.lock().unwrap().len()
    );
}

// Parse "cmd+shift+tab"-style combo strings. Unknown modifiers or keys make
// the whole entry invalid (returned as None), so a typo never silently
// weakens interception.
fn parse_combo(s: &str) -> Option<(u64, i64)> {
    let mut mask: u64 = 0;
    let mut keycode: Option<i64> = None;
    // kCGEventFlagMaskCommand/Alternate/Control/Shift
    const CMD: u64 = 1 << 20;
    const ALT: u64 = 1 << 19;
    const CTRL: u64 = 1 << 18;
    const SHIFT: u64 = 1 << 17;
    // macOS virtual keycodes for function keys (F1-F12).
    const F_KEYS: [i64; 12] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111];
    for token in s.to_lowercase().split('+').map(str::trim) {
        if token.is_empty() {
            return None;
        }
        match token {
            "cmd" | "command" => mask |= CMD,
            "alt" | "option" => mask |= ALT,
            "ctrl" | "control" => mask |= CTRL,
            "shift" => mask |= SHIFT,
            "tab" => keycode = Some(48),
            "space" => keycode = Some(49),
            "esc" | "escape" => keycode = Some(53),
            "del" | "delete" | "backspace" => keycode = Some(51),
            "enter" | "return" => keycode = Some(36),
            "up" => keycode = Some(126),
            "down" => keycode = Some(125),
            "left" => keycode = Some(123),
            "right" => keycode = Some(124),
            "home" => keycode = Some(115),
            "end" => keycode = Some(119),
            "pageup" => keycode = Some(116),
            "pagedown" => keycode = Some(121),
            _ => {
                if let Ok(n) = token.parse::<i64>() {
                    keycode = Some(n);
                } else if token.len() == 1 {
                    keycode = KEYCODE_TO_TOKEN
                        .iter()
                        .find(|(_, t)| *t == token)
                        .map(|(code, _)| *code);
                    if keycode.is_none() {
                        return None;
                    }
                } else if token.starts_with('f') {
                    if let Ok(n) = token[1..].parse::<i64>() {
                        if (1..=12).contains(&n) {
                            keycode = Some(F_KEYS[(n - 1) as usize]);
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
        }
    }
    Some((mask, keycode?))
}

#[no_mangle]
pub extern "C" fn MacOSKeyboardCaptureIsWhitelisted(flags: u64, keycode: i64) -> bool {
    WHITELIST
        .lock()
        .unwrap()
        .iter()
        .any(|(mask, code)| *code == keycode && (flags & mask) == *mask)
}

pub fn set_active(session_id: SessionID, active: bool) {
    if active {
        let Some(session) = sessions::get_session_by_session_id(&session_id) else {
            log::error!("[macos_kb_capture] set_active: session not found");
            return;
        };
        if unsafe { !MacOSKeyboardCaptureStart() } {
            log::error!("[macos_kb_capture] failed to start event tap");
            return;
        }
        let keyboard_mode = session.get_keyboard_mode();
        *CTX.lock().unwrap() = Some(CaptureCtx {
            session_id,
            keyboard_mode,
        });
        log::debug!("[macos_kb_capture] started");
    } else {
        // Only the owning window may release the tap; a non-fullscreen window
        // syncing its own (inactive) state must not kill a fullscreen grab.
        let is_owner = CTX
            .lock()
            .unwrap()
            .as_ref()
            .map(|ctx| ctx.session_id == session_id)
            .unwrap_or(false);
        if is_owner {
            set_inactive();
        }
    }
}

fn set_inactive() {
    let ctx = CTX.lock().unwrap().take();
    unsafe { MacOSKeyboardCaptureStop() };
    if let Some(ctx) = ctx {
        // The peer may hold modifiers (e.g. Cmd was pressed when Cmd+Tab
        // switched away). Release them so the peer does not keep them stuck.
        if let Some(session) = sessions::get_session_by_session_id(&ctx.session_id) {
            let held: Vec<i64> = std::mem::take(&mut *DOWN_KEYS.lock().unwrap());
            for keycode in held {
                forward(&session, &ctx.keyboard_mode, keycode, false);
            }
        }
        log::debug!("[macos_kb_capture] stopped");
    }
    DOWN_KEYS.lock().unwrap().clear();
}

#[no_mangle]
pub extern "C" fn MacOSKeyboardCaptureOnEvent(keycode: i64, down: bool) {
    let ctx = CTX.lock().unwrap();
    let Some(ctx) = ctx.as_ref() else { return };
    let Some(session) = sessions::get_session_by_session_id(&ctx.session_id) else {
        return;
    };
    if down {
        let mut keys = DOWN_KEYS.lock().unwrap();
        if !keys.contains(&keycode) {
            keys.push(keycode);
        }
    } else {
        DOWN_KEYS.lock().unwrap().retain(|k| *k != keycode);
    }
    forward(&session, &ctx.keyboard_mode, keycode, down);
}

fn forward(session: &Session<FlutterHandler>, keyboard_mode: &str, keycode: i64, down: bool) {
    let key = rdev::key_from_code(keycode as _) as rdev::Key;
    let event_type = if down {
        rdev::EventType::KeyPress(key)
    } else {
        rdev::EventType::KeyRelease(key)
    };
    let event = rdev::Event {
        time: SystemTime::now(),
        unicode: None,
        platform_code: keycode as u32,
        position_code: keycode as _,
        event_type,
        usb_hid: 0,
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        extra_data: 0,
    };
    process_event_with_session(keyboard_mode, &event, None, session);
}
