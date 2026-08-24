//! Event Router (architecture.md §1): keybind lookup only. Given a raw
//! `(keycode, shift)` pair from `hyperwm_macos::hyperkey` (fired only while
//! hyper-active -- that gate lives in `hyperkey`, not here), finds which
//! configured `Action` or script it maps to, if any. Dispatching an
//! `Action` against real window state is `DaemonState`'s job (the Action
//! Engine); running a script is `script::run`'s -- kept separate from
//! either so keybind lookup can be reasoned about (and, if it's ever
//! wrong, fixed) independent of what each binding actually does.
//!
//! `config.keybinds.builtin` and `config.keybinds.scripts` are disjoint by
//! construction: `hyperwm_config`'s `validate_keybinds` rejects the same
//! keybind appearing in both (`ConfigError::DuplicateKeybind`), so a given
//! `(keycode, shift)` pair only ever resolves through one of
//! [`Router::action_for`] / [`Router::script_for`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hyperwm_config::{Action, Config};
use hyperwm_macos::keycode::{self, CGKeyCode};

pub struct Router {
    actions: HashMap<(CGKeyCode, bool), Action>,
    /// Script keybinds, pre-resolved to their full path (`config.scripts.dir`
    /// joined onto the filename `hyperwm_config` validated at load time --
    /// see `hyperwm_config::validate::validate_scripts`) so `script::run`
    /// never has to re-derive it.
    scripts: HashMap<(CGKeyCode, bool), PathBuf>,
}

impl Router {
    #[must_use]
    pub fn build(config: &Config) -> Self {
        let mut actions = HashMap::new();
        for (keybind, action) in &config.keybinds.builtin {
            match keycode::lookup(keybind.key.as_str()) {
                Some(code) => {
                    actions.insert((code, keybind.shift), *action);
                }
                None => eprintln!(
                    "hyperwm-daemon: keybind \"{keybind}\" has no known macOS keycode mapping, \
                     skipping (see hyperwm_macos::keycode's f21-f24 note)"
                ),
            }
        }

        let mut scripts = HashMap::new();
        for (keybind, filename) in &config.keybinds.scripts {
            match keycode::lookup(keybind.key.as_str()) {
                Some(code) => {
                    scripts.insert((code, keybind.shift), config.scripts.dir.join(filename));
                }
                None => eprintln!(
                    "hyperwm-daemon: keybind \"{keybind}\" has no known macOS keycode mapping, \
                     skipping (see hyperwm_macos::keycode's f21-f24 note)"
                ),
            }
        }

        Self { actions, scripts }
    }

    #[must_use]
    pub fn action_for(&self, keycode: CGKeyCode, shift: bool) -> Option<Action> {
        self.actions.get(&(keycode, shift)).copied()
    }

    #[must_use]
    pub fn script_for(&self, keycode: CGKeyCode, shift: bool) -> Option<&Path> {
        self.scripts.get(&(keycode, shift)).map(PathBuf::as_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory under the OS temp dir, unique to this test process and
    /// test name, freshly emptied -- same convention as
    /// `hyperwm-config`'s `tests/config_tests.rs::scratch_dir`.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("hyperwm-daemon-router-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_executable_script(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let scripts_dir = dir.join("scripts");
        std::fs::create_dir_all(&scripts_dir).unwrap();
        let path = scripts_dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn load_config(dir: &Path, keybinds_block: &str) -> Config {
        let config_path = dir.join("config.toml");
        let contents = format!(
            r#"
[hyperkey]
watch_keycode = "f18"

[gaps]
outer = 12
inner = 8

{keybinds_block}
"#
        );
        std::fs::write(&config_path, contents).unwrap();
        hyperwm_config::load(&config_path).expect("test config should be valid")
    }

    #[test]
    fn script_keybind_resolves_to_its_full_path() {
        let dir = scratch_dir("script-resolves");
        let script_path = write_executable_script(&dir, "toggle_ghostty.sh");
        let config = load_config(
            &dir,
            r#"
[keybinds.scripts]
"hyper+g" = "toggle_ghostty.sh"
"#,
        );

        let router = Router::build(&config);
        let keycode = keycode::lookup("g").unwrap();

        assert_eq!(router.script_for(keycode, false), Some(script_path.as_path()));
        assert_eq!(router.action_for(keycode, false), None);
    }

    #[test]
    fn builtin_and_script_keybinds_do_not_collide() {
        let dir = scratch_dir("builtin-and-script");
        write_executable_script(&dir, "new_note.sh");
        let config = load_config(
            &dir,
            r#"
[keybinds]
"hyper+f" = "toggle_float"

[keybinds.scripts]
"hyper+n" = "new_note.sh"
"#,
        );

        let router = Router::build(&config);
        let f = keycode::lookup("f").unwrap();
        let n = keycode::lookup("n").unwrap();

        assert_eq!(router.action_for(f, false), Some(Action::ToggleFloat));
        assert_eq!(router.script_for(f, false), None);
        assert!(router.script_for(n, false).is_some());
        assert_eq!(router.action_for(n, false), None);
    }

    #[test]
    fn unbound_keycode_resolves_to_neither() {
        let dir = scratch_dir("unbound");
        let config = load_config(&dir, "");

        let router = Router::build(&config);
        let h = keycode::lookup("h").unwrap();

        assert_eq!(router.action_for(h, false), None);
        assert_eq!(router.script_for(h, false), None);
    }
}
