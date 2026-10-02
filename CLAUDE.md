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

## Checking rendering
Cloud sessions can render headlessly. `cargo run -p bugdom --bin viewer` with
`BUGDOM_VIEWER_CAPTURE=skeleton:Ant:0:/path/shot.png` (or `model:<file>:<object>:...`)
saves a screenshot and exits; `BUGDOM_VIEWER_CAPTURE_TICK=<tick>` freezes the
animation at that tick first. Run it under
`xvfb-run -a -s "-screen 0 1280x720x24"` with
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`. Use `cargo run`, not
the binary directly: development builds link Bevy dynamically.
The game itself (`cargo run -p bugdom`) takes `BUGDOM_CAPTURE=/path/shot.png`
the same way, plus `BUGDOM_LEVEL=<0-9>` to pick the level and
`BUGDOM_CAMERA=x,y,z,yaw,pitch` to place the debug camera (world units and
radians).

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
- Keep everything about a player on its entity, so the game can have
  several players later: state, input and preferences are components, not
  resources, and systems loop over players instead of using `single()`.
  Resources are for things shared by everyone (tuning constants, level data).
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
