//! The tick: a small bug that hides in some nuts. Once its nut is cracked
//! it crawls straight at the player, hurting on contact (it is spiked)
//! without knocking the player over. Landing on it flattens it for good;
//! a kick or any hurt kills it.
//!
//! Port of original/src/Enemies/Enemy_Tick.c. The tick isn't a map item:
//! the nut sends [`SpawnTick`] (`CreateNutContents` calling
//! `MakeTickEnemy`). It is a plain model (`GLOBAL2_MObjType_Tick`), not a
//! skeleton.
//!
//! `PlayerHitEnemy`'s ball switch has no tick case, and the spiked contact
//! is handled by the player's collision, so the tick answers only
//! [`EnemyBopped`], [`EnemyKicked`] and [`EnemyKilled`].
//!
//! `KillTick` bursts the model into shards (`QD3D_ExplodeGeometry`); that
//! effect isn't ported yet, so a killed tick just vanishes.

use std::f32::consts::TAU;

use avian3d::prelude::{CollisionLayers, LayerMask};
use bevy::prelude::*;

use super::{
    EnemyBody, EnemyBopped, EnemyCollision, EnemyKicked, EnemyKilled, EnemyKind, EnemyModel,
    EnemySkeleton, EnemySpawner, EnemySystems, default_enemy_collision_mask, nearest_player,
};
use crate::collision::{CollisionBox, CollisionKind, SolidSides};
use crate::items::pickups::SpawnTick;
use crate::math::{GameRandom, turn_toward, yaw_forward, yaw_of};
use crate::objects::{ModelFile, ModelRef};
use crate::player::Player;
use crate::terrain::TerrainMap;

pub struct TickPlugin;

impl Plugin for TickPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            (
                kick_ticks.in_set(EnemySystems::Kicked),
                // The player's collision bops before the ticks move, as in
                // the original's frame.
                (spawn_ticks, bop_ticks, move_ticks)
                    .chain()
                    .in_set(EnemySystems::Move),
                kill_ticks.in_set(EnemySystems::Killed),
            ),
        );
    }
}

/// `GLOBAL2_MObjType_Tick`
const TICK_MODEL: ModelRef = ModelRef::new(ModelFile::Global2, 8);
/// How fast a tick turns toward the player, in radians per second
/// (`TICK_TURN_SPEED`).
const TICK_TURN_SPEED: f32 = 3.0;
/// A tick's speed, in units per second (`TICK_CHASE_SPEED`).
const TICK_CHASE_SPEED: f32 = 100.0;
const TICK_HEALTH: f32 = 1.0;
/// What touching a tick costs the player (`TICK_DAMAGE`).
const TICK_DAMAGE: f32 = 0.05;
const TICK_SCALE: f32 = 1.2;
/// How much a bop flattens a tick's height (`Scale.y *= .2`).
const TICK_SQUISH_SCALE: f32 = 0.2;
/// The collision box, relative to the origin (`SetObjectCollisionBounds`).
const TICK_BOX: CollisionBox = CollisionBox::new(50.0, 0.0, -30.0, 30.0, 30.0, -30.0);

/// What a tick is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickState {
    /// Crawling at the player.
    Chase,
    /// Bopped flat: it lies still and collides with nothing (`CType == 0`).
    Flattened,
}

/// A tick's state, on its root entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickBrain {
    pub state: TickState,
}

/// The kinds a tick adds to `CTYPE_ENEMY|CTYPE_BLOCKCAMERA`. Unlike most
/// kickable enemies it isn't `AutoTarget`.
fn tick_kinds() -> LayerMask {
    LayerMask::from([
        CollisionKind::Boppable,
        CollisionKind::Spiked,
        CollisionKind::Kickable,
        CollisionKind::HurtNoKnock,
    ])
}

/// Port of `MakeTickEnemy` (original/src/Enemies/Enemy_Tick.c), for each
/// cracked nut that held a tick. Like the original it has no guard
/// (`MAX_TICK` is unused) and the tick has no shadow.
fn spawn_ticks(
    mut spawns: MessageReader<SpawnTick>,
    mut enemies: EnemySpawner,
    mut random: ResMut<GameRandom>,
) {
    for spawn in spawns.read() {
        let at = spawn.position;
        // The tick starts where the nut was, not on the floor; its first
        // move puts it there.
        let floor = enemies.map().floor_height(at.x, at.z);
        let yaw = random.next_f32() * TAU;
        let Some(tick) = enemies.spawn(
            EnemySkeleton::display_group(EnemyKind::Tick, TICK_MODEL, at.xz(), TICK_SCALE)
                .foot_offset(floor - at.y)
                .collision_box(TICK_BOX)
                .with_kinds(tick_kinds())
                .solid(SolidSides::TOUCHABLE)
                .health(TICK_HEALTH)
                .damage(TICK_DAMAGE)
                .yaw(yaw),
        ) else {
            continue;
        };
        enemies.commands().entity(tick).insert(TickBrain {
            state: TickState::Chase,
        });
    }
}

/// Port of `TickGotBopped` (original/src/Enemies/Enemy_Tick.c), called
/// from `PlayerBopEnemy` (original/src/Player/MyGuy.c): the tick is
/// flattened and no longer collides with anything.
fn bop_ticks(
    mut commands: Commands,
    mut bops: MessageReader<EnemyBopped>,
    mut ticks: Query<(&mut TickBrain, &EnemyModel)>,
    mut models: Query<&mut Transform>,
) {
    for bop in bops.read() {
        let Ok((mut brain, model)) = ticks.get_mut(bop.enemy) else {
            continue;
        };
        // The player's collision can report one landing on several steps,
        // but after the first the tick isn't boppable.
        if brain.state == TickState::Flattened {
            continue;
        }
        brain.state = TickState::Flattened;
        if let Ok(mut transform) = models.get_mut(model.0) {
            transform.scale.y *= TICK_SQUISH_SCALE;
        }
        commands
            .entity(bop.enemy)
            .insert(CollisionLayers::new(LayerMask::NONE, LayerMask::NONE));
    }
}

/// Port of `KillTick` (original/src/Enemies/Enemy_Tick.c): the tick is
/// deleted at once. Repeats are ignored.
fn kill_tick(commands: &mut Commands, tick: Entity) {
    // Sound and effect: `QD3D_ExplodeGeometry`, not ported yet.
    commands.entity(tick).try_despawn();
}

/// The kick kills a tick. Port of the `GLOBAL2_MObjType_Tick` case of
/// `DoBugKick` (original/src/Player/Player_Bug.c).
fn kick_ticks(
    mut commands: Commands,
    mut kicks: MessageReader<EnemyKicked>,
    ticks: Query<(), With<TickBrain>>,
) {
    for kick in kicks.read() {
        if ticks.contains(kick.enemy) {
            kill_tick(&mut commands, kick.enemy);
        }
    }
}

/// Port of the `ENEMY_KIND_TICK` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c), for hurts other objects sent.
fn kill_ticks(
    mut commands: Commands,
    mut killed: MessageReader<EnemyKilled>,
    ticks: Query<(), With<TickBrain>>,
) {
    for kill in killed.read() {
        if ticks.contains(kill.enemy) {
            kill_tick(&mut commands, kill.enemy);
        }
    }
}

/// Moves the ticks at the player. Leaving the item window
/// (`TrackTerrainItem`) is handled by `DespawnOutOfRange`.
///
/// Port of `MoveTick` (original/src/Enemies/Enemy_Tick.c).
fn move_ticks(
    mut commands: Commands,
    mut collision: EnemyCollision,
    map: Res<TerrainMap>,
    mut ticks: Query<(EnemyBody, &TickBrain), Without<Player>>,
    players: Query<&Transform, (With<Player>, Without<TickBrain>)>,
) {
    let dt = collision.dt();
    let player_positions: Vec<Vec3> = players.iter().map(|t| t.translation).collect();
    for (mut body, brain) in &mut ticks {
        if brain.state == TickState::Flattened {
            continue;
        }
        let coord = body.transform.translation;
        if let Some(target) = nearest_player(coord, player_positions.iter().copied()) {
            let (yaw, _) = turn_toward(
                yaw_of(body.transform.rotation),
                coord.xz(),
                target.xz(),
                TICK_TURN_SPEED * dt,
            );
            body.transform.rotation = Quat::from_rotation_y(yaw);
        }
        let forward = chase_velocity(yaw_of(body.transform.rotation));
        body.velocity.x = forward.x;
        body.velocity.z = forward.y;
        let mut coord = body.transform.translation;
        coord.x += forward.x * dt;
        coord.z += forward.y * dt;
        coord.y = map.floor_height(coord.x, coord.z);
        body.transform.translation = coord;

        // `KillTick` deletes the tick, which ends its move.
        let contact =
            collision.collide(&mut body, default_enemy_collision_mask(), &mut |_, _| true);
        if contact.deleted {
            kill_tick(&mut commands, body.entity);
        }
    }
}

/// A chasing tick's velocity in x and z, for its heading.
fn chase_velocity(yaw: f32) -> Vec2 {
    yaw_forward(yaw) * TICK_CHASE_SPEED
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// A chasing tick with its model, for the message handlers.
    fn world_with_tick() -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<Messages<EnemyBopped>>();
        world.init_resource::<Messages<EnemyKicked>>();
        world.init_resource::<Messages<EnemyKilled>>();
        let model = world
            .spawn(Transform::from_scale(Vec3::splat(TICK_SCALE)))
            .id();
        let tick = world
            .spawn((
                TickBrain {
                    state: TickState::Chase,
                },
                EnemyModel(model),
                CollisionLayers::new(tick_kinds(), LayerMask::NONE),
            ))
            .id();
        (world, tick, model)
    }

    #[test]
    fn a_bop_flattens_the_tick_once() {
        let (mut world, tick, model) = world_with_tick();
        for _ in 0..2 {
            world.write_message(EnemyBopped {
                player: Entity::PLACEHOLDER,
                enemy: tick,
            });
        }
        world.run_system_once(bop_ticks).expect("the system runs");
        assert_eq!(
            world.get::<TickBrain>(tick).map(|b| b.state),
            Some(TickState::Flattened)
        );
        assert_eq!(
            world.get::<CollisionLayers>(tick).map(|l| l.memberships),
            Some(LayerMask::NONE)
        );
        assert_eq!(
            world.get::<Transform>(model).map(|t| t.scale),
            Some(Vec3::new(
                TICK_SCALE,
                TICK_SCALE * TICK_SQUISH_SCALE,
                TICK_SCALE
            ))
        );
    }

    #[test]
    fn a_kick_deletes_the_tick() {
        let (mut world, tick, _) = world_with_tick();
        for _ in 0..2 {
            world.write_message(EnemyKicked {
                player: Entity::PLACEHOLDER,
                enemy: tick,
                direction: Vec2::NEG_Y,
                damage: 0.4,
            });
        }
        world.run_system_once(kick_ticks).expect("the system runs");
        assert!(world.get_entity(tick).is_err());
    }

    #[test]
    fn a_kill_deletes_the_tick() {
        let (mut world, tick, _) = world_with_tick();
        world.write_message(EnemyKilled {
            enemy: tick,
            knock: Vec3::ZERO,
        });
        world.run_system_once(kill_ticks).expect("the system runs");
        assert!(world.get_entity(tick).is_err());
    }

    #[test]
    fn the_tick_crawls_forward_at_its_speed() {
        // Facing yaw 0 is facing -z (`sin(r) * -speed`, `cos(r) * -speed`).
        let v = chase_velocity(0.0);
        assert!(v.x.abs() < 1e-4);
        assert!((v.y + TICK_CHASE_SPEED).abs() < 1e-4);
    }
}
