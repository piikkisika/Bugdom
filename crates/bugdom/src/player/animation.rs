//! The bug's animation, which follows its [`BugState`].
//!
//! The animation numbers and blend rates come from the
//! `MorphToSkeletonAnim` and `SetSkeletonAnim` calls in
//! original/src/Player/Player_Bug.c; the movement code only changes the
//! state.

use bevy::prelude::*;

use super::bug::{BugState, PlayerSpeed};
use crate::skeleton::SkeletonAnimator;

/// The bug skeleton's animations (`PLAYER_ANIM_*` in
/// original/src/Headers/myguy.h).
mod anim {
    pub const STAND: usize = 0;
    pub const WALK: usize = 1;
    pub const JUMP: usize = 5;
    pub const FALL: usize = 6;
    pub const LAND: usize = 7;
}

/// Walk animation speed per unit of walking speed (`MovePlayerBug_Walk`).
const WALK_ANIM_SPEED_PER_SPEED: f32 = 0.006;

/// The state the animation was last started for. A new bug starts standing,
/// which is also the animation a skeleton starts with.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnimatedBugState(BugState);

fn animation_for(state: BugState) -> usize {
    match state {
        BugState::Stand => anim::STAND,
        BugState::Walk => anim::WALK,
        BugState::Jump => anim::JUMP,
        BugState::Fall => anim::FALL,
        BugState::Land => anim::LAND,
    }
}

/// How fast the blend into a state's animation runs, in blends per second,
/// for each change of state the movement code makes.
fn morph_rate(from: BugState, to: BugState) -> f32 {
    match (from, to) {
        (BugState::Jump, BugState::Land) => 7.0,
        (_, BugState::Land) => 9.0,
        (_, BugState::Jump) => 9.0,
        (_, BugState::Walk) => 9.0,
        (_, BugState::Fall) => 4.0,
        (_, BugState::Stand) => 6.0,
    }
}

/// Starts the animation of a new state, and paces the walk animation to
/// the walking speed.
pub fn animate_bug(
    mut bugs: Query<(
        &BugState,
        &mut AnimatedBugState,
        &PlayerSpeed,
        &mut SkeletonAnimator,
    )>,
) {
    for (&state, mut animated, speed, mut animator) in &mut bugs {
        if animated.0 != state {
            animator.morph_to(animation_for(state), morph_rate(animated.0, state));
            animated.0 = state;
        } else if state == BugState::Walk {
            animator.speed = **speed * WALK_ANIM_SPEED_PER_SPEED;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_blends_faster_after_a_fall_than_after_a_jump() {
        assert_eq!(morph_rate(BugState::Fall, BugState::Land), 9.0);
        assert_eq!(morph_rate(BugState::Jump, BugState::Land), 7.0);
    }
}
