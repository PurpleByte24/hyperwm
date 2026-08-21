# hyperwm — instructions for Claude Code

hyperwm is a keyboard-driven tiling window manager for macOS, written in
Rust. Hyperkey-triggered (Caps Lock remapped to F18), BSP-tree tiling with a
float mode, per-app float rules, and user-defined scripts bound to hyper
keybinds.

**Before writing any code, read `docs/architecture.md`.** It is the frozen
design spec — data structures, algorithms, and behavioral rules are defined
there and are not open to reinterpretation. If something seems ambiguous or
you think a different approach would be better, stop and ask rather than
deciding unilaterally. Do not restate or duplicate architecture.md's content
in code comments at length — reference it (e.g. `// see docs/architecture.md
§3.5`) instead of re-explaining the algorithm inline.

`examples/config.toml` is the frozen config schema contract. Implement
parsing/validation to match it exactly. If the schema needs to change, that's
a discussion to have explicitly, not a silent deviation during
implementation.

## Hard constraints

- **macOS only.** No cross-platform abstraction layers, no `cfg(target_os)`
  scaffolding for platforms that will never be supported. Don't add
  generality nothing will use.
- **Public APIs only.** Accessibility API (`AXUIElement`) and `CGEventTap`
  only. No private/undocumented frameworks (no SkyLight, no anything
  requiring SIP partial disable). If a feature seems to require a private
  API, that feature is out of scope — flag it, don't implement it.
- **No caps-lock-toggle-suppression logic.** The daemon watches a single
  remapped keycode (default F18) as a plain modifier. Do not build a
  hold-duration state machine for this — that complexity was deliberately
  eliminated (see architecture.md §2).
- **No automatic promotion of floats back to tiled, no wraparound on
  directional movement at tree edges.** These are explicitly defined as
  no-ops in architecture.md §3.2 and §3.5 — don't "improve" them
  unprompted.

## Build order (do not reorder or parallelize across units)

Work through these as separate, reviewable units. Do not start a later unit
until the current one meets its definition of done. Each unit should be its
own set of commits, not one giant commit at the end.

1. **Workspace scaffold + `hyperwm-core`.** Cargo workspace layout
   (`hyperwm-core`, `hyperwm-macos`, `hyperwm-config`, `hyperwm-daemon`,
   `hyperwm-cli` crates). `hyperwm-core` implements the BSP tree, insertion
   rule, removal rule, directional-movement neighbor algorithm, and resize
   logic from architecture.md §3. This crate must have **zero macOS-specific
   dependencies** — it operates purely on synthetic window rects, so it's
   testable without a real window server.

   _Definition of done_: `cargo test -p hyperwm-core` passes with, at
   minimum, explicit test cases for: (a) a symmetric 2x2 grid, (b) the
   asymmetric 2-left/1-right case from architecture.md §3.5 — move top-left
   window right, then back left, and assert it lands on the geometrically
   correct leaf, (c) directional movement at a tree edge (no-op case), (d)
   insertion when tree is empty, (e) insertion when max_tiled_windows is
   reached (window should not enter tree), (f) removal/collapse when a
   non-root leaf closes, (g) removal when the last window closes (tree
   becomes empty). `cargo clippy -p hyperwm-core` is clean.

2. **`hyperwm-config`.** Parsing (toml) and validation matching
   `examples/config.toml`'s schema. Implements the checks needed by
   `hyperwm verify` (see architecture.md §5 for script-specific validation
   rules).

   _Definition of done_: valid example config parses with no errors;
   deliberately broken configs (missing required field, bad keybind syntax,
   script reference to nonexistent file) each produce a specific, readable
   error — not a generic parse failure.

3. **`hyperwm-macos` step 1: CGEventTap + permissions.** Get the event tap
   running, watching the configured keycode, logging key events to stdout.
   Detect missing Accessibility/Input Monitoring permissions at startup and
   print clear instructions (System Settings path) rather than failing
   silently.

   _Definition of done_: running the daemon with permissions granted logs
   hyper-active state changes correctly; running without permissions prints
   an actionable message instead of crashing or hanging.

4. **`hyperwm-macos` step 2: AX API window manipulation.** Enumerate windows,
   read/set position and size, get focused window, observe
   creation/destruction/move via `AXObserver`.

   _Definition of done_: a manual test (documented in the crate's
   README/doc-comment) can move and resize at least one real window via AX
   calls driven by a hardcoded rect.

5. **Wire `hyperwm-core` into real AX calls via the daemon.** Event Router +
   Action Engine from architecture.md §1, connecting keybind matches to tree
   operations to actual AX window moves.

   _Definition of done_: hyper+hjkl, hyper+shift+hjkl, hyper+f, hyper+m work
   end-to-end against real windows, matching architecture.md §3.5–3.8
   exactly.

6. **Script runner + `[keybinds.scripts]`.** Detached spawn, stderr/exit
   code logging, per architecture.md §5.

7. **CLI socket client.** `hyperwm reload`, `hyperwm status`, `hyperwm
verify`, `-h`/`--help`, `-v`/`--version`. Reload must follow the
   keep-last-good-config-on-failure rule (architecture.md §6).

8. **Packaging.** launchd plist + `hyperwm daemon start|stop|restart`,
   `hyperwm install-keymap` (hidutil remap convenience), release workflow,
   Homebrew formula (separate `homebrew-hyperwm` repo, not part of this
   build order).

## Testing expectations

- Every function in `hyperwm-core` gets unit tests. This crate carries the
  algorithmic risk of the whole project (see architecture.md §3.5's
  rationale for why geometry-driven, history-free movement matters) — do not
  under-test it.
- Don't invent test scenarios loosely; use the specific cases listed in unit
  1's definition of done as a minimum, add more as needed for coverage.
- `hyperwm-macos` and `hyperwm-daemon` are harder to unit test (real window
  server dependency) — document manual verification steps where automated
  tests aren't practical, don't skip verification entirely.

## Commit discipline

- Small, working commits per logical change. Each commit should leave the
  workspace in a compiling state.
- Commit messages describe _what_ and _why_ in one line; no need for
  verbose bodies unless the change is non-obvious.
- Don't mix unrelated changes (e.g. a core algorithm fix and a CLI flag
  addition) in one commit.

## When something is genuinely ambiguous

Stop and ask rather than picking silently, especially for anything touching:
the BSP movement/insertion rules, config schema shape, or permission-handling
UX. Implementation-level choices (internal function naming, module layout
within a crate, error type design) don't need sign-off — use judgment.
