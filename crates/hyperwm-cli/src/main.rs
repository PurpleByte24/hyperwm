//! `hyperwm` CLI (build unit 7): talks to the running `hyperwm-daemon` over
//! a Unix domain socket for `reload`/`status` (architecture.md §1, §6),
//! using `hyperwm_config::protocol`'s wire format -- the one place that
//! format is implemented, shared with the daemon side (`hyperwm-daemon`'s
//! `socket.rs`). `verify` is deliberately *not* part of that socket
//! protocol: it runs `hyperwm_config::load` directly in this process, so
//! it works whether or not a daemon is running -- the common case is
//! checking a config *before* starting the daemon at all.
//!
//! `daemon start|stop|restart` and `install-keymap` (build unit 8,
//! packaging) manage `hyperwm-daemon` and the Caps Lock remap as
//! `launchd` user agents -- see `daemon.rs` and `keymap.rs` for the
//! specifics (including `daemon start`'s already-running idempotency
//! fix and `install-keymap`'s login-persistence LaunchAgent) and their
//! module docs' manual verification checklists.
//!
//! # Manual verification
//!
//! Needs a real daemon and a real (or deliberately broken) config file, so
//! this isn't exercised by `cargo test` -- same reasoning as
//! `hyperwm-daemon`'s own manual-verification section, which this
//! complements from the CLI side. With `cargo run -p hyperwm-daemon`
//! running in one terminal against `examples/config.toml` (copied to
//! `~/.config/hyperwm/config.toml`), in a second terminal:
//!
//! - `cargo run -p hyperwm-cli -- status` should print the daemon's pid,
//!   config path, and current tiled/floating/focused windows (empty lists
//!   are fine if nothing's been tiled yet).
//! - `cargo run -p hyperwm-cli -- reload` with the config file unchanged
//!   should report success with the same keybind counts the daemon logged
//!   at startup.
//! - Edit the config (e.g. add a keybind under `[keybinds]`) and reload
//!   again -- `status` afterward should reflect it working (e.g. the new
//!   keybind actually does something when pressed).
//! - **The critical case**: edit the config to be invalid (e.g. set a
//!   `[keybinds]` value to an unrecognized action name, or delete
//!   `gaps.outer`) and reload. The CLI must print the specific validation
//!   error and exit non-zero; the daemon's own terminal must keep running
//!   with no crash/panic; a follow-up `status` call must still succeed and
//!   show the daemon still healthy, still on the previous (valid) config
//!   (architecture.md §6).
//! - `cargo run -p hyperwm-cli -- verify` (no daemon needed at all, can
//!   even be run with the daemon stopped) against `examples/config.toml`
//!   should report OK; against a deliberately broken config (missing
//!   field, bad keybind syntax, or a `[keybinds.scripts]` entry pointing at
//!   a nonexistent/non-executable file) should report that specific error.
//! - `cargo run -p hyperwm-cli -- --help` / `-h` and `--version` / `-v`
//!   should print without needing a daemon or a config file at all.

mod daemon;
mod keymap;
mod launchd;

use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;

use hyperwm_config::protocol::{self, Request, Response, WindowSummary};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("reload") => reload(),
        Some("status") => status(),
        Some("verify") => verify(&args[2..]),
        Some("daemon") => daemon::run(&args[2..]),
        Some("install-keymap") => keymap::run(&args[2..]),
        Some("-h" | "--help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("-v" | "--version") => {
            print_version();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("hyperwm: unknown subcommand \"{other}\"\n");
            print_help();
            ExitCode::FAILURE
        }
        None => {
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn print_version() {
    println!("hyperwm {}", env!("CARGO_PKG_VERSION"));
}

fn print_help() {
    println!("hyperwm {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("A keyboard-driven tiling window manager for macOS.");
    println!();
    println!("USAGE:");
    println!("    hyperwm <SUBCOMMAND>");
    println!();
    println!("SUBCOMMANDS:");
    println!("    reload                     Reload the running daemon's config (architecture.md §6)");
    println!("    status                     Show the running daemon's tiling state and health");
    println!("    verify [--config <path>]   Validate a config file (works without a running daemon)");
    println!("    daemon start               Start hyperwm-daemon as a launchd user agent (no-op if already running)");
    println!("    daemon stop                Stop hyperwm-daemon (no-op if not running)");
    println!("    daemon restart             Kill and respawn hyperwm-daemon");
    println!("    install-keymap             Remap Caps Lock via hidutil, persisted across logins (architecture.md §2.1)");
    println!("    -h, --help                 Print this help message");
    println!("    -v, --version              Print the version");
}

/// Connects to the running daemon's socket. The daemon must already be
/// running -- this never starts one (that's `hyperwm daemon start`, build
/// unit 8, not implemented here).
fn connect() -> std::io::Result<UnixStream> {
    let path = protocol::socket_path();
    UnixStream::connect(&path).map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!(
                "couldn't connect to hyperwm-daemon at {} ({err}) -- is the daemon running?",
                path.display()
            ),
        )
    })
}

/// Sends `request` to the running daemon and reads back exactly one
/// [`Response`], per `hyperwm_config::protocol`'s one-request-one-response
/// wire format.
fn call(request: &Request) -> Result<Response, String> {
    let mut stream = connect().map_err(|err| err.to_string())?;
    protocol::write_message(&mut stream, request)
        .map_err(|err| format!("couldn't send request to hyperwm-daemon: {err}"))?;

    let mut reader = BufReader::new(stream);
    match protocol::read_message(&mut reader) {
        Ok(Some(response)) => Ok(response),
        Ok(None) => Err("hyperwm-daemon closed the connection without responding".to_string()),
        Err(err) => Err(format!("couldn't read response from hyperwm-daemon: {err}")),
    }
}

fn reload() -> ExitCode {
    match call(&Request::Reload) {
        Ok(Response::Reloaded { builtin_keybinds, script_keybinds }) => {
            println!(
                "hyperwm: reloaded ({builtin_keybinds} built-in keybinds, {script_keybinds} \
                 script keybinds)"
            );
            ExitCode::SUCCESS
        }
        Ok(Response::ReloadFailed { error }) => {
            eprintln!(
                "hyperwm reload: rejected -- the daemon is still running on its previous \
                 config: {error}"
            );
            ExitCode::FAILURE
        }
        Ok(other) => {
            eprintln!("hyperwm reload: unexpected response from daemon: {other:?}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("hyperwm reload: {err}");
            ExitCode::FAILURE
        }
    }
}

fn status() -> ExitCode {
    match call(&Request::Status) {
        Ok(Response::Status(report)) => {
            println!("hyperwm-daemon: running (pid {})", report.pid);
            println!("config: {}", report.config_path);
            println!();
            println!("tiled ({}):", report.tiled.len());
            for window in &report.tiled {
                print_window(window);
            }
            println!();
            println!("floating ({}):", report.floating.len());
            for window in &report.floating {
                print_window(window);
            }
            println!();
            match &report.focused {
                Some(window) => {
                    print!("focused: ");
                    print_window(window);
                }
                None => println!("focused: none"),
            }
            ExitCode::SUCCESS
        }
        Ok(other) => {
            eprintln!("hyperwm status: unexpected response from daemon: {other:?}");
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("hyperwm status: {err}");
            ExitCode::FAILURE
        }
    }
}

fn print_window(window: &WindowSummary) {
    let rect = &window.rect;
    println!(
        "  {} \u{2014} \"{}\" [{:.0}, {:.0}, {:.0}x{:.0}]",
        window.app, window.title, rect.x, rect.y, rect.width, rect.height
    );
}

/// `hyperwm verify [--config <path>]` (architecture.md §5's script checks
/// included, since they're already part of `hyperwm_config::load`'s
/// validation -- see `hyperwm-config/src/validate.rs`). Runs entirely
/// locally; no socket, no running daemon required.
fn verify(args: &[String]) -> ExitCode {
    let mut config_path: Option<PathBuf> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => match args.next() {
                Some(value) => config_path = Some(PathBuf::from(value)),
                None => {
                    eprintln!("hyperwm verify: --config requires a path argument");
                    return ExitCode::FAILURE;
                }
            },
            other => {
                eprintln!("hyperwm verify: unrecognized argument \"{other}\"");
                return ExitCode::FAILURE;
            }
        }
    }

    let path = config_path.or_else(hyperwm_config::default_config_path);
    let Some(path) = path else {
        eprintln!(
            "hyperwm verify: no config file found (checked ~/.config/hyperwm/config.toml and \
             ~/.hyperwm/config.toml) and none given via --config"
        );
        return ExitCode::FAILURE;
    };

    match hyperwm_config::load(&path) {
        Ok(config) => {
            println!(
                "{}: OK ({} built-in keybinds, {} script keybinds)",
                path.display(),
                config.keybinds.builtin.len(),
                config.keybinds.scripts.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{}: {err}", path.display());
            ExitCode::FAILURE
        }
    }
}
