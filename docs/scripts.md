# Scripts

User-defined scripts, bindable to hyper-prefixed keybinds alongside the
built-in actions in [keybinds.md](keybinds.md). Covers architecture.md §5.

## `[scripts]`

```toml
[scripts]
dir = "scripts"
```

`dir` (default `"scripts"`) is where hyperwm looks for script files. A
relative path is resolved against the config file's *own* directory —
with the default config location, `dir = "scripts"` resolves to
`~/.config/hyperwm/scripts/`. An absolute path is used as-is. `dir` must
not be empty.

## `[keybinds.scripts]`

```toml
[keybinds.scripts]
"hyper+g" = "toggle_ghostty.sh"
"hyper+n" = "new_note.sh"
```

Each entry binds a `"hyper+<key>"` or `"hyper+shift+<key>"` keybind (same
syntax as `[keybinds]` — see [keybinds.md](keybinds.md#syntax)) to a
filename, resolved inside `scripts.dir`. The filename is looked up as
given — no `PATH` search, no shell expansion, no subdirectories implied
beyond what you write in the filename itself.

A script keybind lives in the same namespace as built-in action keybinds:
binding `"hyper+g"` to both a built-in action and a script (or to two
different scripts) is a duplicate-keybind validation error, same as two
built-in actions colliding.

## Validation

`hyperwm verify` (and daemon startup / `hyperwm reload`, which run the
same validation) checks every `[keybinds.scripts]` entry:

- The file must exist at `scripts.dir/<filename>` —
  otherwise: `keybind "hyper+g" references script "toggle_ghostty.sh",
  which doesn't exist at /Users/you/.config/hyperwm/scripts/toggle_ghostty.sh`.
- The file must have the executable bit set (`chmod +x`) — otherwise:
  `keybind "hyper+g" references script "toggle_ghostty.sh" at
  ..., which exists but isn't executable (missing the executable bit)`.

Both are hard validation errors, not warnings — a config referencing a
missing or non-executable script fails to load entirely (and, via
`hyperwm reload`, is rejected in favor of keeping the previous valid
config running).

## Execution

When a script keybind's `hyper+...` combination is pressed, the daemon
spawns the script as a **detached** child process (`Command::spawn`, not
waited on) — the daemon never blocks on a script finishing, no matter how
long it runs.

The daemon logs the script's stderr and any non-zero exit code, for
debugging — there's no UI or notification layer in v1, so these show up
only in the daemon's own log output (see `hyperwm daemon start`'s
`StandardOutPath`/`StandardErrorPath` in [installation.md](installation.md)
if running as a `launchd` agent, or the terminal it's running in if
started with `cargo run -p hyperwm-daemon` / run directly).

## Example

```sh
mkdir -p ~/.config/hyperwm/scripts
cat > ~/.config/hyperwm/scripts/toggle_ghostty.sh <<'EOF'
#!/bin/sh
open -a Ghostty
EOF
chmod +x ~/.config/hyperwm/scripts/toggle_ghostty.sh
```

```toml
[keybinds.scripts]
"hyper+g" = "toggle_ghostty.sh"
```

```
$ hyperwm verify
/Users/you/.config/hyperwm/config.toml: OK (11 built-in keybinds, 1 script keybinds)
```
