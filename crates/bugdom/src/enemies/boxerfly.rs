//! The boxer fly: it hovers until the player comes near, chases it and
//! punches it across the lawn.
//!
//! Port of original/src/Enemies/Enemy_BoxerFly.c. It is both a map item
//! and a spline item ([`kind::BOXERFLY`]). A fly on a spline leaves it to
//! chase the player once the player is in range, and then never flies home.

use std::f32::consts::FRAC_PI_3;

use bevy::camera::primitives::Frustum;
use bevy::math::Affine3A;
use bevy::math::primitives::ViewFrustum;
use bevy::prelude::*;

use super::{
    BallHitEnemy, ENEMY_GRAVITY, EnemyBody, EnemyCollision, EnemyKicked, EnemyKilled, EnemyKind,
    EnemyModel, EnemySkeleton, EnemySpawner, EnemySystems, HomePosition, KICK_SPEED,
    apply_friction, death_enemy_collision_mask, default_enemy_collision_mask,
    detach_enemy_from_spline, move_enemy, per_frame_friction,
};
use crate::camera::GameCamera;
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind, SolidSides, solid_object};
use crate::items::{ItemSpawn, RegisterItemKind, TerrainItemSource, TerrainItems, kind};
use crate::math::{quick_distance, turn_toward, yaw_forward, yaw_from_point_to_point};
use crate::objects::Shadows;
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{HurtPlayer, Player, PlayerSystems};
use crate::skeleton::{
    AnimationFlags, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position,
};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::state::AppState;
use crate::terrain::TerrainMap;

/// The most boxer flies spawned from map items at once (`MAX_BOXERFLY`).
const MAX_BOXERFLY: usize = 4;

/// How close the player must come for a fly to chase it, in units
/// (`BOXERFLY_CHASE_RANGE`).
const CHASE_RANGE: f32 = 700.0;
/// How close a chasing fly must be to punch, in units
/// (`BOXERFLY_PUNCH_RANGE`).
const PUNCH_RANGE: f32 = 200.0;
/// How far off the player a chasing fly may still aim to punch, in radians
/// (the `PI/3` in `MoveBoxerFly_Flying`).
const PUNCH_AIM: f32 = FRAC_PI_3;
/// Turn speed, in radians per second (`BOXERFLY_TURN_SPEED`).
const TURN_SPEED: f32 = 4.0;
/// Flying speed when chasing or going home, in units per second
/// (`BOXERFLY_CHASE_SPEED`).
const CHASE_SPEED: f32 = 400.0;
/// Speed along a spline, in baked points per second
/// (`BOXERFLY_SPLINE_SPEED`).
const SPLINE_SPEED: f32 = 180.0;
/// How far from home a fly from a map item chases before it flies back,
/// in units (`MAX_BOXERFLY_RANGE`).
const MAX_RANGE: f32 = CHASE_RANGE + 1000.0;
/// How close to home a fly going home must get to wait again, in units.
const HOME_RANGE: f32 = 400.0;

/// `BOXERFLY_HEALTH`.
const HEALTH: f32 = 1.0;
/// What touching a fly does to the player (`BOXERFLY_DAMAGE`).
const DAMAGE: f32 = 0.04;
/// What a punch does to the player (`BOXERFLY_PUNCH_DAMAGE`).
const PUNCH_DAMAGE: f32 = 0.2;
/// `BOXERFLY_SCALE`.
const SCALE: f32 = 0.9;
/// Height above the floor it flies at, in units
/// (`BOXERFLY_FLIGHT_HEIGHT`).
const FLIGHT_HEIGHT: f32 = 110.0;
/// How fast the wobble's phase turns, in radians per second
/// (`BoxerFlyWobbleOff`).
const WOBBLE_RATE: f32 = 15.0;
/// How far the fly wobbles up and down, in units.
const WOBBLE_HEIGHT: f32 = 20.0;
/// Shadow size (`AttachShadowToObject(newObj, 7, 7, false)`).
const SHADOW_SCALE: f32 = 7.0;
/// The collision box (`SetObjectCollisionBounds(newObj, 70,-40,-40,40,40,-40)`).
const COLLISION_BOX: CollisionBox = CollisionBox::new(70.0, -40.0, -40.0, 40.0, 40.0, -40.0);

/// The animations (`BOXERFLY_ANIM_*`).
const ANIM_FLY: usize = 0;
const ANIM_PUNCH: usize = 1;
const ANIM_DEATH: usize = 2;
/// How fast it morphs between flying and punching, per second.
const MORPH_RATE: f32 = 3.0;

/// The animation flag the punch sets while the gloves can hit
/// (`PunchActive`, `Flag[0]`).
const PUNCH_ACTIVE_FLAG: usize = 0;
/// The elbow joints; the right one follows the left
/// (`BOXERFLY_LEFT_ELBOW_JOINT`, `BOXERFLY_RIGHT_ELBOW_JOINT`).
const LEFT_ELBOW_JOINT: usize = 8;
/// Where a glove is, in its elbow joint's space (`gloveOffset`).
const GLOVE_OFFSET: Vec3 = Vec3::new(-35.0, 0.0, -100.0);
/// Half a glove's hit box, up and down and across, in units.
const GLOVE_HALF_HEIGHT: f32 = 35.0;
const GLOVE_HALF_WIDTH: f32 = 25.0;
/// How much faster than the fly a punch sends the player, horizontally.
const PUNCH_FLING: f32 = 4.0;
/// How fast a punch sends the player up, in units per second.
const PUNCH_RISE: f32 = 1400.0;

/// How fast the ball must go to knock a fly down, in units per second
/// (`BOXERFLY_KNOCKDOWN_SPEED`).
const KNOCKDOWN_SPEED: f32 = 1400.0;
/// How much of the ball's horizontal velocity a knocked-down fly takes.
const BALL_KNOCK_SHARE: f32 = 0.5;
/// How fast the ball sends a fly up, in units per second.
const BALL_KNOCK_RISE: f32 = 400.0;
/// Friction on a dead fly on the ground, per frame at 60 fps
/// (`ApplyFrictionToDeltas(60.0, ...)`).
const DEATH_FRICTION_PER_FRAME: f32 = 60.0;

pub struct BoxerFlyPlugin;

impl Plugin for BoxerFlyPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Punched>()
            .register_item_kind(kind::BOXERFLY, add_boxer_fly)
            .register_spline_item_kind(kind::BOXERFLY, prime_boxer_fly)
            .add_systems(
                FixedUpdate,
                (kick_boxer_flies, ball_hit_boxer_flies, move_boxer_flies)
                    .chain()
                    .in_set(EnemySystems::Move),
            )
            .add_systems(
                FixedUpdate,
                move_boxer_flies_on_spline.in_set(SplineSystems::Move),
            )
            .add_systems(
                FixedUpdate,
                kill_hurt_boxer_flies.in_set(EnemySystems::Killed),
            )
            .add_systems(
                FixedUpdate,
                fling_punched_players
                    .after(PlayerSystems::Hurt)
                    .before(PlayerSystems::Respawn)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// What a fly is doing; it follows its animation, as the original indexes
/// its move table with `AnimNum`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BoxerFlyState {
    #[default]
    Flying,
    Punching,
    Dying,
}

/// Where a flying fly is going (`BOXERFLY_MODE_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BoxerFlyMode {
    /// Hovering, turned toward the player, until it comes in range.
    #[default]
    Waiting,
    GoHome,
    Chase,
}

/// A boxer fly's own state.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct BoxerFlyBrain {
    pub state: BoxerFlyState,
    pub mode: BoxerFlyMode,
    /// The wobble's phase, in radians (`Wobble`).
    pub wobble: f32,
    /// Whether it flies home when too far from it (`HonorRange`). Flies
    /// that left a spline don't.
    pub honor_range: bool,
}

impl Default for BoxerFlyBrain {
    fn default() -> Self {
        Self {
            state: BoxerFlyState::Flying,
            mode: BoxerFlyMode::Waiting,
            wobble: 0.0,
            honor_range: true,
        }
    }
}

impl BoxerFlyBrain {
    /// Advances the wobble by `dt` seconds and returns the height it adds.
    /// Port of `BoxerFlyWobbleOff`.
    fn wobble(&mut self, dt: f32) -> f32 {
        self.wobble += WOBBLE_RATE * dt;
        self.wobble.sin() * WOBBLE_HEIGHT
    }
}

/// A punch landed on a player: it flies off with this velocity once the
/// punch's hurt has knocked it over.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
struct Punched {
    player: Entity,
    velocity: Vec3,
}

/// The fly's skeleton, as `AddEnemy_BoxerFly` and `PrimeEnemy_BoxerFly`
/// set it up.
fn boxer_fly_skeleton(position: Vec2) -> EnemySkeleton {
    EnemySkeleton::new(EnemyKind::BoxerFly, SkeletonType::BoxerFly, position, SCALE)
        .anim(ANIM_FLY)
        .foot_offset(-FLIGHT_HEIGHT)
        .health(HEALTH)
        .damage(DAMAGE)
        .kickable()
        .solid(SolidSides::NOT_TOP)
        .collision_box(COLLISION_BOX)
        .shadow(SHADOW_SCALE)
}

/// Port of `AddEnemy_BoxerFly` (original/src/Enemies/Enemy_BoxerFly.c).
fn add_boxer_fly(In(spawn): In<ItemSpawn>, mut enemies: EnemySpawner) -> bool {
    if !enemies.can_spawn(EnemyKind::BoxerFly, MAX_BOXERFLY) {
        return false;
    }
    let Some(fly) = enemies.spawn(boxer_fly_skeleton(spawn.position).from_item(spawn.index)) else {
        return false;
    };
    enemies
        .commands()
        .entity(fly)
        .insert(BoxerFlyBrain::default());
    true
}

/// Port of `PrimeEnemy_BoxerFly` (original/src/Enemies/Enemy_BoxerFly.c),
/// which, like every prime routine, doesn't check the enemy counts.
fn prime_boxer_fly(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let on_spline = OnSpline::new(spawn.spline, spawn.placement, SPLINE_SPEED);
    let Some(fly) = enemies.spawn(boxer_fly_skeleton(spawn.position).on_spline(on_spline)) else {
        return false;
    };
    enemies
        .commands()
        .entity(fly)
        .insert(BoxerFlyBrain::default());
    true
}

/// The player a fly goes for: the nearest one (`gMyCoord`, with one
/// player).
fn nearest_player(at: Vec3, players: impl Iterator<Item = Vec3>) -> Option<Vec3> {
    players
        .min_by(|a, b| quick_distance(at.xz(), a.xz()).total_cmp(&quick_distance(at.xz(), b.xz())))
}

/// What one step of flying did.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FlightStep {
    yaw: f32,
    /// The velocity, if the step set it (`gDelta`); waiting leaves it.
    velocity: Option<Vec3>,
    /// Where it got to, in x and z.
    position: Vec2,
    /// Close enough and aimed well enough to start a punch.
    punch: bool,
}

/// Turns toward `target` and flies forward at [`CHASE_SPEED`] for `dt`
/// seconds. Returns the new yaw, velocity and position, and the angle
/// still to turn. The part `MoveBoxerFly_Flying` and
/// `MoveBoxerFly_Punching` share.
fn fly_toward(yaw: f32, position: Vec2, target: Vec2, dt: f32) -> (f32, Vec3, Vec2, f32) {
    let (yaw, aim) = turn_toward(yaw, position, target, TURN_SPEED * dt);
    let forward = yaw_forward(yaw) * CHASE_SPEED;
    let velocity = Vec3::new(forward.x, 0.0, forward.y);
    (yaw, velocity, position + forward * dt, aim)
}

/// One step of a flying fly, changing its mode as it goes.
///
/// Port of the mode switch in `MoveBoxerFly_Flying`
/// (original/src/Enemies/Enemy_BoxerFly.c). The vertical velocity stays as
/// it was, as the original only sets `gDelta.x` and `gDelta.z`.
fn fly_step(
    brain: &mut BoxerFlyBrain,
    yaw: f32,
    position: Vec2,
    home: Vec2,
    player: Vec2,
    dt: f32,
) -> FlightStep {
    match brain.mode {
        BoxerFlyMode::Waiting => {
            let (yaw, _) = turn_toward(yaw, position, player, TURN_SPEED * dt);
            if quick_distance(position, player) < CHASE_RANGE {
                brain.mode = BoxerFlyMode::Chase;
            }
            FlightStep {
                yaw,
                velocity: None,
                position,
                punch: false,
            }
        }
        BoxerFlyMode::GoHome => {
            let (yaw, velocity, position, _) = fly_toward(yaw, position, home, dt);
            if quick_distance(position, home) < HOME_RANGE {
                brain.mode = BoxerFlyMode::Waiting;
            }
            FlightStep {
                yaw,
                velocity: Some(velocity),
                position,
                punch: false,
            }
        }
        BoxerFlyMode::Chase => {
            let (yaw, velocity, position, aim) = fly_toward(yaw, position, player, dt);
            let punch = aim < PUNCH_AIM && quick_distance(position, player) < PUNCH_RANGE;
            if brain.honor_range && quick_distance(home, position) > MAX_RANGE {
                brain.mode = BoxerFlyMode::GoHome;
            }
            FlightStep {
                yaw,
                velocity: Some(velocity),
                position,
                punch,
            }
        }
    }
}

/// A glove's hit box around its point (`DoSimpleBoxCollisionAgainstPlayer`
/// in `MoveBoxerFly_Punching`).
fn glove_box(glove: Vec3) -> CollisionBox {
    CollisionBox::new(
        glove.y + GLOVE_HALF_HEIGHT,
        glove.y - GLOVE_HALF_HEIGHT,
        glove.x - GLOVE_HALF_WIDTH,
        glove.x + GLOVE_HALF_WIDTH,
        glove.z + GLOVE_HALF_WIDTH,
        glove.z - GLOVE_HALF_WIDTH,
    )
}

/// Whether a sphere is on the visible side of the camera's left, right,
/// near and far planes. Port of `IsSphereInFrustum_XZ`
/// (original/src/QD3D/FrustumCulling.c), which ignores the top and bottom.
fn in_frustum_xz(frustum: &Frustum, center: Vec3, radius: f32) -> bool {
    const PLANES: [usize; 4] = [
        0,
        1,
        ViewFrustum::NEAR_PLANE_IDX,
        ViewFrustum::FAR_PLANE_IDX,
    ];
    let center = center.extend(1.0);
    PLANES.iter().all(|&i| {
        frustum
            .half_spaces
            .get(i)
            .is_none_or(|plane| plane.normal_d().dot(center) + radius > 0.0)
    })
}

/// Keeps a killed fly's map item from spawning it again: it drops its
/// [`TerrainItemSource`], and the item stays marked as in use
/// (`theNode->TerrainItemPtr = nil`).
fn forget_terrain_item(fly: &mut EntityCommands) {
    fly.queue(|mut fly: EntityWorldMut| {
        let Some(TerrainItemSource(index)) = fly.take::<TerrainItemSource>() else {
            return;
        };
        fly.world_scope(|world| {
            if let Some(mut items) = world.get_resource_mut::<TerrainItems>() {
                items.mark_in_use(index);
            }
        });
    });
}

/// Kills a fly: it leaves its spline, stops counting as an enemy for
/// collisions, loses its shadow and falls dead. The caller sets its
/// velocity. Does nothing to a fly that is dying already.
///
/// Port of `KillBoxerFly` (original/src/Enemies/Enemy_BoxerFly.c), but for
/// the delta.
fn kill_boxer_fly(
    commands: &mut Commands,
    fly: Entity,
    brain: &mut BoxerFlyBrain,
    animator: Option<Mut<SkeletonAnimator>>,
    shape: CollisionBox,
) -> bool {
    if brain.state == BoxerFlyState::Dying {
        return false;
    }
    // Sound: stop EFFECT_BUZZ.
    let mut entity = commands.entity(fly);
    detach_enemy_from_spline(&mut entity);
    forget_terrain_item(&mut entity);
    // `CType = CTYPE_MISC; BottomOff = 0;`, keeping its solid sides.
    let shape = CollisionBox {
        bottom: 0.0,
        ..shape
    };
    entity
        .insert(solid_object(
            vec![shape],
            CollisionKind::Misc,
            SolidSides::NOT_TOP,
        ))
        .despawn_related::<Shadows>();
    brain.state = BoxerFlyState::Dying;
    if let Some(mut animator) = animator
        && animator.anim != ANIM_DEATH
    {
        animator.set_anim(ANIM_DEATH);
    }
    true
}

/// The parts of a fly's model that its move reads.
type ModelQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut SkeletonAnimator,
        &'static mut AnimationFlags,
        Option<&'static SkeletonRig>,
        &'static Transform,
    ),
    Without<BoxerFlyBrain>,
>;

/// The kick kills a fly, sending it flying.
///
/// Port of the `SKELETON_TYPE_BOXERFLY` case of `DoBugKick`
/// (original/src/Player/Player_Bug.c).
fn kick_boxer_flies(
    mut kicks: MessageReader<EnemyKicked>,
    mut commands: Commands,
    mut flies: Query<(
        &mut BoxerFlyBrain,
        &mut Velocity,
        &EnemyModel,
        &CollisionBoxes,
    )>,
    mut models: Query<&mut SkeletonAnimator, Without<BoxerFlyBrain>>,
) {
    for kick in kicks.read() {
        let Ok((mut brain, mut velocity, model, boxes)) = flies.get_mut(kick.enemy) else {
            continue;
        };
        let shape = boxes.0.first().copied().unwrap_or(COLLISION_BOX);
        if kill_boxer_fly(
            &mut commands,
            kick.enemy,
            &mut brain,
            models.get_mut(model.0).ok(),
            shape,
        ) {
            **velocity = kick.knock(KICK_SPEED, KICK_SPEED);
        }
    }
}

/// A fast enough ball knocks a fly dead.
///
/// Port of `BallHitBoxerFly` (original/src/Enemies/Enemy_BoxerFly.c).
fn ball_hit_boxer_flies(
    mut hits: MessageReader<BallHitEnemy>,
    mut commands: Commands,
    mut flies: Query<(
        &mut BoxerFlyBrain,
        &mut Velocity,
        &EnemyModel,
        &CollisionBoxes,
    )>,
    mut models: Query<&mut SkeletonAnimator, Without<BoxerFlyBrain>>,
) {
    for hit in hits.read() {
        let Ok((mut brain, mut velocity, model, boxes)) = flies.get_mut(hit.enemy) else {
            continue;
        };
        if hit.ball_speed <= KNOCKDOWN_SPEED {
            continue;
        }
        let shape = boxes.0.first().copied().unwrap_or(COLLISION_BOX);
        if kill_boxer_fly(
            &mut commands,
            hit.enemy,
            &mut brain,
            models.get_mut(model.0).ok(),
            shape,
        ) {
            let push = hit.ball_velocity * BALL_KNOCK_SHARE;
            **velocity = Vec3::new(push.x, BALL_KNOCK_RISE, push.z);
            // Sound: EFFECT_POUND at the fly, pitch kMiddleC+2, volume 2.0.
        }
    }
}

/// A fly whose health ran out dies where it is.
///
/// Port of the `ENEMY_KIND_BOXERFLY` case of `KillEnemy`
/// (original/src/Enemies/Enemy.c), which calls `KillBoxerFly` with no
/// delta.
fn kill_hurt_boxer_flies(
    mut killed: MessageReader<EnemyKilled>,
    mut commands: Commands,
    mut flies: Query<(
        &mut BoxerFlyBrain,
        &mut Velocity,
        &EnemyModel,
        &CollisionBoxes,
    )>,
    mut models: Query<&mut SkeletonAnimator, Without<BoxerFlyBrain>>,
) {
    for kill in killed.read() {
        let Ok((mut brain, mut velocity, model, boxes)) = flies.get_mut(kill.enemy) else {
            continue;
        };
        let shape = boxes.0.first().copied().unwrap_or(COLLISION_BOX);
        if kill_boxer_fly(
            &mut commands,
            kill.enemy,
            &mut brain,
            models.get_mut(model.0).ok(),
            shape,
        ) {
            **velocity = kill.knock;
        }
    }
}

/// Moves the flies that aren't on a spline: flying, punching or dying.
///
/// Port of `MoveBoxerFly`, `MoveBoxerFly_Flying`, `MoveBoxerFly_Punching`,
/// `MoveBoxerFly_Death` and `UpdateBoxerFly`
/// (original/src/Enemies/Enemy_BoxerFly.c). Going out of range is the
/// items' `DespawnOutOfRange`. Each fly goes for the nearest player.
#[allow(clippy::too_many_arguments)]
fn move_boxer_flies(
    mut commands: Commands,
    map: Res<TerrainMap>,
    mut collision: EnemyCollision,
    mut flies: Query<
        (EnemyBody, &mut BoxerFlyBrain, &EnemyModel, &HomePosition),
        Without<OnSpline>,
    >,
    mut models: ModelQuery,
    players: Query<(Entity, &Transform, &CollisionBoxes), (With<Player>, Without<BoxerFlyBrain>)>,
    cameras: Query<&Frustum, With<GameCamera>>,
    mut hurts: MessageWriter<HurtPlayer>,
    mut punched: MessageWriter<Punched>,
) {
    let dt = collision.dt();
    for (mut body, mut brain, model, home) in &mut flies {
        let fly = body.entity;
        let shape = body.boxes.0.first().copied().unwrap_or(COLLISION_BOX);

        if brain.state == BoxerFlyState::Dying {
            // Gone once the camera stopped seeing it.
            let at = body.transform.translation;
            let radius = **body.radius;
            if !cameras.iter().any(|f| in_frustum_xz(f, at, radius)) {
                commands.entity(fly).despawn();
                continue;
            }
            let mut velocity = **body.velocity;
            if body.ground.on_ground {
                apply_friction(
                    &mut velocity,
                    per_frame_friction(DEATH_FRICTION_PER_FRAME),
                    dt,
                );
            }
            velocity.y -= ENEMY_GRAVITY * dt;
            **body.velocity = velocity;
            move_enemy(&mut body.transform.translation, velocity, dt);
            collision.collide(&mut body, death_enemy_collision_mask(), &mut |_, _| false);
            continue;
        }

        let Ok((mut animator, mut flags, rig, model_transform)) = models.get_mut(model.0) else {
            continue;
        };
        // The joints are where the last frame left them, and so is the fly
        // (`FindCoordOnJoint` reads the last `UpdateObject`'s matrix).
        let base = Affine3A::from_rotation_translation(
            body.transform.rotation,
            body.transform.translation,
        ) * model_transform.compute_affine();
        let mut coord = body.transform.translation;
        let mut yaw = crate::math::yaw_of(body.transform.rotation);
        let target =
            nearest_player(coord, players.iter().map(|(_, t, _)| t.translation)).unwrap_or(coord);

        match brain.state {
            BoxerFlyState::Flying => {
                let step = fly_step(&mut brain, yaw, coord.xz(), home.xz(), target.xz(), dt);
                yaw = step.yaw;
                coord.x = step.position.x;
                coord.z = step.position.y;
                if let Some(v) = step.velocity {
                    body.velocity.x = v.x;
                    body.velocity.z = v.z;
                }
                if step.punch {
                    animator.morph_to(ANIM_PUNCH, MORPH_RATE);
                    flags.0[PUNCH_ACTIVE_FLAG] = false;
                    brain.state = BoxerFlyState::Punching;
                }
            }
            BoxerFlyState::Punching => {
                let (new_yaw, velocity, position, _) = fly_toward(yaw, coord.xz(), target.xz(), dt);
                yaw = new_yaw;
                coord.x = position.x;
                coord.z = position.y;
                body.velocity.x = velocity.x;
                body.velocity.z = velocity.z;

                if animator.has_stopped {
                    animator.morph_to(ANIM_FLY, MORPH_RATE);
                    flags.0[PUNCH_ACTIVE_FLAG] = false;
                    brain.state = BoxerFlyState::Flying;
                }

                if flags.0[PUNCH_ACTIVE_FLAG]
                    && let Some(rig) = rig
                {
                    let fly_velocity = **body.velocity;
                    for joint in [LEFT_ELBOW_JOINT, LEFT_ELBOW_JOINT + 1] {
                        let Some(glove) = joint_position(rig, joint, GLOVE_OFFSET, base) else {
                            continue;
                        };
                        let glove = glove_box(glove);
                        for (player, transform, boxes) in &players {
                            if !boxes
                                .0
                                .iter()
                                .any(|b| b.at(transform.translation).overlaps(&glove))
                            {
                                continue;
                            }
                            hurts.write(HurtPlayer::new(player, None, PUNCH_DAMAGE));
                            punched.write(Punched {
                                player,
                                velocity: Vec3::new(
                                    fly_velocity.x * PUNCH_FLING,
                                    PUNCH_RISE,
                                    fly_velocity.z * PUNCH_FLING,
                                ),
                            });
                            brain.mode = BoxerFlyMode::GoHome;
                        }
                    }
                }
            }
            BoxerFlyState::Dying => {}
        }

        coord.y = map.floor_height(coord.x, coord.z) + FLIGHT_HEIGHT + brain.wobble(dt);
        body.transform.translation = coord;
        body.transform.rotation = Quat::from_rotation_y(yaw);

        let model_entity = model.0;
        let models = &mut models;
        let brain = &mut *brain;
        collision.collide(&mut body, default_enemy_collision_mask(), &mut |_, _| {
            // `KillBoxerFly`'s delta is overwritten by `gDelta` when the
            // move ends, so the velocity is left alone here.
            kill_boxer_fly(
                &mut commands,
                fly,
                brain,
                models.get_mut(model_entity).ok().map(|(a, ..)| a),
                shape,
            );
            false
        });
        // Sound: start or update EFFECT_BUZZ at the fly.
    }
}

/// Moves the flies along their splines, and lets them off to chase a
/// player that comes in range.
///
/// Port of `MoveBoxerFlyOnSpline` (original/src/Enemies/Enemy_BoxerFly.c).
/// The collision box and shadow follow the transform by themselves.
fn move_boxer_flies_on_spline(
    time: Res<Time>,
    map: Res<TerrainMap>,
    splines: Option<Res<Splines>>,
    mut commands: Commands,
    mut flies: Query<(
        Entity,
        &mut Transform,
        &mut OnSpline,
        &PreviousPosition,
        &mut BoxerFlyBrain,
    )>,
    players: Query<&Transform, (With<Player>, Without<BoxerFlyBrain>)>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    for (fly, mut transform, mut on_spline, previous, mut brain) in &mut flies {
        if brain.state == BoxerFlyState::Dying {
            continue;
        }
        on_spline.advance(&splines, dt);
        let position = on_spline.position(&splines);
        transform.translation.x = position.x;
        transform.translation.z = position.y;

        if !on_spline.visible {
            // Sound: stop EFFECT_BUZZ.
            continue;
        }
        // Sound: start or update EFFECT_BUZZ at the fly.
        let yaw = yaw_from_point_to_point(
            crate::math::yaw_of(transform.rotation),
            previous.xz(),
            position,
        );
        transform.rotation = Quat::from_rotation_y(yaw);
        transform.translation.y =
            map.floor_height(position.x, position.y) + FLIGHT_HEIGHT + brain.wobble(dt);

        let at = transform.translation;
        let near = nearest_player(at, players.iter().map(|t| t.translation))
            .is_some_and(|player| quick_distance(at.xz(), player.xz()) < CHASE_RANGE);
        if near {
            detach_enemy_from_spline(&mut commands.entity(fly));
            brain.mode = BoxerFlyMode::Chase;
            brain.honor_range = false;
        }
    }
}

/// Sends punched players flying, after the punch's hurt has knocked them
/// on their butts (which stops them), as the original sets the player's
/// delta right after `PlayerGotHurt`. It happens even if the hurt did
/// nothing, as in the original.
///
/// Port of the end of the glove check in `MoveBoxerFly_Punching`
/// (original/src/Enemies/Enemy_BoxerFly.c).
fn fling_punched_players(
    mut punched: MessageReader<Punched>,
    mut players: Query<&mut Velocity, With<Player>>,
) {
    for punch in punched.read() {
        if let Ok(mut velocity) = players.get_mut(punch.player) {
            **velocity = punch.velocity;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn a_waiting_fly_turns_to_the_player_and_chases_once_it_is_near() {
        let mut brain = BoxerFlyBrain::default();
        let far = Vec2::new(0.0, -1000.0);
        let step = fly_step(&mut brain, 0.0, Vec2::ZERO, Vec2::ZERO, far, DT);
        assert_eq!(brain.mode, BoxerFlyMode::Waiting);
        assert_eq!(step.velocity, None);
        assert_eq!(step.position, Vec2::ZERO);

        let near = Vec2::new(0.0, -600.0);
        fly_step(&mut brain, 0.0, Vec2::ZERO, Vec2::ZERO, near, DT);
        assert_eq!(brain.mode, BoxerFlyMode::Chase);
    }

    #[test]
    fn a_chasing_fly_flies_at_the_player_and_punches_when_close_and_aimed() {
        let mut brain = BoxerFlyBrain {
            mode: BoxerFlyMode::Chase,
            ..default()
        };
        // Yaw 0 faces −Z, where the player is.
        let step = fly_step(
            &mut brain,
            0.0,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::new(0.0, -150.0),
            DT,
        );
        assert!(step.punch);
        let velocity = step.velocity.unwrap_or_default();
        assert!((velocity.z + CHASE_SPEED).abs() < 1e-3);
        assert!((step.position.y + CHASE_SPEED * DT).abs() < 1e-3);

        // Facing away, it turns but doesn't punch yet.
        let step = fly_step(
            &mut brain,
            std::f32::consts::PI,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::new(0.0, -150.0),
            DT,
        );
        assert!(!step.punch);
    }

    #[test]
    fn a_fly_too_far_from_home_goes_back_unless_it_left_a_spline() {
        let away = Vec2::new(MAX_RANGE + 100.0, 0.0);
        let player = away + Vec2::new(300.0, 0.0);
        let mut brain = BoxerFlyBrain {
            mode: BoxerFlyMode::Chase,
            ..default()
        };
        fly_step(&mut brain, 0.0, away, Vec2::ZERO, player, DT);
        assert_eq!(brain.mode, BoxerFlyMode::GoHome);

        let mut brain = BoxerFlyBrain {
            mode: BoxerFlyMode::Chase,
            honor_range: false,
            ..default()
        };
        fly_step(&mut brain, 0.0, away, Vec2::ZERO, player, DT);
        assert_eq!(brain.mode, BoxerFlyMode::Chase);

        let mut brain = BoxerFlyBrain {
            mode: BoxerFlyMode::GoHome,
            ..default()
        };
        fly_step(
            &mut brain,
            0.0,
            Vec2::new(300.0, 0.0),
            Vec2::ZERO,
            player,
            DT,
        );
        assert_eq!(brain.mode, BoxerFlyMode::Waiting);
    }

    #[test]
    fn the_wobble_turns_at_its_rate() {
        let mut brain = BoxerFlyBrain::default();
        let off = brain.wobble(0.1);
        assert!((brain.wobble - 1.5).abs() < 1e-6);
        assert!((off - 1.5f32.sin() * WOBBLE_HEIGHT).abs() < 1e-4);
    }

    #[test]
    fn killing_a_fly_makes_it_fall_dead_once() {
        let mut world = World::new();
        let fly = world.spawn(Shadows::default()).id();
        let mut brain = BoxerFlyBrain::default();
        let killed = bevy::ecs::system::RunSystemOnce::run_system_once(
            &mut world,
            move |mut commands: Commands| {
                let first = kill_boxer_fly(&mut commands, fly, &mut brain, None, COLLISION_BOX);
                let again = kill_boxer_fly(&mut commands, fly, &mut brain, None, COLLISION_BOX);
                (first, again, brain.state)
            },
        );
        assert_eq!(killed.ok(), Some((true, false, BoxerFlyState::Dying)));
        let boxes = world.get::<CollisionBoxes>(fly).map(|b| b.0.clone());
        assert_eq!(
            boxes,
            Some(vec![CollisionBox {
                bottom: 0.0,
                ..COLLISION_BOX
            }])
        );
    }
}
