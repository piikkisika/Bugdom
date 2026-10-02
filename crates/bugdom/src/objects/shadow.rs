//! Blob shadows under objects, lying on the terrain or on objects that
//! block shadows.
//!
//! Port of `AttachShadowToObject` and `UpdateShadow`
//! (original/src/System/Objects2.c). The shadow is its own entity rather
//! than a child, because it is tilted to the terrain rather than turned
//! with its owner; it is despawned with its owner.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::{ModelFile, ModelRef, ModelSpawner, Shading};
use crate::collision::{CollisionBoxes, CollisionKind};
use crate::liquids::{Liquid, Underwater};
use crate::math::yaw_of;
use crate::player::PlayerSystems;
use crate::splines::SplineSystems;
use crate::state::AppState;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        update_shadows
            .after(PlayerSystems::Move)
            .after(SplineSystems::Move)
            .run_if(in_state(AppState::InGame)),
    );
}

/// The shadow model (`GLOBAL1_MObjType_Shadow`).
const SHADOW_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 0);
/// How far above the ground a shadow floats (`SHADOW_Y_OFF`).
const SHADOW_HEIGHT: f32 = 6.0;
/// How much smaller a shadow gets per unit its owner is above the ground.
const SHADOW_SHRINK_PER_UNIT: f32 = 1.0 / 400.0;
/// The most a shadow shrinks.
const SHADOW_MAX_SHRINK: f32 = 0.5;

/// A blob shadow, following its owner ([`ShadowOf`]).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    /// Size in x and z when the owner is on the ground.
    pub scale: Vec2,
    /// Whether the shadow can lie on objects that block shadows
    /// (`CheckForBlockers`).
    pub on_objects: bool,
}

/// The owner a shadow follows.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
#[relationship(relationship_target = Shadows)]
pub struct ShadowOf(pub Entity);

/// An object's shadows, despawned with it (`ShadowNode`).
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
#[relationship_target(relationship = ShadowOf, linked_spawn)]
pub struct Shadows(Vec<Entity>);

/// Gives `owner` a shadow of the given size (`AttachShadowToObject`).
pub fn attach_shadow(
    commands: &mut Commands,
    models: &mut ModelSpawner,
    owner: Entity,
    scale: Vec2,
    on_objects: bool,
) {
    let shadow = commands
        .spawn((
            Name::new("Shadow"),
            Shadow { scale, on_objects },
            ShadowOf(owner),
            Transform::default(),
            Visibility::default(),
            TransformInterpolation,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    models.spawn(
        commands,
        shadow,
        SHADOW_MODEL,
        Shading::Shadow,
        Transform::default(),
    );
}

/// Puts each shadow under its owner.
/// Port of `UpdateShadow` (original/src/System/Objects2.c).
fn update_shadows(
    map: Res<TerrainMap>,
    spatial: SpatialQuery,
    owners: Query<
        (
            &Transform,
            Option<&CollisionBoxes>,
            Has<Underwater>,
            Option<&Visibility>,
        ),
        Without<Shadow>,
    >,
    blockers: Query<(&Transform, &CollisionBoxes, Option<&Liquid>), Without<Shadow>>,
    mut shadows: Query<(&Shadow, &ShadowOf, &mut Transform, &mut Visibility)>,
) {
    for (shadow, owner, mut transform, mut visibility) in &mut shadows {
        let Ok((owner_transform, boxes, underwater, owner_visibility)) = owners.get(owner.0) else {
            continue;
        };
        // No shadow in a liquid, nor for a hidden owner (such as a spline
        // object out of range, whose `ShadowNode` the original detaches).
        if underwater || owner_visibility == Some(&Visibility::Hidden) {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        visibility.set_if_neq(Visibility::Inherited);
        let coord = owner_transform.translation;
        let bottom = boxes.and_then(|b| b.0.first()).map_or(0.0, |b| b.bottom);
        // The original truncates to whole units for this test.
        let feet = Vec3::new(coord.x.trunc(), (coord.y + bottom).trunc(), coord.z.trunc());
        let yaw = yaw_of(owner_transform.rotation);

        if shadow.on_objects
            && let Some(top) = blocker_top(&spatial, &blockers, feet)
        {
            *transform = Transform::from_xyz(coord.x, top + SHADOW_HEIGHT, coord.z)
                .with_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::new(shadow.scale.x, shadow.scale.x, shadow.scale.y));
            continue;
        }

        // On the terrain: tilted to the slope, and smaller the higher the
        // owner is. The original applies the new size a frame late.
        let (floor, normal) = map.height_at(coord.x, coord.z, crate::terrain::LayerKind::Floor);
        let y = floor + SHADOW_HEIGHT;
        let shrink = 1.0 - ((feet.y - y) * SHADOW_SHRINK_PER_UNIT).clamp(0.0, SHADOW_MAX_SHRINK);
        *transform = Transform::from_xyz(coord.x, y, coord.z)
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, normal) * Quat::from_rotation_y(yaw))
            .with_scale(Vec3::new(
                shrink * shadow.scale.x,
                shadow.scale.x,
                shrink * shadow.scale.y,
            ));
    }
}

/// The top of the first shadow-blocking object under `feet`, if any.
fn blocker_top(
    spatial: &SpatialQuery,
    blockers: &Query<(&Transform, &CollisionBoxes, Option<&Liquid>), Without<Shadow>>,
    feet: Vec3,
) -> Option<f32> {
    // Everything below the feet, in a thin column.
    const DEPTH: f32 = 100_000.0;
    let column = Collider::cuboid(1.0, DEPTH, 1.0);
    let filter = SpatialQueryFilter::from_mask(CollisionKind::BlockShadow);
    let center = feet - Vec3::Y * (DEPTH / 2.0);
    spatial
        .shape_intersections(&column, center, Quat::IDENTITY, &filter)
        .into_iter()
        .filter_map(|entity| {
            let (transform, boxes, liquid) = blockers.get(entity).ok()?;
            let b = boxes.0.first()?.at(transform.translation);
            (feet.y >= b.bottom
                && feet.x >= b.left
                && feet.x <= b.right
                && feet.z <= b.front
                && feet.z >= b.back)
                // A liquid's box is its volume, under the surface.
                .then_some(b.top + liquid.map_or(0.0, |l| l.surface_above_volume()))
        })
        .next()
}
