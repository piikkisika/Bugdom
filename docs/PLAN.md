# Bugdom → Bevy porting plan

Personal, non-distributed port of Pangea Software's *Bugdom* (via Iliyan Jorio's
modern port, `original/` submodule) to Rust + Bevy, written in idiomatic ECS.
The original C code is kept only as a read-only reference and as the source of
the game data.

## Goals

1. **Faithful gameplay**: movement, collision, enemy AI, timings and level
   content should feel like the original. Behaviour is checked against the
   reference C build running side by side.
2. **Reasonably faithful look**: vertex colours, fog, environment maps and
   the overall palette should match. Pixel-exact fog blending and filtering
   are not goals.
3. **Idiomatic Bevy**: entities, components, systems, states, plugins and
   assets. No wrapper around a translated C core.
4. **Moddable**: gameplay components are generic and data-driven where it is
   reasonable to make them so. Content is loaded from open, editable formats.

## Tech stack (locked versions are recorded in `Cargo.toml`)

| Concern | Choice | Notes |
|---|---|---|
| Engine | Bevy 0.19 | |
| Simulation | Bevy `FixedUpdate` (fixed timestep) | Replaces the original's variable timestep, which scales movement by `gFramesPerSecondFrac`. Constants are converted to per-second units. |
| Frame pacing | `bevy_framepace` 0.22 | Render-side frame limiting only |
| Collision | `avian3d` 0.7 | Colliders and spatial queries |
| Character controller | Custom, on avian kinematic bodies plus shape casts | We run our own velocity integration and collision resolution, using avian only for queries, as in the original game's own collision code |

Any new third-party dependency is a review point with the project owner.

## Repository layout (target)

```
original/            git submodule → github.com/jorio/Bugdom (pinned; read-only reference + game data)
crates/
  bugdom_formats/    pure-Rust parsers: resource forks, 3DMF, skeleton, terrain, .sounds, AIFF, TGA (no Bevy)
  bugdom_convert/    CLI: original data → open formats (glTF, PNG, OGG/WAV, RON)
  bugdom/            the game (Bevy app + plugins)
assets/              converted output (generated, git-ignored at first)
docs/                this plan, architecture notes, format notes
```

## Asset strategy

- **MVP**: Bevy asset loaders read the original files directly through
  `bugdom_formats`.
- **Later (for modding)**: `bugdom_convert` writes open formats that the game
  loads natively. It runs as a one-off CLI, possibly as a `build.rs` step later.
  Because the parsers are shared, switching to this path only changes which
  loader is registered. Gameplay code is unaffected.
- Level object lists, enemy parameters and similar tables become RON or Bevy
  scene data instead of hard-coded C tables, so mods can change them.

## Architecture principles

- **Generic components first.** Examples: `Velocity`, `Gravity`, `Health`,
  `Damage`, `CollisionKind` flags, `Pickup`, `Hurtbox`, `MovingPlatform`,
  `RidingPlatform(Entity)`, `SkeletonAnimator`, `Shadow`, `Spline follower`,
  `TerrainItemSource`. Each one works on any entity.
- **Kind-specific behaviour in small plugins.** For example, `AntPlugin` adds
  an `Ant` marker, an `AntBrain` state component and its systems. Behaviour
  is built from the generic components above, never from scratch slots like
  `SpecialL[6]`.
- **Object model mapping:**

  | Original C | Bevy |
  |---|---|
  | `ObjNode` | Entity |
  | `MoveCall` | Marker component plus a system |
  | `SpecialL/F/Flag` scratch slots | Typed components |
  | `ShadowNode`, `ChainNode` | Child entities or relationships |
  | `MPlatform` | `RidingPlatform(Entity)` |
  | Global variables | Resources |
  | Each screen's blocking `while(true)` loop | `States` with `OnEnter`/`OnExit` |

- **Terrain is modernized.** The original keeps only nearby terrain as meshes
  and uses a tile-lookup texture system that forces nearest-neighbour
  filtering. We will load the whole terrain up front, split into chunk meshes
  for culling. Tiles will be packed into a texture array and sampled through
  a custom material, which allows proper filtering and mipmapping without
  seams between tiles. Terrain heights and the item list become plain data.
- **Ported systems cite their source.** Each ported system names the C file
  and function it replaces, so behaviour can be checked against the reference.

## Phases

### Phase 0: Foundations
- [x] Repository restructure: upstream as the `original/` submodule (pinned to `7d7ad99`); README and this plan
- [ ] Build the reference C game from `original/` (CMake + SDL3) for side-by-side comparison
- [x] Cargo workspace skeleton (BevyFlock-style lints, features and build profiles)
- [x] Cloud session start hook (submodule, Linux dependencies, warm build cache)
- [ ] CI (fmt, clippy, test)

### Phase 1: Data formats (parallel subagents, one per format)
- [ ] Resource-fork reader
- [ ] 3DMF parser → mesh, materials, textures
- [ ] Skeleton (`.skeleton.rsrc` + `.3dmf`) → bones, joints, animations
- [ ] Terrain (`.ter.rsrc`) → heightmap, tile map, item list, splines, fences
- [ ] Sound banks (`.sounds`), AIFF music, TGA images
- [ ] Bevy asset loaders
- **Milestone:** model and skeleton viewer (equivalent of `ModelDebug.c`)

### Phase 2: Engine core (main session, because it defines the shared component vocabulary)
- [ ] Game states and the screen flow
- [ ] Terrain rendering (chunks, texture array material), fog, camera
- [ ] Fixed-timestep player controller (avian queries), skeletal animation
- [ ] Spawning terrain items, collision categories, triggers framework
- **Milestone:** the player can walk, jump, roll and swim in the Lawn level, and it feels the same as the reference

### Phase 3: Content (parallel subagents in separate git worktrees)
- [ ] 18 enemies (`Enemies/*.c`), each as its own plugin
- [ ] Items, traps, triggers, liquids, effects/particles, dragonfly ride, spline objects
- **Milestone:** every level is playable from start to finish

### Phase 4: Screens and polish
- [ ] Infobar (on-screen HUD), title screen, menus, level intro, win/lose screen, bonus screen, high scores, settings
- [ ] Sound effects and music, save games, input remapping and gamepad support
- [ ] Converter path and modding documentation

## Review points with the project owner
- New external crates
- Changes to the component vocabulary or crate layout
- Any intentional difference from the original's gameplay
- The end of each milestone
