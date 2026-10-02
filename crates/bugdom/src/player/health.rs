//! The player getting hurt: invincibility after a hit, the shield, being
//! knocked on its butt, and dying when its health runs out.
//!
//! Port of `PlayerGotHurt` and `KillPlayer` (original/src/Player/MyGuy.c),
//! `LoseHealth` and `GetHealth` (original/src/Screens/Infobar.c) and
//! `KnockPlayerBugOnButt` (original/src/Player/Player_Bug.c). The generic
//! [`Health`] and [`Damage`](crate::combat::Damage) live in `combat.rs`.
//!
//! The original does not blink the player while it is invincible, so
//! neither does this.

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use super::ball::become_bug;
use super::bug::BugState;
use super::movement::{PlayerData, PlayerDataItem};
use super::{Dying, Player, PlayerForm, PlayerTuning};
use crate::combat::Health;
use crate::math::{yaw_from_point_to_point, yaw_of};
use crate::physics::Velocity;

/// The player's full health. `GetHealth` caps it here.
pub const PLAYER_MAX_HEALTH: f32 = 1.0;
/// Seconds of invincibility after most hurts (`INVINCIBILITY_DURATION`).
/// It includes the time spent on the butt.
pub const INVINCIBILITY_DURATION: f32 = 3.0;
/// Seconds of invincibility after starting again from a death
/// (`INVINCIBILITY_DURATION_DEATH`).
pub const INVINCIBILITY_DURATION_DEATH: f32 = 3.0;
/// Seconds of shield that a shield powerup gives (`SHIELD_TIME`).
pub const SHIELD_TIME: f32 = 10.0;
/// Upward speed of a knock on the butt from something, in units per
/// second (`PlayerGotHurt`).
pub const KNOCK_RISE_SPEED: f32 = 800.0;

/// Seconds the player can't be hurt for, after a hurt or a death
/// (`InvincibleTimer`). It counts down to zero.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct InvincibleTimer(pub f32);

/// Seconds of shield left (`gShieldTimer`). While it runs, only hurts that
/// override the shield get through. The shield's sparkles and hum arrive
/// with the shield powerup.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct ShieldTimer(pub f32);

/// A knock waiting for the end of the ball's next move, where the ball
/// turns into the knocked bug (`gPlayerKnockOnButt` and
/// `gPlayerKnockOnButtDelta`). Turning into the ball any other way drops it
/// (`InitPlayer_Ball`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct DeferredKnock(pub Vec3);

/// The player has been set on fire (`gTorchPlayer`). The flames and the
/// damage they do arrive with the torch effect; dying or starting the level
/// puts the fire out.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Torched;

/// Hurts a player (`PlayerGotHurt`). Other objects send it; the player's
/// systems apply it after the player has moved. What the player runs into
/// itself hurts it in the middle of its move instead.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct HurtPlayer {
    pub player: Entity,
    /// What hurt the player (`what`), if anything. Its velocity sets the
    /// direction of the knock.
    pub source: Option<Entity>,
    /// Health to take away, on the 0–1 scale.
    pub damage: f32,
    /// Knock the player on its butt, unless the hurt kills it
    /// (`canKnockOnButt`).
    pub knock: bool,
    /// Seconds of invincibility the hurt gives (`invincibleDuration`).
    pub invincible_for: f32,
    /// Hurt even through the shield (`overrideShield`).
    pub override_shield: bool,
    /// Set the player on fire if the hurt kills it (a
    /// `PARTICLE_FLAGS_HURTPLAYERBAD` particle's `gTorchPlayer`).
    pub torch_if_killed: bool,
}

impl HurtPlayer {
    /// The usual hurt: it knocks the player over, gives the usual
    /// invincibility and is stopped by the shield.
    pub fn new(player: Entity, source: Option<Entity>, damage: f32) -> Self {
        Self {
            player,
            source,
            damage,
            knock: true,
            invincible_for: INVINCIBILITY_DURATION,
            override_shield: false,
            torch_if_killed: false,
        }
    }
}

/// What a [`HurtPlayer`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HurtOutcome {
    /// Nothing: no damage, shielded, already dead or invincible.
    Ignored,
    Hurt,
    /// The player's health ran out.
    Killed,
}

/// Applies a hurt's damage and invincibility. A hurt does nothing if it
/// has no damage, if the shield is up (unless it overrides the shield), if
/// the player is already dead or if it is still invincible.
///
/// Port of the checks in `PlayerGotHurt` (original/src/Player/MyGuy.c) and
/// of `LoseHealth` (original/src/Screens/Infobar.c). Easy mode, which
/// halves the damage, arrives with the settings.
pub fn take_hurt(
    hurt: &HurtPlayer,
    dying: bool,
    shield: ShieldTimer,
    health: &mut Health,
    invincible: &mut InvincibleTimer,
) -> HurtOutcome {
    if hurt.damage == 0.0 || (!hurt.override_shield && *shield > 0.0) || dying || **invincible > 0.0
    {
        return HurtOutcome::Ignored;
    }
    **invincible = invincible.max(hurt.invincible_for);
    if health.lose(hurt.damage) {
        HurtOutcome::Killed
    } else {
        HurtOutcome::Hurt
    }
}

/// The velocity a hurt knocks the player off with: the source's horizontal
/// motion plus a hop, or none without a source (`PlayerGotHurt`).
pub fn knock_velocity(
    source: Option<Entity>,
    velocities: &Query<&Velocity, Without<Player>>,
) -> Vec3 {
    match source {
        Some(entity) => {
            let motion = velocities.get(entity).map_or(Vec3::ZERO, |v| **v);
            Vec3::new(motion.x, KNOCK_RISE_SPEED, motion.z)
        }
        None => Vec3::ZERO,
    }
}

/// Applies the hurts other objects sent, in order, after the player has
/// moved.
///
/// Port of `PlayerGotHurt` (original/src/Player/MyGuy.c) with
/// `playerIsCurrent` false: the original's objects hurt the player while
/// they move, after the player's own move.
pub(super) fn hurt_players(
    mut hurts: MessageReader<HurtPlayer>,
    tuning: Res<PlayerTuning>,
    mut commands: Commands,
    velocities: Query<&Velocity, Without<Player>>,
    mut players: Query<PlayerData, With<Player>>,
) {
    // `Dying` is inserted through commands, so a death this tick is
    // remembered here.
    let mut killed = EntityHashSet::default();
    for hurt in hurts.read() {
        let Ok(mut player) = players.get_mut(hurt.player) else {
            continue;
        };
        let dying = player.dying || killed.contains(&player.entity);
        let shield = *player.shield;
        let PlayerDataItem {
            health, invincible, ..
        } = &mut player;
        match take_hurt(hurt, dying, shield, health, invincible) {
            HurtOutcome::Ignored => continue,
            HurtOutcome::Killed => {
                kill_player(&mut player, &mut commands, tuning.kill_delay);
                killed.insert(player.entity);
                if hurt.torch_if_killed {
                    commands.entity(player.entity).insert(Torched);
                }
            }
            HurtOutcome::Hurt => {
                if hurt.knock {
                    let velocity = knock_velocity(hurt.source, &velocities);
                    // `KnockPlayerBugOnButt` with `allowBall` false.
                    if *player.form == PlayerForm::Ball {
                        commands
                            .entity(player.entity)
                            .insert(DeferredKnock(velocity));
                    } else {
                        knock_on_butt(&mut player, velocity);
                    }
                }
            }
        }
        // Sound: EFFECT_OUCH at the player.
    }
}

/// Kills the player: the bug plays its death, and the player starts again
/// once the kill delay is over. A ball turns back into the bug first.
///
/// Port of `KillPlayer(true)` (original/src/Player/MyGuy.c).
pub(super) fn kill_player(player: &mut PlayerDataItem, commands: &mut Commands, kill_delay: f32) {
    if player.dying {
        return;
    }
    if *player.form == PlayerForm::Ball {
        become_bug(player, BugState::Death);
    } else {
        *player.state = BugState::Death;
        player.animated.restart();
    }
    commands
        .entity(player.entity)
        .insert(Dying { timer: kill_delay });
}

/// Knocks the player on its butt with the given velocity, facing where the
/// knock came from. A ball turns back into the bug first.
///
/// Port of `KnockPlayerBugOnButt` (original/src/Player/Player_Bug.c). The
/// original puts off a ball's knock until the end of the ball's move
/// (`gPlayerKnockOnButt`); every knock happens after the move here, so the
/// ball needs no special case.
pub fn knock_on_butt(player: &mut PlayerDataItem, velocity: Vec3) {
    if *player.form == PlayerForm::Ball {
        become_bug(player, BugState::UnRoll);
    }
    *player.state = BugState::KnockedOnButt;
    // Knocked again while already down, the fall starts over.
    player.animated.restart();
    **player.velocity = velocity;
    **player.steering = Vec2::ZERO;
    if velocity != Vec3::ZERO {
        let at = player.transform.translation.xz();
        let yaw =
            yaw_from_point_to_point(yaw_of(player.transform.rotation), at, at - velocity.xz());
        player.transform.rotation = Quat::from_rotation_y(yaw);
    }
}

/// Port of the invincibility countdown at the top of `MovePlayer_Bug`
/// (original/src/Player/Player_Bug.c) and `MovePlayer_Ball`
/// (original/src/Player/Player_Ball.c).
pub(super) fn count_down_invincibility(
    time: Res<Time>,
    mut timers: Query<&mut InvincibleTimer, With<Player>>,
) {
    for mut timer in &mut timers {
        if **timer > 0.0 {
            **timer = (**timer - time.delta_secs()).max(0.0);
        }
    }
}

/// Port of the countdown in `UpdatePlayerShield`
/// (original/src/Player/MyGuy.c). The shield's sparkles arrive with the
/// shield powerup.
pub(super) fn count_down_shield(
    time: Res<Time>,
    mut timers: Query<&mut ShieldTimer, With<Player>>,
) {
    for mut timer in &mut timers {
        if **timer > 0.0 {
            **timer = (**timer - time.delta_secs()).max(0.0);
            // Sound: EFFECT_SHIELD loops while the shield is up, and stops
            // here when it runs out.
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::collision::{CollisionKind, SolidSides, solid_object};

    fn hurt(damage: f32) -> HurtPlayer {
        HurtPlayer::new(Entity::PLACEHOLDER, None, damage)
    }

    #[test]
    fn a_hurt_takes_health_and_makes_the_player_invincible() {
        let mut health = Health(1.0);
        let mut invincible = InvincibleTimer(0.0);
        let outcome = take_hurt(
            &hurt(0.25),
            false,
            ShieldTimer(0.0),
            &mut health,
            &mut invincible,
        );
        assert_eq!(outcome, HurtOutcome::Hurt);
        assert_eq!(health, Health(0.75));
        assert_eq!(invincible, InvincibleTimer(INVINCIBILITY_DURATION));
    }

    #[test]
    fn an_invincible_player_is_not_hurt() {
        let mut health = Health(1.0);
        let mut invincible = InvincibleTimer(0.1);
        let outcome = take_hurt(
            &hurt(0.25),
            false,
            ShieldTimer(0.0),
            &mut health,
            &mut invincible,
        );
        assert_eq!(outcome, HurtOutcome::Ignored);
        assert_eq!(health, Health(1.0));
        assert_eq!(invincible, InvincibleTimer(0.1));
    }

    #[test]
    fn the_shield_stops_hurts_unless_they_override_it() {
        let shield = ShieldTimer(5.0);
        let mut health = Health(1.0);
        let mut invincible = InvincibleTimer(0.0);
        let outcome = take_hurt(&hurt(0.25), false, shield, &mut health, &mut invincible);
        assert_eq!(outcome, HurtOutcome::Ignored);
        assert_eq!(health, Health(1.0));
        assert_eq!(invincible, InvincibleTimer(0.0));

        let overriding = HurtPlayer {
            override_shield: true,
            ..hurt(0.25)
        };
        let outcome = take_hurt(&overriding, false, shield, &mut health, &mut invincible);
        assert_eq!(outcome, HurtOutcome::Hurt);
        assert_eq!(health, Health(0.75));
    }

    #[test]
    fn the_dead_and_harmless_hurts_change_nothing() {
        let mut health = Health(1.0);
        let mut invincible = InvincibleTimer(0.0);
        for (hurt, dying) in [(hurt(0.0), false), (hurt(0.5), true)] {
            let outcome = take_hurt(&hurt, dying, ShieldTimer(0.0), &mut health, &mut invincible);
            assert_eq!(outcome, HurtOutcome::Ignored);
        }
        assert_eq!(health, Health(1.0));
        assert_eq!(invincible, InvincibleTimer(0.0));
    }

    #[test]
    fn a_hurt_without_invincibility_lets_the_next_one_through() {
        let mut health = Health(1.0);
        let mut invincible = InvincibleTimer(0.0);
        let brief = HurtPlayer {
            invincible_for: 0.0,
            ..hurt(0.1)
        };
        take_hurt(
            &brief,
            false,
            ShieldTimer(0.0),
            &mut health,
            &mut invincible,
        );
        assert_eq!(invincible, InvincibleTimer(0.0));
        take_hurt(
            &brief,
            false,
            ShieldTimer(0.0),
            &mut health,
            &mut invincible,
        );
        assert!((health.0 - 0.8).abs() < 1e-6);
    }

    #[test]
    fn running_out_of_health_kills() {
        let mut health = Health(0.3);
        let mut invincible = InvincibleTimer(0.0);
        let outcome = take_hurt(
            &hurt(0.5),
            false,
            ShieldTimer(0.0),
            &mut health,
            &mut invincible,
        );
        assert_eq!(outcome, HurtOutcome::Killed);
        assert_eq!(health, Health(0.0));
    }

    fn spawn_player(world: &mut World, form: PlayerForm, health: f32) -> Entity {
        world
            .spawn((
                Player,
                Transform::from_xyz(100.0, 50.0, 100.0),
                solid_object(
                    vec![form.collision_box()],
                    CollisionKind::Player,
                    SolidSides::TOUCHABLE,
                ),
                form,
                Health(health),
            ))
            .id()
    }

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<PlayerTuning>();
        world.init_resource::<Messages<HurtPlayer>>();
        world
    }

    #[test]
    fn a_hurt_knocks_the_bug_on_its_butt_facing_the_source() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Bug, 1.0);
        // Something moving toward +X hits the bug.
        let source = world.spawn(Velocity(Vec3::new(500.0, 0.0, 0.0))).id();
        world.write_message(HurtPlayer::new(player, Some(source), 0.25));
        world
            .run_system_once(hurt_players)
            .expect("the system runs");

        assert_eq!(world.get::<Health>(player), Some(&Health(0.75)));
        assert_eq!(
            world.get::<BugState>(player),
            Some(&BugState::KnockedOnButt)
        );
        assert_eq!(
            world.get::<Velocity>(player),
            Some(&Velocity(Vec3::new(500.0, KNOCK_RISE_SPEED, 0.0)))
        );
        // It faces back toward −X, where the hit came from.
        let rotation = world.get::<Transform>(player).map(|t| t.rotation);
        let forward = rotation.map(|r| r * Vec3::NEG_Z);
        assert!(forward.is_some_and(|f| f.abs_diff_eq(Vec3::NEG_X, 1e-4)));
    }

    #[test]
    fn a_ball_hurt_by_others_is_knocked_after_its_next_move() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Ball, 1.0);
        world.write_message(HurtPlayer::new(player, None, 0.25));
        world
            .run_system_once(hurt_players)
            .expect("the system runs");
        // Hurt at once, but still the ball, with the knock waiting.
        assert_eq!(world.get::<Health>(player), Some(&Health(0.75)));
        assert_eq!(world.get::<PlayerForm>(player), Some(&PlayerForm::Ball));
        // No source: no knock velocity.
        assert_eq!(
            world.get::<DeferredKnock>(player),
            Some(&DeferredKnock(Vec3::ZERO))
        );
    }

    #[test]
    fn a_hurt_without_knock_leaves_the_state_alone() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Bug, 1.0);
        world.write_message(HurtPlayer {
            knock: false,
            ..HurtPlayer::new(player, None, 0.25)
        });
        world
            .run_system_once(hurt_players)
            .expect("the system runs");
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Stand));
        assert_eq!(world.get::<Health>(player), Some(&Health(0.75)));
    }

    #[test]
    fn running_out_of_health_kills_the_player_once() {
        let mut world = world();
        let player = spawn_player(&mut world, PlayerForm::Ball, 0.2);
        let other = spawn_player(&mut world, PlayerForm::Bug, 1.0);
        world.write_message(HurtPlayer::new(player, None, 0.5));
        world.write_message(HurtPlayer {
            invincible_for: 0.0,
            ..HurtPlayer::new(player, None, 0.5)
        });
        world
            .run_system_once(hurt_players)
            .expect("the system runs");

        assert_eq!(world.get::<Health>(player), Some(&Health(0.0)));
        assert_eq!(world.get::<PlayerForm>(player), Some(&PlayerForm::Bug));
        assert_eq!(world.get::<BugState>(player), Some(&BugState::Death));
        assert_eq!(world.get::<Dying>(player), Some(&Dying { timer: 4.0 }));
        // Each player has its own health.
        assert_eq!(world.get::<Health>(other), Some(&Health(1.0)));
        assert!(!world.entity(other).contains::<Dying>());
    }
}
