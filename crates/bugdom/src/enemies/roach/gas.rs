//! The roach's gas: clouds that drift, spin and grow where a roach walked,
//! drain the ball's time while the player is in them, and go off in a
//! burst of burning sparks when a hot particle touches them.
//!
//! Port of `LeaveGasTrail`, `MoveGasTrail` and `ExplodeGas`
//! (original/src/Enemies/Enemy_Roach.c). The roaches send [`GasPuff`]; the
//! player's own collision takes the drain (`CTYPE_DRAINBALLTIME` with
//! [`Damage`]).

use std::f32::consts::TAU;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::super::EnemySystems;
use crate::assets::model::Model;
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind, SolidSides, solid_object};
use crate::combat::Damage;
use crate::effects::{
    FULL_ALPHA, GlowMaterial, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups,
    ParticleKind, ParticleTexture, particle_hit,
};
use crate::level::{CurrentLevel, LevelType};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef};
use crate::physics::Velocity;
use crate::splines::SplineSystems;
use crate::state::{AppState, LevelAssets};

pub(super) fn plugin(app: &mut App) {
    app.add_message::<GasPuff>()
        .init_resource::<GasExplosionGroup>()
        .add_systems(OnEnter(AppState::InGame), reset_gas_explosion_group)
        .add_systems(
            FixedUpdate,
            (
                move_gas_clouds.in_set(EnemySystems::Move),
                spawn_gas_clouds
                    .after(EnemySystems::Move)
                    .after(SplineSystems::Move)
                    .run_if(in_state(AppState::InGame)),
            ),
        );
}

/// `NIGHT_MObjType_GasCloud`
const NIGHT_GAS_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 12);
/// `ANTHILL_MObjType_GasCloud`
const ANTHILL_GAS_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 2);

/// How far above the roach a cloud starts, in units: at least the base,
/// plus up to the spread.
const GAS_RISE_BASE: f32 = 100.0;
const GAS_RISE_SPREAD: f32 = 120.0;
/// A new cloud's size.
const GAS_START_SCALE: f32 = 0.6;
/// How fast a cloud widens, in scale per second, and the width at which
/// it stops. Its height stays at [`GAS_START_SCALE`].
const GAS_GROWTH_RATE: f32 = 1.0;
const GAS_MAX_SCALE: f32 = 2.0;
/// How long a cloud lasts, in seconds: at least the base, plus up to the
/// spread (`Health`).
const GAS_LIFE_BASE: f32 = 5.0;
const GAS_LIFE_SPREAD: f32 = 3.0;
/// In its last second a cloud fades out and stops draining
/// (`Health < 1`).
const GAS_FADE_TIME: f32 = 1.0;
/// A cloud can be set off only once it has this much life or less left
/// (`Health < 5`), so new ones don't.
const GAS_IGNITE_LIFE: f32 = 5.0;
/// The spread of a cloud's drift, in units per second, across the whole
/// width in x and z.
const GAS_DRIFT_SPREAD: f32 = 50.0;
/// The spread of its spin, in radians per second, across the whole width.
const GAS_SPIN_SPREAD: f32 = 2.0;
/// How much ball time a cloud drains per second (`Damage`).
const GAS_DRAIN: f32 = 0.2;
/// A cloud's collision box, relative to its centre. It doesn't grow with
/// the cloud.
const GAS_BOX: CollisionBox = CollisionBox::new(90.0, -90.0, -100.0, 100.0, 100.0, -100.0);

/// The burning sparks of an exploding cloud (`NewParticleGroup` in
/// `ExplodeGas`).
const GAS_EXPLOSION_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(
        ParticleFlags::HURT_PLAYER.0 | ParticleFlags::HOT.0 | ParticleFlags::HURT_ENEMY.0,
    ),
    gravity: 500.0,
    magnetism: 0.0,
    base_scale: 30.0,
    // Negative: the sparks grow as they fade.
    decay_rate: -1.5,
    fade_rate: 1.2,
    texture: ParticleTexture::OrangeSpot,
};
/// How many sparks an exploding cloud throws.
const GAS_EXPLOSION_SPARKS: usize = 25;
/// The spread of the sparks' sideways speed, across the whole width, and
/// their greatest upward speed, in units per second.
const GAS_EXPLOSION_SPREAD: f32 = 800.0;
const GAS_EXPLOSION_RISE: f32 = 600.0;

/// A roach leaves a gas cloud at `at` (its position).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct GasPuff {
    pub at: Vec3,
}

/// A drifting gas cloud, on its root entity. Its materials are its own,
/// so that it fades alone.
#[derive(Component, Debug, Clone)]
pub struct GasCloud {
    /// Seconds left (`Health`).
    pub life: f32,
    /// Its spin about y, in radians per second (`RotDeltaY`).
    pub spin: f32,
    /// The model child, which carries its rotation and scale.
    model: Entity,
    /// Each part's material and the alpha the model gives it.
    materials: Vec<(Handle<GlowMaterial>, f32)>,
    /// It no longer collides (`CType = 0`).
    spent: bool,
}

/// The particle group the exploding clouds add to while it has room
/// (`gCurrentGasParticleGroup`).
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GasExplosionGroup(pub Option<ParticleGroupId>);

/// Port of the gas part of `InitEnemyManager`
/// (original/src/Enemies/Enemy.c).
fn reset_gas_explosion_group(mut group: ResMut<GasExplosionGroup>) {
    group.0 = None;
}

/// What spawning a cloud needs: the level's gas model, built with glowing
/// materials of the cloud's own.
#[derive(SystemParam)]
pub struct GasMaker<'w> {
    level: Res<'w, CurrentLevel>,
    level_assets: Res<'w, LevelAssets>,
    models: Res<'w, Assets<Model>>,
    standard: Res<'w, Assets<StandardMaterial>>,
    materials: ResMut<'w, Assets<GlowMaterial>>,
}

/// The gas cloud model of a level type: the Ant Hill has its own, and the
/// original takes the Night's on any other level.
fn gas_model(level_type: LevelType) -> ModelRef {
    if level_type == LevelType::AntHill {
        ANTHILL_GAS_MODEL
    } else {
        NIGHT_GAS_MODEL
    }
}

/// How a new cloud starts, from the random numbers in the order the
/// original draws them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NewCloud {
    position: Vec3,
    yaw: f32,
    life: f32,
    velocity: Vec3,
    spin: f32,
}

impl NewCloud {
    /// Port of the random parts of `LeaveGasTrail`.
    fn roll(at: Vec3, random: &mut GameRandom) -> Self {
        let rise = GAS_RISE_BASE + random.next_f32() * GAS_RISE_SPREAD;
        let yaw = random.next_f32() * TAU;
        let life = GAS_LIFE_BASE + random.next_f32() * GAS_LIFE_SPREAD;
        let x = (random.next_f32() - 0.5) * GAS_DRIFT_SPREAD;
        let z = (random.next_f32() - 0.5) * GAS_DRIFT_SPREAD;
        let spin = (random.next_f32() - 0.5) * GAS_SPIN_SPREAD;
        Self {
            position: at + Vec3::Y * rise,
            yaw,
            life,
            velocity: Vec3::new(x, 0.0, z),
            spin,
        }
    }
}

/// Makes a gas cloud for each [`GasPuff`]: unlit, glowing (added to what
/// is behind it), not writing depth and drawn from both sides
/// (`STATUS_BIT_NULLSHADER | STATUS_BIT_GLOW | STATUS_BIT_NOZWRITE |
/// STATUS_BIT_KEEPBACKFACES`).
///
/// Port of the cloud in `LeaveGasTrail` (original/src/Enemies/Enemy_Roach.c).
/// The glow material has no fog, where the original fogs the clouds, and
/// the Night's fading of far objects (`gAutoFadeStatusBits`) isn't ported.
fn spawn_gas_clouds(
    mut commands: Commands,
    mut puffs: MessageReader<GasPuff>,
    mut random: ResMut<GameRandom>,
    mut maker: GasMaker,
) {
    let model_ref = gas_model(maker.level.def().level_type);
    for puff in puffs.read() {
        let cloud = NewCloud::roll(puff.at, &mut random);
        let Some(handle) = maker.level_assets.models.get(model_ref.file as usize) else {
            return;
        };
        let Some(model) = maker.models.get(handle) else {
            return;
        };
        let Some(group) = model.groups.get(model_ref.object) else {
            error!("{model_ref:?} is not in the level's model files");
            return;
        };
        let mut parts = Vec::new();
        let mut materials = Vec::new();
        for part in group.parts.iter().filter_map(|&i| model.parts.get(i)) {
            let base = maker
                .standard
                .get(&part.material)
                .cloned()
                .unwrap_or_default();
            let model_alpha = base.base_color.alpha();
            let material = maker.materials.add(GlowMaterial {
                color: base.base_color.to_linear(),
                texture: base.base_color_texture,
                draw_order: 0.0,
            });
            parts.push((part.mesh.clone(), material.clone()));
            materials.push((material, model_alpha));
        }

        let root = commands
            .spawn((
                Name::new("Gas cloud"),
                Transform::from_translation(cloud.position),
                Visibility::default(),
                Velocity(cloud.velocity),
                Damage(GAS_DRAIN),
                solid_object(
                    vec![GAS_BOX],
                    CollisionKind::DrainBallTime,
                    SolidSides::TOUCHABLE,
                ),
                DespawnOnExit(AppState::InGame),
            ))
            .id();
        let model_entity = commands
            .spawn((
                Name::new("Gas cloud model"),
                Transform::from_rotation(Quat::from_rotation_y(cloud.yaw))
                    .with_scale(Vec3::splat(GAS_START_SCALE)),
                Visibility::default(),
                ChildOf(root),
            ))
            .id();
        for (mesh, material) in parts {
            commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                ChildOf(model_entity),
            ));
        }
        commands.entity(root).insert(GasCloud {
            life: cloud.life,
            spin: cloud.spin,
            model: model_entity,
            materials,
            spent: false,
        });
    }
}

/// What happened to a cloud this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudFate {
    Drifting,
    /// Its life ran out.
    Gone,
}

/// Ages a cloud by `dt` seconds and widens its `scale` (`MoveGasTrail`).
fn age_cloud(life: &mut f32, scale: &mut Vec3, dt: f32) -> CloudFate {
    if scale.x < GAS_MAX_SCALE {
        scale.x += GAS_GROWTH_RATE * dt;
        scale.z = scale.x;
    }
    *life -= dt;
    if *life <= 0.0 {
        CloudFate::Gone
    } else {
        CloudFate::Drifting
    }
}

/// Moves, grows and fades the clouds, removes the spent ones and sets off
/// those a hot particle touches.
///
/// Port of `MoveGasTrail` (original/src/Enemies/Enemy_Roach.c).
fn move_gas_clouds(
    time: Res<Time>,
    mut commands: Commands,
    mut clouds: Query<(
        Entity,
        &mut GasCloud,
        &mut Transform,
        &Velocity,
        &CollisionBoxes,
    )>,
    mut models: Query<&mut Transform, Without<GasCloud>>,
    mut materials: ResMut<Assets<GlowMaterial>>,
    mut groups: ResMut<ParticleGroups>,
    mut explosion: ResMut<GasExplosionGroup>,
    mut random: ResMut<GameRandom>,
) {
    let dt = time.delta_secs();
    for (entity, mut cloud, mut transform, velocity, boxes) in &mut clouds {
        // The box and the cloud's `Coord` stay where the last tick left
        // them until `UpdateObject`, so the ignition test and the
        // explosion use the old position.
        let old = transform.translation;
        let Ok(mut model) = models.get_mut(cloud.model) else {
            continue;
        };
        model.rotate_y(cloud.spin * dt);
        transform.translation += **velocity * dt;

        if age_cloud(&mut cloud.life, &mut model.scale, dt) == CloudFate::Gone {
            commands.entity(entity).despawn();
            continue;
        }

        if cloud.life < GAS_FADE_TIME {
            for (handle, model_alpha) in &cloud.materials {
                if let Some(mut material) = materials.get_mut(handle) {
                    material.color.alpha = model_alpha * cloud.life;
                }
            }
            if !cloud.spent {
                cloud.spent = true;
                commands
                    .entity(entity)
                    .insert(CollisionLayers::new(LayerMask::NONE, LayerMask::NONE));
            }
        }

        if cloud.life < GAS_IGNITE_LIFE && particle_hit(&groups, &boxes.0, old, ParticleFlags::HOT)
        {
            explode_gas(&mut groups, &mut explosion.0, &mut random, old);
            commands.entity(entity).despawn();
        }
    }
}

/// Throws the burning sparks of a cloud going off at `at`, into the
/// current explosion group, starting a new one if it is gone. When the
/// group fills up, the next explosion starts another.
///
/// Port of `ExplodeGas` (original/src/Enemies/Enemy_Roach.c); the caller
/// deletes the cloud.
fn explode_gas(
    groups: &mut ParticleGroups,
    current: &mut Option<ParticleGroupId>,
    random: &mut GameRandom,
    at: Vec3,
) {
    let group = match *current {
        Some(id) if groups.is_valid(id) => Some(id),
        _ => {
            *current = groups.new_group(GAS_EXPLOSION_GROUP);
            *current
        }
    };
    if let Some(group) = group {
        for _ in 0..GAS_EXPLOSION_SPARKS {
            let velocity = Vec3::new(
                (random.next_f32() - 0.5) * GAS_EXPLOSION_SPREAD,
                random.next_f32() * GAS_EXPLOSION_RISE,
                (random.next_f32() - 0.5) * GAS_EXPLOSION_SPREAD,
            );
            let scale = random.next_f32() + 1.0;
            if groups.add_particle(group, at, velocity, scale, FULL_ALPHA) {
                *current = None;
                break;
            }
        }
    }
    // Sound: EFFECT_FIRECRACKER at `at`, pitch kMiddleC-10, volume .9.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ant_hill_has_its_own_gas_and_other_levels_the_nights() {
        assert_eq!(gas_model(LevelType::AntHill), ANTHILL_GAS_MODEL);
        assert_eq!(gas_model(LevelType::Night), NIGHT_GAS_MODEL);
    }

    #[test]
    fn a_new_cloud_starts_above_the_roach_and_drifts_slowly() {
        let mut random = GameRandom::default();
        for _ in 0..100 {
            let at = Vec3::new(10.0, 20.0, 30.0);
            let cloud = NewCloud::roll(at, &mut random);
            let rise = cloud.position.y - at.y;
            assert!((GAS_RISE_BASE..=GAS_RISE_BASE + GAS_RISE_SPREAD).contains(&rise));
            assert_eq!(cloud.position.xz(), at.xz());
            assert!((GAS_LIFE_BASE..=GAS_LIFE_BASE + GAS_LIFE_SPREAD).contains(&cloud.life));
            assert_eq!(cloud.velocity.y, 0.0);
            assert!(cloud.velocity.x.abs() <= GAS_DRIFT_SPREAD / 2.0);
            assert!(cloud.spin.abs() <= GAS_SPIN_SPREAD / 2.0);
        }
    }

    #[test]
    fn a_cloud_widens_to_its_limit_and_runs_out() {
        let mut life = 3.0;
        let mut scale = Vec3::splat(GAS_START_SCALE);
        let dt = 0.1;
        let mut fate = CloudFate::Drifting;
        let mut ticks = 0;
        while fate == CloudFate::Drifting {
            fate = age_cloud(&mut life, &mut scale, dt);
            ticks += 1;
        }
        assert!((29..=31).contains(&ticks));
        // It stops growing once at the limit, a step past it at most.
        assert!(scale.x >= GAS_MAX_SCALE && scale.x < GAS_MAX_SCALE + dt);
        assert_eq!(scale.z, scale.x);
        assert_eq!(scale.y, GAS_START_SCALE);
    }

    #[test]
    fn explosions_share_a_group_until_it_fills() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut current = None;
        explode_gas(&mut groups, &mut current, &mut random, Vec3::ZERO);
        let first = current.expect("a group was made");
        explode_gas(&mut groups, &mut current, &mut random, Vec3::ZERO);
        assert_eq!(current, Some(first));
        let group = groups.get(first).expect("the group is alive");
        assert_eq!(group.particles().len(), 2 * GAS_EXPLOSION_SPARKS);
        assert!(group.desc.flags.contains(ParticleFlags::HOT));
        assert!(group.desc.flags.contains(ParticleFlags::HURT_PLAYER));
        assert!(group.desc.flags.contains(ParticleFlags::HURT_ENEMY));

        // 200 particles fit: the eighth explosion fills the group and
        // lets the next one start another.
        for _ in 2..8 {
            explode_gas(&mut groups, &mut current, &mut random, Vec3::ZERO);
        }
        assert_eq!(current, Some(first));
        explode_gas(&mut groups, &mut current, &mut random, Vec3::ZERO);
        assert_eq!(current, None);
        explode_gas(&mut groups, &mut current, &mut random, Vec3::ZERO);
        assert!(current.is_some_and(|id| id != first));
    }
}
