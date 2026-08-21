//! Scaffold only. Unix socket client (reload/status/verify) and
//! install-keymap land starting in build unit 7 (see CLAUDE.md build order).
//!
//! The `verify` subcommand below is a minimal stand-in that just proves
//! hyperwm-config's `load` is usable from this crate — it does not yet
//! match unit 7's eventual CLI surface (flags, help text, socket-based
//! `reload`/`status`).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("verify") => verify(args.next().map(PathBuf::from)),
        _ => {
            println!("hyperwm: not yet implemented (see CLAUDE.md build order)");
            ExitCode::SUCCESS
        }
    }
}

fn verify(path: Option<PathBuf>) -> ExitCode {
    let path = path.or_else(hyperwm_config::default_config_path);
    let Some(path) = path else {
        eprintln!("hyperwm verify: no config file found and none given");
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
