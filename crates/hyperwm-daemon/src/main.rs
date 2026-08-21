//! Wires the four subsystems together (architecture.md §1): loads config,
//! checks permissions, then attaches the hyperkey watcher (unit 3), an
//! `AXObserver` per already-running app (unit 4, via `lifecycle`) and an
//! `NSWorkspace` watcher for apps launched afterward, all to *one* shared
//! `CFRunLoop` -- and only then makes the single blocking call that runs
//! it. Everything from here on is event-driven: the Event Router
//! (`router`) resolves a keypress to an `Action`, the Action Engine
//! (`state::DaemonState`) carries it out against real `hyperwm-core` tree
//! state and real AX window calls.
//!
//! Script keybinds and CLI reload (`hyperwm reload`/`status`/`verify`) are
//! later build units (6-7), not wired here.
//!
//! # Manual verification
//!
//! This crate depends on a real window server, real Accessibility/Input
//! Monitoring grants, and real hyperkey presses, so it's not practical to
//! exercise from `cargo test` (same reasoning as `hyperwm-macos`'s unit 4
//! examples). Verify by running the daemon itself:
//!
//! ```text
//! cargo run -p hyperwm-daemon
//! ```
//!
//! against `examples/config.toml` copied to `~/.config/hyperwm/config.toml`
//! (or your own, matching its schema), with a few ordinary app windows
//! already open. First run triggers the Accessibility/Input Monitoring
//! prompts; grant both and re-run. Then, with several windows open:
//!
//! - `hyper+f` on a tiled window floats it in place (no jump); on a
//!   floating window, tiles it (unless `max_tiled_windows` is already
//!   reached, in which case it stays floating).
//! - `hyper+m` fills the focused window's display, inset by `gaps.outer`;
//!   works on tiled or floating windows. On a tiled window, the *next*
//!   `hyper+f`/`hyper+hjkl`/new-window event should snap it back to its
//!   tree rect (maximize isn't persistent state -- architecture.md §3.8).
//! - `hyper+hjkl` on a tiled window swaps it with its nearest neighbor in
//!   that direction (geometric, not remembered -- architecture.md §3.5);
//!   at a tree edge, no-op. On a floating window, nudges it by
//!   `floating.float_move_step` pixels.
//! - `hyper+shift+hjkl` on a tiled window resizes the nearest enclosing
//!   split by `tiling.resize.step`; no-op on a floating window.
//! - Opening a new window (in an already-running app, or a freshly
//!   launched one) should tile it if there's room, or float it
//!   (`cascade`/`center`, per config) if `max_tiled_windows` is reached or
//!   its app is in `always_float_apps`.
//! - Closing a tiled window should collapse its sibling into its place
//!   (architecture.md §3.4); closing the last tiled window empties the
//!   tree.
//! - Dragging a *tiled* window by hand should snap back to its tree rect
//!   at or near real-time (architecture.md §3.10); dragging a *floating*
//!   window should just stay where you drop it.

mod lifecycle;
mod registry;
mod router;
mod state;

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;

use core_foundation::runloop::CFRunLoop;
use hyperwm_macos::{ax, hyperkey, keycode, permissions, workspace};

use router::Router;
use state::DaemonState;

/// `CGEventTap::new`'s callback bound requires `Send` (in case the tap
/// ever ran on a thread other than the one that created it -- it doesn't,
/// here: this daemon has exactly one thread, and every AX/Cocoa call in
/// `DaemonState` and its `Rc<RefCell<_>>` requires staying on it anyway).
/// This asserts that requirement away for the one closure that needs it;
/// nothing here is ever actually accessed from a second thread.
struct AssertSend<T>(T);
unsafe impl<T> Send for AssertSend<T> {}

fn main() -> ExitCode {
    let Some(path) = hyperwm_config::default_config_path() else {
        eprintln!(
            "hyperwm-daemon: no config file found (checked ~/.config/hyperwm/config.toml \
             and ~/.hyperwm/config.toml)"
        );
        return ExitCode::FAILURE;
    };

    let config = match hyperwm_config::load(&path) {
        Ok(config) => {
            println!(
                "hyperwm-daemon: loaded {} ({} built-in keybinds, {} script keybinds)",
                path.display(),
                config.keybinds.builtin.len(),
                config.keybinds.scripts.len()
            );
            config
        }
        Err(err) => {
            eprintln!(
                "hyperwm-daemon: invalid config at {}: {err}",
                path.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let status = permissions::check();
    if !status.all_granted() {
        eprintln!(
            "hyperwm-daemon: missing required permission(s); grant these, then restart \
             hyperwm-daemon:\n{}",
            status.instructions()
        );
        return ExitCode::FAILURE;
    }
    println!("hyperwm-daemon: Accessibility and Input Monitoring permissions granted");

    let key_name = config.hyperkey.watch_keycode.clone();
    let Some(watch_keycode) = keycode::lookup(key_name.as_str()) else {
        eprintln!(
            "hyperwm-daemon: hyperkey.watch_keycode \"{key_name}\" has no known macOS virtual \
             keycode mapping (this can happen for f21-f24, which macOS doesn't assign a \
             keycode to); pick a different key in the config"
        );
        return ExitCode::FAILURE;
    };

    let router = Router::build(&config);
    let state = Rc::new(RefCell::new(DaemonState::new(config)));

    println!("hyperwm-daemon: adopting already-running apps' windows");
    for pid in ax::all_app_pids() {
        lifecycle::adopt_app(&state, pid);
    }

    let launch_watcher = {
        let state = Rc::clone(&state);
        workspace::watch_app_launches(move |pid| lifecycle::adopt_app(&state, pid))
    };

    println!(
        "hyperwm-daemon: watching \"{key_name}\" (keycode {watch_keycode}) as the hyperkey"
    );
    let dispatch = AssertSend((Rc::clone(&state), router));
    let watcher = hyperkey::install(
        watch_keycode,
        |active| println!("hyper-active: {active}"),
        move |event| {
            // Matching on `&dispatch` (not `&dispatch.0`) makes the closure
            // capture the whole `AssertSend` wrapper rather than its inner
            // field directly (Rust 2021's per-field capture would otherwise
            // capture the non-`Send` tuple and defeat the wrapper).
            let AssertSend((state, router)) = &dispatch;
            if let Some(action) = router.action_for(event.keycode, event.shift) {
                state.borrow_mut().dispatch(action);
            }
        },
    );
    let watcher = match watcher {
        Ok(watcher) => watcher,
        Err(err) => {
            eprintln!(
                "hyperwm-daemon: {err} -- was reported granted at startup, so try restarting \
                 the daemon, or re-check System Settings > Privacy & Security > Input \
                 Monitoring"
            );
            return ExitCode::FAILURE;
        }
    };

    println!("hyperwm-daemon: running");
    CFRunLoop::run_current();

    // Unreachable under normal operation (the run loop above never
    // returns); keeps `watcher`/`launch_watcher` alive for the daemon's
    // entire lifetime instead of being dropped (and disabled) right after
    // `install`/`watch_app_launches` return.
    drop(watcher);
    drop(launch_watcher);
    ExitCode::SUCCESS
}
