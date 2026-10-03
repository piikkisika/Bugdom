//! The foot: a giant foot that stomps along a spline on the Beach level.
//! It rises, comes down and rests, moving along its spline faster the
//! higher it is, and hurts whatever it touches.
//!
//! Port of `PrimeFoot`, `CalcFootCollisionBoxes` and `MoveFootOnSpline`
//! (original/src/Items/Traps.c). The foot is a spline item only
//! (`NilAdd` as a map item).

use std::f32::consts::PI;

use avian3d::prelude::{Collider, ColliderDisabled, TransformInterpolation};
use bevy::prelude::*;

use super::super::kind as item;
use super::TrapModel;
use crate::collision::{CollisionBox, CollisionBoxes, CollisionKind, SolidSides, solid_object};
use crate::combat::Damage;
use crate::math::{GameRandom, yaw_from_point_to_point, yaw_of};
use crate::physics::PreviousPosition;
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonType};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::state::LevelAssets;
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_spline_item_kind(item::FOOT, prime_foot)
        .add_systems(FixedUpdate, move_feet.in_set(SplineSystems::Move));
}

/// How high the foot lifts above the floor, in units (`FOOT_APEX`).
const FOOT_APEX: f32 = 950.0;
/// Speed along the spline per unit of height above the floor, in baked
/// points per second (`FOOT_SPLINE_SPEED`).
const FOOT_SPLINE_SPEED: f32 = 1.0;
/// The fastest the foot moves along its spline, in baked points per second
/// (`FOOT_SPLINE_MAX_SPEED`).
const FOOT_SPLINE_MAX_SPEED: f32 = 500.0 * FOOT_SPLINE_SPEED;
/// `FOOT_SCALE`
const FOOT_SCALE: f32 = 10.0;
/// Seconds to rise (`FOOT_TIME_GOUP`).
const FOOT_TIME_GO_UP: f32 = 1.0;
/// Seconds to come down (`FOOT_TIME_GODOWN`).
const FOOT_TIME_GO_DOWN: f32 = 0.75;
/// Seconds it rests on the ground (`FOOT_TIME_DOWN`).
const FOOT_TIME_DOWN: f32 = 0.5;
/// What touching the foot does to the player (`Damage`).
const FOOT_DAMAGE: f32 = 0.3;
/// Below this speed along its spline, in points per second, the foot keeps
/// its facing.
const FOOT_TURN_MIN_SPEED: f32 = 5.0;
/// How much of the blend into the flat pose happens per second.
const FLAT_MORPH_RATE: f32 = 6.0;
/// The most random height a primed foot starts above the floor
/// (`MyRandomLong() & 0x3ff`); its first move puts it where its mode says.
const PRIME_HEIGHT_MASK: u32 = 0x3ff;
/// How tall the toe and middle boxes are, in units.
const SOLE_BOX_HEIGHT: f32 = 500.0;
/// How tall the heel box is: it covers the leg too.
const HEEL_BOX_HEIGHT: f32 = 3000.0;
/// `EPS` (original/src/Headers/globals.h).
const EPS: f32 = 1e-5;

/// The foot's animations (`FOOT_ANIM_*`).
mod anim {
    pub const FLAT: usize = 0;
    pub const UP: usize = 1;
    pub const DOWN: usize = 2;
}

/// The corners of the toe, middle and heel boxes, four each, before
/// scaling and turning (`pts` in `CalcFootCollisionBoxes`), as x and z.
const FOOT_BOX_CORNERS: [[Vec2; 4]; 3] = [
    // Toes.
    [
        Vec2::new(-20.0, -67.0),
        Vec2::new(26.0, -67.0),
        Vec2::new(-20.0, -24.0),
        Vec2::new(26.0, -24.0),
    ],
    // Middle.
    [
        Vec2::new(-20.0, -24.0),
        Vec2::new(26.0, -24.0),
        Vec2::new(-20.0, 6.0),
        Vec2::new(26.0, 6.0),
    ],
    // Heel.
    [
        Vec2::new(-15.0, 6.0),
        Vec2::new(18.0, 6.0),
        Vec2::new(-15.0, 49.0),
        Vec2::new(18.0, 49.0),
    ],
];

/// What the foot is doing (`FOOT_MODE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FootMode {
    GoUp,
    GoDown,
    Down,
}

impl FootMode {
    /// How long the mode lasts, in seconds.
    fn duration(self) -> f32 {
        match self {
            Self::GoUp => FOOT_TIME_GO_UP,
            Self::GoDown => FOOT_TIME_GO_DOWN,
            Self::Down => FOOT_TIME_DOWN,
        }
    }
}

/// A change of the foot's animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FootAnim {
    /// `SetSkeletonAnim`
    Set(usize),
    /// `MorphToSkeletonAnim`
    Morph(usize),
}

/// A foot's state (`Mode` and `FootTimer`).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct Foot {
    mode: FootMode,
    /// Seconds in the current mode.
    timer: f32,
}

impl Foot {
    /// The foot's speed along its spline at height `y` over a floor at
    /// `floor`: faster the higher it is, and still while it rests.
    fn spline_speed(&self, y: f32, floor: f32) -> f32 {
        if self.mode == FootMode::Down {
            0.0
        } else {
            ((y - floor) * FOOT_SPLINE_SPEED).min(FOOT_SPLINE_MAX_SPEED)
        }
    }

    /// Runs the mode for one tick whose timer has already advanced: returns
    /// the foot's new height and any change of animation.
    ///
    /// Port of the altitude part of `MoveFootOnSpline`.
    fn update_altitude(&mut self, y: f32, floor: f32) -> (f32, Option<FootAnim>) {
        match self.mode {
            FootMode::GoUp => {
                let t = (self.timer / FOOT_TIME_GO_UP).min(1.0);
                // Ease in and out.
                let t = -0.5 * ((PI * t).cos() - 1.0);
                let y = floor + t * FOOT_APEX;
                if t >= 1.0 - EPS {
                    self.set_mode(FootMode::GoDown);
                    return (y, Some(FootAnim::Set(anim::DOWN)));
                }
                (y, None)
            }
            FootMode::GoDown => {
                let t = (self.timer / FOOT_TIME_GO_DOWN).min(1.0);
                // Ease in.
                let t = t * t;
                let y = floor + (1.0 - t) * FOOT_APEX;
                if t >= 1.0 - EPS {
                    self.set_mode(FootMode::Down);
                    // Sound: EFFECT_FOOTSTEP at the foot.
                    return (y, Some(FootAnim::Morph(anim::FLAT)));
                }
                (y, None)
            }
            FootMode::Down => {
                if self.timer >= FOOT_TIME_DOWN {
                    self.set_mode(FootMode::GoUp);
                    return (y, Some(FootAnim::Set(anim::UP)));
                }
                (y, None)
            }
        }
    }

    fn set_mode(&mut self, mode: FootMode) {
        self.mode = mode;
        self.timer = 0.0;
    }
}

/// The toe, middle and heel boxes of a foot facing `yaw`, relative to its
/// position: each is the extent of its turned corners, standing on the
/// foot's base.
///
/// Port of `CalcFootCollisionBoxes` (original/src/Items/Traps.c).
fn foot_boxes(yaw: f32) -> Vec<CollisionBox> {
    let turn = Quat::from_rotation_y(yaw);
    FOOT_BOX_CORNERS
        .iter()
        .enumerate()
        .map(|(i, corners)| {
            let (min, max) = corners.iter().fold(
                (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
                |(min, max), corner| {
                    let p = turn * Vec3::new(corner.x, 0.0, corner.y) * FOOT_SCALE;
                    (min.min(p.xz()), max.max(p.xz()))
                },
            );
            let top = if i == 2 {
                HEEL_BOX_HEIGHT
            } else {
                SOLE_BOX_HEIGHT
            };
            CollisionBox::new(top, 0.0, min.x, max.x, max.y, min.y)
        })
        .collect()
}

/// Port of `PrimeFoot`. The foot starts in a random mode, part of the way
/// through it, so that the feet on a spline don't stomp together.
fn prime_foot(
    In(spawn): In<SplineItemSpawn>,
    mut commands: Commands,
    mut random: ResMut<GameRandom>,
    level_assets: Res<LevelAssets>,
    map: Res<TerrainMap>,
) -> bool {
    let Some(skeleton) = level_assets.skeleton(SkeletonType::Foot) else {
        error!("The level has no foot skeleton");
        return false;
    };
    let (x, z) = (spawn.position.x, spawn.position.y);
    let y = map.floor_height(x, z) + (random.next_u32() & PRIME_HEIGHT_MASK) as f32;
    let mode = match random.next_u32() % 3 {
        0 => FootMode::GoUp,
        1 => FootMode::GoDown,
        _ => FootMode::Down,
    };
    let timer = random.next_f32() * mode.duration() / 2.0;
    let position = Vec3::new(x, y, z);
    let root = commands
        .spawn((
            Name::new("Foot"),
            Transform::from_translation(position),
            TransformInterpolation,
            PreviousPosition(position),
            // Hidden and out of collision until the visibility check finds
            // it in the window, as the original detaches it.
            OnSpline::new(spawn.spline, spawn.placement, 0.0),
            Visibility::Hidden,
            ColliderDisabled,
            Foot { mode, timer },
            Damage(FOOT_DAMAGE),
            solid_object(
                foot_boxes(0.0),
                [
                    CollisionKind::Misc,
                    CollisionKind::HurtMe,
                    CollisionKind::BlockCamera,
                ],
                SolidSides::TOUCHABLE,
            ),
        ))
        .id();
    let model = commands
        .spawn((
            Name::new("Foot model"),
            Skeleton(skeleton),
            SkeletonAnimator::default(),
            Transform::from_scale(Vec3::splat(FOOT_SCALE)),
            ChildOf(root),
        ))
        .id();
    commands.entity(root).insert(TrapModel(model));
    true
}

/// Moves each foot along its spline and up and down.
///
/// Port of `MoveFootOnSpline` (original/src/Items/Traps.c). The facing and
/// boxes are only updated while the foot is in the window, as there.
fn move_feet(
    time: Res<Time>,
    map: Res<TerrainMap>,
    splines: Option<Res<Splines>>,
    mut feet: Query<(
        &mut Transform,
        &mut OnSpline,
        &mut Foot,
        &PreviousPosition,
        &TrapModel,
        &mut CollisionBoxes,
        &mut Collider,
    )>,
    mut models: Query<(&mut Transform, &mut SkeletonAnimator), Without<Foot>>,
) {
    let Some(splines) = splines else {
        return;
    };
    let dt = time.delta_secs();
    for (mut transform, mut on_spline, mut foot, previous, model, mut boxes, mut collider) in
        &mut feet
    {
        foot.timer += dt;
        // Where it is comes from before this tick's move along the spline.
        let position = on_spline.position(&splines);
        let floor = map.floor_height(position.x, position.y);
        let speed = foot.spline_speed(transform.translation.y, floor);
        on_spline.speed = speed;
        if foot.mode != FootMode::Down {
            on_spline.advance(&splines, dt);
        }
        let (y, anim) = foot.update_altitude(transform.translation.y, floor);
        transform.translation = Vec3::new(position.x, y, position.y);

        let Ok((mut model_transform, mut animator)) = models.get_mut(model.0) else {
            continue;
        };
        match anim {
            Some(FootAnim::Set(anim)) => animator.set_anim(anim),
            Some(FootAnim::Morph(anim)) => animator.morph_to(anim, FLAT_MORPH_RATE),
            None => {}
        }
        if !on_spline.visible {
            continue;
        }
        if speed > FOOT_TURN_MIN_SPEED {
            let yaw =
                yaw_from_point_to_point(yaw_of(model_transform.rotation), previous.xz(), position);
            model_transform.rotation = Quat::from_rotation_y(yaw);
        }
        boxes.0 = foot_boxes(yaw_of(model_transform.rotation));
        *collider = boxes.collider();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foot(mode: FootMode) -> Foot {
        Foot { mode, timer: 0.0 }
    }

    /// Runs a foot on flat ground at 60 ticks per second for `seconds`,
    /// returning its heights and animation changes.
    fn run(foot: &mut Foot, seconds: f32) -> (Vec<f32>, Vec<FootAnim>) {
        let dt = 1.0 / 60.0;
        let mut y = 0.0;
        let mut heights = Vec::new();
        let mut anims = Vec::new();
        for _ in 0..(seconds / dt).round() as usize {
            foot.timer += dt;
            let (new_y, anim) = foot.update_altitude(y, 0.0);
            y = new_y;
            heights.push(y);
            anims.extend(anim);
        }
        (heights, anims)
    }

    #[test]
    fn a_foot_rises_stomps_rests_and_rises_again() {
        let mut f = foot(FootMode::GoUp);
        let (heights, anims) = run(&mut f, FOOT_TIME_GO_UP + 0.01);
        assert!((heights.last().copied().unwrap_or(0.0) - FOOT_APEX).abs() < 1.0);
        assert_eq!(anims, [FootAnim::Set(anim::DOWN)]);
        assert_eq!(f.mode, FootMode::GoDown);

        let (heights, anims) = run(&mut f, FOOT_TIME_GO_DOWN + 0.01);
        assert!(heights.last().copied().unwrap_or(1.0).abs() < 1.0);
        assert_eq!(anims, [FootAnim::Morph(anim::FLAT)]);
        assert_eq!(f.mode, FootMode::Down);

        let (_, anims) = run(&mut f, FOOT_TIME_DOWN + 0.01);
        assert_eq!(anims, [FootAnim::Set(anim::UP)]);
        assert_eq!(f.mode, FootMode::GoUp);
    }

    #[test]
    fn a_foot_moves_faster_the_higher_it_is_and_not_while_resting() {
        let up = foot(FootMode::GoUp);
        assert_eq!(up.spline_speed(100.0, 0.0), 100.0);
        assert_eq!(up.spline_speed(900.0, 0.0), FOOT_SPLINE_MAX_SPEED);
        assert_eq!(foot(FootMode::Down).spline_speed(900.0, 0.0), 0.0);
    }

    #[test]
    fn the_foot_boxes_turn_with_the_foot() {
        let boxes = foot_boxes(0.0);
        // The toes point to −Z, the heel box covers the leg.
        assert_eq!(boxes[0].back, -67.0 * FOOT_SCALE);
        assert_eq!(boxes[2].front, 49.0 * FOOT_SCALE);
        assert_eq!(boxes[2].top, HEEL_BOX_HEIGHT);
        // A half turn takes the toes to +Z.
        let turned = foot_boxes(PI);
        assert!((turned[0].front - 67.0 * FOOT_SCALE).abs() < 1e-2);
        assert!((turned[0].left + 26.0 * FOOT_SCALE).abs() < 1e-2);
    }
}
