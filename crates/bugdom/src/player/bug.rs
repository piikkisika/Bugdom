//! The bug form's movement.
//!
//! [`BugState`] is the single state that both movement and animation derive
//! from. In the original the animation number *is* the state
//! (`MovePlayer_Bug` dispatches on `Skeleton->AnimNum`); here [`move_bug`]
//! reads and writes the state and never touches the animator, and
//! `animate_bug` (in `animation.rs`) starts the animation that matches it.

use bevy::prelude::*;

use super::animation::AnimatedBugState;
use super::{PLAYER_BUG_HEAD_OFFSET, Player, PlayerToCameraAngle};
use crate::collision::collide_floor_and_ceiling;
use crate::input::{Action, ControlInput, ControlSettings};
use crate::math::{turn_toward, yaw_forward, yaw_of};
use crate::physics::{GroundContact, Velocity};
use crate::skeleton::SkeletonAnimator;
use crate::terrain::TerrainMap;

/// What the bug is doing. The variants carry no data; anything a state needs
/// lives in its own component.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[require(PlayerSpeed, PlayerSteering, AnimatedBugState)]
pub enum BugState {
    #[default]
    Stand,
    Walk,
    Jump,
    Fall,
    Land,
}

/// The player's horizontal speed as the controls left it, before collision
/// (`ObjNode::Speed`), in units per second. The walk animation's speed and
/// the turning rate follow it.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct PlayerSpeed(pub f32);

/// The steering acceleration the controls set, in the original's units
/// (`ObjNode::AccelVector`). It persists between ticks: some states move
/// before they read the controls, and so use the previous tick's value.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Deref, DerefMut)]
pub struct PlayerSteering(pub Vec2);

/// The bug's movement constants, from original/src/Player/Player_Bug.c,
/// original/src/Player/Player_Control.c and
/// original/src/Headers/player_control.h. Speeds are in units per second and
/// accelerations in units per second squared.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct PlayerTuning {
    /// Steering acceleration per unit of steering input (`PLAYER_BUG_ACCEL`).
    pub steering_accel: f32,
    /// How much of the steering input reaches the bug (the 0.08 in
    /// `DoPlayerControl_Bug`).
    pub steering_scale: f32,
    /// Steering factor while in the air.
    pub airborne_steering: f32,
    /// Friction while walking (`PLAYER_BUG_FRICTION_ACCEL`).
    pub friction: f32,
    /// Top horizontal speed (`PLAYER_MAX_SPEED_BUG`).
    pub max_speed: f32,
    /// Upward speed at the start of a jump (`PLAYER_BUG_JUMPFORCE`).
    pub jump_speed: f32,
    /// `PLAYER_GRAVITY`
    pub gravity: f32,
    /// The slowest a falling body may fall, in units per tick at 60 ticks
    /// per second; it stops platforms jittering (the `-20 × fps` in
    /// `DoFrictionAndGravity`).
    pub min_fall_speed_per_tick: f32,
    /// Push along a slope (`PLAYER_SLOPE_ACCEL`); the bug feels a quarter.
    pub slope_accel: f32,
    /// Below this floor-normal height a slope counts as steep.
    pub steep_slope_normal_y: f32,
    /// On a steep slope, a body falling faster than this is pushed hard.
    pub steep_slope_fall_speed: f32,
    /// Thrust of the auto-walk key and of player-relative keys (`KEY_THRUST`).
    pub key_thrust: f32,
    /// Turning rate of player-relative keys, in radians per second.
    pub key_turn_rate: f32,
    /// Turning rate toward the direction of motion per unit of speed, in
    /// radians per unit (the 0.013 in `UpdatePlayer_Bug`).
    pub turn_rate_per_speed: f32,
    /// Below this speed the bug stands rather than walks.
    pub walk_speed: f32,
    /// A jump becomes a fall once rising slower than this.
    pub jump_to_fall_speed: f32,
    /// How much horizontal speed a landing from a fall keeps.
    pub fall_landing_slowdown: f32,
    /// Moves are split into steps no longer than this, in units
    /// (`DELTA_SUBDIV`).
    pub max_step: f32,
    /// Counts as on the ground for control while the feet are this close
    /// to the floor.
    pub control_ground_distance: f32,
    /// Slopes push the bug while its feet are this close to the floor.
    pub slope_ground_distance: f32,
}

impl Default for PlayerTuning {
    fn default() -> Self {
        Self {
            steering_accel: 30.0,
            steering_scale: 0.08,
            airborne_steering: 0.5,
            friction: 600.0,
            max_speed: 700.0,
            jump_speed: 2000.0,
            gravity: 5200.0,
            min_fall_speed_per_tick: 20.0 / 60.0,
            slope_accel: 2800.0,
            steep_slope_normal_y: 0.25,
            steep_slope_fall_speed: 150.0,
            key_thrust: 3500.0,
            key_turn_rate: 4.0,
            turn_rate_per_speed: 0.013,
            walk_speed: 1.0,
            jump_to_fall_speed: 900.0,
            fall_landing_slowdown: 0.5,
            max_step: 15.0,
            control_ground_distance: 5.0,
            slope_ground_distance: 15.0,
        }
    }
}

/// Everything one tick of the bug's movement reads and writes, gathered so
/// that the per-state functions read like the original.
struct Bug<'a> {
    state: BugState,
    coord: Vec3,
    yaw: f32,
    velocity: Vec3,
    speed: f32,
    steering: Vec2,
    ground: GroundContact,
    animator: &'a SkeletonAnimator,
    tuning: &'a PlayerTuning,
    input: &'a ControlInput,
    settings: &'a ControlSettings,
    map: &'a TerrainMap,
    camera_angle: f32,
    dt: f32,
}

/// Moves the player's bug for one tick.
///
/// Port of `MovePlayer_Bug` (original/src/Player/Player_Bug.c).
pub fn move_bug(
    time: Res<Time>,
    tuning: Res<PlayerTuning>,
    input: Res<ControlInput>,
    settings: Res<ControlSettings>,
    map: Res<TerrainMap>,
    camera_angle: Res<PlayerToCameraAngle>,
    mut players: Query<
        (
            &mut BugState,
            &mut Transform,
            &mut Velocity,
            &mut PlayerSpeed,
            &mut PlayerSteering,
            &mut GroundContact,
            &SkeletonAnimator,
        ),
        With<Player>,
    >,
) {
    for (mut state, mut transform, mut velocity, mut speed, mut steering, mut ground, animator) in
        &mut players
    {
        let mut bug = Bug {
            state: *state,
            coord: transform.translation,
            yaw: yaw_of(transform.rotation),
            velocity: **velocity,
            speed: **speed,
            steering: **steering,
            ground: *ground,
            animator,
            tuning: &tuning,
            input: &input,
            settings: &settings,
            map: &map,
            camera_angle: **camera_angle,
            dt: time.delta_secs(),
        };
        match bug.state {
            BugState::Stand => bug.stand(),
            BugState::Walk => bug.walk(),
            BugState::Jump => bug.jump(),
            BugState::Fall => bug.fall(),
            BugState::Land => bug.land(),
        }
        bug.update();

        state.set_if_neq(bug.state);
        transform.translation = bug.coord;
        transform.rotation = Quat::from_rotation_y(bug.yaw);
        **velocity = bug.velocity;
        **speed = bug.speed;
        **steering = bug.steering;
        *ground = bug.ground;
    }
}

impl Bug<'_> {
    /// Port of `MovePlayerBug_Stand`. Steering is read last, so that a jump
    /// starts cleanly.
    fn stand(&mut self) {
        self.apply_friction_and_gravity(self.tuning.friction);
        self.move_and_collide(false);
        if self.state == BugState::Stand && self.speed > self.tuning.walk_speed {
            self.state = BugState::Walk;
        }
        self.control(1.0);
    }

    /// Port of `MovePlayerBug_Walk`.
    fn walk(&mut self) {
        self.control(1.0);
        self.apply_friction_and_gravity(self.tuning.friction);
        self.move_and_collide(false);
        if self.state == BugState::Walk && self.speed < self.tuning.walk_speed {
            self.state = BugState::Stand;
        }
    }

    /// Port of `MovePlayerBug_Jump`. Aiming at boppable enemies arrives with
    /// the enemies.
    fn jump(&mut self) {
        self.apply_friction_and_gravity(self.tuning.friction);
        self.move_and_collide(false);
        if self.ground.on_ground {
            self.velocity.y = 0.0;
            if self.state == BugState::Jump {
                self.state = BugState::Land;
            }
        } else if self.velocity.y < self.tuning.jump_to_fall_speed && self.state == BugState::Jump {
            self.state = BugState::Fall;
        }
        self.control(1.0);
    }

    /// Port of `MovePlayerBug_Fall`.
    fn fall(&mut self) {
        self.apply_friction_and_gravity(self.tuning.friction);
        self.move_and_collide(false);
        if self.state == BugState::Fall && self.ground.on_ground {
            self.state = BugState::Land;
            // Landing from a fall slows the bug down.
            self.velocity.x *= self.tuning.fall_landing_slowdown;
            self.velocity.z *= self.tuning.fall_landing_slowdown;
        }
        self.control(1.0);
    }

    /// Port of `MovePlayerBug_Land`: no control until the landing has
    /// blended in.
    fn land(&mut self) {
        self.apply_friction_and_gravity(self.tuning.friction);
        self.move_and_collide(true);
        if !self.animator.is_morphing() {
            self.state = BugState::Stand;
        }
    }

    /// Turns the bug toward its motion. Port of `UpdatePlayer_Bug`.
    fn update(&mut self) {
        if !self.player_relative_keys() {
            let from = self.coord.xz();
            let target = from + self.velocity.xz();
            let max_turn = self.speed * self.tuning.turn_rate_per_speed * self.dt;
            self.yaw = turn_toward(self.yaw, from, target, max_turn).0;
        }
    }

    fn player_relative_keys(&self) -> bool {
        self.input.using_key_control() && self.settings.player_relative_keys
    }

    /// Reads the steering and the jump button.
    ///
    /// Port of `DoPlayerControl_Bug`. `slug_factor` scales the steering
    /// (swimming uses less). Kicking arrives with the kickable objects.
    fn control(&mut self, slug_factor: f32) {
        let on_ground = self.ground.on_ground
            || self.ground.dist_to_floor < self.tuning.control_ground_distance;

        let mut steering = self.input.steering(self.dt) * self.tuning.steering_scale;
        if !on_ground {
            steering *= self.tuning.airborne_steering;
        }
        self.steering = if self.player_relative_keys() {
            Vec2::ZERO
        } else {
            steering * (self.tuning.steering_accel * slug_factor)
        };

        // The original allows a jump from standing or walking even in the
        // air, e.g. right after walking off a ledge.
        if matches!(self.state, BugState::Stand | BugState::Walk)
            && self.input.just_pressed(Action::Jump)
        {
            self.state = BugState::Jump;
            self.velocity.y = self.tuning.jump_speed;
        }
    }

    /// Applies friction against the horizontal motion, then gravity.
    ///
    /// Port of `DoFrictionAndGravity` (original/src/Player/Player_Control.c).
    /// Each axis slows toward zero without changing sign.
    fn apply_friction_and_gravity(&mut self, friction: f32) {
        let against = -self.velocity.xz().normalize_or_zero() * (friction * self.dt);
        for (v, a) in [
            (&mut self.velocity.x, against.x),
            (&mut self.velocity.z, against.y),
        ] {
            if *v < 0.0 {
                *v = (*v + a).min(0.0);
            } else if *v > 0.0 {
                *v = (*v + a).max(0.0);
            }
        }
        if self.velocity.x == 0.0 && self.velocity.z == 0.0 {
            self.speed = 0.0;
        }

        self.velocity.y -= self.tuning.gravity * self.dt;
        // Fall at least a little, so that standing on a platform doesn't
        // jitter.
        let min_fall = -self.tuning.min_fall_speed_per_tick;
        if self.velocity.y < 0.0 && self.velocity.y > min_fall {
            self.velocity.y = min_fall;
        }
    }

    /// Applies the controls to the velocity, then moves in small steps,
    /// keeping the bug on the terrain and letting slopes push it.
    ///
    /// Port of `DoPlayerMovementAndCollision`
    /// (original/src/Player/Player_Control.c). Collision with objects and
    /// fences, platforms and water arrive with those features.
    fn move_and_collide(&mut self, no_control: bool) {
        let tuning = self.tuning;
        self.ground.on_ground = false;
        self.ground.on_terrain = false;
        let old_velocity = self.velocity;

        if !no_control {
            if self.player_relative_keys() {
                if self.input.held(Action::Left) {
                    self.yaw += tuning.key_turn_rate * self.dt;
                } else if self.input.held(Action::Right) {
                    self.yaw -= tuning.key_turn_rate * self.dt;
                }
                let thrust = if self.input.held(Action::Forward) {
                    tuning.key_thrust
                } else if self.input.held(Action::Backward) {
                    -tuning.key_thrust
                } else {
                    0.0
                };
                self.add_horizontal(yaw_forward(self.yaw) * (thrust * self.dt));
            } else {
                // Steering is relative to the camera.
                let accel = Vec2::from_angle(self.camera_angle).rotate(self.steering);
                self.add_horizontal(accel);
                if self.input.held(Action::AutoWalk) {
                    self.add_horizontal(yaw_forward(self.yaw) * (tuning.key_thrust * self.dt));
                }
            }
        }

        self.speed = self.velocity.xz().length();
        if !self.speed.is_finite() {
            self.speed = 0.0;
            self.velocity.x = 0.0;
            self.velocity.z = 0.0;
        }
        if self.speed > tuning.max_speed {
            // Only the horizontal speed is limited; jumps and falls have
            // their own limits.
            let scale = tuning.max_speed / self.speed;
            self.velocity.x *= scale;
            self.velocity.z *= scale;
            self.speed = tuning.max_speed;
        }

        // Split the move so that fast motion doesn't skip through things.
        let passes = (self.speed * self.dt / tuning.max_step) as u32 + 1;
        let dt = self.dt / passes as f32;
        for _ in 0..passes {
            let old_coord = self.coord;
            self.coord += self.velocity * dt;

            // The original measured the speed without the vertical motion,
            // which slowed the bug on gentle slopes at high frame rates; the
            // modern port adds it back without the gravity this tick added.
            let real_speed = Vec3::new(
                self.velocity.x,
                self.velocity.y + tuning.gravity * dt,
                self.velocity.z,
            )
            .length();
            let contact = collide_floor_and_ceiling(
                self.map,
                &mut self.coord,
                old_coord,
                &mut self.velocity,
                old_velocity,
                0.0,
                PLAYER_BUG_HEAD_OFFSET,
                real_speed,
                dt,
            );
            if contact.on_ground {
                self.ground.on_ground = true;
                self.ground.on_terrain = true;
            }
            self.ground.floor_normal = contact.floor_normal;

            if self.ground.on_terrain || self.ground.dist_to_floor < tuning.slope_ground_distance {
                let normal = contact.floor_normal;
                let accel = if normal.y < tuning.steep_slope_normal_y {
                    // A steep slope throws the bug back only if it hits it fast.
                    if self.velocity.y.abs() > tuning.steep_slope_fall_speed {
                        tuning.slope_accel * 8.0
                    } else {
                        0.0
                    }
                } else {
                    tuning.slope_accel / 4.0
                };
                self.add_horizontal(normal.xz() * (accel * dt));
            }
        }

        self.ground.dist_to_floor =
            self.coord.y - self.map.floor_height(self.coord.x, self.coord.z);
    }

    fn add_horizontal(&mut self, delta: Vec2) {
        self.velocity.x += delta.x;
        self.velocity.z += delta.y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    /// Runs the bug's movement for `ticks` ticks from the Lawn's start,
    /// with the same input every tick (apart from presses, which only the
    /// first tick sees), calling `each` after every tick.
    fn simulate(ticks: usize, held: &[Action], pressed: &[Action], mut each: impl FnMut(&Bug)) {
        let map = TerrainMap::load_for_tests("Lawn", false);
        let tuning = PlayerTuning::default();
        let settings = ControlSettings::default();
        let animator = SkeletonAnimator::default();
        let first = ControlInput::for_tests(held, pressed);
        let rest = ControlInput::for_tests(held, &[]);
        let (x, z) = (12720.0, 15780.0);
        let mut state = (
            BugState::Stand,
            Vec3::new(x, map.floor_height(x, z), z),
            0.0,
            Vec3::ZERO,
            0.0,
            Vec2::ZERO,
            GroundContact::default(),
        );
        for tick in 0..ticks {
            let mut bug = Bug {
                state: state.0,
                coord: state.1,
                yaw: state.2,
                velocity: state.3,
                speed: state.4,
                steering: state.5,
                ground: state.6,
                animator: &animator,
                tuning: &tuning,
                input: if tick == 0 { &first } else { &rest },
                settings: &settings,
                map: &map,
                camera_angle: 0.0,
                dt: DT,
            };
            match bug.state {
                BugState::Stand => bug.stand(),
                BugState::Walk => bug.walk(),
                BugState::Jump => bug.jump(),
                BugState::Fall => bug.fall(),
                BugState::Land => bug.land(),
            }
            bug.update();
            each(&bug);
            state = (
                bug.state,
                bug.coord,
                bug.yaw,
                bug.velocity,
                bug.speed,
                bug.steering,
                bug.ground,
            );
        }
    }

    #[test]
    fn standing_still_stays_on_the_floor() {
        simulate(120, &[], &[], |bug| {
            assert_eq!(bug.state, BugState::Stand);
            let floor = bug.map.floor_height(bug.coord.x, bug.coord.z);
            assert!((bug.coord.y - floor).abs() < 0.01);
        });
    }

    #[test]
    fn a_jump_rises_about_v_squared_over_2g() {
        let mut start = None;
        let mut apex = f32::MIN;
        let mut states = Vec::new();
        simulate(90, &[], &[Action::Jump], |bug| {
            start.get_or_insert(bug.coord.y);
            apex = apex.max(bug.coord.y);
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
        simulate(180, &[Action::Forward], &[], |bug| top = top.max(bug.speed));
        assert!((top - 700.0).abs() < 1.0, "top speed {top}");
    }
}
