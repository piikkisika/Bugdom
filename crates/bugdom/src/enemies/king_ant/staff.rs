//! The king's staff: held in his hand with a blue flame at its tip, and the
//! fireballs it shoots at the player.
//!
//! Port of `GiveKingAStaff`, `UpdateKingStaff`, `MakeStaffFlame`,
//! `ShootStaff`, `MoveStaffBullet` and `ExplodeStaffBullet`
//! (original/src/Enemies/Enemy_KingAnt.c).
//!
//! The staff is a child of the king, so that it goes with him, as the
//! original's `ChainNode` does. A fireball is an invisible `HurtMe` object
//! whose [`Damage`] the player's own collision takes, trailing blue fire
//! and white sparks. It bursts on the floor, the ceiling, a solid object or
//! the player, into blue sparks that hurt the player
//! ([`ParticleFlags::HURT_PLAYER`]).

use avian3d::prelude::{SpatialQuery, TransformInterpolation};
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::{KING_ANT_SCALE, KingAntBrain};
use crate::collision::{
    CollisionBox, CollisionBoxes, CollisionKind, SolidSides, box_query, solid_object,
};
use crate::combat::Damage;
use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups, ParticleKind,
    ParticleTexture,
};
use crate::math::GameRandom;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, ObjectMaterial, Shading};
use crate::physics::{PreviousPosition, Velocity};
use crate::skeleton::SkeletonRig;
use crate::state::AppState;
use crate::terrain::{LayerKind, TerrainMap};

/// `ANTHILL_MObjType_Staff`
const STAFF_MODEL: ModelRef = ModelRef::new(ModelFile::Level1, 6);
/// The staff's size (`STAFF_SCALE`).
const STAFF_SCALE: f32 = 0.2;
/// The king's joint that holds the staff (`KING_HOLDING_LIMB`).
const HOLDING_JOINT: usize = 14;
/// The staff's tilt about z in the hand, in radians.
const STAFF_TILT: f32 = -0.25;
/// Where the staff sits in the holding joint's space, before its scale.
const STAFF_GRIP: Vec3 = Vec3::new(240.0, -40.0, -130.0);
/// The top of the staff, in the staff's space: where it burns and shoots
/// from.
const STAFF_TIP: Vec3 = Vec3::new(0.0, 370.0, 0.0);

/// Seconds between the staff flame's particles (`FireTimer > .02`).
const STAFF_FLAME_INTERVAL: f32 = 0.02;
/// How far each flame starts from the tip, in units (the whole width).
const STAFF_FLAME_SPREAD: Vec3 = Vec3::new(60.0, 40.0, 60.0);
/// The flames' largest speed in each direction, in units per second (the
/// whole width), and the speed they rise at on average.
const STAFF_FLAME_SPEED: Vec3 = Vec3::new(50.0, 40.0, 50.0);
const STAFF_FLAME_RISE: f32 = 40.0;
/// The smallest flame's scale; they are up to 1 bigger.
const STAFF_FLAME_MIN_SCALE: f32 = 1.7;
/// The blue flame at the staff's tip (`MakeStaffFlame`).
const STAFF_FLAME_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Gravitoids,
    flags: ParticleFlags(ParticleFlags::ROOF.0 | ParticleFlags::HOT.0),
    gravity: 0.0,
    magnetism: 10000.0,
    base_scale: 13.0,
    decay_rate: 0.6,
    fade_rate: 0.0,
    texture: ParticleTexture::BlueFire,
};

/// How fast a fireball flies, in units per second (`BULLET_SPEED`).
const BULLET_SPEED: f32 = 800.0;
/// How far above the player's origin a fireball is aimed, in units.
const BULLET_AIM_RISE: f32 = 50.0;
/// What a fireball does to the player it touches.
const BULLET_DAMAGE: f32 = 0.2;
/// A fireball's collision box, which the player touches
/// (`SetObjectCollisionBounds(newObj, 70, -70, -70, 70, 70, -70)`).
const BULLET_BOX: CollisionBox = CollisionBox::new(70.0, -70.0, -70.0, 70.0, 70.0, -70.0);
/// Half the size of the box a fireball bursts on, in units
/// (`DoSimpleBoxCollision` at ±20).
const BULLET_HIT_REACH: f32 = 20.0;

/// Seconds between bursts of the fireball's trail (`SparkTimer = .03`).
const TRAIL_INTERVAL: f32 = 0.03;
/// The blue fire it leaves behind.
const TRAIL_FIRE_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::HOT,
    gravity: 0.0,
    magnetism: 0.0,
    base_scale: 10.0,
    decay_rate: -10.0,
    fade_rate: 2.0,
    texture: ParticleTexture::BlueFire,
};
/// The trail fire's smallest scale; they are up to 2 bigger.
const TRAIL_FIRE_MIN_SCALE: f32 = 1.0;
/// The white sparks it throws off.
const TRAIL_SPARK_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::HOT,
    gravity: 900.0,
    magnetism: 0.0,
    base_scale: 10.0,
    decay_rate: 1.6,
    fade_rate: 0.0,
    texture: ParticleTexture::White,
};
/// Sparks per burst.
const TRAIL_SPARKS: usize = 3;
/// The sparks' largest speed in each direction, in units per second (the
/// whole width).
const TRAIL_SPARK_SPEED: f32 = 700.0;
/// The sparks' smallest scale; they are up to 2 bigger.
const TRAIL_SPARK_MIN_SCALE: f32 = 2.0;

/// The burst of a fireball (`ExplodeStaffBullet`): it hurts the player.
const BURST_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(
        ParticleFlags::BOUNCE.0
            | ParticleFlags::HURT_PLAYER.0
            | ParticleFlags::ROOF.0
            | ParticleFlags::HOT.0,
    ),
    gravity: 400.0,
    magnetism: 0.0,
    base_scale: 40.0,
    decay_rate: 0.0,
    fade_rate: 0.7,
    texture: ParticleTexture::BlueFire,
};
const BURST_SPARKS: usize = 60;
/// The burst's largest speed in each direction, in units per second (the
/// whole width).
const BURST_SPEED: f32 = 1400.0;
/// The burst's smallest scale; they are up to 1 bigger.
const BURST_MIN_SCALE: f32 = 1.0;

/// The king's staff and its flame, on the king.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct KingStaff {
    /// The staff's entity, a child of the king.
    pub staff: Option<Entity>,
    /// Where the staff was last put, in the world (`staff->Coord`).
    pub origin: Vec3,
    /// Where its tip was last put, in the world.
    pub tip: Vec3,
    /// Seconds since the last flame (the staff's `FireTimer`).
    flame_timer: f32,
    /// The flame's group (the staff's `ParticleGroup`).
    flame_group: Option<ParticleGroupId>,
}

/// The staff's entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Staff;

/// The staff's model, which glows (`STATUS_BIT_GLOW`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaffGlow;

/// A staff mesh that has its glowing material.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlowingStaffPart;

/// A fireball in flight.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct StaffBullet {
    /// Seconds until the next burst of trail (`SparkTimer`).
    spark_timer: f32,
    /// The trail fire's group (`PGroupA`).
    fire_group: Option<ParticleGroupId>,
    /// The trail sparks' group (`PGroupB`).
    spark_group: Option<ParticleGroupId>,
}

/// Gives the king his staff (and the [`KingStaff`] that keeps it). It
/// arrives when the commands are applied.
pub fn give_staff(commands: &mut Commands, king: Entity) {
    commands.entity(king).insert(KingStaff::default());
    commands.run_system_cached_with(give_king_a_staff, king);
}

/// Port of `GiveKingAStaff` (original/src/Enemies/Enemy_KingAnt.c). The
/// staff is drawn glowing, in its own colours and without writing depth
/// (`STATUS_BIT_GLOW | STATUS_BIT_NULLSHADER | STATUS_BIT_NOZWRITE`).
fn give_king_a_staff(
    In(king): In<Entity>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut kings: Query<&mut KingStaff>,
) {
    let Ok(mut held) = kings.get_mut(king) else {
        return;
    };
    let staff = commands
        .spawn((
            Name::new("King ant's staff"),
            Staff,
            Transform::default(),
            Visibility::default(),
            ChildOf(king),
        ))
        .id();
    let model = models.spawn(
        &mut commands,
        staff,
        STAFF_MODEL,
        Shading::Unlit,
        Transform::default(),
    );
    if let Some(model) = model {
        commands
            .entity(model)
            .insert((StaffGlow, TransformInterpolation));
    }
    held.staff = Some(staff);
}

/// Where the staff is in the holding joint's space: its grip, scaled so
/// that the king's own scale doesn't apply, and tilted.
fn staff_grip() -> Affine3A {
    Affine3A::from_scale(Vec3::splat(STAFF_SCALE / KING_ANT_SCALE))
        * Affine3A::from_translation(STAFF_GRIP)
        * Affine3A::from_rotation_z(STAFF_TILT)
}

/// Puts the staff in the king's hand, given his rig and its transform to
/// the world, `base`, and remembers where it and its tip are.
///
/// Port of `UpdateKingStaff` (original/src/Enemies/Enemy_KingAnt.c), but
/// for the flame, which is [`burn_staff`].
pub(super) fn hold_staff(
    held: &mut KingStaff,
    staffs: &mut Query<&mut Transform, (With<Staff>, Without<KingAntBrain>)>,
    rig: &SkeletonRig,
    base: Affine3A,
    king: &Transform,
) {
    let Some(joint) = rig.joint_transform(HOLDING_JOINT, base) else {
        return;
    };
    let world = joint * staff_grip();
    held.origin = world.translation.into();
    held.tip = world.transform_point3(STAFF_TIP);
    if let Some(Ok(mut transform)) = held.staff.map(|s| staffs.get_mut(s)) {
        let local = king.compute_affine().inverse() * world;
        *transform = Transform::from_matrix(Mat4::from(local));
    }
}

/// Keeps the blue flame burning at the tip of a dry king's staff. Water
/// puts it out, and it starts in a new group once dry.
///
/// Port of `MakeStaffFlame` (original/src/Enemies/Enemy_KingAnt.c). Its
/// check for the death animation reads the staff's own skeleton, which it
/// doesn't have, so the flame burns on while the king dies, as here.
pub(super) fn burn_staff(
    held: &mut KingStaff,
    brain: &KingAntBrain,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    dt: f32,
) {
    if brain.is_wet() {
        held.flame_group = None;
        return;
    }
    held.flame_timer += dt;
    if held.flame_timer <= STAFF_FLAME_INTERVAL {
        return;
    }
    if !held.flame_group.is_some_and(|g| groups.is_valid(g)) {
        held.flame_group = groups.new_group(STAFF_FLAME_GROUP);
    }
    // A full group gets a new one and the flame again (`goto new_group`).
    for _ in 0..2 {
        let Some(group) = held.flame_group else {
            return;
        };
        held.flame_timer = 0.0;
        let mut jitter = || {
            Vec3::new(
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
            )
        };
        let point = held.tip + jitter() * STAFF_FLAME_SPREAD;
        let velocity = jitter() * STAFF_FLAME_SPEED + Vec3::Y * STAFF_FLAME_RISE;
        let scale = random.next_f32() + STAFF_FLAME_MIN_SCALE;
        if !groups.add_particle(group, point, velocity, scale, FULL_ALPHA) {
            return;
        }
        held.flame_group = groups.new_group(STAFF_FLAME_GROUP);
    }
}

/// The velocity of a fireball shot from a staff at `origin` toward a
/// player at `player`: at [`BULLET_SPEED`], toward a little above the
/// player.
fn aim(origin: Vec3, player: Vec3) -> Vec3 {
    (player + Vec3::Y * BULLET_AIM_RISE - origin).normalize_or_zero() * BULLET_SPEED
}

/// Shoots a fireball from the staff's tip at the player at `player`, aimed
/// from the staff's origin, both where the staff was last put.
///
/// Port of `ShootStaff` (original/src/Enemies/Enemy_KingAnt.c).
pub(super) fn shoot_staff(commands: &mut Commands, held: &KingStaff, player: Vec3) {
    let at = held.tip;
    commands.spawn((
        Name::new("King ant's fireball"),
        StaffBullet::default(),
        Transform::from_translation(at),
        Visibility::default(),
        PreviousPosition(at),
        Velocity(aim(held.origin, player)),
        solid_object(vec![BULLET_BOX], CollisionKind::HurtMe, SolidSides::TOUCHABLE),
        Damage(BULLET_DAMAGE),
        DespawnOnExit(AppState::InGame),
    ));
    // Sound: EFFECT_KINGSHOOT at the fireball.
}

/// The objects a fireball bursts on.
type BulletTargets<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static CollisionBoxes,
        &'static SolidSides,
    ),
    Without<StaffBullet>,
>;

/// Flies the fireballs, trailing fire and sparks. One that reaches the
/// floor or the ceiling, or touches a solid object or the player, bursts.
///
/// Port of `MoveStaffBullet` (original/src/Enemies/Enemy_KingAnt.c). Like
/// the original, a fireball doesn't track the item window; it flies until
/// it bursts.
pub(super) fn move_staff_bullets(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    spatial: SpatialQuery,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut bullets: Query<(Entity, &mut Transform, &Velocity, &mut StaffBullet)>,
    targets: BulletTargets,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, velocity, mut bullet) in &mut bullets {
        let old = transform.translation;
        let at = old + **velocity * dt;
        let floor = map.floor_height(at.x, at.z);
        let ceiling = map.height_at(at.x, at.z, LayerKind::Ceiling).0;
        let hit_box = CollisionBox::new(
            at.y + BULLET_HIT_REACH,
            at.y - BULLET_HIT_REACH,
            at.x - BULLET_HIT_REACH,
            at.x + BULLET_HIT_REACH,
            at.z + BULLET_HIT_REACH,
            at.z - BULLET_HIT_REACH,
        );
        if at.y <= floor
            || at.y >= ceiling
            || !box_query(
                &spatial,
                &targets,
                hit_box,
                [CollisionKind::Misc, CollisionKind::Player],
            )
            .is_empty()
        {
            // It bursts where it was, as `ExplodeStaffBullet` reads its
            // `Coord`, which the move hasn't updated yet.
            burst(&mut groups, &mut random, old);
            // Sound: EFFECT_KINGEXPLODE at the fireball.
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation = at;
        leave_trail(&mut bullet, &mut groups, &mut random, at, dt);
    }
}

/// Leaves a burst of fire and sparks behind a fireball at `at` now and
/// then. Port of the trail in `MoveStaffBullet`
/// (original/src/Enemies/Enemy_KingAnt.c). The sparks' group is never
/// checked again once made, so the sparks stop if it goes, as there.
fn leave_trail(
    bullet: &mut StaffBullet,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    at: Vec3,
    dt: f32,
) {
    bullet.spark_timer -= dt;
    if bullet.spark_timer > 0.0 {
        return;
    }
    bullet.spark_timer = TRAIL_INTERVAL;

    if !bullet.fire_group.is_some_and(|g| groups.is_valid(g)) {
        bullet.fire_group = groups.new_group(TRAIL_FIRE_GROUP);
    }
    if let Some(group) = bullet.fire_group {
        let scale = random.next_f32() * 2.0 + TRAIL_FIRE_MIN_SCALE;
        groups.add_particle(group, at, Vec3::ZERO, scale, FULL_ALPHA);
    }

    if bullet.spark_group.is_none() {
        bullet.spark_group = groups.new_group(TRAIL_SPARK_GROUP);
    }
    if let Some(group) = bullet.spark_group {
        for _ in 0..TRAIL_SPARKS {
            let velocity = Vec3::new(
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
            ) * TRAIL_SPARK_SPEED;
            let scale = random.next_f32() * 2.0 + TRAIL_SPARK_MIN_SCALE;
            groups.add_particle(group, at, velocity, scale, FULL_ALPHA);
        }
    }
}

/// The blue sparks a fireball bursts into at `at`, which hurt the player.
/// Port of `ExplodeStaffBullet` (original/src/Enemies/Enemy_KingAnt.c).
fn burst(groups: &mut ParticleGroups, random: &mut GameRandom, at: Vec3) {
    let Some(group) = groups.new_group(BURST_GROUP) else {
        return;
    };
    for _ in 0..BURST_SPARKS {
        let velocity = Vec3::new(
            random.next_f32() - 0.5,
            random.next_f32() - 0.5,
            random.next_f32() - 0.5,
        ) * BURST_SPEED;
        let scale = random.next_f32() + BURST_MIN_SCALE;
        groups.add_particle(group, at, velocity, scale, FULL_ALPHA);
    }
}

/// Makes the staff's meshes glow once its model has been spawned: they add
/// to what is behind them (`STATUS_BIT_GLOW`), which also writes no depth
/// (`STATUS_BIT_NOZWRITE`). The meshes get materials of their own.
///
/// Port of the status bits `GiveKingAStaff` sets
/// (original/src/Enemies/Enemy_KingAnt.c).
pub(super) fn make_staffs_glow(
    mut commands: Commands,
    staffs: Query<Entity, With<StaffGlow>>,
    children: Query<&Children>,
    mut meshes: Query<&mut MeshMaterial3d<ObjectMaterial>, Without<GlowingStaffPart>>,
    mut materials: ResMut<Assets<ObjectMaterial>>,
) {
    for staff in &staffs {
        for part in children.iter_descendants(staff) {
            let Ok(mut handle) = meshes.get_mut(part) else {
                continue;
            };
            let Some(mut material) = materials.get(&handle.0).cloned() else {
                continue;
            };
            material.base.alpha_mode = AlphaMode::Add;
            handle.0 = materials.add(material);
            commands.entity(part).insert(GlowingStaffPart);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn the_staff_is_held_small_and_tilted() {
        let grip = staff_grip();
        let scale = STAFF_SCALE / KING_ANT_SCALE;
        let origin = Vec3::from(grip.translation);
        assert!((origin - STAFF_GRIP * scale).length() < 1e-3);
        let up = grip.transform_vector3(Vec3::Y);
        assert!((up.length() - scale).abs() < 1e-5);
        // Tilted toward +x by the negative turn about z.
        assert!(up.x > 0.0 && up.y > 0.0, "{up:?}");
    }

    #[test]
    fn a_fireball_is_aimed_above_the_player_at_its_speed() {
        let velocity = aim(Vec3::new(0.0, 50.0, 0.0), Vec3::new(0.0, 0.0, -1000.0));
        assert!((velocity.length() - BULLET_SPEED).abs() < 1e-3);
        assert!(velocity.y.abs() < 1e-3 && velocity.z < 0.0);
    }

    #[test]
    fn the_staff_flame_burns_until_the_king_is_wet() {
        let mut held = KingStaff {
            tip: Vec3::new(0.0, 500.0, 0.0),
            ..default()
        };
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let dry = KingAntBrain::default();
        burn_staff(&mut held, &dry, &mut groups, &mut random, 0.015);
        assert_eq!(held.flame_group, None, "not due yet");
        burn_staff(&mut held, &dry, &mut groups, &mut random, 0.015);
        let first = held.flame_group;
        let count = first.and_then(|g| groups.get(g)).map(|g| g.particles().len());
        assert_eq!(count, Some(1));

        let wet = KingAntBrain {
            wet_timer: 1.0,
            ..default()
        };
        burn_staff(&mut held, &wet, &mut groups, &mut random, 0.05);
        assert_eq!(held.flame_group, None);
        burn_staff(&mut held, &dry, &mut groups, &mut random, 0.05);
        assert!(held.flame_group.is_some() && held.flame_group != first);
    }

    #[test]
    fn a_full_staff_flame_moves_to_a_new_group() {
        let mut held = KingStaff::default();
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let dry = KingAntBrain::default();
        burn_staff(&mut held, &dry, &mut groups, &mut random, 0.05);
        let first = held.flame_group;
        for _ in 0..crate::effects::MAX_PARTICLES {
            burn_staff(&mut held, &dry, &mut groups, &mut random, 0.05);
        }
        assert_ne!(held.flame_group, first);
        let count = held
            .flame_group
            .and_then(|g| groups.get(g))
            .map(|g| g.particles().len());
        assert_eq!(count, Some(1));
    }

    #[test]
    fn a_fireball_trails_fire_and_sparks_now_and_then() {
        let mut bullet = StaffBullet::default();
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let at = Vec3::new(10.0, 20.0, 30.0);
        leave_trail(&mut bullet, &mut groups, &mut random, at, DT);
        let fire = bullet.fire_group.and_then(|g| groups.get(g));
        assert_eq!(fire.map(|g| g.particles().len()), Some(1));
        let sparks = bullet.spark_group.and_then(|g| groups.get(g));
        assert_eq!(sparks.map(|g| g.particles().len()), Some(TRAIL_SPARKS));
        // Not again until the interval has passed.
        leave_trail(&mut bullet, &mut groups, &mut random, at, DT);
        let fire = bullet.fire_group.and_then(|g| groups.get(g));
        assert_eq!(fire.map(|g| g.particles().len()), Some(1));
        leave_trail(&mut bullet, &mut groups, &mut random, at, DT);
        let fire = bullet.fire_group.and_then(|g| groups.get(g));
        assert_eq!(fire.map(|g| g.particles().len()), Some(2));
    }

    #[test]
    fn a_fireball_bursts_into_hurting_sparks() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        burst(&mut groups, &mut random, Vec3::ZERO);
        let (_, group) = groups.iter().next().expect("a burst");
        assert_eq!(group.particles().len(), BURST_SPARKS);
        assert!(group.desc.flags.contains(ParticleFlags::HURT_PLAYER));
    }
}
