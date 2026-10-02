//! The fixed timestep, avian's collision queries, and the motion components
//! that movers share.
//!
//! Avian supplies colliders, the broad phase, spatial queries and transform
//! interpolation. It has no rigid bodies, solver or gravity here: movers
//! integrate their own motion in `FixedUpdate`, as the original does
//! (docs/design/phase2-engine-core.md §4).

use avian3d::dynamics::solver::joint_graph::JointGraphPlugin;
use avian3d::prelude::*;
use bevy::prelude::*;

/// Gameplay ticks per second.
pub const FIXED_HZ: f64 = 60.0;

/// Avian's length unit, in world units. A terrain tile is 160 units and the
/// bug about 180 units tall, so avian's tolerances need scaling up.
const LENGTH_UNIT: f32 = 100.0;

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_hz(FIXED_HZ))
            .insert_resource(PhysicsLengthUnit(LENGTH_UNIT))
            .add_plugins(
                PhysicsPlugins::default()
                    .build()
                    // Everything that simulates bodies. `SolverSchedulePlugin`
                    // stays, because the collider tree's systems are ordered
                    // against its sets.
                    .disable::<SolverBodyPlugin>()
                    .disable::<IntegratorPlugin>()
                    .disable::<SolverPlugin>()
                    .disable::<CcdPlugin>()
                    .disable::<IslandPlugin>()
                    .disable::<IslandSleepingPlugin>()
                    .disable::<JointGraphPlugin<FixedJoint>>()
                    .disable::<JointGraphPlugin<RevoluteJoint>>()
                    .disable::<JointGraphPlugin<PrismaticJoint>>()
                    .disable::<JointGraphPlugin<DistanceJoint>>()
                    .disable::<JointGraphPlugin<SphericalJoint>>()
                    .disable::<JointPlugin>()
                    .disable::<ForcePlugin>()
                    .disable::<MassPropertyPlugin>()
                    .disable::<NarrowPhasePlugin<Collider>>(),
            )
            .add_systems(FixedPreUpdate, remember_previous_positions);
    }
}

/// Where an entity was at the start of the tick (`OldCoord`). Box
/// collision compares against it to tell which side was crossed.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct PreviousPosition(pub Vec3);

/// Port of the `OldCoord` half of `KeepOldCollisionBoxes`
/// (original/src/System/Objects2.c), which `MoveObjects` calls for every
/// object before it moves.
fn remember_previous_positions(mut query: Query<(&Transform, &mut PreviousPosition)>) {
    for (transform, mut previous) in &mut query {
        previous.0 = transform.translation;
    }
}

/// Velocity in world units per second (`ObjNode::Delta`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct Velocity(pub Vec3);

/// How an entity touches the terrain, as its last move left it.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct GroundContact {
    /// Standing on the floor or on an object (`STATUS_BIT_ONGROUND`).
    pub on_ground: bool,
    /// Standing on the terrain itself (`STATUS_BIT_ONTERRAIN`).
    pub on_terrain: bool,
    /// The floor's normal under the entity (`gRecentTerrainNormal[FLOOR]`).
    pub floor_normal: Vec3,
    /// Height of the feet above the terrain floor (`gMyDistToFloor`).
    pub dist_to_floor: f32,
}

impl Default for GroundContact {
    fn default() -> Self {
        Self {
            on_ground: false,
            on_terrain: false,
            floor_normal: Vec3::Y,
            dist_to_floor: 0.0,
        }
    }
}
