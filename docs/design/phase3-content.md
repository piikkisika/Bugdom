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

**Change of method (2026-10-03):** the fan-out drained the usage budget
quickly, and workers that hit the usage limit stopped mid-package. Once the
packages already in flight are merged, the remaining packages and the
integration are done in the main session, one at a time, without
subagents or worktrees. §3 and §6 still describe how the fan-out worked.

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
minutes. The session start hook warms both the check cache and the test
binaries. Measured in this container:

| Command | Time |
|---|---|
| `cargo check -p bugdom --all-targets`, warm | 7.5 s |
| `cargo test -p bugdom --no-run`, warm | 7 s |
| the same after changing one file | 49 s |
| `cargo test --workspace` in a new, prepared worktree | 66 s, no dependency rebuilt |

Workers therefore **do** run check, clippy and their tests. Each worktree is
first prepared with `.claude/scripts/prepare-worktree.sh <worktree>`, which
links `original/` and seeds the worktree's own `target/` with hard links to
the main checkout's dependency artifacts. That takes no extra disk space.

Worktrees must **not** share one target directory through
`CARGO_TARGET_DIR`. Cargo hashes workspace crates relative to the workspace
root, so two checkouts write the same artifact files. One checkout then
treated the other's build as fresh: its tests ran with the other
worktree's paths, and could equally run its code. The script leaves the
workspace crates' artifacts out of the seeded directory, so each worktree
builds them from its own sources.

Workers do not change `RUSTFLAGS`, features or profiles, since any of those
invalidates the cache.

## 7. Intentional differences **[review]**

Approved by the owner:

1. **Flying bee killed mid-collision** (`MoveFlyingBee_Flying`): the
   original runs the rest of the frame's flying logic for a bee that a
   hurt has just killed, so a dead bee could start a dive and become
   spiked again. The port ends the dead bee's frame.
2. **Firefly error paths** (`AddFireFly`, `FindFireFlyTarget`): where the
   original stops the game, the port logs an error. A firefly outside the
   Night level doesn't spawn; with no target item on the level, it lets go
   of the player at once.
3. **Firefly with several players**: each firefly chases the nearest
   player and keeps to it; "one chaser at a time" stays global.
4. **Firefly carry climb**: above the carry height the original halves a
   fast rise once per frame; the port applies it per second at the
   original's 60 fps (`0.5^(dt·60)`).
5. **Mosquito killed while sucking** (`KillMosquito`): the original
   stands the bug up whatever its state, even a dead bug, which ends the
   death animation and gives the player control until it starts again.
   The port leaves a dying bug alone. Reverting is one condition in
   `Victims::stand_up` (enemies/mosquito.rs).
6. **Mosquito letting go**: the bug blends into standing at the bug's own
   rate (6 per second) rather than the original's 7, except from the blood
   suck itself, which already uses 7.

7. **Spider killed mid-collision** (`MoveSpider_Walk`, `MoveSpider_Jump`):
   as for the flying bee (1), a spider that a hurt kills during its
   collision ends its move. The original carries on and can switch the
   dead spider back to walking or spitting, leaving it alive in effect
   with only `CTYPE_MISC` collision.

8. **Roach killed again** (`KillRoach`): a roach already dead ignores
   further kills. The original restarts the death blend on every call, so
   a dead roach in hurting particles freezes at the start of its fall.
9. **Spline roach collision** (`MoveRoachOnSpline`): the original runs
   the collision on whatever object moved last (`gCoord`/`gDelta` are not
   set there); the port collides the roach itself, then puts it back on
   its spline.
10. **Gas cloud looks**: the clouds use the unfogged glow material, and
    the Night level's fading of distant objects (`gAutoFadeStatusBits`) is
    not ported yet.

11. **Queen bee killed mid-collision**: as for the flying bee (1), a
    hurt that kills her during her collision ends her move, so a timer
    running out in the same frame can't start a spit, a flight or standing
    up instead of dying.
12. **Queen bee with no next base**: the original reads past the end of
    its base list; the port logs a warning and she lands where she is.
    The real level's bases (0–8) never reach this.
13. **Ball or kick on the dead queen**: ignored, which is also what the
    original's collision kinds lead to.

14. **King ant killed mid-collision**: as for the flying bee (1), a hurt
    that kills him during his own collision ends that tick's move. The
    original carries on with the old state, which could set the dying
    king walking (and shooting) again with only `Misc` collision.

15. **Hive door opens in place** (`MakeOpenHiveDoor`): the original
    deletes the closed door, freeing its map item, and makes an open door
    tied to no item. The port swaps the model and boxes on the same
    entity, which keeps the item and avoids a second open door if the
    item's row is scanned again before the old door leaves range.
16. **Floor spike timing** (`MoveFloorSpike`): the spike moves before the
    player's collision rather than after it, so a standing player is hit;
    its nearness test therefore sees player positions one tick old.

17. **Full particle groups** (Ant Hill items): where the original starts a
    new group and redoes a burst for as long as groups fill up (forever,
    for a burst bigger than a group), the port redoes it once.
18. **Root swing grabs**: a bug can grab by its state (jumping or
    falling) rather than its animation number, and every player is
    checked, not one.
19. **Valve numbers above 7** count as shut, since `WaterValves` holds 8.
    The data uses 0, 2, 4, 5 and 99, and 99 never opens in the original
    either.
20. **Root swing's unused joint object**: the invisible object the
    original puts on the root's second-to-last joint has no model or
    collision, so it is not ported.
21. **Ant Hill items on the wrong level** log a warning and are skipped,
    where the original stops the game.

22. **Rolling boulder's turn** (`MoveRollingBoulder`): the original
    scales its turn by the frame time twice, so how fast the axle turns
    depends on the frame rate; the port uses its 60 fps value, as for the
    firefly (4).
23. **Rolling boulder with several players**: a waiting boulder sets off
    toward the nearest player in range.

24. **Bat with several players** (`MakeBat`): it dives on the player
    whose dragonfly called it, rather than on `gMyCoord`.
25. **Dragonfly losing its rider** without being told: it lets itself go
    (`PlayerOffDragonfly`). The original can't reach this case.
26. **Dragonflies and water bugs on the wrong level** log a warning and
    are skipped, where the original stops the game (as 21).
27. **A buddy per player** (`BuddyFollowsMe`): each player can have its
    own following buddy; the original has one.
28. **Buddy's climb steadying**: halving its vertical speed once per frame
    when level with its enemy becomes the same rate per second at 60 fps
    (as 4).
29. **Lawn and Night doors on the wrong level** log a warning and are
    skipped (as 21).
30. **Water bug's nose lift** (`DriveWaterBug`): it goes by the speed
    gained per 60 fps frame, as the original's per-frame difference does at
    that rate (as 4).

Any others that come up during the ports are listed here for approval
before they merge.
