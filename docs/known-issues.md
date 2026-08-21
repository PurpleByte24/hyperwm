# Known issues

Platform/app limitations discovered during manual verification, kept here
so later units don't have to rediscover them. These are not bugs in
hyperwm's AX calls -- they're the target app declining to honor the
request.

## Some apps clamp programmatic resize to a minimum size

**Observed**: Google Chrome (build unit 4 manual verification,
2026-08-21, `cargo run -p hyperwm-macos --example move_window --
"Google Chrome"`).

`AXUIElementSetAttributeValue` was called with `kAXPositionAttribute` =
(100, 100) and `kAXSizeAttribute` = 800x600 against Chrome's focused
window ("Who's using Chrome?", initially at (208, 20), 1024x822).
Reading the attributes back immediately after:

- Position: (100, 100) -- honored exactly.
- Size: 800x652 -- width honored, **height silently clamped to 652**
  instead of the requested 600. No error was returned from
  `AXUIElementSetAttributeValue`; the call reports success and the
  window simply ends up larger than asked.

This matches the general, previously-known behavior of Chrome/Electron
apps under the public Accessibility API: they impose their own minimum
content size (here, apparently around 652px tall for this window) and
silently clamp AX resize requests that would go below it, rather than
rejecting the request or reporting the actual applied size as an error.

TextEdit, by contrast, honored the same kind of move/resize exactly with
no clamping (see the same manual verification pass).

**Why this isn't a hyperwm bug**: hyperwm-macos's `set_size`/`set_position`
(`crates/hyperwm-macos/src/ax/element.rs`) do exactly what the public AX
API contract promises -- construct the `AXValue`, call
`AXUIElementSetAttributeValue`, and check the returned `AXError`. There is
no private/undocumented API that would let hyperwm force a smaller size;
if `kAXErrorSuccess` comes back but the app internally overrides the
value, that's the app's own AX implementation, outside anything hyperwm
can control while staying within the public-API-only constraint
(CLAUDE.md's hard constraints, architecture.md §7).

**Why it matters for later units**: architecture.md §3's BSP tiling math
computes tree-derived rects and hands them to the window manager assuming
they'll be applied as requested. Once real AX calls are wired into
`hyperwm-core` (build unit 5) and gaps math (architecture.md §4), a
clamped resize means the window's *actual* on-screen rect can end up
different from what the tree thinks it assigned -- e.g. a clamped-taller
window could overlap its sibling below it, or leave an unexpected gap.
Unit 5 (or whichever unit first depends on requested-size accuracy)
should decide whether to:

- re-read the window's actual rect after every resize and reconcile, or
- accept some drift for apps that clamp, since architecture.md's
  directional-movement rule (§3.5) already recomputes from live geometry
  on every keypress rather than trusting remembered state, which
  naturally self-corrects most of the time.

This is a decision for that unit, not resolved here -- flagging it now
per CLAUDE.md's instruction to report AX resize/move quirks as soon as
they're found rather than discovering them later.
