# Changelog

All notable changes to this project are documented here.
Format based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.3.0] - 2026-09-08

### Added

- `hyperwm status`: color-coded daemon state and tiled/floating window
  lists, and every default config path considered (not just the one
  loaded), shown with which one is actually in play
- `hyperwm status`: a designed "daemon isn't running" view (state, config
  paths considered, a `hyperwm daemon start` hint) in place of the raw
  connection-refused error

## [0.2.2] - 2026-09-08

### Fixed

- `hyperwm daemon start` no longer auto-starts the daemon at login --
  `RunAtLoad` on the daemon's launchd plist had regressed to `true`;
  starting the daemon is meant to stay an explicit `hyperwm daemon start`
- `daemon.log`/`daemon.err.log` lines are now UTC-timestamped; launchd
  redirects the daemon's stdout/stderr to these files with no timestamping
  of its own, which made correlating them with anything else much harder
- The release workflow now re-signs the `lipo`-combined universal binary,
  not just each per-arch slice before combining -- the combined binary was
  shipping unsigned, which made `hyperwm daemon start` (launched via
  launchd) fail its Accessibility/Input Monitoring permission checks even
  with a valid grant on file. The daemon has never actually worked when
  installed via Homebrew and started this way until this fix.

## [0.2.1] - 2026-08-24

### Fixed

- `hyperwm --version`/`--help` now embed the version from the pushed
  release tag at build time, instead of the workspace's checked-in
  `Cargo.toml` version (stuck at `0.1.0` since the `0.2.0` tag)
- `--help`/`-h` now works at every subcommand level (`daemon`,
  `daemon start|stop|restart`, `install-keymap`, `reload`, `status`,
  `verify`), instead of being misread as an invalid subcommand/argument
- `hyperwm install-keymap --help` no longer performs the real Caps Lock
  remap; previously it ignored its arguments entirely and always ran
  the `hidutil` remap and installed the login LaunchAgent, even when
  passed `--help`

### Added

- `.github/workflows/release.yml`: a job that updates and pushes
  `PurpleByte24/homebrew-hyperwm`'s `Formula/hyperwm.rb` (`url`,
  `sha256`) automatically after each release, instead of by hand

## [0.2.0] - 2026-08-24

### Added

- `hyperwm daemon start|stop|restart` — launchd-managed daemon lifecycle
- `hyperwm install-keymap` — automates the Caps Lock → hyperkey remap,
  persisted across reboots via a login item
- Full documentation set: configuration, keybinds, scripts, installation

## [0.1.0] - 2026-08-24

### Added

- Initial release: BSP tiling, hyperkey-driven window management,
  per-app float rules, configurable gaps, user scripts, Space-aware
  tiling, CLI (`reload`, `status`, `verify`)
