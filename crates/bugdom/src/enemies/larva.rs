//! The larva: a slow grub in the bee hive that crawls at the player. It
//! hurts on contact (it is spiked), can be bopped flat by landing on it,
//! and dies to one kick.
//!
//! Port of original/src/Enemies/Enemy_Larva.c. Larvae come from map items
//! (`AddEnemy_Larva`), from spline items (`PrimeEnemy_Larva`), and from the
//! queen bee's spawn blobs ([`make_larva_enemy`], `MakeLarvaEnemy`).
//!
//! `PlayerHitEnemy`'s ball switch has no larva case, and the spiked contact
//! is handled by the player's collision, so the larva answers only
//! [`EnemyBopped`], [`EnemyKicked`] and [`EnemyKilled`].

use std::f32::consts::TAU;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::prelude::*;

use super::{
    EnemyBody, EnemyBopped, EnemyCollision, EnemyCulling, EnemyKicked, EnemyKilled, EnemyKind,
    EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, default_enemy_collision_mask,
    detach_enemy_from_spline, nearest_player,
};
use crate::collision::{CollisionBox, CollisionKind};
use crate::items::{ItemSpawn, RegisterItemKind, forget_terrain_item, kind};
use crate::math::{
    GameRandom, quick_distance, turn_toward, yaw_forward, yaw_from_point_to_point, yaw_of,
};
use crate::player::Player;
use crate::skeleton::{SkeletonAnimator, SkeletonType};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::terrain::TerrainMap;

pub struct LarvaPlugin;

impl Plugin for LarvaPlugin {
    fn build(&self, app: &mut App) {
        app.register_item_kind(kind::LARVA, add_larva)
            .register_spline_item_kind(kind::LARVA, prime_larva)
            .add_systems(
                FixedUpdate,
                (
                    kick_larvae.in_set(EnemySystems::Kicked),
                    // The player's collision bops before the larvae move,
                    // as in the original's frame.
                    (bop_larvae, move_larvae).chain().in_set(EnemySystems::Move),
                    kill_larvae.in_set(EnemySystems::Killed),
                    move_larvae_on_spline.in_set(SplineSystems::Move),
                ),
            );
    }
}

/// The most larvae counted at once (`MAX_LARVA`, and the 5 in
/// `MakeLarvaEnemy`).
const MAX_LARVA: usize = 5;
/// How close the player must come, in units, for a waiting larva to start
/// crawling (`LARVA_CHASE_RANGE`).
const LARVA_CHASE_RANGE: f32 = 700.0;
/// How fast a larva turns toward the player, in radians per second
/// (`LARVA_TURN_SPEED`).
const LARVA_TURN_SPEED: f32 = 3.0;
/// A crawling larva's speed, in units per second (`LARVA_CHASE_SPEED`).
const LARVA_CHASE_SPEED: f32 = 100.0;
/// How close the player must come, in units, for a larva to leave its
/// spline and chase (`LARVA_CHASE_RANGE2`).
const LARVA_CHASE_RANGE2: f32 = 80.0;
/// Speed along a spline, in baked points per second (`LARVA_SPLINE_SPEED`).
const LARVA_SPLINE_SPEED: f32 = 70.0;
const LARVA_HEALTH: f32 = 1.0;
/// What touching a larva costs the player (`LARVA_DAMAGE`).
const LARVA_DAMAGE: f32 = 0.1;
const LARVA_SCALE: f32 = 0.5;
/// The animation speed while crawling (`AnimSpeed = 2.0`).
const LARVA_CRAWL_ANIM_SPEED: f32 = 2.0;
/// How much of the blend into crawling happens per second.
const LARVA_WALK_MORPH_RATE: f32 = 7.0;
/// How much of the blend into dying happens per second.
const LARVA_DEAD_MORPH_RATE: f32 = 3.0;
/// How much a bop flattens a larva's height (`Scale.y *= .2`).
const LARVA_SQUISH_SCALE: f32 = 0.2;
/// The collision box, relative to the origin (`SetObjectCollisionBounds`).
const LARVA_BOX: CollisionBox = CollisionBox::new(70.0, 0.0, -40.0, 40.0, 40.0, -40.0);

/// The larva's animations (`LARVA_ANIM_*`).
mod anim {
    pub const WAIT: usize = 0;
    pub const WALK: usize = 1;
    pub const SQUISHED: usize = 2;
    pub const DEAD: usize = 3;
}

/// What a larva is doing. The original dispatches on its animation
/// (`myMoveTable[AnimNum]`); each state here is the animation of the same
/// name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LarvaState {
    /// Facing the player until it comes in range (`MoveLarva_Wait`).
    Wait,
    /// Crawling at the player (`MoveLarva_Walk`), or along its spline.
    Walk,
    /// Bopped flat (`MoveLarva_Squished`).
    Squished,
    /// Killed, waiting to be out of view (`MoveLarva_Dead`).
    Dead,
}

/// A larva's state, on its root entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LarvaBrain {
    pub state: LarvaState,
}

/// The kinds every larva adds to `CTYPE_ENEMY|CTYPE_BLOCKCAMERA`. Larvae
/// off splines are also kickable.
fn larva_kinds() -> LayerMask {
    LayerMask::from([
        CollisionKind::AutoTarget,
        CollisionKind::AutoTargetJump,
        CollisionKind::Boppable,
        CollisionKind::Spiked,
    ])
}

/// A larva's skeleton at `position`, as `MakeEnemySkeleton` and the fields
/// all three larva makers set.
fn larva_skeleton(position: Vec2) -> EnemySkeleton {
    EnemySkeleton::new(EnemyKind::Larva, SkeletonType::Larva, position, LARVA_SCALE)
        .collision_box(LARVA_BOX)
        .with_kinds(larva_kinds())
        .health(LARVA_HEALTH)
        .damage(LARVA_DAMAGE)
}

/// Port of `AddEnemy_Larva` (original/src/Enemies/Enemy_Larva.c).
fn add_larva(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    if !enemies.can_spawn(EnemyKind::Larva, MAX_LARVA) {
        return false;
    }
    let Some(larva) = enemies.spawn(
        larva_skeleton(spawn.position)
            .from_item(spawn.index)
            .kickable()
            .anim(anim::WAIT),
    ) else {
        return false;
    };
    enemies.commands().entity(larva).insert(LarvaBrain {
        state: LarvaState::Wait,
    });
    true
}

/// Makes a free-roaming larva at `position` (world x and z), facing a
/// random way, and returns it; `None` if there are already
/// [`MAX_LARVA`] larvae, or the level has no larva skeleton. Unlike map
/// items, it doesn't check the total of enemies.
///
/// Port of `MakeLarvaEnemy` (original/src/Enemies/Enemy_Larva.c), which
/// the queen bee's spawn blobs call.
pub fn make_larva_enemy(
    enemies: &mut EnemySpawner,
    random: &mut GameRandom,
    position: Vec2,
) -> Option<Entity> {
    if enemies.counts().of_kind(EnemyKind::Larva) >= MAX_LARVA {
        return None;
    }
    let yaw = random.next_f32() * TAU;
    let larva = enemies.spawn(
        larva_skeleton(position)
            .kickable()
            .anim(anim::WAIT)
            .yaw(yaw),
    )?;
    enemies.commands().entity(larva).insert(LarvaBrain {
        state: LarvaState::Wait,
    });
    Some(larva)
}

/// Port of `PrimeEnemy_Larva` (original/src/Enemies/Enemy_Larva.c). It
/// has no Add guard, and a larva on a spline can't be kicked.
fn prime_larva(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let on_spline = OnSpline::new(spawn.spline, spawn.placement, LARVA_SPLINE_SPEED);
    let Some(larva) = enemies.spawn(
        larva_skeleton(spawn.position)
            .on_spline(on_spline)
            .anim(anim::WALK),
    ) else {
        return false;
    };
    enemies.commands().entity(larva).insert(LarvaBrain {
        state: LarvaState::Walk,
    });
    true
}

/// A larva's collision kinds (`CType = ...`). The layers are immutable, so
/// they are replaced.
fn collision_kinds(kinds: LayerMask) -> CollisionLayers {
    CollisionLayers::new(kinds, LayerMask::NONE)
}

/// The parts of a larva that its kill and bop change.
struct LarvaParts<'a> {
    entity: Entity,
    brain: &'a mut LarvaBrain,
    animator: Option<Mut<'a, SkeletonAnimator>>,
}

/// Port of `KillLarva` (original/src/Enemies/Enemy_Larva.c): the larva
/// dies, never comes back, and is just an obstacle until it is out of
/// view. It never deletes the larva at once. Repeats are ignored.
fn kill_larva(larva: LarvaParts, commands: &mut Commands) {
    if larva.brain.state == LarvaState::Dead {
        return;
    }
    larva.brain.state = LarvaState::Dead;
    forget_terrain_item(&mut commands.entity(larva.entity));
    if let Some(mut animator) = larva.animator {
        animator.morph_to(anim::DEAD, LARVA_DEAD_MORPH_RATE);
    }
    commands
        .entity(larva.entity)
        .insert(collision_kinds(CollisionKind::Misc.into()));
}

/// Port of `LarvaGotBopped` (original/src/Enemies/Enemy_Larva.c), called
/// from `PlayerBopEnemy` (original/src/Player/MyGuy.c): the larva leaves
/// its spline, is flattened, and no longer collides with anything.
fn bop_larvae(
    mut commands: Commands,
    mut bops: MessageReader<EnemyBopped>,
    mut larvae: Query<(&mut LarvaBrain, &EnemyModel)>,
    mut models: Query<(&mut SkeletonAnimator, &mut Transform)>,
) {
    for bop in bops.read() {
        let Ok((mut brain, model)) = larvae.get_mut(bop.enemy) else {
            continue;
        };
        // The player's collision can report one landing on several steps,
        // but after the first the larva isn't boppable.
        if matches!(brain.state, LarvaState::Squished | LarvaState::Dead) {
            continue;
        }
        detach_enemy_from_spline(&mut commands.entity(bop.enemy));
        brain.state = LarvaState::Squished;
        if let Ok((mut animator, mut transform)) = models.get_mut(model.0) {
            animator.set_anim(anim::SQUISHED);
            transform.scale.y *= LARVA_SQUISH_SCALE;
        }
        commands
            .entity(bop.enemy)
            .insert(collision_kinds(LayerMask::NONE));
    }
}

/// The kick kills a larva outright, whatever its health. Port of the
/// `SKELETON_TYPE_LARVA` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c).
fn kick_larvae(
    mut commands: Commands,
    mut kicks: MessageReader<EnemyKicked>,
    mut larvae: Query<(&mut LarvaBrain, &EnemyModel)>,
    mut animators: Query<&mut SkeletonAnimator>,
) {
    for kick in kicks.read() {
        let Ok((mut brain, model)) = larvae.get_mut(kick.enemy) else {
            continue;
        };
        let larva = LarvaParts {
            entity: kick.enemy,
            brain: &mut brain,
            animator: animators.get_mut(model.0).ok(),
        };
        kill_larva(larva, &mut commands);
    }
}

/// Port of the `ENEMY_KIND_LARVA` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c), for hurts other objects sent.
fn kill_larvae(
    mut commands: Commands,
    mut killed: MessageReader<EnemyKilled>,
    mut larvae: Query<(&mut LarvaBrain, &EnemyModel)>,
    mut animators: Query<&mut SkeletonAnimator>,
) {
    for kill in killed.read() {
        let Ok((mut brain, model)) = larvae.get_mut(kill.enemy) else {
            continue;
        };
        let larva = LarvaParts {
            entity: kill.enemy,
            brain: &mut brain,
            animator: animators.get_mut(model.0).ok(),
        };
        kill_larva(larva, &mut commands);
    }
}

/// Moves the free-roaming larvae by their state. Leaving the item window
/// (`TrackTerrainItem`) is handled by `DespawnOutOfRange`.
///
/// Port of `MoveLarva`, `MoveLarva_Wait`, `MoveLarva_Walk`,
/// `MoveLarva_Squished` and `MoveLarva_Dead`
/// (original/src/Enemies/Enemy_Larva.c).
fn move_larvae(
    mut commands: Commands,
    mut collision: EnemyCollision,
    map: Res<TerrainMap>,
    mut larvae: Query<
        (EnemyBody, &mut LarvaBrain, &EnemyModel),
        (Without<OnSpline>, Without<Player>),
    >,
    mut animators: Query<&mut SkeletonAnimator>,
    players: Query<&Transform, (With<Player>, Without<LarvaBrain>)>,
    culling: EnemyCulling,
) {
    let dt = collision.dt();
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    for (mut body, mut brain, model) in &mut larvae {
        let coord = body.transform.translation;
        let target = nearest_player(coord, player_positions.iter().copied()).map(|p| p.xz());
        let mut animator = animators.get_mut(model.0).ok();
        match brain.state {
            LarvaState::Wait => {
                if let Some(target) = target {
                    turn_toward_player(&mut body.transform, target, dt);
                    if quick_distance(coord.xz(), target) < LARVA_CHASE_RANGE {
                        brain.state = LarvaState::Walk;
                        if let Some(animator) = animator.as_mut() {
                            animator.morph_to(anim::WALK, LARVA_WALK_MORPH_RATE);
                        }
                    }
                }
            }
            LarvaState::Walk => {
                if let Some(animator) = animator.as_mut() {
                    animator.speed = LARVA_CRAWL_ANIM_SPEED;
                }
                if let Some(target) = target {
                    turn_toward_player(&mut body.transform, target, dt);
                }
                let forward = yaw_forward(yaw_of(body.transform.rotation)) * LARVA_CHASE_SPEED;
                body.velocity.x = forward.x;
                body.velocity.z = forward.y;
                let mut coord = body.transform.translation;
                coord.x += body.velocity.x * dt;
                coord.z += body.velocity.z * dt;
                coord.y = map.floor_height(coord.x, coord.z);
                body.transform.translation = coord;
            }
            // `UpdateEnemy` only: it lies where it was bopped.
            LarvaState::Squished => continue,
            LarvaState::Dead => {
                if culling.is_culled(body.transform.translation, **body.radius) {
                    commands.entity(body.entity).despawn();
                }
                continue;
            }
        }

        // Port of `DoEnemyCollisionDetect` for Wait and Walk. `KillLarva`
        // never deletes, so its effects can wait until the collision ends.
        let mut killed = false;
        collision.collide(&mut body, default_enemy_collision_mask(), &mut |_, _| {
            killed = true;
            false
        });
        if killed {
            let larva = LarvaParts {
                entity: body.entity,
                brain: &mut brain,
                animator,
            };
            kill_larva(larva, &mut commands);
        }
    }
}

/// Turns toward the player at [`LARVA_TURN_SPEED`] (`TurnObjectTowardTarget`).
fn turn_toward_player(transform: &mut Transform, target: Vec2, dt: f32) {
    let yaw = yaw_of(transform.rotation);
    let (yaw, _) = turn_toward(
        yaw,
        transform.translation.xz(),
        target,
        LARVA_TURN_SPEED * dt,
    );
    transform.rotation = Quat::from_rotation_y(yaw);
}

/// Moves the larvae along their splines, and lets one chase the player
/// once the player is close.
///
/// Port of `MoveLarvaOnSpline` (original/src/Enemies/Enemy_Larva.c).
fn move_larvae_on_spline(
    mut commands: Commands,
    time: Res<Time>,
    splines: Option<Res<Splines>>,
    map: Res<TerrainMap>,
    mut larvae: Query<(Entity, &mut Transform, &mut OnSpline, &EnemyModel), With<LarvaBrain>>,
    mut animators: Query<&mut SkeletonAnimator>,
    players: Query<&Transform, (With<Player>, Without<LarvaBrain>)>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    for (entity, mut transform, mut on_spline, model) in &mut larvae {
        let old = transform.translation.xz();
        on_spline.advance(&splines, dt);
        let position = on_spline.position(&splines);
        transform.translation.x = position.x;
        transform.translation.z = position.y;

        if !on_spline.visible {
            // Sound: stop EFFECT_BUZZ (`StopObjectStreamEffect`).
            continue;
        }
        if let Ok(mut animator) = animators.get_mut(model.0) {
            animator.speed = LARVA_CRAWL_ANIM_SPEED;
        }
        // Sound: start or update EFFECT_BUZZ at the larva.
        let yaw = yaw_from_point_to_point(yaw_of(transform.rotation), old, position);
        transform.rotation = Quat::from_rotation_y(yaw);
        transform.translation.y = map.floor_height(position.x, position.y);

        if let Some(target) =
            nearest_player(position.extend(0.0).xzy(), player_positions.iter().copied())
                .map(|p| p.xz())
            && quick_distance(position, target) < LARVA_CHASE_RANGE2
        {
            // Off the spline it crawls on (`MoveLarva` with the walk
            // animation it has had since `PrimeEnemy_Larva`).
            detach_enemy_from_spline(&mut commands.entity(entity));
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::items::kind;

    /// A larva on a spline, with its model, for the message handlers.
    fn world_with_larva() -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<super::super::EnemyCounts>();
        world.init_resource::<Messages<EnemyBopped>>();
        world.init_resource::<Messages<EnemyKicked>>();
        let model = world
            .spawn((
                SkeletonAnimator::default(),
                Transform::from_scale(Vec3::splat(LARVA_SCALE)),
            ))
            .id();
        let larva = world
            .spawn((
                LarvaBrain {
                    state: LarvaState::Walk,
                },
                EnemyModel(model),
                OnSpline::new(0, 0.0, LARVA_SPLINE_SPEED),
                collision_kinds(larva_kinds()),
            ))
            .id();
        (world, larva, model)
    }

    fn kinds(world: &World, larva: Entity) -> Option<LayerMask> {
        world.get::<CollisionLayers>(larva).map(|l| l.memberships)
    }

    #[test]
    fn a_bop_flattens_the_larva_once() {
        let (mut world, larva, model) = world_with_larva();
        let player = Entity::PLACEHOLDER;
        for _ in 0..2 {
            world.write_message(EnemyBopped {
                player,
                enemy: larva,
            });
        }
        world.run_system_once(bop_larvae).expect("the system runs");
        assert_eq!(
            world.get::<LarvaBrain>(larva).map(|b| b.state),
            Some(LarvaState::Squished)
        );
        assert!(!world.entity(larva).contains::<OnSpline>());
        assert_eq!(kinds(&world, larva), Some(LayerMask::NONE));
        let scale = world.get::<Transform>(model).map(|t| t.scale);
        assert_eq!(
            scale,
            Some(Vec3::new(
                LARVA_SCALE,
                LARVA_SCALE * LARVA_SQUISH_SCALE,
                LARVA_SCALE
            ))
        );
        assert_eq!(
            world.get::<SkeletonAnimator>(model).map(|a| a.anim),
            Some(anim::SQUISHED)
        );
    }

    #[test]
    fn a_kick_kills_the_larva() {
        let (mut world, larva, model) = world_with_larva();
        world.write_message(EnemyKicked {
            player: Entity::PLACEHOLDER,
            enemy: larva,
            direction: Vec2::NEG_Y,
            damage: 0.4,
        });
        world.run_system_once(kick_larvae).expect("the system runs");
        assert_eq!(
            world.get::<LarvaBrain>(larva).map(|b| b.state),
            Some(LarvaState::Dead)
        );
        assert_eq!(kinds(&world, larva), Some(CollisionKind::Misc.into()));
        let animator = world.get::<SkeletonAnimator>(model);
        assert_eq!(animator.map(|a| a.anim), Some(anim::DEAD));
        assert!(animator.is_some_and(|a| a.is_morphing()));
    }

    #[test]
    fn the_bee_hive_has_larvae() {
        use bugdom_formats::rsrc::ResourceFork;
        let path = bugdom_formats::original_data_dir().join("Terrain/BeeHive.ter.rsrc");
        let fork = ResourceFork::open(&path).expect("terrain file");
        let terrain = bugdom_formats::terrain::parse(&fork).expect("terrain");
        let items = terrain
            .items
            .iter()
            .filter(|i| i.kind == kind::LARVA)
            .count();
        let on_splines = terrain
            .splines
            .iter()
            .flat_map(|s| &s.items)
            .filter(|i| i.kind == kind::LARVA)
            .count();
        assert!(items + on_splines > 0);
    }
}
