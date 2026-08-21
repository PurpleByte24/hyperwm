use std::fmt;
use std::path::PathBuf;

use crate::keybind::KeybindSyntaxError;

/// Every way `Config::load` can fail. Each variant carries enough context
/// to print a specific, actionable message — never a generic "parse
/// failed".
#[derive(Debug)]
pub enum ConfigError {
    /// The config file itself couldn't be read.
    Io { path: PathBuf, source: std::io::Error },
    /// Malformed TOML (syntax error, wrong value type for a field, etc.).
    /// `toml::de::Error`'s own message already includes line/column context.
    Toml(toml::de::Error),
    /// A field with no documented default was left out entirely.
    MissingField(&'static str),
    /// A field was present but its value is out of range or otherwise
    /// invalid.
    InvalidValue { field: &'static str, reason: String },
    /// A key in a `[keybinds]`/`[keybinds.scripts]` table isn't a valid
    /// `"hyper+..."` keybind string.
    InvalidKeybind { raw: String, reason: KeybindSyntaxError },
    /// A `[keybinds]` value isn't one of the recognized built-in action
    /// names.
    UnknownAction { keybind: String, action: String },
    /// The same keybind is assigned more than once (built-in action vs.
    /// built-in action, script vs. script, or built-in vs. script).
    DuplicateKeybind { keybind: String },
    /// A `[keybinds.scripts]` entry names a file that doesn't exist in
    /// `scripts.dir`.
    ScriptNotFound { keybind: String, script: String, path: PathBuf },
    /// A `[keybinds.scripts]` entry names a file that exists but isn't
    /// executable.
    ScriptNotExecutable { keybind: String, script: String, path: PathBuf },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "couldn't read config file {}: {source}", path.display())
            }
            Self::Toml(source) => write!(f, "invalid TOML: {source}"),
            Self::MissingField(field) => {
                write!(f, "missing required field \"{field}\"")
            }
            Self::InvalidValue { field, reason } => {
                write!(f, "invalid value for \"{field}\": {reason}")
            }
            Self::InvalidKeybind { raw, reason } => {
                write!(f, "invalid keybind \"{raw}\": {reason}")
            }
            Self::UnknownAction { keybind, action } => {
                write!(
                    f,
                    "keybind \"{keybind}\" is bound to unknown action \"{action}\""
                )
            }
            Self::DuplicateKeybind { keybind } => {
                write!(f, "keybind \"{keybind}\" is bound more than once")
            }
            Self::ScriptNotFound { keybind, script, path } => {
                write!(
                    f,
                    "keybind \"{keybind}\" references script \"{script}\", \
                     which doesn't exist at {}",
                    path.display()
                )
            }
            Self::ScriptNotExecutable { keybind, script, path } => {
                write!(
                    f,
                    "keybind \"{keybind}\" references script \"{script}\" at {}, \
                     which exists but isn't executable (missing the executable bit)",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Toml(source) => Some(source),
            _ => None,
        }
    }
}
