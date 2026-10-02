//! The ball form: changing between bug and ball, the ball's movement and
//! spin, nitro boosts, and the ball timer.
//!
//! Port of original/src/Player/Player_Ball.c, `CheckPlayerMorph`
//! (original/src/Player/Player_Control.c) and the ball timer in
//! original/src/Screens/Infobar.c.
//!
//! The original replaces the bug's skeleton object with a new object whose
//! mesh is the bug's last roll-up pose, frozen. Here the player stays one
//! entity: the bug's skeleton keeps that pose because the roll-up animation
//! has stopped, and the model is turned about the ball's centre
//! (`pose_player_model` in `mod.rs`).

use std::f32::consts::TAU;

use bevy::prelude::*;

use super::bug::BugState;
use super::effects::{NitroTrail, PlayerEffects};
use super::health::{kill_player, knock_on_butt};
use super::movement::{Motion, MotionContext, PlayerData, PlayerDataItem, PlayerMessages};
use super::{Player, PlayerForm, PlayerTuning};
use crate::input::Action;
use crate::math::turn_toward;
use crate::terrain::{LayerKind, TerrainMap};

/// Distance from the ball's centre down to the floor it rests on
/// (`PLAYER_BALL_FOOTOFFSET`). The ball's centre is this far above where
/// the bug's feet were.
pub const PLAYER_BALL_FOOT_OFFSET: f32 = 50.0;
/// Distance from the ball's centre up to its top (`PLAYER_BALL_HEADOFFSET`).
pub const PLAYER_BALL_HEAD_OFFSET: f32 = 45.0;

/// How much ball time a player has left, from 0 to 1 (`gBallTimer`). The
/// ball drains it, and the player can't become the ball without it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Deref, DerefMut)]
pub struct BallTime(pub f32);

impl Default for BallTime {
    /// A new game starts with a full timer (`InitInventoryForGame`).
    fn default() -> Self {
        Self(1.0)
    }
}

/// The ball's roll about its own x axis: the angle (`Rot.x`), and its rate
/// in radians per second (`RotDeltaX`).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct BallSpin {
    pub angle: f32,
    pub rate: f32,
}

/// Seconds left of the current nitro boost (`gNitroTimer`). Another boost
/// can't start until it runs out.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct Nitro(pub f32);

/// Turns the bug into the ball once it has rolled up, keeping its velocity
/// and heading.
///
/// Port of `InitPlayer_Ball` (original/src/Player/Player_Ball.c).
pub(super) fn become_ball(player: &mut PlayerDataItem) {
    player.set_form(PlayerForm::Ball);
    player.transform.translation.y += PLAYER_BALL_FOOT_OFFSET;
    *player.spin = BallSpin::default();
    **player.nitro = 0.0;
}

/// Turns the ball back into the bug, which starts in `state`: unrolling,
/// or dying.
///
/// Port of `InitPlayer_Bug` (original/src/Player/Player_Bug.c) as called
/// with an old player object. The original then
/// records the new position as the old one before anything moves, which
/// [`PreviousPosition`](crate::physics::PreviousPosition) does here.
pub(super) fn become_bug(player: &mut PlayerDataItem, state: BugState) {
    player.set_form(PlayerForm::Bug);
    player.transform.translation.y -= PLAYER_BALL_FOOT_OFFSET;
    **player.previous = player.transform.translation;
    *player.state = state;
    player.animated.restart();
    *player.spin = BallSpin::default();
}

/// Turns the ball that fell into a liquid into the bug, swimming where the
/// ball's move left it.
///
/// Port of `InitPlayer_Bug` (original/src/Player/Player_Bug.c) as called
/// by `DoPlayerMovementAndCollision` with `PLAYER_ANIM_SWIM`.
fn become_swimming_bug(player: &mut PlayerDataItem) {
    player.set_form(PlayerForm::Bug);
    *player.state = BugState::Swim;
    player.animated.restart();
    *player.spin = BallSpin::default();
}

/// Whether the bug's head would fit under the ceiling if the ball unrolled
/// at `coord`. The ball is shorter than the bug, so it can reach places with
/// lower ceilings.
///
/// Port of `BallHasHeadroomToMorphToBug` (original/src/Player/Player_Ball.c).
pub fn has_headroom_to_unroll(map: &TerrainMap, tuning: &PlayerTuning, coord: Vec3) -> bool {
    if map.ceiling.is_none() {
        return true;
    }
    let ceiling = map.height_at(coord.x, coord.z, LayerKind::Ceiling).0;
    let floor = map.floor_height(coord.x, coord.z);
    ceiling - floor > PlayerForm::Bug.collision_box().top + tuning.ball.unroll_headroom
}

/// Changes form when the morph button is pressed and there is ball time
/// left. The bug rolls up only from standing, walking, rolling up (which
/// starts the roll-up again) or unrolling.
///
/// Port of `CheckPlayerMorph` (original/src/Player/Player_Control.c), which
/// the original calls once a frame before anything moves. The morph sound
/// arrives with the sound effects.
pub fn check_player_morph(
    map: Res<TerrainMap>,
    tuning: Res<PlayerTuning>,
    mut players: Query<PlayerData, With<Player>>,
) {
    for mut player in &mut players {
        if **player.ball_time <= 0.0 || !player.input.just_pressed(Action::MorphPlayer) {
            continue;
        }
        match *player.form {
            PlayerForm::Ball => {
                if has_headroom_to_unroll(&map, &tuning, player.transform.translation) {
                    become_bug(&mut player, BugState::UnRoll);
                }
            }
            PlayerForm::Bug => {
                if matches!(
                    *player.state,
                    BugState::Stand | BugState::Walk | BugState::RollUp | BugState::UnRoll
                ) {
                    *player.state = BugState::RollUp;
                    player.animated.restart();
                }
            }
        }
    }
}

/// One tick of the ball's movement.
struct Ball<'a> {
    motion: Motion<'a>,
    spin: BallSpin,
    nitro: f32,
    ball_time: f32,
    /// The ball ran out of time and turns back into the bug.
    unrolled: bool,
    /// A boost started this tick.
    boost_started: bool,
    /// The boost leaves its trail this tick.
    leaves_trail: bool,
}

/// Moves the player's ball for one tick.
///
/// Port of `MovePlayer_Ball` (original/src/Player/Player_Ball.c).
pub fn move_ball(
    mut commands: Commands,
    context: MotionContext,
    mut messages: PlayerMessages,
    mut effects: PlayerEffects,
    mut players: Query<(PlayerData, &mut NitroTrail), With<Player>>,
) {
    for (mut player, mut trail) in &mut players {
        if *player.form != PlayerForm::Ball {
            continue;
        }
        let mut ball = Ball {
            motion: context.motion(&player),
            spin: *player.spin,
            nitro: **player.nitro,
            ball_time: **player.ball_time,
            unrolled: false,
            boost_started: false,
            leaves_trail: false,
        };
        ball.tick();

        if ball.boost_started {
            trail.start();
        }
        if ball.leaves_trail {
            let m = &ball.motion;
            effects.nitro_trail(&mut trail, m.old_coord, m.coord, m.dt);
        }
        if ball.nitro <= 0.0 {
            trail.end();
        }

        let (spin, nitro, unrolled) = (ball.spin, ball.nitro, ball.unrolled);
        let swimming = ball.motion.form == PlayerForm::Bug;
        let (knocked, died) = (ball.motion.knocked, ball.motion.died);
        player.ball_time.set_if_neq(BallTime(ball.ball_time));
        ball.motion
            .store(&mut player, &mut commands, &mut messages, &mut effects);
        player.spin.set_if_neq(spin);
        player.nitro.set_if_neq(Nitro(nitro));
        if swimming {
            become_swimming_bug(&mut player);
        } else if unrolled {
            become_bug(&mut player, BugState::UnRoll);
        }
        // What the ball ran into hurt it during the move; the ball only
        // turns into the knocked bug now that its move is over
        // (`gPlayerKnockOnButt`, original/src/Player/Player_Ball.c).
        if died {
            kill_player(&mut player, &mut commands, context.tuning().kill_delay);
        } else if let Some(velocity) = knocked {
            knock_on_butt(&mut player, velocity);
        }
    }
}

impl Ball<'_> {
    fn tick(&mut self) {
        self.control();
        let tuning = self.motion.tuning;
        self.motion.apply_friction_and_gravity(tuning.ball.friction);
        self.motion.move_and_collide(false);
        // Ball-time drains touched during the move (`LoseBallTime` in
        // `DoPlayerCollisionDetect`), which can unroll the ball.
        let drained = std::mem::take(&mut self.motion.ball_time_drained);
        if drained > 0.0 {
            self.lose_ball_time(drained);
        }
        // `LeaveNitroTrail`, which the original skips if the move killed
        // the ball.
        if self.nitro > 0.0 {
            self.leaves_trail = !self.motion.died;
            self.nitro = (self.nitro - self.motion.dt).max(0.0);
        }
        self.update();
    }

    /// Port of `UpdatePlayer_Ball`. Being knocked over arrives with the
    /// enemies. A ball that has just turned into the swimming bug still
    /// loses ball time this tick, as in the original.
    fn update(&mut self) {
        if self.motion.form == PlayerForm::Ball {
            self.spin_ball();
        }
        // `ProcessBallTimer`
        let drain = self.motion.tuning.ball.ball_time_drain * self.motion.dt;
        self.lose_ball_time(drain);
    }

    /// Reads the steering, and the kick and jump buttons, which start a
    /// nitro boost.
    ///
    /// Port of `DoPlayerControl_Ball`. The boost sound arrives with the
    /// sound effects.
    fn control(&mut self) {
        let m = &mut self.motion;
        let tuning = &m.tuning.ball;
        m.steering = if m.player_relative_keys() {
            Vec2::ZERO
        } else {
            m.input.steering(m.dt) * (tuning.steering_scale * tuning.steering_accel)
        };

        if self.nitro <= 0.0
            && (m.input.just_pressed(Action::Kick) || m.input.just_pressed(Action::Jump))
        {
            m.speed = tuning.motion.max_speed;
            m.velocity.x *= tuning.nitro_boost;
            m.velocity.z *= tuning.nitro_boost;
            self.nitro = tuning.nitro_duration;
            self.boost_started = true;
            // Not through `lose_ball_time`: a boost alone never unrolls the
            // ball, though the timer's drain right after can.
            self.ball_time -= tuning.nitro_ball_time;
        }

        // The modern port keeps a boost from overflowing the velocity.
        let horizontal = m.velocity.xz();
        if horizontal.length() > tuning.motion.max_speed {
            let capped = horizontal.normalize() * tuning.motion.max_speed;
            m.velocity.x = capped.x;
            m.velocity.z = capped.y;
        }
    }

    /// Rolls the ball as fast as it moves while on the ground, lets the roll
    /// die down in the air, and heads it the way it is going.
    ///
    /// Port of `SpinBall`.
    fn spin_ball(&mut self) {
        let m = &mut self.motion;
        let tuning = &m.tuning.ball;
        if m.ground.on_ground {
            self.spin.rate = -m.speed * tuning.spin_per_speed;
        } else if self.spin.rate < 0.0 {
            self.spin.rate = (self.spin.rate + tuning.air_spin_decay * m.dt).min(0.0);
        }
        self.spin.angle = (self.spin.angle + self.spin.rate * m.dt).rem_euclid(TAU);

        if !m.player_relative_keys() {
            let from = m.coord.xz();
            let target = from + m.velocity.xz();
            m.yaw = turn_toward(m.yaw, from, target, tuning.turn_rate * m.dt).0;
        }
    }

    /// Port of `LoseBallTime` (original/src/Screens/Infobar.c). Once the time
    /// runs out the ball unrolls, unless the bug wouldn't fit under the
    /// ceiling; then it stays a ball until it would.
    fn lose_ball_time(&mut self, amount: f32) {
        self.ball_time -= amount;
        if self.ball_time <= 0.0 {
            self.ball_time = 0.0;
            if self.motion.form == PlayerForm::Ball
                && has_headroom_to_unroll(self.motion.map, self.motion.tuning, self.motion.coord)
            {
                self.unrolled = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::movement::bench::{Bench, DT};
    use super::*;
    use crate::input::ControlInput;

    /// Runs the ball's movement for `ticks` ticks from the Lawn's start,
    /// with the same input every tick (apart from presses, which only the
    /// first tick sees), calling `each` after every tick. Stops when the
    /// ball unrolls.
    fn simulate(
        ball_time: f32,
        ticks: usize,
        held: &[Action],
        pressed: &[Action],
        mut each: impl FnMut(&Ball),
    ) {
        let bench = Bench::lawn();
        let first = ControlInput::for_tests(held, pressed);
        let rest = ControlInput::for_tests(held, &[]);
        let mut ball = Ball {
            motion: bench.motion(PlayerForm::Ball, &first, &[]),
            spin: BallSpin::default(),
            nitro: 0.0,
            ball_time,
            unrolled: false,
            boost_started: false,
            leaves_trail: false,
        };
        for tick in 0..ticks {
            if tick > 0 {
                ball.motion = ball.motion.next_tick(&rest);
            }
            ball.tick();
            each(&ball);
            if ball.unrolled {
                return;
            }
        }
    }

    #[test]
    fn the_ball_rests_on_its_bottom() {
        simulate(1.0, 60, &[], &[], |ball| {
            let m = &ball.motion;
            let floor = m.map.floor_height(m.coord.x, m.coord.z);
            assert!((m.coord.y - PLAYER_BALL_FOOT_OFFSET - floor).abs() < 0.01);
            assert!(m.ground.on_ground);
        });
    }

    #[test]
    fn rolling_with_keys_reaches_the_ball_top_speed() {
        let mut top = 0.0f32;
        simulate(1.0, 180, &[Action::Forward], &[], |ball| {
            top = top.max(ball.motion.speed);
        });
        assert!((top - 2800.0).abs() < 1.0, "top speed {top}");
    }

    #[test]
    fn the_ball_spins_with_its_speed_on_the_ground() {
        simulate(1.0, 30, &[Action::Forward], &[], |ball| {
            let expected = -ball.motion.speed * 0.01;
            assert!((ball.spin.rate - expected).abs() < 1e-4);
        });
    }

    #[test]
    fn a_nitro_boost_reaches_top_speed_and_costs_ball_time() {
        // Roll a little first, so there is a velocity to boost.
        let bench = Bench::lawn();
        let roll = ControlInput::for_tests(&[Action::Forward], &[]);
        let boost = ControlInput::for_tests(&[], &[Action::Kick]);
        let mut ball = Ball {
            motion: bench.motion(PlayerForm::Ball, &roll, &[]),
            spin: BallSpin::default(),
            nitro: 0.0,
            ball_time: 1.0,
            unrolled: false,
            boost_started: false,
            leaves_trail: false,
        };
        for _ in 0..5 {
            ball.tick();
            ball.motion = ball.motion.next_tick(&roll);
        }
        let before = ball.ball_time;
        ball.motion = ball.motion.next_tick(&boost);
        ball.tick();
        // The boost comes before the tick's friction.
        let boosted = 2800.0 - 400.0 * DT;
        assert!(
            (ball.motion.speed - boosted).abs() < 0.1,
            "{}",
            ball.motion.speed
        );
        assert!((ball.nitro - (0.6 - DT)).abs() < 1e-5);
        let spent = before - ball.ball_time;
        assert!((spent - (0.05 + 0.04 * DT)).abs() < 1e-5, "spent {spent}");

        // No second boost until the first has run out.
        let speed = ball.motion.speed;
        ball.motion = ball.motion.next_tick(&boost);
        ball.tick();
        assert!(ball.motion.speed < speed);
    }

    #[test]
    fn the_ball_unrolls_when_its_time_runs_out() {
        let mut ticks = 0;
        let mut unrolled = false;
        simulate(0.04, 120, &[], &[], |ball| {
            ticks += 1;
            unrolled = ball.unrolled;
        });
        assert!(unrolled);
        // 0.04 of the timer lasts a second at 0.04 per second.
        assert!((59..=61).contains(&ticks), "unrolled after {ticks} ticks");
    }

    #[test]
    fn the_bug_unrolls_only_where_its_head_fits_under_the_ceiling() {
        let map = TerrainMap::load_for_tests("BeeHive", true);
        let tuning = PlayerTuning::default();
        let (mut low, mut high) = (0, 0);
        for row in 0..map.depth {
            for col in 0..map.width {
                let coord = Vec3::new(col as f32 * 160.0 + 80.0, 0.0, row as f32 * 160.0 + 80.0);
                let ceiling = map.height_at(coord.x, coord.z, LayerKind::Ceiling).0;
                let gap = ceiling - map.floor_height(coord.x, coord.z);
                let fits = has_headroom_to_unroll(&map, &tuning, coord);
                assert_eq!(fits, gap > 190.0, "gap {gap}");
                if fits {
                    high += 1;
                } else {
                    low += 1;
                }
            }
        }
        assert!(high > 0 && low > 0, "{high} roomy, {low} low tiles");
        // Without a ceiling there is always room.
        let lawn = Bench::lawn();
        assert!(has_headroom_to_unroll(&lawn.map, &tuning, Vec3::ZERO));
    }

    #[test]
    fn each_player_morphs_on_its_own_input_and_ball_time() {
        use bevy::ecs::system::RunSystemOnce;

        use crate::collision::{CollisionKind, SolidSides, solid_object};

        let mut world = World::new();
        world.insert_resource(TerrainMap::load_for_tests("Lawn", false));
        world.init_resource::<PlayerTuning>();
        let mut spawn = |pressed: &[Action], ball_time: f32| {
            world
                .spawn((
                    Player,
                    Transform::default(),
                    solid_object(
                        vec![PlayerForm::Bug.collision_box()],
                        CollisionKind::Player,
                        SolidSides::TOUCHABLE,
                    ),
                    ControlInput::for_tests(&[], pressed),
                    BallTime(ball_time),
                ))
                .id()
        };
        let presses = spawn(&[Action::MorphPlayer], 1.0);
        let idle = spawn(&[], 1.0);
        let out_of_time = spawn(&[Action::MorphPlayer], 0.0);

        world
            .run_system_once(check_player_morph)
            .expect("the system runs");
        let state = |entity| *world.get::<BugState>(entity).expect("a bug state");
        assert_eq!(state(presses), BugState::RollUp);
        assert_eq!(state(idle), BugState::Stand);
        assert_eq!(state(out_of_time), BugState::Stand);
    }

    #[test]
    fn the_ball_turns_into_the_bug_in_deep_water() {
        let bench = Bench::lawn();
        let water = [bench.liquid(crate::liquids::LiquidKind::Water, 300.0)];
        let input = ControlInput::default();
        let mut motion = bench.motion(PlayerForm::Ball, &input, &water);
        motion.candidate_liquids = vec![Some(crate::liquids::LiquidKind::Water)];
        let mut ball = Ball {
            motion,
            spin: BallSpin::default(),
            nitro: 0.0,
            ball_time: 1.0,
            unrolled: false,
            boost_started: false,
            leaves_trail: false,
        };
        ball.tick();
        assert_eq!(ball.motion.form, PlayerForm::Bug);
        assert!(!ball.unrolled);
        // Ball time still drains on the tick it changes.
        assert!(ball.ball_time < 1.0);
    }
}
