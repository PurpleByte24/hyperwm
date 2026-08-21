//! Build unit 3: loads config, checks Accessibility/Input Monitoring
//! permissions, then watches `hyperkey.watch_keycode` and logs hyper-active
//! transitions to stdout (architecture.md §2.2-2.3). Event Router + Action
//! Engine wiring -- turning those transitions and other key events into
//! tree operations and real AX window moves -- lands starting in build
//! unit 5 (see CLAUDE.md build order). The keep-last-good-config-on-reload
//! behavior (architecture.md §6) is a unit 7 concern, not implemented here.

use std::process::ExitCode;

use hyperwm_macos::{hyperkey, keycode, permissions};

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

    let key_name = &config.hyperkey.watch_keycode;
    let Some(watch_keycode) = keycode::lookup(key_name.as_str()) else {
        eprintln!(
            "hyperwm-daemon: hyperkey.watch_keycode \"{key_name}\" has no known macOS virtual \
             keycode mapping (this can happen for f21-f24, which macOS doesn't assign a \
             keycode to); pick a different key in the config"
        );
        return ExitCode::FAILURE;
    };

    println!(
        "hyperwm-daemon: watching \"{key_name}\" (keycode {watch_keycode}) as the hyperkey; \
         press/release it to see hyper-active transitions below"
    );
    let result = hyperkey::watch(watch_keycode, |active| {
        println!("hyper-active: {active}");
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!(
                "hyperwm-daemon: {err} -- was reported granted at startup, so try restarting \
                 the daemon, or re-check System Settings > Privacy & Security > Input \
                 Monitoring"
            );
            ExitCode::FAILURE
        }
    }
}
