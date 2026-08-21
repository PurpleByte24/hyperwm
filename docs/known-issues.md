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
