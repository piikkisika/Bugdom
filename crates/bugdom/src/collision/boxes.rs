//! Box-against-box collision with side detection.
//!
//! Port of `CollisionDetect` and `HandleCollisions`
//! (original/src/System/Collision.c). Which side of a target was hit is
//! decided by comparing the boxes now with the boxes at the start of the
//! tick, so a mover only bumps into a side it actually crossed.

use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashSet;
use bevy::math::bounding::BoundingVolume;
use bevy::prelude::*;

/// What an object is, for collision purposes (`CType`). An entity's kinds are
/// the memberships of its `CollisionLayers`; a mover's mask chooses which
/// kinds it collides with. The bits match the original's `CTYPE_*` values.
#[derive(PhysicsLayer, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CollisionKind {
    #[default]
    Player,
    Enemy,
    Viscous,
    Trigger,
    Misc,
    /// Shadows lie on top of it.
    BlockShadow,
    MovingPlatform,
    HurtMe,
    HurtEnemy,
    /// With `Trigger`: only the player sets it off.
    PlayerTriggerOnly,
    /// The ball can't hit it.
    Spiked,
    Kickable,
    AutoTarget,
    Liquid,
    /// An enemy that can be bopped on the head.
    Boppable,
    /// The camera rises above it.
    BlockCamera,
    DrainBallTime,
    /// `HurtMe` without knocking the player over.
    HurtNoKnock,
    /// Takes priority over other collisions and can't be pushed through.
    Impenetrable,
    /// With `Impenetrable`: the player isn't sent back to where it was.
    Impenetrable2,
    AutoTargetJump,
}

/// A set of box sides (`SIDE_BITS_*`). As an object's component it says
/// which of its sides are solid, plus [`SolidSides::TOUCHABLE`] for objects
/// that only report contact (`CBits`); as a collision result it says which
/// sides of the mover hit something.
///
/// Sides are named from the mover: `FRONT` is +Z, `RIGHT` is +X, `TOP` is +Y.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SolidSides(pub u8);

impl SolidSides {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(1);
    pub const BOTTOM: Self = Self(1 << 1);
    pub const LEFT: Self = Self(1 << 2);
    pub const RIGHT: Self = Self(1 << 3);
    pub const FRONT: Self = Self(1 << 4);
    pub const BACK: Self = Self(1 << 5);
    /// `ALL_SOLID_SIDES`
    pub const ALL: Self = Self(0b11_1111);
    /// `CBITS_NOTTOP`
    pub const NOT_TOP: Self = Self(Self::LEFT.0 | Self::RIGHT.0 | Self::FRONT.0 | Self::BACK.0);
    /// `CBITS_TOUCHABLE`: reports contact without being solid.
    pub const TOUCHABLE: Self = Self(1 << 6);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for SolidSides {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for SolidSides {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// An axis-aligned box (`CollisionBoxType`), either relative to its object's
/// position or in world space.
///
/// Boxes are kept exactly as the original builds them. A few have their
/// front and back swapped (the exit log facing one way), which makes them
/// never collide; that quirk is kept.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionBox {
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
    /// The −Z side.
    pub back: f32,
    /// The +Z side.
    pub front: f32,
}

impl CollisionBox {
    /// A box from offsets in the order `SetObjectCollisionBounds` takes them.
    pub const fn new(top: f32, bottom: f32, left: f32, right: f32, front: f32, back: f32) -> Self {
        Self {
            left,
            right,
            bottom,
            top,
            back,
            front,
        }
    }

    /// This box moved by `offset`.
    pub fn at(self, offset: Vec3) -> Self {
        Self {
            left: self.left + offset.x,
            right: self.right + offset.x,
            bottom: self.bottom + offset.y,
            top: self.top + offset.y,
            back: self.back + offset.z,
            front: self.front + offset.z,
        }
    }

    /// The original's rectangle intersection test; touching counts.
    pub fn overlaps(&self, other: &Self) -> bool {
        !(self.right < other.left
            || self.left > other.right
            || self.front < other.back
            || self.back > other.front
            || self.bottom > other.top
            || self.top < other.bottom)
    }

    /// Whether a point is inside (`DoSimplePointCollision`).
    pub fn contains(&self, point: Vec3) -> bool {
        self.overlaps(&Self::new(
            point.y, point.y, point.x, point.x, point.z, point.z,
        ))
    }

    /// The box's lower and upper corners, whichever way round its sides are.
    pub fn min_max(&self) -> (Vec3, Vec3) {
        let a = Vec3::new(self.left, self.bottom, self.back);
        let b = Vec3::new(self.right, self.top, self.front);
        (a.min(b), a.max(b))
    }
}

/// An object's collision boxes, relative to its position (`CollisionBoxes`).
/// The first box is the one used when the object itself moves.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct CollisionBoxes(pub Vec<CollisionBox>);

impl CollisionBoxes {
    /// Bounds of all the boxes, relative to the object.
    pub fn bounds(&self) -> Option<bevy::math::bounding::Aabb3d> {
        let (min, max) = self
            .0
            .iter()
            .map(CollisionBox::min_max)
            .reduce(|(min_a, max_a), (min_b, max_b)| (min_a.min(min_b), max_a.max(max_b)))?;
        Some(bevy::math::bounding::Aabb3d {
            min: min.into(),
            max: max.into(),
        })
    }

    /// An avian collider covering all the boxes, for finding candidates.
    pub fn collider(&self) -> Collider {
        let Some(bounds) = self.bounds() else {
            return Collider::compound(Vec::<(Vec3, Quat, Collider)>::new());
        };
        // Give flat boxes some thickness so the broad phase still sees them.
        let size = Vec3::from(bounds.max - bounds.min).max(Vec3::splat(1.0));
        Collider::compound(vec![(
            Vec3::from(bounds.center()),
            Quat::IDENTITY,
            Collider::cuboid(size.x, size.y, size.z),
        )])
    }
}

/// An object that runs a handler when a mover hits one of its trigger
/// sides (`CTYPE_TRIGGER` with `TriggerSides`). The handler runs in the
/// object's own system, after reading the [`TriggerHit`] message.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    /// Which of its sides set it off (`TriggerSides`).
    pub sides: SolidSides,
    /// Whether the mover treats it as solid after setting it off: the value
    /// the original's `DoTrig_*` handler returns.
    pub solid: bool,
}

/// Sent when a mover sets off a [`Trigger`].
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerHit {
    pub trigger: Entity,
    pub mover: Entity,
    /// The mover's sides that hit it.
    pub sides: SolidSides,
}

/// A solid object as a mover sees it during one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct BoxTarget {
    pub entity: Entity,
    pub kinds: LayerMask,
    pub solid: SolidSides,
    /// World boxes now and at the start of the tick.
    pub boxes: Vec<CollisionBox>,
    pub old_boxes: Vec<CollisionBox>,
    pub velocity: Vec3,
    pub trigger: Option<Trigger>,
}

/// The moving object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoxMover {
    pub entity: Entity,
    /// Only the player sets off player-only triggers.
    pub is_player: bool,
    /// Its first collision box, relative to its position.
    pub shape: CollisionBox,
    /// Its position at the start of the tick (`OldCoord`).
    pub old_coord: Vec3,
    /// The velocity of the platform it rides, if any.
    pub platform_velocity: Vec3,
}

/// One entry of the collision list (`CollisionRec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollisionHit {
    /// Index into the targets.
    pub target: usize,
    pub target_box: usize,
    /// The mover's sides that hit; empty for touch-only contact or a trigger
    /// that isn't solid.
    pub sides: SolidSides,
}

/// What [`resolve_box_collisions`] found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoxCollisions {
    /// Every side that hit something (the return value of `HandleCollisions`).
    pub sides: SolidSides,
    /// The whole collision list (`gCollisionList`), which callers scan for
    /// what they hit.
    pub hits: Vec<CollisionHit>,
    /// Landed on something solid (`STATUS_BIT_ONGROUND`).
    pub on_ground: bool,
    /// Triggers set off, once each.
    pub triggered: Vec<TriggerHit>,
}

/// How many times collisions are detected and resolved (`numPasses < 3`).
const MAX_PASSES: usize = 3;
/// Starting value of the push-out search (`-10000`).
const NO_OFFSET: f32 = -10_000.0;

/// Moves `coord` out of the solid sides it crossed this tick and stops the
/// velocity on those axes. `targets` are the candidates; `mask` chooses which
/// kinds count; `dt` is the length of this move, used to lift the mover a
/// little off tops so that it doesn't sink through. `spent` holds triggers
/// that stopped being solid when they went off; they are skipped for the
/// rest of the tick, as the original deletes them.
///
/// Port of `HandleCollisions` (original/src/System/Collision.c), including
/// its habit of handling the whole list again on every pass.
pub fn resolve_box_collisions(
    mover: &BoxMover,
    coord: &mut Vec3,
    velocity: &mut Vec3,
    mask: LayerMask,
    targets: &[BoxTarget],
    dt: f32,
    spent: &mut EntityHashSet,
) -> BoxCollisions {
    let mut result = BoxCollisions::default();
    let original = *coord;
    let mut passes = 0;
    let mut hit_impenetrable = false;

    loop {
        let base = mover.shape.at(*coord);
        detect(
            mover,
            base,
            *coord,
            *velocity,
            mask,
            targets,
            spent,
            &mut result.hits,
        );

        let mut total = SolidSides::NONE;
        let mut max_offset = Vec3::splat(NO_OFFSET);
        let mut sign = Vec3::ZERO;
        let mut solid_hits = 0;

        for hit in &mut result.hits {
            total |= hit.sides;
            let target = &targets[hit.target];
            if spent.contains(&target.entity) {
                continue;
            }
            let target_box = target.boxes[hit.target_box];

            if let Some(trigger) = target.trigger
                && target.kinds.has_all(CollisionKind::Trigger)
                && mask.has_all(CollisionKind::Trigger)
            {
                if !handle_trigger(mover, target, trigger, hit.sides, &mut result.triggered) {
                    hit.sides = SolidSides::NONE;
                    spent.insert(target.entity);
                }
                // A handler may move the mover, so the original takes how
                // far it has moved so far as the push to beat.
                max_offset.x = coord.x - original.x;
                max_offset.z = coord.z - original.z;
            }

            if !hit.sides.intersects(SolidSides::ALL) {
                continue;
            }
            solid_hits += 1;
            if target.kinds.has_all(CollisionKind::Impenetrable) {
                // Throw away every other push.
                hit_impenetrable = true;
                max_offset = Vec3::splat(NO_OFFSET);
                sign = Vec3::ZERO;
            }

            if hit.sides.contains(SolidSides::BACK) {
                push(
                    &mut max_offset.z,
                    &mut sign.z,
                    target_box.front - base.back + 1.0,
                    1.0,
                );
                velocity.z = 0.0;
            } else if hit.sides.contains(SolidSides::FRONT) {
                push(
                    &mut max_offset.z,
                    &mut sign.z,
                    base.front - target_box.back + 1.0,
                    -1.0,
                );
                velocity.z = 0.0;
            }
            if hit.sides.contains(SolidSides::LEFT) {
                push(
                    &mut max_offset.x,
                    &mut sign.x,
                    target_box.right - base.left + 1.0,
                    1.0,
                );
                velocity.x = 0.0;
            } else if hit.sides.contains(SolidSides::RIGHT) {
                push(
                    &mut max_offset.x,
                    &mut sign.x,
                    base.right - target_box.left + 1.0,
                    -1.0,
                );
                velocity.x = 0.0;
            }
            if hit.sides.contains(SolidSides::BOTTOM) {
                // Lift a little more than flush, or the mover would fall
                // through; but not so much that it jitters. The original
                // adds one frame's time, which at 60 fps is 1/60.
                push(
                    &mut max_offset.y,
                    &mut sign.y,
                    target_box.top - base.bottom + dt,
                    1.0,
                );
                velocity.y = 0.0;
            } else if hit.sides.contains(SolidSides::TOP) {
                push(
                    &mut max_offset.y,
                    &mut sign.y,
                    base.top - target_box.bottom + 1.0,
                    -1.0,
                );
                velocity.y = 0.0;
            }

            if hit_impenetrable {
                break;
            }
        }
        result.sides = total;

        if solid_hits == 0 {
            break;
        }
        *coord = original + max_offset * sign;
        if total.contains(SolidSides::BOTTOM) {
            result.on_ground = true;
        }
        passes += 1;
        if passes >= MAX_PASSES || hit_impenetrable {
            break;
        }
    }
    result
}

/// Keeps the biggest push along one axis.
fn push(max: &mut f32, sign: &mut f32, offset: f32, direction: f32) {
    if offset > *max {
        *max = offset;
        *sign = direction;
    }
}

/// Adds every box the mover overlaps to `hits`, with the sides it crossed.
///
/// Port of `CollisionDetect` (original/src/System/Collision.c).
fn detect(
    mover: &BoxMover,
    base: CollisionBox,
    coord: Vec3,
    velocity: Vec3,
    mask: LayerMask,
    targets: &[BoxTarget],
    spent: &EntityHashSet,
    hits: &mut Vec<CollisionHit>,
) {
    let old_base = mover.shape.at(mover.old_coord);
    // Only the direction of motion matters. The first pass uses the
    // velocity; later ones how far the mover has really got this tick.
    let motion = if hits.is_empty() {
        velocity + mover.platform_velocity
    } else {
        coord - mover.old_coord
    };

    for (index, target) in targets.iter().enumerate() {
        if (target.kinds & mask) == LayerMask::NONE
            || target.solid.is_empty()
            || target.entity == mover.entity
            || spent.contains(&target.entity)
        {
            continue;
        }
        for (box_index, (now, old)) in target.boxes.iter().zip(&target.old_boxes).enumerate() {
            if !base.overlaps(now) {
                continue;
            }
            // Touch-only targets (liquids, the player) are listed with no
            // sides, whichever way the mover came in.
            let touchable = target.solid.contains(SolidSides::TOUCHABLE);
            let sides = if touchable {
                SolidSides::NONE
            } else {
                crossed_sides(
                    &base,
                    &old_base,
                    now,
                    old,
                    target.solid,
                    motion - target.velocity,
                )
            };
            if sides.is_empty() && !touchable && !target.kinds.has_all(CollisionKind::Impenetrable)
            {
                continue;
            }
            hits.push(CollisionHit {
                target: index,
                target_box: box_index,
                sides,
            });
        }
    }
}

/// Which of the mover's sides crossed into a solid side of the target since
/// the start of the tick, moving at `relative` to it.
fn crossed_sides(
    base: &CollisionBox,
    old_base: &CollisionBox,
    target: &CollisionBox,
    old_target: &CollisionBox,
    solid: SolidSides,
    relative: Vec3,
) -> SolidSides {
    let mut sides = SolidSides::NONE;

    if solid.contains(SolidSides::BACK) && relative.z > 0.0 {
        if old_base.front < old_target.back
            && base.front >= target.back
            && base.front <= target.front
        {
            sides = SolidSides::FRONT;
        }
    } else if solid.contains(SolidSides::FRONT)
        && relative.z < 0.0
        && old_base.back > old_target.front
        && base.back <= target.front
        && base.back >= target.back
    {
        sides = SolidSides::BACK;
    }

    if solid.contains(SolidSides::LEFT) && relative.x > 0.0 {
        if old_base.right < old_target.left
            && base.right >= target.left
            && base.right <= target.right
        {
            sides |= SolidSides::RIGHT;
        }
    } else if solid.contains(SolidSides::RIGHT)
        && relative.x < 0.0
        && old_base.left > old_target.right
        && base.left <= target.right
        && base.left >= target.left
    {
        sides |= SolidSides::LEFT;
    }

    if solid.contains(SolidSides::BOTTOM) && relative.y > 0.0 {
        if old_base.top < old_target.bottom && base.top >= target.bottom && base.top <= target.top {
            sides |= SolidSides::TOP;
        }
    } else if solid.contains(SolidSides::TOP)
        && relative.y < 0.0
        // Unlike the other sides, already touching the top counts as
        // having been outside.
        && old_base.bottom >= old_target.top
        && base.bottom <= target.top
        && base.bottom >= target.bottom
    {
        sides |= SolidSides::BOTTOM;
    }
    sides
}

/// Decides whether a hit sets the trigger off, recording it if so, and
/// returns whether the trigger is solid to the mover.
///
/// Port of `HandleTrigger` (original/src/Items/Triggers.c). The handler
/// itself runs later, in the trigger's own system.
fn handle_trigger(
    mover: &BoxMover,
    target: &BoxTarget,
    trigger: Trigger,
    sides: SolidSides,
    triggered: &mut Vec<TriggerHit>,
) -> bool {
    if target.kinds.has_all(CollisionKind::PlayerTriggerOnly) && !mover.is_player {
        return true;
    }
    // The mover's side that hit must face one of the trigger's sides; the
    // first side found decides.
    let facing = [
        (SolidSides::BACK, SolidSides::FRONT),
        (SolidSides::FRONT, SolidSides::BACK),
        (SolidSides::LEFT, SolidSides::RIGHT),
        (SolidSides::RIGHT, SolidSides::LEFT),
        (SolidSides::TOP, SolidSides::BOTTOM),
        (SolidSides::BOTTOM, SolidSides::TOP),
    ]
    .into_iter()
    .find(|(mine, _)| sides.contains(*mine));
    let Some((_, theirs)) = facing else {
        return true;
    };
    if !trigger.sides.contains(theirs) {
        return true;
    }
    if !triggered.iter().any(|t| t.trigger == target.entity) {
        triggered.push(TriggerHit {
            trigger: target.entity,
            mover: mover.entity,
            sides,
        });
    }
    trigger.solid
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn mover(old_coord: Vec3) -> BoxMover {
        BoxMover {
            entity: Entity::from_raw_u32(1).unwrap(),
            is_player: true,
            shape: CollisionBox::new(180.0, 0.0, -42.0, 42.0, 42.0, -42.0),
            old_coord,
            platform_velocity: Vec3::ZERO,
        }
    }

    /// A rock: a 200-unit cube at the origin, solid on every side.
    fn rock(kinds: LayerMask) -> BoxTarget {
        let shape = CollisionBox::new(200.0, 0.0, -100.0, 100.0, 100.0, -100.0);
        BoxTarget {
            entity: Entity::from_raw_u32(2).unwrap(),
            kinds,
            solid: SolidSides::ALL,
            boxes: vec![shape],
            old_boxes: vec![shape],
            velocity: Vec3::ZERO,
            trigger: None,
        }
    }

    fn resolve(
        old: Vec3,
        new: Vec3,
        velocity: Vec3,
        target: &BoxTarget,
    ) -> (Vec3, Vec3, BoxCollisions) {
        let mut coord = new;
        let mut velocity = velocity;
        let result = resolve_box_collisions(
            &mover(old),
            &mut coord,
            &mut velocity,
            LayerMask::ALL,
            std::slice::from_ref(target),
            DT,
            &mut EntityHashSet::default(),
        );
        (coord, velocity, result)
    }

    #[test]
    fn touchable_targets_are_listed_without_sides_or_a_push() {
        let mut target = rock(CollisionKind::Liquid.into());
        target.solid = SolidSides::TOUCHABLE;
        // Already inside it, standing still.
        let inside = Vec3::new(0.0, 50.0, 0.0);
        let (coord, _, result) = resolve(inside, inside, Vec3::ZERO, &target);
        assert_eq!(coord, inside);
        assert_eq!(
            result.hits,
            [CollisionHit {
                target: 0,
                target_box: 0,
                sides: SolidSides::NONE,
            }]
        );
    }

    #[test]
    fn walking_into_a_side_pushes_back_out() {
        let target = rock(CollisionKind::Misc.into());
        // Walking +X: the mover's right side crosses the rock's left side.
        let (coord, velocity, result) = resolve(
            Vec3::new(-145.0, 0.0, 0.0),
            Vec3::new(-135.0, 0.0, 0.0),
            Vec3::new(600.0, 0.0, 0.0),
            &target,
        );
        assert_eq!(result.sides, SolidSides::RIGHT);
        assert_eq!(coord.x, -100.0 - 42.0 - 1.0);
        assert_eq!(velocity.x, 0.0);
        assert!(!result.on_ground);
    }

    #[test]
    fn landing_on_top_stands_on_it() {
        let target = rock(CollisionKind::Misc.into());
        let (coord, velocity, result) = resolve(
            Vec3::new(0.0, 205.0, 0.0),
            Vec3::new(0.0, 195.0, 0.0),
            Vec3::new(0.0, -600.0, 0.0),
            &target,
        );
        assert_eq!(result.sides, SolidSides::BOTTOM);
        assert!(result.on_ground);
        assert!((coord.y - (200.0 + DT)).abs() < 1e-4);
        assert_eq!(velocity.y, 0.0);
    }

    #[test]
    fn already_inside_is_not_pushed() {
        // Side detection needs the side to have been crossed this tick.
        let target = rock(CollisionKind::Misc.into());
        let (coord, _, result) = resolve(
            Vec3::new(-50.0, 50.0, 0.0),
            Vec3::new(-40.0, 50.0, 0.0),
            Vec3::new(600.0, 0.0, 0.0),
            &target,
        );
        assert!(result.hits.is_empty());
        assert_eq!(coord, Vec3::new(-40.0, 50.0, 0.0));
    }

    #[test]
    fn masked_out_kinds_are_ignored() {
        let target = rock(CollisionKind::Enemy.into());
        let mut coord = Vec3::new(-135.0, 0.0, 0.0);
        let result = resolve_box_collisions(
            &mover(Vec3::new(-145.0, 0.0, 0.0)),
            &mut coord,
            &mut Vec3::new(600.0, 0.0, 0.0),
            CollisionKind::Misc.into(),
            &[target],
            DT,
            &mut EntityHashSet::default(),
        );
        assert!(result.hits.is_empty());
    }

    #[test]
    fn a_trigger_that_is_not_solid_lets_the_mover_through_once() {
        let mut target = rock(CollisionKind::Trigger.into());
        target.trigger = Some(Trigger {
            sides: SolidSides::ALL,
            solid: false,
        });
        let mut spent = EntityHashSet::default();
        let mut coord = Vec3::new(-135.0, 0.0, 0.0);
        let result = resolve_box_collisions(
            &mover(Vec3::new(-145.0, 0.0, 0.0)),
            &mut coord,
            &mut Vec3::new(600.0, 0.0, 0.0),
            LayerMask::ALL,
            std::slice::from_ref(&target),
            DT,
            &mut spent,
        );
        assert_eq!(coord.x, -135.0);
        assert_eq!(result.triggered.len(), 1);
        assert_eq!(result.triggered[0].sides, SolidSides::RIGHT);
        assert!(spent.contains(&target.entity));
    }

    #[test]
    fn swapped_boxes_never_collide() {
        let mut target = rock(CollisionKind::Misc.into());
        let flipped = CollisionBox::new(200.0, 0.0, -100.0, 100.0, -100.0, 100.0);
        target.boxes = vec![flipped];
        target.old_boxes = vec![flipped];
        let (coord, _, result) = resolve(
            Vec3::new(0.0, 0.0, -145.0),
            Vec3::new(0.0, 0.0, -135.0),
            Vec3::new(0.0, 0.0, 600.0),
            &target,
        );
        assert!(result.hits.is_empty());
        assert_eq!(coord.z, -135.0);
    }
}
