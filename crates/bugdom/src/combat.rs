//! Components for taking and dealing damage that the player and the enemies
//! share.
//!
//! The player's rules for getting hurt are in `player/health.rs`; the
//! enemies' arrive with the enemy base.

use bevy::prelude::*;

/// How much health an entity has left (`gMyHealth` for the player, `Health`
/// for enemies). The player's runs from 0 to 1; enemies use their own
/// scales.
#[derive(Component, Debug, Clone, Copy, PartialEq, Deref, DerefMut)]
pub struct Health(pub f32);

impl Default for Health {
    /// Full health on the player's scale.
    fn default() -> Self {
        Self(1.0)
    }
}

impl Health {
    /// Adds health, up to `max`. Port of `GetHealth`
    /// (original/src/Screens/Infobar.c) for the player, whose `max` is 1.
    pub fn gain(&mut self, amount: f32, max: f32) {
        self.0 = (self.0 + amount).min(max);
    }

    /// Takes health away, stopping at zero, and returns whether none is
    /// left. The health part of `LoseHealth`
    /// (original/src/Screens/Infobar.c).
    pub fn lose(&mut self, amount: f32) -> bool {
        self.0 -= amount;
        if self.0 <= 0.0 {
            self.0 = 0.0;
            true
        } else {
            false
        }
    }
}

/// How much damage an entity deals to what touches it (`ObjNode::Damage`).
/// The player reads it from hurting objects (`CTYPE_HURTME`), spiked
/// enemies and ball-time drains.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct Damage(pub f32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaining_health_stops_at_the_maximum() {
        let mut health = Health(0.5);
        health.gain(0.3, 1.0);
        assert!((health.0 - 0.8).abs() < 1e-6);
        health.gain(0.5, 1.0);
        assert_eq!(health, Health(1.0));
    }

    #[test]
    fn losing_health_stops_at_zero_and_says_so() {
        let mut health = Health(0.5);
        assert!(!health.lose(0.25));
        assert_eq!(health, Health(0.25));
        assert!(health.lose(0.25));
        assert_eq!(health, Health(0.0));
        let mut health = Health(0.1);
        assert!(health.lose(1.0));
        assert_eq!(health, Health(0.0));
    }
}
