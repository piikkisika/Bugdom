//! What the player leaves behind: splashes and ripples in water, the nitro
//! trail's rings and the flames of a burning bug.
//!
//! Port of the effect calls in original/src/Player (Player_Control.c,
//! Player_Bug.c and Player_Ball.c). The movement decides when an effect is
//! due; this module makes it.

use std::f32::consts::TAU;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::effects::{
    FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroupId, ParticleGroups, ParticleKind,
    ParticleTexture, RippleMaker, make_ripple, make_splash,
};
use crate::math::GameRandom;

/// What the player's effects need.
#[derive(SystemParam)]
pub(super) struct PlayerEffects<'w> {
    groups: ResMut<'w, ParticleGroups>,
    random: ResMut<'w, GameRandom>,
    ripples: RippleMaker<'w>,
}

/// A splash a player's move threw up (the arguments of `MakeSplash`). It is
/// made once the move is over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Splash {
    pub position: Vec3,
    pub force: f32,
    pub volume: f32,
}

impl PlayerEffects<'_> {
    pub fn splash(&mut self, splash: Splash) {
        make_splash(
            &mut self.groups,
            &mut self.random,
            splash.position,
            splash.force,
            splash.volume,
        );
    }

    pub fn ripple(&mut self, commands: &mut Commands, position: Vec3, start_scale: f32) {
        make_ripple(commands, &mut self.ripples, position, start_scale);
    }

    pub fn torch(&mut self, fire: &mut TorchFire, pelvis: Option<Vec3>, dt: f32) {
        torch_player(fire, &mut self.groups, &mut self.random, pelvis, dt);
    }

    pub fn nitro_trail(&mut self, trail: &mut NitroTrail, from: Vec3, to: Vec3, dt: f32) {
        leave_nitro_trail(trail, &mut self.groups, &mut self.random, from, to, dt);
    }
}

/// Adds a batch of particles to a group that lives across ticks, starting
/// the group if there is none. `add` returns true when a particle could not
/// be added, as [`ParticleGroups::add_particle`] does; the original then
/// starts a new group and adds the whole batch again (`goto new_pgroup`).
fn emit_into(
    groups: &mut ParticleGroups,
    group: &mut Option<ParticleGroupId>,
    desc: ParticleGroupDesc,
    mut add: impl FnMut(&mut ParticleGroups, ParticleGroupId) -> bool,
) {
    loop {
        let (id, fresh) = match *group {
            Some(id) => (id, false),
            None => {
                let Some(id) = groups.new_group(desc) else {
                    return;
                };
                *group = Some(id);
                (id, true)
            }
        };
        // A new group holds more than any batch, so this only guards
        // against looping forever.
        if !add(groups, id) || fresh {
            return;
        }
        *group = None;
    }
}

/// Seconds between the swimming bug's ripples.
pub(super) const SWIM_RIPPLE_INTERVAL: f32 = 0.25;
/// The swimming bug's ripples' starting scale.
pub(super) const SWIM_RIPPLE_SCALE: f32 = 3.0;

/// Seconds since the swimming bug last left a ripple (`RippleTimer`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct SwimRipple(pub f32);

/// Seconds between bursts of a burning bug's flames.
const TORCH_INTERVAL: f32 = 0.04;
/// Flames per burst.
const TORCH_PARTICLES: usize = 3;
/// How far each flame wanders from the last, in units (the whole width).
const TORCH_SPREAD: Vec3 = Vec3::new(50.0, 30.0, 50.0);
/// The flames' largest sideways speed, in units per second (the whole
/// width), and their largest upward speed.
const TORCH_SIDEWAYS_SPEED: f32 = 100.0;
const TORCH_RISE: f32 = 50.0;
/// The smallest flame's scale; they are up to 1 bigger.
const TORCH_MIN_SCALE: f32 = 2.1;

/// The burning bug's flames (`TorchPlayer`).
const TORCH_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::HOT,
    gravity: -100.0,
    magnetism: 9000.0,
    base_scale: 25.0,
    decay_rate: 0.1,
    fade_rate: 0.7,
    texture: ParticleTexture::Fire,
};

/// The flames of a burning player: the time since the last burst
/// (`gTorchTimer`) and the group they go in (`ParticleGroup`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct TorchFire {
    timer: f32,
    group: Option<ParticleGroupId>,
}

/// Spews flames from the pelvis now and then. Port of `TorchPlayer`
/// (original/src/Player/Player_Bug.c). `pelvis` is `None` if the model has
/// no skeleton yet; the burst is skipped then.
fn torch_player(
    fire: &mut TorchFire,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    pelvis: Option<Vec3>,
    dt: f32,
) {
    fire.timer += dt;
    if fire.timer <= TORCH_INTERVAL {
        return;
    }
    fire.timer = 0.0;
    let Some(pelvis) = pelvis else {
        return;
    };
    emit_into(groups, &mut fire.group, TORCH_GROUP, |groups, id| {
        // Each flame starts where the last one did, give or take.
        let mut point = pelvis;
        for _ in 0..TORCH_PARTICLES {
            point += Vec3::new(
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
                random.next_f32() - 0.5,
            ) * TORCH_SPREAD;
            let velocity = Vec3::new(
                (random.next_f32() - 0.5) * TORCH_SIDEWAYS_SPEED,
                random.next_f32() * TORCH_RISE,
                (random.next_f32() - 0.5) * TORCH_SIDEWAYS_SPEED,
            );
            let scale = random.next_f32() + TORCH_MIN_SCALE;
            if groups.add_particle(id, point, velocity, scale, FULL_ALPHA) {
                return true;
            }
        }
        false
    });
}

/// Seconds between the nitro trail's rings.
const NITRO_RING_INTERVAL: f32 = 0.05;
/// Particles in a ring (`NITRO_RING_SIZE`).
const NITRO_RING_SIZE: usize = 16;
/// A ring's radius, in units.
const NITRO_RING_RADIUS: f32 = 50.0;
/// The rings' particles' largest speed in each direction, in units per
/// second (the whole width), and the upward speed they have on average.
const NITRO_SPEED: f32 = 200.0;
const NITRO_RISE: f32 = 200.0;

/// The nitro trail's green rings (`LeaveNitroTrail`).
const NITRO_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::NONE,
    gravity: 600.0,
    magnetism: 10000.0,
    base_scale: 25.0,
    decay_rate: -1.9,
    fade_rate: 1.0,
    texture: ParticleTexture::GreenRing,
};

/// The nitro trail: the time since the last ring (`gNitroTrailTick`) and
/// the group the rings go in (`gNitroParticleGroup`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct NitroTrail {
    tick: f32,
    group: Option<ParticleGroupId>,
}

impl NitroTrail {
    /// A boost starts the trail over (`StartNitroTrail`). The original's
    /// group was already let go when the last boost ran out or the ball
    /// was made.
    pub fn start(&mut self) {
        *self = Self::default();
    }

    /// The boost has run out; the next one starts a new group.
    pub fn end(&mut self) {
        self.group = None;
    }
}

/// Point `i` of a ring around `from`, upright and facing along the motion
/// from `from` to `to`.
///
/// `SetLookAtMatrixAndTranslate` with the world's up as the up vector. Its
/// sideways axis is up × the look direction, which isn't normalised, so the
/// ring narrows as the motion steepens.
fn nitro_ring_point(from: Vec3, to: Vec3, i: usize) -> Vec3 {
    let look = (from - to).normalize_or_zero();
    let sideways = Vec3::Y.cross(look);
    let angle = TAU * i as f32 / NITRO_RING_SIZE as f32;
    from + (sideways * angle.sin() + Vec3::Y * angle.cos()) * NITRO_RING_RADIUS
}

/// Leaves a ring of green sparks where the ball was, now and then. Port of
/// `LeaveNitroTrail` (original/src/Player/Player_Ball.c); `from` is where
/// the ball started the tick (`OldCoord`) and `to` where it is now.
fn leave_nitro_trail(
    trail: &mut NitroTrail,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    from: Vec3,
    to: Vec3,
    dt: f32,
) {
    trail.tick += dt;
    if trail.tick < NITRO_RING_INTERVAL {
        return;
    }
    trail.tick = 0.0;
    emit_into(groups, &mut trail.group, NITRO_GROUP, |groups, id| {
        for i in 0..NITRO_RING_SIZE {
            let point = nitro_ring_point(from, to, i);
            let velocity = Vec3::new(
                (random.next_f32() - 0.5) * NITRO_SPEED,
                NITRO_RISE + (random.next_f32() - 0.5) * NITRO_SPEED,
                (random.next_f32() - 0.5) * NITRO_SPEED,
            );
            let scale = 1.0 + random.next_f32();
            if groups.add_particle(id, point, velocity, scale, FULL_ALPHA) {
                return true;
            }
        }
        false
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::MAX_PARTICLES;

    #[test]
    fn a_nitro_ring_stands_across_the_motion() {
        let from = Vec3::new(100.0, 0.0, 0.0);
        let to = Vec3::new(200.0, 0.0, 0.0);
        for i in 0..NITRO_RING_SIZE {
            let p = nitro_ring_point(from, to, i);
            assert!((p.x - from.x).abs() < 1e-3);
            assert!((p.distance(from) - NITRO_RING_RADIUS).abs() < 1e-3);
        }
        assert!((nitro_ring_point(from, to, 0).y - NITRO_RING_RADIUS).abs() < 1e-3);
    }

    #[test]
    fn the_nitro_trail_leaves_a_ring_every_twentieth_of_a_second() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut trail = NitroTrail::default();
        let (from, to) = (Vec3::ZERO, Vec3::X);
        leave_nitro_trail(&mut trail, &mut groups, &mut random, from, to, 0.03);
        assert_eq!(groups.iter().count(), 0);
        leave_nitro_trail(&mut trail, &mut groups, &mut random, from, to, 0.03);
        let group = trail.group.and_then(|id| groups.get(id)).unwrap();
        assert_eq!(group.particles().len(), NITRO_RING_SIZE);
        assert_eq!(group.desc.texture, ParticleTexture::GreenRing);
    }

    #[test]
    fn a_full_group_is_replaced_and_the_batch_added_again() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut trail = NitroTrail::default();
        let rings = MAX_PARTICLES / NITRO_RING_SIZE + 1;
        for _ in 0..rings {
            leave_nitro_trail(
                &mut trail,
                &mut groups,
                &mut random,
                Vec3::ZERO,
                Vec3::X,
                0.06,
            );
        }
        // The first group filled up part-way through the last ring; that
        // ring went whole into a second group.
        let mut counts: Vec<_> = groups.iter().map(|(_, g)| g.particles().len()).collect();
        counts.sort();
        assert_eq!(counts, vec![NITRO_RING_SIZE, MAX_PARTICLES]);
    }

    #[test]
    fn a_burning_bug_spews_flames_from_its_pelvis() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        let mut fire = TorchFire::default();
        let pelvis = Vec3::new(10.0, 20.0, 30.0);
        torch_player(&mut fire, &mut groups, &mut random, Some(pelvis), 0.03);
        assert!(fire.group.is_none());
        torch_player(&mut fire, &mut groups, &mut random, Some(pelvis), 0.03);
        let group = fire.group.and_then(|id| groups.get(id)).unwrap();
        assert_eq!(group.particles().len(), TORCH_PARTICLES);
        assert!(group.desc.flags.contains(ParticleFlags::HOT));
        for p in group.particles() {
            assert!(p.position.distance(pelvis) < TORCH_SPREAD.length() * 1.5);
            assert!(p.velocity.y >= 0.0);
        }
    }
}
