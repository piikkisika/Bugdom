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

## Layout

| Path | Purpose |
|---|---|
| `original/` | Upstream C source and game data (read-only submodule) |
| `crates/` | Rust workspace (to be added in Phase 0) |
| `docs/PLAN.md` | Porting plan, architecture principles and progress checklist |

## License

The game's code and assets are licensed under
[CC BY-NC-SA 4.0](LICENSE.md), and this port inherits that license. See
`original/README.md` for credits.
