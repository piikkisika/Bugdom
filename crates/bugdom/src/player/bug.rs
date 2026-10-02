//! The bug form's movement.
//!
//! [`BugState`] is the single state that both movement and animation derive
//! from. In the original the animation number *is* the state
//! (`MovePlayer_Bug` dispatches on `Skeleton->AnimNum`); here [`move_bug`]
//! reads and writes the state and never touches the animator, and
//! `animate_bug` (in `animation.rs`) starts the animation that matches it.

use bevy::prelude::*;

use super::animation::AnimatedBugState;
use super::ball::become_ball;
use super::movement::{Motion, MotionContext, PlayerData};
use super::{Player, PlayerForm, PlayerModel};
use crate::collision::TriggerHit;
use crate::input::Action;
use crate::math::turn_toward;
use crate::skeleton::SkeletonAnimator;

/// What the bug is doing. The variants carry no data; anything a state needs
/// lives in its own component.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[require(AnimatedBugState)]
pub enum BugState {
    #[default]
    Stand,
    Walk,
    /// Rolling up into the ball. The ball keeps this state, because it keeps
    /// the last pose of the roll-up.
    RollUp,
    /// Unrolling after being the ball.
    UnRoll,
    Jump,
    Fall,
    Land,
}

/// One tick of the bug's movement.
struct Bug<'a> {
    motion: Motion<'a>,
    state: BugState,
    animator: &'a SkeletonAnimator,
    /// The roll-up has finished: the bug becomes the ball.
    rolled_up: bool,
}

/// Moves the player's bug for one tick.
///
/// Port of `MovePlayer_Bug` (original/src/Player/Player_Bug.c).
pub fn move_bug(
    context: MotionContext,
    mut trigger_hits: MessageWriter<TriggerHit>,
    mut players: Query<(PlayerData, &PlayerModel), With<Player>>,
    animators: Query<&SkeletonAnimator>,
) {
    for (mut player, model) in &mut players {
        if *player.form != PlayerForm::Bug {
            continue;
        }
        let Ok(animator) = animators.get(model.0) else {
            continue;
        };
        let mut bug = Bug {
            motion: context.motion(&player),
            state: *player.state,
            animator,
            rolled_up: false,
        };
        bug.tick();

        let (state, rolled_up) = (bug.state, bug.rolled_up);
        bug.motion.store(&mut player, &mut trigger_hits);
        player.state.set_if_neq(state);
        if rolled_up {
            become_ball(&mut player);
        }
    }
}

impl Bug<'_> {
    fn tick(&mut self) {
        match self.state {
            BugState::Stand => self.stand(),
            BugState::Walk => self.walk(),
            BugState::RollUp => self.roll_up(),
            BugState::UnRoll => self.unroll(),
            BugState::Jump => self.jump(),
            BugState::Fall => self.fall(),
            BugState::Land => self.land(),
        }
        if !self.rolled_up {
            self.update();
        }
    }

    /// Port of `MovePlayerBug_Stand`. Steering is read last, so that a jump
    /// starts cleanly.
    fn stand(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion.apply_friction_and_gravity(tuning.friction);
        self.motion.move_and_collide(false);
        if self.state == BugState::Stand && self.motion.speed > tuning.walk_speed {
            self.state = BugState::Walk;
        }
        self.control(1.0);
    }

    /// Port of `MovePlayerBug_Walk`.
    fn walk(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.control(1.0);
        self.motion.apply_friction_and_gravity(tuning.friction);
        self.motion.move_and_collide(false);
        if self.state == BugState::Walk && self.motion.speed < tuning.walk_speed {
            self.state = BugState::Stand;
        }
    }

    /// Port of `MovePlayerBug_RollUp`: the bug stops and can't be steered,
    /// and becomes the ball when the animation ends.
    fn roll_up(&mut self) {
        if self.animator.has_stopped {
            self.rolled_up = true;
            return;
        }
        let tuning = &self.motion.tuning.bug;
        self.motion
            .apply_friction_and_gravity(tuning.super_friction);
        self.motion.move_and_collide(true);
    }

    /// Port of `MovePlayerBug_UnRoll`.
    fn unroll(&mut self) {
        if self.animator.has_stopped {
            self.state = BugState::Stand;
        }
        let tuning = &self.motion.tuning.bug;
        self.motion
            .apply_friction_and_gravity(tuning.super_friction);
        self.motion.move_and_collide(true);
    }

    /// Port of `MovePlayerBug_Jump`. Aiming at boppable enemies arrives with
    /// the enemies.
    fn jump(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion.apply_friction_and_gravity(tuning.friction);
        self.motion.move_and_collide(false);
        if self.motion.ground.on_ground {
            self.motion.velocity.y = 0.0;
            if self.state == BugState::Jump {
                self.state = BugState::Land;
            }
        } else if self.motion.velocity.y < tuning.jump_to_fall_speed && self.state == BugState::Jump
        {
            self.state = BugState::Fall;
        }
        self.control(1.0);
    }

    /// Port of `MovePlayerBug_Fall`.
    fn fall(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion.apply_friction_and_gravity(tuning.friction);
        self.motion.move_and_collide(false);
        if self.state == BugState::Fall && self.motion.ground.on_ground {
            self.state = BugState::Land;
            // Landing from a fall slows the bug down.
            self.motion.velocity.x *= tuning.fall_landing_slowdown;
            self.motion.velocity.z *= tuning.fall_landing_slowdown;
        }
        self.control(1.0);
    }

    /// Port of `MovePlayerBug_Land`: no control until the landing has
    /// blended in.
    fn land(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion.apply_friction_and_gravity(tuning.friction);
        self.motion.move_and_collide(true);
        if !self.animator.is_morphing() {
            self.state = BugState::Stand;
        }
    }

    /// Turns the bug toward its motion. Port of `UpdatePlayer_Bug`.
    fn update(&mut self) {
        let m = &mut self.motion;
        if !m.player_relative_keys() {
            let from = m.coord.xz();
            let target = from + m.velocity.xz();
            let max_turn = m.speed * m.tuning.bug.turn_rate_per_speed * m.dt;
            m.yaw = turn_toward(m.yaw, from, target, max_turn).0;
        }
    }

    /// Reads the steering and the jump button.
    ///
    /// Port of `DoPlayerControl_Bug`. `slug_factor` scales the steering
    /// (swimming uses less). Kicking arrives with the kickable objects.
    fn control(&mut self, slug_factor: f32) {
        let m = &mut self.motion;
        let tuning = &m.tuning.bug;
        let on_ground =
            m.ground.on_ground || m.ground.dist_to_floor < tuning.control_ground_distance;

        let mut steering = m.input.steering(m.dt) * tuning.steering_scale;
        if !on_ground {
            steering *= tuning.airborne_steering;
        }
        m.steering = if m.player_relative_keys() {
            Vec2::ZERO
        } else {
            steering * (tuning.steering_accel * slug_factor)
        };

        // The original allows a jump from standing or walking even in the
        // air, e.g. right after walking off a ledge.
        if matches!(self.state, BugState::Stand | BugState::Walk)
            && m.input.just_pressed(Action::Jump)
        {
            self.state = BugState::Jump;
            m.velocity.y = tuning.jump_speed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::movement::bench::{Bench, START};
    use super::*;
    use crate::collision::{BoxTarget, CollisionKind, SolidSides};
    use crate::input::ControlInput;

    /// Runs the bug's movement for `ticks` ticks from the Lawn's start,
    /// with the same input every tick (apart from presses, which only the
    /// first tick sees), calling `each` after every tick.
    fn simulate(ticks: usize, held: &[Action], pressed: &[Action], each: impl FnMut(&Bug)) {
        let animator = SkeletonAnimator::default();
        simulate_with(&animator, BugState::Stand, &[], ticks, held, pressed, each);
    }

    /// [`simulate`] from a given state, with solid objects around and with
    /// a given animator.
    fn simulate_with(
        animator: &SkeletonAnimator,
        state: BugState,
        candidates: &[BoxTarget],
        ticks: usize,
        held: &[Action],
        pressed: &[Action],
        mut each: impl FnMut(&Bug),
    ) {
        let bench = Bench::lawn();
        let first = ControlInput::for_tests(held, pressed);
        let rest = ControlInput::for_tests(held, &[]);
        let mut bug = Bug {
            motion: bench.motion(PlayerForm::Bug, &first, candidates),
            state,
            animator,
            rolled_up: false,
        };
        for tick in 0..ticks {
            if tick > 0 {
                bug.motion = bug.motion.next_tick(&rest);
            }
            bug.tick();
            each(&bug);
        }
    }

    #[test]
    fn standing_still_stays_on_the_floor() {
        simulate(120, &[], &[], |bug| {
            assert_eq!(bug.state, BugState::Stand);
            let m = &bug.motion;
            let floor = m.map.floor_height(m.coord.x, m.coord.z);
            assert!((m.coord.y - floor).abs() < 0.01);
        });
    }

    #[test]
    fn a_jump_rises_about_v_squared_over_2g() {
        let mut start = None;
        let mut apex = f32::MIN;
        let mut states = Vec::new();
        simulate(90, &[], &[Action::Jump], |bug| {
            start.get_or_insert(bug.motion.coord.y);
            apex = apex.max(bug.motion.coord.y);
            if states.last() != Some(&bug.state) {
                states.push(bug.state);
            }
        });
        // The first tick both jumps and rises one tick's worth.
        let height = apex - start.unwrap_or(0.0);
        let ideal = 2000.0f32.powi(2) / (2.0 * 5200.0);
        assert!((height - ideal).abs() < 40.0, "jumped {height}");
        // The test's animator never blends, so the landing ends at once.
        assert_eq!(
            states,
            [
                BugState::Jump,
                BugState::Fall,
                BugState::Land,
                BugState::Stand
            ]
        );
    }

    #[test]
    fn walking_with_keys_reaches_the_top_speed() {
        let mut top = 0.0f32;
        simulate(180, &[Action::Forward], &[], |bug| {
            top = top.max(bug.motion.speed);
        });
        assert!((top - 700.0).abs() < 1.0, "top speed {top}");
    }

    #[test]
    fn walking_into_a_rock_stops_at_its_side() {
        // A wall-like rock across the bug's path, 300 units ahead (−Z).
        let rock =
            crate::collision::CollisionBox::new(10_000.0, -10_000.0, -500.0, 500.0, 50.0, -50.0)
                .at(Vec3::new(START.x, 0.0, START.y - 300.0));
        let target = BoxTarget {
            entity: Entity::from_raw_u32(7).unwrap(),
            kinds: CollisionKind::Misc.into(),
            solid: SolidSides::ALL,
            boxes: vec![rock],
            old_boxes: vec![rock],
            velocity: Vec3::ZERO,
            trigger: None,
        };
        let animator = SkeletonAnimator::default();
        let mut nearest = f32::MAX;
        simulate_with(
            &animator,
            BugState::Stand,
            &[target],
            180,
            &[Action::Forward],
            &[],
            |bug| nearest = nearest.min(bug.motion.coord.z),
        );
        // The bug's back is its −Z side, 42 units from its middle.
        assert!(
            (nearest - (rock.front + 42.0 + 1.0)).abs() < 0.01,
            "got to {nearest}"
        );
    }

    #[test]
    fn rolling_up_ignores_the_controls_and_ends_in_the_ball() {
        let mut animator = SkeletonAnimator::default();
        let mut speeds = Vec::new();
        simulate_with(
            &animator,
            BugState::RollUp,
            &[],
            10,
            &[Action::Forward],
            &[],
            |bug| {
                assert!(!bug.rolled_up);
                speeds.push(bug.motion.speed);
            },
        );
        assert!(speeds.iter().all(|&s| s == 0.0), "{speeds:?}");

        animator.has_stopped = true;
        simulate_with(&animator, BugState::RollUp, &[], 1, &[], &[], |bug| {
            assert!(bug.rolled_up);
        });
    }

    #[test]
    fn unrolling_ends_standing() {
        let mut animator = SkeletonAnimator::default();
        animator.has_stopped = true;
        simulate_with(&animator, BugState::UnRoll, &[], 1, &[], &[], |bug| {
            assert_eq!(bug.state, BugState::Stand);
        });
    }
}
