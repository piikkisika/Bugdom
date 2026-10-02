//! The enemy base: what every enemy kind shares.
//!
//! Port of original/src/Enemies/Enemy.c and original/src/Headers/enemy.h.
//! Each enemy kind is its own plugin in `enemies/<name>.rs`, declared with
//! a `mod` line here and added in [`EnemiesPlugin::build`]'s
//! `add_plugins` (kept there, not in `lib.rs`, whose tuples are nearly
//! full). This module gives them:
//!
//! - [`Enemy`] and [`EnemyKind`]: every enemy's root entity carries
//!   `Enemy { kind }`. Its arrival and removal keep [`EnemyCounts`] up to
//!   date (`gNumEnemies`, `gNumEnemyOfKind`), so no kind counts by hand.
//! - [`EnemySpawner`] and [`EnemySkeleton`]: `MakeEnemySkeleton` plus what
//!   the Add and Prime routines set on top of it.
//! - [`EnemyCollision`] and [`EnemyBody`]: `DoEnemyCollisionDetect`, plus
//!   [`ENEMY_GRAVITY`], [`apply_friction`] and [`move_enemy`].
//! - [`HurtEnemy`] and [`EnemyKilled`]: `EnemyGotHurt` and `KillEnemy`.
//! - [`detach_enemy_from_spline`], [`closest_enemy`] and [`Bosses`].
//!
//! # Writing an enemy plugin
//!
//! **Spawning from a map item.** Register a spawn system with
//! [`RegisterItemKind::register_item_kind`](crate::items::RegisterItemKind)
//! for the kind's number in [`crate::items::kind`]. It starts with the Add
//! routine's guard and builds the skeleton:
//!
//! ```ignore
//! fn add_ant(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
//!     if !enemies.can_spawn(EnemyKind::Ant, MAX_ANTS) {
//!         return false;
//!     }
//!     let Some(ant) = enemies.spawn(
//!         EnemySkeleton::new(EnemyKind::Ant, SkeletonType::Ant, spawn.position, ANT_SCALE)
//!             .from_item(spawn.index)
//!             .foot_offset(ANT_FOOT_OFFSET)
//!             .collision_box(CollisionBox::new(ANT_HEAD_OFFSET, ANT_FOOT_OFFSET, -70.0, 70.0, 70.0, -70.0))
//!             .kickable()
//!             .solid(SolidSides::NOT_TOP)
//!             .health(1.0)
//!             .damage(ANT_DAMAGE)
//!             .shadow(8.0),
//!     ) else {
//!         return false;
//!     };
//!     enemies.commands().entity(ant).insert(AntBrain::default());
//!     true
//! }
//! ```
//!
//! Routines that skip a guard (the fire ant and roach ignore the total,
//! the flying bee checks only its own kind) read
//! [`EnemyCounts::of_kind`] and [`EnemyCounts::total`] instead.
//!
//! **Spawning on a spline.** Register a prime system with
//! [`RegisterSplineItemKind::register_spline_item_kind`](crate::splines::RegisterSplineItemKind)
//! and build with [`EnemySkeleton::on_spline`], giving the
//! [`OnSpline`] in the same spawn: an enemy on a spline is not counted
//! (`STATUS_BIT_ONSPLINE`), and the count is decided when `Enemy` is
//! added. When it leaves the spline, call [`detach_enemy_from_spline`]
//! (`DetachEnemyFromSpline`), which counts it, sets its [`HomePosition`]
//! and makes it despawn out of range.
//!
//! **Moving.** Per-kind state is a typed component (`AntBrain` and a
//! state enum matched in one system, like `BugState`). The model is the
//! child in [`EnemyModel`], which has the [`Skeleton`], its
//! `SkeletonAnimator` and `AnimationFlags`; the root has the collision
//! and a unit scale. Free-roaming enemies' move systems go in
//! [`EnemySystems::Move`]; enemies on splines move in
//! [`SplineSystems::Move`](crate::splines::SplineSystems::Move). A move
//! applies its friction ([`apply_friction`]) and gravity
//! ([`ENEMY_GRAVITY`]), moves ([`move_enemy`]) and collides
//! ([`EnemyCollision::collide`] with [`default_enemy_collision_mask`], or
//! [`death_enemy_collision_mask`] for a dying one). The collision takes
//! the kind's kill routine ([`KillRoutine`], or [`no_kill`]): a hurt it
//! runs into is applied at once, and if it takes the last of the health
//! the kill routine runs in the middle of the collision, as `KillEnemy`
//! does. When the contact says [`EnemyContact::deleted`], the move ends.
//! The same routine answers [`EnemyKilled`], for hurts other objects send. Use `Query<EnemyBody,
//! With<AntBrain>>` for the parts the collision needs; `UpdateEnemy`'s
//! speed is `velocity.length()`.
//!
//! **Answering messages.** Each plugin reads these and ignores the ones for
//! entities that aren't of its kind (filter on its brain component):
//! [`TouchedEnemy`], [`BallHitEnemy`], [`EnemyBopped`] (the player's
//! collision), [`EnemyKicked`] (the bug's kick) and [`EnemyKilled`] (its
//! health ran out: `KillEnemy`'s switch). To hurt an enemy, send
//! [`HurtEnemy`]; to hurt the player, send
//! [`HurtPlayer`](crate::player::HurtPlayer) from a system that runs
//! before the player's hurts are applied (any system in
//! [`EnemySystems::Move`] does). Despawning the root deletes the enemy
//! (`DeleteEnemy`): its shadow and model go with it and the counts drop.

use avian3d::prelude::{ColliderDisabled, CollisionLayers, LayerMask, TransformInterpolation};
use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::query::QueryData;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

pub use crate::player::{
    BallHitEnemy, EnemyBopped, EnemyKicked, ItemKicked, KICK_ENEMY_DAMAGE, KICK_SPEED, TouchedEnemy,
};

use crate::assets::skeleton::SkeletonAsset;
use crate::collision::{
    BoxCollisions, BoxMover, BoxTarget, CollisionBox, CollisionBoxes, CollisionCandidates,
    CollisionKind, SolidSides, collide_floor_and_ceiling, resolve_box_collisions, solid_object,
};
use crate::combat::{Damage, Health};
use crate::effects::{EffectsSystems, ParticleFlags, ParticleGroups, particle_hit};
use crate::fences::Fences;
use crate::items::{DespawnOutOfRange, TerrainItemSource};
use crate::liquids::{Liquid, LiquidKind, Underwater};
use crate::math::quick_distance;
use crate::objects::{ModelSpawner, attach_shadow};
use crate::physics::{GroundContact, PreviousPosition, Velocity};
use crate::player::PlayerSystems;
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonType};
use crate::splines::{OnSpline, SplineSystems, detach_from_spline};
use crate::state::{AppState, LevelAssets};
use crate::terrain::TerrainMap;

pub mod boxerfly;

pub struct EnemiesPlugin;

impl Plugin for EnemiesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EnemyCounts>()
            .init_resource::<Bosses>()
            .add_message::<HurtEnemy>()
            .add_message::<EnemyKilled>()
            .add_observer(count_new_enemy)
            .add_observer(forget_removed_enemy)
            .add_systems(
                OnEnter(AppState::InGame),
                reset_enemy_manager.before(PlayerSystems::Spawn),
            )
            // `MoveObjects`, then `MoveSplineObjects`, then
            // `MoveParticleGroups`, all after the player has moved.
            .configure_sets(
                FixedUpdate,
                (EnemySystems::Move, EnemySystems::Hurt, EnemySystems::Killed)
                    .chain()
                    .after(PlayerSystems::Move)
                    .before(EffectsSystems::MoveParticles)
                    .before(PlayerSystems::Hurt)
                    .run_if(in_state(AppState::InGame)),
            )
            .configure_sets(
                FixedUpdate,
                EnemySystems::Move.before(SplineSystems::Visibility),
            )
            .add_systems(FixedUpdate, apply_enemy_hurts.in_set(EnemySystems::Hurt));
        // The enemy kinds' plugins, one line each (at most 15 per tuple).
        app.add_plugins((boxerfly::BoxerFlyPlugin,));
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum EnemySystems {
    /// The free-roaming enemies' move systems, and their handlers for the
    /// player's contact messages and the kick. It runs after the player
    /// has moved and before the spline objects and the particles move
    /// (`MoveObjects`), so that the [`HurtPlayer`](crate::player::HurtPlayer)
    /// messages sent here are applied in the same tick.
    Move,
    /// Takes [`HurtEnemy`] damage off and sends [`EnemyKilled`].
    Hurt,
    /// The kinds' [`EnemyKilled`] handlers (`KillEnemy`'s switch).
    Killed,
}

/// The most enemies that are counted at once (`MAX_ENEMIES`). The Add
/// routines refuse to spawn more.
pub const MAX_ENEMIES: usize = 11;
/// Gravity on enemies, in units per second squared (`ENEMY_GRAVITY`).
pub const ENEMY_GRAVITY: f32 = 2000.0;
/// Damage a hurting particle does to an enemy it touches, on the enemy's
/// health scale (`DoEnemyCollisionDetect`).
pub const PARTICLE_ENEMY_DAMAGE: f32 = 0.3;
/// An empty collision box, until an Add routine sets one.
const NO_BOX: CollisionBox = CollisionBox::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
/// The original's frame rate, at which the per-frame amounts some enemies
/// pass to `ApplyFrictionToDeltas` are taken (phase 2 design §8.2).
pub const ORIGINAL_FRAME_RATE: f32 = 60.0;

/// The kinds of enemy (`ENEMY_KIND_*` in original/src/Headers/enemy.h), in
/// the original's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnemyKind {
    BoxerFly,
    Slug,
    Ant,
    FireAnt,
    PondFish,
    Mosquito,
    Spider,
    Caterpiller,
    Larva,
    FlyingBee,
    WorkerBee,
    QueenBee,
    Roach,
    Skippy,
    KingAnt,
    Tick,
    FireFly,
}

impl EnemyKind {
    pub const COUNT: usize = 17;

    /// The kind's `ENEMY_KIND_*` number.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Whether the kind counts toward [`MAX_ENEMIES`]. Fireflies don't
    /// (`AddFireFly`, `DeleteEnemy`).
    pub const fn counts_toward_total(self) -> bool {
        !matches!(self, Self::FireFly)
    }
}

/// An enemy (an `ObjNode` with a `Kind`), on its root entity. Adding and
/// removing it keeps [`EnemyCounts`] up to date, so spawn it together with
/// any [`OnSpline`] it starts on.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enemy {
    pub kind: EnemyKind,
}

/// The child entity that shows an enemy's skeleton, like the player's
/// `PlayerModel`. Its animator and animation flags are what the original
/// keeps in `theNode->Skeleton` and `theNode->Flag`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnemyModel(pub Entity);

/// Where an enemy started, or where it left its spline (`InitCoord`).
/// Many enemies wander back toward it.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct HomePosition(pub Vec3);

/// How far an object keeps from fences, in units: its bounding sphere's
/// radius (`BoundingSphere.radius`, which `DoFenceCollision` reads).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct BoundingRadius(pub f32);

/// How many enemies are counted, in total and per kind (`gNumEnemies`,
/// `gNumEnemyOfKind`). Enemies on splines aren't counted until they leave
/// them; fireflies count only for their kind.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct EnemyCounts {
    total: usize,
    of_kind: [usize; EnemyKind::COUNT],
}

impl EnemyCounts {
    /// The guard every Add routine starts with: fewer than
    /// [`MAX_ENEMIES`] in total and fewer than `max_of_kind` of this kind.
    pub fn can_spawn(&self, kind: EnemyKind, max_of_kind: usize) -> bool {
        self.total < MAX_ENEMIES && self.of_kind(kind) < max_of_kind
    }

    /// Counted enemies, fireflies excepted (`gNumEnemies`).
    pub fn total(&self) -> usize {
        self.total
    }

    /// Counted enemies of one kind (`gNumEnemyOfKind`).
    pub fn of_kind(&self, kind: EnemyKind) -> usize {
        self.of_kind[kind.index()]
    }

    fn add(&mut self, kind: EnemyKind) {
        self.of_kind[kind.index()] += 1;
        if kind.counts_toward_total() {
            self.total += 1;
        }
    }

    /// Port of the counting in `DeleteEnemy`, which stops at zero rather
    /// than going negative.
    fn remove(&mut self, kind: EnemyKind) {
        let of_kind = &mut self.of_kind[kind.index()];
        if *of_kind == 0 {
            warn!("Deleting a {kind:?} that wasn't counted");
        }
        *of_kind = of_kind.saturating_sub(1);
        if kind.counts_toward_total() {
            self.total = self.total.saturating_sub(1);
        }
    }
}

/// The bosses, while they are alive (`gTheQueen`, `gAntKingObj`). Set and
/// cleared as an enemy of their kind is added and removed; their own
/// plugins and the infobar read them.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bosses {
    pub queen_bee: Option<Entity>,
    pub king_ant: Option<Entity>,
}

impl Bosses {
    fn slot(&mut self, kind: EnemyKind) -> Option<&mut Option<Entity>> {
        match kind {
            EnemyKind::QueenBee => Some(&mut self.queen_bee),
            EnemyKind::KingAnt => Some(&mut self.king_ant),
            _ => None,
        }
    }
}

/// Hurts an enemy (`EnemyGotHurt`). [`EnemySystems::Hurt`] takes the
/// damage off its [`Health`] and, if none is left, sends [`EnemyKilled`].
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct HurtEnemy {
    pub enemy: Entity,
    /// Health to take away, on the enemy's own scale.
    pub damage: f32,
}

/// An enemy's health ran out (`KillEnemy`). Its kind's plugin answers it,
/// usually by playing a death and despawning the enemy later.
///
/// As `KillEnemy` is called for every hurt that leaves the health at or
/// below zero, an enemy can be killed more than once (for example by two
/// hurts in one tick). The handlers ignore the repeats, as the originals'
/// `Kill*` routines do.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct EnemyKilled {
    pub enemy: Entity,
    /// The velocity `KillEnemy` passes to the kinds that are sent flying
    /// (`KillMosquito(theEnemy, 0, 0, 0)`): always zero from a hurt. The
    /// kick and the ball give their own velocities through their own
    /// messages.
    pub knock: Vec3,
}

/// Counts a new enemy unless it starts on a spline, and remembers a boss.
/// Port of the counting at the end of each Add routine, and of
/// `gTheQueen`/`gAntKingObj` being set there.
fn count_new_enemy(
    add: On<Add, Enemy>,
    enemies: Query<(&Enemy, Has<OnSpline>)>,
    mut counts: ResMut<EnemyCounts>,
    mut bosses: ResMut<Bosses>,
) {
    let Ok((enemy, on_spline)) = enemies.get(add.entity) else {
        return;
    };
    if !on_spline {
        counts.add(enemy.kind);
    }
    if let Some(slot) = bosses.slot(enemy.kind) {
        *slot = Some(add.entity);
    }
}

/// Port of the counting in `DeleteEnemy`: enemies still on their spline
/// were never counted.
fn forget_removed_enemy(
    remove: On<Remove, Enemy>,
    enemies: Query<(&Enemy, Has<OnSpline>)>,
    mut counts: ResMut<EnemyCounts>,
    mut bosses: ResMut<Bosses>,
) {
    let Ok((enemy, on_spline)) = enemies.get(remove.entity) else {
        return;
    };
    if !on_spline {
        counts.remove(enemy.kind);
    }
    if let Some(slot) = bosses.slot(enemy.kind)
        && *slot == Some(remove.entity)
    {
        *slot = None;
    }
}

/// Port of `InitEnemyManager`. The gas particle group arrives with the
/// roach.
fn reset_enemy_manager(mut counts: ResMut<EnemyCounts>, mut bosses: ResMut<Bosses>) {
    *counts = EnemyCounts::default();
    *bosses = Bosses::default();
}

/// Takes an enemy off its spline, so that it moves freely and counts as
/// an enemy. Does nothing if it isn't on one (when
/// `DetachEnemyFromSpline` returns false).
///
/// Port of `DetachEnemyFromSpline` (original/src/Enemies/Enemy.c): on top
/// of [`detach_from_spline`] it counts the enemy, sets its
/// [`HomePosition`] to where it is (`InitCoord`) and makes it despawn when
/// out of range, as every free move routine calls `TrackTerrainItem`. The
/// caller switches it to its free-roaming state (the `moveCall`).
pub fn detach_enemy_from_spline(entity: &mut EntityCommands) {
    // Queued before the spline is removed, so that it sees the enemy
    // still on it, and applied with it.
    entity.queue(|mut enemy: EntityWorldMut| {
        if !enemy.contains::<OnSpline>() {
            return;
        }
        let home = enemy
            .get::<Transform>()
            .map_or(Vec3::ZERO, |t| t.translation);
        enemy.insert((HomePosition(home), DespawnOutOfRange));
        if let Some(kind) = enemy.get::<Enemy>().map(|e| e.kind) {
            enemy.resource_mut::<EnemyCounts>().add(kind);
        }
    });
    detach_from_spline(entity);
}

/// Port of `EnemyGotHurt` (original/src/Enemies/Enemy.c): the damage comes
/// off, and an enemy left with no health is killed.
fn apply_enemy_hurts(
    mut hurts: MessageReader<HurtEnemy>,
    mut enemies: Query<&mut Health, With<Enemy>>,
    mut killed: MessageWriter<EnemyKilled>,
) {
    for hurt in hurts.read() {
        let Ok(mut health) = enemies.get_mut(hurt.enemy) else {
            continue;
        };
        if health.lose(hurt.damage) {
            killed.write(EnemyKilled {
                enemy: hurt.enemy,
                knock: Vec3::ZERO,
            });
        }
    }
}

/// Where a new enemy comes from, which decides how it goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnemySource {
    /// A map item (`TerrainItemPtr`): it despawns out of range, and the
    /// item can then spawn it again.
    Item(u32),
    /// A spline item: it stays for the level, uncounted, until
    /// [`detach_enemy_from_spline`].
    Spline(OnSpline),
    /// Made by another object (a hive's bees, a nut's ticks, a ghost): it
    /// despawns out of range for good.
    Spawned,
}

/// What a new enemy skeleton is: `MakeEnemySkeleton`'s arguments and the
/// fields the Add and Prime routines set on top. [`Self::new`] has
/// `MakeEnemySkeleton`'s defaults; the builder methods set the rest.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemySkeleton {
    pub kind: EnemyKind,
    pub skeleton: SkeletonType,
    /// World x and z.
    pub position: Vec2,
    pub scale: f32,
    pub source: EnemySource,
    /// The enemy's origin is this far below the floor (`Coord.y -=
    /// FOOT_OFFSET`); the usual foot offsets are negative, which lifts it.
    pub foot_offset: f32,
    /// Its collision box, relative to its origin (`SetObjectCollisionBounds`).
    pub collision_box: CollisionBox,
    /// Its collision kinds (`CType`).
    pub kinds: LayerMask,
    /// Its solid sides (`CBits`).
    pub solid: SolidSides,
    pub health: f32,
    /// What it deals on contact (`Damage`).
    pub damage: f32,
    /// The size of its shadow (`AttachShadowToObject`'s scale), if it has
    /// one.
    pub shadow: Option<Vec2>,
    /// The animation it starts with (`animNum`).
    pub anim: usize,
    /// Its heading (`Rot.y`).
    pub yaw: f32,
}

impl EnemySkeleton {
    /// `MakeEnemySkeleton`: an enemy on the floor, solid all round, of the
    /// kinds `Enemy | BlockCamera`, from no item, with no box, health,
    /// damage or shadow yet, playing animation 0.
    pub fn new(kind: EnemyKind, skeleton: SkeletonType, position: Vec2, scale: f32) -> Self {
        Self {
            kind,
            skeleton,
            position,
            scale,
            source: EnemySource::Spawned,
            foot_offset: 0.0,
            collision_box: NO_BOX,
            kinds: LayerMask::from([CollisionKind::Enemy, CollisionKind::BlockCamera]),
            solid: SolidSides::ALL,
            health: 0.0,
            damage: 0.0,
            shadow: None,
            anim: 0,
            yaw: 0.0,
        }
    }

    /// Spawned from the map item with this index.
    pub fn from_item(mut self, index: u32) -> Self {
        self.source = EnemySource::Item(index);
        self
    }

    /// Spawned on a spline.
    pub fn on_spline(mut self, on_spline: OnSpline) -> Self {
        self.source = EnemySource::Spline(on_spline);
        self
    }

    pub fn foot_offset(mut self, foot_offset: f32) -> Self {
        self.foot_offset = foot_offset;
        self
    }

    pub fn collision_box(mut self, collision_box: CollisionBox) -> Self {
        self.collision_box = collision_box;
        self
    }

    /// Adds collision kinds (`CType |= ...`).
    pub fn with_kinds(mut self, kinds: impl Into<LayerMask>) -> Self {
        self.kinds |= kinds;
        self
    }

    /// Adds `Kickable | AutoTarget`, which most Add routines do ("these
    /// can be kicked").
    pub fn kickable(self) -> Self {
        self.with_kinds([CollisionKind::Kickable, CollisionKind::AutoTarget])
    }

    /// Sets the solid sides; most Add routines use
    /// [`SolidSides::NOT_TOP`].
    pub fn solid(mut self, solid: SolidSides) -> Self {
        self.solid = solid;
        self
    }

    pub fn health(mut self, health: f32) -> Self {
        self.health = health;
        self
    }

    pub fn damage(mut self, damage: f32) -> Self {
        self.damage = damage;
        self
    }

    /// A shadow of the same size in x and z.
    pub fn shadow(mut self, scale: f32) -> Self {
        self.shadow = Some(Vec2::splat(scale));
        self
    }

    pub fn anim(mut self, anim: usize) -> Self {
        self.anim = anim;
        self
    }

    pub fn yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }
}

/// Spawns enemies, for the kinds' spawn and prime systems.
#[derive(SystemParam)]
pub struct EnemySpawner<'w, 's> {
    commands: Commands<'w, 's>,
    counts: Res<'w, EnemyCounts>,
    level_assets: Res<'w, LevelAssets>,
    skeletons: Res<'w, Assets<SkeletonAsset>>,
    map: Res<'w, TerrainMap>,
    models: ModelSpawner<'w>,
}

impl<'w, 's> EnemySpawner<'w, 's> {
    /// See [`EnemyCounts::can_spawn`].
    pub fn can_spawn(&self, kind: EnemyKind, max_of_kind: usize) -> bool {
        self.counts.can_spawn(kind, max_of_kind)
    }

    pub fn counts(&self) -> &EnemyCounts {
        &self.counts
    }

    pub fn map(&self) -> &TerrainMap {
        &self.map
    }

    /// For adding the kind's own components to what [`Self::spawn`]
    /// returns.
    pub fn commands(&mut self) -> &mut Commands<'w, 's> {
        &mut self.commands
    }

    /// Spawns an enemy skeleton and returns its root entity, or `None` if
    /// the level doesn't load that skeleton.
    ///
    /// Port of `MakeEnemySkeleton` (original/src/Enemies/Enemy.c) and of
    /// what the Add and Prime routines set after it: the root has the
    /// collision, motion, [`Health`], [`Damage`], [`HomePosition`]
    /// (`InitCoord`) and [`BoundingRadius`]; its [`EnemyModel`] child has
    /// the skeleton, scaled.
    pub fn spawn(&mut self, def: EnemySkeleton) -> Option<Entity> {
        let Some(handle) = self.level_assets.skeleton(def.skeleton) else {
            error!("The level has no {:?} skeleton", def.skeleton);
            return None;
        };
        let radius = self.skeletons.get(&handle).map_or_else(
            || {
                warn!("The {:?} skeleton isn't loaded", def.skeleton);
                0.0
            },
            |s| s.radius,
        ) * def.scale;
        let (x, z) = (def.position.x, def.position.y);
        let position = Vec3::new(x, self.map.floor_height(x, z) - def.foot_offset, z);
        let root = self
            .commands
            .spawn((
                Name::new(format!("{:?}", def.kind)),
                Transform::from_translation(position).with_rotation(Quat::from_rotation_y(def.yaw)),
                Visibility::default(),
                TransformInterpolation,
                PreviousPosition(position),
                Velocity::default(),
                GroundContact::default(),
                CollisionCandidates::default(),
                Health(def.health),
                Damage(def.damage),
                HomePosition(position),
                BoundingRadius(radius),
                solid_object(vec![def.collision_box], def.kinds, def.solid),
                DespawnOnExit(AppState::InGame),
            ))
            .id();
        match def.source {
            EnemySource::Item(index) => {
                self.commands
                    .entity(root)
                    .insert((TerrainItemSource(index), DespawnOutOfRange));
            }
            EnemySource::Spline(on_spline) => {
                // Hidden and out of collision until the visibility check
                // finds it in the window, as Prime routines detach it.
                self.commands.entity(root).insert((
                    on_spline,
                    Visibility::Hidden,
                    ColliderDisabled,
                ));
            }
            EnemySource::Spawned => {
                self.commands.entity(root).insert(DespawnOutOfRange);
            }
        }
        // Last, so that the counting sees whether it is on a spline.
        self.commands.entity(root).insert(Enemy { kind: def.kind });
        let mut animator = SkeletonAnimator::default();
        animator.set_anim(def.anim);
        let model = self
            .commands
            .spawn((
                Name::new("Enemy model"),
                Skeleton(handle),
                animator,
                Transform::from_scale(Vec3::splat(def.scale)),
                TransformInterpolation,
                ChildOf(root),
            ))
            .id();
        self.commands.entity(root).insert(EnemyModel(model));
        if let Some(scale) = def.shadow {
            attach_shadow(&mut self.commands, &mut self.models, root, scale, false);
        }
        Some(root)
    }
}

/// What enemies bump into while alive (`DEFAULT_ENEMY_COLLISION_CTYPES`).
pub fn default_enemy_collision_mask() -> LayerMask {
    LayerMask::from([
        CollisionKind::Misc,
        CollisionKind::HurtEnemy,
        CollisionKind::Enemy,
        CollisionKind::Liquid,
    ])
}

/// What dying enemies bump into (`DEATH_ENEMY_COLLISION_CTYPES`).
pub fn death_enemy_collision_mask() -> LayerMask {
    LayerMask::from([CollisionKind::Misc, CollisionKind::Enemy])
}

/// Slows each horizontal axis toward zero by `friction` (in units per
/// second squared) for `dt` seconds, without changing its sign.
///
/// Port of `ApplyFrictionToDeltas` (original/src/System/Misc.c). The
/// enemies pass it a per-frame amount (e.g. 140 for a knocked ant), so
/// their slowing depends on the frame rate; give those as
/// [`per_frame_friction`] to get the original's 60 fps behaviour.
pub fn apply_friction(velocity: &mut Vec3, friction: f32, dt: f32) {
    let amount = friction * dt;
    for v in [&mut velocity.x, &mut velocity.z] {
        if *v < 0.0 {
            *v = (*v + amount).min(0.0);
        } else if *v > 0.0 {
            *v = (*v - amount).max(0.0);
        }
    }
}

/// A friction the original applies once per frame, unscaled, as units per
/// second squared at [`ORIGINAL_FRAME_RATE`].
pub const fn per_frame_friction(per_frame: f32) -> f32 {
    per_frame * ORIGINAL_FRAME_RATE
}

/// Moves by the velocity for `dt` seconds. Port of `MoveEnemy`.
pub fn move_enemy(coord: &mut Vec3, velocity: Vec3, dt: f32) {
    *coord += velocity * dt;
}

/// An enemy's parts that its collision reads and writes.
#[derive(QueryData)]
#[query_data(mutable)]
pub struct EnemyBody {
    pub entity: Entity,
    pub transform: &'static mut Transform,
    pub velocity: &'static mut Velocity,
    pub previous: &'static PreviousPosition,
    pub boxes: &'static CollisionBoxes,
    pub candidates: &'static CollisionCandidates,
    pub ground: &'static mut GroundContact,
    pub underwater: Option<&'static Underwater>,
    pub radius: &'static BoundingRadius,
    pub health: &'static mut Health,
}

/// What the world around an enemy is, for [`collide_enemy`].
pub struct EnemySurroundings<'a> {
    pub map: &'a TerrainMap,
    pub fences: Option<&'a Fences>,
    pub particles: Option<&'a ParticleGroups>,
    /// The solid objects near the enemy.
    pub candidates: &'a [BoxTarget],
    /// The damage each candidate deals ([`Damage`], or 0).
    pub candidate_damage: &'a [f32],
    /// The liquid each candidate is, if it is one.
    pub candidate_liquids: &'a [Option<LiquidKind>],
    pub dt: f32,
}

/// A moving enemy, for [`collide_enemy`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyMover {
    pub entity: Entity,
    /// Its collision box, relative to its origin.
    pub shape: CollisionBox,
    /// Where it was at the start of the tick (`OldCoord`).
    pub old_coord: Vec3,
    /// Its fence radius ([`BoundingRadius`]).
    pub radius: f32,
}

/// What [`collide_enemy`] found.
#[derive(Debug, Clone, Default)]
pub struct EnemyContact {
    /// The box collisions, whose hits index the candidates.
    pub boxes: BoxCollisions,
    pub ground: GroundContact,
    /// In a liquid's volume (`STATUS_BIT_UNDERWATER`).
    pub underwater: Option<Underwater>,
    /// The damage of each hurt the enemy took, in order: one per box of a
    /// `HurtEnemy` object it touched, then one for touching a hurting
    /// particle. They have been taken off its health already.
    pub hurts: Vec<f32>,
    /// A hurt left the enemy without health, and its kill routine ran.
    pub killed: bool,
    /// The kill routine deleted the enemy, so the collision stopped there
    /// (`DoEnemyCollisionDetect` returning true).
    pub deleted: bool,
}

/// An enemy kind's kill routine (`KillEnemy`'s switch), called in the
/// middle of the collision when a hurt leaves the enemy without health. It
/// gets the enemy's position and velocity as they are at that point
/// (`gCoord`, `gDelta`) and returns whether it deleted the enemy. Kinds the
/// original's switch has no case for pass [`no_kill`].
pub type KillRoutine<'a> = &'a mut dyn FnMut(&mut Vec3, &mut Vec3) -> bool;

/// The kill routine of kinds that `KillEnemy` ignores.
pub fn no_kill(_: &mut Vec3, _: &mut Vec3) -> bool {
    false
}

/// Collides a moving enemy with objects, particles, fences, the floor and
/// the ceiling, moving `coord` and `velocity` out of what it hit.
///
/// Port of `DoEnemyCollisionDetect` (original/src/Enemies/Enemy.c). A hurt
/// is applied where it happens (`EnemyGotHurt`): the damage comes off
/// `health` and, if none is left, `kill` runs at once. If it deleted the
/// enemy, the collision stops there, as in the original.
///
/// `ground` is the contact the enemy had before; like the original, only
/// `on_ground` starts over (`on_terrain` stays set once set).
pub fn collide_enemy(
    world: &EnemySurroundings,
    mover: &EnemyMover,
    coord: &mut Vec3,
    velocity: &mut Vec3,
    health: &mut Health,
    ground: GroundContact,
    mask: LayerMask,
    kill: KillRoutine,
) -> EnemyContact {
    let box_mover = BoxMover {
        entity: mover.entity,
        is_player: false,
        shape: mover.shape,
        old_coord: mover.old_coord,
        platform_velocity: Vec3::ZERO,
    };
    let boxes = resolve_box_collisions(
        &box_mover,
        coord,
        velocity,
        mask,
        world.candidates,
        world.dt,
        &mut EntityHashSet::default(),
    );
    let mut contact = EnemyContact {
        ground: GroundContact {
            on_ground: boxes.on_ground,
            ..ground
        },
        boxes,
        ..default()
    };
    // Port of `EnemyGotHurt`: true if the hurt deleted the enemy.
    let mut hurt = |contact: &mut EnemyContact, coord: &mut Vec3, velocity: &mut Vec3, damage| {
        contact.hurts.push(damage);
        if health.lose(damage) {
            contact.killed = true;
            contact.deleted = kill(coord, velocity);
        }
        contact.deleted
    };

    for hit in contact.boxes.hits.clone() {
        let Some(target) = world.candidates.get(hit.target) else {
            continue;
        };
        if target.kinds.has_all(CollisionKind::HurtEnemy) {
            let damage = world.candidate_damage.get(hit.target).copied();
            if hurt(&mut contact, coord, velocity, damage.unwrap_or(0.0)) {
                return contact;
            }
        }
        if target.kinds.has_all(CollisionKind::Liquid)
            && let Some(volume) = target.boxes.first()
        {
            let liquid = world.candidate_liquids.get(hit.target).copied().flatten();
            contact.underwater = Some(Underwater {
                volume_top: volume.top,
                liquid: liquid.unwrap_or(LiquidKind::Water),
            });
        }
    }

    // The object's boxes are still where the last tick left them
    // (`UpdateObject` recalculates them after the move).
    if let Some(particles) = world.particles
        && particle_hit(
            particles,
            &[mover.shape],
            mover.old_coord,
            ParticleFlags::HURT_ENEMY,
        )
        && hurt(&mut contact, coord, velocity, PARTICLE_ENEMY_DAMAGE)
    {
        return contact;
    }

    if let Some(fences) = world.fences {
        fences.collide(
            world.map,
            mover.old_coord.xz(),
            coord,
            velocity,
            mover.radius,
            coord.y + mover.shape.bottom,
        );
    }

    // The original passes the same delta as the new and the old one.
    let speed = velocity.length();
    let old_velocity = *velocity;
    let floor = collide_floor_and_ceiling(
        world.map,
        coord,
        mover.old_coord,
        velocity,
        old_velocity,
        -mover.shape.bottom,
        mover.shape.top,
        speed,
        world.dt,
    );
    if floor.on_ground {
        contact.ground.on_ground = true;
        contact.ground.on_terrain = true;
    }
    contact.ground.floor_normal = floor.floor_normal;
    contact.ground.dist_to_floor =
        coord.y + mover.shape.bottom - world.map.floor_height(coord.x, coord.z);
    contact
}

/// The enemy collision as a system parameter: [`collide_enemy`] for an
/// [`EnemyBody`], with the liquid stored as [`Underwater`].
///
/// It reads every [`Damage`] and [`Liquid`], so a system using it can't
/// also write those.
#[derive(SystemParam)]
pub struct EnemyCollision<'w, 's> {
    time: Res<'w, Time>,
    map: Res<'w, TerrainMap>,
    fences: Option<Res<'w, Fences>>,
    particles: Option<Res<'w, ParticleGroups>>,
    liquids: Query<'w, 's, &'static Liquid>,
    damages: Query<'w, 's, &'static Damage>,
    commands: Commands<'w, 's>,
}

impl EnemyCollision<'_, '_> {
    /// The tick's length, in seconds.
    pub fn dt(&self) -> f32 {
        self.time.delta_secs()
    }

    /// Collides the enemy where its move left it, with the kinds in `mask`
    /// ([`default_enemy_collision_mask`] or
    /// [`death_enemy_collision_mask`]), running `kill` if a hurt takes the
    /// last of its health. Port of `DoEnemyCollisionDetect`.
    ///
    /// If [`EnemyContact::deleted`] is set, the move should end there, as
    /// the original's move functions return when it returns true.
    pub fn collide(
        &mut self,
        body: &mut EnemyBodyItem,
        mask: LayerMask,
        kill: KillRoutine,
    ) -> EnemyContact {
        let candidates = &body.candidates.0;
        let candidate_damage: Vec<f32> = candidates
            .iter()
            .map(|t| self.damages.get(t.entity).map_or(0.0, |d| d.0))
            .collect();
        let candidate_liquids: Vec<Option<LiquidKind>> = candidates
            .iter()
            .map(|t| self.liquids.get(t.entity).ok().map(|l| l.0))
            .collect();
        let world = EnemySurroundings {
            map: &self.map,
            fences: self.fences.as_deref(),
            particles: self.particles.as_deref(),
            candidates,
            candidate_damage: &candidate_damage,
            candidate_liquids: &candidate_liquids,
            dt: self.time.delta_secs(),
        };
        let mover = EnemyMover {
            entity: body.entity,
            shape: body.boxes.0.first().copied().unwrap_or(NO_BOX),
            old_coord: **body.previous,
            radius: **body.radius,
        };
        let mut coord = body.transform.translation;
        let mut velocity = **body.velocity;
        let contact = collide_enemy(
            &world,
            &mover,
            &mut coord,
            &mut velocity,
            &mut body.health,
            *body.ground,
            mask,
            kill,
        );
        if contact.deleted {
            return contact;
        }

        body.transform.translation = coord;
        **body.velocity = velocity;
        *body.ground = contact.ground;
        if body.underwater.copied() != contact.underwater {
            let mut entity = self.commands.entity(body.entity);
            match contact.underwater {
                Some(underwater) => entity.insert(underwater),
                None => entity.remove::<Underwater>(),
            };
        }
        contact
    }
}

/// The enemy nearest to `point` in x and z (by `CalcQuickDistance`) among
/// `candidates` (entity, position, collision kinds), and its distance.
/// Only objects whose kinds include [`CollisionKind::Enemy`] count, so a
/// dying enemy, which the original turns into `CTYPE_MISC`, doesn't.
///
/// Port of `FindClosestEnemy` (original/src/Enemies/Enemy.c), which
/// returns no enemy and a distance of 10,000,000 when there is none.
pub fn closest_enemy(
    point: Vec3,
    candidates: impl IntoIterator<Item = (Entity, Vec3, LayerMask)>,
) -> Option<(Entity, f32)> {
    let mut best = None;
    let mut min_dist = 10_000_000.0;
    for (entity, position, kinds) in candidates {
        if !kinds.has_all(CollisionKind::Enemy) {
            continue;
        }
        let d = quick_distance(point.xz(), position.xz());
        if d < min_dist {
            min_dist = d;
            best = Some((entity, d));
        }
    }
    best
}

/// The enemies [`closest_enemy`] looks through: those in the object list,
/// which leaves out the ones on splines that are out of the window.
#[derive(SystemParam)]
pub struct EnemyFinder<'w, 's> {
    enemies: Query<
        'w,
        's,
        (
            Entity,
            &'static Transform,
            &'static CollisionLayers,
            Option<&'static OnSpline>,
        ),
        With<Enemy>,
    >,
}

impl EnemyFinder<'_, '_> {
    /// See [`closest_enemy`].
    pub fn closest(&self, point: Vec3) -> Option<(Entity, f32)> {
        closest_enemy(
            point,
            self.enemies
                .iter()
                .filter(|(.., on_spline)| on_spline.is_none_or(|s| s.visible))
                .map(|(entity, transform, layers, _)| {
                    (entity, transform.translation, layers.memberships)
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn world_with_enemy_base() -> World {
        let mut world = World::new();
        world.init_resource::<EnemyCounts>();
        world.init_resource::<Bosses>();
        world.add_observer(count_new_enemy);
        world.add_observer(forget_removed_enemy);
        world
    }

    #[test]
    fn enemies_are_counted_while_they_exist() {
        let mut world = world_with_enemy_base();
        let ant = world
            .spawn((Enemy {
                kind: EnemyKind::Ant,
            },))
            .id();
        world.spawn(Enemy {
            kind: EnemyKind::Ant,
        });
        let firefly = world
            .spawn(Enemy {
                kind: EnemyKind::FireFly,
            })
            .id();
        let counts = world.resource::<EnemyCounts>();
        assert_eq!(counts.total(), 2);
        assert_eq!(counts.of_kind(EnemyKind::Ant), 2);
        // Fireflies only count for their kind.
        assert_eq!(counts.of_kind(EnemyKind::FireFly), 1);

        world.despawn(ant);
        world.despawn(firefly);
        let counts = world.resource::<EnemyCounts>();
        assert_eq!(counts.total(), 1);
        assert_eq!(counts.of_kind(EnemyKind::Ant), 1);
        assert_eq!(counts.of_kind(EnemyKind::FireFly), 0);
    }

    #[test]
    fn the_add_guard_stops_at_both_limits() {
        let mut counts = EnemyCounts::default();
        assert!(counts.can_spawn(EnemyKind::Ant, 2));
        counts.add(EnemyKind::Ant);
        counts.add(EnemyKind::Ant);
        assert!(!counts.can_spawn(EnemyKind::Ant, 2));
        assert!(counts.can_spawn(EnemyKind::Spider, 2));
        for _ in 2..MAX_ENEMIES {
            counts.add(EnemyKind::BoxerFly);
        }
        assert_eq!(counts.total(), MAX_ENEMIES);
        assert!(!counts.can_spawn(EnemyKind::Spider, 20));
        // Fireflies don't fill the total.
        counts.remove(EnemyKind::BoxerFly);
        counts.add(EnemyKind::FireFly);
        assert!(counts.can_spawn(EnemyKind::Spider, 20));
        // Removing more than were counted stops at zero.
        let mut counts = EnemyCounts::default();
        counts.remove(EnemyKind::Tick);
        assert_eq!(counts, EnemyCounts::default());
    }

    #[test]
    fn spline_enemies_count_only_once_detached() {
        let mut world = world_with_enemy_base();
        let on_spline = OnSpline::new(0, 0.5, 10.0);
        let slug = world
            .spawn((
                Enemy {
                    kind: EnemyKind::Ant,
                },
                Transform::from_xyz(1.0, 2.0, 3.0),
                on_spline,
            ))
            .id();
        let stays = world
            .spawn((
                Enemy {
                    kind: EnemyKind::Ant,
                },
                on_spline,
            ))
            .id();
        assert_eq!(world.resource::<EnemyCounts>().total(), 0);

        world
            .run_system_once(move |mut commands: Commands| {
                detach_enemy_from_spline(&mut commands.entity(slug));
                // A second detach does nothing (`DetachEnemyFromSpline`
                // returning false).
                detach_enemy_from_spline(&mut commands.entity(slug));
            })
            .expect("the system runs");
        assert_eq!(world.resource::<EnemyCounts>().of_kind(EnemyKind::Ant), 1);
        let detached = world.entity(slug);
        assert!(!detached.contains::<OnSpline>());
        assert!(detached.contains::<DespawnOutOfRange>());
        assert_eq!(
            detached.get::<HomePosition>(),
            Some(&HomePosition(Vec3::new(1.0, 2.0, 3.0)))
        );

        // Deleting an enemy still on its spline doesn't uncount anything.
        world.despawn(stays);
        assert_eq!(world.resource::<EnemyCounts>().total(), 1);
        world.despawn(slug);
        assert_eq!(world.resource::<EnemyCounts>().total(), 0);
    }

    #[test]
    fn bosses_are_remembered_while_they_live() {
        let mut world = world_with_enemy_base();
        let queen = world
            .spawn(Enemy {
                kind: EnemyKind::QueenBee,
            })
            .id();
        assert_eq!(world.resource::<Bosses>().queen_bee, Some(queen));
        assert_eq!(world.resource::<Bosses>().king_ant, None);
        world.despawn(queen);
        assert_eq!(world.resource::<Bosses>().queen_bee, None);
    }

    #[test]
    fn hurts_take_health_and_kill_when_it_runs_out() {
        let mut world = world_with_enemy_base();
        world.init_resource::<Messages<HurtEnemy>>();
        world.init_resource::<Messages<EnemyKilled>>();
        let enemy = world
            .spawn((
                Enemy {
                    kind: EnemyKind::Ant,
                },
                Health(1.0),
            ))
            .id();
        let hurt = |world: &mut World, damage| {
            world.write_message(HurtEnemy { enemy, damage });
            world
                .run_system_once(apply_enemy_hurts)
                .expect("the system runs");
            // Each run has a new reader, which would read them again.
            world.resource_mut::<Messages<HurtEnemy>>().clear();
            world
                .resource_mut::<Messages<EnemyKilled>>()
                .drain()
                .collect::<Vec<_>>()
        };
        assert!(hurt(&mut world, 0.4).is_empty());
        assert_eq!(world.get::<Health>(enemy), Some(&Health(0.6)));
        let killed = EnemyKilled {
            enemy,
            knock: Vec3::ZERO,
        };
        assert_eq!(hurt(&mut world, 0.6), [killed]);
        // As `KillEnemy` is called again for each further hurt.
        assert_eq!(hurt(&mut world, 0.1), [killed]);
    }

    #[test]
    fn friction_slows_each_axis_to_zero() {
        let mut velocity = Vec3::new(100.0, -50.0, -10.0);
        apply_friction(&mut velocity, per_frame_friction(60.0), 1.0 / 60.0);
        assert!((velocity - Vec3::new(40.0, -50.0, 0.0)).length() < 1e-3);
        apply_friction(&mut velocity, per_frame_friction(60.0), 1.0 / 60.0);
        assert_eq!(velocity, Vec3::new(0.0, -50.0, 0.0));
    }

    #[test]
    fn the_closest_enemy_ignores_other_kinds() {
        let e = |i| Entity::from_raw_u32(i).expect("a valid index");
        let enemy = LayerMask::from(CollisionKind::Enemy);
        let misc = LayerMask::from(CollisionKind::Misc);
        let found = closest_enemy(
            Vec3::ZERO,
            [
                (e(1), Vec3::new(300.0, 0.0, 0.0), enemy),
                (e(2), Vec3::new(0.0, 900.0, -100.0), enemy),
                (e(3), Vec3::new(10.0, 0.0, 0.0), misc),
            ],
        );
        // Height doesn't matter.
        assert_eq!(found, Some((e(2), 100.0)));
        assert_eq!(closest_enemy(Vec3::ZERO, []), None);
    }

    /// A flat-floored test world: the Lawn's start, with the given objects
    /// around the enemy.
    fn collide(
        candidates: &[BoxTarget],
        damage: &[f32],
        liquids: &[Option<LiquidKind>],
        coord: &mut Vec3,
        velocity: &mut Vec3,
        old_coord: Vec3,
    ) -> EnemyContact {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let world = EnemySurroundings {
            map: &map,
            fences: None,
            particles: None,
            candidates,
            candidate_damage: damage,
            candidate_liquids: liquids,
            dt: 1.0 / 60.0,
        };
        let mover = EnemyMover {
            entity: Entity::PLACEHOLDER,
            shape: CollisionBox::new(70.0, 0.0, -40.0, 40.0, 40.0, -40.0),
            old_coord,
            radius: 50.0,
        };
        collide_enemy(
            &world,
            &mover,
            coord,
            velocity,
            &mut Health(1.0),
            GroundContact::default(),
            default_enemy_collision_mask(),
            &mut no_kill,
        )
    }

    const SPOT: Vec2 = Vec2::new(12720.0, 15780.0);

    fn target(id: u32, kinds: LayerMask, solid: SolidSides, b: CollisionBox) -> BoxTarget {
        BoxTarget {
            entity: Entity::from_raw_u32(id).expect("a valid index"),
            kinds,
            solid,
            boxes: vec![b],
            old_boxes: vec![b],
            velocity: Vec3::ZERO,
            trigger: None,
        }
    }

    #[test]
    fn a_falling_enemy_lands_on_the_floor() {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let floor = map.floor_height(SPOT.x, SPOT.y);
        let old = Vec3::new(SPOT.x, floor + 5.0, SPOT.y);
        let mut coord = old - Vec3::Y * 20.0;
        let mut velocity = Vec3::new(0.0, -1200.0, 0.0);
        let contact = collide(&[], &[], &[], &mut coord, &mut velocity, old);
        assert!(contact.ground.on_ground && contact.ground.on_terrain);
        assert!((coord.y - floor).abs() < 0.01, "{} vs {floor}", coord.y);
        assert!(contact.hurts.is_empty());
        assert_eq!(contact.underwater, None);
    }

    #[test]
    fn hurting_objects_and_liquids_are_reported() {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let floor = map.floor_height(SPOT.x, SPOT.y);
        let here = Vec3::new(SPOT.x, floor, SPOT.y);
        let around = CollisionBox::new(floor + 500.0, floor - 500.0, -100.0, 100.0, 100.0, -100.0)
            .at(Vec3::new(SPOT.x, 0.0, SPOT.y));
        let candidates = [
            target(
                1,
                LayerMask::from(CollisionKind::HurtEnemy),
                SolidSides::TOUCHABLE,
                around,
            ),
            target(
                2,
                LayerMask::from(CollisionKind::Liquid),
                SolidSides::TOUCHABLE,
                around,
            ),
            // Not in the mask: ignored.
            target(
                3,
                LayerMask::from(CollisionKind::HurtMe),
                SolidSides::TOUCHABLE,
                around,
            ),
        ];
        let mut coord = here;
        let mut velocity = Vec3::ZERO;
        let contact = collide(
            &candidates,
            &[0.25, 0.0, 1.0],
            &[None, Some(LiquidKind::Water), None],
            &mut coord,
            &mut velocity,
            here,
        );
        assert_eq!(contact.hurts, [0.25]);
        assert_eq!(
            contact.underwater,
            Some(Underwater {
                volume_top: floor + 500.0,
                liquid: LiquidKind::Water
            })
        );
    }

    #[test]
    fn a_fatal_hurt_kills_in_the_middle_of_the_collision() {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let floor = map.floor_height(SPOT.x, SPOT.y);
        let here = Vec3::new(SPOT.x, floor + 200.0, SPOT.y);
        let around = CollisionBox::new(floor + 500.0, floor - 500.0, -100.0, 100.0, 100.0, -100.0)
            .at(Vec3::new(SPOT.x, 0.0, SPOT.y));
        let candidates = [
            target(
                1,
                LayerMask::from(CollisionKind::HurtEnemy),
                SolidSides::TOUCHABLE,
                around,
            ),
            target(
                2,
                LayerMask::from(CollisionKind::Liquid),
                SolidSides::TOUCHABLE,
                around,
            ),
        ];
        let world = EnemySurroundings {
            map: &map,
            fences: None,
            particles: None,
            candidates: &candidates,
            candidate_damage: &[0.6, 0.0],
            candidate_liquids: &[None, Some(LiquidKind::Water)],
            dt: 1.0 / 60.0,
        };
        let mover = EnemyMover {
            entity: Entity::PLACEHOLDER,
            shape: CollisionBox::new(70.0, 0.0, -40.0, 40.0, 40.0, -40.0),
            old_coord: here,
            radius: 50.0,
        };
        let on_terrain = GroundContact {
            on_terrain: true,
            ..default()
        };

        // A kill that sends the enemy flying: the rest of the collision
        // goes on with its new velocity.
        let (mut coord, mut velocity, mut health) = (here, Vec3::ZERO, Health(0.5));
        let mut kills = 0;
        let mut fly = |_: &mut Vec3, v: &mut Vec3| {
            kills += 1;
            *v = Vec3::new(0.0, 900.0, 0.0);
            false
        };
        let contact = collide_enemy(
            &world,
            &mover,
            &mut coord,
            &mut velocity,
            &mut health,
            on_terrain,
            default_enemy_collision_mask(),
            &mut fly,
        );
        assert_eq!(kills, 1);
        assert!(contact.killed && !contact.deleted);
        assert_eq!(velocity.y, 900.0);
        assert!(contact.underwater.is_some());
        assert!(contact.ground.on_terrain);

        // A kill that deletes the enemy stops the collision there.
        let (mut coord, mut velocity, mut health) = (here, Vec3::ZERO, Health(0.5));
        let contact = collide_enemy(
            &world,
            &mover,
            &mut coord,
            &mut velocity,
            &mut health,
            GroundContact::default(),
            default_enemy_collision_mask(),
            &mut |_, _| true,
        );
        assert!(contact.deleted);
        assert_eq!(contact.underwater, None);

        // A hurt that leaves health kills nothing.
        let (mut coord, mut velocity, mut health) = (here, Vec3::ZERO, Health(1.0));
        let contact = collide_enemy(
            &world,
            &mover,
            &mut coord,
            &mut velocity,
            &mut health,
            GroundContact::default(),
            default_enemy_collision_mask(),
            &mut |_, _| panic!("not killed"),
        );
        assert!(!contact.killed);
        assert!((health.0 - 0.4).abs() < 1e-6);
    }

    #[test]
    fn solid_objects_stop_the_enemy() {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let floor = map.floor_height(SPOT.x, SPOT.y);
        let old = Vec3::new(SPOT.x, floor + 1.0, SPOT.y);
        let wall = CollisionBox::new(floor + 500.0, floor - 500.0, -500.0, 500.0, 50.0, -50.0)
            .at(Vec3::new(SPOT.x, 0.0, SPOT.y - 100.0));
        let candidates = [target(
            1,
            LayerMask::from(CollisionKind::Misc),
            SolidSides::ALL,
            wall,
        )];
        // Walking into the wall's front (+Z) side.
        let mut coord = old - Vec3::Z * 20.0;
        let mut velocity = Vec3::new(0.0, 0.0, -1200.0);
        collide(&candidates, &[0.0], &[None], &mut coord, &mut velocity, old);
        assert_eq!(velocity.z, 0.0);
        assert!(
            (coord.z - (wall.front + 40.0 + 1.0)).abs() < 0.01,
            "{}",
            coord.z
        );
    }
}
