//! `hyperwm daemon start|stop|restart` (build unit 8): manages
//! `hyperwm-daemon` as a `launchd` user agent, via `launchd.rs`'s
//! `launchctl` wrapper.
//!
//! # The `start`-on-already-running bug, and why this version avoids it
//!
//! An earlier version of this command unconditionally tore the job down
//! and rebuilt it on every `start` call -- `bootout` (or `unload`) then
//! `bootstrap`/`load`, regardless of whether the daemon was already
//! running. Called against an already-running daemon, that's not a
//! no-op: it silently kills the live daemon (dropping its in-memory tree
//! state -- window positions the tree remembers, not just the process)
//! and respawns a fresh one, which looks like nothing happened unless
//! you were watching for the window flicker or checked `hyperwm status`'s
//! pid before and after.
//!
//! `start` here checks [`launchd::is_running`] *first* and, if true,
//! does nothing at all beyond printing that it's already running --
//! it never bootouts or re-bootstraps a job that's already up. Only
//! `restart` intentionally kills and respawns; `start` and `restart` are
//! meant to behave differently, and conflating them was the bug.
//!
//! # Manual verification
//!
//! Needs a real `launchd` user session (this doesn't run in `cargo test`):
//!
//! - `hyperwm daemon start` with no prior state: should print that it
//!   started the daemon, and `hyperwm status` afterward should succeed
//!   (daemon socket alive).
//! - `hyperwm daemon start` again immediately after: **the critical
//!   case** -- should print that the daemon is already running and do
//!   nothing else. Check `hyperwm status`'s pid before and after this
//!   second call: it must be the *same* pid (no kill-and-respawn). This
//!   is the bug this rebuild is explicitly fixing.
//! - `hyperwm daemon restart`: should print that it restarted, and
//!   `hyperwm status`'s pid should *change* (this one is supposed to
//!   kill and respawn).
//! - `hyperwm daemon stop`: `hyperwm status` afterward should fail to
//!   connect ("is the daemon running?"). `hyperwm daemon stop` again
//!   should print that it's already stopped, not error.
//! - Log in fresh (or `launchctl bootout` + reboot) after a `start`:
//!   the daemon should **not** come back up on its own -- `RunAtLoad` is
//!   `false` (see `write_daemon_plist`'s doc comment), so only an explicit
//!   `hyperwm daemon start` (or a `KeepAlive`-driven respawn after a crash,
//!   if that's ever added) should bring it up.
//! - Inspect `~/Library/LaunchAgents/com.purplebyte24.hyperwm.daemon.plist`
//!   after any of the above: `ProgramArguments` should point at the
//!   `hyperwm-daemon` binary actually installed next to this `hyperwm`
//!   binary (`which hyperwm-daemon` after a brew install should match),
//!   and `RunAtLoad` should be `false`.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::launchd;

pub const LABEL: &str = "com.purplebyte24.hyperwm.daemon";

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "hyperwm daemon: $HOME is not set".to_string())
}

/// The `hyperwm-daemon` binary is installed alongside this `hyperwm`
/// binary (both `bin.install`-ed by the Homebrew formula into the same
/// `bin/`, and both built into the same `target/<profile>/` directory in
/// a dev checkout) -- looking next to `current_exe()` works for both
/// without hardcoding a Homebrew-specific prefix.
fn daemon_binary_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("hyperwm daemon: couldn't determine this binary's path: {err}"))?;
    let dir = exe.parent().ok_or_else(|| {
        "hyperwm daemon: couldn't determine this binary's containing directory".to_string()
    })?;
    let daemon = dir.join("hyperwm-daemon");
    if !daemon.is_file() {
        return Err(format!(
            "hyperwm daemon: expected hyperwm-daemon at {} but it isn't there -- reinstall \
             (brew reinstall hyperwm) or, in a dev checkout, build the whole workspace \
             (cargo build --workspace) so both binaries land in the same target directory",
            daemon.display()
        ));
    }
    Ok(daemon)
}

fn log_dir(home: &std::path::Path) -> Result<PathBuf, String> {
    let dir = home.join("Library/Logs/hyperwm");
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("hyperwm daemon: couldn't create {}: {err}", dir.display()))?;
    Ok(dir)
}

/// `RunAtLoad` is `false`: unit 8 deliberately defers auto-start-at-login
/// (unlike `install-keymap`'s LaunchAgent, whose whole point is to persist
/// the remap across logins) -- starting the daemon is an explicit `hyperwm
/// daemon start`, not something a login should trigger on its own.
fn write_daemon_plist() -> Result<PathBuf, String> {
    let home = home_dir()?;
    let daemon_bin = daemon_binary_path()?;
    let logs = log_dir(&home)?;
    launchd::write_plist(
        &home,
        LABEL,
        &[daemon_bin.display().to_string()],
        false,
        &logs.join("daemon.log"),
        &logs.join("daemon.err.log"),
    )
    .map_err(|err| format!("hyperwm daemon: couldn't write launchd plist: {err}"))
}

pub fn run(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("start") => start(&args[1..]),
        Some("stop") => stop(&args[1..]),
        Some("restart") => restart(&args[1..]),
        Some("-h" | "--help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!(
                "hyperwm daemon: unknown subcommand \"{other}\" (expected start, stop, or \
                 restart)"
            );
            ExitCode::FAILURE
        }
        None => {
            eprintln!("hyperwm daemon: expected a subcommand (start, stop, or restart)");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!("hyperwm daemon");
    println!();
    println!("Manage hyperwm-daemon as a launchd user agent.");
    println!();
    println!("USAGE:");
    println!("    hyperwm daemon <SUBCOMMAND>");
    println!();
    println!("SUBCOMMANDS:");
    println!("    start      Start hyperwm-daemon (no-op if already running)");
    println!("    stop       Stop hyperwm-daemon (no-op if not running)");
    println!("    restart    Kill and respawn hyperwm-daemon");
}

fn start(args: &[String]) -> ExitCode {
    if crate::wants_help(args) {
        println!("hyperwm daemon start");
        println!();
        println!("Start hyperwm-daemon as a launchd user agent. No-op if it's already running.");
        return ExitCode::SUCCESS;
    }
    match launchd::is_running(LABEL) {
        Ok(true) => {
            println!("hyperwm-daemon is already running -- nothing to do");
            ExitCode::SUCCESS
        }
        Ok(false) => match start_stopped_daemon() {
            Ok(()) => {
                println!("hyperwm-daemon started");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            eprintln!("hyperwm daemon: couldn't query launchd state: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Only reached once `start` has already confirmed the daemon is *not*
/// running -- handles both "never bootstrapped" (fresh install, or after
/// a `stop`) and "bootstrapped but not running" (e.g. a past crash) by
/// checking [`launchd::is_bootstrapped`] separately: a bootstrapped-but-
/// dead job needs `kickstart`, not a second `bootstrap` (which fails --
/// the label already exists in the domain).
fn start_stopped_daemon() -> Result<(), String> {
    write_daemon_plist()?;
    let plist_path = launchd::plist_path(&home_dir()?, LABEL);
    if launchd::is_bootstrapped(LABEL).map_err(|e| e.to_string())? {
        launchd::kickstart(LABEL, false)
    } else {
        launchd::bootstrap(LABEL, &plist_path)
    }
}

fn stop(args: &[String]) -> ExitCode {
    if crate::wants_help(args) {
        println!("hyperwm daemon stop");
        println!();
        println!("Stop hyperwm-daemon. No-op if it isn't running.");
        return ExitCode::SUCCESS;
    }
    match launchd::is_bootstrapped(LABEL) {
        Ok(false) => {
            println!("hyperwm-daemon is already stopped -- nothing to do");
            ExitCode::SUCCESS
        }
        Ok(true) => match launchd::bootout(LABEL) {
            Ok(()) => {
                println!("hyperwm-daemon stopped");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("hyperwm daemon: {err}");
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            eprintln!("hyperwm daemon: couldn't query launchd state: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Unlike `start`, always forces a fresh process -- kill-if-running, then
/// (re)bootstrap/kickstart. This is deliberately unconditional: `restart`
/// means "I want a new process," whereas `start` means "make sure one is
/// running" (see this module's doc comment for why conflating the two was
/// the bug being fixed here).
fn restart(args: &[String]) -> ExitCode {
    if crate::wants_help(args) {
        println!("hyperwm daemon restart");
        println!();
        println!("Kill and respawn hyperwm-daemon (always, even if it wasn't running).");
        return ExitCode::SUCCESS;
    }
    match restart_inner() {
        Ok(()) => {
            println!("hyperwm-daemon restarted");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn restart_inner() -> Result<(), String> {
    write_daemon_plist()?;
    let plist_path = launchd::plist_path(&home_dir()?, LABEL);
    if launchd::is_bootstrapped(LABEL).map_err(|e| e.to_string())? {
        launchd::kickstart(LABEL, true)
    } else {
        launchd::bootstrap(LABEL, &plist_path)
    }
}
