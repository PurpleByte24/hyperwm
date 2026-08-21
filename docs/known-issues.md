# Known issues

Platform/app limitations discovered during manual verification, kept here
so later units don't have to rediscover them. These are not bugs in
hyperwm's AX calls -- they're the target app declining to honor the
request.

## Chrome/Electron windows inconsistently honor programmatic resize

**Observed**: Google Chrome (build unit 4 manual verification,
2026-08-21, `cargo run -p hyperwm-macos --example move_window --
"Google Chrome"`), tested twice against the same window
("Who's using Chrome?").

`AXUIElementSetAttributeValue` was called with `kAXPositionAttribute` =
(100, 100) and `kAXSizeAttribute` = 800x600 both times. Reading the
attributes back immediately after:

- Position: (100, 100) both times -- honored exactly.
- Size: **inconsistent across the two runs**. Once, height came back
  652 instead of the requested 600 (width honored, height silently
  clamped to an apparent internal minimum). The other time, both
  width and height came back exactly as requested, no clamping.

No error was returned from `AXUIElementSetAttributeValue` in either
case -- the call reports success regardless of whether the requested
size was actually applied.

Chrome/Electron windows have been observed to inconsistently honor
programmatic resize -- sometimes exact, sometimes clamped to an apparent
internal minimum. The cause has not been fully diagnosed (candidates
include window state at resize time, e.g. whether a given pane/sidebar
was rendered, or some internal debounce/animation racing the AX call).
**Don't assume either behavior when building on top of this** -- treat
Chrome/Electron resize as unreliable rather than "clamped" or "exact."

TextEdit, by contrast, honored the same kind of move/resize exactly with
no clamping every time it was tested (see the same manual verification
pass).

**Why this isn't a hyperwm bug**: hyperwm-macos's `set_size`/`set_position`
(`crates/hyperwm-macos/src/ax/element.rs`) do exactly what the public AX
API contract promises -- construct the `AXValue`, call
`AXUIElementSetAttributeValue`, and check the returned `AXError`. There is
no private/undocumented API that would let hyperwm force a specific size;
if `kAXErrorSuccess` comes back but the app internally overrides the
value (consistently or not), that's the app's own AX implementation,
outside anything hyperwm can control while staying within the
public-API-only constraint (CLAUDE.md's hard constraints,
architecture.md §7).

**Why it matters for later units**: architecture.md §3's BSP tiling math
computes tree-derived rects and hands them to the window manager assuming
they'll be applied as requested. Once real AX calls are wired into
`hyperwm-core` (build unit 5) and gaps math (architecture.md §4), an
unreliably-applied resize means the window's *actual* on-screen rect can
end up different from what the tree thinks it assigned -- e.g. a
taller-than-requested window could overlap its sibling below it, or
leave an unexpected gap. Because the behavior isn't even consistent for
the same app/window, this can't be special-cased per-app with confidence.
Unit 5 (or whichever unit first depends on requested-size accuracy)
should decide whether to:

- re-read the window's actual rect after every resize and reconcile, or
- accept some drift, since architecture.md's directional-movement rule
  (§3.5) already recomputes from live geometry on every keypress rather
  than trusting remembered state, which naturally self-corrects most of
  the time -- and see architecture.md §3.10 (added after this unit
  started), which already defines exactly this reconciliation for
  externally-triggered geometry changes and may be the right mechanism
  to reuse here too.

This is a decision for that unit, not resolved here -- flagging it now
per CLAUDE.md's instruction to report AX resize/move quirks as soon as
they're found rather than discovering them later.

## Build unit 5: known limitations of the daemon wiring

Recorded at implementation time (2026-08-21), not yet confirmed or
contradicted by manual verification.

**Single global tree, not one tree per macOS Space.** architecture.md §3.1
scopes one BSP tree per Space. There is no public API to learn which Space
(or which display, for multi-monitor setups where displays have separate
Spaces) a given window belongs to -- the private `CGSCopySpacesForWindows`
family is the only way, and CLAUDE.md's hard constraints rule out private
APIs. `hyperwm-daemon` therefore runs exactly one tree for the whole
session, laid out against whichever display its own tiled windows are
currently on (falls back to the primary display if the tree is empty). On
a single-display machine this matches the spec exactly. With multiple
displays or multiple Spaces in use, expect best-effort behavior, not
per-Space isolation -- this wasn't a choice among equally-valid options,
it's what's left once private APIs are off the table.

**AppKit dependency beyond the literal "AX + CGEventTap only" wording.**
Two things architecture.md needs have no CoreGraphics/Accessibility-only
public API: the Dock/menu-bar-aware "visible frame" (§4's gaps math) and
learning that a new app has launched, in order to attach an `AXObserver`
to it (§3.10's "always observed" lifecycle rule). Both are available only
through AppKit (`NSScreen.visibleFrame`, `NSWorkspace`'s launch
notification) -- fully public, documented APIs, but outside CLAUDE.md's
literal two-API list. Confirmed with the user before adding
`objc2`/`objc2-app-kit`/`objc2-foundation`/`block2` to `hyperwm-macos`
(see `crates/hyperwm-macos/src/screen.rs` and `src/workspace.rs`'s module
docs for the specifics) rather than deciding unilaterally.

**Per-app `AXObserver`s aren't cleaned up when an app quits.** Each pid
gets one `AXObserver` (`DaemonState::app_observers`), created once and
never removed, even after every window it watched has closed and the
process has terminated. This doesn't cause incorrect behavior (a dead
pid's observer just never fires again), but it's an unbounded-over-a-long-
enough-session resource leak. `NSWorkspace` also offers a
`didTerminateApplicationNotification` that could drive cleanup
symmetrically with the launch-notification adoption path; not implemented
in this unit for scope reasons.

**Cascade placement's pixel step is an implementation default, not a
config value.** `floating.new_float_placement = "cascade"`
(architecture.md §3.9, examples/config.toml) specifies the strategy
("offset from the previous floating window's position") but not a pixel
amount. `hyperwm-daemon` uses a fixed 32px step, wrapping after 8 windows,
as an internal constant (`state.rs`'s `CASCADE_STEP`/`CASCADE_WRAP`) --
this is implementation-level judgment, not a config-schema decision, so it
wasn't raised as a question, but it's worth knowing about if cascade
placement looks off during manual verification.
