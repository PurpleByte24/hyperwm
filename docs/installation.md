# Installation

## 1. Install

```sh
brew tap PurpleByte24/hyperwm
brew install hyperwm
```

This installs two binaries: `hyperwm` (the CLI) and `hyperwm-daemon` (the
window manager itself, run as a background `launchd` agent — you don't
invoke it directly). Confirm both landed:

```sh
which hyperwm hyperwm-daemon
hyperwm --version
```

## 2. Write a config

hyperwm looks for a config at `~/.config/hyperwm/config.toml` (preferred)
or `~/.hyperwm/config.toml` (fallback). Start from the annotated example:

```sh
mkdir -p ~/.config/hyperwm
cp "$(brew --prefix)/opt/hyperwm/examples/config.toml" ~/.config/hyperwm/config.toml \
  2>/dev/null || curl -fsSL \
  https://raw.githubusercontent.com/PurpleByte24/hyperwm/main/examples/config.toml \
  -o ~/.config/hyperwm/config.toml
```

(The Homebrew formula doesn't currently ship `examples/config.toml` as an
installed resource, so the `curl` fallback above is the reliable path
until that's added — see the formula at
`https://github.com/PurpleByte24/homebrew-hyperwm` if you want to confirm.)

Validate it before going further:

```sh
hyperwm verify
```

See [configuration.md](configuration.md) for the full field reference and
[keybinds.md](keybinds.md)/[scripts.md](scripts.md) for keybind and script
setup.

## 3. Remap Caps Lock to the hyperkey

`config.toml`'s `hyperkey.watch_keycode` (default `"f18"`) names a key
that must already be remapped from Caps Lock at the system level —
`hyperwm-daemon` only *watches* that key, it never performs the remap
itself (architecture.md §2.1). Two ways to do this:

### Option A: `hyperwm install-keymap` (recommended)

```sh
hyperwm install-keymap
```

This shells out to `hidutil` to remap Caps Lock to whatever key your
config's `hyperkey.watch_keycode` names (or `"f18"` if no config exists
yet), applies it immediately for the current session, and installs a
`launchd` LaunchAgent (`RunAtLoad = true`, separate from the daemon's own
agent) that reapplies the same remap at every future login/reboot —
`hidutil property --set` on its own is a live, in-memory remap with no
persistence, so without this LaunchAgent it would silently revert on
every logout.

Only function keys (`f1`–`f20`) are supported as a remap target — see
[keybinds.md](keybinds.md#key-names) for why. If your config uses a
different kind of key, remap manually instead (Option B).

Re-running `hyperwm install-keymap` after changing
`hyperkey.watch_keycode` updates both the live remap and the login
LaunchAgent to match.

### Option B: Manual (System Settings)

System Settings → Keyboard → Keyboard Shortcuts… → Modifier Keys → set
Caps Lock to your chosen key. This persists on its own (it's a normal
system preference), no LaunchAgent involved.

Either way, once remapped, Caps Lock's native function is gone entirely —
there's no dual-mode "still works as Caps Lock sometimes" behavior
(architecture.md §2).

## 4. Grant permissions

`hyperwm-daemon` needs two permissions, both grantable only by you via
System Settings (there's no way to request either fully programmatically):

- **Accessibility** — System Settings → Privacy & Security →
  Accessibility, then enable it for `hyperwm-daemon` (add it with "+" if
  it isn't listed). Required for AX window manipulation.
- **Input Monitoring** — System Settings → Privacy & Security → Input
  Monitoring, then enable it for `hyperwm-daemon` (add it with "+" if it
  isn't listed). Required for the `CGEventTap` that watches the hyperkey.

The first `hyperwm daemon start` (next step) triggers the native macOS
prompts for both if they've never been granted or denied before; if
either was already denied, or you dismissed a prompt, the daemon prints
the exact instructions above and exits rather than crash-looping — run
`hyperwm daemon start` again after granting.

## 5. Start the daemon

```sh
hyperwm daemon start
```

Runs `hyperwm-daemon` as a `launchd` user agent
(`~/Library/LaunchAgents/com.purplebyte24.hyperwm.daemon.plist`,
`RunAtLoad = true`, logging to `~/Library/Logs/hyperwm/daemon.log` and
`daemon.err.log`), so it comes back automatically on future logins
without running this again. Calling `start` again while it's already
running is a no-op (it checks `launchd`'s state first, rather than
killing and respawning a live daemon — see `hyperwm-cli/src/daemon.rs`
if you're curious about the exact check).

```sh
hyperwm daemon stop      # stop it (no-op if already stopped)
hyperwm daemon restart   # kill and respawn, e.g. after a config change
                          # you'd rather apply as a fresh process than
                          # via `hyperwm reload`
```

Confirm it's running and see current state:

```sh
hyperwm status
```

After editing `config.toml`, prefer `hyperwm reload` over
`daemon restart` for keybind/gap/threshold changes — it keeps existing
windows' tree positions in place (architecture.md §6); `daemon restart`
is for when you want an entirely fresh daemon process.

## Uninstalling

```sh
hyperwm daemon stop
launchctl bootout gui/$(id -u)/com.purplebyte24.hyperwm.daemon 2>/dev/null
launchctl bootout gui/$(id -u)/com.purplebyte24.hyperwm.keymap 2>/dev/null
rm -f ~/Library/LaunchAgents/com.purplebyte24.hyperwm.{daemon,keymap}.plist
brew uninstall hyperwm
```

The keymap LaunchAgent's removal only stops *future* logins from
reapplying the remap — Caps Lock stays remapped for the rest of the
current session until you log out, or reverse it: System Settings →
Keyboard → Keyboard Shortcuts… → Modifier Keys → Caps Lock → Caps Lock
(the default).
