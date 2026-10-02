# Phase 2 design: engine core

Status: **approved** (2026-10-02), with the changes recorded below. The
terrain texture filtering in §3 is still open.

Milestone: the player can walk, jump, roll and swim in the Lawn level, and it
feels the same as the reference.

## 1. Units, axes and timestep

- **Units: keep the original's world units** (a terrain tile is 160 units,
  the bug is about 180 units tall). Every constant in the C code then ports
  unchanged, apart from the per-second conversion. Avian's
  `PhysicsLengthUnit` is set to 100 so its internal tolerances suit that
  scale. **[review]**
- **Axes: unchanged.** The original is right-handed and Y-up like Bevy,
  and its forward direction at yaw 0 is −Z, as in Bevy. Item and terrain
  coordinates map straight across.
- **Fixed timestep: 60 Hz** (`Time<Fixed>`). The original runs at a
  variable rate but was tuned and is normally played at 60 fps, so a
  frame-rate-dependent quirk (see §8) behaves as it does at 60 fps.
  **[review]**
- **Smooth rendering:** simulated entities carry avian's
  `TransformInterpolation`, which comes from the `bevy_transform_interpolation`
  crate that avian already depends on. It interpolates between fixed ticks,
  so display rates other than 60 Hz don't stutter, and we don't write our own.

## 2. States and screen flow

```text
AppState:   Boot → Loading → InGame            (Phase 4 adds Title, MainMenu,
                                                  LevelIntro, Bonus, …)
InGame sub-state (GameplayState): Playing | Paused
```

- Completing an area (`gAreaCompleted`) loads the next level; completing
  the last one returns to `Boot` until the win screen exists. Holding the
  backquote key and pressing F1 completes the area (`CheckForCheats`).
- Pausing (Escape or the gamepad's Start) stops `Time<Virtual>`, so
  `FixedUpdate` does not run. The pause menu waits for Phase 4; until then
  the pause key resumes.

- `CurrentLevel(LevelNum)` holds the resource that is `gRealLevel`. The
  loading screen reads it to pick the level's files.
- `LevelDef` is the per-level-type table from `Main.c`: terrain file, model
  files, fog, light colours and directions, ceiling, cyclorama, active range.
  For Phase 2 it is a Rust table of plain structs (`levels.rs`). Making it a
  RON asset needs `serde` and `ron`, so it waits for the modding work in
  Phase 4. **[review]**
- `Loading` uses Bevy's asset handles plus a "level assets ready" check. It
  enters `InGame` when everything has loaded. Everything spawned for a level
  carries `DespawnOnExit(AppState::InGame)`, so `CleanupLevel` comes for free.
- A debug start option (`BUGDOM_LEVEL=1` or a command-line argument) replaces
  the original's boot-time number keys until the menus exist.
- `DoDeathReset` and `KILL_DELAY` became a per-player `Dying` component
  with a timer rather than a sub-state, so that each player can die on its
  own. Lives and the game over arrive with the inventory in Phase 4.

## 3. Terrain

### Rendering (modernised, as planned)

- **Whole level up front.** One chunk entity per supertile (5×5 tiles) and
  layer (floor, and ceiling where the level has one), with Bevy frustum
  culling. The original's scrolling supertile cache goes.
- **Texture array:** one layer per 32×32 tile image, with mipmaps generated
  on the CPU and linear filtering. Tile flip and rotate flags become
  per-vertex UVs, so each tile has its own four vertices (no shared vertices
  across tiles). Within a tile, filtering is smooth. Across tile edges it is
  not: each tile samples only its own texels, so the edge between two tiles
  is a hard half-texel step. No colour bleeds in from unrelated tiles, but it
  is not seamless either.
- **Open: filtering across tile edges.** The original filters smoothly but
  shows seams. The fix is a post-import step that bakes every chunk's tiles
  into one texture with a one-tile border copied from its neighbours. That is
  what the original's `SUPERTILE_DETAIL_SEAMLESS` mode does for each
  supertile, extended so that neighbouring chunks agree. The baked chunk
  textures go into an array, one layer per chunk. Decided later; the
  per-tile array comes first, and the chunk mesh and material only change in
  how they compute UVs.
- **`TerrainMaterial`**, a custom `Material`: texture array × vertex colour,
  unlit, with Bevy's distance fog. This matches the original, which pre-lights
  terrain on the CPU: `BuildTerrainSuperTile` multiplies the map's vertex
  colours by ambient plus two fill lights. We port that lighting exactly at
  load time, including the vertex normals averaged across tiles. This is the
  most faithful option, and it is also the cheapest.
- Item shadow casting onto vertex colours (`DoItemShadowCasting`) is ported
  at load time too.

### Gameplay data (ported exactly)

- `TerrainHeights` resource: heights, split modes
  (`CalculateSplitModeMatrix`), and `height_at(x, z, layer) -> (y, normal)`,
  ported from `GetTerrainHeightAtCoord`. Gameplay never queries the render
  mesh or avian for terrain height. These functions get unit tests against
  the real files, and `bugdom_formats` already provides the split mode.

### Terrain items: keep the original's active window **[review]**

Although the whole terrain is rendered, items **still stream in and out** the
way the original does, because this changes gameplay:

- Items spawn when they enter the window around the point 500 units ahead of
  the camera (`DoMyTerrainUpdate`, `ScanForPlayfieldItems`).
- They despawn when they leave the slightly larger delete window
  (`TrackTerrainItem`).
- Enemies therefore respawn when you come back, and only nearby ones are
  active.

The window keeps the per-level-type `gSuperTileActiveRange`. The in-use flag
lives in a `TerrainItems` resource. Spawned entities carry
`TerrainItemSource(index)`, which clears that flag when they despawn.
Spawning goes through a registry from item kind to spawn function, the
equivalent of `gTerrainItemAddRoutines`. Phase 3 plugins register their own
kinds there, and later, data files can too.

Fences (`Fences.c`) are drawn as meshes at load time, and their collision is
ported as the original's segment-versus-circle test (`DoFenceCollision`).

## 4. Collision **[review]**

The original's object collision is box-against-box with **side detection
from the previous frame's boxes** (`Collision.c`). The way walls, platform
tops and triggers feel comes from that logic, so we port it rather than
substitute shape casts:

| Original | Port |
|---|---|
| `CollisionBoxes`, `OldCollisionBoxes` | `CollisionBoxes` component (offsets relative to the entity; current and previous world boxes are computed from it) |
| `CType` bits | `CollisionKind`: an avian `PhysicsLayer` enum (21 kinds fit in avian's 32-bit masks) |
| `CBits` solid sides and `TOUCHABLE` | `SolidSides` bit set |
| `HandleCollisions` | `resolve_box_collisions(…)`: the same multi-pass resolution, as a function a mover calls |
| `HandleFloorAndCeilingCollision` | the same, against `TerrainHeights` |
| `DoFenceCollision` | the same, against a `Fences` resource |

**Avian's role:** every collidable entity gets an axis-aligned
`Collider::cuboid` and `CollisionLayers`, and nothing else. There are no
rigid bodies, solver or gravity, only the collider, broad-phase and
spatial-query plugins. Movers use `SpatialQuery::shape_intersections` to find
candidates, then run the original's side logic on them. Raycasts, such as
shadows on objects and the camera's `CTYPE_BLOCKCAMERA` check, use avian too.

The two dependencies already named in the plan, `avian3d` 0.7 and
`bevy_framepace` 0.22, are added now. No other new dependencies are needed.

## 5. Shared component vocabulary **[review]**

These are generic components, each usable on any entity:

| Component | Replaces | Notes |
|---|---|---|
| `Velocity(Vec3)` | `Delta` | units per second |
| `PreviousPosition(Vec3)` | `OldCoord` | written at the start of each fixed tick; needed for side detection |
| `CollisionBoxes`, `CollisionKind`, `SolidSides` | `CollisionBoxes`, `CType`, `CBits` | see §4 |
| `GroundContact { on_ground, on_terrain, floor_normal, dist_to_floor }` | `STATUS_BIT_ONGROUND`/`ONTERRAIN`, `gRecentTerrainNormal`, `gMyDistToFloor` | |
| `RidingPlatform(Entity)` | `MPlatform` | already in the plan |
| `Underwater { surface_y }` | `STATUS_BIT_UNDERWATER`, `gPlayerCurrentWaterY` | added while the entity is in a liquid volume |
| `TerrainItemSource(u32)` | `TerrainItemPtr` | see §3 |
| `Shadow` (child entity) | `ShadowNode` | blob shadow that follows the terrain |
| `SkeletonAnimator` (exists) | `SkeletonObjDataType` | |

Player-specific state:

- `Player`: a marker plus `PlayerForm { Bug, Ball }`.
- `BallTimer`, `InvincibleTimer` and similar: typed components.
- `PlayerTuning`: the movement constants (`PLAYER_BUG_ACCEL`,
  `PLAYER_BUG_FRICTION_ACCEL`, jump force, max speeds, gravity) as one asset
  or resource with units, to make the game moddable.

**One bug state drives both movement and animation.** `MovePlayer_Bug` picks
its behaviour from the current animation number. We keep a single source of
truth too, but make it explicit: a `BugState` component (a plain enum: `Stand`,
`Walk`, `Jump`, …) is what the movement code reads and writes.

- **Movement:** one `move_bug` system matches on `BugState` and calls one small
  function per state, each citing its C original. It never touches the
  animator. It may read the animator's state (an animation has stopped, a flag
  such as the kick frame), as the original does.
- **Animation:** a separate system notices when `BugState` changes and starts
  the matching animation. It holds the animation numbers and the
  per-transition morph rates, taken from the original's `MorphToSkeletonAnim`
  calls.
- **Data:** the enum variants carry no data. Per-state data (the walk
  animation's speed, the kick-aim target) lives in its own components.

## 6. Input and camera

- **Input is sampled in `PreUpdate` and consumed in `FixedUpdate`.** Mouse
  motion is summed between ticks. Key presses (`GetNewKeyState`) are latched
  until a fixed tick reads them, so a jump pressed between ticks isn't lost.
- **Debug fly camera** for the early steps, modelled on Bevy's
  `bevy_camera_controller` free camera (`free_camera.rs`) but written as our
  own small system, so it needs no feature flag and we can change it.
- Controls, as in the original's port:
  - The mouse drives the bug, as camera-relative acceleration.
  - Keys and gamepad stand in for the mouse (`GetMouseDelta`).
  - The player-relative key mode is supported.
- **Camera:** `MoveCamera_Manual` and `UpdateCamera` are ported into
  `FixedUpdate`, because `gPlayerToCameraAngle` feeds the controls. The
  camera is interpolated for display. Projection: FOV 1.1 rad, near 20,
  far = the level's yon distance.

## 7. Look

- **Lighting matches the original's fixed-function model:**
  - Ambient 0.2 × the ambient colour, plus two directional fill lights with
    the per-level colours and directions from `Main.c`.
  - No shadow maps.
  - `Tonemapping::None`, with exposure calibrated so that Bevy's Lambert term
    equals the original's `colour × brightness × N·L`.
  - Objects are lit per pixel rather than per vertex, which is acceptable for
    a "reasonably faithful look".
- **Fog:** `DistanceFog` with linear falloff, start and end = the level's
  fractions × yon. The original's fog is plane-based and ours is radial;
  the difference is minor and allowed by the plan.
- Clear colour per level. Cyclorama, lens flare and environment maps come
  after the movement milestone, at the end of Phase 2 if time allows,
  otherwise early in Phase 3.

## 8. Intentional differences **[review]**

1. **Mouse sensitivity is fixed at its 60 fps behaviour.** In the original,
   how hard a mouse movement pushes the bug depends on the frame rate: the
   delta is scaled by the frame time and then added once per frame. Keys and
   gamepad don't have this problem. We use the 60 fps result.
2. **Other frame-rate-dependent details also take their 60 Hz values.** For
   example, the falling-speed floor `-20 × fps` in `DoFrictionAndGravity` and
   the `+= gFramesPerSecondFrac` anti-jitter offset in `HandleCollisions`
   are evaluated at a 1/60 s step.
3. **Terrain is rendered whole** rather than in the scrolling window. This
   affects visuals only, and fog and the far plane still hide the distance
   as before. The plan already includes it.

## 9. Order of work

1. Add the dependencies. Add `AppState`, the level table and level loading.
   Show the Lawn terrain with a fly camera. (screenshot check)
2. Terrain chunks, texture array material, prelighting, fog and lights.
   (screenshot check)
3. `TerrainHeights`, with tests.
4. Input, the bug controller (stand, walk, jump, fall, land), the camera and
   interpolation.
5. The collision framework, terrain item streaming, Lawn's scenery items
   (rocks, flowers, wall ends, door, log), fences and checkpoints.
6. Ball form (roll-up, unroll, ball physics), then the liquids (water,
   honey, slime and lava patches) and swimming, so that the milestone can
   check swimming.
7. The milestone checklist (jump height, run speed, camera behaviour) for the
   owner to compare against the original.
