//! Collision against the terrain.
//!
//! Port of the terrain parts of original/src/System/Collision.c. Object
//! collision (box side detection) joins this module with the collision
//! framework.

use bevy::prelude::*;

use crate::terrain::{LayerKind, TerrainMap};

/// Below this length a vector counts as zero (`EPS`).
const EPSILON: f32 = 1e-5;
/// Height used for the ceiling where a level has none.
const NO_CEILING_Y: f32 = 1_000_000.0;
/// How much of a vertical speed survives bouncing off the floor or ceiling.
const BOUNCE_FACTOR: f32 = -0.3;

/// The result of [`collide_floor_and_ceiling`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloorContact {
    /// The feet are on (or were pushed up onto) the floor.
    pub on_ground: bool,
    /// The normal of the floor triangle sampled last
    /// (`gRecentTerrainNormal[FLOOR]`).
    pub floor_normal: Vec3,
}

/// Which surface was hit first, before testing again (`WH_FOOT`, `WH_HEAD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstHit {
    Foot,
    Head,
}

/// Keeps a moving object between the floor and the ceiling.
///
/// `foot_offset` and `head_offset` are the distances from the object's
/// origin down to its feet and up to its head; `speed` is its speed before
/// the move, which an uphill move keeps; `dt` is the length of this move in
/// seconds.
///
/// Port of `HandleFloorAndCeilingCollision` (original/src/System/Collision.c).
/// When the feet go under the floor, the object is lifted onto it and
/// "rolls" uphill at its old speed rather than bouncing physically.
pub fn collide_floor_and_ceiling(
    map: &TerrainMap,
    coord: &mut Vec3,
    old_coord: Vec3,
    delta: &mut Vec3,
    old_delta: Vec3,
    foot_offset: f32,
    head_offset: f32,
    speed: f32,
    dt: f32,
) -> FloorContact {
    let has_ceiling = map.ceiling.is_some();
    let mut first_hit = None;
    let mut floor_normal;
    let mut ceiling_normal = Vec3::NEG_Y;

    loop {
        let (floor_y, normal) = map.height_at(coord.x, coord.z, LayerKind::Floor);
        floor_normal = normal;
        let ceiling_y = if has_ceiling {
            let (y, normal) = map.height_at(coord.x, coord.z, LayerKind::Ceiling);
            ceiling_normal = normal;
            y.max(floor_y)
        } else {
            NO_CEILING_Y
        };
        let dist_to_floor = (coord.y - foot_offset) - floor_y;
        let dist_to_ceiling = ceiling_y - (coord.y + head_offset);
        let hit_head = dist_to_ceiling <= 0.0;
        let hit_foot = dist_to_floor <= 0.0;

        let wedged = (hit_foot && hit_head)
            || (hit_head && first_hit == Some(FirstHit::Foot))
            || (hit_foot && !hit_head && first_hit == Some(FirstHit::Head));
        if wedged {
            // Both ends hit: go back to where it was safe last time, and
            // reflect the old motion off the average of the two surfaces.
            *coord = old_coord;
            let wall = (floor_normal + ceiling_normal).normalize_or_zero();
            let incoming = old_delta.normalize_or_zero();
            *delta = (wall + (wall + incoming)).normalize_or_zero() * speed;
            // The original samples both layers again here, which updates the
            // floor normal.
            floor_normal = map.height_at(coord.x, coord.z, LayerKind::Floor).1;
            return FloorContact {
                on_ground: true,
                floor_normal,
            };
        }

        if hit_head {
            coord.y += dist_to_ceiling;
            if delta.y > 0.0 {
                delta.y *= BOUNCE_FACTOR;
            }
            if first_hit.is_none() {
                first_hit = Some(FirstHit::Head);
                continue;
            }
            return FloorContact {
                on_ground: false,
                floor_normal,
            };
        }

        if hit_foot {
            coord.y = floor_y + foot_offset;
            if coord.y > old_coord.y {
                // Went uphill: point the motion along the climb, at the old
                // speed.
                delta.y = (coord.y - old_coord.y) / dt;
                let length = delta.length();
                let scale = if length < EPSILON {
                    0.0
                } else {
                    speed / length
                };
                *delta *= scale;
            }
            if delta.y < 0.0 {
                delta.y *= BOUNCE_FACTOR;
            }
            if first_hit.is_none() && has_ceiling {
                first_hit = Some(FirstHit::Foot);
                continue;
            }
            return FloorContact {
                on_ground: true,
                floor_normal,
            };
        }

        return FloorContact {
            on_ground: false,
            floor_normal,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feet_below_the_floor_are_lifted_onto_it() {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let (x, z) = (12720.0, 15780.0);
        let floor = map.floor_height(x, z);
        let old = Vec3::new(x, floor + 10.0, z);
        let mut coord = Vec3::new(x, floor - 20.0, z);
        let mut delta = Vec3::new(0.0, -1800.0, 0.0);
        let contact = collide_floor_and_ceiling(
            &map,
            &mut coord,
            old,
            &mut delta,
            Vec3::new(0.0, -1800.0, 0.0),
            0.0,
            180.0,
            0.0,
            1.0 / 60.0,
        );
        assert!(contact.on_ground);
        assert_eq!(coord.y, floor);
        // Moving down onto the floor, so the fall bounces back up a little.
        assert_eq!(delta.y, 1800.0 * 0.3);
        assert!(contact.floor_normal.y > 0.0);
    }

    #[test]
    fn in_the_air_nothing_changes() {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let (x, z) = (12720.0, 15780.0);
        let floor = map.floor_height(x, z);
        let mut coord = Vec3::new(x, floor + 100.0, z);
        let mut delta = Vec3::new(10.0, 50.0, 0.0);
        let contact = collide_floor_and_ceiling(
            &map,
            &mut coord,
            Vec3::new(x, floor + 100.0, z),
            &mut delta,
            Vec3::new(10.0, 50.0, 0.0),
            0.0,
            180.0,
            10.0,
            1.0 / 60.0,
        );
        assert!(!contact.on_ground);
        assert_eq!(delta, Vec3::new(10.0, 50.0, 0.0));
    }
}
