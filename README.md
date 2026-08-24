# hyperwm

A keyboard-driven tiling window manager for macOS.

Caps Lock becomes a dedicated hyperkey. Windows tile automatically in a
BSP layout up to a configurable count, then overflow to floating. Per-app
float rules, configurable gaps, and user-defined scripts bindable to
hyper-prefixed shortcuts.

macOS only. No SIP shenanigans — built on the public Accessibility API and
`CGEventTap`.

## Status

Early development. Not yet ready for general use.

## Install

```sh
brew tap PurpleByte24/hyperwm
brew install hyperwm
```

See [docs/installation.md](docs/installation.md) for full setup, including
the Caps Lock remap step and permission grants.

## Configuration

See [examples/config.toml](examples/config.toml) for a fully annotated
example, and [docs/configuration.md](docs/configuration.md) for the full
reference.

## Documentation

- [docs/architecture.md](docs/architecture.md) — design and behavior spec
- [docs/configuration.md](docs/configuration.md) — config reference
- [docs/keybinds.md](docs/keybinds.md) — keybind reference
- [docs/scripts.md](docs/scripts.md) — user scripts
- [docs/installation.md](docs/installation.md) — setup, including hyperkey
  remap

## License

MIT — see [LICENSE](LICENSE).
