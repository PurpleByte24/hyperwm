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

1. Locate the **tiled leaf with the largest on-screen area** (`width *
   height` of its current on-screen rect). If the tree is empty, the new
   window becomes the tree's sole leaf (root) instead. **Tie-break** (exact
   area tie): pick the candidate with the lowest internal window ID (same
   convention as §3.5 step 5 — stable, deterministic, never ambiguous to the
   program). This rule is deliberately **not** focus-based: it doesn't matter
   which window the user last focused, only which pane currently has the
   most space to give up — so a sequence of new windows self-balances toward
   roughly equal panes regardless of click/focus order.
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

**Tiled windows: the tree is authoritative, and drift is corrected once the
mouse button is released — never while it's held.** The daemon subscribes to
`AXWindowMoved`/`AXWindowResized` notifications for tiled windows
specifically in order to detect drift from the tree's computed rect and
correct it, and separately watches the left mouse button's own down/up state
via a `CGEventTap` (`ListenOnly` — it never intercepts or alters a
click/drag, purely observes). The two combine as follows:

1. While the left mouse button is down, every `AXWindowMoved`/
   `AXWindowResized` notification is ignored outright — no comparison, no
   write, not even a queued correction. The window is free to move/resize
   exactly as the user drags it, with no fighting.
2. The instant the button is released, sweep every currently tiled window
   once: compare its current on-screen rect to the rect the tree says it
   should have, and issue a corrective `set_position`/`set_size` call back to
   the tree's rect for any that differ. A window whose rect already matches
   (e.g. no drag touched it) is left untouched — no-op, not a redundant
   write.
3. Outside of an active drag (button up throughout), correction still
   happens on every `AXWindowMoved`/`AXWindowResized` notification,
   comparing current-vs-target and writing back only on a mismatch — this
   covers geometry changes that aren't a hand drag at all (e.g. another
   process repositioning a window). The daemon must **deduplicate by
   comparing against the target rect**, not by time-based debouncing — a
   timer-based approach introduces a window during which the daemon is
   knowingly stale, which solves nothing.

Net effect: dragging or resizing a tiled window moves/resizes it freely for
as long as the button is held, then snaps to its tree rect the instant the
button comes up — not mid-drag, and not deferred to the next unrelated
hyperkey action either. This is a deliberate v1 revision from the daemon's
first cut (which corrected on every notification with no mouse-state gate at
all, fighting slower drags before release): a tiled window's position is
still not something the user is meant to permanently change by dragging, but
the correction should never visibly contest the drag itself.

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

### 3.11 Space switching

There is no reliable public API for stable per-window Space identity
(`kCGWindowWorkspace` is deprecated since macOS 10.8 and not populated on
modern macOS; the only real per-window→Space mapping is via private
SkyLight/CGS calls, which are out of scope per §7). hyperwm therefore does
not maintain a persistent tree per Space. Instead, it uses a lighter,
stateless-by-default approach built entirely on `optionOnScreenOnly`
enumeration, on a fixed poll interval (0.5s).

**Not event-driven.** The natural public-API mechanism for this is
`NSWorkspace.activeSpaceDidChangeNotification`, and that was the original
design. In practice, confirmed by running the real daemon and repeatedly
switching Spaces, that notification never fires for this daemon at all —
not after establishing a window-server connection via `NSApplication`, not
after setting an `.accessory` activation policy and calling
`finishLaunching`, not after explicit main-queue delivery, and not after
granting Screen Recording. The most likely remaining explanation is that
this notification requires a proper Launch-Services-registered `.app`
bundle, which a bare `cargo run`/CLI executable isn't (that's build unit
8's packaging work, not done as of this writing). Polling needs no
bundling and is fully public API, so hyperwm-macos's
`workspace::watch_space_changes` polls instead, at the cost of up to the
poll interval's worth of latency recognizing a switch — never a
correctness issue, since step 3 below never writes to AX either way, so a
slightly-late passive adopt/no-op is harmless. If a future packaging pass
(build unit 8) resolves the underlying notification issue, switching back
to an event-driven watch is a candidate follow-up, not a behavioral
change to anything below.

**On a Space switch** (i.e. each time the poll notices the on-screen
window set differs from what's currently active):

1. Enumerate the currently on-screen windows
   (`kCGWindowListOptionOnScreenOnly`). This is always exactly "whatever
   Space is now active," with no Space-ID bookkeeping required.
2. Check a lightweight in-memory cache, keyed by the current on-screen
   window set (window IDs), for a previously-seen layout matching that
   exact set. If found, and the windows' current on-screen rects already
   match that cached layout within tolerance, do nothing. This is the
   cheap, common-case path — most Space visits are to a Space you were
   just on, unchanged.
3. If there's no cache hit, or the current rects don't match the cached
   layout: hyperwm does **not** automatically recompute or move anything.
   It leaves the current on-screen arrangement as-is, whatever it is
   (already-tiled-looking, ad-hoc, freshly opened windows, doesn't
   matter), and simply adopts that arrangement as the new known state
   going forward (subsequent hyperkey actions, drift correction, etc. all
   operate against it as found).
4. Whenever a layout is established or changes (via step 3 adoption, or
   via any hyperkey action), cache it against the current window-ID set for
   future fast-path matching in step 2.

**Auto-tile keybind.** Since hyperwm does not automatically force a fresh
computed layout on an unrecognized Space (step 3 above is deliberately
passive), the user has an explicit keybind — bound in
`[keybinds]` as `"hyper+<key>" = "auto_tile"` (exact key left to the user's
config, no default reserved yet) — that forces the current on-screen
window set (up to `max_tiled_windows`) into a freshly computed tree layout
immediately, using the same insertion rule as §3.3 (largest-on-screen-area
leaf, split per `insert_heuristic`), applied in on-screen enumeration
order. This is the explicit, user-triggered escape hatch for "this Space
doesn't look right, make it look right" — it is never invoked
automatically by hyperwm itself, including on first visit to a new Space.

**No resizing during passive adoption (step 3).** Because hyperwm cannot
distinguish "this Space's windows are already correctly tiled" from "this
Space has an ad-hoc arrangement the user wants left alone" without a
persistent tree to compare against, step 3 never resizes or repositions
anything on its own. The only way tiling is forcibly applied to an
unrecognized Space's windows is the explicit `auto_tile` keybind above.

**Explicit non-goals carried over from this design**: no true persistent
per-Space tree state across arbitrarily long absences from a Space: if a
Space's window set or arrangement changes while hyperwm isn't observing it
(e.g. another tool moved something, or this is genuinely the first visit),
there is no "remembered original layout" to restore — only whatever the
cache last captured for that exact window-ID set. Multi-display Space
sequencing and windows dragged between Spaces mid-session remain out of
scope, per the existing non-goals in §7.

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
- No persistent per-Space tree state or stable Space identity (see §3.11 —
  not achievable with public APIs; hyperwm uses cache-by-window-set instead).
- No automatic window movement between Spaces, and no support for a window
  being dragged between Spaces mid-session by the user.
- No multi-display Space sequencing awareness.
- No animation of window movement/resizing (instant reposition only).
- No GUI/menu-bar app — CLI + daemon only.
- No automatic promotion of floating windows back into the tiling tree on
  tree space becoming available (see 3.2).
- No wraparound on directional movement at tree edges (see 3.5 step 7).
