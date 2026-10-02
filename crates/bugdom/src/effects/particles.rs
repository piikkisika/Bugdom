//! Particle groups: the simulation and the bookkeeping, with no rendering.
//!
//! Port of the particle half of original/src/Items/Effects.c
//! (`NewParticleGroup`, `AddParticleToGroup`, `MoveParticleGroups`,
//! `VerifyParticleGroup` and `ParticleHitObject`).

use bevy::prelude::*;

use crate::collision::CollisionBox;
use crate::terrain::{LayerKind, TerrainMap};

/// How many groups can exist at once (`MAX_PARTICLE_GROUPS`).
pub const MAX_PARTICLE_GROUPS: usize = 50;
/// How many particles a group holds (`MAX_PARTICLES`).
pub const MAX_PARTICLES: usize = 200;
/// A particle's alpha when it starts fully opaque (`FULL_ALPHA`).
pub const FULL_ALPHA: f32 = 1.0;

/// How far above the floor a bouncing particle stays.
const BOUNCE_FLOOR_CLEARANCE: f32 = 10.0;
/// How much of its downward speed a particle keeps when it bounces, reversed.
const BOUNCE_RESTITUTION: f32 = -0.4;
/// The sideways kick, in units per second, along the floor's slope when a
/// particle bounces.
const BOUNCE_SLOPE_KICK: f32 = 300.0;
/// How far below the ceiling a [`ParticleFlags::ROOF`] particle stays.
const ROOF_CLEARANCE: f32 = 10.0;
/// The sideways kick, in units per second, when a particle hits the ceiling.
const ROOF_SLOPE_KICK: f32 = 1000.0;
/// Particles fainter than this don't hit objects (`ParticleHitObject`).
pub const HIT_MIN_ALPHA: f32 = 0.4;
/// Half the size of a particle's box when it hits an object
/// (`ParticleHitObject`).
pub const HIT_OBJECT_REACH: f32 = 40.0;
/// Half the size of a particle's box when it hurts the player
/// (`MoveParticleGroups`).
pub const HURT_PLAYER_REACH: f32 = 30.0;

/// How a group's particles move (`PARTICLE_TYPE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ParticleKind {
    /// Each particle flies on its own (`PARTICLE_TYPE_FALLINGSPARKS`).
    #[default]
    Sparks,
    /// Every particle pulls on every other (`PARTICLE_TYPE_GRAVITOIDS`).
    Gravitoids,
}

/// What a group's particles do besides moving (`PARTICLE_FLAGS_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ParticleFlags(pub u8);

impl ParticleFlags {
    pub const NONE: Self = Self(0);
    /// Bounces off the floor.
    pub const BOUNCE: Self = Self(1 << 0);
    /// Hurts the player it touches.
    pub const HURT_PLAYER: Self = Self(1 << 1);
    /// With [`Self::HURT_PLAYER`], hurts enough to kill and sets the player
    /// on fire.
    pub const HURT_PLAYER_BAD: Self = Self(1 << 2);
    /// Hurts enemies.
    pub const HURT_ENEMY: Self = Self(1 << 3);
    /// Stops at the ceiling.
    pub const ROOF: Self = Self(1 << 4);
    /// Puts out fires.
    pub const EXTINGUISH: Self = Self(1 << 5);
    /// Sets things on fire.
    pub const HOT: Self = Self(1 << 6);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for ParticleFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for ParticleFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// The particle textures, `Images/Textures/130.tga` to `137.tga` in this
/// order (`PARTICLE_TEXTURE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ParticleTexture {
    #[default]
    Fire,
    White,
    Patchy,
    Dots,
    OrangeSpot,
    BlueFire,
    GreenRing,
    YellowBall,
}

impl ParticleTexture {
    pub const ALL: [Self; 8] = [
        Self::Fire,
        Self::White,
        Self::Patchy,
        Self::Dots,
        Self::OrangeSpot,
        Self::BlueFire,
        Self::GreenRing,
        Self::YellowBall,
    ];

    /// The texture's file in the original data.
    pub fn path(self) -> String {
        format!("Images/Textures/{}.tga", 130 + self as u32)
    }
}

/// How a new group behaves: the arguments of `NewParticleGroup`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ParticleGroupDesc {
    pub kind: ParticleKind,
    pub flags: ParticleFlags,
    /// Downward acceleration, in units per second².
    pub gravity: f32,
    /// How strongly gravitoids pull on each other.
    pub magnetism: f32,
    /// Half the width of a particle of scale 1, in units.
    pub base_scale: f32,
    /// How fast particles shrink, in scale per second. Negative grows them.
    pub decay_rate: f32,
    /// How fast particles fade, in alpha per second.
    pub fade_rate: f32,
    pub texture: ParticleTexture,
}

/// A handle to a particle group. It stops being valid when its group is
/// freed, even if the slot is reused (the original's magic number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParticleGroupId {
    slot: u8,
    generation: u32,
}

impl ParticleGroupId {
    /// The group's slot, `0..MAX_PARTICLE_GROUPS`.
    pub fn slot(self) -> usize {
        usize::from(self.slot)
    }
}

/// One particle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub position: Vec3,
    /// Units per second.
    pub velocity: Vec3,
    /// Multiplies the group's base scale.
    pub scale: f32,
    pub alpha: f32,
}

/// A live group.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleGroup {
    pub desc: ParticleGroupDesc,
    /// In the order they were added, as the original's pool lists them.
    particles: Vec<Particle>,
}

impl ParticleGroup {
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }
}

#[derive(Debug, Clone, Default)]
struct Slot {
    generation: u32,
    group: Option<ParticleGroup>,
}

/// Every particle group of the level (`gParticleGroups`).
#[derive(Resource, Debug, Clone)]
pub struct ParticleGroups {
    slots: Vec<Slot>,
    /// The generation the next group gets (`gParticleGroupMagicAllocator`).
    next_generation: u32,
}

impl Default for ParticleGroups {
    fn default() -> Self {
        Self {
            slots: vec![Slot::default(); MAX_PARTICLE_GROUPS],
            next_generation: 0,
        }
    }
}

impl ParticleGroups {
    /// Starts a group, or returns `None` if all slots are taken.
    /// Port of `NewParticleGroup`.
    pub fn new_group(&mut self, desc: ParticleGroupDesc) -> Option<ParticleGroupId> {
        let slot = self.slots.iter().position(|s| s.group.is_none())?;
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1);
        self.slots[slot] = Slot {
            generation,
            group: Some(ParticleGroup {
                desc,
                particles: Vec::with_capacity(MAX_PARTICLES),
            }),
        };
        Some(ParticleGroupId {
            slot: slot as u8,
            generation,
        })
    }

    /// Whether the group still exists. Port of `VerifyParticleGroup`.
    pub fn is_valid(&self, id: ParticleGroupId) -> bool {
        self.get(id).is_some()
    }

    pub fn get(&self, id: ParticleGroupId) -> Option<&ParticleGroup> {
        let slot = self.slots.get(id.slot())?;
        if slot.generation != id.generation {
            return None;
        }
        slot.group.as_ref()
    }

    /// Adds a particle. Returns **true if it could not**, because the group
    /// is gone or full, so emitters can stop or start a new group, exactly
    /// like `AddParticleToGroup`.
    pub fn add_particle(
        &mut self,
        id: ParticleGroupId,
        position: Vec3,
        velocity: Vec3,
        scale: f32,
        alpha: f32,
    ) -> bool {
        let Some(slot) = self.slots.get_mut(id.slot()) else {
            return true;
        };
        if slot.generation != id.generation {
            return true;
        }
        let Some(group) = slot.group.as_mut() else {
            return true;
        };
        if group.particles.len() >= MAX_PARTICLES {
            return true;
        }
        group.particles.push(Particle {
            position,
            velocity,
            scale,
            alpha,
        });
        false
    }

    /// The live groups with their handles.
    pub fn iter(&self) -> impl Iterator<Item = (ParticleGroupId, &ParticleGroup)> {
        self.slots.iter().enumerate().filter_map(|(i, slot)| {
            let group = slot.group.as_ref()?;
            Some((
                ParticleGroupId {
                    slot: i as u8,
                    generation: slot.generation,
                },
                group,
            ))
        })
    }

    /// Frees every group (`DeleteAllParticleGroups`).
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.group = None;
        }
    }

    /// Moves every particle by `dt` seconds, bounces it off the terrain,
    /// shrinks and fades it, and removes it once it is gone. A group that
    /// had no particles at the start of the step is freed.
    ///
    /// Port of `MoveParticleGroups`, in per-second units. Player damage
    /// is not done here; see [`Self::particle_hits_box`].
    pub fn step(&mut self, dt: f32, terrain: Option<&TerrainMap>) {
        for slot in &mut self.slots {
            let Some(group) = slot.group.as_mut() else {
                continue;
            };
            if group.particles.is_empty() {
                slot.group = None;
                continue;
            }
            step_group(group, dt, terrain);
        }
    }

    /// The flags of the first group that has `flags` (any of them; all
    /// groups if `flags` is empty) and a particle at least `min_alpha`
    /// opaque whose box of half size `reach` touches `world_box`.
    ///
    /// This is the box test of both `ParticleHitObject` (with
    /// [`HIT_OBJECT_REACH`] and [`HIT_MIN_ALPHA`]) and the player check in
    /// `MoveParticleGroups` (with [`ParticleFlags::HURT_PLAYER`],
    /// [`HURT_PLAYER_REACH`] and no alpha limit).
    pub fn particle_hits_box(
        &self,
        world_box: &CollisionBox,
        flags: ParticleFlags,
        reach: f32,
        min_alpha: f32,
    ) -> Option<ParticleFlags> {
        self.iter()
            .map(|(_, group)| group)
            .filter(|group| flags.is_empty() || flags.intersects(group.desc.flags))
            .find(|group| {
                group.particles.iter().any(|particle| {
                    particle.alpha >= min_alpha
                        && particle_box(particle.position, reach).overlaps(world_box)
                })
            })
            .map(|group| group.desc.flags)
    }
}

/// A cube of half size `reach` around a particle.
fn particle_box(position: Vec3, reach: f32) -> CollisionBox {
    CollisionBox::new(
        position.y + reach,
        position.y - reach,
        position.x - reach,
        position.x + reach,
        position.z + reach,
        position.z - reach,
    )
}

/// Whether a particle of a group with any of `flags` (any group if `flags`
/// is empty) touches one of an object's boxes. `boxes` are relative to
/// `position`.
///
/// Port of `ParticleHitObject`.
pub fn particle_hit(
    groups: &ParticleGroups,
    boxes: &[CollisionBox],
    position: Vec3,
    flags: ParticleFlags,
) -> bool {
    boxes.iter().any(|b| {
        groups
            .particle_hits_box(&b.at(position), flags, HIT_OBJECT_REACH, HIT_MIN_ALPHA)
            .is_some()
    })
}

fn step_group(group: &mut ParticleGroup, dt: f32, terrain: Option<&TerrainMap>) {
    let desc = group.desc;
    let particles = &mut group.particles;
    let count = particles.len();

    // The original fills a 1/distance² table as it goes, reading each pair
    // once both are known. Each pair is first met before either particle
    // has moved this step, so taking the table from the start positions
    // gives the same numbers.
    let pull = if desc.kind == ParticleKind::Gravitoids {
        let max_pull = 1.0 / (desc.base_scale * desc.base_scale);
        let mut table = vec![0.0f32; count * count];
        for p in 0..count {
            for q in p + 1..count {
                let distance = particles[p].position.distance(particles[q].position);
                let mut inverse_square = if distance != 0.0 {
                    1.0 / (distance * distance)
                } else {
                    0.0
                };
                // Closer than a particle's radius pulls no harder.
                if inverse_square > max_pull {
                    inverse_square = max_pull;
                }
                table[p * count + q] = inverse_square;
                table[q * count + p] = inverse_square;
            }
        }
        table
    } else {
        Vec::new()
    };

    let mut alive = vec![true; count];
    for p in 0..count {
        let mut particle = particles[p];
        particle.velocity.y -= desc.gravity * dt;

        if desc.kind == ParticleKind::Gravitoids {
            // Particles removed earlier in this step no longer pull, as
            // they have left the original's pool.
            for q in (0..count).rev() {
                if q == p || !alive[q] {
                    continue;
                }
                let inverse_square = pull[p * count + q];
                let toward = if inverse_square != 0.0 {
                    fast_normalize(particles[q].position - particle.position)
                } else {
                    Vec3::ZERO
                };
                particle.velocity += toward * (inverse_square * desc.magnetism * dt);
            }
        }
        particle.position += particle.velocity * dt;

        if let Some(map) = terrain {
            if desc.flags.contains(ParticleFlags::BOUNCE) && particle.velocity.y < 0.0 {
                let (floor, normal) =
                    map.height_at(particle.position.x, particle.position.z, LayerKind::Floor);
                let floor = floor + BOUNCE_FLOOR_CLEARANCE;
                if particle.position.y < floor {
                    particle.position.y = floor;
                    particle.velocity.y *= BOUNCE_RESTITUTION;
                    particle.velocity.x += normal.x * BOUNCE_SLOPE_KICK;
                    particle.velocity.z += normal.z * BOUNCE_SLOPE_KICK;
                }
            }

            // HURT_PLAYER: the original hurts the player here, once per
            // touching particle per frame (`PlayerGotHurt` with 0.15, 0.1 as
            // the ball, or a kill and `gTorchPlayer` with HURT_PLAYER_BAD).
            // Player damage lives with the player: it calls
            // `ParticleGroups::particle_hits_box(player_box,
            // ParticleFlags::HURT_PLAYER, HURT_PLAYER_REACH, 0.0)` after
            // `EffectsSystems::MoveParticles`.

            if map.ceiling.is_some()
                && desc.flags.contains(ParticleFlags::ROOF)
                && particle.velocity.y > 0.0
            {
                let (ceiling, _) =
                    map.height_at(particle.position.x, particle.position.z, LayerKind::Ceiling);
                let ceiling = ceiling - ROOF_CLEARANCE;
                if particle.position.y > ceiling {
                    particle.position.y = ceiling;
                    // The original kicks along the *floor's* normal here
                    // (`gRecentTerrainNormal[FLOOR]`, left over from the last
                    // floor query); the floor under the particle is the
                    // closest stand-in.
                    let (_, normal) =
                        map.height_at(particle.position.x, particle.position.z, LayerKind::Floor);
                    particle.velocity.x += normal.x * ROOF_SLOPE_KICK;
                    particle.velocity.z += normal.z * ROOF_SLOPE_KICK;
                }
            }
        }

        particle.scale -= desc.decay_rate * dt;
        if particle.scale <= 0.0 {
            alive[p] = false;
        } else {
            particle.alpha -= desc.fade_rate * dt;
            if particle.alpha <= 0.0 {
                alive[p] = false;
            }
        }
        particles[p] = particle;
    }

    let mut index = 0;
    particles.retain(|_| {
        let keep = alive[index];
        index += 1;
        keep
    });
}

/// Port of `FastNormalizeVector`.
fn fast_normalize(v: Vec3) -> Vec3 {
    if v == Vec3::ZERO {
        return Vec3::ZERO;
    }
    v / (v.length() + f32::MIN_POSITIVE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sparks(gravity: f32, decay_rate: f32, fade_rate: f32) -> ParticleGroupDesc {
        ParticleGroupDesc {
            kind: ParticleKind::Sparks,
            gravity,
            base_scale: 20.0,
            decay_rate,
            fade_rate,
            ..default()
        }
    }

    #[test]
    fn gravity_accelerates_in_units_per_second() {
        let mut groups = ParticleGroups::default();
        let id = groups.new_group(sparks(800.0, 0.0, 0.0)).unwrap();
        assert!(!groups.add_particle(id, Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0), 1.0, 1.0));
        for _ in 0..60 {
            groups.step(1.0 / 60.0, None);
        }
        let p = groups.get(id).unwrap().particles()[0];
        assert!((p.velocity.y + 800.0).abs() < 1e-2, "{:?}", p.velocity);
        assert!((p.position.x - 100.0).abs() < 1e-2);
        // Semi-implicit Euler, as the original: velocity first, then position.
        let expected_y = -800.0 / 60.0 / 60.0 * (60.0 * 61.0 / 2.0);
        assert!((p.position.y - expected_y).abs() < 1e-1, "{}", p.position.y);
    }

    #[test]
    fn particles_go_when_shrunk_or_faded_and_then_the_group_goes() {
        let mut groups = ParticleGroups::default();
        let shrink = groups.new_group(sparks(0.0, 1.0, 0.0)).unwrap();
        let fade = groups.new_group(sparks(0.0, 0.0, 2.0)).unwrap();
        groups.add_particle(shrink, Vec3::ZERO, Vec3::ZERO, 0.5, 1.0);
        groups.add_particle(shrink, Vec3::ZERO, Vec3::ZERO, 2.0, 1.0);
        groups.add_particle(fade, Vec3::ZERO, Vec3::ZERO, 1.0, 1.0);

        groups.step(0.25, None);
        assert_eq!(groups.get(shrink).unwrap().particles().len(), 2);
        assert!((groups.get(fade).unwrap().particles()[0].alpha - 0.5).abs() < 1e-6);

        groups.step(0.25, None);
        let left = groups.get(shrink).unwrap().particles();
        assert_eq!(left.len(), 1);
        assert!((left[0].scale - 1.5).abs() < 1e-6);
        // Faded out, but the group lives until the step after it empties.
        assert!(groups.get(fade).unwrap().particles().is_empty());

        groups.step(0.25, None);
        assert!(!groups.is_valid(fade));
        assert!(groups.is_valid(shrink));
    }

    #[test]
    fn handles_die_with_their_group_even_when_the_slot_is_reused() {
        let mut groups = ParticleGroups::default();
        let old = groups.new_group(sparks(0.0, 0.0, 0.0)).unwrap();
        // Empty, so the next step frees it.
        groups.step(0.1, None);
        assert!(!groups.is_valid(old));
        assert!(groups.add_particle(old, Vec3::ZERO, Vec3::ZERO, 1.0, 1.0));

        let new = groups.new_group(sparks(0.0, 0.0, 0.0)).unwrap();
        assert_eq!(new.slot(), old.slot());
        assert!(groups.is_valid(new));
        assert!(!groups.is_valid(old));
        assert!(groups.add_particle(old, Vec3::ZERO, Vec3::ZERO, 1.0, 1.0));
        assert!(!groups.add_particle(new, Vec3::ZERO, Vec3::ZERO, 1.0, 1.0));
        assert_eq!(groups.get(new).unwrap().particles().len(), 1);

        groups.clear();
        assert!(!groups.is_valid(new));
    }

    #[test]
    fn full_groups_and_full_slots_refuse() {
        let mut groups = ParticleGroups::default();
        let id = groups.new_group(sparks(0.0, 0.0, 0.0)).unwrap();
        for _ in 0..MAX_PARTICLES {
            assert!(!groups.add_particle(id, Vec3::ZERO, Vec3::ZERO, 1.0, 1.0));
        }
        assert!(groups.add_particle(id, Vec3::ZERO, Vec3::ZERO, 1.0, 1.0));

        for _ in 1..MAX_PARTICLE_GROUPS {
            assert!(groups.new_group(sparks(0.0, 0.0, 0.0)).is_some());
        }
        assert!(groups.new_group(sparks(0.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn gravitoids_pull_together_no_harder_than_at_their_radius() {
        let mut groups = ParticleGroups::default();
        let desc = ParticleGroupDesc {
            kind: ParticleKind::Gravitoids,
            magnetism: 1000.0,
            base_scale: 10.0,
            ..default()
        };
        let far = groups.new_group(desc).unwrap();
        groups.add_particle(far, Vec3::ZERO, Vec3::ZERO, 1.0, 1.0);
        groups.add_particle(far, Vec3::new(100.0, 0.0, 0.0), Vec3::ZERO, 1.0, 1.0);
        let near = groups.new_group(desc).unwrap();
        groups.add_particle(near, Vec3::ZERO, Vec3::ZERO, 1.0, 1.0);
        groups.add_particle(near, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO, 1.0, 1.0);

        let dt = 0.01;
        groups.step(dt, None);
        let far = groups.get(far).unwrap().particles();
        // 1000 / 100² per second toward each other.
        assert!((far[0].velocity.x - 0.1 * dt).abs() < 1e-6, "{:?}", far[0]);
        assert!((far[1].velocity.x + 0.1 * dt).abs() < 1e-6, "{:?}", far[1]);
        let near = groups.get(near).unwrap().particles();
        // Clamped to 1 / base_scale²: 1000 / 10² per second.
        assert!(
            (near[0].velocity.x - 10.0 * dt).abs() < 1e-4,
            "{:?}",
            near[0]
        );
    }

    #[test]
    fn bouncing_particles_stay_above_the_floor() {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let (x, z) = (3000.0, 3000.0);
        let floor = map.floor_height(x, z);
        let mut groups = ParticleGroups::default();
        let id = groups
            .new_group(ParticleGroupDesc {
                flags: ParticleFlags::BOUNCE,
                ..sparks(800.0, 0.0, 0.0)
            })
            .unwrap();
        groups.add_particle(
            id,
            Vec3::new(x, floor + 20.0, z),
            Vec3::new(0.0, -600.0, 0.0),
            1.0,
            1.0,
        );
        groups.step(1.0 / 30.0, Some(&map));
        let p = groups.get(id).unwrap().particles()[0];
        assert_eq!(p.position.y, floor + BOUNCE_FLOOR_CLEARANCE);
        assert!(p.velocity.y > 0.0);
    }

    #[test]
    fn hits_need_matching_flags_and_enough_alpha() {
        let mut groups = ParticleGroups::default();
        let hot = groups
            .new_group(ParticleGroupDesc {
                flags: ParticleFlags::HOT | ParticleFlags::HURT_ENEMY,
                ..sparks(0.0, 0.0, 0.0)
            })
            .unwrap();
        groups.add_particle(hot, Vec3::new(89.0, 0.0, 0.0), Vec3::ZERO, 1.0, 1.0);
        let faint = groups.new_group(sparks(0.0, 0.0, 0.0)).unwrap();
        groups.add_particle(faint, Vec3::new(-100.0, 0.0, 0.0), Vec3::ZERO, 1.0, 0.3);

        let boxes = [CollisionBox::new(50.0, -50.0, -50.0, 50.0, 50.0, -50.0)];
        // The particle's box starts at 89 - 40 = 49, just inside the box's
        // right side at 50; touching counts.
        assert!(particle_hit(
            &groups,
            &boxes,
            Vec3::ZERO,
            ParticleFlags::HOT
        ));
        assert!(particle_hit(
            &groups,
            &boxes,
            Vec3::new(-1.0, 0.0, 0.0),
            ParticleFlags::HOT
        ));
        assert!(!particle_hit(
            &groups,
            &boxes,
            Vec3::new(-2.0, 0.0, 0.0),
            ParticleFlags::HOT
        ));
        assert!(!particle_hit(
            &groups,
            &boxes,
            Vec3::ZERO,
            ParticleFlags::EXTINGUISH
        ));
        assert!(particle_hit(
            &groups,
            &boxes,
            Vec3::ZERO,
            ParticleFlags::NONE
        ));
        // The faint particle doesn't count.
        assert!(!particle_hit(
            &groups,
            &boxes,
            Vec3::new(-60.0, 0.0, 0.0),
            ParticleFlags::NONE
        ));
        assert_eq!(
            groups.particle_hits_box(
                &boxes[0].at(Vec3::new(-60.0, 0.0, 0.0)),
                ParticleFlags::NONE,
                HURT_PLAYER_REACH,
                0.0
            ),
            Some(ParticleFlags::NONE)
        );
    }
}
