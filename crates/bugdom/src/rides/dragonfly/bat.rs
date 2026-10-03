//! The bat that swallows a bug flying the dragonfly too high.
//!
//! Port of `MakeBat`, `MoveBat` and `SeeIfBatEatsPlayer`
//! (original/src/Items/Traps.c). The dragonfly calls it ([`BatCaller`]);
//! it dives straight down onto the bug that called it and, once its mouth
//! reaches the bug, swallows it ([`Hold::Eaten`], which also takes the bug
//! off the dragonfly) and kills it, then circles away with it.
//!
//! The original draws the bat without fog (`STATUS_BIT_NOFOG`); skeletons
//! can't do that yet, so it is fogged like everything else.

use avian3d::prelude::TransformInterpolation;
use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::prelude::*;

use crate::collision::{CollisionBox, CollisionKind, SolidSides, solid_object};
use crate::enemies::EnemyModel;
use crate::items::{DespawnOutOfRange, ItemSystems};
use crate::physics::Velocity;
use crate::player::{Hold, HoldPlayer, KillPlayer, Player, PlayerForm, PlayerSystems};
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonRig, SkeletonType, joint_position};
use crate::state::{AppState, LevelAssets};
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        FixedUpdate,
        // The bat is just ahead of the player in the original's object list
        // (`PLAYER_SLOT - 1`), so it sees where the player was last tick.
        move_bats
            .after(ItemSystems::Track)
            .before(PlayerSystems::Ride)
            .before(PlayerSystems::Move)
            .run_if(in_state(AppState::InGame)),
    );
}

/// `BAT_SCALE`
const BAT_SCALE: f32 = 8.0;
/// How far above where it was called the bat starts its dive, in units.
const START_HEIGHT: f32 = 1400.0;
/// Its box (`SetObjectCollisionBounds(newObj, 100, -100, -200, 200, 200,
/// -200)`). Nothing collides with it, as it has no solid sides.
const BAT_BOX: CollisionBox = CollisionBox::new(100.0, -100.0, -200.0, 200.0, 200.0, -200.0);

/// How fast it dives, in units per second.
const DIVE_SPEED: f32 = 2000.0;
/// How fast it pulls out of the dive once it has the bug, in units per
/// second squared.
const PULL_UP: f32 = 2500.0;
/// How fast it circles away, in radians per second, and its speed, in units
/// per second.
const CIRCLE_TURN_RATE: f32 = 0.9;
const CIRCLE_SPEED: f32 = 1500.0;

/// The head joint (`BAT_JOINT_HEAD`) and the mouth in its space
/// (`gBatMouthOff`).
pub const BAT_JOINT_HEAD: usize = 1;
pub const BAT_MOUTH_OFFSET: Vec3 = Vec3::new(0.0, -8.0, -20.0);
/// How far above the bug's feet the mouth swallows it, in units.
const MOUTH_REACH: f32 = 30.0;

/// The bat's animations (`BAT_ANIM_*`), and how fast it morphs into flying
/// away, per second.
const ANIM_FLY_UP: usize = 0;
const ANIM_DIVE: usize = 1;
const FLY_UP_MORPH_RATE: f32 = 2.0;

/// What the bat does; it follows its animation, as the original switches on
/// `AnimNum`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BatState {
    /// Diving onto the bug (`BAT_ANIM_DIVE`).
    #[default]
    Dive,
    /// Circling away with the bug in its mouth (`BAT_ANIM_FLYUP`).
    FlyUp,
}

/// The bat. There is only one at a time (`gBatExists`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Bat {
    pub state: BatState,
    /// The player it dives on: the one whose dragonfly called it
    /// (`gMyCoord`).
    pub prey: Entity,
    /// Its heading (`Rot.y`).
    pub yaw: f32,
    /// The skeleton's entity, under the bat's.
    pub model: Entity,
}

impl Bat {
    /// Dives for `dt` seconds above the prey, at `prey_xz`. Returns false
    /// once it has gone below the floor, where it is deleted.
    fn dive(coord: &mut Vec3, velocity: &mut Vec3, prey_xz: Vec2, floor: f32, dt: f32) -> bool {
        velocity.y = -DIVE_SPEED;
        coord.y += velocity.y * dt;
        coord.x = prey_xz.x;
        coord.z = prey_xz.y;
        coord.y >= floor
    }

    /// Pulls out of the dive and circles for `dt` seconds.
    fn fly_up(&mut self, coord: &mut Vec3, velocity: &mut Vec3, dt: f32) {
        velocity.y = (velocity.y + PULL_UP * dt).min(0.0);
        self.yaw -= CIRCLE_TURN_RATE * dt;
        velocity.x = self.yaw.sin() * -CIRCLE_SPEED;
        velocity.z = self.yaw.cos() * -CIRCLE_SPEED;
        *coord += *velocity * dt;
    }
}

/// Whether the bat's mouth has reached a bug whose feet are at `prey_y`
/// (`SeeIfBatEatsPlayer`). It can't eat the ball.
fn reaches(mouth: Vec3, prey_y: f32, form: PlayerForm) -> bool {
    form != PlayerForm::Ball && mouth.y <= prey_y + MOUTH_REACH
}

/// Calls the bat, for the dragonflies.
#[derive(SystemParam)]
pub struct BatCaller<'w, 's> {
    commands: Commands<'w, 's>,
    level_assets: Res<'w, LevelAssets>,
    bats: Query<'w, 's, (), With<Bat>>,
}

impl BatCaller<'_, '_> {
    /// Makes a bat dive on the first caller's player, from above where it
    /// called, unless there is a bat already. `calls` are the players and
    /// the points they called from.
    ///
    /// Port of `MakeBat` (original/src/Items/Traps.c).
    pub fn call(&mut self, calls: impl IntoIterator<Item = (Entity, Vec3)>) {
        if !self.bats.is_empty() {
            return;
        }
        let Some((prey, at)) = calls.into_iter().next() else {
            return;
        };
        let Some(skeleton) = self.level_assets.skeleton(SkeletonType::Bat) else {
            error!("The level has no bat skeleton");
            return;
        };
        let mut animator = SkeletonAnimator::default();
        animator.set_anim(ANIM_DIVE);
        let model = self
            .commands
            .spawn((
                Name::new("Bat model"),
                Skeleton(skeleton),
                animator,
                Transform::from_scale(Vec3::splat(BAT_SCALE)),
                TransformInterpolation,
            ))
            .id();
        self.commands
            .spawn((
                Name::new("Bat"),
                Bat {
                    state: BatState::Dive,
                    prey,
                    yaw: 0.0,
                    model,
                },
                // The player's hold finds the mouth through it.
                EnemyModel(model),
                Transform::from_translation(at + Vec3::Y * START_HEIGHT),
                Visibility::default(),
                TransformInterpolation,
                Velocity::default(),
                solid_object(vec![BAT_BOX], CollisionKind::BlockCamera, SolidSides::NONE),
                DespawnOutOfRange,
                DespawnOnExit(AppState::InGame),
            ))
            .add_child(model);
    }
}

/// Moves the bat: diving onto its bug, swallowing it, and circling away with
/// it. Going out of range is the items' [`DespawnOutOfRange`].
///
/// Port of `MoveBat` and `SeeIfBatEatsPlayer` (original/src/Items/Traps.c).
/// The original's check for a culled bat that has no bug never applies, as
/// the bat only flies away once it has one.
#[allow(clippy::type_complexity)]
fn move_bats(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    mut bats: Query<(Entity, &mut Bat, &mut Transform, &mut Velocity)>,
    mut models: Query<(&mut SkeletonAnimator, Option<&SkeletonRig>, &Transform), Without<Bat>>,
    players: Query<(&Transform, &PlayerForm), (With<Player>, Without<Bat>)>,
    mut holds: MessageWriter<HoldPlayer>,
    mut kills: MessageWriter<KillPlayer>,
) {
    let dt = time.delta_secs();
    for (entity, mut bat, mut transform, mut velocity) in &mut bats {
        let mut coord = transform.translation;
        let mut v = **velocity;
        match bat.state {
            BatState::Dive => {
                let prey = players.get(bat.prey).ok();
                let Ok((mut animator, rig, model_transform)) = models.get_mut(bat.model) else {
                    continue;
                };
                // The mouth as the last frame drew it (`FindCoordOnJoint`).
                let base = Affine3A::from_rotation_translation(transform.rotation, coord)
                    * model_transform.compute_affine();
                let mouth =
                    rig.and_then(|rig| joint_position(rig, BAT_JOINT_HEAD, BAT_MOUTH_OFFSET, base));
                let eats = matches!(
                    (mouth, prey),
                    (Some(mouth), Some((prey, form))) if reaches(mouth, prey.translation.y, *form)
                );
                if eats {
                    bat.state = BatState::FlyUp;
                    animator.morph_to(ANIM_FLY_UP, FLY_UP_MORPH_RATE);
                    // The bug leaves its dragonfly, goes into the mouth and
                    // out of others' collision, and dies there.
                    holds.write(HoldPlayer {
                        player: bat.prey,
                        hold: Hold::Eaten {
                            by: entity,
                            joint: BAT_JOINT_HEAD,
                            mouth_offset: BAT_MOUTH_OFFSET,
                            // On the Forest levels the camera follows the
                            // bat.
                            follow: true,
                        },
                    });
                    kills.write(KillPlayer {
                        player: bat.prey,
                        change_anims: false,
                    });
                } else {
                    let prey_xz = prey.map_or(coord.xz(), |(t, _)| t.translation.xz());
                    let floor = map.floor_height(prey_xz.x, prey_xz.y);
                    if !Bat::dive(&mut coord, &mut v, prey_xz, floor, dt) {
                        commands.entity(entity).despawn();
                        continue;
                    }
                }
            }
            BatState::FlyUp => bat.fly_up(&mut coord, &mut v, dt),
        }
        transform.translation = coord;
        transform.rotation = Quat::from_rotation_y(bat.yaw);
        **velocity = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn the_bat_dives_onto_its_prey_until_it_is_under_the_floor() {
        let mut coord = Vec3::new(0.0, 1000.0, 0.0);
        let mut v = Vec3::ZERO;
        let prey = Vec2::new(50.0, -30.0);
        assert!(Bat::dive(&mut coord, &mut v, prey, 0.0, DT));
        assert_eq!(coord.xz(), prey);
        assert_eq!(v.y, -DIVE_SPEED);
        assert!((coord.y - (1000.0 - DIVE_SPEED * DT)).abs() < 1e-3);
        let ticks = (1..100)
            .find(|_| !Bat::dive(&mut coord, &mut v, prey, 0.0, DT))
            .expect("it goes under");
        // 1000 units at 2000 per second.
        assert!((29..=31).contains(&ticks), "{ticks}");
    }

    #[test]
    fn it_swallows_the_bug_but_not_the_ball() {
        let mouth = Vec3::new(0.0, 129.0, 0.0);
        assert!(reaches(mouth, 100.0, PlayerForm::Bug));
        assert!(!reaches(mouth, 98.0, PlayerForm::Bug));
        assert!(!reaches(mouth, 100.0, PlayerForm::Ball));
    }

    #[test]
    fn with_the_bug_it_pulls_out_of_the_dive_and_circles() {
        let mut bat = Bat {
            state: BatState::FlyUp,
            prey: Entity::PLACEHOLDER,
            yaw: 0.0,
            model: Entity::PLACEHOLDER,
        };
        let mut coord = Vec3::ZERO;
        let mut v = Vec3::new(0.0, -DIVE_SPEED, 0.0);
        bat.fly_up(&mut coord, &mut v, DT);
        assert!(v.y < 0.0 && v.y > -DIVE_SPEED);
        for _ in 0..60 {
            bat.fly_up(&mut coord, &mut v, DT);
        }
        // Never climbing, it levels out and circles.
        assert_eq!(v.y, 0.0);
        assert!((v.xz().length() - CIRCLE_SPEED).abs() < 1e-2);
        assert!((bat.yaw + CIRCLE_TURN_RATE * 61.0 * DT).abs() < 1e-4);
    }
}
