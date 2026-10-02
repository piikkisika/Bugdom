//! Movement that the bug and the ball share: friction and gravity, the
//! controls' push, and moving in small steps against objects, the terrain and
//! fences.
//!
//! Port of original/src/Player/Player_Control.c and of
//! `DoPlayerCollisionDetect` (original/src/Player/MyGuy.c).

use avian3d::prelude::Collider;
use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::query::QueryData;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::animation::AnimatedBugState;
use super::ball::{BallSpin, BallTime, Nitro};
use super::bug::BugState;
use super::contact::{BallHitEnemy, EnemyBopped, TouchedEnemy};
use super::health::HurtPlayer;
use super::{
    Dying, PLAYER_RADIUS, PlayerForm, PlayerSpeed, PlayerSteering, PlayerToCameraAngle,
    PlayerTuning, player_collision_mask,
};
use crate::collision::{
    BoxMover, BoxTarget, CollisionBoxes, CollisionCandidates, CollisionKind, SolidSides,
    TriggerHit, collide_floor_and_ceiling, resolve_box_collisions,
};
use crate::combat::Damage;
use crate::fences::Fences;
use crate::input::{Action, ControlInput, ControlSettings};
use crate::liquids::{Liquid, LiquidKind, Underwater};
use crate::math::{yaw_forward, yaw_of};
use crate::physics::{GroundContact, PreviousPosition, Velocity};
use crate::terrain::TerrainMap;

/// The world a tick of a player's movement reads: the same for every
/// player.
#[derive(SystemParam)]
pub(super) struct MotionContext<'w, 's> {
    time: Res<'w, Time>,
    tuning: Res<'w, PlayerTuning>,
    map: Res<'w, TerrainMap>,
    fences: Option<Res<'w, Fences>>,
    liquids: Query<'w, 's, &'static Liquid>,
    damages: Query<'w, 's, &'static Damage>,
}

/// The messages a tick of a player's movement sends.
#[derive(SystemParam)]
pub(super) struct PlayerMessages<'w> {
    triggers: MessageWriter<'w, TriggerHit>,
    hurts: MessageWriter<'w, HurtPlayer>,
    touched: MessageWriter<'w, TouchedEnemy>,
    ball_hits: MessageWriter<'w, BallHitEnemy>,
    bops: MessageWriter<'w, EnemyBopped>,
}

/// The player's components that its movement reads and writes, whatever
/// its form.
#[derive(QueryData)]
#[query_data(mutable)]
pub(super) struct PlayerData {
    pub entity: Entity,
    pub previous: &'static mut PreviousPosition,
    pub candidates: &'static CollisionCandidates,
    pub input: &'static ControlInput,
    pub settings: &'static ControlSettings,
    pub camera_angle: &'static PlayerToCameraAngle,
    pub ball_time: &'static mut BallTime,
    pub form: &'static mut PlayerForm,
    pub boxes: &'static mut CollisionBoxes,
    pub collider: &'static mut Collider,
    pub state: &'static mut BugState,
    pub animated: &'static mut AnimatedBugState,
    pub spin: &'static mut BallSpin,
    pub nitro: &'static mut Nitro,
    pub transform: &'static mut Transform,
    pub velocity: &'static mut Velocity,
    pub speed: &'static mut PlayerSpeed,
    pub steering: &'static mut PlayerSteering,
    pub ground: &'static mut GroundContact,
    pub underwater: Option<&'static Underwater>,
    pub dying: Has<Dying>,
}

/// Everything one tick of movement reads and writes, gathered so that the
/// forms' functions read like the original.
pub(super) struct Motion<'a> {
    pub entity: Entity,
    pub form: PlayerForm,
    /// Where the player was at the start of the tick (`OldCoord`).
    pub old_coord: Vec3,
    pub coord: Vec3,
    pub yaw: f32,
    pub velocity: Vec3,
    /// The velocity at the start of the tick (`ObjNode::Delta`, as opposed
    /// to `gDelta`).
    pub start_velocity: Vec3,
    pub speed: f32,
    /// The controls' speed limit applies (it doesn't while the bug is
    /// knocked on its butt).
    pub limit_speed: bool,
    pub steering: Vec2,
    pub ground: GroundContact,
    pub tuning: &'a PlayerTuning,
    pub input: &'a ControlInput,
    pub settings: &'a ControlSettings,
    pub map: &'a TerrainMap,
    pub fences: Option<&'a Fences>,
    pub candidates: &'a [BoxTarget],
    /// The liquid each candidate is, if it is one.
    pub candidate_liquids: Vec<Option<LiquidKind>>,
    /// The damage each candidate deals ([`Damage`], or 0).
    pub candidate_damage: Vec<f32>,
    /// In a liquid's volume, as of the last collision check
    /// (`STATUS_BIT_UNDERWATER`).
    pub underwater: Option<Underwater>,
    /// Killed, and waiting to start again (`gPlayerGotKilledFlag`).
    pub killed: bool,
    /// Triggers that went off and stopped being solid this tick.
    pub spent: EntityHashSet,
    pub triggered: Vec<TriggerHit>,
    /// Hurts from what the player touched this tick.
    pub hurts: Vec<HurtPlayer>,
    pub touched: Vec<TouchedEnemy>,
    pub ball_hits: Vec<BallHitEnemy>,
    pub bops: Vec<EnemyBopped>,
    /// Ball time that ball-time drains took this tick, not yet taken off.
    pub ball_time_drained: f32,
    pub camera_angle: f32,
    pub dt: f32,
}

impl PlayerDataItem<'_, '_> {
    /// Changes form, and the collision box with it.
    pub fn set_form(&mut self, form: PlayerForm) {
        *self.form = form;
        *self.boxes = CollisionBoxes(vec![form.collision_box()]);
        *self.collider = self.boxes.collider();
    }
}

impl MotionContext<'_, '_> {
    pub fn tuning(&self) -> &PlayerTuning {
        &self.tuning
    }

    /// The motion of one player at the start of its tick.
    pub fn motion<'a>(&'a self, player: &PlayerDataItem<'a, '_>) -> Motion<'a> {
        // Copied out so that the motion doesn't borrow the player.
        let candidates: &'a CollisionCandidates = player.candidates;
        let input: &'a ControlInput = player.input;
        let settings: &'a ControlSettings = player.settings;
        Motion {
            entity: player.entity,
            form: *player.form,
            old_coord: **player.previous,
            coord: player.transform.translation,
            yaw: yaw_of(player.transform.rotation),
            velocity: **player.velocity,
            start_velocity: **player.velocity,
            speed: **player.speed,
            limit_speed: true,
            steering: **player.steering,
            ground: *player.ground,
            tuning: &self.tuning,
            input,
            settings,
            map: &self.map,
            fences: self.fences.as_deref(),
            candidates: &candidates.0,
            candidate_liquids: candidates
                .0
                .iter()
                .map(|t| self.liquids.get(t.entity).ok().map(|l| l.0))
                .collect(),
            candidate_damage: candidates
                .0
                .iter()
                .map(|t| self.damages.get(t.entity).map_or(0.0, |d| d.0))
                .collect(),
            underwater: player.underwater.copied(),
            killed: player.dying,
            spent: EntityHashSet::default(),
            triggered: Vec::new(),
            hurts: Vec::new(),
            touched: Vec::new(),
            ball_hits: Vec::new(),
            bops: Vec::new(),
            ball_time_drained: 0.0,
            camera_angle: **player.camera_angle,
            dt: self.time.delta_secs(),
        }
    }
}

impl Motion<'_> {
    /// Writes the tick's result back to the player and sends the triggers
    /// it set off and what it touched. Ball time drained this tick and not
    /// already taken off comes off here.
    pub fn store(
        mut self,
        player: &mut PlayerDataItem,
        commands: &mut Commands,
        messages: &mut PlayerMessages,
    ) {
        messages.triggers.write_batch(self.triggered.drain(..));
        messages.hurts.write_batch(self.hurts.drain(..));
        messages.touched.write_batch(self.touched.drain(..));
        messages.ball_hits.write_batch(self.ball_hits.drain(..));
        messages.bops.write_batch(self.bops.drain(..));
        if self.ball_time_drained > 0.0 {
            // `LoseBallTime` for the bug, which has nothing to unroll.
            **player.ball_time = (**player.ball_time - self.ball_time_drained).max(0.0);
        }
        if player.underwater.copied() != self.underwater {
            let mut entity = commands.entity(player.entity);
            match self.underwater {
                Some(underwater) => entity.insert(underwater),
                None => entity.remove::<Underwater>(),
            };
        }
        player.transform.translation = self.coord;
        player.transform.rotation = Quat::from_rotation_y(self.yaw);
        **player.velocity = self.velocity;
        **player.speed = self.speed;
        **player.steering = self.steering;
        *player.ground = self.ground;
    }

    pub fn player_relative_keys(&self) -> bool {
        self.input.using_key_control() && self.settings.player_relative_keys
    }

    /// Applies friction against the horizontal motion, then gravity.
    ///
    /// Port of `DoFrictionAndGravity` (original/src/Player/Player_Control.c).
    /// Each axis slows toward zero without changing sign.
    pub fn apply_friction_and_gravity(&mut self, friction: f32) {
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
    /// keeping the player on the terrain and letting slopes push it.
    ///
    /// Port of `DoPlayerMovementAndCollision`
    /// (original/src/Player/Player_Control.c). In a liquid the player floats
    /// just under the top of its volume, and the ball turns back into the
    /// bug to swim. Moving platforms and viscous traps arrive with those
    /// features.
    pub fn move_and_collide(&mut self, no_control: bool) {
        let tuning = self.tuning;
        let form = tuning.form(self.form);
        // From the last check, before this move's.
        let max_speed = if self.underwater.is_some() {
            tuning.swim_max_speed
        } else {
            form.max_speed
        };
        self.ground.on_ground = false;
        self.ground.on_terrain = false;
        let old_velocity = self.velocity;

        if !self.killed {
            self.apply_controls(no_control, max_speed);
        }

        // Split the move so that fast motion doesn't skip through things.
        let passes = (self.speed * self.dt / tuning.max_step) as u32 + 1;
        let dt = self.dt / passes as f32;
        for _ in 0..passes {
            let old_coord = self.coord;
            self.coord += self.velocity * dt;
            self.collide_with_objects(dt);

            if !self.killed
                && let Some(underwater) = self.underwater
            {
                // The ball can't swim (`InitPlayer_Bug` with
                // `PLAYER_ANIM_SWIM`).
                self.form = PlayerForm::Bug;
                // Splashes arrive with the particle effects.
                self.coord.y = underwater.volume_top - 1.0;
                self.velocity.y = -1.0;
            }

            // The original measured the speed without the vertical motion,
            // which slowed the bug on gentle slopes at high frame rates; the
            // modern port adds it back without the gravity this tick added.
            let real_speed = Vec3::new(
                self.velocity.x,
                self.velocity.y + tuning.gravity * dt,
                self.velocity.z,
            )
            .length();
            let shape = self.form.collision_box();
            let contact = collide_floor_and_ceiling(
                self.map,
                &mut self.coord,
                old_coord,
                &mut self.velocity,
                old_velocity,
                -shape.bottom,
                shape.top,
                real_speed,
                dt,
            );
            if contact.on_ground {
                self.ground.on_ground = true;
                self.ground.on_terrain = true;
            }
            self.ground.floor_normal = contact.floor_normal;

            if self.ground.on_terrain || self.ground.dist_to_floor < tuning.slope_ground_distance {
                let form = tuning.form(self.form);
                let normal = contact.floor_normal;
                let accel = if normal.y < tuning.steep_slope_normal_y {
                    // A steep slope throws the player back only if it hits
                    // it fast.
                    if self.velocity.y.abs() > tuning.steep_slope_fall_speed {
                        tuning.slope_accel * form.steep_slope_scale
                    } else {
                        0.0
                    }
                } else {
                    tuning.slope_accel * form.gentle_slope_scale
                };
                self.add_horizontal(normal.xz() * (accel * dt));
            }
        }

        let shape = self.form.collision_box();
        if let Some(fences) = self.fences {
            let feet = self.coord.y + shape.bottom;
            fences.collide(
                self.map,
                self.old_coord.xz(),
                &mut self.coord,
                &mut self.velocity,
                PLAYER_RADIUS * tuning.fence_radius_scale,
                feet,
            );
        }

        self.ground.dist_to_floor =
            self.coord.y + shape.bottom - self.map.floor_height(self.coord.x, self.coord.z);
    }

    /// The controls' push and the speed limit: part 1 of
    /// `DoPlayerMovementAndCollision`.
    fn apply_controls(&mut self, no_control: bool, max_speed: f32) {
        let tuning = self.tuning;
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
        if self.limit_speed && self.speed > max_speed {
            // Only the horizontal speed is limited; jumps and falls have
            // their own limits.
            let scale = max_speed / self.speed;
            self.velocity.x *= scale;
            self.velocity.z *= scale;
            self.speed = max_speed;
        }
    }

    /// Bumps into solid objects, sets off triggers, finds out whether the
    /// player is in a liquid, and notes what hurt it and which enemies it
    /// touched or bopped. A killed player only bumps into solid things.
    ///
    /// Port of `DoPlayerCollisionDetect` (original/src/Player/MyGuy.c).
    /// Platforms and viscous objects arrive with those features.
    fn collide_with_objects(&mut self, dt: f32) {
        let mover = BoxMover {
            entity: self.entity,
            is_player: true,
            shape: self.form.collision_box(),
            old_coord: self.old_coord,
            platform_velocity: Vec3::ZERO,
        };
        let result = resolve_box_collisions(
            &mover,
            &mut self.coord,
            &mut self.velocity,
            if self.killed {
                CollisionKind::Misc.into()
            } else {
                player_collision_mask()
            },
            self.candidates,
            dt,
            &mut self.spent,
        );
        if result.on_ground {
            self.ground.on_ground = true;
        }

        self.underwater = None;
        let candidates = self.candidates;
        for hit in &result.hits {
            let target = &candidates[hit.target];
            if self.spent.contains(&target.entity) {
                continue;
            }
            // Something that can't be pushed through sends the player back
            // to where it was safe, unless it landed on top.
            if target.kinds.has_all(CollisionKind::Impenetrable)
                && !target.kinds.has_all(CollisionKind::Impenetrable2)
                && !hit.sides.contains(SolidSides::BOTTOM)
            {
                self.coord.x = self.old_coord.x;
                self.coord.z = self.old_coord.z;
            }

            let damage = self
                .candidate_damage
                .get(hit.target)
                .copied()
                .unwrap_or(0.0);
            if target.kinds.has_all(CollisionKind::Enemy) {
                // Only landing on top of it bops it.
                if target.kinds.has_all(CollisionKind::Boppable)
                    && hit.sides.contains(SolidSides::BOTTOM)
                {
                    self.bops.push(EnemyBopped {
                        player: self.entity,
                        enemy: target.entity,
                    });
                } else {
                    self.hit_enemy(target, damage);
                }
            }
            if target.kinds.has_all(CollisionKind::HurtMe) {
                self.hurts.push(HurtPlayer {
                    knock: !target.kinds.has_all(CollisionKind::HurtNoKnock),
                    ..HurtPlayer::new(self.entity, Some(target.entity), damage)
                });
            }
            // A drain's damage is ball time per second.
            if target.kinds.has_all(CollisionKind::DrainBallTime) {
                self.ball_time_drained += damage * dt;
            }
            // Something solid underfoot wins over the liquid, so that
            // standing on things in it is reliable. The liquid must also be
            // above the floor here, not through it.
            if let Some(Some(liquid)) = self.candidate_liquids.get(hit.target)
                && !result.sides.contains(SolidSides::BOTTOM)
                && let Some(volume) = target.boxes.first()
            {
                let surface = volume.top + liquid.collision_top_offset();
                if self.map.floor_height(self.coord.x, self.coord.z) < surface {
                    self.underwater = Some(Underwater {
                        volume_top: volume.top,
                        liquid: *liquid,
                    });
                }
            }
        }
        for trigger in result.triggered {
            if !self.triggered.iter().any(|t| t.trigger == trigger.trigger) {
                self.triggered.push(trigger);
            }
        }
    }

    /// A spiked enemy hurts the player; the ball runs into the enemy.
    ///
    /// Port of `PlayerHitEnemy` (original/src/Player/MyGuy.c). Its switch
    /// on the enemy's kind becomes the messages that the enemies answer.
    fn hit_enemy(&mut self, enemy: &BoxTarget, damage: f32) {
        let spiked = enemy.kinds.has_all(CollisionKind::Spiked);
        if spiked {
            self.hurts
                .push(HurtPlayer::new(self.entity, Some(enemy.entity), damage));
        }
        self.touched.push(TouchedEnemy {
            player: self.entity,
            enemy: enemy.entity,
            spiked,
        });
        if self.form == PlayerForm::Ball {
            self.ball_hits.push(BallHitEnemy {
                player: self.entity,
                enemy: enemy.entity,
                ball_velocity: self.start_velocity,
                ball_speed: self.speed,
            });
        }
    }

    pub fn add_horizontal(&mut self, delta: Vec2) {
        self.velocity.x += delta.x;
        self.velocity.z += delta.y;
    }
}

/// A test bench that runs the player's movement on the real Lawn terrain.
#[cfg(test)]
pub(super) mod bench {
    use avian3d::prelude::LayerMask;

    use super::*;
    use crate::collision::CollisionBox;

    pub const DT: f32 = 1.0 / 60.0;
    /// An open, fairly flat spot near the Lawn's start.
    pub const START: Vec2 = Vec2::new(12720.0, 15780.0);

    pub struct Bench {
        pub map: TerrainMap,
        pub tuning: PlayerTuning,
        pub settings: ControlSettings,
    }

    impl Bench {
        pub fn lawn() -> Self {
            Self {
                map: TerrainMap::load_for_tests("Lawn", false),
                tuning: PlayerTuning::default(),
                settings: ControlSettings::default(),
            }
        }

        /// A player of the given form standing on the floor at [`START`].
        pub fn motion<'a>(
            &'a self,
            form: PlayerForm,
            input: &'a ControlInput,
            candidates: &'a [BoxTarget],
        ) -> Motion<'a> {
            let floor = self.map.floor_height(START.x, START.y);
            let coord = Vec3::new(START.x, floor - form.collision_box().bottom, START.y);
            Motion {
                entity: Entity::PLACEHOLDER,
                form,
                old_coord: coord,
                coord,
                yaw: 0.0,
                velocity: Vec3::ZERO,
                start_velocity: Vec3::ZERO,
                speed: 0.0,
                limit_speed: true,
                steering: Vec2::ZERO,
                ground: GroundContact::default(),
                tuning: &self.tuning,
                input,
                settings: &self.settings,
                map: &self.map,
                fences: None,
                candidates,
                candidate_liquids: vec![None; candidates.len()],
                candidate_damage: vec![0.0; candidates.len()],
                underwater: None,
                killed: false,
                spent: EntityHashSet::default(),
                triggered: Vec::new(),
                hurts: Vec::new(),
                touched: Vec::new(),
                ball_hits: Vec::new(),
                bops: Vec::new(),
                ball_time_drained: 0.0,
                camera_angle: 0.0,
                dt: DT,
            }
        }
    }

    impl Bench {
        /// A wide liquid around [`START`] whose surface is `depth` above
        /// the floor there.
        pub fn liquid(&self, kind: LiquidKind, depth: f32) -> BoxTarget {
            let floor = self.map.floor_height(START.x, START.y);
            let top = floor + depth - kind.collision_top_offset();
            let volume = CollisionBox::new(top, top - 2000.0, -2000.0, 2000.0, 2000.0, -2000.0)
                .at(Vec3::new(START.x, 0.0, START.y));
            BoxTarget {
                entity: Entity::from_raw_u32(9).expect("a valid index"),
                kinds: LayerMask::from([CollisionKind::Liquid, CollisionKind::BlockCamera]),
                solid: SolidSides::TOUCHABLE,
                boxes: vec![volume],
                old_boxes: vec![volume],
                velocity: Vec3::ZERO,
                trigger: None,
            }
        }
    }

    impl<'a> Motion<'a> {
        /// The next tick's motion: this one's result, with new input.
        pub fn next_tick(self, input: &'a ControlInput) -> Self {
            Motion {
                old_coord: self.coord,
                start_velocity: self.velocity,
                input,
                spent: EntityHashSet::default(),
                triggered: Vec::new(),
                hurts: Vec::new(),
                touched: Vec::new(),
                ball_hits: Vec::new(),
                bops: Vec::new(),
                ball_time_drained: 0.0,
                ..self
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use avian3d::prelude::LayerMask;

    use super::bench::{Bench, DT, START};
    use super::*;
    use crate::collision::CollisionBox;
    use crate::player::INVINCIBILITY_DURATION;

    const TARGET: u32 = 11;

    fn target_entity() -> Entity {
        Entity::from_raw_u32(TARGET).expect("a valid index")
    }

    /// A box of the given kinds and sides around the player at [`START`],
    /// from `bottom` to `top` above the floor.
    fn target(
        bench: &Bench,
        kinds: &[CollisionKind],
        solid: SolidSides,
        bottom: f32,
        top: f32,
    ) -> BoxTarget {
        let floor = bench.map.floor_height(START.x, START.y);
        let shape = CollisionBox::new(floor + top, floor + bottom, -100.0, 100.0, 100.0, -100.0)
            .at(Vec3::new(START.x, 0.0, START.y));
        BoxTarget {
            entity: target_entity(),
            kinds: kinds
                .iter()
                .fold(LayerMask::NONE, |mask, &kind| mask | kind),
            solid,
            boxes: vec![shape],
            old_boxes: vec![shape],
            velocity: Vec3::ZERO,
            trigger: None,
        }
    }

    /// A touch-only box of the given kinds around the player.
    fn around(bench: &Bench, kinds: &[CollisionKind]) -> [BoxTarget; 1] {
        [target(bench, kinds, SolidSides::TOUCHABLE, -50.0, 300.0)]
    }

    /// Moves a still player of the given form once among `targets`, each
    /// dealing `damage`.
    fn touch<'a>(
        bench: &'a Bench,
        form: PlayerForm,
        input: &'a ControlInput,
        targets: &'a [BoxTarget],
        damage: f32,
    ) -> Motion<'a> {
        let mut motion = bench.motion(form, input, targets);
        motion.candidate_damage = vec![damage; targets.len()];
        motion.move_and_collide(false);
        motion
    }

    #[test]
    fn hurting_objects_hurt_with_their_damage() {
        let bench = Bench::lawn();
        let input = ControlInput::default();
        let hurts = around(&bench, &[CollisionKind::HurtMe]);
        let motion = touch(&bench, PlayerForm::Bug, &input, &hurts, 0.2);
        let hurt = motion.hurts.first().expect("a hurt");
        assert_eq!(hurt.damage, 0.2);
        assert!(hurt.knock);
        assert!(!hurt.override_shield);
        assert_eq!(hurt.invincible_for, INVINCIBILITY_DURATION);
        assert_eq!(hurt.source, Some(target_entity()));
        assert!(motion.touched.is_empty());

        let no_knock = around(&bench, &[CollisionKind::HurtMe, CollisionKind::HurtNoKnock]);
        let motion = touch(&bench, PlayerForm::Bug, &input, &no_knock, 0.2);
        assert!(motion.hurts.first().is_some_and(|h| !h.knock));
    }

    #[test]
    fn ball_time_drains_drain_their_damage_per_second() {
        let bench = Bench::lawn();
        let input = ControlInput::default();
        let drain = around(&bench, &[CollisionKind::Misc, CollisionKind::DrainBallTime]);
        let motion = touch(&bench, PlayerForm::Ball, &input, &drain, 0.5);
        assert!((motion.ball_time_drained - 0.5 * DT).abs() < 1e-6);
        assert!(motion.hurts.is_empty());
    }

    #[test]
    fn a_spiked_enemy_hurts_and_the_ball_hits_it() {
        let bench = Bench::lawn();
        let input = ControlInput::default();
        let enemy = around(&bench, &[CollisionKind::Enemy, CollisionKind::Spiked]);
        let motion = touch(&bench, PlayerForm::Ball, &input, &enemy, 0.3);
        assert_eq!(motion.hurts.first().map(|h| h.damage), Some(0.3));
        assert!(motion.touched.first().is_some_and(|t| t.spiked));
        assert!(!motion.ball_hits.is_empty());
        assert!(motion.bops.is_empty());

        // The bug only touches it.
        let motion = touch(&bench, PlayerForm::Bug, &input, &enemy, 0.3);
        assert!(!motion.hurts.is_empty());
        assert!(motion.ball_hits.is_empty());
    }

    #[test]
    fn a_plain_enemy_is_touched_without_hurting() {
        let bench = Bench::lawn();
        let input = ControlInput::default();
        let enemy = around(&bench, &[CollisionKind::Enemy]);
        let motion = touch(&bench, PlayerForm::Bug, &input, &enemy, 0.3);
        assert!(motion.hurts.is_empty());
        assert!(motion.touched.first().is_some_and(|t| !t.spiked));
    }

    #[test]
    fn landing_on_a_boppable_enemy_bops_it() {
        let bench = Bench::lawn();
        let input = ControlInput::default();
        // An enemy whose top is 40 units above the floor; the bug comes
        // down onto it from above.
        let enemy = [target(
            &bench,
            &[CollisionKind::Enemy, CollisionKind::Boppable],
            SolidSides::ALL,
            -100.0,
            40.0,
        )];
        let mut motion = bench.motion(PlayerForm::Bug, &input, &enemy);
        motion.coord.y += 38.0;
        motion.old_coord = motion.coord + Vec3::Y * 20.0;
        motion.velocity = Vec3::new(0.0, -600.0, 0.0);
        motion.move_and_collide(true);
        assert_eq!(motion.bops.first().map(|b| b.enemy), Some(target_entity()));
        assert!(motion.touched.is_empty());

        // Walking into its side is only a touch.
        let mut motion = bench.motion(PlayerForm::Bug, &input, &enemy);
        motion.coord.z += 150.0;
        motion.old_coord = motion.coord;
        motion.velocity = Vec3::new(0.0, 0.0, -1200.0);
        motion.move_and_collide(true);
        assert!(motion.bops.is_empty());
        assert!(!motion.touched.is_empty());
    }
}
