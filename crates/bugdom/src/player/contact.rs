//! What the player's collision tells the enemies it touched.
//!
//! `DoPlayerCollisionDetect` (original/src/Player/MyGuy.c) calls into each
//! enemy's code through a switch on its kind. Here the player's movement
//! sends these messages instead, and each enemy plugin answers the ones for
//! its own kind. The handlers arrive with the enemy base and the enemies
//! (docs/design/phase3-content.md §2.2).
//!
//! The player checks its collisions once per step of its move, so, as in
//! the original, one contact can be reported on several steps of a tick.

use bevy::prelude::*;

/// The player touched an enemy without bopping it (`PlayerHitEnemy`).
/// A spiked enemy has already hurt the player through
/// [`HurtPlayer`](super::HurtPlayer).
///
/// The flying bee dies when it stings the player: its handler kills it
/// when `spiked` is set, and then ignores the [`BallHitEnemy`] of the same
/// contact, as `PlayerHitEnemy` returns before the ball's switch.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct TouchedEnemy {
    pub player: Entity,
    pub enemy: Entity,
    /// The enemy was spiked (`CTYPE_SPIKED`) when the player touched it.
    pub spiked: bool,
}

/// The player's ball ran into an enemy (the `BallHit*` switch in
/// `PlayerHitEnemy`).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct BallHitEnemy {
    pub player: Entity,
    pub enemy: Entity,
    /// The ball's velocity at the start of the tick (`me->Delta`), in units
    /// per second.
    pub ball_velocity: Vec3,
    /// The ball's horizontal speed this tick (`me->Speed`), which the
    /// handlers compare with their knockdown speeds.
    pub ball_speed: f32,
}

/// The player landed on a boppable enemy (`PlayerBopEnemy`): its bottom
/// hit the enemy's top.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnemyBopped {
    pub player: Entity,
    pub enemy: Entity,
}
