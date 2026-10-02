//! Visual effects: particle groups, ripples and splashes.
//!
//! Port of the core of original/src/Items/Effects.c. The effects that
//! belong to one enemy or item (fire, gas, sparks of a checkpoint, …) are
//! made by their owners from these pieces.

mod particles;
mod render;
mod ripple;

use bevy::prelude::*;
use bevy::transform::TransformSystems;

pub use particles::{
    FULL_ALPHA, HIT_MIN_ALPHA, HIT_OBJECT_REACH, HURT_PLAYER_REACH, MAX_PARTICLE_GROUPS,
    MAX_PARTICLES, Particle, ParticleFlags, ParticleGroup, ParticleGroupDesc, ParticleGroupId,
    ParticleGroups, ParticleKind, ParticleTexture, particle_hit,
};
pub use ripple::{Ripple, RippleMaker, make_ripple};

use crate::math::GameRandom;
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ParticleGroups>()
            .add_systems(Startup, render::load_particle_materials)
            .add_systems(OnExit(AppState::InGame), clear_particle_groups)
            .add_systems(
                FixedUpdate,
                (
                    move_particle_groups.in_set(EffectsSystems::MoveParticles),
                    ripple::move_ripples,
                )
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(Update, render::clamp_particle_textures)
            .add_systems(
                PostUpdate,
                render::draw_particle_groups
                    .after(TransformSystems::Propagate)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum EffectsSystems {
    /// Moves the particles and frees spent groups each fixed tick. Systems
    /// that test particles against objects run after it.
    MoveParticles,
}

/// Port of `MoveParticleGroups`.
fn move_particle_groups(
    time: Res<Time>,
    terrain: Option<Res<TerrainMap>>,
    mut groups: ResMut<ParticleGroups>,
) {
    groups.step(time.delta_secs(), terrain.as_deref());
}

/// Port of `DeleteAllParticleGroups` as `CleanupLevel` calls it.
fn clear_particle_groups(mut groups: ResMut<ParticleGroups>) {
    groups.clear();
}

/// How many particles a splash of force 1 throws.
const SPLASH_PARTICLES_PER_FORCE: f32 = 30.0;
/// How wide the splash starts, in units at force 1.
const SPLASH_SPREAD: f32 = 130.0;
/// The largest sideways speed at force 1, in units per second.
const SPLASH_SIDEWAYS_SPEED: f32 = 400.0;
/// The upward speed every drop has, in units per second.
const SPLASH_BASE_RISE: f32 = 500.0;
/// The extra upward speed at force 1, in units per second.
const SPLASH_EXTRA_RISE: f32 = 300.0;

/// The white drops of a splash (`MakeSplash`).
const SPLASH_GROUP: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags::NONE,
    gravity: 800.0,
    magnetism: 0.0,
    base_scale: 40.0,
    // Negative: the drops grow as they fade.
    decay_rate: -0.6,
    fade_rate: 0.8,
    texture: ParticleTexture::Patchy,
};

/// Throws up white drops from a liquid's surface at `position`. `force`
/// scales how many and how far; `volume` is the splash sound's volume.
/// Port of `MakeSplash`.
pub fn make_splash(
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
    position: Vec3,
    force: f32,
    volume: f32,
) {
    let count = (SPLASH_PARTICLES_PER_FORCE * force) as i32;
    if let Some(group) = groups.new_group(SPLASH_GROUP) {
        for _ in 0..count {
            let x = position.x + (random.next_f32() - 0.5) * SPLASH_SPREAD * force;
            let z = position.z + (random.next_f32() - 0.5) * SPLASH_SPREAD * force;
            let velocity = Vec3::new(
                (random.next_f32() - 0.5) * SPLASH_SIDEWAYS_SPEED * force,
                SPLASH_BASE_RISE + random.next_f32() * SPLASH_EXTRA_RISE * force,
                (random.next_f32() - 0.5) * SPLASH_SIDEWAYS_SPEED * force,
            );
            let scale = random.next_f32() + 1.0;
            groups.add_particle(
                group,
                Vec3::new(x, position.y, z),
                velocity,
                scale,
                FULL_ALPHA,
            );
        }
    }
    // Sound: EFFECT_SPLASH at `position`, middle C, with `volume`.
    let _ = volume;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_splash_throws_its_drops_up() {
        let mut groups = ParticleGroups::default();
        let mut random = GameRandom::default();
        make_splash(
            &mut groups,
            &mut random,
            Vec3::new(100.0, 50.0, 200.0),
            1.5,
            1.0,
        );
        let (_, group) = groups.iter().next().unwrap();
        assert_eq!(group.particles().len(), 45);
        assert_eq!(group.desc.texture, ParticleTexture::Patchy);
        for p in group.particles() {
            assert_eq!(p.position.y, 50.0);
            assert!((p.position.x - 100.0).abs() <= 0.5 * SPLASH_SPREAD * 1.5);
            assert!(p.velocity.y >= SPLASH_BASE_RISE);
            assert!((1.0..=2.0).contains(&p.scale));
        }
    }
}
