//! Built-in `[keybinds]` action names (examples/config.toml,
//! architecture.md §3.5–3.8).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleFloat,
    Maximize,
    MoveLeft,
    MoveDown,
    MoveUp,
    MoveRight,
    ResizeLeft,
    ResizeDown,
    ResizeUp,
    ResizeRight,
}

impl Action {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "toggle_float" => Self::ToggleFloat,
            "maximize" => Self::Maximize,
            "move_left" => Self::MoveLeft,
            "move_down" => Self::MoveDown,
            "move_up" => Self::MoveUp,
            "move_right" => Self::MoveRight,
            "resize_left" => Self::ResizeLeft,
            "resize_down" => Self::ResizeDown,
            "resize_up" => Self::ResizeUp,
            "resize_right" => Self::ResizeRight,
            _ => return None,
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ToggleFloat => "toggle_float",
            Self::Maximize => "maximize",
            Self::MoveLeft => "move_left",
            Self::MoveDown => "move_down",
            Self::MoveUp => "move_up",
            Self::MoveRight => "move_right",
            Self::ResizeLeft => "resize_left",
            Self::ResizeDown => "resize_down",
            Self::ResizeUp => "resize_up",
            Self::ResizeRight => "resize_right",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
