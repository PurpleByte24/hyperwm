//! Event Router (architecture.md §1): keybind lookup only. Given a raw
//! `(keycode, shift)` pair from `hyperwm_macos::hyperkey` (fired only while
//! hyper-active -- that gate lives in `hyperkey`, not here), finds which
//! configured `Action` it maps to, if any. Dispatching that `Action`
//! against real window state is `DaemonState`'s job (the Action Engine),
//! not this module's -- kept separate so keybind lookup can be
//! reasoned about (and, if it's ever wrong, fixed) independent of what
//! each action actually does.
//!
//! Script keybinds (`[keybinds.scripts]`, architecture.md §5) are out of
//! scope for this build unit (CLAUDE.md's build order reserves them for
//! unit 6) -- this router only resolves `config.keybinds.builtin`.

use std::collections::HashMap;

use hyperwm_config::{Action, Config};
use hyperwm_macos::keycode::{self, CGKeyCode};

pub struct Router {
    table: HashMap<(CGKeyCode, bool), Action>,
}

impl Router {
    #[must_use]
    pub fn build(config: &Config) -> Self {
        let mut table = HashMap::new();
        for (keybind, action) in &config.keybinds.builtin {
            match keycode::lookup(keybind.key.as_str()) {
                Some(code) => {
                    table.insert((code, keybind.shift), *action);
                }
                None => eprintln!(
                    "hyperwm-daemon: keybind \"{keybind}\" has no known macOS keycode mapping, \
                     skipping (see hyperwm_macos::keycode's f21-f24 note)"
                ),
            }
        }
        Self { table }
    }

    #[must_use]
    pub fn action_for(&self, keycode: CGKeyCode, shift: bool) -> Option<Action> {
        self.table.get(&(keycode, shift)).copied()
    }
}
