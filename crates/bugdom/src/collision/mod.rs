//! Collision: objects against objects (box side detection), against the
//! terrain, and against fences.
//!
//! Port of original/src/System/Collision.c. Avian only stores the colliders
//! and finds candidates; the original's own logic decides which sides were
//! hit and how to push out (docs/design/phase2-engine-core.md §4).

mod boxes;
mod terrain;

use avian3d::prelude::*;
use bevy::ecs::query::QueryFilter;
use bevy::math::bounding::BoundingVolume;
use bevy::prelude::*;

pub use boxes::{
    BoxCollisions, BoxMover, BoxTarget, CollisionBox, CollisionBoxes, CollisionHit, CollisionKind,
    SolidSides, Trigger, TriggerHit, resolve_box_collisions,
};
pub use terrain::{FloorContact, collide_floor_and_ceiling};

use crate::physics::{PreviousPosition, Velocity};

pub struct CollisionPlugin;

impl Plugin for CollisionPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<TriggerHit>().add_systems(
            FixedUpdate,
            gather_candidates.in_set(CollisionSystems::Gather),
        );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum CollisionSystems {
    /// Collects each mover's [`CollisionCandidates`] before anything moves.
    Gather,
}

/// The solid objects near a mover, collected once per tick for its box
/// collision (`gFirstNodePtr` scanned by `CollisionDetect`, narrowed down by
/// avian's broad phase).
#[derive(Component, Debug, Clone, Default)]
pub struct CollisionCandidates(pub Vec<BoxTarget>);

/// How much further than its own motion a mover looks for candidates, in
/// world units. Collision pushes can move it a little beyond its motion.
const CANDIDATE_MARGIN: f32 = 100.0;

/// Fills in [`CollisionCandidates`] for every mover from the colliders around
/// where it can get to this tick.
fn gather_candidates(
    time: Res<Time>,
    spatial: SpatialQuery,
    mut movers: Query<(
        Entity,
        &Transform,
        &CollisionBoxes,
        Option<&Velocity>,
        &mut CollisionCandidates,
    )>,
    targets: Query<(
        &Transform,
        &CollisionBoxes,
        &SolidSides,
        &CollisionLayers,
        Option<&PreviousPosition>,
        Option<&Velocity>,
        Option<&Trigger>,
    )>,
) {
    let dt = time.delta_secs();
    for (entity, transform, boxes, velocity, mut candidates) in &mut movers {
        candidates.0.clear();
        let Some(bounds) = boxes.bounds() else {
            continue;
        };
        let reach = velocity.map_or(0.0, |v| v.length() * dt) * 2.0 + CANDIDATE_MARGIN;
        let position = transform.translation;
        let half = Vec3::from(bounds.half_size()) + Vec3::splat(reach);
        let center = position + Vec3::from(bounds.center());
        let shape = Collider::cuboid(half.x * 2.0, half.y * 2.0, half.z * 2.0);
        let filter = SpatialQueryFilter::default().with_excluded_entities([entity]);
        for hit in spatial.shape_intersections(&shape, center, Quat::IDENTITY, &filter) {
            let Ok((transform, boxes, solid, layers, previous, velocity, trigger)) =
                targets.get(hit)
            else {
                continue;
            };
            let now = transform.translation;
            let before = previous.map_or(now, |p| **p);
            candidates.0.push(BoxTarget {
                entity: hit,
                kinds: layers.memberships,
                solid: *solid,
                boxes: boxes.0.iter().map(|b| b.at(now)).collect(),
                old_boxes: boxes.0.iter().map(|b| b.at(before)).collect(),
                velocity: velocity.map_or(Vec3::ZERO, |v| **v),
                trigger: trigger.copied(),
            });
        }
        // The original scans objects in list order; entity order is the
        // closest stable equivalent.
        candidates.0.sort_by_key(|t| t.entity);
    }
}

/// One object box that a [`box_query`] area overlaps (`CollisionRec`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxQueryHit {
    pub entity: Entity,
    /// Which of the entity's [`CollisionBoxes`] it is.
    pub target_box: usize,
    /// That box, in world space.
    pub world_box: CollisionBox,
}

/// The smallest size of the broad-phase shape, in world units, so that a
/// flat or inside-out area still finds what it touches.
const MIN_QUERY_SIZE: f32 = 1.0;

/// Every box of the objects of `kinds` that overlaps `area`, a world-space
/// box, ordered by entity and then box. Objects without solid or touchable
/// sides are skipped. `targets` supplies the objects' boxes; its filter
/// lets the caller keep it apart from its own queries.
///
/// Port of `DoSimpleBoxCollision` (original/src/System/Collision.c), with
/// avian's broad phase standing in for the scan of every object. It
/// returns the collision list (`gCollisionList`) rather than its length.
pub fn box_query<F: QueryFilter>(
    spatial: &SpatialQuery,
    targets: &Query<(&Transform, &CollisionBoxes, &SolidSides), F>,
    area: CollisionBox,
    kinds: impl Into<LayerMask>,
) -> Vec<BoxQueryHit> {
    let (min, max) = area.min_max();
    let size = (max - min).max(Vec3::splat(MIN_QUERY_SIZE));
    let shape = Collider::cuboid(size.x, size.y, size.z);
    let filter = SpatialQueryFilter::from_mask(kinds);
    let mut entities =
        spatial.shape_intersections(&shape, (min + max) / 2.0, Quat::IDENTITY, &filter);
    // The original scans objects in list order; entity order is the closest
    // stable equivalent.
    entities.sort();
    let mut hits = Vec::new();
    for entity in entities {
        let Ok((transform, boxes, solid)) = targets.get(entity) else {
            continue;
        };
        if solid.is_empty() {
            continue;
        }
        let position = transform.translation;
        hits.extend(
            boxes
                .0
                .iter()
                .enumerate()
                .map(|(target_box, b)| (target_box, b.at(position)))
                .filter(|(_, world_box)| area.overlaps(world_box))
                .map(|(target_box, world_box)| BoxQueryHit {
                    entity,
                    target_box,
                    world_box,
                }),
        );
    }
    hits
}

/// The components a solid object needs: its boxes and solid sides, its
/// collision kinds, and an avian collider covering its boxes for the broad
/// phase. `boxes` are relative to the entity's position, which must be on
/// an entity with no parent and unit scale.
pub fn solid_object(
    boxes: Vec<CollisionBox>,
    kinds: impl Into<LayerMask>,
    solid: SolidSides,
) -> impl Bundle {
    let boxes = CollisionBoxes(boxes);
    let collider = boxes.collider();
    (
        collider,
        CollisionLayers::new(kinds, LayerMask::NONE),
        solid,
        boxes,
    )
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::physics::PhysicsPlugin;

    #[test]
    fn box_query_finds_the_overlapping_boxes_of_the_chosen_kinds() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::mesh::MeshPlugin,
            PhysicsPlugin,
        ));
        let cube = |size: f32| CollisionBox::new(size, -size, -size, size, size, -size);
        let mut spawn = |at: Vec3, boxes: Vec<CollisionBox>, kind: CollisionKind, solid| {
            app.world_mut()
                .spawn((
                    Transform::from_translation(at),
                    solid_object(boxes, kind, solid),
                ))
                .id()
        };
        let near = spawn(
            Vec3::new(50.0, 0.0, 0.0),
            vec![cube(10.0), cube(10.0).at(Vec3::X * 500.0)],
            CollisionKind::Misc,
            SolidSides::ALL,
        );
        // Each of these is left out for one reason.
        spawn(
            Vec3::ZERO,
            vec![cube(10.0)],
            CollisionKind::Enemy,
            SolidSides::ALL,
        );
        spawn(
            Vec3::X * 200.0,
            vec![cube(10.0)],
            CollisionKind::Misc,
            SolidSides::ALL,
        );
        spawn(
            Vec3::ZERO,
            vec![cube(10.0)],
            CollisionKind::Misc,
            SolidSides::NONE,
        );
        app.finish();
        app.cleanup();
        app.world_mut().run_schedule(FixedPostUpdate);

        let hits = app
            .world_mut()
            .run_system_once(
                move |spatial: SpatialQuery,
                      targets: Query<(&Transform, &CollisionBoxes, &SolidSides)>| {
                    box_query(
                        &spatial,
                        &targets,
                        cube(45.0),
                        [CollisionKind::Misc, CollisionKind::Player],
                    )
                },
            )
            .expect("the system runs");

        assert_eq!(
            hits,
            [BoxQueryHit {
                entity: near,
                target_box: 0,
                world_box: cube(10.0).at(Vec3::new(50.0, 0.0, 0.0)),
            }]
        );
    }
}
