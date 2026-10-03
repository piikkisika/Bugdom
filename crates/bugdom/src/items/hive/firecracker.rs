//! Firecrackers: on the Hive they go off with their detonator; at Night
//! the cherry bombs and firecrackers go off when a hot spark hits them,
//! with a shockwave that hurts the player and the enemies.
//!
//! Port of `AddFirecracker`, `MoveFirecracker`, `MoveFirecrackerAtNight`
//! and `ExplodeFirecracker` (original/src/Items/Items2.c), and of
//! `MakeShockwave` and `MoveShockwave` (original/src/Items/Traps.c).

use avian3d::prelude::LayerMask;
use bevy::prelude::*;

use super::model;
use crate::collision::{
    CollisionBox, CollisionBoxes, CollisionKind, CollisionSystems, SolidSides, solid_object,
};
use crate::combat::Damage;
use crate::effects::{
    EffectsSystems, Explosion, FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroups,
    ParticleKind, ParticleTexture, ShardMode, explode_geometry, particle_hit,
};
use crate::items::kind as item;
use crate::items::pickups::DetonatorsBlown;
use crate::items::scenery::on_level;
use crate::items::{
    DespawnOutOfRange, ItemSpawn, ItemSystems, RegisterItemKind, TerrainItemSource,
    forget_terrain_item,
};
use crate::level::{CurrentLevel, LevelType};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, ObjectMaterial, Shading};
use crate::player::PlayerSystems;
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::FIRECRACKER, add_firecracker)
        .add_systems(
            FixedUpdate,
            (
                // After the detonators' plungers, which move before the
                // player, and before this tick's particles move.
                explode_firecrackers
                    .after(PlayerSystems::Move)
                    .before(EffectsSystems::MoveParticles),
                // The shockwave only touches, so when it grows doesn't
                // change what it hits; growing it before the collision
                // keeps its box and its collider in step.
                grow_shockwaves
                    .after(ItemSystems::Track)
                    .before(CollisionSystems::Gather),
            )
                .run_if(in_state(AppState::InGame)),
        )
        .add_systems(Update, fade_shockwaves.run_if(in_state(AppState::InGame)));
}

const HIVE_FIRECRACKER_SCALE: f32 = 0.3;
const NIGHT_FIRECRACKER_SCALE: f32 = 0.6;
const FIRECRACKER_BOX: CollisionBox = CollisionBox::new(60.0, 0.0, -70.0, 70.0, 30.0, -30.0);
const CHERRY_BOMB_BOX: CollisionBox = CollisionBox::new(200.0, 0.0, -160.0, 160.0, 160.0, -160.0);
/// At Night `params[0]` picks the cherry bomb (0) or the firecracker.
const NIGHT_CHERRY_BOMB: u8 = 0;

/// The white sparks of an explosion.
const WHITE_SPARKS: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(ParticleFlags::BOUNCE.0 | ParticleFlags::HOT.0),
    gravity: 400.0,
    magnetism: 0.0,
    base_scale: 35.0,
    decay_rate: 1.5,
    fade_rate: 0.0,
    texture: ParticleTexture::White,
};
const WHITE_SPARK_COUNT: usize = 40;
/// The white sparks' speed: the full width across, and the most upward,
/// in units per second.
const WHITE_SPARK_SPREAD: f32 = 1200.0;
const WHITE_SPARK_RISE: f32 = 1100.0;
/// The fire sparks of an explosion.
const FIRE_SPARKS: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(ParticleFlags::BOUNCE.0 | ParticleFlags::HOT.0),
    gravity: 400.0,
    magnetism: 0.0,
    base_scale: 30.0,
    decay_rate: -2.1,
    fade_rate: 2.5,
    texture: ParticleTexture::Fire,
};
const FIRE_SPARK_COUNT: usize = 50;
const FIRE_SPARK_SPREAD: f32 = 1000.0;
const FIRE_SPARK_RISE: f32 = 800.0;

/// The shockwave's size when made, in units.
const SHOCKWAVE_START_SCALE: f32 = 300.0;
/// How fast it grows, in units per second.
const SHOCKWAVE_GROWTH: f32 = 600.0;
/// How long it lasts, in seconds, which is also its starting opacity
/// (`Health`).
const SHOCKWAVE_LIFE: f32 = 0.6;
const SHOCKWAVE_DAMAGE: f32 = 0.25;

/// What sets a firecracker off.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firecracker {
    /// The Hive's: when its detonator blows (`DetonatorID`).
    Detonator(u8),
    /// The Night's: when a hot spark hits it. It makes a shockwave.
    Spark,
}

/// Port of `AddFirecracker`. On the Hive `params[0]` is the detonator, and
/// one that has already blown leaves nothing (the item is still used up);
/// at Night it picks the cherry bomb or the firecracker.
fn add_firecracker(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level: Res<CurrentLevel>,
    blown: Res<DetonatorsBlown>,
) -> bool {
    if !on_level(&level, &[LevelType::Hive, LevelType::Night], "Firecracker") {
        return false;
    }
    let (firecracker, model, scale, shape) = if level.def().level_type == LevelType::Night {
        let shape = if spawn.params[0] == NIGHT_CHERRY_BOMB {
            CHERRY_BOMB_BOX
        } else {
            FIRECRACKER_BOX
        };
        let model = ModelRef::new(
            ModelFile::Level1,
            model::NIGHT_CHERRY_BOMB + usize::from(spawn.params[0]),
        );
        (Firecracker::Spark, model, NIGHT_FIRECRACKER_SCALE, shape)
    } else {
        let id = spawn.params[0];
        if blown.is_blown(id) {
            return true;
        }
        (
            Firecracker::Detonator(id),
            model::FIRECRACKER,
            HIVE_FIRECRACKER_SCALE,
            FIRECRACKER_BOX,
        )
    };
    // Night's fading of distant objects (`gAutoFadeStatusBits`) is not
    // ported yet.
    let entity = commands
        .spawn((
            Name::new("Firecracker"),
            Transform::from_xyz(
                spawn.position.x,
                map.floor_height(spawn.position.x, spawn.position.y),
                spawn.position.y,
            ),
            Visibility::default(),
            firecracker,
            solid_object(
                vec![shape],
                [CollisionKind::Misc, CollisionKind::BlockCamera],
                SolidSides::ALL,
            ),
            TerrainItemSource(spawn.index),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    models.spawn(
        &mut commands,
        entity,
        model,
        Shading::Lit,
        Transform::from_scale(Vec3::splat(scale)),
    );
    true
}

/// Sets off the Hive's firecrackers whose detonator has blown and the
/// Night's that a hot spark hits. Port of `MoveFirecracker` and
/// `MoveFirecrackerAtNight`; leaving the item window is
/// [`DespawnOutOfRange`].
fn explode_firecrackers(
    mut commands: Commands,
    mut models: ModelSpawner,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    blown: Res<DetonatorsBlown>,
    map: Res<TerrainMap>,
    firecrackers: Query<(Entity, &Firecracker, &Transform, &CollisionBoxes)>,
) {
    for (entity, firecracker, transform, boxes) in &firecrackers {
        let at = transform.translation;
        let shockwave = match *firecracker {
            Firecracker::Detonator(id) => {
                if !blown.is_blown(id) {
                    continue;
                }
                false
            }
            Firecracker::Spark => {
                if !particle_hit(&groups, &boxes.0, at, ParticleFlags::HOT) {
                    continue;
                }
                true
            }
        };
        explode(&mut commands, entity, at, &mut groups, &mut random);
        if shockwave {
            make_shockwave(&mut commands, &mut models, &map, at);
        }
    }
}

/// `QD3D_ExplodeGeometry(fc, 2000, SHARD_MODE_BOUNCE, 1, .6)` in
/// `ExplodeFirecracker`.
const FIRECRACKER_SHARDS: Explosion = Explosion {
    force: 2000.0,
    mode: ShardMode::BOUNCE,
    density: 1,
    decay: 0.6,
};

/// The firecracker goes for good in two bursts of sparks. Port of
/// `ExplodeFirecracker`; the shockwave is the caller's.
fn explode(
    commands: &mut Commands,
    entity: Entity,
    at: Vec3,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
) {
    commands.queue(explode_geometry(entity, FIRECRACKER_SHARDS));
    let mut firecracker = commands.entity(entity);
    forget_terrain_item(&mut firecracker);
    firecracker.despawn();

    let bursts = [
        (
            WHITE_SPARKS,
            WHITE_SPARK_COUNT,
            WHITE_SPARK_SPREAD,
            WHITE_SPARK_RISE,
        ),
        (
            FIRE_SPARKS,
            FIRE_SPARK_COUNT,
            FIRE_SPARK_SPREAD,
            FIRE_SPARK_RISE,
        ),
    ];
    for (desc, count, spread, rise) in bursts {
        let Some(group) = groups.new_group(desc) else {
            continue;
        };
        for _ in 0..count {
            let velocity = Vec3::new(
                (random.next_f32() - 0.5) * spread,
                random.next_f32() * rise,
                (random.next_f32() - 0.5) * spread,
            );
            let scale = random.next_f32() + 1.0;
            groups.add_particle(group, at, velocity, scale, FULL_ALPHA);
        }
    }
    // Sound: EFFECT_KABLAM at the firecracker.
}

/// A growing, fading ring that hurts what it touches.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Shockwave {
    /// Seconds left, and its opacity (`Health`).
    life: f32,
    /// Its size, in units.
    scale: f32,
    model: Option<Entity>,
}

impl Shockwave {
    /// Ages and grows the shockwave for `dt` seconds; returns whether it is
    /// still there.
    fn step(&mut self, dt: f32) -> bool {
        self.life -= dt;
        if self.life <= 0.0 {
            return false;
        }
        self.scale += SHOCKWAVE_GROWTH * dt;
        true
    }
}

/// A cube as big as the shockwave.
fn shockwave_box(scale: f32) -> CollisionBox {
    CollisionBox::new(scale, -scale, -scale, scale, scale, -scale)
}

/// The fading material of the shockwave's model (`MakeObjectTransparent`,
/// `STATUS_BIT_NOZWRITE` and `STATUS_BIT_KEEPBACKFACES_2PASS`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct ShockwaveMaterial {
    alpha: f32,
    applied: f32,
}

/// Port of `MakeShockwave`: on the floor below `at`.
fn make_shockwave(commands: &mut Commands, models: &mut ModelSpawner, map: &TerrainMap, at: Vec3) {
    let scale = SHOCKWAVE_START_SCALE;
    let entity = commands
        .spawn((
            Name::new("Shockwave"),
            Transform::from_xyz(at.x, map.floor_height(at.x, at.z), at.z),
            Visibility::default(),
            Damage(SHOCKWAVE_DAMAGE),
            solid_object(
                vec![shockwave_box(scale)],
                LayerMask::from([CollisionKind::HurtMe, CollisionKind::HurtEnemy]),
                SolidSides::TOUCHABLE,
            ),
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    let model = models.spawn(
        commands,
        entity,
        model::SHOCKWAVE,
        Shading::Lit,
        Transform::from_scale(Vec3::splat(scale)),
    );
    commands.entity(entity).insert(Shockwave {
        life: SHOCKWAVE_LIFE,
        scale,
        model,
    });
}

/// Grows the shockwaves and their boxes until they fade out. Port of
/// `MoveShockwave`.
fn grow_shockwaves(
    time: Res<Time>,
    mut commands: Commands,
    mut shockwaves: Query<(Entity, &mut Shockwave)>,
    mut models: Query<&mut Transform>,
) {
    for (entity, mut shockwave) in &mut shockwaves {
        if !shockwave.step(time.delta_secs()) {
            commands.entity(entity).despawn();
            continue;
        }
        let scale = shockwave.scale;
        let boxes = CollisionBoxes(vec![shockwave_box(scale)]);
        commands.entity(entity).insert((boxes.collider(), boxes));
        if let Some(mut transform) = shockwave.model.and_then(|m| models.get_mut(m).ok()) {
            transform.scale = Vec3::splat(scale);
        }
    }
}

/// Fades each shockwave's model with its life. Its meshes get materials of
/// their own the first time, so other objects of the same model are left
/// alone.
fn fade_shockwaves(
    mut commands: Commands,
    shockwaves: Query<(Entity, &Shockwave)>,
    children: Query<&Children>,
    mut meshes: Query<(
        &mut MeshMaterial3d<ObjectMaterial>,
        Option<&mut ShockwaveMaterial>,
    )>,
    mut materials: ResMut<Assets<ObjectMaterial>>,
) {
    for (root, shockwave) in &shockwaves {
        let opacity = shockwave.life.max(0.0);
        for entity in children.iter_descendants(root) {
            let Ok((mut handle, own)) = meshes.get_mut(entity) else {
                continue;
            };
            match own {
                Some(mut own) => {
                    if own.applied == opacity {
                        continue;
                    }
                    own.applied = opacity;
                    if let Some(mut material) = materials.get_mut(&handle.0) {
                        material.base.base_color.set_alpha(own.alpha * opacity);
                    }
                }
                None => {
                    let Some(mut material) = materials.get(&handle.0).cloned() else {
                        continue;
                    };
                    let base = &mut material.base;
                    let alpha = base.base_color.alpha();
                    base.base_color.set_alpha(alpha * opacity);
                    base.alpha_mode = AlphaMode::Blend;
                    base.cull_mode = None;
                    base.double_sided = true;
                    handle.0 = materials.add(material);
                    commands.entity(entity).insert(ShockwaveMaterial {
                        alpha,
                        applied: opacity,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shockwave_grows_until_it_fades_out() {
        let mut shockwave = Shockwave {
            life: SHOCKWAVE_LIFE,
            scale: SHOCKWAVE_START_SCALE,
            model: None,
        };
        assert!(shockwave.step(0.5));
        assert_eq!(shockwave.scale, 600.0);
        assert!((shockwave.life - 0.1).abs() < 1e-6);
        assert!(!shockwave.step(0.2));
        let cube = shockwave_box(600.0);
        assert_eq!((cube.left, cube.top, cube.back), (-600.0, 600.0, -600.0));
    }

    #[test]
    fn explosions_hot_sparks_set_off_other_firecrackers() {
        assert!(WHITE_SPARKS.flags.contains(ParticleFlags::HOT));
        assert!(FIRE_SPARKS.flags.contains(ParticleFlags::HOT));
        assert!(!WHITE_SPARKS.flags.intersects(ParticleFlags::HURT_PLAYER));
    }
}
