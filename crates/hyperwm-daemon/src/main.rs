//! Scaffold only. Event Router + Action Engine wiring lands starting in
//! build unit 5 (see CLAUDE.md build order).
//!
//! Startup below just proves hyperwm-config's `load` is usable from this
//! crate on normal daemon startup — the keep-last-good-config-on-reload
//! behavior (architecture.md §6) is a unit 7 concern, not implemented here.

use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = hyperwm_config::default_config_path() else {
        eprintln!(
            "hyperwm-daemon: no config file found (checked ~/.config/hyperwm/config.toml \
             and ~/.hyperwm/config.toml)"
        );
        return ExitCode::FAILURE;
    };

    match hyperwm_config::load(&path) {
        Ok(config) => {
            println!(
                "hyperwm-daemon: loaded {} ({} built-in keybinds, {} script keybinds)",
                path.display(),
                config.keybinds.builtin.len(),
                config.keybinds.scripts.len()
            );
            println!("hyperwm-daemon: not yet implemented beyond config loading (see CLAUDE.md build order)");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("hyperwm-daemon: invalid config at {}: {err}", path.display());
            ExitCode::FAILURE
        }
    }
}
