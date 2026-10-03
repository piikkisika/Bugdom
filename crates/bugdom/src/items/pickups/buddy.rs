//! The buddy bug: a little bug that hides in some nuts. Once freed it
//! hovers around its player; on the buddy attack button it flies at the
//! nearest enemy and bursts on it, or on whatever solid it hits first.
//!
//! Port of `CreateMyBuddy`, `MoveMyBuddy`, `BuddyFollowsMe`,
//! `MoveBuddyTowardEnemy`, `SplatterBuddy` and the buddy part of
//! `ResetPlayer` (original/src/Player/MyGuy.c). The nut sends
//! [`SpawnBuddy`] (`CreateNutContents`).
//!
//! The original has one buddy (`gMyBuddy`); here each [`Buddy`] belongs to
//! a player, so each player can have one following it.

use avian3d::prelude::{LayerMask, SpatialQuery, TransformInterpolation};
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use super::SpawnBuddy;
use super::nut::is_out_of_range_far;
use crate::collision::{
    BoxMover, CollisionBox, CollisionBoxes, CollisionCandidates, CollisionKind, SolidSides,
    box_query, resolve_box_collisions,
};
use crate::effects::{
    EffectsSystems, FULL_ALPHA, ParticleFlags, ParticleGroupDesc, ParticleGroups, ParticleKind,
    ParticleTexture,
};
use crate::enemies::{Enemy, EnemyFinder, EnemySystems, HurtEnemy, ORIGINAL_FRAME_RATE};
use crate::input::{Action, ControlInput};
use crate::items::ItemWindow;
use crate::liquids::Underwater;
use crate::math::{GameRandom, quick_distance, turn_toward, yaw_forward, yaw_of};
use crate::objects::{ModelSpawner, attach_shadow};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{BugState, Player, PlayerForm, PlayerRespawned, PlayerSystems};
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonType};
use crate::state::{AppState, LevelAssets};
use crate::terrain::{LayerKind, TerrainMap};

pub(super) fn plugin(app: &mut App) {
    // The buddy moves right after the player (`PLAYER_SLOT+1`), before the
    // enemies, so its hurt is applied in the same tick.
    app.add_systems(
        FixedUpdate,
        (spawn_buddies, dismiss_buddies, move_buddies)
            .chain()
            .after(PlayerSystems::Move)
            .before(EnemySystems::Move)
            .before(EffectsSystems::MoveParticles)
            .run_if(in_state(AppState::InGame)),
    );
}

const BUDDY_SCALE: f32 = 0.3;
/// How high above the floor a new buddy starts, in units.
const BUDDY_START_HEIGHT: f32 = 30.0;
const BUDDY_BOX: CollisionBox = CollisionBox::new(40.0, -40.0, -40.0, 40.0, 40.0, -40.0);
const BUDDY_SHADOW_SCALE: f32 = 1.0;
/// The buddy's only animation.
const BUDDY_ANIM: usize = 0;

/// How far from the player the buddy likes to hover, in units
/// (`BUDDY_DIST_FROM_ME`).
const BUDDY_DIST_FROM_ME: f32 = 120.0;
/// How quickly the buddy closes on where it wants to be: the fraction of
/// the gap per second (`BUDDY_ACCEL`).
const BUDDY_ACCEL: f32 = 4.0;
/// The most of the gap on each of x and z that counts, in units.
const BUDDY_MAX_GAP: f32 = 500.0;
/// Within this distance of the player the buddy stays at its lowest
/// height, in units (`BUDDY_CLOSEST`).
const BUDDY_CLOSEST: f32 = 50.0;
/// How much higher it flies for each unit further away
/// (`BUDDY_HEIGHT_FACTOR`).
const BUDDY_HEIGHT_FACTOR: f32 = 0.3;
/// Its lowest height above the player's feet, in units (`BUDDY_MINY`).
const BUDDY_MINY: f32 = 160.0;
/// Half the size of the area checked for objects to fly over, and how far
/// above their top it then flies, in units.
const BUDDY_OBJECT_CLEARANCE: f32 = 100.0;
/// How far below a ceiling and above the floor it keeps, in units.
const BUDDY_CEILING_CLEARANCE: f32 = 30.0;
const BUDDY_FLOOR_CLEARANCE: f32 = 50.0;
/// How fast it turns toward the player, in radians per second.
const BUDDY_FOLLOW_TURN_SPEED: f32 = 3.0;

/// The attacking buddy's speed, in units per second (`BUDDY_ATTACK_SPEED`).
const BUDDY_ATTACK_SPEED: f32 = 1200.0;
/// How fast it turns toward its enemy, in radians per second.
const BUDDY_ATTACK_TURN_SPEED: f32 = 4.0;
/// How fast it climbs or dives to its enemy's height, in units per second².
const BUDDY_CLIMB_ACCELERATION: f32 = 400.0;
/// What is left of its vertical speed after one original frame at the
/// enemy's height.
const BUDDY_CLIMB_DAMPING_PER_FRAME: f32 = 0.5;
/// What it does to the enemy it hits: more than any enemy's health.
const BUDDY_DAMAGE: f32 = 1.1;
/// How far beyond the item window an attacking buddy is kept, in units
/// (`TrackTerrainItem_Far`).
const BUDDY_TRACK_RANGE: f32 = 500.0;

/// The sparks of a bursting buddy (`SplatterBuddy`).
const SPLATTER_SPARKS: ParticleGroupDesc = ParticleGroupDesc {
    kind: ParticleKind::Sparks,
    flags: ParticleFlags(ParticleFlags::BOUNCE.0 | ParticleFlags::HOT.0),
    gravity: 550.0,
    magnetism: 0.0,
    base_scale: 35.0,
    decay_rate: 0.7,
    fade_rate: 0.0,
    texture: ParticleTexture::White,
};
const SPLATTER_SPARK_COUNT: usize = 100;
/// The sparks' speed: the full width across, in units per second, and
/// where in it the vertical speed is centred (a little upward).
const SPLATTER_SPARK_SPEED: f32 = 800.0;
const SPLATTER_SPARK_RISE_BIAS: f32 = 0.1;
/// The size of the cube the sparks start in, in units.
const SPLATTER_SPREAD: f32 = 80.0;

/// What a buddy is doing (`BUDDY_MODE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuddyState {
    /// Hovering around its player (`BUDDY_MODE_LIKESME`).
    Follow,
    /// Flying at the nearest enemy (`BUDDY_MODE_ATTACK`). It belongs to
    /// no one any more.
    Attack,
}

/// A buddy bug, on its root entity. Its child is the skeleton.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buddy {
    /// The player it was freed for.
    pub player: Entity,
    pub state: BuddyState,
}

impl Buddy {
    /// Whether this buddy is still with `player` (`gMyBuddy`).
    pub fn follows(&self, player: Entity) -> bool {
        self.state == BuddyState::Follow && self.player == player
    }
}

/// Port of `CreateMyBuddy` (original/src/Player/MyGuy.c), for each nut
/// that held the buddy. Like the original it has no collision kinds of its
/// own: nothing else bumps into it.
fn spawn_buddies(
    mut commands: Commands,
    mut spawns: MessageReader<SpawnBuddy>,
    mut models: ModelSpawner,
    level_assets: Res<LevelAssets>,
    map: Res<TerrainMap>,
) {
    for spawn in spawns.read() {
        let Some(skeleton) = level_assets.skeleton(SkeletonType::Buddy) else {
            error!("The level has no buddy skeleton");
            continue;
        };
        let (x, z) = (spawn.position.x, spawn.position.z);
        let at = Vec3::new(x, map.floor_height(x, z) + BUDDY_START_HEIGHT, z);
        let buddy = commands
            .spawn((
                Name::new("Buddy"),
                Buddy {
                    player: spawn.player,
                    state: BuddyState::Follow,
                },
                Transform::from_translation(at),
                Visibility::default(),
                TransformInterpolation,
                PreviousPosition(at),
                Velocity::default(),
                CollisionBoxes(vec![BUDDY_BOX]),
                CollisionCandidates::default(),
                DespawnOnExit(AppState::InGame),
            ))
            .id();
        let mut animator = SkeletonAnimator::default();
        animator.set_anim(BUDDY_ANIM);
        commands.spawn((
            Name::new("Buddy model"),
            Skeleton(skeleton),
            animator,
            Transform::from_scale(Vec3::splat(BUDDY_SCALE)),
            TransformInterpolation,
            ChildOf(buddy),
        ));
        attach_shadow(
            &mut commands,
            &mut models,
            buddy,
            Vec2::splat(BUDDY_SHADOW_SCALE),
            true,
        );
    }
}

/// A restarted player loses the buddy that follows it; one already on
/// the attack carries on. Port of the end of `ResetPlayer`
/// (original/src/Player/MyGuy.c).
fn dismiss_buddies(
    mut commands: Commands,
    mut respawned: MessageReader<PlayerRespawned>,
    buddies: Query<(Entity, &Buddy)>,
) {
    for PlayerRespawned(player) in respawned.read() {
        for (entity, buddy) in &buddies {
            if buddy.follows(*player) {
                commands.entity(entity).try_despawn();
            }
        }
    }
}

/// Whether the player is free to act (`gPlayerCanMove`): always as the
/// ball; as the bug, unless an animation or something else has hold of it.
/// Port of the states of `MovePlayer_Bug` and `MovePlayer_Ball`
/// (original/src/Player) that set it.
fn player_can_move(form: PlayerForm, state: BugState) -> bool {
    form == PlayerForm::Ball
        || matches!(
            state,
            BugState::Stand | BugState::Walk | BugState::Jump | BugState::Fall | BugState::Swim
        )
}

/// The objects the buddy flies over or bursts on.
type Solids<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        &'static CollisionBoxes,
        &'static SolidSides,
    ),
    Without<Buddy>,
>;

/// Moves the buddies. Port of `MoveMyBuddy` (original/src/Player/MyGuy.c).
#[allow(clippy::too_many_arguments)]
fn move_buddies(
    time: Res<Time>,
    mut commands: Commands,
    map: Res<TerrainMap>,
    window: Option<Res<ItemWindow>>,
    spatial: SpatialQuery,
    solids: Solids,
    finder: EnemyFinder,
    mut groups: ResMut<ParticleGroups>,
    mut random: ResMut<GameRandom>,
    mut hurts: MessageWriter<HurtEnemy>,
    mut buddies: Query<
        (
            Entity,
            &mut Buddy,
            &mut Transform,
            &mut Velocity,
            &PreviousPosition,
            &CollisionCandidates,
            Has<Underwater>,
        ),
        Without<Enemy>,
    >,
    players: Query<
        (
            &Transform,
            &CollisionBoxes,
            &ControlInput,
            &PlayerForm,
            &BugState,
            Option<&Underwater>,
        ),
        (With<Player>, Without<Buddy>),
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, mut buddy, mut transform, mut velocity, previous, candidates, underwater) in
        &mut buddies
    {
        // Sound: EFFECT_BUZZ, looping at the buddy.
        let mut coord = transform.translation;
        let mut yaw = yaw_of(transform.rotation);
        match buddy.state {
            BuddyState::Follow => {
                let Ok((player, boxes, input, form, state, player_underwater)) =
                    players.get(buddy.player)
                else {
                    continue;
                };
                let feet = player.translation.y + boxes.0.first().map_or(0.0, |b| b.bottom);
                let me = Vec3::new(player.translation.x, feet, player.translation.z);
                let (next, mut target) = follow_target(coord, me, dt);
                if let Some(top) = top_of_object_near(&spatial, &solids, target) {
                    target.y = top + BUDDY_OBJECT_CLEARANCE;
                }
                coord = Vec3::new(
                    next.x,
                    coord.y + (target.y - coord.y) * BUDDY_ACCEL * dt,
                    next.y,
                );
                coord.y = keep_between_floor_and_ceiling(&map, coord);
                yaw = turn_toward(yaw, coord.xz(), me.xz(), BUDDY_FOLLOW_TURN_SPEED * dt).0;

                // In a liquid with the player, so that its shadow goes too.
                match (player_underwater, underwater) {
                    (Some(liquid), false) => {
                        commands.entity(entity).insert(*liquid);
                    }
                    (None, true) => {
                        commands.entity(entity).remove::<Underwater>();
                    }
                    _ => {}
                }

                if input.just_pressed(Action::BuddyAttack) && player_can_move(*form, *state) {
                    buddy.state = BuddyState::Attack;
                    // Sound: EFFECT_BUDDYLAUNCH at the buddy.
                }
            }
            BuddyState::Attack => {
                // `MoveBuddyTowardEnemy`
                if let Some((enemy, _)) = finder.closest(coord)
                    && let Ok((enemy_transform, enemy_boxes, _)) = solids.get(enemy)
                {
                    let at = enemy_transform.translation;
                    yaw = turn_toward(yaw, coord.xz(), at.xz(), BUDDY_ATTACK_TURN_SPEED * dt).0;
                    let span = enemy_boxes
                        .0
                        .first()
                        .map(|b| (at.y + b.bottom, at.y + b.top));
                    velocity.y = climb_toward(velocity.y, coord.y, span, dt);
                }
                let forward = yaw_forward(yaw) * BUDDY_ATTACK_SPEED;
                velocity.x = forward.x;
                velocity.z = forward.y;
                coord += **velocity * dt;

                let floor = map.floor_height(coord.x, coord.z);
                let ceiling = map.height_at(coord.x, coord.z, LayerKind::Ceiling).0;
                let hit_solid = coord.y < floor
                    || coord.y > ceiling
                    || !box_query(
                        &spatial,
                        &solids,
                        CollisionBox::new(coord.y, coord.y, coord.x, coord.x, coord.z, coord.z),
                        CollisionKind::Misc,
                    )
                    .is_empty();
                let enemy = if hit_solid {
                    None
                } else {
                    hit_enemy(entity, previous, candidates, &mut coord, &mut velocity, dt)
                };
                if let Some(enemy) = enemy {
                    hurts.write(HurtEnemy {
                        enemy,
                        damage: BUDDY_DAMAGE,
                    });
                }
                if hit_solid || enemy.is_some() {
                    splatter(&mut commands, entity, coord, &mut groups, &mut random);
                    continue;
                }
                if window.as_ref().is_some_and(|window| {
                    is_out_of_range_far(window, coord.xz(), BUDDY_TRACK_RANGE)
                }) {
                    commands.entity(entity).despawn();
                    continue;
                }
            }
        }
        transform.translation = coord;
        transform.rotation = Quat::from_rotation_y(yaw);
    }
}

/// Where a following buddy at `coord` goes this tick in x and z, and the
/// point it aims for: [`BUDDY_DIST_FROM_ME`] from the player on the
/// buddy's side, at a height that grows the further it is from the player
/// (before flying over objects). It closes the gap at [`BUDDY_ACCEL`].
/// `me` is the player's feet. Port of the first half of `BuddyFollowsMe`
/// (original/src/Player/MyGuy.c).
fn follow_target(coord: Vec3, me: Vec3, dt: f32) -> (Vec2, Vec3) {
    let side = (coord.xz() - me.xz()).normalize_or_zero();
    let spot = me.xz() + side * BUDDY_DIST_FROM_ME;
    let gap = (spot - coord.xz()).clamp(Vec2::splat(-BUDDY_MAX_GAP), Vec2::splat(BUDDY_MAX_GAP));
    let next = coord.xz() + gap * (dt * BUDDY_ACCEL);
    let away = (quick_distance(next, me.xz()) - BUDDY_CLOSEST).max(0.0);
    let height = me.y + away * BUDDY_HEIGHT_FACTOR + BUDDY_MINY;
    (next, Vec3::new(spot.x, height, spot.y))
}

/// The top of the first box of the first object near `target` that the
/// buddy should fly over (`DoSimpleBoxCollision` with
/// `CTYPE_MISC|CTYPE_ENEMY|CTYPE_TRIGGER`).
fn top_of_object_near(spatial: &SpatialQuery, solids: &Solids, target: Vec3) -> Option<f32> {
    let c = BUDDY_OBJECT_CLEARANCE;
    let area = CollisionBox::new(
        target.y + c,
        target.y - c,
        target.x - c,
        target.x + c,
        target.z + c,
        target.z - c,
    );
    let kinds = LayerMask::from([
        CollisionKind::Misc,
        CollisionKind::Enemy,
        CollisionKind::Trigger,
    ]);
    let hit = box_query(spatial, solids, area, kinds).into_iter().next()?;
    let (transform, boxes, _) = solids.get(hit.entity).ok()?;
    Some(boxes.0.first()?.top + transform.translation.y)
}

/// The buddy's height kept under any ceiling and above the floor.
fn keep_between_floor_and_ceiling(map: &TerrainMap, coord: Vec3) -> f32 {
    let mut y = coord.y;
    if map.layer(LayerKind::Ceiling).is_some() {
        let ceiling = map.height_at(coord.x, coord.z, LayerKind::Ceiling).0;
        y = y.min(ceiling - BUDDY_CEILING_CLEARANCE);
    }
    y.max(map.floor_height(coord.x, coord.z) + BUDDY_FLOOR_CLEARANCE)
}

/// The attacking buddy's new vertical speed: it climbs or dives toward the
/// enemy's height `span` (bottom, top), and steadies once within it.
///
/// The original halves the speed once per frame; this halves it at that
/// rate per second at the original's 60 fps, so it doesn't depend on the
/// frame rate.
fn climb_toward(speed: f32, y: f32, span: Option<(f32, f32)>, dt: f32) -> f32 {
    match span {
        Some((bottom, _)) if y < bottom => speed + BUDDY_CLIMB_ACCELERATION * dt,
        Some((_, top)) if y > top => speed - BUDDY_CLIMB_ACCELERATION * dt,
        Some(_) => speed * BUDDY_CLIMB_DAMPING_PER_FRAME.powf(dt * ORIGINAL_FRAME_RATE),
        None => speed,
    }
}

/// The enemy the attacking buddy flew into this tick, if any
/// (`HandleCollisions(theNode, CTYPE_ENEMY)`).
fn hit_enemy(
    entity: Entity,
    previous: &PreviousPosition,
    candidates: &CollisionCandidates,
    coord: &mut Vec3,
    velocity: &mut Velocity,
    dt: f32,
) -> Option<Entity> {
    let mover = BoxMover {
        entity,
        is_player: false,
        shape: BUDDY_BOX,
        old_coord: **previous,
        platform_velocity: Vec3::ZERO,
    };
    let collisions = resolve_box_collisions(
        &mover,
        coord,
        velocity,
        LayerMask::from(CollisionKind::Enemy),
        &candidates.0,
        dt,
        &mut EntityHashSet::default(),
    );
    let first = collisions.hits.first()?;
    candidates.0.get(first.target).map(|t| t.entity)
}

/// Bursts the buddy into white sparks. Port of `SplatterBuddy`
/// (original/src/Player/MyGuy.c).
fn splatter(
    commands: &mut Commands,
    buddy: Entity,
    at: Vec3,
    groups: &mut ParticleGroups,
    random: &mut GameRandom,
) {
    commands.entity(buddy).despawn();
    if let Some(group) = groups.new_group(SPLATTER_SPARKS) {
        for _ in 0..SPLATTER_SPARK_COUNT {
            let velocity = Vec3::new(
                (random.next_f32() - 0.5) * SPLATTER_SPARK_SPEED,
                (random.next_f32() - 0.5 + SPLATTER_SPARK_RISE_BIAS) * SPLATTER_SPARK_SPEED,
                (random.next_f32() - 0.5) * SPLATTER_SPARK_SPEED,
            );
            let position = at
                + Vec3::new(
                    (random.next_f32() - 0.5) * SPLATTER_SPREAD,
                    (random.next_f32() - 0.5) * SPLATTER_SPREAD,
                    (random.next_f32() - 0.5) * SPLATTER_SPREAD,
                );
            let scale = random.next_f32() + 1.0;
            groups.add_particle(group, position, velocity, scale, FULL_ALPHA);
        }
    }
    // Sound: EFFECT_FIRECRACKER at the buddy.
}
