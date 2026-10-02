# Phase 3 design: content

Status: **approved** (2026-10-02). Items marked **[review]** change the
shared component vocabulary or differ from the original.

Milestone: every level is playable from start to finish.

## 1. How the work is split

Phase 3 is mostly independent ports (17 enemy files, about 25 item kinds, a
dozen effects), but they all lean on a few shared pieces that do not exist
yet. Built in parallel, each worker would invent its own version of them. So
the phase runs in three waves:

1. **Foundations (main session, sequential).** The shared vocabulary in §2:
   player damage, enemy base, splines, particles, level skeletons. Each lands
   as its own commit with tests and a screenshot check where it renders.
2. **Fan-out (parallel subagents in git worktrees).** One work package per
   enemy, item group or effect (§4). Each package touches only its own new
   module plus one registration line, so merges are trivial.
3. **Integration (main session).** Merge, run the full checks, play each
   level through with the debug camera and screenshots, fix the gaps.

## 2. Shared vocabulary **[review]**

### 2.1 Player damage (`player/health.rs`)

Nothing hurts the player yet, and almost every enemy and trap needs it.

| Rust | Original |
|---|---|
| `Health(f32)` component (0..=1), on the player | `gMyHealth` |
| `InvincibleTimer(Timer)` component | `InvincibleTimer` (3.0 s, `INVINCIBILITY_DURATION`) |
| `ShieldTimer` component | `gShieldTimer` |
| `HurtPlayer { player, source, damage, knock, invincible_for, override_shield }` message | `PlayerGotHurt` + `LoseHealth` |
| `BugState::KnockedOnButt` | `KnockPlayerBugOnButt` |
| `Torched` component | `gTorchPlayer` |

`Health` is generic and is also used by enemies (§2.2). Running out of
health uses the existing `Dying` path. Lives and the infobar stay in Phase 4.

The player's object collision (`collide_with_objects`) gains the missing
`DoPlayerCollisionDetect` branches: `HurtMe`, `HurtNoKnock`,
`DrainBallTime`, `Spiked`, enemy contact and bopping, each sending a message
rather than calling into enemy code.

### 2.2 Enemy base (`enemies/mod.rs`, `EnemyPlugin`)

Port of original/src/Enemies/Enemy.c.

- `Enemy { kind: EnemyKind }`, where `EnemyKind` follows `ENEMY_KIND_*`.
- `Health(f32)` and `Damage(f32)` (the damage it deals on contact).
- `EnemyCounts` resource with `MAX_ENEMIES = 11` and
  `can_spawn(kind, max_of_kind)`, the guard every Add routine starts with.
  An `On<Remove, Enemy>` observer decrements it (`DeleteEnemy`), skipped for
  enemies still on a spline and, for the total, for fireflies.
- `spawn_enemy_skeleton(...)` = `MakeEnemySkeleton`: places the skeleton on
  the floor and adds `Velocity`, `PreviousPosition`, `GroundContact`,
  `CollisionCandidates`, `CollisionKind::Enemy | BlockCamera`, a shadow,
  `DespawnOutOfRange` and `TerrainItemSource`.
- `enemy_collision(...)` = `DoEnemyCollisionDetect`: boxes, liquid →
  `Underwater`, `HurtEnemy` hits, particle hits, fences, floor and ceiling.
  `ENEMY_GRAVITY = 2000` and `apply_friction` (`ApplyFrictionToDeltas`)
  live here too.
- Messages, which each enemy plugin answers for its own kind:
  - `HurtEnemy { enemy, damage }` (`EnemyGotHurt`); reaching zero health
    sends `EnemyKilled { enemy, knock }` (`KillEnemy`'s switch becomes each
    plugin's handler).
  - `EnemyKicked { enemy, direction, damage }` (`DoBugKick`'s switch).
  - `BallHitEnemy { enemy, ball_velocity }` (`BallHitX`).
  - `EnemyBopped { enemy }` (`PlayerBopEnemy`).
- `BossRef` resources for the queen bee and king ant (`gTheQueen`,
  `gAntKingObj`), for their boss logic now and the infobar later.

The kick itself (`DoBugKick`, `AimAtClosestKickableObject`) is added to the
bug controller here, since its only targets are enemies and kickable items.

### 2.3 Splines (`splines.rs`)

Port of original/src/Terrain/SplineItems.c. `bugdom_formats` already parses
splines and spline items; the game does not use them yet.

- `Splines` resource: the baked point lists per spline.
- `RegisterSplineItemKind`, the equivalent of `gSplineItemPrimeRoutines`,
  shaped like `RegisterItemKind`. Spline items are primed at level start.
- `OnSpline { spline, placement, speed }` with `advance` (`IncreaseSplineIndex`,
  plus the zigzag variant), `position_on_spline` (`GetObjectCoordOnSpline`)
  and the visibility test (`IsSplineItemVisible`), which hides rather than
  despawns.
- `detach_from_spline` (`DetachEnemyFromSpline`): removes `OnSpline`, counts
  the enemy and hands it to its free-roaming behaviour.

### 2.4 Particles and ripples (`effects/`)

Port of the core of original/src/Items/Effects.c.

- `ParticleGroups` resource: 50 slots, up to 200 particles each, handles
  with a generation counter (`ParticleGroupId`, as the original's magic
  number).
- `ParticleGroupDesc { kind: Sparks | Gravitoids, flags, gravity, magnetism,
  base_scale, decay, fade, texture }`, with the flags `BOUNCE`, `HURT_PLAYER`,
  `HURT_PLAYER_BAD`, `HURT_ENEMY`, `ROOF`, `EXTINGUISH` and `HOT`.
- Simulation in `FixedUpdate` (`MoveParticleGroups`). Rendering is one dynamic
  mesh per group, rebuilt each frame as camera-facing quads, using
  `AlphaMode::Add`, unlit and without fog. The textures are
  `Images/Textures/130–137.tga`.
- `particle_hits(box, flags)` (`ParticleHitObject`).
- `MakeRipple` and `make_splash`.

### 2.5 Level art (`state.rs`)

`LevelAssets` loads only the player's skeleton today. A `SkeletonType` enum
(`Headers/skeletonobj.h`) and the per-level skeleton table from
`LoadLevelArt` (`System/File.c`) make every enemy's skeleton available as
`LevelAssets::skeleton(SkeletonType)`.

### 2.6 Inventory, kick, riding and level flags

- `Inventory` component on the player: keys, money, ladybugs, clovers,
  shield (`GetKey`/`UseKey`/`DoWeHaveTheKey`, `GetMoney`, `GetLadyBug`, …,
  Screens/Infobar.c). For now it is reset at every level start. Keeping it
  between levels is the Phase 4 inventory item.
- The kick (`DoBugKick`, `AimAtClosestKickableObject`) is added to the bug
  controller. It sends `EnemyKicked` (§2.2) or `ItemKicked { item }` for
  kickable items (`KickNut`, `KickKingWaterPipe`).
- `Riding(Entity)` on the player plus `BugState::Riding`, for the dragonfly
  and the water bug (`MovePlayerBug_RideDragonFly`, `RideWaterBug`,
  `gCurrentDragonFly`). The ride entity drives; the player follows it.
- `DetonatorsBlown` resource (`gDetonatorBlown`), read by the hive items and
  the bee enemies, and a `RattleHive` message from the dragonfly to the
  stump hive.
- `items::kind`: one module with all 64 item kind constants, replacing the
  private copies in `scenery.rs` and `liquids.rs`, so packages don't edit
  each other's files.

### 2.7 Smaller helpers

- `joint_position(rig, joint, offset)` (`FindCoordOnJoint`): used by the
  kick, the torch, the mosquito and the fire ant.
- `box_query(center, half, kinds)` (`DoSimpleBoxCollision`).
- `closest_enemy(point)` (`FindClosestEnemy`).
- Sound calls go through whatever playback exists. Where none exists yet,
  the call site gets a `// Sound: EFFECT_X` note, so that Phase 4 can find it.

## 3. Rules for workers

- A package adds **one new module** (for example `enemies/ant.rs`) and one line
  that registers its plugin. It does not edit shared modules. If it needs
  something shared that is missing, it stops and reports back instead of
  adding it.
- Each package follows CLAUDE.md: it cites its source on every ported system,
  keeps the original's behaviour, writes tunables as named constants with
  units, and uses typed components rather than scratch slots.
- Per-kind state lives in typed components (`AntBrain`, …). An enemy's
  mode-indexed move table (`myMoveTable[AnimNum]`) becomes a state enum,
  matched in one system, like `BugState`.
- Each package ends with `cargo fmt`, `cargo clippy` and its tests passing,
  and, where it can be seen, one screenshot of it in its level.

## 4. Work packages (wave 2)

Sizes are the C line counts. Enemies that depend on another package run
after it.

**Enemies** (one plugin each):

| Package | C file | Lines | Notes |
|---|---|---|---|
| Slug + Caterpillar | Enemy_Slug.c, Enemy_Caterpiller.c | 387 | spline only; share `SetCrawlingEnemyJointTransforms` |
| Larva | Enemy_Larva.c | 406 | bopping; QueenBee depends on it |
| Tick | Enemy_Tick.c | 163 | spawned by the kicked nut item |
| Skippy | Enemy_Skippy.c | 362 | ripples |
| PondFish | Enemy_PondFish.c | 449 | ripples, splash, eats the player |
| BoxerFly | Enemy_BoxerFly.c | 600 | |
| FlyingBee | Enemy_Bee_Flying.c | 625 | spawned by the hive too |
| Mosquito | Enemy_Mosquito.c | 735 | carries the player (joint position) |
| Roach | Enemy_Roach.c | 698 | gas particles |
| WorkerBee | Enemy_WorkerBee.c | 810 | stinger projectile |
| FireFly | Enemy_FireFly.c | 600 | carries the player; its target items |
| Ant | Enemy_Ant.c | 1397 | spear and rock throwing; the largest |
| FireAnt | Enemy_FireAnt.c | 900 | fire breath |
| Spider | Enemy_Spider.c | 957 | thread, web bullet traps the player |
| QueenBee | Enemy_QueenBee.c | 866 | boss; after Larva |
| KingAnt | Enemy_KingAnt.c | 913 | boss; staff flame and bullets |

**Items, traps, triggers, rides:** see §5.

**Effects:** the four hooks already marked in the engine (the water splash
and swim ripples, checkpoint sparks, the nitro trail, the lava torch) each
form one small package. The other effects go with the enemy or item that
emits them.

## 5. Items, traps and triggers

The item kinds come from `gTerrainItemAddRoutines` (Terrain/Terrain2.c) and
`gSplineItemPrimeRoutines` (Terrain/SplineItems.c). Done already: 4–7 and
10–13 for Lawn, 14, 27, 32, 33 (without opening), 39, 55 and 56.

| Package | Kinds | C lines | Needs |
|---|---|---|---|
| A. Remaining scenery | 4 and 6 for Forest and Night, 17 Tree, 19–24 pond plants, 34 Dock, 45 HoneyTube, 50 RockLedge, 60 Faucet, 61 WoodPost | ~650 | models and boxes only |
| B. Pickups | 2 Nut (and its contents, powerups), 1 LadyBug cage, door opening for 33 | ~950 | inventory, kick, health |
| C. Hive mechanisms | 26 HoneycombPlatform (item and spline), 28 Firecracker, 29 Detonator, 30 HiveDoor, 62 FloorSpike, shockwave | ~1050 | splines, damage, particles, `DetonatorsBlown` |
| D. Ant Hill plumbing | 40 RootSwing, 43 FireWall, 44 WaterValve, 57–58 AntPipes, 63 KingWaterPipe | ~850 | damage, particles, kick, `WaterValves` |
| E. Traps | 35 Foot (spline), 41 Thorn, 51 Stump and hive, 52 RollingBoulder | ~930 | splines, damage, FlyingBee |
| F. Dragonfly ride | Ride/DragonFly.c, the bat (Traps.c) | ~780 | riding, damage, particles |
| G. Water bug ride | Ride/WaterBug.c | ~600 | riding (after F) |

## 6. Build time in parallel work

A cold build of the `bugdom` crate (all of Bevy, codegen) takes about 30
minutes. A cold `cargo check` is much cheaper and is warmed by the session
start hook. Measured in this container:

| Command | Warm | After touching `lib.rs` |
|---|---|---|
| `cargo check -p bugdom --all-targets` | 7.5 s | — |
| `cargo test -p bugdom --no-run` | 7 s | 49 s |
| same, from a fresh worktree with the shared target directory | — | 63 s, no dependency rebuilt |

So workers **do** run check, clippy and their tests, but every worktree
builds into the main checkout's target directory
(`CARGO_TARGET_DIR=/home/user/Bugdom/target`). Dependencies are then never
rebuilt; only the workspace crates are compiled per worktree. Cargo's lock
on the target directory serialises concurrent builds, which costs some
waiting but never a rebuild. Workers do not change `RUSTFLAGS`, features or
profiles, since any of those invalidates the whole cache.

## 7. Intentional differences **[review]**

None planned. Any that come up during the ports will be listed here for
approval before they merge.
