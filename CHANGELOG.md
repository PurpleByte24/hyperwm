# Changelog

All notable changes to this project are documented here.
Format based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [0.2.1] - unreleased

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
