# hyperwm Roadmap

This document tracks features and improvements deliberately deferred
during hyperwm's initial build (v0.1.0–v0.2.2). None of this is committed
to a timeline — it's a record of ideas raised and _why_ they were
deferred, so they don't get lost or re-litigated from scratch later.

Nothing here is part of the frozen spec in `docs/architecture.md` yet. If
and when any of these move from "idea" to "in progress," the relevant
design should be written into `docs/architecture.md` first, same as every
other feature in this project, before implementation begins.

---

## Configuration UX

The config file (`~/.config/hyperwm/config.toml`) currently requires
hand-editing TOML. The plan is to make this optional, not to replace the
file format itself — the TOML file remains the source of truth; these
tools read and write it.

### Interactive config tool (`hyperwm wizard` / `hyperwm setup`)

One underlying interactive terminal tool with two entry modes, not two
separate tools — both should share the same rendering, validation, and
save logic rather than being built independently:

- **Guided first-run mode** (`hyperwm wizard`, or triggered automatically
  via a `first_setup` flag when no config exists yet): a linear series of
  questions producing a complete `config.toml` from scratch. Should
  cover, at minimum (list can grow):
  - Movement keys: vim-style (hjkl) vs. arrow keys
  - Whether to enable resize (`hyper+shift+<key>`) and on which
    modifier combination
  - Outer and inner gap sizes
  - Helping the user find and select apps that should always float
    (`floating.always_float_apps`) — likely by listing currently running
    apps and letting them multi-select, rather than requiring them to
    know bundle IDs upfront
- **Ongoing edit mode** (running the same tool without the wizard flag,
  once a config already exists): a fuller TUI — checkboxes/toggles for
  boolean and enum-like settings, a save action, and a flow for adding
  new keybinds that includes a file selector step for binding a script
  (browsing `scripts.dir` rather than requiring the user to type a
  filename correctly).

Open questions to resolve before implementation, worth designing
deliberately rather than guessing mid-build (same discipline as
everything else in this project):

- TUI library choice (needs to be evaluated against what's idiomatic and
  well-maintained in the Rust ecosystem at implementation time).
- How the tool avoids clobbering hand-edited config content the user may
  have added outside what the tool understands (e.g. comments, a script
  keybind added manually) — a naive "regenerate the whole file" approach
  would be destructive; needs a real answer.
- Exact validation/error-display UX when a user's choices would produce
  an invalid config.

### Why not a menu bar app for this

Explicitly considered and rejected for the _configuration_ UX
specifically (a menu bar _presence_ is still a separate, open idea below)
— the preference is a polished terminal tool over a GUI settings panel,
partly on principle (keeping this a keyboard/terminal-centric tool) and
partly to avoid the larger scope of building and maintaining GUI
settings UI on top of everything else.

---

## Daemon lifecycle & distribution

### Start at login

`RunAtLoad` on the daemon's LaunchAgent is currently `false` — starting
the daemon is a manual `hyperwm daemon start`. Making this automatic at
login was deliberately deferred during unit 8 so packaging could ship
without also taking on "should this behave differently for users who
don't want auto-start" as an open question. Small addition once
prioritized — likely just a config flag or a CLI flag on `install-keymap`
or a new `hyperwm daemon enable-at-login` command.

### Proper code signing (Developer ID + notarization)

The release binary is currently ad-hoc signed (fixed in v0.2.2 after a
real bug where `lipo -create`-combined universal binaries lost a valid
signature, causing TCC permission checks to fail specifically when
launched via `launchctl`). Ad-hoc signing works for this, but:

- Requires re-granting Accessibility/Input Monitoring on some rebuilds,
  since ad-hoc signatures don't carry a stable identity the way a real
  Developer ID signature does.
- Will show Gatekeeper warnings for anyone other than the developer
  installing via Homebrew.

Proper Developer ID signing + notarization (requires an Apple Developer
Program membership, $99/year) would resolve both. Worth doing if/when
this is ever meant for real use beyond the original author.

---

## Window management features

### Drag-to-swap

Dragging a tiled window and releasing it on top of another tiled
window's position should swap the two windows — a mouse-driven
equivalent of `hyper+hjkl`'s geometric swap (architecture.md §3.5).

**Needs explicit reconciliation with §3.10 (drift correction) before
implementation, not just a bolt-on**: §3.10 currently treats any
release-position mismatch for a tiled window as drift to be corrected
back. Drag-to-swap needs a rule for distinguishing "released on top of
another tiled window (intentional swap)" from "released somewhere else
entirely (drift, correct it)" — likely: if the release position
overlaps/hovers another tiled leaf's rect, treat it as a swap; otherwise,
existing §3.10 drift correction applies unchanged. This needs to be
written into architecture.md as a real design section before any code is
written, same process as every other behavioral rule in this project.

### Quieter AXObserver logging

`daemon.err.log` currently logs a "couldn't watch window creation"
warning for every system process and XPC helper (WindowManager, Dock,
UniversalControl, WebKit's networking/GPU/WebContent helpers, etc.) that
hyperwm attempts (and correctly fails) to attach an observer to. This is
expected, correct behavior — none of these have real windows to track —
but it's noisy. Low-priority cleanup: filter out known system/helper
processes before attempting to attach an observer at all, rather than
trying and logging a failure for each one.

---

## CLI / status output polish

### `hyperwm status` visual polish

Current output is plain, undifferentiated text. Wanted:

- Color-coded state (e.g. daemon running vs. not, tiled vs. floating
  window lists visually distinguished).
- Show which config file(s) were found/considered, not just the one that
  was loaded (useful when both `~/.config/hyperwm/config.toml` and
  `~/.hyperwm/config.toml` exist, or neither does).
- A polished, informative view specifically for the "daemon isn't running
  yet" case — currently just a raw connection-refused error:
  ```
  hyperwm status: couldn't connect to hyperwm-daemon at
  /var/folders/.../hyperwm.sock (Connection refused (os error 61)) --
  is the daemon running?
  ```
  This should look like a deliberate, designed output (e.g. clearly
  state the daemon isn't running, suggest `hyperwm daemon start`, maybe
  still show which config would be used) rather than a raw error
  message leaking through.

---

## Explicitly not on this list

Non-goals from `docs/architecture.md` §7 (no private/undocumented APIs,
no cross-platform support, no true persistent per-Space tree state, no
cross-Space window movement, no multi-display Space sequencing, no
animation) remain non-goals. Nothing above is meant to revisit those.
