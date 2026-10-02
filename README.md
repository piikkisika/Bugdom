# Bugdom (Bevy port)

A personal, non-distributed port of Pangea Software's *Bugdom* to Rust and
[Bevy](https://bevyengine.org), written in idiomatic ECS rather than as a
wrapper around the original engine.

The original game, as modernized by Iliyan Jorio, is included as a git
submodule in [`original/`](https://github.com/jorio/Bugdom). It is the
behavioural reference and the source of the game data. It is never modified.

## Getting started

```sh
git clone --recurse-submodules <this repo>
# or, in an existing clone:
git submodule update --init --recursive
```

## Building

```sh
cargo run -p bugdom                                  # development build (dynamic linking, asset hot-reload)
cargo build -p bugdom --release --no-default-features  # standalone release build
```

On Linux, Bevy needs `libasound2-dev`, `libudev-dev`, `libwayland-dev` and
`libxkbcommon-dev` (Ubuntu/Debian package names). For faster builds, copy
`.cargo/config_fast_builds.toml` to `.cargo/config.toml` and follow its
comments.

## Layout

| Path | Purpose |
|---|---|
| `original/` | Upstream C source and game data (read-only submodule) |
| `crates/bugdom_formats` | Parsers for the original data formats (no Bevy dependency) |
| `crates/bugdom_convert` | Converter from the original data to open formats |
| `crates/bugdom` | The game |
| `docs/PLAN.md` | Porting plan, architecture principles and progress checklist |

## License

The game's code and assets are licensed under
[CC BY-NC-SA 4.0](LICENSE.md), and this port inherits that license. See
`original/README.md` for credits.
