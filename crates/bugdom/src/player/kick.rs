//! The bug's kick: what it hits, and whom it tells.
//!
//! Port of `DoBugKick` and the search of `AimAtClosestKickableObject`
//! (original/src/Player/Player_Bug.c). The kick's state is
//! [`BugState::Kick`](super::BugState::Kick) in `bug.rs`; the hit objects
//! answer [`EnemyKicked`] and [`ItemKicked`] in their own plugins.

use avian3d::prelude::{CollisionLayers, SpatialQuery};
use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::Player;
use super::contact::{EnemyKicked, ItemKicked, KICK_ENEMY_DAMAGE};
use crate::collision::{
    BoxQueryHit, CollisionBox, CollisionBoxes, CollisionKind, SolidSides, box_query,
};
use crate::enemies::Enemy;
use crate::math::yaw_forward;
use crate::skeleton::{SkeletonRig, joint_position};
use crate::splines::OnSpline;

/// The animation flag the kick animation sets on the frame the foot lands
/// (`KickNow`, `Flag[0]`).
pub const KICK_NOW_FLAG: usize = 0;
/// The bug's pelvis joint (`BUG_LIMB_NUM_PELVIS`).
const PELVIS_JOINT: usize = 0;
/// Where the kick lands, in the pelvis joint's space (`offsetCoord`).
const KICK_OFFSET: Vec3 = Vec3::new(0.0, -10.0, -50.0);
/// Half the size of the cube the kick hits around that point, in units.
const KICK_REACH: f32 = 40.0;

/// Where the kick lands, given the model's rig, its transform relative to
/// the player, and the player's position and heading. `None` if the rig
/// has no pelvis. Port of the `FindCoordOnJoint` in `DoBugKick`.
pub fn kick_impact(rig: &SkeletonRig, model: &Transform, coord: Vec3, yaw: f32) -> Option<Vec3> {
    let base = Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), coord)
        * model.compute_affine();
    joint_position(rig, PELVIS_JOINT, KICK_OFFSET, base)
}

/// The box the kick hits (`DoSimpleBoxCollision` in `DoBugKick`).
pub fn kick_area(impact: Vec3) -> CollisionBox {
    CollisionBox::new(
        impact.y + KICK_REACH,
        impact.y - KICK_REACH,
        impact.x - KICK_REACH,
        impact.x + KICK_REACH,
        impact.z + KICK_REACH,
        impact.z - KICK_REACH,
    )
}

/// What the kick tells each object it hit: enemies get [`EnemyKicked`],
/// other kickable objects [`ItemKicked`]. As in the original, an object
/// hit with several boxes is told once per box.
pub fn kick_messages(
    player: Entity,
    yaw: f32,
    hits: &[BoxQueryHit],
    is_enemy: impl Fn(Entity) -> bool,
) -> (Vec<EnemyKicked>, Vec<ItemKicked>) {
    let direction = yaw_forward(yaw);
    let mut enemies = Vec::new();
    let mut items = Vec::new();
    for hit in hits {
        if is_enemy(hit.entity) {
            enemies.push(EnemyKicked {
                player,
                enemy: hit.entity,
                direction,
                damage: KICK_ENEMY_DAMAGE,
            });
        } else {
            items.push(ItemKicked {
                player,
                item: hit.entity,
                direction,
            });
        }
    }
    (enemies, items)
}

/// Where the kickable objects are, for aiming the kick.
#[derive(SystemParam)]
pub(super) struct Kickables<'w, 's> {
    kickables: Query<
        'w,
        's,
        (
            &'static Transform,
            &'static CollisionLayers,
            Option<&'static OnSpline>,
        ),
        Without<Player>,
    >,
}

impl Kickables<'_, '_> {
    /// Where the kickable objects in the object list are, in x and z. Those
    /// on splines out of the window aren't in the list.
    pub fn positions(&self) -> Vec<Vec2> {
        self.kickables
            .iter()
            .filter(|(_, layers, on_spline)| {
                layers.memberships.has_all(CollisionKind::Kickable)
                    && on_spline.is_none_or(|s| s.visible)
            })
            .map(|(transform, ..)| transform.translation.xz())
            .collect()
    }
}

/// A kick landed this tick: the bug's movement sends it, and [`land_kicks`]
/// finds what it hit. They are apart because the hit test's spatial query
/// reads the colliders that the movement writes.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub(super) struct KickLanded {
    pub player: Entity,
    /// Where the foot lands.
    pub impact: Vec3,
    /// The bug's heading when it kicked.
    pub yaw: f32,
}

/// Kicks whatever kickable is around each kick's impact, telling enemies
/// with [`EnemyKicked`] and other objects with [`ItemKicked`].
///
/// Port of the collision part of `DoBugKick`
/// (original/src/Player/Player_Bug.c). The per-kind switch is left to the
/// kicked objects' plugins.
pub(super) fn land_kicks(
    mut kicks: MessageReader<KickLanded>,
    spatial: SpatialQuery,
    targets: Query<(&Transform, &CollisionBoxes, &SolidSides), Without<Player>>,
    enemies: Query<(), With<Enemy>>,
    mut enemy_kicks: MessageWriter<EnemyKicked>,
    mut item_kicks: MessageWriter<ItemKicked>,
) {
    for kick in kicks.read() {
        let hits = box_query(
            &spatial,
            &targets,
            kick_area(kick.impact),
            CollisionKind::Kickable,
        );
        if hits.is_empty() {
            continue;
        }
        // Sound: EFFECT_POUND at the impact.
        let (enemy_hits, item_hits) =
            kick_messages(kick.player, kick.yaw, &hits, |e| enemies.contains(e));
        enemy_kicks.write_batch(enemy_hits);
        item_kicks.write_batch(item_hits);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use avian3d::prelude::LayerMask;

    use super::*;
    use crate::collision::solid_object;
    use crate::physics::PhysicsPlugin;

    #[test]
    fn the_kick_tells_enemies_and_items_apart() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            bevy::asset::AssetPlugin::default(),
            bevy::mesh::MeshPlugin,
            PhysicsPlugin,
        ))
        .add_message::<EnemyKicked>()
        .add_message::<ItemKicked>()
        .add_message::<KickLanded>();
        let cube = CollisionBox::new(30.0, -30.0, -30.0, 30.0, 30.0, -30.0);
        let mut spawn = |at: Vec3, kinds: LayerMask, enemy: bool| {
            let mut entity = app.world_mut().spawn((
                Transform::from_translation(at),
                solid_object(vec![cube], kinds, SolidSides::NOT_TOP),
            ));
            if enemy {
                entity.insert(Enemy {
                    kind: crate::enemies::EnemyKind::Ant,
                });
            }
            entity.id()
        };
        let impact = Vec3::new(1000.0, 50.0, 1000.0);
        let ant = spawn(
            impact + Vec3::X * 60.0,
            LayerMask::from([CollisionKind::Enemy, CollisionKind::Kickable]),
            true,
        );
        let nut = spawn(
            impact - Vec3::Z * 60.0,
            LayerMask::from(CollisionKind::Kickable),
            false,
        );
        // Out of reach, and not kickable.
        spawn(
            impact + Vec3::X * 200.0,
            LayerMask::from([CollisionKind::Enemy, CollisionKind::Kickable]),
            true,
        );
        spawn(impact, LayerMask::from(CollisionKind::Enemy), true);
        app.finish();
        app.cleanup();
        app.world_mut().run_schedule(FixedPostUpdate);

        let player = Entity::PLACEHOLDER;
        app.world_mut().write_message(KickLanded {
            player,
            impact,
            yaw: 0.0,
        });
        app.world_mut()
            .run_system_once(land_kicks)
            .expect("the system runs");

        let enemies: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<EnemyKicked>>()
            .drain()
            .collect();
        let items: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<ItemKicked>>()
            .drain()
            .collect();
        // Facing −Z at yaw 0.
        let direction = Vec2::new(0.0, -1.0);
        assert_eq!(
            enemies,
            [EnemyKicked {
                player,
                enemy: ant,
                direction,
                damage: KICK_ENEMY_DAMAGE,
            }]
        );
        assert_eq!(
            items,
            [ItemKicked {
                player,
                item: nut,
                direction,
            }]
        );
        let knock = enemies[0].knock(700.0, 700.0);
        assert!((knock - Vec3::new(0.0, 700.0, -700.0)).length() < 1e-3);
    }
}
