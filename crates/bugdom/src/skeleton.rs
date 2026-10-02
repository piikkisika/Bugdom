//! Skeletal models: spawning a skinned rig from a [`SkeletonAsset`] and
//! playing its keyframe animations.
//!
//! Animation playback is a port of the original's own player rather than
//! Bevy's animation graph, because gameplay depends on its exact behaviour:
//! markers, loops and zig-zags, morphing between animations, and events that
//! set flags and play sounds.

use std::sync::Arc;

use bevy::math::Affine3A;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bugdom_formats::skeleton::{
    AccelerationMode, AnimEventKind, Keyframe, SkeletonDefinition, TICKS_PER_SECOND,
};

use crate::assets::skeleton::SkeletonAsset;

pub struct SkeletonPlugin;

impl Plugin for SkeletonPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<AnimationSound>()
            .add_systems(Update, spawn_rigs)
            .add_systems(
                FixedUpdate,
                advance_animations.in_set(SkeletonSystems::Advance),
            )
            .add_systems(PostUpdate, pose_joints.before(TransformSystems::Propagate));
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SkeletonSystems {
    /// Advances animations, in `FixedUpdate`. As in the original's
    /// `MoveObjects`, an object's animation advances before it moves.
    Advance,
}

/// The game's skeletal characters (`SKELETON_TYPE_*` in
/// original/src/Headers/skeletonobj.h), each with its own skeleton file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkeletonType {
    BoxerFly,
    /// The player's bug.
    Me,
    Slug,
    Ant,
    FireAnt,
    WaterBug,
    DragonFly,
    PondFish,
    Mosquito,
    Foot,
    Spider,
    Caterpiller,
    FireFly,
    Bat,
    LadyBug,
    RootSwing,
    KingAnt,
    Larva,
    FlyingBee,
    WorkerBee,
    QueenBee,
    Roach,
    Buddy,
    Skippy,
}

impl SkeletonType {
    /// Every type, in the original's order.
    pub const ALL: [Self; 24] = [
        Self::BoxerFly,
        Self::Me,
        Self::Slug,
        Self::Ant,
        Self::FireAnt,
        Self::WaterBug,
        Self::DragonFly,
        Self::PondFish,
        Self::Mosquito,
        Self::Foot,
        Self::Spider,
        Self::Caterpiller,
        Self::FireFly,
        Self::Bat,
        Self::LadyBug,
        Self::RootSwing,
        Self::KingAnt,
        Self::Larva,
        Self::FlyingBee,
        Self::WorkerBee,
        Self::QueenBee,
        Self::Roach,
        Self::Buddy,
        Self::Skippy,
    ];

    /// The file's name without its extension, from `LoadSkeletonFile`
    /// (original/src/System/File.c).
    pub fn file_name(self) -> &'static str {
        match self {
            Self::BoxerFly => "BoxerFly",
            Self::Me => "DoodleBug",
            Self::Slug => "Slug",
            Self::Ant => "Ant",
            Self::FireAnt => "WingedFireAnt",
            Self::WaterBug => "WaterBug",
            Self::DragonFly => "DragonFly",
            Self::PondFish => "PondFish",
            Self::Mosquito => "Mosquito",
            Self::Foot => "Foot",
            Self::Spider => "Spider",
            Self::Caterpiller => "Caterpillar",
            Self::FireFly => "FireFly",
            Self::Bat => "Bat",
            Self::LadyBug => "LadyBug",
            Self::RootSwing => "RootSwing",
            Self::KingAnt => "AntKing",
            Self::Larva => "Larva",
            Self::FlyingBee => "FlyingBee",
            Self::WorkerBee => "WorkerBee",
            Self::QueenBee => "QueenBee",
            Self::Roach => "Roach",
            Self::Buddy => "Buddy",
            Self::Skippy => "Skippy",
        }
    }

    /// The skeleton file, relative to the data directory. Its geometry is
    /// the `.3dmf` of the same name next to it.
    pub fn path(self) -> String {
        format!("Skeletons/{}.skeleton.rsrc", self.file_name())
    }
}

/// A skeletal model. Its rig (joint entities and skinned meshes) is spawned
/// as children once the asset has loaded.
#[derive(Component, Debug, Clone)]
#[require(Transform, Visibility, SkeletonAnimator, AnimationFlags)]
pub struct Skeleton(pub Handle<SkeletonAsset>);

/// The spawned rig of a [`Skeleton`].
#[derive(Component, Debug, Clone)]
pub struct SkeletonRig {
    pub definition: Arc<SkeletonDefinition>,
    /// Joint entities, indexed like the definition's bones.
    pub joints: Vec<Entity>,
    /// Each joint's transform relative to its parent as last posed: a copy
    /// of the joint entities' transforms (`jointTransformMatrix`), so that
    /// gameplay can find joints without querying them.
    pose: Vec<Transform>,
}

impl SkeletonRig {
    fn new(definition: Arc<SkeletonDefinition>, joints: Vec<Entity>) -> Self {
        let pose = definition
            .bones
            .iter()
            .map(|bone| Transform::from_translation(bind_offset(&definition, bone)))
            .collect();
        Self {
            definition,
            joints,
            pose,
        }
    }

    /// The transform from a joint's space to the world, given `base`, the
    /// rig entity's own transform to the world (for the player, its
    /// transform times its model's). `None` if there is no such joint.
    ///
    /// Port of `FindJointFullMatrix` (original/src/Skeleton/SkeletonJoints.c).
    /// As there, the joints are where the last frame drew them and `base`
    /// is where the object is now.
    pub fn joint_transform(&self, joint: usize, base: Affine3A) -> Option<Affine3A> {
        let mut matrix = self.pose.get(joint)?.compute_affine();
        let mut bone = self.definition.bones.get(joint)?;
        // Bounded, so that a parent loop in a bad file can't hang the game.
        for _ in 0..self.definition.bones.len() {
            let Some(parent) = bone.parent else {
                return Some(base * matrix);
            };
            matrix = self.pose.get(parent)?.compute_affine() * matrix;
            bone = self.definition.bones.get(parent)?;
        }
        None
    }
}

/// The world position of `offset`, a point in a joint's space, given the
/// rig entity's transform to the world as `base` (see
/// [`SkeletonRig::joint_transform`]). `None` if there is no such joint.
///
/// Port of `FindCoordOnJoint` (original/src/Skeleton/SkeletonJoints.c).
pub fn joint_position(
    rig: &SkeletonRig,
    joint: usize,
    offset: Vec3,
    base: Affine3A,
) -> Option<Vec3> {
    rig.joint_transform(joint, base)
        .map(|matrix| matrix.transform_point3(offset))
}

/// A bone's bind-pose position relative to its parent. Bone coordinates in
/// the files are absolute.
fn bind_offset(definition: &SkeletonDefinition, bone: &bugdom_formats::skeleton::Bone) -> Vec3 {
    let parent_coord = bone
        .parent
        .and_then(|p| definition.bones.get(p))
        .map_or(Vec3::ZERO, |p| Vec3::from(p.coord));
    Vec3::from(bone.coord) - parent_coord
}

/// Flags that animation events set and clear, which gameplay code reads to
/// sync with an animation (e.g. the frame an attack lands). Port of the
/// `ObjNode::Flag` array as used by `ANIMEVENT_TYPE_SETFLAG`/`CLEARFLAG`.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnimationFlags(pub [bool; ANIMATION_FLAG_COUNT]);

/// `MAX_FLAGS_IN_OBJNODE` in original/src/Headers/structs.h.
pub const ANIMATION_FLAG_COUNT: usize = 4;

/// Sent when an animation reaches a sound event (`ANIMEVENT_TYPE_PLAYSOUND`).
/// The audio code decides what to play: 0 is the kick, 1 the water bug.
#[derive(Message, Debug, Clone, Copy)]
pub struct AnimationSound {
    pub entity: Entity,
    pub sound: u8,
}

/// Playback state of a skeleton's animation (`SkeletonObjDataType` in
/// original/src/Headers/structs.h).
#[derive(Component, Debug, Clone)]
pub struct SkeletonAnimator {
    pub anim: usize,
    /// Current time in animation ticks.
    pub time: f32,
    /// Playback speed multiplier (`AnimSpeed`).
    pub speed: f32,
    /// Set when the animation reaches a stop event, or the start of a
    /// zig-zag that does not repeat.
    pub has_stopped: bool,
    backward: bool,
    zigzag: bool,
    loop_back_time: f32,
    event_index: usize,
    morph: Option<Morph>,
    /// The pose computed last, per joint.
    pose: Vec<JointPose>,
}

impl Default for SkeletonAnimator {
    fn default() -> Self {
        Self {
            anim: 0,
            time: 0.0,
            speed: 1.0,
            has_stopped: false,
            backward: false,
            zigzag: false,
            loop_back_time: 0.0,
            event_index: 0,
            morph: None,
            pose: Vec::new(),
        }
    }
}

impl SkeletonAnimator {
    /// Starts `anim` from the beginning. Port of `SetSkeletonAnim`
    /// (original/src/Skeleton/SkeletonAnim.c).
    pub fn set_anim(&mut self, anim: usize) {
        *self = Self {
            anim,
            pose: std::mem::take(&mut self.pose),
            ..default()
        };
    }

    /// Blends from the current pose to the first frame of `anim`, then plays
    /// it. `rate` is how much of the blend happens per second.
    /// Port of `MorphToSkeletonAnim`.
    pub fn morph_to(&mut self, anim: usize, rate: f32) {
        let start = self.pose.clone();
        self.set_anim(anim);
        self.morph = Some(Morph {
            percent: 0.0,
            rate,
            start,
        });
    }

    pub fn is_morphing(&self) -> bool {
        self.morph.is_some()
    }
}

#[derive(Debug, Clone)]
struct Morph {
    percent: f32,
    rate: f32,
    start: Vec<JointPose>,
}

/// A joint's transform relative to its parent, as interpolated between
/// keyframes. Rotations are Euler angles and are interpolated as such, like
/// the original.
#[derive(Debug, Clone, Copy, PartialEq)]
struct JointPose {
    coord: Vec3,
    rotation: Vec3,
    scale: Vec3,
}

impl From<&Keyframe> for JointPose {
    fn from(kf: &Keyframe) -> Self {
        Self {
            coord: kf.coord.into(),
            rotation: kf.rotation.into(),
            scale: kf.scale.into(),
        }
    }
}

impl JointPose {
    /// Port of `UpdateJointTransforms` (original/src/Skeleton/SkeletonJoints.c).
    /// The original applies rotation, then scale, then translation; Bevy
    /// applies scale first, which is the same because every scaled keyframe
    /// in the data is uniform.
    fn transform(&self) -> Transform {
        Transform {
            translation: self.coord,
            rotation: euler_xyz(self.rotation),
            scale: self.scale,
        }
    }
}

/// The rotation of `Q3Matrix4x4_SetRotate_XYZ`: about X, then Y, then Z.
fn euler_xyz(r: Vec3) -> Quat {
    Quat::from_euler(EulerRot::ZYX, r.z, r.y, r.x)
}

/// Spawns joint entities and skinned meshes for newly loaded skeletons.
fn spawn_rigs(
    mut commands: Commands,
    skeletons: Query<(Entity, &Skeleton), Without<SkeletonRig>>,
    assets: Res<Assets<SkeletonAsset>>,
) {
    for (entity, skeleton) in &skeletons {
        let Some(asset) = assets.get(&skeleton.0) else {
            continue;
        };
        let joints = spawn_joints(&mut commands, &asset.definition, entity);
        for part in &asset.parts {
            commands.spawn((
                Mesh3d(part.mesh.clone()),
                MeshMaterial3d(part.material.clone()),
                SkinnedMesh {
                    inverse_bindposes: asset.inverse_bindposes.clone(),
                    joints: joints.clone(),
                },
                ChildOf(entity),
            ));
        }

        commands
            .entity(entity)
            .insert(SkeletonRig::new(asset.definition.clone(), joints));
    }
}

/// Spawns a skeleton's joint entities under `rig`, in the bind pose.
fn spawn_joints(
    commands: &mut Commands,
    definition: &SkeletonDefinition,
    rig: Entity,
) -> Vec<Entity> {
    // Bones are spawned in index order and parented afterwards, because the
    // files do not guarantee that a parent comes before its children.
    let joints: Vec<Entity> = definition
        .bones
        .iter()
        .map(|bone| {
            commands
                .spawn((
                    Name::new(bone.name.clone()),
                    Transform::from_translation(bind_offset(definition, bone)),
                ))
                .id()
        })
        .collect();
    for (bone, &joint) in definition.bones.iter().zip(&joints) {
        let parent = bone.parent.and_then(|p| joints.get(p)).copied();
        commands
            .entity(joint)
            .insert(ChildOf(parent.unwrap_or(rig)));
    }
    joints
}

/// Advances animation time and handles animation events.
/// Port of `UpdateSkeletonAnimation` (original/src/Skeleton/SkeletonAnim.c),
/// with time in seconds instead of scaled by `gFramesPerSecondFrac`.
fn advance_animations(
    time: Res<Time>,
    mut animators: Query<(
        Entity,
        &mut SkeletonAnimator,
        &mut AnimationFlags,
        &SkeletonRig,
    )>,
    mut sounds: MessageWriter<AnimationSound>,
) {
    let dt = time.delta_secs();
    for (entity, mut animator, mut flags, rig) in &mut animators {
        let Some(anim) = rig.definition.animations.get(animator.anim) else {
            continue;
        };
        let animator = &mut *animator;

        if let Some(morph) = &mut animator.morph {
            morph.percent += morph.rate * dt;
            if morph.percent >= 1.0 {
                animator.morph = None;
            }
            continue;
        }

        let step = TICKS_PER_SECOND * dt * animator.speed;
        let mut t = animator.time;
        if !animator.backward {
            t += step;
        } else {
            t -= step;
            let lb = animator.loop_back_time;
            if t < lb {
                t = lb + (lb - t);
                if animator.zigzag {
                    animator.backward = false;
                    animator.event_index = if lb == 0.0 { 0 } else { next_event_at(anim, t) };
                } else {
                    animator.has_stopped = true;
                }
            }
        }

        // A loop that lands on its own event would spin forever, so stop
        // after the second loop-type event in one update, as the original does.
        let mut loop_count = 0;
        while let Some(event) = anim.events.get(animator.event_index) {
            let event_time = f32::from(event.time);
            if t < event_time {
                break;
            }
            match event.kind {
                AnimEventKind::Stop => {
                    animator.has_stopped = true;
                    animator.event_index += 1;
                }
                AnimEventKind::SetMarker => {
                    animator.event_index += 1;
                    animator.loop_back_time = event_time;
                }
                AnimEventKind::Loop => {
                    loop_count += 1;
                    let lb = animator.loop_back_time;
                    if lb != 0.0 {
                        t = t - event_time + lb;
                        animator.event_index = next_event_at(anim, t);
                    } else if t != 0.0 {
                        t -= event_time;
                        animator.event_index = 0;
                    } else {
                        // A loop of length zero is a stop.
                        animator.has_stopped = true;
                        animator.event_index += 1;
                    }
                }
                AnimEventKind::ZigZag => {
                    loop_count += 1;
                    animator.backward = true;
                    t -= event_time - t;
                    animator.zigzag = true;
                    animator.event_index += 1;
                }
                AnimEventKind::SetFlag { flag } | AnimEventKind::ClearFlag { flag } => {
                    let set = matches!(event.kind, AnimEventKind::SetFlag { .. });
                    if let Some(f) = flags.0.get_mut(usize::from(flag)) {
                        *f = set;
                    } else {
                        warn_once!("animation flag {flag} is out of range");
                    }
                    animator.event_index += 1;
                }
                AnimEventKind::PlaySound { sound } => {
                    sounds.write(AnimationSound { entity, sound });
                    animator.event_index += 1;
                }
                AnimEventKind::GotoMarker { .. } => animator.event_index += 1,
            }
            if loop_count > 1 {
                break;
            }
        }

        animator.time = t;
    }
}

/// Index of the first event at or after `time`, or 0 if there is none.
/// Port of `GetNextAnimEventAtTime`.
fn next_event_at(anim: &bugdom_formats::skeleton::Animation, time: f32) -> usize {
    anim.events
        .iter()
        .position(|e| f32::from(e.time) >= time)
        .unwrap_or(0)
}

/// Computes each joint's pose for the current time and writes it to the
/// joint's transform. Port of `GetModelCurrentPosition` and
/// `GetModelMorphPosition` (original/src/Skeleton/SkeletonAnim.c).
fn pose_joints(
    mut animators: Query<(&mut SkeletonAnimator, &mut SkeletonRig)>,
    mut transforms: Query<&mut Transform>,
) {
    for (mut animator, mut rig) in &mut animators {
        let rig = &mut *rig;
        let Some(anim) = rig.definition.animations.get(animator.anim) else {
            continue;
        };
        let animator = &mut *animator;
        animator
            .pose
            .resize(rig.joints.len(), JointPose::from(&IDENTITY_KEYFRAME));

        for (joint, keyframes) in anim.keyframes.iter().enumerate() {
            let pose = if let Some(morph) = &animator.morph {
                let start = morph
                    .start
                    .get(joint)
                    .copied()
                    .unwrap_or(animator.pose[joint]);
                let end = keyframes.first().map_or(start, JointPose::from);
                lerp_pose(&start, &end, morph.percent)
            } else {
                // The original stops at the first joint without keyframes,
                // leaving the remaining joints in their previous pose.
                let Some(pose) = sample(keyframes, animator.time) else {
                    break;
                };
                pose
            };
            animator.pose[joint] = pose;
            if let Some(posed) = rig.pose.get_mut(joint) {
                *posed = pose.transform();
            }
            if let Some(mut transform) = rig
                .joints
                .get(joint)
                .and_then(|&e| transforms.get_mut(e).ok())
            {
                *transform = pose.transform();
            }
        }
    }
}

const IDENTITY_KEYFRAME: Keyframe = Keyframe {
    tick: 0,
    acceleration: AccelerationMode::Linear,
    coord: [0.0; 3],
    rotation: [0.0; 3],
    scale: [1.0; 3],
};

/// The pose at `time` from one joint's keyframes, or `None` if it has none.
fn sample(keyframes: &[Keyframe], time: f32) -> Option<JointPose> {
    let next = keyframes.iter().position(|kf| kf.tick as f32 >= time);
    Some(match next {
        Some(0) => JointPose::from(&keyframes[0]),
        Some(i) if keyframes[i].tick as f32 == time => JointPose::from(&keyframes[i]),
        Some(i) => interpolate(&keyframes[i - 1], &keyframes[i], time),
        None => JointPose::from(keyframes.last()?),
    })
}

/// Port of `InterpolateKeyFrames`.
fn interpolate(kf1: &Keyframe, kf2: &Keyframe, time: f32) -> JointPose {
    let t1 = kf1.tick as f32;
    let t2 = kf2.tick as f32;
    let mut k2 = (time - t1) / (t2 - t1);
    match kf1.acceleration {
        AccelerationMode::Linear => {}
        AccelerationMode::EaseInOut => k2 = 1.0 - acceleration_curve(1.0 - k2),
        AccelerationMode::EaseIn => {
            let k1 = (acceleration_curve(0.5 * (1.0 - k2)) * 2.0).clamp(0.0, 1.0);
            k2 = 1.0 - k1;
        }
        AccelerationMode::EaseOut => {
            k2 = (acceleration_curve(0.5 * k2) * 2.0).clamp(0.0, 1.0);
        }
    }
    let mut pose = lerp_pose(&JointPose::from(kf1), &JointPose::from(kf2), k2);
    if kf1.scale[0] == 1.0 && kf2.scale[0] == 1.0 {
        pose.scale = Vec3::ONE;
    }
    pose
}

/// Linear blend of coordinates and Euler angles. Scale blends the Z
/// component and applies it to all axes, as the original does.
fn lerp_pose(a: &JointPose, b: &JointPose, k: f32) -> JointPose {
    JointPose {
        coord: a.coord.lerp(b.coord, k),
        rotation: a.rotation.lerp(b.rotation, k),
        scale: Vec3::splat(a.scale.z + (b.scale.z - a.scale.z) * k),
    }
}

/// `CURVE_SIZE` in original/src/Headers/skeletonanim.h.
const CURVE_SIZE: usize = 2000;

/// Port of `AccelerationPercent`: a smoothstep sampled from a lookup table of
/// `CURVE_SIZE` entries, including the table's quantisation.
fn acceleration_curve(percent: f32) -> f32 {
    let index = ((CURVE_SIZE - 1) as f32 * percent.clamp(0.0, 1.0)) as usize;
    let x = index as f32 / CURVE_SIZE as f32;
    x * x * (3.0 - 2.0 * x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Q3Matrix4x4_SetRotate_XYZ` (original/extern/Pomme/src/QD3D/QD3DMath.h)
    /// builds a row-vector matrix; its rows are the images of the axes.
    #[test]
    fn euler_matches_original_matrix() {
        let (x, y, z) = (0.3f32, -1.1f32, 2.0f32);
        let (sx, cx, sy, cy, sz, cz) = (x.sin(), x.cos(), y.sin(), y.cos(), z.sin(), z.cos());
        let rows = [
            Vec3::new(cy * cz, cy * sz, -sy),
            Vec3::new(sx * sy * cz - cx * sz, sx * sy * sz + cx * cz, sx * cy),
            Vec3::new(cx * sy * cz + sx * sz, cx * sy * sz - sx * cz, cx * cy),
        ];
        let q = euler_xyz(Vec3::new(x, y, z));
        for (axis, row) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().zip(rows) {
            assert!((q * axis - row).length() < 1e-5, "{axis} -> {}", q * axis);
        }
    }

    fn load_definition(kind: SkeletonType) -> Arc<SkeletonDefinition> {
        let path = bugdom_formats::original_data_dir().join(kind.path());
        Arc::new(bugdom_formats::skeleton::open(&path).expect("the skeleton parses"))
    }

    #[test]
    fn joints_start_at_their_bind_pose_coordinates() {
        let definition = load_definition(SkeletonType::Me);
        let rig = SkeletonRig::new(definition.clone(), Vec::new());
        let base = Affine3A::from_scale_rotation_translation(
            Vec3::splat(0.5),
            Quat::IDENTITY,
            Vec3::new(100.0, 20.0, -30.0),
        );
        for (joint, bone) in definition.bones.iter().enumerate() {
            let at = joint_position(&rig, joint, Vec3::new(0.0, 2.0, 0.0), base)
                .expect("the joint exists");
            let expected = base.transform_point3(Vec3::from(bone.coord) + Vec3::Y * 2.0);
            assert!(
                (at - expected).length() < 1e-3,
                "{}: {at} != {expected}",
                bone.name
            );
        }
        assert_eq!(
            joint_position(&rig, definition.bones.len(), Vec3::ZERO, base),
            None
        );
    }

    /// A posed joint is where the rendered joint entity ends up.
    #[test]
    fn joint_positions_match_the_posed_joint_entities() {
        let definition = load_definition(SkeletonType::Ant);
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .add_systems(PostUpdate, pose_joints.before(TransformSystems::Propagate));
        let root_transform = Transform::from_xyz(300.0, -40.0, 1200.0)
            .with_rotation(Quat::from_rotation_y(1.2))
            .with_scale(Vec3::splat(1.3));
        let root = app.world_mut().spawn(root_transform).id();
        let joints = spawn_joints(&mut app.world_mut().commands(), &definition, root);
        app.world_mut().flush();
        let mut animator = SkeletonAnimator::default();
        animator.set_anim(1);
        animator.time = 7.5;
        app.world_mut().entity_mut(root).insert((
            SkeletonRig::new(definition.clone(), joints.clone()),
            animator,
        ));

        app.update();

        let world = app.world();
        let rig = world.get::<SkeletonRig>(root).expect("the rig exists");
        let offset = Vec3::new(3.0, -5.0, 8.0);
        let base = root_transform.compute_affine();
        let mut moved = false;
        for (joint, &entity) in joints.iter().enumerate() {
            let at = joint_position(rig, joint, offset, base).expect("the joint exists");
            let global = world.get::<GlobalTransform>(entity).expect("propagated");
            let expected = global.transform_point(offset);
            assert!(
                (at - expected).length() < 1e-2,
                "joint {joint}: {at} != {expected}"
            );
            let bind = base.transform_point3(Vec3::from(definition.bones[joint].coord) + offset);
            moved |= (at - bind).length() > 1.0;
        }
        assert!(
            moved,
            "the animation should move some joint off its bind pose"
        );
    }

    #[test]
    fn acceleration_curve_is_monotonic_smoothstep() {
        assert_eq!(acceleration_curve(0.0), 0.0);
        assert!((acceleration_curve(1.0) - 1.0).abs() < 0.002);
        assert!((acceleration_curve(0.5) - 0.5).abs() < 0.002);
        let mut last = 0.0;
        for i in 0..=100 {
            let v = acceleration_curve(i as f32 / 100.0);
            assert!(v >= last);
            last = v;
        }
    }
}
