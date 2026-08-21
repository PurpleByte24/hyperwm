//! Parsing and validation of `config.toml`, matching
//! `examples/config.toml`'s schema exactly (see docs/architecture.md for
//! the behavioral rules each field controls).
//!
//! [`load`] is the single entry point both the daemon (normal startup) and
//! the CLI's `hyperwm verify` call — neither should reimplement validation,
//! they should both just call this.

mod action;
mod error;
mod keybind;
mod raw;
mod validate;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hyperwm_core::{Gaps, InsertHeuristic};

pub use action::Action;
pub use error::ConfigError;
pub use keybind::{KeyName, Keybind, KeybindSyntaxError};

/// A fully parsed and validated hyperwm configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub hyperkey: HyperkeyConfig,
    pub tiling: TilingConfig,
    pub gaps: Gaps,
    pub floating: FloatingConfig,
    pub keybinds: KeybindsConfig,
    pub scripts: ScriptsConfig,
}

#[derive(Debug, Clone)]
pub struct HyperkeyConfig {
    pub watch_keycode: KeyName,
}

#[derive(Debug, Clone)]
pub struct TilingConfig {
    pub max_tiled_windows: usize,
    pub split_ratio_default: f64,
    pub insert_heuristic: InsertHeuristic,
    pub resize: ResizeConfig,
}

#[derive(Debug, Clone, Copy)]
pub struct ResizeConfig {
    pub step: f64,
    pub min_ratio: f64,
    pub max_ratio: f64,
}

#[derive(Debug, Clone)]
pub struct FloatingConfig {
    pub always_float_apps: Vec<String>,
    pub new_float_placement: FloatPlacement,
    pub float_move_step: f64,
}

/// `floating.new_float_placement` (architecture.md §3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatPlacement {
    Cascade,
    Center,
}

#[derive(Debug, Clone)]
pub struct KeybindsConfig {
    pub builtin: HashMap<Keybind, Action>,
    pub scripts: HashMap<Keybind, String>,
}

#[derive(Debug, Clone)]
pub struct ScriptsConfig {
    /// Resolved to an absolute-or-as-given path: `scripts.dir` joined onto
    /// the config file's own directory if given as a relative path
    /// (architecture.md §5), used as-is if already absolute.
    pub dir: PathBuf,
}

/// Reads, parses, and validates the config file at `path`. This is the one
/// function both the daemon and `hyperwm verify` should call — it's the
/// shared validation path, not to be duplicated by either caller.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|source| ConfigError::Io { path: path.to_path_buf(), source })?;
    let raw: raw::RawConfig = toml::from_str(&text).map_err(ConfigError::Toml)?;
    let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
    validate::validate(raw, config_dir)
}

/// `~/.config/hyperwm/config.toml` if it exists, else
/// `~/.hyperwm/config.toml` if that exists, else `None`. Matches the lookup
/// order documented at the top of examples/config.toml.
#[must_use]
pub fn default_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let home = PathBuf::from(home);

    let preferred = home.join(".config/hyperwm/config.toml");
    if preferred.is_file() {
        return Some(preferred);
    }
    let fallback = home.join(".hyperwm/config.toml");
    if fallback.is_file() {
        return Some(fallback);
    }
    None
}
