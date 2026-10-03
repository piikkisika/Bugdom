//! The stump and the hive hanging from it, on the Dragonfly Attack level.
//! The dragonfly's fireballs rattle the hive, which lets out bees; once
//! its health is gone it burns, and when the fire is out the level is
//! completed.
//!
//! Port of `AddStump`, `MoveStump` and `RattleHive`
//! (original/src/Items/Items.c). The hive's health shows on the infobar's
//! boss bar on that level (`ShowBossHealth`, original/src/Screens/Infobar.c).

use avian3d::prelude::LayerMask;
use bevy::prelude::*;

use super::super::kind as item;
use super::super::scenery::{StaticObject, on_level};
use super::super::triggers::ChainedTo;
use super::super::{
    AreaCompleted, DespawnOutOfRange, Hive, ItemSpawn, RattleHive, RegisterItemKind,
};
use super::TrapModel;
use crate::collision::{CollisionBox, CollisionKind, SolidSides, solid_object};
use crate::combat::{BossHealthBar, Health};
use crate::effects::{
    EffectsSystems, FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups,
    ParticleKind, ParticleTexture,
};
use crate::enemies::EnemySpawner;
use crate::enemies::flying_bee::make_flying_bee;
use crate::level::{CurrentLevel, LevelType};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::STUMP, add_stump).add_systems(
        FixedUpdate,
        (
            rattle_hives,
            wobble_hives,
            burn_hives.before(EffectsSystems::MoveParticles),
        )
            .chain()
            .run_if(in_state(AppState::InGame)),
    );
}

/// `FOREST_MObjType_Stump` and `FOREST_MObjType_Hive`.
const STUMP_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 6);
const HIVE_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 7);
/// `STUMP_SCALE`
const STUMP_SCALE: f32 = 25.0;
/// `HIVE_SCALE`
const HIVE_SCALE: f32 = 17.0;
/// Where the hive hangs from the stump's base, before scaling.
const HIVE_OFFSET: Vec3 = Vec3::new(130.0, 150.0, 0.0);
/// The hive's health when full, which is also the boss bar's full value.
const HIVE_HEALTH: f32 = 1.0;
/// `LEVEL_NUM_FLIGHT`, where the hive's health shows on the boss bar.
const FLIGHT_LEVEL: usize = 4;
/// What each fireball takes off the hive's health.
const RATTLE_DAMAGE: f32 = 0.02;
/// How far a rattled hive swings, in radians.
const RATTLE_WOBBLE: f32 = 0.12;
/// How fast the hive swings, in radians of its sine per second.
const WOBBLE_RATE: f32 = 4.0;
/// How fast the swing dies down, in radians per second.
const WOBBLE_DECAY: f32 = 0.01;
/// How many bees a rattled hive lets out.
const RATTLE_BEES: usize = 3;
/// The bees come out of a box this big around the hive's body, in units.
const BEE_SPREAD: Vec3 = Vec3::new(400.0, 300.0, 200.0);
/// How far below the hive's origin its body is, before scaling.
const HIVE_BODY_DEPTH: f32 = 163.0;
/// How far toward +Z of the hive's origin the bees come out, before scaling.
const BEE_EXIT_Z: f32 = 20.0;
/// How long the hive burns before the level is completed, in seconds.
const BURN_TIME: f32 = 6.0;
/// Seconds between bursts of flames (`FireTimer > .01`).
const FLAME_INTERVAL: f32 = 0.01;
/// How many flames each burst has.
const FLAMES_PER_BURST: usize = 8;
/// The size of the box the flames start in, before scaling.
const FLAME_SPREAD: Vec3 = Vec3::new(60.0, 70.0, 60.0);
/// The flames' random speed, the whole width, in units per second.
const FLAME_SPEED_SPREAD: Vec3 = Vec3::new(20.0, 30.0, 20.0);
/// How fast the flames rise besides, in units per second.
const FLAME_RISE: f32 = 40.0;
/// The flames of the burning hive.
const FLAME_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::NONE,
    gravity: -1000.0,
    magnetism: 0.0,
    base_scale: 40.0,
    decay_rate: -1.0,
    fade_rate: 0.3,
    texture: ParticleTexture::Fire,
};

/// A hive's swing and fire (`HiveWobbleIndex`, `HiveWobbleStrength`,
/// `FireTimer`, `HiveOnFire`, `HiveBurning`, and its particle group).
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
struct HiveState {
    /// Where in its swing it is, in radians.
    wobble_index: f32,
    /// How far it swings, in radians.
    wobble_strength: f32,
    /// Seconds until the fire is out, once burning; it keeps counting
    /// below zero.
    burning: Option<f32>,
    /// Seconds since the last burst of flames.
    flame_timer: f32,
    flames: Option<ParticleGroupId>,
}

impl HiveState {
    /// Hurts the hive. Returns whether it is still whole, and so swings
    /// and lets bees out.
    ///
    /// Port of `RattleHive` without the bees.
    fn rattle(&mut self, health: &mut Health) -> bool {
        if health.lose(RATTLE_DAMAGE) {
            if self.burning.is_none() {
                self.burning = Some(BURN_TIME);
            }
            false
        } else {
            self.wobble_strength = RATTLE_WOBBLE;
            true
        }
    }

    /// Swings for `dt` seconds and returns the hive's tilt about x.
    fn wobble(&mut self, dt: f32) -> f32 {
        self.wobble_index += WOBBLE_RATE * dt;
        self.wobble_strength = (self.wobble_strength - WOBBLE_DECAY * dt).max(0.0);
        self.wobble_index.sin() * self.wobble_strength
    }
}

/// The stump's two boxes, relative to its base: the trunk and the branch
/// the hive hangs from.
fn stump_boxes() -> Vec<CollisionBox> {
    let s = STUMP_SCALE;
    vec![
        CollisionBox::new(386.0 * s, 0.0, -18.0 * s, 18.0 * s, 18.0 * s, -18.0 * s),
        CollisionBox::new(
            162.0 * s,
            134.0 * s,
            18.0 * s,
            194.0 * s,
            13.0 * s,
            -13.0 * s,
        ),
    ]
}

/// The hive's three boxes, relative to where it hangs: the stalk, the body
/// and the entrance.
fn hive_boxes() -> Vec<CollisionBox> {
    let s = HIVE_SCALE;
    vec![
        CollisionBox::new(0.0, -113.0 * s, -6.0 * s, 6.0 * s, 6.0 * s, -6.0 * s),
        CollisionBox::new(
            -113.0 * s,
            -198.0 * s,
            -31.0 * s,
            31.0 * s,
            31.0 * s,
            -31.0 * s,
        ),
        CollisionBox::new(
            -147.0 * s,
            -163.0 * s,
            -10.0 * s,
            10.0 * s,
            41.0 * s,
            31.0 * s,
        ),
    ]
}

/// Port of `AddStump`. The stump never goes away once added ("this thing
/// doesnt ever go away"), so it has no [`DespawnOutOfRange`]; the hive
/// goes with it.
fn add_stump(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
) -> bool {
    if !on_level(&level, &[LevelType::Forest], "Stump") {
        return false;
    }
    let y = map.floor_height(spawn.position.x, spawn.position.y);
    let stump = StaticObject {
        y,
        model: STUMP_MODEL,
        shading: Shading::Lit,
        yaw: 0.0,
        scale: STUMP_SCALE,
        boxes: stump_boxes(),
        kinds: CollisionKind::Misc.into(),
    }
    .spawn("Stump", &mut commands, &mut models, &spawn);
    commands.entity(stump).remove::<DespawnOutOfRange>();

    let at = Vec3::new(spawn.position.x, y, spawn.position.y) + HIVE_OFFSET * STUMP_SCALE;
    let hive = commands
        .spawn((
            Name::new("Hive"),
            Transform::from_translation(at),
            Visibility::default(),
            Hive,
            HiveState::default(),
            Health(HIVE_HEALTH),
            solid_object(
                hive_boxes(),
                LayerMask::from(CollisionKind::Misc),
                SolidSides::ALL,
            ),
            ChainedTo(stump),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    if level.0 == FLIGHT_LEVEL {
        commands
            .entity(hive)
            .insert(BossHealthBar { full: HIVE_HEALTH });
    }
    if let Some(model) = models.spawn(
        &mut commands,
        hive,
        HIVE_MODEL,
        Shading::Lit,
        Transform::from_scale(Vec3::splat(HIVE_SCALE)),
    ) {
        commands.entity(hive).insert(TrapModel(model));
    }
    true
}

/// Hurts the hives the dragonfly's fireballs hit; a hive still whole swings
/// and lets out bees.
///
/// Port of `RattleHive` (original/src/Items/Items.c). Each bee is spawned
/// on its own, so that the bee count it checks includes the ones before.
fn rattle_hives(
    mut commands: Commands,
    mut rattles: MessageReader<RattleHive>,
    mut hives: Query<(&Transform, &mut Health, &mut HiveState), With<Hive>>,
) {
    for rattle in rattles.read() {
        let Ok((transform, mut health, mut hive)) = hives.get_mut(rattle.hive) else {
            continue;
        };
        if hive.rattle(&mut health) {
            for _ in 0..RATTLE_BEES {
                commands.run_system_cached_with(let_out_bee, transform.translation);
            }
        }
    }
}

/// Lets one bee out of the hive hanging at `hive`, if there aren't too
/// many already (`MakeFlyingBee`).
fn let_out_bee(In(hive): In<Vec3>, mut enemies: EnemySpawner, mut random: ResMut<GameRandom>) {
    let mut spread = || random.next_f32() - 0.5;
    let offset = Vec3::new(spread(), spread(), spread()) * BEE_SPREAD;
    let at = hive + Vec3::new(0.0, -HIVE_BODY_DEPTH, BEE_EXIT_Z) * HIVE_SCALE + offset;
    make_flying_bee(&mut enemies, &mut random, at);
}

/// Swings the hives. Port of the wobble in `MoveStump`.
fn wobble_hives(
    time: Res<Time>,
    mut hives: Query<(&mut HiveState, &TrapModel)>,
    mut models: Query<&mut Transform, Without<HiveState>>,
) {
    let dt = time.delta_secs();
    for (mut hive, model) in &mut hives {
        let tilt = hive.wobble(dt);
        if let Ok(mut transform) = models.get_mut(model.0) {
            transform.rotation = Quat::from_rotation_x(tilt);
        }
    }
}

/// Sets the burning hives' flames going and completes the area once the
/// fire is out. Port of the fire in `MoveStump`.
fn burn_hives(
    time: Res<Time>,
    mut hives: Query<(&Transform, &mut HiveState)>,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    completed: Option<ResMut<AreaCompleted>>,
) {
    let dt = time.delta_secs();
    let mut done = false;
    for (transform, mut hive) in &mut hives {
        let Some(left) = hive.burning else {
            continue;
        };
        // Sound: EFFECT_FIRECRACKLE at the hive's body, kept going.
        let left = left - dt;
        hive.burning = Some(left);
        if left < 0.0 {
            done = true;
        }
        let body = transform.translation - Vec3::Y * HIVE_BODY_DEPTH * HIVE_SCALE;
        emit_flames(&mut hive, &mut groups, &mut random, body, dt);
    }
    if done && let Some(mut completed) = completed {
        **completed = true;
    }
}

/// A burst of flames every [`FLAME_INTERVAL`]. When the group is full the
/// burst starts over in a new group, once per tick, as the original jumps
/// back to making one.
fn emit_flames(
    hive: &mut HiveState,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    body: Vec3,
    dt: f32,
) {
    let mut made_new = false;
    if !hive.flames.is_some_and(|id| groups.is_valid(id)) {
        hive.flames = groups.new_group(FLAME_GROUP);
        made_new = true;
    }
    'burst: while let Some(group) = hive.flames {
        hive.flame_timer += dt;
        if hive.flame_timer <= FLAME_INTERVAL {
            break;
        }
        hive.flame_timer = 0.0;
        for _ in 0..FLAMES_PER_BURST {
            let mut spread = || random.next_f32() - 0.5;
            let at = body + Vec3::new(spread(), spread(), spread()) * FLAME_SPREAD * HIVE_SCALE;
            let velocity =
                Vec3::new(spread(), spread(), spread()) * FLAME_SPEED_SPREAD + Vec3::Y * FLAME_RISE;
            let scale = random.next_f32() + 3.0;
            if groups.add_particle(group, at, velocity, scale, FULL_ALPHA) && !made_new {
                hive.flames = groups.new_group(FLAME_GROUP);
                made_new = true;
                continue 'burst;
            }
        }
        break;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rattling_hurts_the_hive_and_sets_it_swinging() {
        let mut hive = HiveState::default();
        let mut health = Health(HIVE_HEALTH);
        assert!(hive.rattle(&mut health));
        assert!((health.0 - (HIVE_HEALTH - RATTLE_DAMAGE)).abs() < 1e-6);
        assert_eq!(hive.wobble_strength, RATTLE_WOBBLE);
        assert_eq!(hive.burning, None);
    }

    #[test]
    fn the_last_rattle_sets_the_hive_on_fire_once() {
        let mut hive = HiveState::default();
        let mut health = Health(RATTLE_DAMAGE / 2.0);
        assert!(!hive.rattle(&mut health));
        assert_eq!(hive.burning, Some(BURN_TIME));
        hive.burning = Some(1.0);
        assert!(!hive.rattle(&mut health));
        assert_eq!(hive.burning, Some(1.0));
        assert_eq!(hive.wobble_strength, 0.0);
    }

    #[test]
    fn the_swing_dies_down() {
        let mut hive = HiveState {
            wobble_strength: RATTLE_WOBBLE,
            ..default()
        };
        let tilt = hive.wobble(0.25);
        assert!(tilt.abs() <= RATTLE_WOBBLE);
        hive.wobble(RATTLE_WOBBLE / WOBBLE_DECAY);
        assert_eq!(hive.wobble_strength, 0.0);
        assert_eq!(hive.wobble(0.1), 0.0);
    }

    #[test]
    fn a_burning_hive_completes_the_area_when_the_fire_is_out() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ParticleGroups>()
            .init_resource::<GameRandom>()
            .insert_resource(AreaCompleted(false))
            .add_systems(Update, burn_hives);
        app.world_mut().spawn((
            Transform::default(),
            HiveState {
                // Just out: the count has gone below zero.
                burning: Some(-1e-3),
                ..default()
            },
        ));
        app.update();
        assert!(**app.world().resource::<AreaCompleted>());
        assert!(app.world().resource::<ParticleGroups>().iter().count() >= 1);
    }

    #[test]
    fn a_full_flame_group_is_replaced_once_per_tick() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut hive = HiveState::default();
        let dt = 1.0 / 60.0;
        emit_flames(&mut hive, &mut groups, &mut random, Vec3::ZERO, dt);
        let first = hive.flames;
        assert!(first.is_some());
        // Fill it, then burn again: no new group in a tick that made one.
        for _ in 0..crate::effects::MAX_PARTICLES {
            emit_flames(&mut hive, &mut groups, &mut random, Vec3::ZERO, dt);
        }
        assert_ne!(hive.flames, first);
    }
}
