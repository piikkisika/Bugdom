//! The fireballs the dragonfly breathes.
//!
//! Port of `DragonFlyShootFireball`, `MoveFireball` and `ExplodeFireball`
//! (original/src/Ride/DragonFly.c). A fireball has no model: it is a box
//! that hurts the enemies it touches (`CTYPE_HURTENEMY`, which the enemies'
//! own collision answers) and leaves a trail of fire and sparks. It bursts
//! on the floor and on solid objects, and rattles the stump's hive when it
//! hits it.

use avian3d::prelude::SpatialQuery;
use bevy::prelude::*;

use super::DRAGONFLY_MAX_SPEED;
use crate::collision::{
    CollisionBox, CollisionBoxes, CollisionKind, CollisionSystems, SolidSides, box_query,
    solid_object,
};
use crate::combat::Damage;
use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups, ParticleKind,
    ParticleTexture,
};
use crate::items::{Hive, ItemSystems, RattleHive};
use crate::math::GameRandom;
use crate::physics::{PreviousPosition, Velocity};
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        // Moved before the collision is gathered, so that the enemies'
        // collision sees where it is.
        move_fireballs
            .after(ItemSystems::Track)
            .before(CollisionSystems::Gather)
            .run_if(in_state(AppState::InGame)),
    );
}

/// How fast a fireball flies, in units per second.
const FIREBALL_SPEED: f32 = DRAGONFLY_MAX_SPEED * 4.3;
/// How long it burns, in seconds (`Health = 1.8`).
const FIREBALL_LIFE: f32 = 1.8;
/// What it does to an enemy it touches (`Damage = 2.0`, "massive damage").
const FIREBALL_DAMAGE: f32 = 2.0;
/// Its box (`SetObjectCollisionBounds(newObj, 70, -70, -70, 70, 70, -70)`).
const FIREBALL_BOX: CollisionBox = CollisionBox::new(70.0, -70.0, -70.0, 70.0, 70.0, -70.0);
/// Half the size of the box it tests against solid objects, in units.
const HIT_REACH: f32 = 20.0;

/// How often its trail leaves fire and sparks, in seconds.
const TRAIL_INTERVAL: f32 = 0.02;
/// How many sparks each puff of the trail throws, and their largest speed
/// in each direction, in units per second (the whole width).
const TRAIL_SPARKS: usize = 3;
const TRAIL_SPARK_SPEED: f32 = 900.0;
/// How many sparks its burst throws, and their largest speed.
const BURST_SPARKS: usize = 60;
const BURST_SPARK_SPEED: f32 = 1400.0;

/// The fire of its trail.
const TRAIL_FIRE: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::HOT,
    gravity: 0.0,
    magnetism: 0.0,
    base_scale: 20.0,
    decay_rate: -10.0,
    fade_rate: 2.0,
    texture: ParticleTexture::BlueFire,
};
/// The sparks of its trail.
const TRAIL_SPARKS_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::HOT,
    gravity: 900.0,
    magnetism: 0.0,
    base_scale: 15.0,
    decay_rate: 1.6,
    fade_rate: 0.0,
    texture: ParticleTexture::OrangeSpot,
};
/// The sparks of its burst.
const BURST_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(ParticleFlags::BOUNCE.0 | ParticleFlags::HOT.0),
    gravity: 400.0,
    magnetism: 0.0,
    base_scale: 40.0,
    decay_rate: 0.0,
    fade_rate: 0.7,
    texture: ParticleTexture::BlueFire,
};

/// A fireball in flight.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Fireball {
    /// Seconds before it burns out (`Health`).
    pub life: f32,
    /// Seconds until its trail leaves the next puff (`SparkTimer`).
    pub trail_timer: f32,
    /// Its trail's fire and sparks (`PGroupA`, `PGroupB`).
    pub fire: Option<ParticleGroupId>,
    pub sparks: Option<ParticleGroupId>,
}

impl Fireball {
    fn new() -> Self {
        Self {
            life: FIREBALL_LIFE,
            trail_timer: 0.0,
            fire: None,
            sparks: None,
        }
    }

    /// Ages it by `dt` seconds. Returns false once it has burned out.
    fn age(&mut self, dt: f32) -> bool {
        self.life -= dt;
        self.life > 0.0
    }

    /// Counts the trail's timer down and returns whether to leave a puff
    /// now.
    fn trail_due(&mut self, dt: f32) -> bool {
        self.trail_timer -= dt;
        if self.trail_timer <= 0.0 {
            self.trail_timer = TRAIL_INTERVAL;
            true
        } else {
            false
        }
    }
}

/// A fireball's velocity: along the dragonfly's motion, at the fireball's
/// speed.
fn fireball_velocity(dragonfly_velocity: Vec3) -> Vec3 {
    dragonfly_velocity.normalize_or_zero() * FIREBALL_SPEED
}

/// Breathes a fireball from the dragonfly's mouth, in the direction it
/// flies.
///
/// Port of `DragonFlyShootFireball` (original/src/Ride/DragonFly.c).
pub(super) fn shoot(commands: &mut Commands, mouth: Vec3, dragonfly_velocity: Vec3) {
    commands.spawn((
        Name::new("Fireball"),
        Fireball::new(),
        Transform::from_translation(mouth),
        PreviousPosition(mouth),
        Velocity(fireball_velocity(dragonfly_velocity)),
        solid_object(
            vec![FIREBALL_BOX],
            CollisionKind::HurtEnemy,
            SolidSides::TOUCHABLE,
        ),
        Damage(FIREBALL_DAMAGE),
        DespawnOnExit(AppState::InGame),
    ));
    // Sound: EFFECT_PLASMABURST.
}

/// The box a fireball tests against solid objects.
fn hit_box(at: Vec3) -> CollisionBox {
    CollisionBox::new(
        at.y + HIT_REACH,
        at.y - HIT_REACH,
        at.x - HIT_REACH,
        at.x + HIT_REACH,
        at.z + HIT_REACH,
        at.z - HIT_REACH,
    )
}

/// A random velocity of up to half `speed` in each direction.
fn random_spread(random: &mut GameRandom, speed: f32) -> Vec3 {
    Vec3::new(
        (random.next_f32() - 0.5) * speed,
        (random.next_f32() - 0.5) * speed,
        (random.next_f32() - 0.5) * speed,
    )
}

/// Bursts a fireball into sparks where it was.
///
/// Port of `ExplodeFireball` (original/src/Ride/DragonFly.c).
fn explode(particles: &mut ParticleGroups, random: &mut GameRandom, at: Vec3) {
    // Sound: EFFECT_PLASMAEXPLODE at the fireball (kMiddleC-4, volume 6).
    let Some(group) = particles.new_group(BURST_GROUP) else {
        return;
    };
    for _ in 0..BURST_SPARKS {
        let velocity = random_spread(random, BURST_SPARK_SPEED);
        let scale = random.next_f32() + 1.0;
        particles.add_particle(group, at, velocity, scale, FULL_ALPHA);
    }
}

/// Leaves a puff of the trail: a flame and a few sparks, each in its own
/// group, started again when the old one has gone.
fn leave_trail(
    fireball: &mut Fireball,
    particles: &mut ParticleGroups,
    random: &mut GameRandom,
    at: Vec3,
) {
    if !fireball.fire.is_some_and(|g| particles.is_valid(g)) {
        fireball.fire = particles.new_group(TRAIL_FIRE);
    }
    if let Some(group) = fireball.fire {
        let scale = random.next_f32() * 2.0 + 1.0;
        particles.add_particle(group, at, Vec3::ZERO, scale, FULL_ALPHA);
    }
    if !fireball.sparks.is_some_and(|g| particles.is_valid(g)) {
        fireball.sparks = particles.new_group(TRAIL_SPARKS_GROUP);
    }
    if let Some(group) = fireball.sparks {
        for _ in 0..TRAIL_SPARKS {
            let velocity = random_spread(random, TRAIL_SPARK_SPEED);
            let scale = random.next_f32() * 2.0 + 2.0;
            particles.add_particle(group, at, velocity, scale, FULL_ALPHA);
        }
    }
}

/// Flies the fireballs: they burn out, burst on the floor and on solid
/// objects (rattling the hive if that is what they hit), and leave a trail.
///
/// Port of `MoveFireball` (original/src/Ride/DragonFly.c).
#[allow(clippy::too_many_arguments)]
fn move_fireballs(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    spatial: SpatialQuery,
    mut particles: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut rattles: MessageWriter<RattleHive>,
    mut fireballs: Query<(Entity, &mut Fireball, &mut Transform, &Velocity)>,
    targets: Query<(&Transform, &CollisionBoxes, &SolidSides), Without<Fireball>>,
    hives: Query<(), With<Hive>>,
) {
    let dt = time.delta_secs();
    for (entity, mut fireball, mut transform, velocity) in &mut fireballs {
        if !fireball.age(dt) {
            commands.entity(entity).despawn();
            continue;
        }
        let old = transform.translation;
        let coord = old + **velocity * dt;

        // It bursts where it was, as the original's `ExplodeFireball` reads
        // the object's coordinate before the move is written back.
        if coord.y <= map.floor_height(coord.x, coord.z) {
            explode(&mut particles, &mut random, old);
            commands.entity(entity).despawn();
            continue;
        }
        let hits = box_query(&spatial, &targets, hit_box(coord), CollisionKind::Misc);
        if let Some(hit) = hits.first() {
            explode(&mut particles, &mut random, old);
            commands.entity(entity).despawn();
            if hives.contains(hit.entity) {
                rattles.write(RattleHive { hive: hit.entity });
            }
            continue;
        }

        transform.translation = coord;
        if fireball.trail_due(dt) {
            leave_trail(&mut fireball, &mut particles, &mut random, coord);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn a_fireball_burns_for_its_life() {
        let mut fireball = Fireball::new();
        let ticks = (1..1000).find(|_| !fireball.age(DT)).expect("it burns out");
        // 1.8 seconds, give or take a tick of rounding.
        assert!((108..=109).contains(&ticks), "{ticks}");
    }

    #[test]
    fn the_trail_puffs_every_other_tick_at_sixty_hertz() {
        let mut fireball = Fireball::new();
        let puffs: Vec<bool> = (0..6).map(|_| fireball.trail_due(DT)).collect();
        assert_eq!(puffs, [true, false, true, false, true, false]);
    }

    #[test]
    fn a_fireball_flies_along_the_dragonflys_motion() {
        let v = fireball_velocity(Vec3::new(0.0, 0.0, -10.0));
        assert!(v.abs_diff_eq(Vec3::new(0.0, 0.0, -FIREBALL_SPEED), 1e-2));
        assert_eq!(fireball_velocity(Vec3::ZERO), Vec3::ZERO);
    }

    #[test]
    fn a_trail_puff_starts_its_groups_once_and_then_adds_to_them() {
        let mut particles = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut fireball = Fireball::new();
        leave_trail(&mut fireball, &mut particles, &mut random, Vec3::ZERO);
        let (fire, sparks) = (fireball.fire.unwrap(), fireball.sparks.unwrap());
        leave_trail(&mut fireball, &mut particles, &mut random, Vec3::ONE);
        assert_eq!(fireball.fire, Some(fire));
        assert_eq!(particles.get(fire).unwrap().particles().len(), 2);
        assert_eq!(particles.get(sparks).unwrap().particles().len(), 6);
    }

    #[test]
    fn a_burst_throws_its_sparks() {
        let mut particles = ParticleGroups::default();
        let mut random = GameRandom::default();
        explode(&mut particles, &mut random, Vec3::ZERO);
        let (_, group) = particles.iter().next().expect("a group");
        assert_eq!(group.particles().len(), BURST_SPARKS);
        assert_eq!(group.desc, BURST_GROUP);
    }
}
