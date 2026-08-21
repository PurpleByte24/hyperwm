//! Keybind string syntax: `"hyper+<key>"` or `"hyper+shift+<key>"`
//! (examples/config.toml, "Keybinds — built-in actions"). `hyper` is always
//! required and always first; `shift` is the only other modifier v1
//! supports and, if present, always sits between `hyper` and the key.

use std::fmt;

/// One of the key names hyperwm recognizes, used both for
/// `hyperkey.watch_keycode` and for the `<key>` part of a keybind string.
/// Stored canonicalized (lowercase) so `Eq`/`Hash` work for duplicate-bind
/// detection regardless of the casing used in the config file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyName(String);

impl KeyName {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let lower = raw.to_ascii_lowercase();
        if is_known_key_name(&lower) {
            Some(Self(lower))
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_known_key_name(name: &str) -> bool {
    if name.len() == 1 {
        let c = name.chars().next().unwrap();
        return c.is_ascii_lowercase() || c.is_ascii_digit();
    }
    if let Some(n) = name.strip_prefix('f') {
        if let Ok(n) = n.parse::<u32>() {
            return (1..=24).contains(&n);
        }
    }
    matches!(
        name,
        "space"
            | "tab"
            | "escape"
            | "return"
            | "enter"
            | "delete"
            | "backspace"
            | "up"
            | "down"
            | "left"
            | "right"
            | "minus"
            | "equal"
            | "comma"
            | "period"
            | "slash"
            | "semicolon"
            | "quote"
            | "leftbracket"
            | "rightbracket"
            | "backslash"
            | "grave"
    )
}

/// A parsed `"hyper+..."` keybind: the hyper prefix plus an optional shift
/// modifier and the base key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Keybind {
    pub shift: bool,
    pub key: KeyName,
}

impl fmt::Display for Keybind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.shift {
            write!(f, "hyper+shift+{}", self.key)
        } else {
            write!(f, "hyper+{}", self.key)
        }
    }
}

/// Reason a keybind string failed to parse, for a specific, readable error
/// message (as opposed to a generic "invalid keybind").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindSyntaxError {
    Empty,
    MissingHyperPrefix,
    UnknownModifier(String),
    UnknownKeyName(String),
    MissingKey,
    TooManyParts,
}

impl fmt::Display for KeybindSyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "keybind is empty"),
            Self::MissingHyperPrefix => {
                write!(f, "keybind must start with \"hyper+\"")
            }
            Self::UnknownModifier(m) => {
                write!(f, "unknown modifier \"{m}\" (only \"shift\" is supported)")
            }
            Self::UnknownKeyName(k) => write!(f, "unknown key name \"{k}\""),
            Self::MissingKey => write!(f, "keybind has no key after the modifiers"),
            Self::TooManyParts => {
                write!(f, "keybind has too many \"+\"-separated parts")
            }
        }
    }
}

pub(crate) fn parse_keybind(raw: &str) -> Result<Keybind, KeybindSyntaxError> {
    let parts: Vec<&str> = raw.split('+').collect();
    if raw.trim().is_empty() {
        return Err(KeybindSyntaxError::Empty);
    }
    match parts.as_slice() {
        [hyper, key] => {
            if !hyper.eq_ignore_ascii_case("hyper") {
                return Err(KeybindSyntaxError::MissingHyperPrefix);
            }
            if key.is_empty() {
                return Err(KeybindSyntaxError::MissingKey);
            }
            let key = KeyName::parse(key)
                .ok_or_else(|| KeybindSyntaxError::UnknownKeyName((*key).to_string()))?;
            Ok(Keybind { shift: false, key })
        }
        [hyper, modifier, key] => {
            if !hyper.eq_ignore_ascii_case("hyper") {
                return Err(KeybindSyntaxError::MissingHyperPrefix);
            }
            if !modifier.eq_ignore_ascii_case("shift") {
                return Err(KeybindSyntaxError::UnknownModifier((*modifier).to_string()));
            }
            if key.is_empty() {
                return Err(KeybindSyntaxError::MissingKey);
            }
            let key = KeyName::parse(key)
                .ok_or_else(|| KeybindSyntaxError::UnknownKeyName((*key).to_string()))?;
            Ok(Keybind { shift: true, key })
        }
        [hyper] => {
            if !hyper.eq_ignore_ascii_case("hyper") {
                return Err(KeybindSyntaxError::MissingHyperPrefix);
            }
            Err(KeybindSyntaxError::MissingKey)
        }
        _ => Err(KeybindSyntaxError::TooManyParts),
    }
}
