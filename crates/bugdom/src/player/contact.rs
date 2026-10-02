//! What the player's collision and its kick tell the enemies and items
//! they touched.
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

/// How fast the kick sends most enemies flying, horizontally and upward,
/// in units per second (the 700 in `DoBugKick`). Some kinds use their own
/// speeds (the queen bee, the roach and the king ant use 300
/// horizontally); their handlers pick.
pub const KICK_SPEED: f32 = 700.0;
/// Health the kick takes from the enemies that it knocks over rather than
/// kills (`MY_KICK_ENEMY_DAMAGE`).
pub const KICK_ENEMY_DAMAGE: f32 = 0.4;

/// The bug's kick hit an enemy (`DoBugKick`'s switch on the skeleton type,
/// and `KillTick`). Each enemy plugin answers it for its own kind: most
/// are knocked on their butts or killed, sent flying along
/// [`Self::knock`].
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct EnemyKicked {
    pub player: Entity,
    pub enemy: Entity,
    /// The way the player faces, in x and z (`-sin(Rot.y)`, `-cos(Rot.y)`).
    pub direction: Vec2,
    /// [`KICK_ENEMY_DAMAGE`], for the handlers that take health.
    pub damage: f32,
}

impl EnemyKicked {
    /// The velocity the original gives a kicked enemy: `horizontal` along
    /// the kick's direction and `rise` upward, in units per second (for
    /// most kinds [`KICK_SPEED`] for both).
    pub fn knock(&self, horizontal: f32, rise: f32) -> Vec3 {
        Vec3::new(
            self.direction.x * horizontal,
            rise,
            self.direction.y * horizontal,
        )
    }
}

/// The bug's kick hit a kickable object that isn't an enemy (`KickNut`,
/// `KickLadyBugBox`, `KickKingWaterPipe`). The item's own plugin answers
/// it.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ItemKicked {
    pub player: Entity,
    pub item: Entity,
    /// The way the player faces, in x and z.
    pub direction: Vec2,
}
