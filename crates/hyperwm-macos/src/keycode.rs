//! Maps the key names `hyperwm-config` accepts (see
//! `hyperwm_config::KeyName`) to macOS virtual keycodes (`CGKeyCode`, i.e.
//! Carbon's `kVK_*` constants), so `hyperkey.watch_keycode` can be turned
//! into the numeric code a `CGEventTap` callback compares against.
//!
//! This table is independent of keyboard layout (it's the physical ANSI-US
//! key position, same as the `kVK_*` constants themselves).

/// A macOS virtual keycode, as reported by `kCGKeyboardEventKeycode`.
pub type CGKeyCode = u16;

/// Looks up the `CGKeyCode` for a key name already validated by
/// `hyperwm_config::KeyName` (so `name` is expected lowercase and
/// well-formed). Returns `None` for names hyperwm-config's schema accepts
/// syntactically but that have no assigned macOS virtual keycode (`f21`
/// through `f24` -- Apple defines `kVK_*` constants only up to F20).
#[must_use]
pub fn lookup(name: &str) -> Option<CGKeyCode> {
    let code = match name {
        "a" => 0x00,
        "b" => 0x0B,
        "c" => 0x08,
        "d" => 0x02,
        "e" => 0x0E,
        "f" => 0x03,
        "g" => 0x05,
        "h" => 0x04,
        "i" => 0x22,
        "j" => 0x26,
        "k" => 0x28,
        "l" => 0x25,
        "m" => 0x2E,
        "n" => 0x2D,
        "o" => 0x1F,
        "p" => 0x23,
        "q" => 0x0C,
        "r" => 0x0F,
        "s" => 0x01,
        "t" => 0x11,
        "u" => 0x20,
        "v" => 0x09,
        "w" => 0x0D,
        "x" => 0x07,
        "y" => 0x10,
        "z" => 0x06,

        "0" => 0x1D,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "5" => 0x17,
        "6" => 0x16,
        "7" => 0x1A,
        "8" => 0x1C,
        "9" => 0x19,

        "f1" => 0x7A,
        "f2" => 0x78,
        "f3" => 0x63,
        "f4" => 0x76,
        "f5" => 0x60,
        "f6" => 0x61,
        "f7" => 0x62,
        "f8" => 0x64,
        "f9" => 0x65,
        "f10" => 0x6D,
        "f11" => 0x67,
        "f12" => 0x6F,
        "f13" => 0x69,
        "f14" => 0x6B,
        "f15" => 0x71,
        "f16" => 0x6A,
        "f17" => 0x40,
        "f18" => 0x4F,
        "f19" => 0x50,
        "f20" => 0x5A,
        // f21..f24: no kVK_* constant exists; unsupported.
        "space" => 0x31,
        "tab" => 0x30,
        "escape" => 0x35,
        "return" | "enter" => 0x24,
        "delete" | "backspace" => 0x33,
        "up" => 0x7E,
        "down" => 0x7D,
        "left" => 0x7B,
        "right" => 0x7C,
        "minus" => 0x1B,
        "equal" => 0x18,
        "comma" => 0x2B,
        "period" => 0x2F,
        "slash" => 0x2C,
        "semicolon" => 0x29,
        "quote" => 0x27,
        "leftbracket" => 0x21,
        "rightbracket" => 0x1E,
        "backslash" => 0x2A,
        "grave" => 0x32,

        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keys_resolve() {
        assert_eq!(lookup("f18"), Some(0x4F));
        assert_eq!(lookup("a"), Some(0x00));
        assert_eq!(lookup("space"), Some(0x31));
    }

    #[test]
    fn unassigned_function_keys_are_none() {
        assert_eq!(lookup("f21"), None);
        assert_eq!(lookup("f24"), None);
    }

    #[test]
    fn unknown_name_is_none() {
        assert_eq!(lookup("bogus"), None);
    }
}
