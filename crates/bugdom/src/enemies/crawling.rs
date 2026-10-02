//! What the slug and the caterpillar share: a body laid joint by joint
//! along their spline, undulating as it goes.
//!
//! Port of `SetCrawlingEnemyJointTransforms`
//! (original/src/Enemies/Enemy_Slug.c), which both kinds call. Their
//! skeletons' joints are global (`JointsAreGlobal`): instead of being
//! animated, each joint is put straight into world space on the spline,
//! some points behind the one before it, and has a collision box of its
//! own. The joint matrices keep the original's shape, which is not a
//! rotation (the body stays upright on slopes), so they are written as the
//! joints' [`GlobalTransform`]s after transform propagation.

use avian3d::prelude::Collider;
use bevy::camera::visibility::{NoFrustumCulling, VisibilitySystems};
use bevy::ecs::query::QueryData;
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use super::EnemyModel;
use crate::collision::{CollisionBox, CollisionBoxes};
use crate::skeleton::{SkeletonAnimator, SkeletonRig};
use crate::splines::{OnSpline, Spline, Splines};
use crate::terrain::TerrainMap;

/// The systems both crawling kinds need once. Added by the kinds' plugins
/// through [`add_crawling`].
struct CrawlingPlugin;

impl Plugin for CrawlingPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(draw_whole_body).add_systems(
            PostUpdate,
            pose_crawling_joints
                .after(TransformSystems::Propagate)
                .before(VisibilitySystems::CheckVisibility),
        );
    }
}

/// Adds the shared crawling systems, unless another kind did already.
pub(super) fn add_crawling(app: &mut App) {
    if !app.is_plugin_added::<CrawlingPlugin>() {
        app.add_plugins(CrawlingPlugin);
    }
}

/// How the undulation's phase moves on, in radians per second, for each
/// joint laid (`gFramesPerSecondFrac * .8f`, inside the joint loop).
const UNDULATE_RATE: f32 = 0.8;
/// The undulation's phase difference between neighbouring joints, in
/// radians.
const UNDULATE_JOINT_PHASE: f32 = 1.6;
/// How much each joint's scale differs from the one before it, at most.
const UNDULATE_SCALE: f32 = 0.3;

/// The shape of a crawling kind: `SetCrawlingEnemyJointTransforms`'
/// constant arguments.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrawlingShape {
    /// How far apart the joints are on the spline, in baked spline points
    /// (`splineIndexDelta`, the kind's `STRETCH`).
    pub joint_spacing: f32,
    /// How far below the floor the joints are, in units (`footOffset`).
    pub foot_offset: f32,
    /// Half the size of each joint's collision box, in units.
    pub box_size: f32,
    /// The head joint's scale (`undulateScale`, the kind's `SCALE`).
    pub scale: f32,
}

/// A crawling enemy on its spline: its shape, the undulation's phase
/// (`SpecialF[0]`) and its joints' world transforms as last laid out.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Crawler {
    pub shape: CrawlingShape,
    /// In radians.
    pub undulate_phase: f32,
    joints: Vec<Affine3A>,
}

impl Crawler {
    pub fn new(shape: CrawlingShape) -> Self {
        Self {
            shape,
            undulate_phase: 0.0,
            joints: Vec::new(),
        }
    }
}

/// The joints of a crawling body, from head to tail.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrawlingJoints {
    /// Each joint's transform to world space (`jointTransformMatrix`).
    pub transforms: Vec<Affine3A>,
    /// Each joint's collision box, in world space.
    pub boxes: Vec<CollisionBox>,
}

/// Lays `num_joints` joints along `spline` backward from `placement`, on
/// the floor `floor_height(x, z)` gives, and moves the undulation on by
/// `dt` seconds.
///
/// Port of `SetCrawlingEnemyJointTransforms`
/// (original/src/Enemies/Enemy_Slug.c). Each joint faces the next one
/// toward the tail with its up axis kept straight up, as `SetLookAtMatrix`
/// builds it: on a slope the result is sheared rather than rotated. The
/// phase moves on once per joint, so a longer body undulates faster, as in
/// the original.
pub fn lay_crawling_joints(
    spline: &Spline,
    placement: f32,
    num_joints: usize,
    shape: &CrawlingShape,
    undulate_phase: &mut f32,
    dt: f32,
    floor_height: impl Fn(f32, f32) -> f32,
) -> CrawlingJoints {
    let num_points = spline.points.len();
    if num_points == 0 {
        return CrawlingJoints::default();
    }
    let placement_delta = shape.joint_spacing / num_points as f32;
    let at = |placement: f32| {
        let xz = spline.coord_at(placement).0;
        Vec3::new(xz.x, floor_height(xz.x, xz.y) - shape.foot_offset, xz.y)
    };

    let mut joints = CrawlingJoints {
        transforms: Vec::with_capacity(num_joints),
        boxes: Vec::with_capacity(num_joints),
    };
    let mut walk = placement;
    let mut next = at(walk);
    let mut scale = shape.scale;
    let size = shape.box_size;
    for joint in 0..num_joints {
        let this = next;
        walk -= placement_delta;
        if walk < 0.0 {
            walk += 1.0;
        }
        next = at(walk);

        joints.boxes.push(CollisionBox::new(
            this.y + size,
            this.y - size,
            this.x - size,
            this.x + size,
            this.z + size,
            this.z - size,
        ));

        // `SetLookAtMatrix(up, next, this)`: z toward the tail, y up and x
        // their cross product, then scaled and moved to the joint.
        let look = (next - this).normalize_or_zero();
        let x_axis = Vec3::new(look.z, 0.0, -look.x);
        joints.transforms.push(Affine3A::from_cols(
            (x_axis * scale).into(),
            (Vec3::Y * scale).into(),
            (look * scale).into(),
            this.into(),
        ));

        *undulate_phase -= UNDULATE_RATE * dt;
        scale += (*undulate_phase + joint as f32 * UNDULATE_JOINT_PHASE).sin() * UNDULATE_SCALE;
    }
    joints
}

/// The parts of a crawling enemy that its spline move uses.
#[derive(QueryData)]
#[query_data(mutable)]
pub struct CrawlerBody {
    pub on_spline: &'static mut OnSpline,
    pub transform: &'static mut Transform,
    pub crawler: &'static mut Crawler,
    pub boxes: &'static mut CollisionBoxes,
    pub collider: &'static mut Collider,
    pub model: &'static EnemyModel,
}

/// What the crawling kinds' moves read.
pub struct CrawlingWorld<'a> {
    pub splines: &'a Splines,
    pub map: &'a TerrainMap,
    pub dt: f32,
}

/// Moves a crawling enemy along its spline and, if it is in the window,
/// lays its body out behind it.
///
/// Port of the body of `MoveSlugOnSpline` (original/src/Enemies/Enemy_Slug.c)
/// and `MoveCaterpillerOnSpline` (original/src/Enemies/Enemy_Caterpiller.c),
/// which differ only in their constants. Only x and z follow the spline:
/// the origin keeps the height it was primed at, as in the original. The
/// joints' boxes are kept relative to the origin, as [`CollisionBoxes`]
/// are.
pub(super) fn crawl_on_spline(
    body: &mut CrawlerBodyItem,
    world: &CrawlingWorld,
    rig: Option<&SkeletonRig>,
) {
    let placement = body.on_spline.advance(world.splines, world.dt);
    let position = body.on_spline.position(world.splines);
    body.transform.translation.x = position.x;
    body.transform.translation.z = position.y;

    if !body.on_spline.visible {
        return;
    }
    let (Some(rig), Some(spline)) = (rig, world.splines.get(body.on_spline.spline)) else {
        return;
    };
    let crawler = &mut *body.crawler;
    let joints = lay_crawling_joints(
        spline,
        placement,
        rig.definition.bones.len(),
        &crawler.shape,
        &mut crawler.undulate_phase,
        world.dt,
        |x, z| world.map.floor_height(x, z),
    );
    crawler.joints = joints.transforms;
    let origin = body.transform.translation;
    body.boxes.0 = joints.boxes.iter().map(|b| b.at(-origin)).collect();
    *body.collider = body.boxes.collider();
}

/// Makes a newly spawned crawling enemy's joints global: its model is no
/// longer animated, since the original skips the animation of global
/// joints (`UpdateSkeletonAnimation`, original/src/Skeleton/SkeletonAnim.c)
/// and [`pose_crawling_joints`] places them instead.
pub(super) fn make_joints_global(enemy: &mut EntityCommands) {
    enemy.queue(|mut enemy: EntityWorldMut| {
        let Some(model) = enemy.get::<EnemyModel>().map(|m| m.0) else {
            return;
        };
        enemy.world_scope(|world| {
            if let Ok(mut model) = world.get_entity_mut(model) {
                model.remove::<SkeletonAnimator>();
            }
        });
    });
}

/// Puts each crawling enemy's joints where its last move laid them,
/// over what transform propagation computed from the rig.
fn pose_crawling_joints(
    crawlers: Query<(&Crawler, &EnemyModel)>,
    rigs: Query<&SkeletonRig>,
    mut globals: Query<&mut GlobalTransform>,
) {
    for (crawler, model) in &crawlers {
        let Ok(rig) = rigs.get(model.0) else {
            continue;
        };
        for (&joint, &transform) in rig.joints.iter().zip(&crawler.joints) {
            if let Ok(mut global) = globals.get_mut(joint) {
                *global = GlobalTransform::from(transform);
            }
        }
    }
}

/// Keeps a crawling body drawn while any of it is on screen. Its meshes'
/// bounds are around the head, but the body trails far behind it.
///
/// Stands in for the source port's fix in the Prime routines
/// (`BoundingSphere.radius *= 1.5f`, "otherwise tail gets culled early").
fn draw_whole_body(
    add: On<Add, SkeletonRig>,
    mut commands: Commands,
    parents: Query<&ChildOf>,
    crawlers: Query<(), With<Crawler>>,
    children: Query<&Children>,
    meshes: Query<(), With<Mesh3d>>,
) {
    let Ok(parent) = parents.get(add.entity) else {
        return;
    };
    if !crawlers.contains(parent.parent()) {
        return;
    }
    for child in children.iter_descendants(add.entity) {
        if meshes.contains(child) {
            commands.entity(child).insert(NoFrustumCulling);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: CrawlingShape = CrawlingShape {
        joint_spacing: 10.0,
        foot_offset: 5.0,
        box_size: 40.0,
        scale: 3.0,
    };

    /// A straight spline of 100 points along +x, one unit apart.
    fn line() -> Spline {
        Spline {
            points: (0..100).map(|i| Vec2::new(i as f32, 0.0)).collect(),
            nubs: Vec::new(),
        }
    }

    #[test]
    fn joints_trail_back_along_the_spline() {
        let mut phase = 0.0;
        let joints = lay_crawling_joints(&line(), 0.5, 3, &SHAPE, &mut phase, 0.0, |_, _| 100.0);
        assert_eq!(joints.transforms.len(), 3);
        // The head is at the placement, each joint ten points further back,
        // on the floor less the foot offset.
        for (i, (transform, b)) in joints.transforms.iter().zip(&joints.boxes).enumerate() {
            let at = Vec3::new(50.0 - 10.0 * i as f32, 95.0, 0.0);
            assert!(Vec3::from(transform.translation).distance(at) < 1e-3, "{i}");
            let (min, max) = b.min_max();
            assert!((min - (at - Vec3::splat(40.0))).length() < 1e-3);
            assert!((max - (at + Vec3::splat(40.0))).length() < 1e-3);
        }
        // Each faces its tail (-x), upright, at the kind's scale while the
        // phase stands still at zero for the head.
        let head = joints.transforms[0];
        assert!((Vec3::from(head.matrix3.z_axis) - Vec3::new(-3.0, 0.0, 0.0)).length() < 1e-3);
        assert!((Vec3::from(head.matrix3.y_axis) - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-3);
        assert!((Vec3::from(head.matrix3.x_axis) - Vec3::new(0.0, 0.0, 3.0)).length() < 1e-3);
        // The next joint's scale has moved on by sin(0 + 0 * 1.6) * 0.3,
        // and the one after by sin(1.6) * 0.3.
        let scale_of = |t: &Affine3A| Vec3::from(t.matrix3.y_axis).y;
        assert!((scale_of(&joints.transforms[1]) - 3.0).abs() < 1e-5);
        let third = 3.0 + 1.6f32.sin() * UNDULATE_SCALE;
        assert!((scale_of(&joints.transforms[2]) - third).abs() < 1e-5);
    }

    #[test]
    fn the_body_wraps_round_the_start_of_the_spline() {
        let mut phase = 0.0;
        let joints = lay_crawling_joints(&line(), 0.05, 2, &SHAPE, &mut phase, 0.0, |_, _| 0.0);
        // Ten points behind point 5 is point 95.
        assert!((joints.transforms[1].translation.x - 95.0).abs() < 1e-3);
    }

    #[test]
    fn the_undulation_moves_on_per_joint_and_per_second() {
        let mut phase = 1.0;
        lay_crawling_joints(&line(), 0.5, 4, &SHAPE, &mut phase, 0.5, |_, _| 0.0);
        assert!((phase - (1.0 - 4.0 * UNDULATE_RATE * 0.5)).abs() < 1e-5);
    }

    #[test]
    fn slopes_shear_the_body_upright() {
        let mut phase = 0.0;
        // The floor rises toward the tail: the joints stay upright and
        // look up the slope.
        let joints = lay_crawling_joints(&line(), 0.5, 2, &SHAPE, &mut phase, 0.0, |x, _| -x);
        let head = joints.transforms[0];
        let look = Vec3::from(head.matrix3.z_axis) / SHAPE.scale;
        assert!((look - Vec3::new(-1.0, 1.0, 0.0).normalize()).length() < 1e-3);
        assert!((Vec3::from(head.matrix3.y_axis) - Vec3::Y * SHAPE.scale).length() < 1e-3);
    }

    #[test]
    fn an_empty_spline_lays_nothing() {
        let mut phase = 0.0;
        let joints = lay_crawling_joints(
            &Spline::default(),
            0.5,
            4,
            &SHAPE,
            &mut phase,
            0.1,
            |_, _| 0.0,
        );
        assert_eq!(joints, CrawlingJoints::default());
    }
}
