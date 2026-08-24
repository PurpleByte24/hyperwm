//! `hyperwm install-keymap` (build unit 8, architecture.md §2.1): the
//! CLI-assisted alternative to the manual System Settings > Keyboard >
//! Keyboard Shortcuts > Modifier Keys remap. Shells out to `hidutil` to
//! remap Caps Lock to whichever key `hyperkey.watch_keycode` names (the
//! same key `hyperwm-daemon` watches), applying it immediately for this
//! session *and* installing a separate `launchd` LaunchAgent
//! (`RunAtLoad = true`) that re-runs the same `hidutil` command at every
//! login -- `hidutil property --set` is a live, in-memory HID remap with
//! no persistence of its own; without the LaunchAgent, the remap would
//! silently revert on every reboot/logout and `hyperwm-daemon` would go
//! back to watching a key nothing sends anymore.
//!
//! This was a deliberate design choice, not an oversight: a simpler
//! version that only applies the remap once (no LaunchAgent) was
//! considered and rejected -- re-running `install-keymap` by hand after
//! every reboot defeats the point of a CLI-assisted remap over the manual
//! System Settings path, which persists on its own.
//!
//! # Scope: function keys only
//!
//! Only `f1`..`f20` are supported as a remap target (the same range
//! `hyperwm_macos::keycode` has real `CGKeyCode`s for -- see
//! `hyperwm-macos/src/keycode.rs`'s doc comment on `f21`-`f24`). Remapping
//! Caps Lock onto a letter, digit, or punctuation key is syntactically
//! valid config (`hyperwm_config::KeyName` doesn't restrict it) but would
//! make that key stop typing its normal character everywhere, system-wide
//! -- not something `install-keymap` should do silently. A config using
//! one of those as `hyperkey.watch_keycode` still works with the *manual*
//! remap path (architecture.md §2.1); `install-keymap` just declines to
//! automate it.
//!
//! # Manual verification
//!
//! Needs a real keyboard and a real `launchd` user session:
//!
//! - `hyperwm install-keymap` with no config file present (fresh
//!   install): should remap Caps Lock -> F18 (the documented default) and
//!   print that it applied the remap and installed the login
//!   LaunchAgent.
//! - **Immediate effect**: right after running it, press the physical
//!   Caps Lock key. It should do nothing to text input (no caps-lock
//!   toggle) -- if `hyperwm-daemon` is also running with `hyperkey.
//!   watch_keycode = "f18"`, its "hyper-active: true/false" log lines
//!   should fire on Caps Lock down/up.
//! - **Persistence**: log out and back in (or reboot). Press Caps Lock
//!   again without re-running `install-keymap` -- it should still be
//!   remapped (this is what the LaunchAgent is for; if this step fails,
//!   the remap reverted and the LaunchAgent isn't working).
//! - With a config file present setting `hyperkey.watch_keycode` to a
//!   different function key (e.g. `"f19"`): `install-keymap` should remap
//!   to *that* key instead of F18 -- confirm by checking
//!   `~/Library/LaunchAgents/com.purplebyte24.hyperwm.keymap.plist`'s
//!   `ProgramArguments` for the updated `hidutil` JSON, and by pressing
//!   the physical key that key name corresponds to.
//! - With `hyperkey.watch_keycode` set to something outside `f1`-`f20`
//!   (e.g. `"a"`): should fail with a clear error and do nothing to the
//!   keyboard, rather than silently remapping Caps Lock onto a letter.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use crate::launchd;

pub const LABEL: &str = "com.purplebyte24.hyperwm.keymap";

/// USB HID Usage Page for Keyboard/Keypad (0x07), left-shifted into the
/// 64-bit "usage" form `hidutil`'s `UserKeyMapping` expects
/// (`(page << 32) | usage_id`).
const KEYBOARD_PAGE: u64 = 0x07 << 32;
const CAPS_LOCK_USAGE_ID: u64 = 0x39;

/// HID usage IDs for F1-F20 on the Keyboard/Keypad page. F13-F20 (the
/// range architecture.md §2.1 recommends, since no physical Mac keyboard
/// has them) sit at 0x68-0x6F; F1-F12 at 0x3A-0x45, included too since
/// they're still valid, if unusual, `hyperkey.watch_keycode` choices.
fn function_key_usage_id(key_name: &str) -> Option<u64> {
    let n: u32 = key_name.strip_prefix('f')?.parse().ok()?;
    match n {
        1..=12 => Some(0x3A + u64::from(n - 1)),
        13..=20 => Some(0x68 + u64::from(n - 13)),
        _ => None,
    }
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "hyperwm install-keymap: $HOME is not set".to_string())
}

/// The key to remap Caps Lock onto: `hyperkey.watch_keycode` from the
/// user's config if one loads successfully, else the documented default
/// (`"f18"`, matching examples/config.toml) with a printed note -- so
/// `install-keymap` works before a config has ever been created, which is
/// a normal first-run order of operations (remap the key, *then* write a
/// config that uses it).
fn target_key_name() -> String {
    match hyperwm_config::default_config_path() {
        Some(path) => match hyperwm_config::load(&path) {
            Ok(config) => config.hyperkey.watch_keycode.as_str().to_string(),
            Err(err) => {
                println!(
                    "hyperwm install-keymap: config at {} doesn't parse ({err}) -- defaulting \
                     to \"f18\" (fix the config and re-run install-keymap to target its \
                     hyperkey.watch_keycode instead)",
                    path.display()
                );
                "f18".to_string()
            }
        },
        None => {
            println!(
                "hyperwm install-keymap: no config file found yet -- defaulting to \"f18\" \
                 (the convention used by examples/config.toml)"
            );
            "f18".to_string()
        }
    }
}

fn hidutil_json(target_usage_id: u64) -> String {
    format!(
        "{{\"UserKeyMapping\":[{{\"HIDKeyboardModifierMappingSrc\":0x{:X},\
         \"HIDKeyboardModifierMappingDst\":0x{:X}}}]}}",
        KEYBOARD_PAGE + CAPS_LOCK_USAGE_ID,
        KEYBOARD_PAGE + target_usage_id,
    )
}

pub fn run(_args: &[String]) -> ExitCode {
    let key_name = target_key_name();
    let Some(usage_id) = function_key_usage_id(&key_name) else {
        eprintln!(
            "hyperwm install-keymap: hyperkey.watch_keycode \"{key_name}\" isn't a function \
             key (f1-f20) -- install-keymap only automates remapping Caps Lock onto a spare \
             function key. Remap manually instead: System Settings > Keyboard > Keyboard \
             Shortcuts > Modifier Keys (architecture.md §2.1)."
        );
        return ExitCode::FAILURE;
    };
    let json = hidutil_json(usage_id);

    match apply_and_persist(&key_name, &json) {
        Ok(()) => {
            println!(
                "hyperwm install-keymap: Caps Lock remapped to \"{key_name}\" and applied \
                 immediately; a login item ({LABEL}) will reapply it automatically on every \
                 future login/reboot"
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn apply_and_persist(key_name: &str, json: &str) -> Result<(), String> {
    apply_now(json)?;

    let home = home_dir()?;
    let plist_path = launchd::write_plist(
        &home,
        LABEL,
        &[
            "/usr/bin/hidutil".to_string(),
            "property".to_string(),
            "--set".to_string(),
            json.to_string(),
        ],
        true,
        std::path::Path::new("/dev/null"),
        std::path::Path::new("/dev/null"),
    )
    .map_err(|err| format!("hyperwm install-keymap: couldn't write launchd plist: {err}"))?;

    // Re-running install-keymap (e.g. after changing hyperkey.
    // watch_keycode) should update an already-installed LaunchAgent's
    // command in place, not fail because the label's already bootstrapped
    // -- kickstart the existing job so login-time behavior matches what
    // was just applied above; bootstrap fresh only the first time.
    if launchd::is_bootstrapped(LABEL).map_err(|e| e.to_string())? {
        launchd::kickstart(LABEL, true)
    } else {
        launchd::bootstrap(LABEL, &plist_path)
    }
    .map_err(|err| format!("hyperwm install-keymap: applied to this session, but couldn't install the login persistence agent: {err} (re-run install-keymap, or the remap won't survive a reboot for \"{key_name}\")"))
}

fn apply_now(json: &str) -> Result<(), String> {
    let output = Command::new("hidutil")
        .args(["property", "--set", json])
        .output()
        .map_err(|err| format!("hyperwm install-keymap: couldn't run hidutil: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("hyperwm install-keymap: hidutil failed: {}", stderr.trim()))
    }
}
