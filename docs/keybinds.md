# Keybinds reference

hyperwm has no non-hyper global keybinds — every binding is `hyper`-prefixed
(`hyperwm-config/src/keybind.rs`), which avoids conflicting with normal app
shortcuts. This document covers the keybind string syntax, the full set of
recognized key names, and every built-in action. Script keybinds
(`[keybinds.scripts]`) are covered in [scripts.md](scripts.md).

## Syntax

```
"hyper+<key>" = "<action>"
"hyper+shift+<key>" = "<action>"
```

- `hyper` is always required and always first.
- `shift` is the only other modifier v1 supports, and if present it always
  sits between `hyper` and the key (`hyper+shift+h`, not `hyper+h+shift`
  or `shift+hyper+h`).
- The modifier keyword is case-insensitive (`HYPER+H` parses the same as
  `hyper+h`), but key names are canonicalized lowercase internally.

Malformed keybind strings produce a specific error rather than a generic
one, e.g. `keybind must start with "hyper+"`, `unknown modifier "cmd" (only
"shift" is supported)`, `unknown key name "fn"`, `keybind has no key after
the modifiers`, or `keybind has too many "+"-separated parts`.

Binding the same keybind twice — as two built-in actions, two scripts, or
one of each — is a validation error (`keybind "hyper+f" is bound more than
once`), whichever occurrence is parsed second.

## Key names

Any of:

- **Letters**: `a`–`z`
- **Digits**: `0`–`9`
- **Function keys**: `f1`–`f24` (syntactically — see the caveat below)
- **Named keys**: `space`, `tab`, `escape`, `return`, `enter`, `delete`,
  `backspace`, `up`, `down`, `left`, `right`, `minus`, `equal`, `comma`,
  `period`, `slash`, `semicolon`, `quote`, `leftbracket`, `rightbracket`,
  `backslash`, `grave`

**`f21`–`f24` parse but have no runtime keycode.** They're accepted by
`hyperwm_config::KeyName` (the config schema doesn't know about macOS
keycode limits), but `hyperwm-macos::keycode::lookup` has no `kVK_*`
mapping for them — Apple doesn't define one. Using one of these as
`hyperkey.watch_keycode` fails at daemon startup with `"f21" has no known
macOS virtual keycode mapping`; using one as the `<key>` half of an action
or script keybind (e.g. `"hyper+f21"`) is currently unenforced by
`hyperwm verify` and would simply never fire, since no `CGEventTap`
keycode ever matches it.

`hyperkey.watch_keycode` (see [configuration.md](configuration.md#hyperkey))
uses this same key-name set, and `hyperwm install-keymap` further narrows
it to `f1`–`f20` for the automated Caps Lock remap — see
[installation.md](installation.md).

## Built-in actions

| Action           | Keybind (example config) | Behavior |
|------------------|---------------------------|----------|
| `toggle_float`   | `hyper+f`       | Tiled → floating (in place, no jump) or floating → tiled (per the insertion rule, no-op if `max_tiled_windows` is already reached). Architecture §3.8. |
| `maximize`       | `hyper+m`       | Fills the focused window's display, inset by `gaps.outer`. Works on tiled or floating windows. Not persistent state — a tiled window snaps back to its tree rect on the next tiling-affecting event. Architecture §3.8. |
| `move_left`      | `hyper+h`       | Tiled: swap with the nearest tiled leaf to the left (geometric, recomputed live every press — never remembered). Floating: nudge left by `floating.float_move_step` px. No-op at a tree edge; no wraparound. Architecture §3.5, §3.7. |
| `move_down`      | `hyper+j`       | Same as above, downward. |
| `move_up`        | `hyper+k`       | Same as above, upward. |
| `move_right`     | `hyper+l`       | Same as above, rightward. |
| `resize_left`    | `hyper+shift+h` | Adjusts the nearest enclosing split's ratio by `tiling.resize.step`, in the direction pressed. No-op on a floating window. Architecture §3.6. |
| `resize_down`    | `hyper+shift+j` | Same, downward split axis. |
| `resize_up`      | `hyper+shift+k` | Same, upward split axis. |
| `resize_right`   | `hyper+shift+l` | Same, rightward split axis. |
| `auto_tile`      | *(no default — user-assigned)* | Forces the current on-screen window set (up to `max_tiled_windows`) into a freshly computed tree layout immediately, using the normal insertion rule in on-screen enumeration order. Never invoked automatically by hyperwm itself, including on first visit to a new Space. Architecture §3.11. |

This is the complete list — `hyperwm-config/src/action.rs` is the single
source of truth for recognized action names; a `[keybinds]` value that
doesn't match one of the strings in the left column exactly (e.g.
`"float_toggle"` instead of `"toggle_float"`) is a validation error naming
both the offending keybind and the unrecognized action.

`move_*`/`resize_*` are named by screen direction, not by any notion of
"next"/"previous" window — see architecture.md §3.5 for why directional,
geometry-driven movement was chosen over an ordered/history-based scheme.

## Checking your bindings

`hyperwm verify` reports the total built-in and script keybind counts on
success, and the specific validation error (duplicate bind, unknown
action, missing/non-executable script) on failure — see
[configuration.md](configuration.md#validating-a-config).
