# hyperwm Architecture

This document is the frozen design reference for hyperwm. It defines _what_ the
system does and the _rules_ that govern behavior — not implementation details
left to the builder's discretion. Code should implement this spec; it should
not redesign it.

Target platform: macOS only, no cross-platform ambition.

---

## 1. System overview

Four subsystems:

```
Hyperkey Watcher  ──▶  Event Router  ──▶  Action Engine  ──┬─▶ Window Manager (AX API)
   (CGEventTap)         (keybind          (tiling/float/        │
                          lookup)           script dispatch)     └─▶ Script Runner (spawn)
                                                  │
                                                  ▼
                                          Window State Store
                                          (per-space BSP tree
                                           + float window list)
```

Plus a CLI (`hyperwm`) that communicates with the running daemon over a local
Unix domain socket for `reload`, `status`, `verify`.

The daemon runs as a `launchd` user agent.

---

## 2. Hyperkey

Caps Lock is fully replaced. There is no dual-mode (short-tap-as-caps-lock)
behavior — the native Caps Lock function ceases to exist once set up.

### 2.1 Remap layer (out of band, not daemon responsibility)

The physical Caps Lock key must be remapped to an unused HID keycode before
the daemon starts watching it. Two supported paths:

- **Manual**: user remaps Caps Lock → F18 via System Settings → Keyboard →
  Keyboard Shortcuts → Modifier Keys.
- **CLI-assisted**: `hyperwm install-keymap` shells out to `hidutil` to apply
  the same remap (`HIDKeyboardModifierMappingSrc` for Caps Lock →
  `HIDKeyboardModifierMappingDst` for F18), so the user doesn't have to click
  through System Settings.

Either way, the remap is a system-level concern outside the daemon's runtime
logic. The daemon does not implement its own caps-lock-disabling or
toggle-suppression logic — that entire class of complexity is eliminated by
delegating the remap to macOS itself.

### 2.2 Daemon-side watching

The daemon watches exactly one keycode (configurable, default F18) via a
`CGEventTap` at `kCGHIDEventTap`, listening for `flagsChanged` / key down-up
on that code.

- Key down on watched code → hyper-active = true
- Key up on watched code → hyper-active = false
- No hold-duration timing, no state machine. This is a plain modifier flag.

While hyper-active is true, all subsequent key events are intercepted and
routed to the Event Router instead of reaching the focused application. Keys
with no matching binding while hyper-active are swallowed (not forwarded) —
this is configurable in future but the default is swallow, to avoid
unexpected input landing in the focused app when a user mistypes.

### 2.3 Required permissions

- **Accessibility** (`AXIsProcessTrusted`) — required for AX window
  manipulation.
- **Input Monitoring** — required for the CGEventTap.

Both must be granted manually by the user via System Settings; neither can be
requested programmatically beyond prompting the user to go grant them. The
daemon must detect missing permissions at startup and print/log clear
instructions rather than failing silently or crash-looping.

---

## 3. Window tiling model

### 3.1 Data structure: per-space BSP tree

Each macOS Space has one BSP (binary space partition) tree. Nodes are one of:

- **Leaf**: holds exactly one tiled window reference.
- **Split**: internal node with `direction` (`horizontal` | `vertical`),
  `ratio` (0.0–1.0, fraction allocated to the first child), and two children
  (`first`, `second`).

`direction: vertical` means the split line is vertical (children arranged
left/right). `direction: horizontal` means the split line is horizontal
(children arranged top/bottom). This terminology must be used consistently
throughout code and docs.

Floating windows are **not** part of the tree. They live in a flat per-space
list carrying position, size, and z-order.

### 3.2 Tiled window cap

- `max_tiled_windows` (config, default 4) caps how many windows the tree
  manages.
- Windows beyond the cap are floated automatically (not an error state, not
  user-visible as unusual — it's just the defined behavior at the boundary).
- If a tiled window closes while floaters exist beyond the cap, no automatic
  promotion of a floater back into the tree happens. Floating is sticky once
  assigned. (Rationale: automatic promotion would move a window the user
  didn't touch, which is surprising. User can manually toggle it back to
  tiled with `hyper+f` if desired.)

### 3.3 Insertion rule (new window becomes tiled)

Applies only when current tiled count < `max_tiled_windows` and the window is
not in `always_float_apps`.

1. Locate the **leaf holding the currently focused tiled window**. If no
   tiled window is currently focused (e.g. focus is on a float, or this is
   the very first window), fall back to the most-recently-focused tiled leaf;
   if none exists (tree is empty), the new window becomes the tree's sole
   leaf (root).
2. Split that leaf. The leaf's window becomes one child, the new window
   becomes the other child (new window is placed as `second`, i.e.
   right/bottom, by default).
3. Split direction is chosen by `insert_heuristic` (config):
   - `aspect_ratio` (default): if the leaf's current on-screen rect is wider
     than tall, split `vertical` (left/right); if taller than wide, split
     `horizontal` (top/bottom).
   - `always_vertical`: always split left/right.
   - `always_horizontal`: always split top/bottom.
4. New split's `ratio` = `split_ratio_default` (config, default 0.5).

### 3.4 Removal rule (tiled window closes)

- The closed window's leaf is removed. Its sibling takes the place of their
  shared parent node (standard BSP collapse — the parent split node is
  replaced by the sibling subtree).
- If the removed leaf was the tree root (last window), the tree becomes
  empty.

### 3.5 Directional movement (`hyper+hjkl`)

This is a **swap of leaf contents**, driven purely by real-time geometry.
It must never depend on movement history or "remembered" prior positions —
every invocation recomputes from current on-screen rects only. This is a
deliberate constraint: history-based approaches produce unpredictable
behavior in asymmetric layouts and are explicitly rejected.

Given the focused tiled window `W` and a direction `d ∈ {left, right, up,
down}`:

1. Compute `W`'s current center point `(wx, wy)` from its live on-screen
   rect.
2. For every other tiled leaf `L` in the same tree, compute `L`'s center
   point and the vector from `W`'s center to `L`'s center.
3. Filter to candidates within a **±45° cone** around the direction vector
   for `d` (e.g. `d = right` → vector angle must be within 45° of due east,
   measured from positive x-axis, screen y-down coordinate convention noted
   explicitly in code comments to avoid sign errors).
4. Among filtered candidates, pick the one with the **smallest Euclidean
   distance** between centers.
5. **Tie-break** (exact angle/distance tie): pick the candidate with the
   lowest internal window ID (stable, deterministic — arbitrary but
   consistent, never ambiguous to the program even if visually ambiguous to
   a human).
6. If a candidate is found: **swap the window references held by `W`'s leaf
   and the candidate's leaf.** The tree shape (splits, ratios) does not
   change — only which window occupies which leaf changes. Focus follows the
   moved window (i.e. `W` remains focused after the move, now positioned at
   the former candidate's leaf).
7. If no candidate is found (window is at the tree's edge in that
   direction): no-op. Do not wrap around, do not re-parent nodes to create a
   new slot. (This may be revisited as a configurable option later but is
   explicitly out of scope for v1 — no-op is the only defined behavior.)

This rule is what resolves the asymmetric-layout ambiguity described in
design discussion: because step 1–4 are recomputed fresh from live geometry
on every keypress, there is no "which slot does it remember" question — the
program always asks "what is nearest right now" and answers deterministically
via distance, never via memory of past state.

### 3.6 Resize (`hyper+shift+hjkl`)

Adjusts the `ratio` of the nearest enclosing split in the direction pressed
(i.e. walk up from the focused leaf to find the first ancestor split whose
axis matches the direction; adjust its ratio by a configurable step, default
5% per press, clamped to a sane min/max e.g. 0.1–0.9 to prevent
degenerate/zero-size panes).

### 3.7 Floating window movement

Floating windows are **not** governed by the tree or the neighbor-swap
algorithm. `hyper+hjkl` on a focused floating window nudges its position by
a fixed step (`floating.float_move_step`, default 40px) in the pressed
direction. This is independent, simpler logic — no geometry search, just a
direct position delta.

### 3.8 Toggle float / maximize

- `hyper+f` — **toggle float**: if the focused window is tiled, remove it
  from the tree (3.4's removal rule applies to the tree) and add it to the
  floating list at its current on-screen rect (no jump). If already
  floating, attempt to insert it into the tree per the insertion rule (3.3);
  if `max_tiled_windows` is already reached, this is a no-op (window stays
  floating; do not silently evict another window to make room).
- `hyper+m` — **maximize**: resizes the focused window (tiled or floating)
  to fill its current display's visible frame, minus `gaps.outer` on all
  sides. This does not change tiled/float status or tree structure — it's a
  pure geometry operation. For a tiled window, maximizing is a _visual_
  override; on next tiling-affecting event (new window inserted, window
  moved, etc.) the window snaps back to its tree-computed rect. (Maximize is
  not a persistent state stored in the tree — it's not a "third mode.")

### 3.9 Always-float apps

`floating.always_float_apps` (config, list of app names or bundle IDs — bundle
ID preferred for reliability, app name accepted for convenience) — windows
belonging to these apps are never inserted into the tiling tree, regardless
of `max_tiled_windows` headroom. New windows from these apps go straight to
the floating list using `floating.new_float_placement` (`cascade` | `center`).

### 3.10 Externally-triggered geometry changes (manual drag/resize)

This section defines how the daemon reacts when a window's position or size
changes by a means other than hyperwm itself issuing the change (e.g. the
user drags or resizes a window by hand with the mouse).

**Tiled windows: the tree is authoritative, and drift is corrected
immediately.** The daemon subscribes to `AXWindowMoved`/`AXWindowResized`
notifications for tiled windows specifically in order to detect drift from
the tree's computed rect and correct it right away — not to track or cache
the window's live position. On each such notification:

1. Compare the window's current on-screen rect to the rect the tree says it
   should have.
2. If they differ, issue a corrective `set_position`/`set_size` call back to
   the tree's rect.
3. If they already match (e.g. a redundant notification from the same
   settled position), do nothing — no-op, not a repeated write.

A single drag gesture produces a burst of `AXWindowMoved` notifications (one
per intermediate frame of the drag, observed in practice to be on the order
of 10+ events for a short drag). This is expected AX behavior, not a bug.
The daemon must **deduplicate by comparing against the target rect** (step 3
above), not by time-based debouncing — a timer-based approach introduces a
window during which the daemon is knowingly stale, which solves nothing.
Comparing current-vs-target on every event and only acting when they differ
is correct regardless of how many redundant events arrive, and requires no
timing assumptions.

Net effect: dragging a tiled window snaps it back at or near real-time
(bounded by however quickly AX delivers the notification burst), not on the
next unrelated hyperkey action. This matches the behavior of established
tools in this space (e.g. yabai) and is the intended, opinionated feel of
hyperwm's tiling mode — a tiled window's position is not something the user
is meant to permanently change by dragging.

**Floating windows: no tracking, no caching — query live, on demand.**
The daemon does **not** subscribe to `AXWindowMoved`/`AXWindowResized` for
floating windows, and does not maintain a cached copy of a floating window's
position/size anywhere in its state. Whenever a floating window's current
geometry is needed for an operation (e.g. `hyper+f` toggling it back to
tiled, `hyper+m` maximizing it and later restoring), the daemon queries the
window's live position/size via AX **at the moment it's needed**, using the
same `position()`/`size()` calls unit 4 provides. This sidesteps staleness
entirely: there is no cache to go stale, because nothing is cached.

**Window lifecycle (`AXWindowCreated` / `AXUIElementDestroyed`) is always
observed**, for both tiled and floating windows, regardless of the above —
this is how the tree's insertion (§3.3) and removal (§3.4) rules get
triggered by real window creation/destruction, independent of whether the
change originated from a hyperkey action or externally (e.g. the user opens
a new document window, or quits an app). This is not optional and is
unaffected by the tiled/floating distinction above.

---

## 4. Gaps

- `gaps.outer` — pixels between tiled window edges and the screen edge /
  menu bar / dock-avoiding visible frame.
- `gaps.inner` — pixels between adjacent tiled windows (applied as half-gap
  on each side of a split boundary, so adjacent windows have exactly
  `inner` px between them, not `inner * 2`).
- Maximize (3.8) respects `gaps.outer` only (no inner gap applies to a single
  maximized window).
- Floating windows are not gap-constrained beyond initial placement
  (`cascade`/`center`); the user can move them anywhere including flush to
  screen edge.

---

## 5. Scripts

- `scripts.dir` (config, default `"scripts"`) resolves relative to the
  config file's own directory (i.e. `~/.config/hyperwm/scripts/` by default).
  An absolute path is also accepted.
- Any executable file in that directory can be bound under `[keybinds.scripts]`
  by filename (e.g. `"hyper+g" = "toggle_ghostty.sh"`).
- Execution: spawned as a detached child process (`Command::spawn`, not
  waited on). Daemon does not block on script completion.
- Daemon logs the script's stderr and non-zero exit codes for debugging, but
  does not surface them to the user beyond logs (no UI/notification layer in
  v1).
- `hyperwm verify` (CLI) must check: every filename referenced under
  `[keybinds.scripts]` exists in the resolved scripts dir and has the
  executable bit set. Missing or non-executable scripts are a verify error,
  not a warning.

---

## 6. Config reload

- `hyperwm reload` (CLI) sends a request over the Unix socket to the running
  daemon, which re-reads and re-validates the config file.
- If the new config fails validation, the daemon **keeps running on the
  previous valid config** and reports the validation error back to the CLI
  caller — it must never crash or drop to an unconfigured state on a bad
  reload.
- Reload does not tear down or rebuild existing window trees; it only
  replaces keybind/gap/threshold/etc. values going forward. (Live windows
  keep their current tree positions.)

---

## 7. Explicit non-goals (v1)

These are deliberately out of scope, to prevent scope creep during
implementation:

- No private/undocumented macOS APIs (no SkyLight, no SIP-partial-disable
  dependent features). Public Accessibility API and CGEventTap only.
- No cross-platform support.
- No space-switching automation / space-aware window movement across spaces.
- No animation of window movement/resizing (instant reposition only).
- No GUI/menu-bar app — CLI + daemon only.
- No automatic promotion of floating windows back into the tiling tree on
  tree space becoming available (see 3.2).
- No wraparound on directional movement at tree edges (see 3.5 step 7).
