//! Minimal, dependency-free ANSI color helpers for `status`'s output
//! (roadmap: "CLI / status output polish"). Hand-rolled rather than pulling
//! in a crate -- this binary has exactly one dependency (`hyperwm-config`)
//! and a handful of `println!`-friendly color codes don't justify a new
//! one.
//!
//! Respects `NO_COLOR` (<https://no-color.org>) and only colors when stdout
//! is actually a terminal, so piping `hyperwm status` into a file or
//! another program gets plain text, not escape codes.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy)]
pub struct Painter {
    enabled: bool,
}

impl Painter {
    #[must_use]
    pub fn detect() -> Self {
        let enabled = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Self { enabled }
    }

    /// Wraps `text` in the given SGR code(s) (e.g. `"32"` for green,
    /// `"1;31"` for bold red) -- a no-op when coloring is disabled.
    #[must_use]
    pub fn colorize(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    #[must_use]
    pub fn bold(&self, text: &str) -> String {
        self.colorize("1", text)
    }

    #[must_use]
    pub fn dim(&self, text: &str) -> String {
        self.colorize("2", text)
    }

    #[must_use]
    pub fn green(&self, text: &str) -> String {
        self.colorize("32", text)
    }

    #[must_use]
    pub fn red(&self, text: &str) -> String {
        self.colorize("31", text)
    }

    #[must_use]
    pub fn bold_green(&self, text: &str) -> String {
        self.colorize("1;32", text)
    }

    #[must_use]
    pub fn bold_red(&self, text: &str) -> String {
        self.colorize("1;31", text)
    }
}
