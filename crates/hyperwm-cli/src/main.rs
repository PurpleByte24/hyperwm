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

mod color;
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
        Some("reload") => reload(&args[2..]),
        Some("status") => status(&args[2..]),
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

/// Every hand-rolled subcommand parser in this crate (`main.rs`, `daemon.rs`,
/// `keymap.rs`) checks this before doing anything else with side effects --
/// there's no argument-parsing framework here to give `-h`/`--help` that for
/// free, so each parser is responsible for checking it explicitly rather
/// than falling through to its normal argument handling (which previously
/// either misread `--help` as an invalid subcommand/argument, or -- for
/// `install-keymap`, which ignored its argv entirely -- silently ran the
/// real command instead of printing help).
pub(crate) fn wants_help(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-h" || arg == "--help")
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
    let stream = connect().map_err(|err| err.to_string())?;
    call_on(stream, request)
}

/// Same as [`call`], but over a connection the caller already has -- lets
/// `status` inspect a failed [`connect`]'s [`std::io::ErrorKind`] itself
/// (to recognize "daemon isn't running" specifically) before it would
/// otherwise get flattened into a plain `String`.
fn call_on(mut stream: UnixStream, request: &Request) -> Result<Response, String> {
    protocol::write_message(&mut stream, request)
        .map_err(|err| format!("couldn't send request to hyperwm-daemon: {err}"))?;

    let mut reader = BufReader::new(stream);
    match protocol::read_message(&mut reader) {
        Ok(Some(response)) => Ok(response),
        Ok(None) => Err("hyperwm-daemon closed the connection without responding".to_string()),
        Err(err) => Err(format!("couldn't read response from hyperwm-daemon: {err}")),
    }
}

fn reload(args: &[String]) -> ExitCode {
    if wants_help(args) {
        println!("hyperwm reload");
        println!();
        println!("Reload the running daemon's config (architecture.md §6).");
        println!();
        println!("USAGE:");
        println!("    hyperwm reload");
        return ExitCode::SUCCESS;
    }
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

fn status(args: &[String]) -> ExitCode {
    if wants_help(args) {
        println!("hyperwm status");
        println!();
        println!("Show the running daemon's tiling state and health.");
        println!();
        println!("USAGE:");
        println!("    hyperwm status");
        return ExitCode::SUCCESS;
    }
    let paint = color::Painter::detect();

    let stream = match connect() {
        Ok(stream) => stream,
        Err(err) => return print_daemon_not_running(&paint, &err),
    };

    match call_on(stream, &Request::Status) {
        Ok(Response::Status(report)) => {
            print_status_report(&paint, &report);
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

fn print_status_report(paint: &color::Painter, report: &protocol::StatusReport) {
    println!("hyperwm-daemon: {} (pid {})", paint.bold_green("running"), report.pid);
    println!();
    print_config_section(paint, Some(&report.config_path));
    println!();
    print_window_section(paint, "tiled", "36", &report.tiled);
    println!();
    print_window_section(paint, "floating", "33", &report.floating);
    println!();
    match &report.focused {
        Some(window) => println!("focused: {} {}", paint.green("\u{25cf}"), window_line(paint, window)),
        None => println!("focused: {}", paint.dim("none")),
    }
}

/// The common case: `connect()` failed with `ConnectionRefused` (socket
/// exists, nothing listening) or `NotFound` (socket file doesn't exist at
/// all) -- in practice both just mean "the daemon isn't running". Rather
/// than let the raw `io::Error` leak through (the pre-polish behavior:
/// `couldn't connect to hyperwm-daemon at ... (Connection refused (os
/// error 61)) -- is the daemon running?`), this prints a deliberate,
/// designed status view: which config *would* be loaded, and how to start
/// the daemon. Roadmap: "CLI / status output polish".
fn print_daemon_not_running(paint: &color::Painter, err: &std::io::Error) -> ExitCode {
    use std::io::ErrorKind;

    println!("hyperwm-daemon: {}", paint.bold_red("not running"));
    println!();
    print_config_section(paint, None);
    println!();
    if !matches!(err.kind(), ErrorKind::ConnectionRefused | ErrorKind::NotFound) {
        // Something other than the expected "nothing's there" -- don't
        // hide it, it might be a real problem (e.g. a permissions issue on
        // the socket path).
        println!("{}", paint.dim(&err.to_string()));
        println!();
    }
    println!("start it with: {}", paint.bold("hyperwm daemon start"));
    ExitCode::FAILURE
}

/// Prints every config path `hyperwm` considers by default (roadmap: "show
/// which config file(s) were found/considered, not just the one that was
/// loaded"), marking whichever one is actually in play:
/// - `loaded_path: Some(path)` -- a running daemon reported the config path
///   it actually loaded (`StatusReport::config_path`); that candidate is
///   marked "(loaded)".
/// - `loaded_path: None` -- no daemon is running, so nothing has "loaded" a
///   config yet; the first *existing* candidate is marked "(would be
///   used)", matching `hyperwm_config::default_config_path`'s own lookup
///   order.
fn print_config_section(paint: &color::Painter, loaded_path: Option<&str>) {
    let Some(candidates) = hyperwm_config::config_candidates() else {
        println!(
            "config: {}",
            paint.dim("$HOME is not set -- can't locate a default config file")
        );
        return;
    };

    println!("config:");
    let mut marked = false;
    for candidate in &candidates {
        let path = candidate.path.display().to_string();
        let is_active = match loaded_path {
            Some(loaded) => loaded == path,
            None => !marked && candidate.exists,
        };
        if is_active {
            marked = true;
            let label = if loaded_path.is_some() { "loaded" } else { "would be used" };
            println!("  {} {path} {}", paint.green("*"), paint.dim(&format!("({label})")));
        } else if candidate.exists {
            println!("  {} {path}", paint.dim("-"));
        } else {
            println!("  {} {path} {}", paint.dim("-"), paint.dim("(not found)"));
        }
    }
    if !marked {
        match loaded_path {
            // Only possible if the daemon's loaded config path doesn't
            // match either default candidate -- can't happen today since
            // hyperwm-daemon only ever loads `default_config_path()`, but
            // report what it actually loaded rather than silently dropping
            // it if that ever changes.
            Some(loaded) => println!("  {} {loaded} {}", paint.green("*"), paint.dim("(loaded)")),
            None => println!("  {}", paint.red("no config file found at any of the above")),
        }
    }
}

fn print_window_section(paint: &color::Painter, label: &str, color_code: &str, windows: &[WindowSummary]) {
    println!("{}", paint.bold(&format!("{label} ({}):", windows.len())));
    if windows.is_empty() {
        println!("  {}", paint.dim("(none)"));
        return;
    }
    for window in windows {
        println!("  {} {}", paint.colorize(color_code, "\u{25cf}"), window_line(paint, window));
    }
}

fn window_line(paint: &color::Painter, window: &WindowSummary) -> String {
    let rect = &window.rect;
    format!(
        "{} \u{2014} \"{}\" [{:.0}, {:.0}, {:.0}x{:.0}]",
        paint.bold(&window.app),
        window.title,
        rect.x,
        rect.y,
        rect.width,
        rect.height
    )
}

/// `hyperwm verify [--config <path>]` (architecture.md §5's script checks
/// included, since they're already part of `hyperwm_config::load`'s
/// validation -- see `hyperwm-config/src/validate.rs`). Runs entirely
/// locally; no socket, no running daemon required.
fn verify(args: &[String]) -> ExitCode {
    if wants_help(args) {
        println!("hyperwm verify");
        println!();
        println!("Validate a config file (works without a running daemon).");
        println!();
        println!("USAGE:");
        println!("    hyperwm verify [--config <path>]");
        println!();
        println!("Defaults to ~/.config/hyperwm/config.toml, falling back to");
        println!("~/.hyperwm/config.toml, when --config isn't given.");
        return ExitCode::SUCCESS;
    }

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
