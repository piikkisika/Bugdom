//! The bug form's movement.
//!
//! [`BugState`] is the single state that both movement and animation derive
//! from. In the original the animation number *is* the state
//! (`MovePlayer_Bug` dispatches on `Skeleton->AnimNum`); here [`move_bug`]
//! reads and writes the state and never touches the animator, and
//! `animate_bug` (in `animation.rs`) starts the animation that matches it.

use bevy::math::Affine3A;
use bevy::prelude::*;

use super::animation::AnimatedBugState;
use super::ball::become_ball;
use super::effects::{
    PlayerEffects, SWIM_RIPPLE_INTERVAL, SWIM_RIPPLE_SCALE, Splash, SwimRipple, TorchFire,
};
use super::health::{DeferredKnock, Torched, kill_player};
use super::held::{CARRIED_DROP, CarriedBy, EatenBy, player_layers};
use super::kick::{KICK_NOW_FLAG, KickLanded, Kickables, PELVIS_JOINT, kick_impact};
use super::movement::{Motion, MotionContext, PlayerData, PlayerMessages};
use super::ride::{dragonfly_rider_mask, hops_off};
use super::swing::{PrevRope, SwingingOn, rope_leave_velocity};
use super::{BugTuning, Dying, Player, PlayerForm, PlayerModel, PlayerTuning};
use crate::input::Action;
use crate::liquids::LiquidKind;
use crate::math::{turn_toward, yaw_of};
use crate::skeleton::{AnimationFlags, SkeletonAnimator, SkeletonRig, joint_position};

/// The splash of jumping out of water (`MakeSplash`).
const JUMP_OUT_SPLASH_FORCE: f32 = 0.2;
const JUMP_OUT_SPLASH_VOLUME: f32 = 3.0;

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
    /// Kicking, until the kick's animation ends.
    Kick,
    Jump,
    Fall,
    Land,
    /// Swimming in a liquid, or drowning in one that kills.
    Swim,
    /// Knocked over by a hurt, until the fall has played out.
    KnockedOnButt,
    /// Killed, until the player starts again.
    Death,
    /// In an enemy's mouth ([`EatenBy`]), until the player starts again.
    BeingEaten,
    /// Stung by a mosquito, until it lets go.
    BloodSuck,
    /// Caught in a spider's web, until it lets go.
    Webbed,
    /// Hanging under a firefly ([`CarriedBy`]), until it lets go.
    Carried,
    /// Riding the water bug ([`Riding`](super::ride::Riding)), until it hops off.
    RideWaterBug,
    /// Riding the dragonfly ([`Riding`](super::ride::Riding)), until it hops off or an enemy
    /// throws it off.
    RideDragonFly,
    /// Swinging on an Ant Hill root ([`SwingingOn`]), until it lets go.
    RopeSwing,
}

/// One tick of the bug's movement.
struct Bug<'a> {
    motion: Motion<'a>,
    state: BugState,
    animator: &'a SkeletonAnimator,
    /// The roll-up has finished: the bug becomes the ball.
    rolled_up: bool,
    /// A liquid has killed the bug.
    drowned: bool,
    /// The bug is on fire (`gTorchPlayer`); lava sets it.
    torched: bool,
    /// Seconds since the last swimming ripple (`RippleTimer`).
    ripple_timer: f32,
    /// A ripple is due at this point on the water's surface.
    ripple_at: Option<Vec3>,
    /// A kick started this tick, so its animation flag starts clear.
    kick_started: bool,
    /// Where the carrier is and its heading, while it carries the bug.
    carrier: Option<(Vec3, f32)>,
}

/// Moves the player's bug for one tick.
///
/// Port of `MovePlayer_Bug` (original/src/Player/Player_Bug.c).
pub fn move_bug(
    mut commands: Commands,
    context: MotionContext,
    mut messages: PlayerMessages,
    mut effects: PlayerEffects,
    mut players: Query<
        (
            PlayerData,
            &PlayerModel,
            &mut SwimRipple,
            &mut TorchFire,
            Has<Torched>,
            Option<&CarriedBy>,
            Has<EatenBy>,
            (Has<SwingingOn>, Has<PrevRope>),
        ),
        With<Player>,
    >,
    carriers: Query<&Transform, Without<Player>>,
    mut models: Query<
        (
            &SkeletonAnimator,
            &mut AnimationFlags,
            Option<&SkeletonRig>,
            &Transform,
        ),
        Without<Player>,
    >,
) {
    for (
        mut player,
        model,
        mut ripple,
        mut fire,
        torched,
        carried_by,
        eaten,
        (swinging, prev_rope),
    ) in &mut players
    {
        if *player.form != PlayerForm::Bug {
            continue;
        }
        let Ok((animator, mut flags, rig, model_transform)) = models.get_mut(model.0) else {
            continue;
        };
        let mut bug = Bug {
            motion: context.motion(&player),
            state: *player.state,
            animator,
            rolled_up: false,
            drowned: false,
            torched,
            ripple_timer: **ripple,
            ripple_at: None,
            kick_started: false,
            carrier: carried_by
                .and_then(|c| carriers.get(c.0).ok())
                .map(|t| (t.translation, yaw_of(t.rotation))),
        };
        bug.tick();

        if bug.kick_started {
            flags.0[KICK_NOW_FLAG] = false;
        }

        ripple.set_if_neq(SwimRipple(bug.ripple_timer));
        if let Some(at) = bug.ripple_at {
            effects.ripple(&mut commands, at, SWIM_RIPPLE_SCALE);
        }
        if bug.torched && !torched {
            commands.entity(player.entity).insert(Torched);
        }
        // `DrownInLiquid` is the only caller of `TorchPlayer`.
        if bug.drowned && bug.torched {
            let base = Affine3A::from_rotation_translation(
                Quat::from_rotation_y(bug.motion.yaw),
                bug.motion.coord,
            ) * model_transform.compute_affine();
            let pelvis = rig.and_then(|rig| joint_position(rig, PELVIS_JOINT, Vec3::ZERO, base));
            effects.torch(&mut fire, pelvis, bug.motion.dt);
        }

        let (state, rolled_up, drowned) = (bug.state, bug.rolled_up, bug.drowned);
        let (knocked, died) = (bug.motion.knocked.is_some(), bug.motion.died);
        // A ball's waiting knock is dropped once it is the bug; turning back
        // into the ball would clear it anyway (`InitPlayer_Ball`).
        if player.deferred_knock.is_some() {
            commands.entity(player.entity).remove::<DeferredKnock>();
        }
        bug.motion
            .store(&mut player, &mut commands, &mut messages, &mut effects);
        player.state.set_if_neq(state);
        if died {
            kill_player(&mut player, &mut commands, context.tuning().kill_delay);
        } else if knocked && state == BugState::KnockedOnButt {
            // Knocked again while already down, the fall starts over.
            player.animated.restart();
        }
        if rolled_up {
            become_ball(&mut player);
            // `InitPlayer_Ball` makes the player collidable again, even
            // after a web left it out.
            commands.entity(player.entity).insert(player_layers(true));
        }
        // Who held the bug is forgotten once it is out of the hold.
        if carried_by.is_some() && state != BugState::Carried {
            commands.entity(player.entity).remove::<CarriedBy>();
        }
        if eaten && state != BugState::BeingEaten {
            commands.entity(player.entity).remove::<EatenBy>();
        }
        if swinging && state != BugState::RopeSwing {
            commands.entity(player.entity).remove::<SwingingOn>();
        }
        // Standing or walking, it may grab the root it last let go of again
        // (`gPrevRope = nil` in `MovePlayerBug_Stand` and `_Walk`).
        if prev_rope && matches!(state, BugState::Stand | BugState::Walk) {
            commands.entity(player.entity).remove::<PrevRope>();
        }
        if drowned && !player.dying {
            commands.entity(player.entity).insert(Dying {
                timer: context.tuning().kill_delay,
            });
        }
    }
}

/// The start of `MovePlayerBug_Kick`, before the bug moves: it turns
/// toward the closest kickable object (`AimAtClosestKickableObject`) and,
/// on the animation's kick frame, kicks from where it stands (`DoBugKick`).
/// The kicked objects answer before the bug moves, as in the original.
///
/// Port of original/src/Player/Player_Bug.c.
pub(super) fn aim_and_kick(
    time: Res<Time>,
    tuning: Res<PlayerTuning>,
    kickables: Kickables,
    mut kicks: MessageWriter<KickLanded>,
    mut players: Query<
        (Entity, &mut Transform, &BugState, &PlayerForm, &PlayerModel),
        With<Player>,
    >,
    mut models: Query<(&mut AnimationFlags, Option<&SkeletonRig>, &Transform), Without<Player>>,
) {
    let mut positions = None;
    for (player, mut transform, state, form, model) in &mut players {
        if *form != PlayerForm::Bug || *state != BugState::Kick {
            continue;
        }
        let positions = positions.get_or_insert_with(|| kickables.positions());
        let coord = transform.translation;
        let yaw = aim_at_closest(
            coord.xz(),
            yaw_of(transform.rotation),
            positions,
            &tuning.bug,
            time.delta_secs(),
        );
        transform.rotation = Quat::from_rotation_y(yaw);

        let Ok((mut flags, rig, model_transform)) = models.get_mut(model.0) else {
            continue;
        };
        if !flags.0[KICK_NOW_FLAG] {
            continue;
        }
        flags.0[KICK_NOW_FLAG] = false;
        if let Some(impact) = rig.and_then(|rig| kick_impact(rig, model_transform, coord, yaw)) {
            kicks.write(KickLanded {
                player,
                impact,
                yaw,
            });
        }
    }
}

/// The heading after turning toward the closest kickable object, if it is
/// in range. Port of `AimAtClosestKickableObject`.
fn aim_at_closest(from: Vec2, yaw: f32, kickables: &[Vec2], tuning: &BugTuning, dt: f32) -> f32 {
    let mut min_dist = 10_000_000.0;
    let mut closest = None;
    for &at in kickables {
        let dist = from.distance(at);
        if dist < min_dist {
            min_dist = dist;
            closest = Some(at);
        }
    }
    match closest {
        Some(target) if min_dist < tuning.kick_aim_range => {
            turn_toward(yaw, from, target, tuning.kick_aim_turn_rate * dt).0
        }
        _ => yaw,
    }
}

impl Bug<'_> {
    fn tick(&mut self) {
        match self.state {
            BugState::Stand => self.stand(),
            BugState::Walk => self.walk(),
            BugState::RollUp => self.roll_up(),
            BugState::UnRoll => self.unroll(),
            BugState::Kick => self.kick(),
            BugState::Jump => self.jump(),
            BugState::Fall => self.fall(),
            BugState::Land => self.land(),
            BugState::Swim => self.swim(),
            BugState::KnockedOnButt => self.knocked_on_butt(),
            BugState::Death => self.death(),
            BugState::BeingEaten => {}
            BugState::BloodSuck => self.blood_suck(),
            BugState::Webbed => self.webbed(),
            BugState::Carried => self.carried(),
            BugState::RideWaterBug => self.ride_water_bug(),
            BugState::RideDragonFly => self.ride_dragonfly(),
            BugState::RopeSwing => self.rope_swing(),
        }
        // A held or riding bug doesn't turn (`UpdatePlayer_Bug` isn't
        // called).
        if !self.rolled_up
            && !matches!(
                self.state,
                BugState::BeingEaten
                    | BugState::Carried
                    | BugState::RideWaterBug
                    | BugState::RideDragonFly
                    | BugState::RopeSwing
            )
        {
            self.update();
        }
    }

    /// Port of `MovePlayerBug_Stand`. Steering is read last, so that a jump
    /// starts cleanly.
    fn stand(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion.apply_friction_and_gravity(tuning.friction);
        self.move_and_collide(false);
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
        self.move_and_collide(false);
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
        self.move_and_collide(true);
    }

    /// Port of `MovePlayerBug_UnRoll`.
    fn unroll(&mut self) {
        if self.animator.has_stopped {
            self.state = BugState::Stand;
        }
        let tuning = &self.motion.tuning.bug;
        self.motion
            .apply_friction_and_gravity(tuning.super_friction);
        self.move_and_collide(true);
    }

    /// Port of `MovePlayerBug_Kick`: the bug can't move and stands again
    /// when the kick's animation ends. The aiming and the kick itself come
    /// first, in [`aim_and_kick`], as in the original.
    fn kick(&mut self) {
        if self.animator.has_stopped {
            self.state = BugState::Stand;
        }
        let tuning = &self.motion.tuning.bug;
        self.motion
            .apply_friction_and_gravity(tuning.super_friction * tuning.kick_friction_scale);
        self.move_and_collide(true);
    }

    /// Port of `MovePlayerBug_Jump`. Aiming at boppable enemies arrives with
    /// the enemies.
    fn jump(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion.apply_friction_and_gravity(tuning.friction);
        self.move_and_collide(false);
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
        self.move_and_collide(false);
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
        self.move_and_collide(true);
        if !self.animator.is_morphing() {
            self.state = BugState::Stand;
        }
    }

    /// Port of `MovePlayerBug_Swim`: slow and floaty in water, leaving
    /// ripples. Any other liquid kills, and lava sets the bug on fire.
    fn swim(&mut self) {
        let liquid = self
            .motion
            .underwater
            .map_or(LiquidKind::Water, |u| u.liquid);
        if liquid != LiquidKind::Water {
            if liquid == LiquidKind::Lava {
                self.torched = true;
            }
            self.drown();
            return;
        }
        // The last water the bug was in (`gPlayerCurrentWaterY`).
        let water_y = self.motion.underwater.map(|u| u.volume_top);
        let tuning = &self.motion.tuning.bug;
        self.control(tuning.swim_steering);
        self.motion
            .apply_friction_and_gravity(tuning.friction * tuning.swim_friction_scale);
        self.move_and_collide(false);
        // The original skips the rest when the move killed the bug.
        if self.motion.died {
            return;
        }

        self.ripple_timer += self.motion.dt;
        if self.ripple_timer > SWIM_RIPPLE_INTERVAL {
            self.ripple_timer = 0.0;
            let surface = self.motion.coord.y + LiquidKind::Water.collision_top_offset();
            self.ripple_at = Some(Vec3::new(self.motion.coord.x, surface, self.motion.coord.z));
        }

        if self.motion.underwater.is_none() {
            if self.state == BugState::Swim {
                self.state = BugState::Stand;
            }
            if self.state == BugState::Jump
                && let Some(water_y) = water_y
            {
                let m = &mut self.motion;
                m.splashes.push(Splash {
                    position: Vec3::new(m.coord.x, water_y, m.coord.z),
                    force: JUMP_OUT_SPLASH_FORCE,
                    volume: JUMP_OUT_SPLASH_VOLUME,
                });
            }
        }
    }

    /// Port of `MovePlayerBug_FallOnButt`: no control, and the knock's
    /// speed isn't limited, until the fall's animation ends.
    fn knocked_on_butt(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion
            .apply_friction_and_gravity(tuning.super_friction);
        self.motion.limit_speed = false;
        self.move_and_collide(true);
        self.motion.limit_speed = true;
        if self.state == BugState::KnockedOnButt && self.animator.has_stopped {
            self.state = BugState::Stand;
        }
    }

    /// Port of `MovePlayerBug_Death`: the dead bug slides to a stop. The
    /// original also takes the player out of others' collisions
    /// (`CType = 0`); a dying player can't be hurt anyway.
    fn death(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion
            .apply_friction_and_gravity(tuning.friction * tuning.death_friction_scale);
        self.move_and_collide(true);
    }

    /// Port of `MovePlayerBug_BloodSuck`: no control, the bug slows to a
    /// stop and loses ball time. The original also takes the player out of
    /// others' collisions (`CType = 0`), which the hold does here.
    fn blood_suck(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion.steering = Vec2::ZERO;
        self.motion
            .apply_friction_and_gravity(tuning.friction * tuning.blood_suck_friction_scale);
        self.move_and_collide(true);
        self.motion.ball_time_drained += tuning.blood_suck_ball_time_drain * self.motion.dt;
    }

    /// Port of `MovePlayerBug_Webbed`: no control, and the bug slows to a
    /// stop. Its `CType = 0` is the hold's, as for the blood suck.
    fn webbed(&mut self) {
        let tuning = &self.motion.tuning.bug;
        self.motion.steering = Vec2::ZERO;
        self.motion
            .apply_friction_and_gravity(tuning.friction * tuning.webbed_friction_scale);
        self.move_and_collide(true);
    }

    /// Port of `MovePlayerBug_Carried`: the bug hangs under its carrier,
    /// facing its way, and falls from a standstill once let go.
    fn carried(&mut self) {
        match self.carrier {
            Some((at, yaw)) => {
                self.motion.coord = at - Vec3::Y * CARRIED_DROP;
                self.motion.yaw = yaw;
            }
            None => {
                self.motion.velocity = Vec3::ZERO;
                self.state = BugState::Fall;
                self.fall();
            }
        }
    }

    /// Port of `MovePlayerBug_RideWaterBug`: the water bug drives itself
    /// and has seated the bug ([`seat_riders`](super::ride::seat_riders)),
    /// so all that is left is hopping off, which keeps the bug's own
    /// velocity: the original never updates it while riding.
    fn ride_water_bug(&mut self) {
        if hops_off(self.motion.input) {
            self.hop_off();
        }
    }

    /// Port of `MovePlayerBug_RideDragonFly`: the bug moves with the
    /// dragonfly, which has seated it, and only collides with enemies; any
    /// hit throws it off, as does the jump button.
    fn ride_dragonfly(&mut self) {
        if !hops_off(self.motion.input) {
            let m = &mut self.motion;
            m.velocity = (m.coord - m.old_coord) / m.dt;
            if m.collide_with(dragonfly_rider_mask(), m.dt) == 0 {
                return;
            }
        }
        self.motion.velocity.x = 0.0;
        self.motion.velocity.z = 0.0;
        self.hop_off();
    }

    /// Jumps off the ride (the `kKey_Jump` case of the riding states).
    /// A killed bug only falls off, without the jump's animation.
    fn hop_off(&mut self) {
        self.motion.velocity.y = self.motion.tuning.bug.jump_speed;
        self.state = if self.motion.killed {
            BugState::Death
        } else {
            BugState::Jump
        };
        self.jump();
    }

    /// Port of `MovePlayerBug_RopeSwing`: the root has swung and turned the
    /// bug ([`swing_on_roots`](super::swing::swing_on_roots)); the jump
    /// button lets go (`PlayerLeaveRootSwing`) and the bug moves as falling
    /// at once.
    fn rope_swing(&mut self) {
        self.motion.steering = Vec2::ZERO;
        if hops_off(self.motion.input) {
            self.motion.velocity = rope_leave_velocity(self.motion.velocity);
            self.state = BugState::Fall;
            self.fall();
        }
    }

    /// Sinks slowly; the bug starts again once the kill delay is over.
    /// Port of `DrownInLiquid`.
    fn drown(&mut self) {
        self.drowned = true;
        self.motion.coord.y -= self.motion.tuning.bug.drown_sink_speed * self.motion.dt;
    }

    /// Moves, and starts swimming on landing in a liquid (the
    /// `PLAYER_ANIM_SWIM` morph in `DoPlayerMovementAndCollision`).
    fn move_and_collide(&mut self, no_control: bool) {
        self.motion.move_and_collide(no_control);
        // What the bug ran into hurt it during the move, so the rest of
        // this tick already sees it fallen (`PlayerGotHurt` morphs the
        // skeleton at once).
        if self.motion.died {
            self.state = BugState::Death;
        } else if self.motion.knocked.is_some() {
            self.state = BugState::KnockedOnButt;
        }
        if self.motion.underwater.is_some() && !self.motion.killed {
            self.state = BugState::Swim;
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

    /// Reads the steering, the kick and the jump button.
    ///
    /// Port of `DoPlayerControl_Bug`. `slug_factor` scales the steering
    /// (swimming uses less).
    fn control(&mut self, slug_factor: f32) {
        // Both the kick and the jump check the state the tick started
        // with, so a jump pressed with the kick wins.
        let state = self.state;
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

        if on_ground
            && matches!(state, BugState::Stand | BugState::Walk)
            && m.input.just_pressed(Action::Kick)
        {
            self.state = BugState::Kick;
            self.kick_started = true;
        }

        // The original allows a jump from standing or walking even in the
        // air, e.g. right after walking off a ledge. Jumping out of water
        // is weaker.
        if matches!(state, BugState::Stand | BugState::Walk | BugState::Swim)
            && m.input.just_pressed(Action::Jump)
        {
            m.velocity.y = if state == BugState::Swim {
                tuning.jump_speed * tuning.swim_jump_scale
            } else {
                tuning.jump_speed
            };
            self.state = BugState::Jump;
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
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
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
    fn a_hurt_knocks_the_bug_over_within_its_tick() {
        let bench = Bench::lawn();
        let hurt_box =
            crate::collision::CollisionBox::new(10_000.0, -10_000.0, -100.0, 100.0, 100.0, -100.0)
                .at(Vec3::new(START.x, 0.0, START.y));
        let hurts = [BoxTarget {
            entity: Entity::from_raw_u32(7).unwrap(),
            kinds: CollisionKind::HurtMe.into(),
            solid: SolidSides::TOUCHABLE,
            boxes: vec![hurt_box],
            old_boxes: vec![hurt_box],
            velocity: Vec3::ZERO,
            trigger: None,
        }];
        // A jump pressed this tick doesn't happen: by the time the bug
        // reads its controls, it is already on its butt.
        let input = ControlInput::for_tests(&[], &[Action::Jump]);
        let animator = SkeletonAnimator::default();
        let mut bug = Bug {
            motion: bench.motion(PlayerForm::Bug, &input, &hurts),
            state: BugState::Stand,
            animator: &animator,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        };
        bug.motion.candidate_damage = vec![0.25];
        bug.tick();
        assert_eq!(bug.state, BugState::KnockedOnButt);
        assert!(bug.motion.velocity.y > 0.0);

        // A fatal hurt kills it at once.
        let mut bug = Bug {
            motion: bench.motion(PlayerForm::Bug, &input, &hurts),
            state: BugState::Walk,
            animator: &animator,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        };
        bug.motion.candidate_damage = vec![1.0];
        bug.tick();
        assert_eq!(bug.state, BugState::Death);
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

    #[test]
    fn a_knocked_bug_slides_unchecked_until_the_fall_ends() {
        let bench = Bench::lawn();
        let input = ControlInput::for_tests(&[Action::Forward], &[Action::Jump]);
        let animator = SkeletonAnimator::default();
        let mut bug = Bug {
            motion: bench.motion(PlayerForm::Bug, &input, &[]),
            state: BugState::KnockedOnButt,
            animator: &animator,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        };
        // Faster than the bug may walk: the knock's speed isn't limited,
        // only slowed by friction, and the controls do nothing.
        bug.motion.velocity = Vec3::new(1500.0, 0.0, 0.0);
        bug.tick();
        assert_eq!(bug.state, BugState::KnockedOnButt);
        let slowed = 1500.0 - 2000.0 * super::super::movement::bench::DT;
        assert!(
            (bug.motion.speed - slowed).abs() < 1.0,
            "{}",
            bug.motion.speed
        );
        assert_eq!(bug.motion.steering, Vec2::ZERO);

        let mut stopped = SkeletonAnimator::default();
        stopped.has_stopped = true;
        let mut bug = Bug {
            animator: &stopped,
            ..bug
        };
        bug.motion = bug.motion.next_tick(&input);
        bug.tick();
        assert_eq!(bug.state, BugState::Stand);
    }

    #[test]
    fn the_dead_bug_slides_to_a_stop_without_control() {
        let bench = Bench::lawn();
        let input = ControlInput::for_tests(&[Action::Forward], &[Action::Jump]);
        let animator = SkeletonAnimator::default();
        let mut motion = bench.motion(PlayerForm::Bug, &input, &[]);
        motion.killed = true;
        motion.velocity = Vec3::new(400.0, 0.0, 0.0);
        let mut bug = Bug {
            motion,
            state: BugState::Death,
            animator: &animator,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        };
        for _ in 0..30 {
            bug.tick();
            bug.motion = bug.motion.next_tick(&input);
        }
        assert_eq!(bug.state, BugState::Death);
        // 3000 units/s² of friction stops 400 units/s within 0.14 s.
        assert_eq!(bug.motion.speed, 0.0);
    }

    /// Runs the bug from standing at the Lawn's start in a liquid whose
    /// surface is `depth` above the floor, with a fresh input each tick.
    fn simulate_in_liquid(
        kind: LiquidKind,
        depth: f32,
        inputs: &[ControlInput],
        mut each: impl FnMut(&Bug),
    ) {
        let bench = Bench::lawn();
        let liquid = [bench.liquid(kind, depth)];
        let animator = SkeletonAnimator::default();
        let idle = ControlInput::default();
        let mut motion = bench.motion(PlayerForm::Bug, &idle, &liquid);
        motion.candidate_liquids = vec![Some(kind)];
        let mut bug = Bug {
            motion,
            state: BugState::Stand,
            animator: &animator,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        };
        for input in inputs {
            bug.motion = bug.motion.next_tick(input);
            bug.tick();
            each(&bug);
        }
    }

    #[test]
    fn deep_water_makes_the_bug_swim_just_under_the_volume_top() {
        let forward = ControlInput::for_tests(&[Action::Forward], &[]);
        let mut ticks = 0;
        simulate_in_liquid(LiquidKind::Water, 300.0, &vec![forward; 120], |bug| {
            ticks += 1;
            assert_eq!(bug.state, BugState::Swim);
            let underwater = bug.motion.underwater.expect("in the water");
            assert!((bug.motion.coord.y - (underwater.volume_top - 1.0)).abs() < 1.0);
            // The swimming limit applies from the tick after it got in.
            if ticks > 1 {
                assert!(bug.motion.speed <= 250.0 + 1e-3, "{}", bug.motion.speed);
            }
        });
    }

    #[test]
    fn shallow_water_is_walked_through() {
        let forward = ControlInput::for_tests(&[Action::Forward], &[]);
        simulate_in_liquid(LiquidKind::Water, 50.0, &vec![forward; 30], |bug| {
            assert!(bug.motion.underwater.is_none());
            assert_ne!(bug.state, BugState::Swim);
        });
    }

    #[test]
    fn a_jump_out_of_water_is_weaker() {
        let idle = ControlInput::default();
        let jump = ControlInput::for_tests(&[], &[Action::Jump]);
        let mut states = Vec::new();
        let mut rise = 0.0;
        let mut splashes = Vec::new();
        let mut water_y = 0.0;
        simulate_in_liquid(LiquidKind::Water, 300.0, &[idle, jump], |bug| {
            states.push(bug.state);
            rise = bug.motion.velocity.y;
            splashes.push(bug.motion.splashes.clone());
            if let Some(underwater) = bug.motion.underwater {
                water_y = underwater.volume_top;
            }
        });
        assert_eq!(states, [BugState::Swim, BugState::Jump]);
        // Jumping out splashes at the top of the water it left.
        assert!(splashes[0].is_empty());
        assert_eq!(splashes[1].len(), 1);
        assert_eq!(splashes[1][0].position.y, water_y);
        assert_eq!(splashes[1][0].force, JUMP_OUT_SPLASH_FORCE);
        // The jump comes before the tick's gravity.
        assert!(
            (rise - (2000.0 / 1.4 - 5200.0 / 60.0)).abs() < 1e-2,
            "{rise}"
        );
    }

    #[test]
    fn swimming_leaves_a_ripple_every_quarter_second() {
        let idle = ControlInput::default();
        let mut ripples = Vec::new();
        simulate_in_liquid(LiquidKind::Water, 300.0, &vec![idle; 60], |bug| {
            // The timer starts over with each ripple.
            if let Some(at) = bug.ripple_at
                && bug.ripple_timer == 0.0
            {
                let top = bug.motion.underwater.expect("in the water").volume_top;
                ripples.push((at, top));
            }
        });
        // The first tick only gets the bug into the water.
        assert_eq!(ripples.len(), 3);
        for (at, top) in ripples {
            // On the visible surface, 1 above the bug's float height.
            assert!((at.y - (top - 1.0 + LiquidKind::Water.collision_top_offset())).abs() < 1e-3);
        }
    }

    #[test]
    fn lava_sets_the_drowning_bug_on_fire_and_honey_does_not() {
        let idle = ControlInput::default();
        for (kind, torched) in [(LiquidKind::Lava, true), (LiquidKind::Honey, false)] {
            let mut on_fire = false;
            simulate_in_liquid(kind, 300.0, &[idle.clone(), idle.clone()], |bug| {
                on_fire = bug.torched;
            });
            assert_eq!(on_fire, torched, "{kind:?}");
        }
    }

    #[test]
    fn honey_drowns_the_bug() {
        let idle = ControlInput::default();
        let mut heights = Vec::new();
        let mut drowned = Vec::new();
        simulate_in_liquid(
            LiquidKind::Honey,
            300.0,
            &[idle.clone(), idle.clone(), idle],
            |bug| {
                heights.push(bug.motion.coord.y);
                drowned.push(bug.drowned);
            },
        );
        assert_eq!(drowned, [false, true, true]);
        assert!((heights[1] - heights[2] - 30.0 / 60.0).abs() < 1e-3);
    }

    #[test]
    fn kicking_starts_on_the_ground() {
        let bench = Bench::lawn();
        let kick = ControlInput::for_tests(&[], &[Action::Kick]);
        let animator = SkeletonAnimator::default();
        let mut bug = Bug {
            motion: bench.motion(PlayerForm::Bug, &kick, &[]),
            state: BugState::Stand,
            animator: &animator,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        };
        bug.tick();
        assert_eq!(bug.state, BugState::Kick);
        assert!(bug.kick_started);

        // In the air there is no kick.
        let mut bug = Bug {
            motion: bench.motion(PlayerForm::Bug, &kick, &[]),
            state: BugState::Jump,
            animator: &animator,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        };
        bug.motion.coord.y += 300.0;
        bug.tick();
        assert_ne!(bug.state, BugState::Kick);
    }

    #[test]
    fn a_kick_aims_at_the_closest_kickable_in_range() {
        let tuning = PlayerTuning::default().bug;
        let dt = super::super::movement::bench::DT;
        // Facing −Z; one object to the right in range, one farther away.
        let kickables = [Vec2::new(200.0, 0.0), Vec2::new(-250.0, 0.0)];
        let yaw = aim_at_closest(Vec2::ZERO, 0.0, &kickables, &tuning, dt);
        // 9 radians per second toward +X, which is a turn to the right
        // (negative yaw, wrapped).
        let turned = std::f32::consts::TAU - 9.0 * dt;
        assert!((yaw - turned).abs() < 1e-4, "{yaw}");
        // Out of range: no turn.
        let far = [Vec2::new(400.0, 0.0)];
        assert_eq!(aim_at_closest(Vec2::ZERO, 0.0, &far, &tuning, dt), 0.0);
    }

    #[test]
    fn a_kicking_bug_stands_up_when_the_animation_ends() {
        let bench = Bench::lawn();
        let idle = ControlInput::default();
        let mut stopped = SkeletonAnimator::default();
        stopped.has_stopped = true;
        let mut bug = Bug {
            motion: bench.motion(PlayerForm::Bug, &idle, &[]),
            state: BugState::Kick,
            animator: &stopped,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        };
        bug.tick();
        assert_eq!(bug.state, BugState::Stand);
    }

    /// A bug in `state` at the Lawn's start, with no input.
    fn held_bug<'a>(
        bench: &'a super::super::movement::bench::Bench,
        input: &'a ControlInput,
        animator: &'a SkeletonAnimator,
        state: BugState,
    ) -> Bug<'a> {
        Bug {
            motion: bench.motion(PlayerForm::Bug, input, &[]),
            state,
            animator,
            rolled_up: false,
            drowned: false,
            torched: false,
            ripple_timer: 0.0,
            ripple_at: None,
            kick_started: false,
            carrier: None,
        }
    }

    #[test]
    fn a_carried_bug_hangs_under_its_carrier_and_falls_when_let_go() {
        let bench = Bench::lawn();
        let input = ControlInput::for_tests(&[Action::Forward], &[Action::Jump]);
        let animator = SkeletonAnimator::default();
        let mut bug = held_bug(&bench, &input, &animator, BugState::Carried);
        let carrier = Vec3::new(START.x + 50.0, 2000.0, START.y - 30.0);
        bug.carrier = Some((carrier, 1.25));
        bug.motion.velocity = Vec3::new(100.0, -50.0, 0.0);
        bug.tick();
        assert_eq!(bug.state, BugState::Carried);
        assert_eq!(bug.motion.coord, carrier - Vec3::Y * CARRIED_DROP);
        assert_eq!(bug.motion.yaw, 1.25);
        // The controls do nothing.
        assert_eq!(bug.motion.steering, Vec2::ZERO);

        // Let go: it falls from a standstill, high above the floor.
        bug.carrier = None;
        bug.motion = bug.motion.next_tick(&input);
        bug.tick();
        assert_eq!(bug.state, BugState::Fall);
        assert_eq!(bug.motion.velocity.x, 0.0);
        let gravity = bench.tuning.gravity * super::super::movement::bench::DT;
        assert!((bug.motion.velocity.y + gravity).abs() < 1e-3);
    }

    #[test]
    fn the_blood_suck_drains_ball_time_and_holds_the_bug_still() {
        let bench = Bench::lawn();
        let input = ControlInput::for_tests(&[Action::Forward], &[Action::Jump, Action::Kick]);
        let animator = SkeletonAnimator::default();
        let mut bug = held_bug(&bench, &input, &animator, BugState::BloodSuck);
        bug.motion.steering = Vec2::new(5.0, 5.0);
        let mut drained = 0.0;
        for _ in 0..60 {
            bug.tick();
            drained += bug.motion.ball_time_drained;
            assert_eq!(bug.state, BugState::BloodSuck);
            bug.motion = bug.motion.next_tick(&input);
        }
        // 0.1 of a full timer per second.
        assert!((drained - 0.1).abs() < 1e-4, "{drained}");
        assert_eq!(bug.motion.speed, 0.0);
        assert_eq!(bug.motion.steering, Vec2::ZERO);
    }

    #[test]
    fn webbing_stops_a_bug_ten_times_as_hard() {
        let bench = Bench::lawn();
        let input = ControlInput::for_tests(&[Action::Forward], &[]);
        let animator = SkeletonAnimator::default();
        let mut bug = held_bug(&bench, &input, &animator, BugState::Webbed);
        bug.motion.velocity = Vec3::new(500.0, 0.0, 0.0);
        bug.tick();
        assert_eq!(bug.state, BugState::Webbed);
        let slowed = 500.0 - 6000.0 * super::super::movement::bench::DT;
        assert!(
            (bug.motion.speed - slowed).abs() < 1.0,
            "{}",
            bug.motion.speed
        );
        assert_eq!(bug.motion.ball_time_drained, 0.0);
    }

    #[test]
    fn an_eaten_bug_doesnt_move_by_itself() {
        let bench = Bench::lawn();
        let input = ControlInput::for_tests(&[Action::Forward], &[Action::Jump]);
        let animator = SkeletonAnimator::default();
        let mut bug = held_bug(&bench, &input, &animator, BugState::BeingEaten);
        let (coord, yaw) = (bug.motion.coord, bug.motion.yaw);
        bug.motion.velocity = Vec3::new(500.0, 0.0, 0.0);
        bug.tick();
        assert_eq!(bug.state, BugState::BeingEaten);
        assert_eq!((bug.motion.coord, bug.motion.yaw), (coord, yaw));
    }

    #[test]
    fn a_rider_sits_still_until_it_jumps_off_the_water_bug() {
        let bench = Bench::lawn();
        let idle = ControlInput::for_tests(&[Action::Forward], &[]);
        let animator = SkeletonAnimator::default();
        let mut bug = held_bug(&bench, &idle, &animator, BugState::RideWaterBug);
        bug.motion.coord.y += 500.0;
        bug.motion.velocity = Vec3::new(120.0, 0.0, -40.0);
        let seat = bug.motion.coord;
        bug.tick();
        // The water bug placed it and drives; the bug itself doesn't move.
        assert_eq!(bug.state, BugState::RideWaterBug);
        assert_eq!(bug.motion.coord, seat);
        assert_eq!(bug.motion.steering, Vec2::ZERO);

        // The jump keeps the bug's own (stale) velocity across the ground.
        let jump = ControlInput::for_tests(&[], &[Action::Jump]);
        bug.motion = bug.motion.next_tick(&jump);
        bug.tick();
        assert_eq!(bug.state, BugState::Jump);
        assert!(bug.motion.velocity.y > 0.0);
        assert_ne!(bug.motion.velocity.x, 0.0);
    }

    #[test]
    fn hopping_off_the_dragonfly_stops_the_bug_across_the_ground() {
        let bench = Bench::lawn();
        let jump = ControlInput::for_tests(&[], &[Action::Jump]);
        let animator = SkeletonAnimator::default();
        let mut bug = held_bug(&bench, &jump, &animator, BugState::RideDragonFly);
        bug.motion.coord.y += 800.0;
        bug.motion.velocity = Vec3::new(900.0, 0.0, 300.0);
        bug.tick();
        assert_eq!(bug.state, BugState::Jump);
        assert_eq!(bug.motion.velocity.xz(), Vec2::ZERO);
        assert!(bug.motion.velocity.y > 0.0);
    }

    #[test]
    fn the_dragonfly_rider_moves_with_its_seat_and_an_enemy_throws_it_off() {
        let bench = Bench::lawn();
        let idle = ControlInput::for_tests(&[], &[]);
        let animator = SkeletonAnimator::default();
        let mut bug = held_bug(&bench, &idle, &animator, BugState::RideDragonFly);
        // Seated 30 units on from last tick, high in the air.
        bug.motion.coord.y += 800.0;
        bug.motion.old_coord = bug.motion.coord - Vec3::X * 30.0;
        bug.tick();
        assert_eq!(bug.state, BugState::RideDragonFly);
        let expected = 30.0 / bug.motion.dt;
        assert!((bug.motion.velocity.x - expected).abs() < 1e-2);

        // An enemy where the bug is about to sit.
        let at = bug.motion.coord;
        let enemy_box =
            crate::collision::CollisionBox::new(10_000.0, -10_000.0, -100.0, 100.0, 100.0, -100.0)
                .at(at);
        let enemy = [BoxTarget {
            entity: Entity::from_raw_u32(7).unwrap(),
            kinds: CollisionKind::Enemy.into(),
            solid: SolidSides::ALL,
            boxes: vec![enemy_box],
            old_boxes: vec![enemy_box],
            velocity: Vec3::ZERO,
            trigger: None,
        }];
        let mut bug = held_bug(&bench, &idle, &animator, BugState::RideDragonFly);
        bug.motion = bench.motion(PlayerForm::Bug, &idle, &enemy);
        // The dragonfly carries it into the enemy from the side.
        bug.motion.coord = at;
        bug.motion.old_coord = at - Vec3::X * 300.0;
        bug.tick();
        assert_eq!(bug.state, BugState::Jump);
        assert_eq!(bug.motion.velocity.xz(), Vec2::ZERO);
    }

    #[test]
    fn a_swinging_bug_hangs_on_until_it_jumps_off_flying() {
        let bench = Bench::lawn();
        let idle = ControlInput::for_tests(&[Action::Forward], &[]);
        let animator = SkeletonAnimator::default();
        let mut bug = held_bug(&bench, &idle, &animator, BugState::RopeSwing);
        bug.motion.coord.y += 600.0;
        bug.motion.velocity = Vec3::new(0.0, 0.0, -500.0);
        let at = bug.motion.coord;
        bug.tick();
        // The root moves it; the bug itself does nothing.
        assert_eq!(bug.state, BugState::RopeSwing);
        assert_eq!(bug.motion.coord, at);
        assert_eq!(bug.motion.steering, Vec2::ZERO);

        let jump = ControlInput::for_tests(&[], &[Action::Jump]);
        bug.motion = bug.motion.next_tick(&jump);
        bug.tick();
        assert_eq!(bug.state, BugState::Fall);
        // Flung along its swing, then falling; the fall's move caps the
        // fling at the bug's top speed at once, as in the original.
        let max_speed = bench.tuning.form(PlayerForm::Bug).max_speed;
        assert!((bug.motion.velocity.z + max_speed).abs() < 1.0);
        assert_eq!(bug.motion.velocity.x, 0.0);
        assert!(bug.motion.coord.z < at.z);
    }
}
