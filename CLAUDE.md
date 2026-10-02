# Bugdom Bevy port: agent guidelines

Rust + Bevy port of Bugdom, written as idiomatic ECS. Plan and progress are in
`docs/PLAN.md`; read it only when your task needs that context.

## Repository
- `original/`: upstream C source and game data (git submodule). **Read-only.**
  Use it as the behavioural reference; never edit, format or commit inside it.
  If it is empty, run `git submodule update --init --recursive`.
- `crates/bugdom_formats`: pure-Rust parsers for the original file formats. No Bevy dependency.
- `crates/bugdom_convert`: CLI that converts original data to open formats.
- `crates/bugdom`: the game.

## Before you finish
Run all of these and make sure they pass: `cargo fmt --all`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test -p <crate>` for each crate you changed. Testing `bugdom` builds
all of Bevy, which is slow the first time in a session; `bugdom_formats`
tests do not need Bevy.

## Bevy
- The Bevy version is pinned in the workspace `Cargo.toml` (0.19). Your
  training data likely covers older Bevy APIs, so check the locked version
  (docs.rs or `cargo doc`) rather than writing from memory.
- One plugin per feature, e.g. `AntPlugin` or `TerrainPlugin`. Keep systems
  small and give each one a single job.
- Gameplay runs in `FixedUpdate`. Use per-second units; never scale by frame
  rate the way the C code does with `gFramesPerSecondFrac`.
- Use `avian3d` for colliders and spatial queries. The player and enemy
  movement uses our own integration and collision resolution, not avian's
  dynamic bodies.

## Gameplay code
- Prefer generic, reusable components (`Velocity`, `Health`, `Hurtbox`,
  `RidingPlatform(Entity)`, …) over kind-specific fields. Check the existing
  components before adding new ones.
- Kind-specific state goes in typed components. Never copy the C scratch slots
  (`SpecialL`/`SpecialF`/`Flag`).
- Tunable numbers (speeds, health, timings) go in data or named constants
  with units, not bare magic numbers.
- Every ported system names its source in a doc comment, e.g.
  `/// Port of \`MoveAnt_Walking\` (original/src/Enemies/Enemy_Ant.c).`
  Keep the original behaviour; if you change it on purpose, say so in the comment.

## Style
- Rust 2024 edition, rustfmt defaults. In non-test code, avoid `unwrap()` and
  `expect()` outside startup and asset-loading code that reports its errors.
- Write comments that explain *why*, not *what*.
- Parsers are tested against the real files in `original/Data`.

## Ask first (report back instead of deciding yourself)
- Adding a dependency
- Changing a shared component, the crate layout, or `docs/PLAN.md` (apart from ticking checkboxes)
- Any intentional difference from the original's gameplay
