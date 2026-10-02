//! The player's movement constants, from original/src/Player/Player_Bug.c,
//! original/src/Player/Player_Ball.c, original/src/Player/Player_Control.c,
//! original/src/Screens/Infobar.c and original/src/Headers/player_control.h.
//! Speeds are in units per second and accelerations in units per second
//! squared.

use bevy::prelude::*;

use super::PlayerForm;

/// Constants that both forms use, and each form's own.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct PlayerTuning {
    /// `PLAYER_GRAVITY`
    pub gravity: f32,
    /// The slowest a falling body may fall, in units per tick at 60 ticks
    /// per second; it stops platforms jittering (the `-20 × fps` in
    /// `DoFrictionAndGravity`).
    pub min_fall_speed_per_tick: f32,
    /// Push along a slope (`PLAYER_SLOPE_ACCEL`), before each form's scale.
    pub slope_accel: f32,
    /// Below this floor-normal height a slope counts as steep.
    pub steep_slope_normal_y: f32,
    /// On a steep slope, a body falling faster than this is pushed hard.
    pub steep_slope_fall_speed: f32,
    /// Slopes push the player while its feet are this close to the floor.
    pub slope_ground_distance: f32,
    /// Thrust of the auto-walk key and of player-relative keys (`KEY_THRUST`).
    pub key_thrust: f32,
    /// Turning rate of player-relative keys, in radians per second.
    pub key_turn_rate: f32,
    /// Moves are split into steps no longer than this, in units
    /// (`DELTA_SUBDIV`).
    pub max_step: f32,
    /// The player's radius against fences, as a fraction of its radius (it
    /// squeezes a little closer).
    pub fence_radius_scale: f32,
    pub bug: BugTuning,
    pub ball: BallTuning,
}

/// What differs between the forms in the shared movement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormMotion {
    /// Top horizontal speed (`PLAYER_MAX_SPEED_BUG`, `PLAYER_MAX_SPEED_BALL`).
    pub max_speed: f32,
    /// How much of the slope push the form feels on a walkable slope.
    pub gentle_slope_scale: f32,
    /// How much of the slope push the form feels when it hits a steep slope
    /// fast.
    pub steep_slope_scale: f32,
}

/// The bug's constants.
#[derive(Debug, Clone, PartialEq)]
pub struct BugTuning {
    pub motion: FormMotion,
    /// Steering acceleration per unit of steering input (`PLAYER_BUG_ACCEL`).
    pub steering_accel: f32,
    /// How much of the steering input reaches the bug (the 0.08 in
    /// `DoPlayerControl_Bug`).
    pub steering_scale: f32,
    /// Steering factor while in the air.
    pub airborne_steering: f32,
    /// Friction while walking (`PLAYER_BUG_FRICTION_ACCEL`).
    pub friction: f32,
    /// Friction that stops the bug while it rolls up or unrolls
    /// (`PLAYER_BUG_SUPERFRICTION`).
    pub super_friction: f32,
    /// Upward speed at the start of a jump (`PLAYER_BUG_JUMPFORCE`).
    pub jump_speed: f32,
    /// Turning rate toward the direction of motion per unit of speed, in
    /// radians per unit (the 0.013 in `UpdatePlayer_Bug`).
    pub turn_rate_per_speed: f32,
    /// Below this speed the bug stands rather than walks.
    pub walk_speed: f32,
    /// A jump becomes a fall once rising slower than this.
    pub jump_to_fall_speed: f32,
    /// How much horizontal speed a landing from a fall keeps.
    pub fall_landing_slowdown: f32,
    /// Counts as on the ground for control while the feet are this close
    /// to the floor.
    pub control_ground_distance: f32,
}

/// The ball's constants.
#[derive(Debug, Clone, PartialEq)]
pub struct BallTuning {
    pub motion: FormMotion,
    /// Steering acceleration per unit of steering input (`PLAYER_BALL_ACCEL`).
    pub steering_accel: f32,
    /// How much of the steering input reaches the ball (the 0.05 in
    /// `DoPlayerControl_Ball`).
    pub steering_scale: f32,
    /// `PLAYER_BALL_FRICTION_ACCEL`
    pub friction: f32,
    /// A nitro boost multiplies the horizontal velocity by this, before the
    /// top speed caps it (`DoPlayerControl_Ball`).
    pub nitro_boost: f32,
    /// How long a nitro boost lasts, in seconds (`StartNitroTrail`).
    pub nitro_duration: f32,
    /// Ball time a nitro boost costs, as a fraction of a full timer.
    pub nitro_ball_time: f32,
    /// Turning rate toward the direction of motion, in radians per second
    /// (`SpinBall`).
    pub turn_rate: f32,
    /// Rolling speed on the ground per unit of speed, in radians per unit
    /// (`SpinBall`).
    pub spin_per_speed: f32,
    /// How fast the rolling slows in the air, in radians per second squared.
    pub air_spin_decay: f32,
    /// Ball time lost per second, as a fraction of a full timer
    /// (`ProcessBallTimer`; easy mode, which arrives with the settings,
    /// loses 0.03).
    pub ball_time_drain: f32,
    /// The bug unrolls only where the gap between floor and ceiling is
    /// taller than its head by more than this
    /// (`BallHasHeadroomToMorphToBug`).
    pub unroll_headroom: f32,
}

impl PlayerTuning {
    /// The movement constants of a form.
    pub fn form(&self, form: PlayerForm) -> &FormMotion {
        match form {
            PlayerForm::Bug => &self.bug.motion,
            PlayerForm::Ball => &self.ball.motion,
        }
    }
}

impl Default for PlayerTuning {
    fn default() -> Self {
        Self {
            gravity: 5200.0,
            min_fall_speed_per_tick: 20.0 / 60.0,
            slope_accel: 2800.0,
            steep_slope_normal_y: 0.25,
            steep_slope_fall_speed: 150.0,
            slope_ground_distance: 15.0,
            key_thrust: 3500.0,
            key_turn_rate: 4.0,
            max_step: 15.0,
            fence_radius_scale: 0.7,
            bug: BugTuning {
                motion: FormMotion {
                    max_speed: 700.0,
                    gentle_slope_scale: 0.25,
                    steep_slope_scale: 8.0,
                },
                steering_accel: 30.0,
                steering_scale: 0.08,
                airborne_steering: 0.5,
                friction: 600.0,
                super_friction: 2000.0,
                jump_speed: 2000.0,
                turn_rate_per_speed: 0.013,
                walk_speed: 1.0,
                jump_to_fall_speed: 900.0,
                fall_landing_slowdown: 0.5,
                control_ground_distance: 5.0,
            },
            ball: BallTuning {
                motion: FormMotion {
                    max_speed: 2800.0,
                    gentle_slope_scale: 1.0,
                    steep_slope_scale: 7.0,
                },
                steering_accel: 66.0,
                steering_scale: 0.05,
                friction: 400.0,
                nitro_boost: 100.0,
                nitro_duration: 0.6,
                nitro_ball_time: 0.05,
                turn_rate: 8.0,
                spin_per_speed: 0.01,
                air_spin_decay: 12.0,
                ball_time_drain: 0.04,
                unroll_headroom: 10.0,
            },
        }
    }
}
