//! The bug's animation, which follows its [`BugState`].
//!
//! The animation numbers and blend rates come from the
//! `MorphToSkeletonAnim` and `SetSkeletonAnim` calls in
//! original/src/Player/Player_Bug.c and original/src/Player/Player_Control.c;
//! the movement code only changes the state.

use bevy::prelude::*;

use super::bug::BugState;
use super::{Dying, PlayerModel, PlayerSpeed};
use crate::skeleton::SkeletonAnimator;

/// The bug skeleton's animations (`PLAYER_ANIM_*` in
/// original/src/Headers/myguy.h).
mod anim {
    pub const STAND: usize = 0;
    pub const WALK: usize = 1;
    pub const ROLLUP: usize = 2;
    pub const UNROLL: usize = 3;
    pub const KICK: usize = 4;
    pub const JUMP: usize = 5;
    pub const FALL: usize = 6;
    pub const LAND: usize = 7;
    pub const SWIM: usize = 8;
    pub const FALL_ON_BUTT: usize = 9;
    pub const DEATH: usize = 13;
}

/// Walk animation speed per unit of walking speed (`MovePlayerBug_Walk`).
const WALK_ANIM_SPEED_PER_SPEED: f32 = 0.006;
/// Swim animation speed while swimming and while drowning
/// (`MovePlayerBug_Swim`, `DrownInLiquid`).
const SWIM_ANIM_SPEED: f32 = 1.5;
const DROWN_ANIM_SPEED: f32 = 0.7;

/// The state the animation was last started for, or `None` when the
/// current state's animation must start again from the beginning. A new
/// bug starts standing, which is also the animation a skeleton starts with.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimatedBugState(Option<BugState>);

impl Default for AnimatedBugState {
    fn default() -> Self {
        Self(Some(BugState::Stand))
    }
}

impl AnimatedBugState {
    /// Starts the state's animation again even if the state is unchanged,
    /// e.g. when the morph button is pressed during a roll-up.
    pub fn restart(&mut self) {
        self.0 = None;
    }
}

fn animation_for(state: BugState) -> usize {
    match state {
        BugState::Stand => anim::STAND,
        BugState::Walk => anim::WALK,
        BugState::RollUp => anim::ROLLUP,
        BugState::UnRoll => anim::UNROLL,
        BugState::Kick => anim::KICK,
        BugState::Jump => anim::JUMP,
        BugState::Fall => anim::FALL,
        BugState::Land => anim::LAND,
        BugState::Swim => anim::SWIM,
        BugState::KnockedOnButt => anim::FALL_ON_BUTT,
        BugState::Death => anim::DEATH,
    }
}

/// How fast the blend into a state's animation runs, in blends per second,
/// for each change of state; `None` where the original starts the animation
/// at once (`SetSkeletonAnim`, or a new skeleton when the ball unrolls).
fn morph_rate(from: Option<BugState>, to: BugState) -> Option<f32> {
    match (from, to) {
        // Restarted by every knock, even from a ball (`KnockPlayerBugOnButt`).
        (_, BugState::KnockedOnButt) => Some(3.0),
        (_, BugState::RollUp | BugState::UnRoll | BugState::Death) => None,
        (Some(BugState::UnRoll), BugState::Stand) => None,
        // `MovePlayerBug_Kick` ends with `SetSkeletonAnim`.
        (Some(BugState::Kick), BugState::Stand) => None,
        (None, _) => None,
        (_, BugState::Swim) => Some(5.0),
        (Some(BugState::Swim), BugState::Stand) => Some(5.0),
        (Some(BugState::Jump), BugState::Land) => Some(7.0),
        (_, BugState::Land) => Some(9.0),
        (_, BugState::Jump) => Some(9.0),
        (_, BugState::Kick) => Some(9.0),
        (_, BugState::Walk) => Some(9.0),
        (_, BugState::Fall) => Some(4.0),
        (_, BugState::Stand) => Some(6.0),
    }
}

/// Starts the animation of a new state, and paces the walk animation to
/// the walking speed.
pub fn animate_bug(
    mut bugs: Query<(
        &BugState,
        &mut AnimatedBugState,
        &PlayerSpeed,
        &PlayerModel,
        Has<Dying>,
    )>,
    mut animators: Query<&mut SkeletonAnimator>,
) {
    for (&state, mut animated, speed, model, dying) in &mut bugs {
        let Ok(mut animator) = animators.get_mut(model.0) else {
            continue;
        };
        if animated.0 != Some(state) {
            let anim = animation_for(state);
            match morph_rate(animated.0, state) {
                Some(rate) => animator.morph_to(anim, rate),
                None => animator.set_anim(anim),
            }
            animated.0 = Some(state);
        } else if state == BugState::Walk {
            animator.speed = **speed * WALK_ANIM_SPEED_PER_SPEED;
        } else if state == BugState::Swim {
            animator.speed = if dying {
                DROWN_ANIM_SPEED
            } else {
                SWIM_ANIM_SPEED
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_blends_faster_after_a_fall_than_after_a_jump() {
        assert_eq!(morph_rate(Some(BugState::Fall), BugState::Land), Some(9.0));
        assert_eq!(morph_rate(Some(BugState::Jump), BugState::Land), Some(7.0));
    }

    #[test]
    fn rolling_up_and_unrolling_start_without_a_blend() {
        assert_eq!(morph_rate(Some(BugState::Walk), BugState::RollUp), None);
        assert_eq!(morph_rate(None, BugState::RollUp), None);
        assert_eq!(morph_rate(Some(BugState::RollUp), BugState::UnRoll), None);
        assert_eq!(morph_rate(Some(BugState::UnRoll), BugState::Stand), None);
        assert_eq!(morph_rate(Some(BugState::Walk), BugState::Stand), Some(6.0));
    }

    #[test]
    fn a_kick_blends_in_and_ends_at_once() {
        assert_eq!(morph_rate(Some(BugState::Walk), BugState::Kick), Some(9.0));
        assert_eq!(morph_rate(Some(BugState::Kick), BugState::Stand), None);
    }

    #[test]
    fn a_knock_always_blends_and_death_never_does() {
        assert_eq!(morph_rate(None, BugState::KnockedOnButt), Some(3.0));
        assert_eq!(
            morph_rate(Some(BugState::Walk), BugState::KnockedOnButt),
            Some(3.0)
        );
        assert_eq!(
            morph_rate(Some(BugState::KnockedOnButt), BugState::Stand),
            Some(6.0)
        );
        assert_eq!(morph_rate(Some(BugState::Walk), BugState::Death), None);
        assert_eq!(morph_rate(None, BugState::Death), None);
    }
}
