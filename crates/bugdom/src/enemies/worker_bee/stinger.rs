//! The worker bee's stinger: held at its butt, then shot at the player.
//!
//! Port of `GiveWorkerBeeStinger`, `AlignStingerOnBee`, `ShootStinger` and
//! `MoveStinger` (original/src/Enemies/Enemy_WorkerBee.c).
//!
//! A held stinger is a child of its bee, so that it is shown, hidden and
//! deleted with it, as the original's `ChainNode` is. Shot, it leaves the
//! bee and becomes a projectile: a `HurtMe` object whose [`Damage`] the
//! player's own collision takes, flying until it hits the floor or the
//! ceiling.

use std::f32::consts::PI;

use avian3d::prelude::TransformInterpolation;
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{WORKER_BEE_SCALE, WorkerBeeBrain};
use crate::collision::{CollisionBox, CollisionKind, SolidSides, solid_object};
use crate::combat::Damage;
use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroups, ParticleKind, ParticleTexture,
};
use crate::enemies::EnemyModel;
use crate::items::DespawnOutOfRange;
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::Player;
use crate::skeleton::SkeletonRig;
use crate::state::AppState;
use crate::terrain::{LayerKind, TerrainMap};

/// `HIVE_MObjType_Stinger`
const STINGER_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 26);
/// The held stinger's size, relative to the bee's joint (`STINGER_SCALE`).
const STINGER_SCALE: f32 = WORKER_BEE_SCALE * 0.25;
/// The shot stinger's size. `ShootStinger` leaves the scale
/// `GiveWorkerBeeStinger` gave it, which only shows once `UpdateObject`
/// rebuilds the matrix from it in flight.
const FLYING_STINGER_SCALE: f32 = WORKER_BEE_SCALE - WORKER_BEE_SCALE * STINGER_SCALE;
/// The bee's butt joint, which holds the stinger (`WORKERBEE_JOINT_BUTT`).
const BUTT_JOINT: usize = 1;
/// The stinger's tilt on the butt, about x, in radians.
const STINGER_TILT: f32 = 0.4;
/// Where the stinger sits in the butt joint's space.
const STINGER_GRIP: Vec3 = Vec3::new(0.0, -14.0 * WORKER_BEE_SCALE, 38.0 * WORKER_BEE_SCALE);

/// How fast a stinger is shot, in units per second (`STINGER_SPEED`).
const STINGER_SPEED: f32 = 1000.0;
/// Gravity on a flying stinger, in units per second squared.
const STINGER_GRAVITY: f32 = 300.0;
/// What a stinger does to the player it hits.
const STINGER_DAMAGE: f32 = 0.3;
/// A shot stinger's collision box.
const STINGER_BOX: CollisionBox = CollisionBox::new(40.0, -40.0, -50.0, 50.0, 50.0, -50.0);

/// The sparks a shot makes (`NewParticleGroup` in `ShootStinger`).
const SHOT_SPARK_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::BOUNCE,
    gravity: 500.0,
    magnetism: 0.0,
    base_scale: 15.0,
    decay_rate: 1.3,
    fade_rate: 0.0,
    texture: ParticleTexture::YellowBall,
};
/// How many sparks a shot makes.
const SHOT_SPARKS: usize = 15;
/// The spread of the sparks' velocities, in units per second.
const SHOT_SPARK_SPEED: f32 = 400.0;

/// A worker bee's stinger, held or shot.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Stinger {
    /// Its model, which carries its scale.
    pub model: Option<Entity>,
    /// Where it was in the world when last placed on the butt (`Coord`).
    pub held_at: Vec3,
    /// It has been shot (`MoveCall = MoveStinger`).
    pub flying: bool,
}

/// A stinger was shot from here; its sparks fly.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct StingerShot {
    pub at: Vec3,
}

/// The stingers, as the bees' move reads and shoots them.
pub type Stingers<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut Stinger),
    (Without<WorkerBeeBrain>, Without<Player>),
>;

/// Gives a bee its stinger. It arrives when the commands are applied,
/// after the bee's [`WorkerBeeBrain`].
pub fn give_stinger(commands: &mut Commands, bee: Entity) {
    commands.run_system_cached_with(give_worker_bee_stinger, bee);
}

/// Port of `GiveWorkerBeeStinger` (original/src/Enemies/Enemy_WorkerBee.c).
fn give_worker_bee_stinger(
    In(bee): In<Entity>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut bees: Query<&mut WorkerBeeBrain>,
) {
    let Ok(mut brain) = bees.get_mut(bee) else {
        return;
    };
    let stinger = commands
        .spawn((
            Name::new("Worker bee's stinger"),
            Transform::default(),
            Visibility::default(),
            ChildOf(bee),
        ))
        .id();
    let model = models.spawn(
        &mut commands,
        stinger,
        STINGER_MODEL,
        Shading::Lit,
        Transform::default(),
    );
    if let Some(model) = model {
        commands.entity(model).insert(TransformInterpolation);
    }
    commands.entity(stinger).insert(Stinger {
        model,
        held_at: Vec3::ZERO,
        flying: false,
    });
    brain.stinger = Some(stinger);
}

/// Where a held stinger is in the butt joint's space: scaled, tilted and
/// moved onto the butt.
fn stinger_grip() -> Affine3A {
    Affine3A::from_scale_rotation_translation(
        Vec3::splat(STINGER_SCALE),
        Quat::from_rotation_x(STINGER_TILT),
        STINGER_GRIP,
    )
}

/// Puts each held stinger on its bee's butt.
///
/// Port of `AlignStingerOnBee` (original/src/Enemies/Enemy_WorkerBee.c),
/// which `UpdateWorkerBee` and `MoveWorkerBeeOnSpline` call once the bee
/// has moved. As there, the joints are where the last frame drew them.
pub(super) fn align_stingers(
    bees: Query<(&Transform, &WorkerBeeBrain, &EnemyModel)>,
    models: Query<(&Transform, Option<&SkeletonRig>), Without<Stinger>>,
    mut stingers: Query<(&mut Transform, &mut Stinger), Without<WorkerBeeBrain>>,
) {
    for (bee_transform, brain, model) in &bees {
        let Some(held) = brain.stinger else {
            continue;
        };
        let Ok((mut transform, mut stinger)) = stingers.get_mut(held) else {
            continue;
        };
        let Ok((model_transform, Some(rig))) = models.get(model.0) else {
            continue;
        };
        let Some(joint) = rig.joint_transform(BUTT_JOINT, model_transform.compute_affine()) else {
            continue;
        };
        let local = joint * stinger_grip();
        *transform = Transform::from_matrix(Mat4::from(local));
        stinger.held_at = bee_transform
            .compute_affine()
            .transform_point3(local.translation.into());
    }
}

/// The velocity and heading a stinger is shot with by a bee facing
/// `bee_yaw`: straight ahead of the bee, pointing back at it.
fn launch(bee_yaw: f32) -> (Vec3, f32) {
    let velocity = Vec3::new(
        -bee_yaw.sin() * STINGER_SPEED,
        0.0,
        -bee_yaw.cos() * STINGER_SPEED,
    );
    (velocity, bee_yaw + PI)
}

/// Shoots a bee's stinger from where it was last held: it leaves the bee
/// and hurts the player it hits.
///
/// Port of `ShootStinger` (original/src/Enemies/Enemy_WorkerBee.c). The
/// sparks are made by [`make_shot_sparks`].
pub fn shoot_stinger(
    commands: &mut Commands,
    brain: &mut WorkerBeeBrain,
    stingers: &mut Stingers,
    bee_yaw: f32,
    shots: &mut MessageWriter<StingerShot>,
) {
    let Some(entity) = brain.stinger.take() else {
        return;
    };
    let Ok((mut transform, mut stinger)) = stingers.get_mut(entity) else {
        return;
    };
    let at = stinger.held_at;
    let (velocity, yaw) = launch(bee_yaw);
    stinger.flying = true;
    *transform = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    if let Some(model) = stinger.model {
        commands
            .entity(model)
            .insert(Transform::from_scale(Vec3::splat(FLYING_STINGER_SCALE)));
    }
    commands.entity(entity).remove::<ChildOf>().insert((
        Velocity(velocity),
        PreviousPosition(at),
        TransformInterpolation,
        solid_object(
            vec![STINGER_BOX],
            CollisionKind::HurtMe,
            SolidSides::TOUCHABLE,
        ),
        Damage(STINGER_DAMAGE),
        DespawnOutOfRange,
        DespawnOnExit(AppState::InGame),
    ));
    shots.write(StingerShot { at });
    // Sound: EFFECT_STINGERSHOOT at the stinger.
}

/// The yellow sparks of each shot.
///
/// Port of the sparks in `ShootStinger`
/// (original/src/Enemies/Enemy_WorkerBee.c).
pub(super) fn make_shot_sparks(
    mut shots: MessageReader<StingerShot>,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
) {
    for shot in shots.read() {
        let Some(group) = groups.new_group(SHOT_SPARK_GROUP) else {
            continue;
        };
        for _ in 0..SHOT_SPARKS {
            let velocity = Vec3::new(
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
            ) * SHOT_SPARK_SPEED;
            let scale = random.next_f32() + 1.0;
            groups.add_particle(group, shot.at, velocity, scale, FULL_ALPHA);
        }
    }
}

/// Flies a stinger for `dt` seconds. Returns whether it is still between
/// the floor and the ceiling, which `heights` gives at a point.
fn fly(
    coord: &mut Vec3,
    velocity: &mut Vec3,
    heights: impl Fn(Vec3) -> (f32, f32),
    dt: f32,
) -> bool {
    velocity.y -= STINGER_GRAVITY * dt;
    *coord += *velocity * dt;
    let (floor, ceiling) = heights(*coord);
    coord.y >= floor && coord.y <= ceiling
}

/// Moves the shot stingers; one that hits the floor or the ceiling is
/// gone. Leaving the item window (`TrackTerrainItem`) is
/// `DespawnOutOfRange`.
///
/// Port of `MoveStinger` (original/src/Enemies/Enemy_WorkerBee.c).
pub(super) fn move_stingers(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    mut stingers: Query<(Entity, &mut Transform, &mut Velocity, &Stinger)>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut velocity, stinger) in &mut stingers {
        if !stinger.flying {
            continue;
        }
        let mut coord = transform.translation;
        let inside = fly(
            &mut coord,
            &mut velocity,
            |at| {
                (
                    map.floor_height(at.x, at.z),
                    map.height_at(at.x, at.z, LayerKind::Ceiling).0,
                )
            },
            dt,
        );
        if !inside {
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation = coord;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stinger_is_shot_ahead_of_the_bee_pointing_back() {
        let (velocity, yaw) = launch(0.0);
        assert!((velocity - Vec3::new(0.0, 0.0, -STINGER_SPEED)).length() < 1e-3);
        assert!((yaw - PI).abs() < 1e-6);
    }

    #[test]
    fn a_stinger_falls_slowly_and_stops_at_the_floor_or_ceiling() {
        let mut coord = Vec3::new(0.0, 100.0, 0.0);
        let mut velocity = Vec3::new(0.0, 0.0, -STINGER_SPEED);
        let dt = 0.1;
        assert!(fly(&mut coord, &mut velocity, |_| (0.0, 1000.0), dt));
        assert!((velocity.y + STINGER_GRAVITY * dt).abs() < 1e-4);
        assert!((coord.z + STINGER_SPEED * dt).abs() < 1e-3);
        assert!(!fly(&mut coord, &mut velocity, |_| (200.0, 1000.0), dt));
        assert!(!fly(&mut coord, &mut velocity, |_| (-500.0, 0.0), dt));
    }

    #[test]
    fn the_grip_scales_tilts_and_moves_the_stinger() {
        let grip = stinger_grip();
        assert!((Vec3::from(grip.translation) - STINGER_GRIP).length() < 1e-4);
        let tip = grip.transform_vector3(Vec3::Z);
        assert!((tip.length() - STINGER_SCALE).abs() < 1e-5);
        assert!(tip.y < 0.0, "tilted about x: {tip:?}");
    }
}
