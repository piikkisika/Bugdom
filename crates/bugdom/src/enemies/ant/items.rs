//! The spears and rocks the ants hold and throw.
//!
//! Port of `GiveAntASpear`, `UpdateAntSpear`, `AntThrowSpear`,
//! `MoveAntSpear`, `GiveAntARock`, `UpdateAntRock`, `AntThrowRock` and
//! `MoveAntRock` (original/src/Enemies/Enemy_Ant.c).
//!
//! A held item is a child of its ant, so that it is shown, hidden and
//! deleted with it, as the original's `ChainNode` is. Thrown, it leaves the
//! ant and becomes a projectile: a `HurtMe` object whose
//! [`Damage`] the player's own collision takes, flying until it lands.

use std::f32::consts::{FRAC_PI_2, PI};

use avian3d::prelude::{CollisionLayers, LayerMask, TransformInterpolation};
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::AntBrain;
use crate::collision::{CollisionBox, CollisionKind, SolidSides, solid_object};
use crate::combat::Damage;
use crate::effects::{Explosion, ShardMode, explode_geometry};
use crate::enemies::EnemyModel;
use crate::items::DespawnOutOfRange;
use crate::math::quick_distance;
use crate::objects::{ModelFile, ModelRef, ModelSpawner, ObjectModel, Shading, attach_shadow};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::Player;
use crate::skeleton::SkeletonRig;
use crate::state::AppState;
use crate::terrain::TerrainMap;

/// `GLOBAL1_MObjType_Spear`
const SPEAR_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 5);
/// `GLOBAL1_MObjType_ThrowRock`
const ROCK_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 6);
/// The ant's joint that holds an item (`ANT_HOLDING_LIMB`).
const HOLDING_JOINT: usize = 4;
/// Where a held spear and a held rock are, in the holding joint's space.
const SPEAR_GRIP: Vec3 = Vec3::new(21.0, -80.0, -33.0);
const ROCK_GRIP: Vec3 = Vec3::new(30.0, 0.0, -50.0);

/// What a thrown spear or rock does to the player (`SPEAR_DAMAGE`; the
/// rock uses it too).
const SPEAR_DAMAGE: f32 = 0.12;
/// How much faster than its distance to the player, per second, an item
/// is thrown (`* 1.6f`): it would reach the player in 1/1.6 s if nothing
/// pulled it down.
const THROW_SPEED_PER_DISTANCE: f32 = 1.6;
/// How far to the side of the ant's aim a spear is thrown, in radians
/// ("offset a tad").
const SPEAR_AIM_OFFSET: f32 = 0.1;
/// The angle a spear leaves the hand at, pointing ahead (`Rot.x = PI/2`).
const SPEAR_LAUNCH_PITCH: f32 = FRAC_PI_2;
/// How fast a flying spear tips down, in radians per second, until it
/// points straight down at `-PI`.
const SPEAR_TIP_RATE: f32 = 0.8;
/// Gravity on a flying spear, in units per second squared.
const SPEAR_GRAVITY: f32 = 1100.0;
/// A thrown spear's collision box.
const SPEAR_BOX: CollisionBox = CollisionBox::new(30.0, -30.0, -30.0, 30.0, 30.0, -30.0);
/// A thrown spear's shadow, across and along it.
const SPEAR_SHADOW_SCALE: Vec2 = Vec2::new(1.7, 4.5);

/// How fast a rock is thrown up, in units per second.
const ROCK_THROW_RISE: f32 = 300.0;
/// Gravity on a flying rock, in units per second squared.
const ROCK_GRAVITY: f32 = 1300.0;
/// How fast a flying rock tumbles about x and y, in radians per second.
const ROCK_SPIN: Vec2 = Vec2::new(3.2, 1.2);
/// A thrown rock's collision box.
const ROCK_BOX: CollisionBox = CollisionBox::new(40.0, -40.0, -40.0, 40.0, 40.0, -40.0);
/// A thrown rock's shadow.
const ROCK_SHADOW_SCALE: Vec2 = Vec2::new(2.0, 2.0);

/// What an ant holds or threw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carried {
    Spear,
    Rock,
}

impl Carried {
    fn model(self) -> ModelRef {
        match self {
            Self::Spear => SPEAR_MODEL,
            Self::Rock => ROCK_MODEL,
        }
    }

    fn grip(self) -> Vec3 {
        match self {
            Self::Spear => SPEAR_GRIP,
            Self::Rock => ROCK_GRIP,
        }
    }
}

/// A spear or rock, held or thrown.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct AntItem {
    pub kind: Carried,
    /// The ant that holds it, or threw it (`SpearOwner`).
    pub owner: Entity,
    /// Its model, which carries its tilt.
    pub model: Option<Entity>,
    /// Where it was in the world when last placed in the hand.
    pub held_at: Vec3,
    /// Its flight, once thrown.
    pub flight: Option<Flight>,
}

impl AntItem {
    /// A spear that has landed (`SpearIsInGround`).
    pub fn is_in_ground(&self) -> bool {
        self.flight.is_some_and(|f| f.in_ground)
    }
}

/// A thrown item's tilt and landing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    /// About its own x axis, in radians (`Rot.x`).
    pub pitch: f32,
    /// About y (`Rot.y`).
    pub yaw: f32,
    /// A spear stuck in the ground (`SpearIsInGround`).
    pub in_ground: bool,
}

/// The items, as the ants' move reads and throws them.
pub type AntItems<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut AntItem),
    (Without<AntBrain>, Without<Player>),
>;

/// Gives an ant a new spear or rock in its hand, unless it holds a rock
/// already. It arrives when the commands are applied.
pub fn give_item(commands: &mut Commands, ant: Entity, kind: Carried) {
    commands.run_system_cached_with(give_ant_item, (ant, kind));
}

/// Port of `GiveAntASpear` and `GiveAntARock`
/// (original/src/Enemies/Enemy_Ant.c). Only the rock checks for something
/// already in the hand.
fn give_ant_item(
    In((ant, kind)): In<(Entity, Carried)>,
    mut commands: Commands,
    mut models: ModelSpawner,
    mut ants: Query<&mut AntBrain>,
) {
    let Ok(mut brain) = ants.get_mut(ant) else {
        return;
    };
    if kind == Carried::Rock && brain.held.is_some() {
        return;
    }
    let item = commands
        .spawn((
            Name::new(format!("Ant's {kind:?}")),
            Transform::default(),
            Visibility::default(),
            ChildOf(ant),
        ))
        .id();
    let Some(model) = models.spawn(
        &mut commands,
        item,
        kind.model(),
        Shading::Lit,
        Transform::default(),
    ) else {
        commands.entity(item).despawn();
        return;
    };
    commands.entity(model).insert(TransformInterpolation);
    commands.entity(item).insert(AntItem {
        kind,
        owner: ant,
        model: Some(model),
        held_at: Vec3::ZERO,
        flight: None,
    });
    brain.held = Some(item);
}

/// Where an item held at `grip` in the holding joint's space is, relative
/// to the ant, given its model's transform, if the rig has the joint.
fn grip_transform(rig: &SkeletonRig, model: &Transform, grip: Vec3) -> Option<Affine3A> {
    let joint = rig.joint_transform(HOLDING_JOINT, model.compute_affine())?;
    Some(joint * Affine3A::from_translation(grip))
}

/// Puts each held spear and rock in its ant's hand.
///
/// Port of `UpdateAntSpear` and `UpdateAntRock`
/// (original/src/Enemies/Enemy_Ant.c). As there, the joints are where the
/// last frame drew them.
pub(super) fn hold_ant_items(
    ants: Query<(&Transform, &AntBrain, &EnemyModel)>,
    models: Query<(&Transform, Option<&SkeletonRig>), Without<AntItem>>,
    mut items: Query<(&mut Transform, &mut AntItem), Without<AntBrain>>,
) {
    for (ant_transform, brain, model) in &ants {
        let Some(held) = brain.held else {
            continue;
        };
        let Ok((mut transform, mut item)) = items.get_mut(held) else {
            continue;
        };
        let Ok((model_transform, Some(rig))) = models.get(model.0) else {
            continue;
        };
        let Some(grip) = grip_transform(rig, model_transform, item.kind.grip()) else {
            continue;
        };
        *transform = Transform::from_matrix(Mat4::from(grip));
        item.held_at = ant_transform
            .compute_affine()
            .transform_point3(grip.translation.into());
    }
}

/// The velocity and tilt an item leaves the hand with, thrown from `from`
/// by an ant facing `ant_yaw` at a player at `player`.
///
/// Port of the throw vectors in `AntThrowSpear` and `AntThrowRock`
/// (original/src/Enemies/Enemy_Ant.c). A rock keeps no tilt from the hand.
pub fn launch(kind: Carried, from: Vec3, ant_yaw: f32, player: Vec3) -> (Vec3, Flight) {
    let speed = quick_distance(player.xz(), from.xz()) * THROW_SPEED_PER_DISTANCE;
    let (yaw, pitch, rise) = match kind {
        Carried::Spear => (ant_yaw + SPEAR_AIM_OFFSET, SPEAR_LAUNCH_PITCH, 0.0),
        Carried::Rock => (ant_yaw, 0.0, ROCK_THROW_RISE),
    };
    let velocity = Vec3::new(-yaw.sin() * speed, rise, -yaw.cos() * speed);
    let flight = Flight {
        pitch,
        yaw: match kind {
            Carried::Spear => yaw,
            Carried::Rock => 0.0,
        },
        in_ground: false,
    };
    (velocity, flight)
}

/// Throws a held item from where it was last held: it leaves the ant,
/// hurts what it hits, and casts a shadow.
///
/// Port of the parts `AntThrowSpear` and `AntThrowRock` share
/// (original/src/Enemies/Enemy_Ant.c).
pub fn throw_item(
    commands: &mut Commands,
    models: &mut ModelSpawner,
    entity: Entity,
    transform: &mut Transform,
    item: &mut AntItem,
    ant_yaw: f32,
    player: Vec3,
) {
    let at = item.held_at;
    let (velocity, flight) = launch(item.kind, at, ant_yaw, player);
    item.flight = Some(flight);
    *transform = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(flight.yaw));
    if let Some(model) = item.model {
        commands
            .entity(model)
            .insert(Transform::from_rotation(Quat::from_rotation_x(
                flight.pitch,
            )));
    }
    let (shape, shadow) = match item.kind {
        Carried::Spear => (SPEAR_BOX, SPEAR_SHADOW_SCALE),
        Carried::Rock => (ROCK_BOX, ROCK_SHADOW_SCALE),
    };
    commands.entity(entity).remove::<ChildOf>().insert((
        Velocity(velocity),
        PreviousPosition(at),
        TransformInterpolation,
        solid_object(vec![shape], CollisionKind::HurtMe, SolidSides::ALL),
        Damage(SPEAR_DAMAGE),
        DespawnOutOfRange,
        DespawnOnExit(AppState::InGame),
    ));
    attach_shadow(commands, models, entity, shadow, false);
    if item.kind == Carried::Spear {
        // Sound: EFFECT_THROWSPEAR at the spear.
    }
}

/// What a flying item did this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    Flying,
    /// A spear stuck in the ground.
    Stuck,
    /// A rock hit the ground and broke.
    Shattered,
}

/// Flies a thrown item for `dt` seconds over a floor at `floor` (its
/// height under where the item gets to).
///
/// Port of the moves in `MoveAntSpear` and `MoveAntRock`
/// (original/src/Enemies/Enemy_Ant.c).
pub fn fly(
    kind: Carried,
    flight: &mut Flight,
    coord: &mut Vec3,
    velocity: &mut Vec3,
    floor: impl Fn(Vec3) -> f32,
    dt: f32,
) -> Landing {
    match kind {
        Carried::Spear => {
            velocity.y -= SPEAR_GRAVITY * dt;
            *coord += *velocity * dt;
            if flight.pitch > -PI {
                flight.pitch -= SPEAR_TIP_RATE * dt;
            }
            let ground = floor(*coord);
            if coord.y <= ground {
                coord.y = ground;
                *velocity = Vec3::ZERO;
                flight.in_ground = true;
                return Landing::Stuck;
            }
        }
        Carried::Rock => {
            velocity.y -= ROCK_GRAVITY * dt;
            *coord += *velocity * dt;
            flight.pitch += ROCK_SPIN.x * dt;
            flight.yaw += ROCK_SPIN.y * dt;
            if coord.y <= floor(*coord) {
                return Landing::Shattered;
            }
        }
    }
    Landing::Flying
}

/// `QD3D_ExplodeGeometry(theNode, 500, 0, 1, .3)` in `MoveAntRock`.
const ROCK_SHARDS: Explosion = Explosion {
    force: 500.0,
    mode: ShardMode::NONE,
    density: 1,
    decay: 0.3,
};

/// Moves the thrown spears and rocks. Leaving the item window
/// (`TrackTerrainItem`) is `DespawnOutOfRange`.
///
/// Port of `MoveAntSpear` and `MoveAntRock`
/// (original/src/Enemies/Enemy_Ant.c). A spear in the ground stays there as
/// a plain obstacle; a rock that lands shatters into shards
/// (`QD3D_ExplodeGeometry`) and is gone.
pub(super) fn move_thrown_items(
    mut commands: Commands,
    time: Res<Time>,
    map: Res<TerrainMap>,
    mut items: Query<(Entity, &mut Transform, &mut Velocity, &mut AntItem)>,
    mut models: Query<&mut Transform, (With<ObjectModel>, Without<AntItem>)>,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut velocity, mut item) in &mut items {
        let kind = item.kind;
        let Some(flight) = item.flight.as_mut() else {
            continue;
        };
        if flight.in_ground {
            continue;
        }
        let mut coord = transform.translation;
        let landing = fly(
            kind,
            flight,
            &mut coord,
            &mut velocity,
            |at| map.floor_height(at.x, at.z),
            dt,
        );
        transform.translation = coord;
        transform.rotation = Quat::from_rotation_y(flight.yaw);
        let pitch = flight.pitch;
        if let Some(mut model) = item.model.and_then(|m| models.get_mut(m).ok()) {
            model.rotation = Quat::from_rotation_x(pitch);
        }
        match landing {
            Landing::Flying => {}
            Landing::Stuck => {
                // Sound: EFFECT_HITDIRT at the spear.
                // In the ground it can't hurt; it is just solid.
                commands
                    .entity(entity)
                    .insert(CollisionLayers::new(CollisionKind::Misc, LayerMask::NONE));
            }
            Landing::Shattered => {
                // Sound: EFFECT_HITDIRT at the rock.
                commands.queue(explode_geometry(entity, ROCK_SHARDS));
                commands.entity(entity).despawn();
            }
        }
    }
}
