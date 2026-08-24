# Configuration reference

This is the field-by-field reference for `config.toml`. See
[`examples/config.toml`](../examples/config.toml) for a complete, annotated
example, and [`architecture.md`](architecture.md) for the *behavioral*
rules each field controls — this document covers syntax, defaults, and
validation, not the tiling algorithm itself.

Every claim below reflects `hyperwm-config/src/*.rs` (parsing in `raw.rs`,
defaults and validation in `validate.rs`) as of build unit 8, not a
guess at what the schema "should" say.

## File location

`hyperwm-daemon` and `hyperwm verify` (with no `--config` flag) both
resolve the config file the same way, via
`hyperwm_config::default_config_path`:

1. `~/.config/hyperwm/config.toml`, if it exists.
2. Otherwise `~/.hyperwm/config.toml`, if it exists.
3. Otherwise, no config is found (the daemon refuses to start; `hyperwm
   verify` reports an error).

If both exist, `~/.config/hyperwm/config.toml` wins — the fallback path is
never even checked in that case.

`hyperwm verify --config <path>` (see [scripts.md](scripts.md) and the
`hyperwm verify` docs below) can point at any path instead, bypassing this
lookup entirely.

## Unknown fields are rejected

Every section except `[keybinds]` is parsed with serde's
`deny_unknown_fields`. A typo'd field name (e.g. `tiling.max_tiled_window`
missing the trailing `s`) is a hard parse error, not a silently ignored
field — `hyperwm verify` and daemon startup will both report it. This is
deliberate: a config field that looks accepted but does nothing is a worse
failure mode than a startup error pointing at the typo.

`[keybinds]` is the one exception, by necessity — every key in that table
*is* a keybind string chosen by the user, so there's no fixed set of field
names to validate against at the TOML level. Unrecognized action names
under `[keybinds]` are instead rejected during validation (see
[keybinds.md](keybinds.md)).

## `[hyperkey]`

| Field           | Type   | Default | Notes |
|-----------------|--------|---------|-------|
| `watch_keycode` | string | *(required)* | The key `hyperwm-daemon` watches as the hyper modifier. |

`watch_keycode` must be one of the key names hyperwm recognizes (see
[keybinds.md](keybinds.md#key-names) for the full list — the same set
valid as the `<key>` part of a `"hyper+<key>"` keybind). It has no
default; omitting `[hyperkey]` or `watch_keycode` is a validation error
(`missing required field "hyperkey.watch_keycode"`).

The convention is `"f18"` — no physical Mac keyboard has an F18 key, so
it's always free to repurpose as a hyperkey. This assumes Caps Lock (or
whichever physical key you prefer) has already been remapped to this key
at the system level; hyperwm does not perform the remap itself. See
[installation.md](installation.md) for both remap paths (`hyperwm
install-keymap` or the manual System Settings route).

If `watch_keycode` names a key with no known macOS virtual keycode
(`f21` through `f24` — Apple doesn't assign `kVK_*` constants for those),
the daemon fails to start with a specific error rather than silently
watching nothing.

## `[tiling]`

| Field                 | Type   | Default          | Constraints |
|-----------------------|--------|------------------|-------------|
| `max_tiled_windows`   | int    | `4`              | positive integer (≥ 1) |
| `split_ratio_default` | float  | `0.5`            | `0.0`–`1.0` |
| `insert_heuristic`    | string | `"aspect_ratio"` | `"aspect_ratio"` \| `"always_vertical"` \| `"always_horizontal"` |

`max_tiled_windows` caps how many windows the BSP tree manages at once
(architecture.md §3.2); windows beyond the cap float automatically.

`split_ratio_default` is the starting `ratio` assigned to a new split when
a window is inserted (architecture.md §3.3 step 4); `hyper+shift+hjkl`
adjusts a given split's ratio afterward, independent of this default.

`insert_heuristic` picks the split direction for a newly inserted window
(architecture.md §3.3 step 3):

- `"aspect_ratio"` — vertical (left/right) if the target leaf's current
  rect is wider than tall, horizontal (top/bottom) if taller than wide.
- `"always_vertical"` — always left/right.
- `"always_horizontal"` — always top/bottom.

### `[tiling.resize]`

| Field       | Type  | Default | Constraints |
|-------------|-------|---------|-------------|
| `step`      | float | `0.05`  | `> 0.0` and `≤ 1.0` |
| `min_ratio` | float | `0.1`   | `0.0`–`1.0` |
| `max_ratio` | float | `0.9`   | `0.0`–`1.0`, must be `>` `min_ratio` |

Governs `hyper+shift+hjkl` (architecture.md §3.6): `step` is the
percentage-point adjustment per press; `min_ratio`/`max_ratio` clamp the
resulting ratio so a split can't be resized to zero or full size.
`max_ratio ≤ min_ratio` is a validation error.

## `[gaps]`

| Field   | Type  | Default | Constraints |
|---------|-------|---------|-------------|
| `outer` | float | *(required)* | `≥ 0.0` |
| `inner` | float | *(required)* | `≥ 0.0` |

Both are required — there's no documented default, so omitting either is a
`missing required field` error. `outer` is the gap between tiled window
edges and the display's visible frame (also respected by `hyper+m`
maximize); `inner` is the gap between adjacent tiled windows, applied as a
half-gap on each side of a split boundary so adjacent windows end up
exactly `inner` px apart (architecture.md §4).

## `[floating]`

| Field                 | Type         | Default     | Constraints |
|-----------------------|--------------|-------------|-------------|
| `always_float_apps`   | string array | `[]`        | bundle IDs or app names |
| `new_float_placement` | string       | `"cascade"` | `"cascade"` \| `"center"` |
| `float_move_step`     | float        | `40`        | `≥ 0.0` |

`always_float_apps` lists apps whose windows are never inserted into the
tiling tree (architecture.md §3.9), regardless of `max_tiled_windows`
headroom. Bundle ID is preferred (get one with `osascript -e 'id of app
"App Name"'`) since it's unambiguous; a plain app name is accepted too but
matches on window owner name, which can be ambiguous if multiple
installed apps share a display name.

`new_float_placement` picks how a newly-floated window with no prior
position is placed: `"cascade"` offsets it from the previous floating
window's position; `"center"` centers it on its display.

`float_move_step` is the pixel step `hyper+hjkl` nudges a focused floating
window by (architecture.md §3.7) — unrelated to the tiled-window
neighbor-swap geometry.

## `[keybinds]` and `[keybinds.scripts]`

See [keybinds.md](keybinds.md) for the full keybind syntax and built-in
action list, and [scripts.md](scripts.md) for `[keybinds.scripts]` and the
`[scripts]` section together.

## `[scripts]`

| Field | Type   | Default      |
|-------|--------|--------------|
| `dir` | string | `"scripts"`  |

Covered in full in [scripts.md](scripts.md); `dir` must not be empty
(`must not be empty` validation error for `""` or all-whitespace).

## Validating a config

`hyperwm verify [--config <path>]` runs the exact same `hyperwm_config::load`
path the daemon uses at startup and on `hyperwm reload`, without needing a
daemon running:

```
$ hyperwm verify
/Users/you/.config/hyperwm/config.toml: OK (11 built-in keybinds, 2 script keybinds)
```

A broken config produces a specific, single-line error rather than a
generic parse failure, e.g.:

```
$ hyperwm verify
/Users/you/.config/hyperwm/config.toml: missing required field "gaps.outer"
```

```
$ hyperwm verify
/Users/you/.config/hyperwm/config.toml: keybind "hyper+f" is bound to unknown action "float_toggle"
```

```
$ hyperwm verify
/Users/you/.config/hyperwm/config.toml: keybind "hyper+g" references script "toggle_ghostty.sh", which doesn't exist at /Users/you/.config/hyperwm/scripts/toggle_ghostty.sh
```

## Reloading

`hyperwm reload` sends the running daemon a request (over the Unix domain
socket — see [architecture.md §6](architecture.md#6-config-reload)) to
re-read and re-validate the config file. If validation fails, the daemon
**keeps running on its previous valid config** and reports the specific
error back to the CLI; it never crashes or drops to an unconfigured state
on a bad reload. Reload only replaces keybind/gap/threshold/etc. values
going forward — it doesn't tear down or rebuild existing window trees, so
live windows keep their current tree positions.
