//! The ladybug bonus: a ladybug in a cage, freed by a kick or the ball.
//!
//! Port of `AddLadyBugBonus`, `MoveLadyBugBonus`, `KickLadyBugBox`,
//! `MoveLadyBug` and `DoTrig_Cage` (original/src/Items/Triggers2.c).

use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::TransformInterpolation;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use super::MaterialOverride;
use crate::assets::skeleton::SkeletonAsset;
use crate::collision::{
    CollisionBox, CollisionKind, SolidSides, Trigger, TriggerHit, solid_object,
};
use crate::effects::{Explosion, ShardMode, explode_geometry};
use crate::enemies::EnemyCulling;
use crate::items::kind as item;
use crate::items::triggers::ChainedTo;
use crate::items::{
    DespawnOutOfRange, ItemSpawn, RegisterItemKind, TerrainItemSource, forget_terrain_item,
};
use crate::objects::{ModelFile, ModelRef, ModelSpawner, Shading, ShadowOf, attach_shadow};
use crate::player::{Inventory, ItemKicked, Player, PlayerForm, PlayerSpeed, PlayerSystems};
use crate::skeleton::{Skeleton, SkeletonAnimator, SkeletonSystems, SkeletonType};
use crate::state::{AppState, LevelAssets};
use crate::terrain::TerrainMap;

pub(super) fn plugin(app: &mut App) {
    app.register_item_kind(item::LADYBUG_BONUS, add_ladybug_bonus)
        .add_systems(
            FixedUpdate,
            (open_cages, move_ladybugs)
                .chain()
                .after(PlayerSystems::Move)
                .after(SkeletonSystems::Advance)
                .run_if(in_state(AppState::InGame)),
        );
}

/// `GLOBAL1_MObjType_LadyBugCage` and `GLOBAL1_MObjType_LadyBugPost`.
const CAGE_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 9);
const POST_MODEL: ModelRef = ModelRef::new(ModelFile::Global1, 10);
const CAGE_SCALE: f32 = 0.3;
const LADYBUG_SCALE: f32 = 0.9;
/// How far the posts stand from the centre along x and z, before scaling.
const POST_OFFSET: f32 = 430.0;
/// How high the posts and the cage sit above the floor, in units.
const CAGE_HEIGHT: f32 = 10.0;
/// How high the ladybug waits above the floor, in units.
const LADYBUG_HEIGHT: f32 = 100.0;
const CAGE_BOX: CollisionBox = CollisionBox::new(200.0, 0.0, -130.0, 130.0, 130.0, -130.0);
/// How fast the ball must go to smash the cage, in units per second.
const CAGE_SMASH_SPEED: f32 = 900.0;
const LADYBUG_SHADOW_SCALE: f32 = 5.0;
/// Morph rates into the unfolding and flying animations.
const UNFOLD_MORPH_RATE: f32 = 7.0;
const FLY_MORPH_RATE: f32 = 8.0;
/// Upward acceleration of the flying ladybug, in units per second².
const LADYBUG_RISE_ACCELERATION: f32 = 100.0;
/// How fast she turns while flying, in radians per second.
const LADYBUG_SPIN_RATE: f32 = 1.0;
/// She can't be culled until she has risen this far, in units, so that
/// her sound isn't cut off (`LADYBUG_MAX_ALTITUDE_BEFORE_CULLING`).
const LADYBUG_ALTITUDE_BEFORE_CULLING: f32 = 3000.0;
/// Her shadow has faded completely at this altitude, in units.
const LADYBUG_ALTITUDE_FOR_SHADOW: f32 = 1250.0;

/// The ladybug's animations (`LADYBUG_ANIM_*`).
mod anim {
    pub const FLY: usize = 0;
    pub const UNFOLD: usize = 1;
    pub const WAIT: usize = 2;
}

/// A cage with a ladybug in it (`TRIGTYPE_CAGE`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
struct LadyBugCage {
    /// The first post, which holds the map item.
    posts: Entity,
    bug: Option<Entity>,
}

/// A ladybug, caged or freed.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
struct LadyBug {
    /// The child with the skeleton.
    model: Entity,
    freed: bool,
    /// Upward speed, in units per second.
    rise_speed: f32,
    /// Her height when she was put in the cage (`InitCoord.y`).
    start_height: f32,
    /// Her turn about y.
    yaw: f32,
    /// Her bounding sphere's radius, for culling.
    radius: f32,
    /// Whether she may be culled yet (`STATUS_BIT_DONTCULL` cleared).
    cullable: bool,
}

/// Port of `AddLadyBugBonus`: four posts, the cage and the ladybug. The
/// first post holds the map item; the rest are chained to it and go with
/// it.
fn add_ladybug_bonus(
    In(spawn): In<ItemSpawn>,
    mut commands: Commands,
    mut models: ModelSpawner,
    map: Res<TerrainMap>,
    level_assets: Res<LevelAssets>,
    skeletons: Res<Assets<SkeletonAsset>>,
) -> bool {
    let (x, z) = (spawn.position.x, spawn.position.y);
    let floor = map.floor_height(x, z);
    let corner = POST_OFFSET * CAGE_SCALE;
    let offsets = [
        Vec2::new(-corner, -corner),
        Vec2::new(-corner, corner),
        Vec2::new(corner, corner),
        Vec2::new(corner, -corner),
    ];
    let mut first_post = None;
    for (i, offset) in offsets.into_iter().enumerate() {
        let mut post = commands.spawn((
            Name::new("Ladybug cage post"),
            Transform::from_xyz(x + offset.x, floor + CAGE_HEIGHT, z + offset.y),
            Visibility::default(),
            DespawnOnExit(AppState::InGame),
        ));
        match first_post {
            Some(first) => {
                post.insert(ChainedTo(first));
            }
            None => {
                post.insert((TerrainItemSource(spawn.index), DespawnOutOfRange));
            }
        }
        let post = post.id();
        let yaw = -FRAC_PI_2 + FRAC_PI_2 * i as f32;
        models.spawn(
            &mut commands,
            post,
            POST_MODEL,
            Shading::Lit,
            Transform::from_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::splat(CAGE_SCALE)),
        );
        first_post.get_or_insert(post);
    }
    let Some(posts) = first_post else {
        return false;
    };

    let cage = commands
        .spawn((
            Name::new("Ladybug cage"),
            Transform::from_xyz(x, floor + CAGE_HEIGHT, z),
            Visibility::default(),
            ChainedTo(posts),
            DespawnOnExit(AppState::InGame),
            Trigger {
                sides: SolidSides::ALL,
                solid: true,
            },
            solid_object(
                vec![CAGE_BOX],
                [
                    CollisionKind::Misc,
                    CollisionKind::Kickable,
                    CollisionKind::BlockCamera,
                    CollisionKind::Trigger,
                ],
                SolidSides::ALL,
            ),
            // `STATUS_BIT_KEEPBACKFACES`
            MaterialOverride {
                double_sided: true,
                ..default()
            },
        ))
        .id();
    models.spawn(
        &mut commands,
        cage,
        CAGE_MODEL,
        Shading::Lit,
        Transform::from_scale(Vec3::splat(CAGE_SCALE)),
    );

    let bug = level_assets
        .skeleton(SkeletonType::LadyBug)
        .map(|skeleton| {
            let radius = skeletons.get(&skeleton).map_or(0.0, |s| s.radius) * LADYBUG_SCALE;
            let height = floor + LADYBUG_HEIGHT;
            let bug = commands
                .spawn((
                    Name::new("Ladybug"),
                    Transform::from_xyz(x, height, z),
                    Visibility::default(),
                    TransformInterpolation,
                    ChainedTo(cage),
                    DespawnOnExit(AppState::InGame),
                ))
                .id();
            let mut animator = SkeletonAnimator::default();
            animator.set_anim(anim::WAIT);
            let model = commands
                .spawn((
                    Skeleton(skeleton),
                    animator,
                    Transform::from_scale(Vec3::splat(LADYBUG_SCALE)),
                    TransformInterpolation,
                    ChildOf(bug),
                ))
                .id();
            commands.entity(bug).insert(LadyBug {
                model,
                freed: false,
                rise_speed: 0.0,
                start_height: height,
                yaw: 0.0,
                radius,
                cullable: false,
            });
            bug
        });
    if bug.is_none() {
        error!("The level has no ladybug skeleton");
    }
    commands.entity(cage).insert(LadyBugCage { posts, bug });
    true
}

/// Frees the ladybugs of the cages the bug kicks or the ball smashes.
/// Port of `DoTrig_Cage` and of the kick's call to `KickLadyBugBox`.
fn open_cages(
    mut commands: Commands,
    mut models: ModelSpawner,
    mut hits: MessageReader<TriggerHit>,
    mut kicks: MessageReader<ItemKicked>,
    cages: Query<&LadyBugCage>,
    mut players: Query<(&PlayerForm, &PlayerSpeed, &mut Inventory), With<Player>>,
    mut bugs: Query<&mut LadyBug>,
    mut animators: Query<&mut SkeletonAnimator>,
) {
    // The original lets anything that sets off the cage smash it while the
    // player is the ball, judging by that thing's speed; only the player's
    // ball does here.
    let mut openers: Vec<(Entity, Entity)> = Vec::new();
    for hit in hits.read() {
        let Ok((form, speed, _)) = players.get(hit.mover) else {
            continue;
        };
        if cages.contains(hit.trigger) && *form == PlayerForm::Ball && **speed > CAGE_SMASH_SPEED {
            // Sound: EFFECT_POUND at the player, two notes up, at double
            // volume.
            openers.push((hit.trigger, hit.mover));
        }
    }
    openers.extend(kicks.read().map(|kick| (kick.item, kick.player)));

    let mut opened = EntityHashSet::default();
    for (cage, player) in openers {
        let Ok(&state) = cages.get(cage) else {
            continue;
        };
        if !opened.insert(cage) {
            continue;
        }
        kick_ladybug_box(
            &mut commands,
            &mut models,
            cage,
            state,
            &mut bugs,
            &mut animators,
        );
        if let Ok((_, _, mut inventory)) = players.get_mut(player) {
            inventory.get_ladybug();
        }
    }
}

/// `QD3D_ExplodeGeometry(cage, 700, SHARD_MODE_BOUNCE |
/// SHARD_MODE_NULLSHADER, 1, .6)` in `KickLadyBugBox`.
const CAGE_SHARDS: Explosion = Explosion {
    force: 700.0,
    mode: ShardMode::BOUNCE.union(ShardMode::NULL_SHADER),
    density: 1,
    decay: 0.6,
};

/// Releases the ladybug and bursts the cage; the posts stay but never
/// come back. Port of `KickLadyBugBox`, apart from its `GetLadyBug`, which
/// the caller does for the player who opened it.
fn kick_ladybug_box(
    commands: &mut Commands,
    models: &mut ModelSpawner,
    cage: Entity,
    state: LadyBugCage,
    bugs: &mut Query<&mut LadyBug>,
    animators: &mut Query<&mut SkeletonAnimator>,
) {
    if let Some(bug) = state.bug
        && let Ok(mut ladybug) = bugs.get_mut(bug)
    {
        ladybug.freed = true;
        if let Ok(mut animator) = animators.get_mut(ladybug.model) {
            animator.morph_to(anim::UNFOLD, UNFOLD_MORPH_RATE);
        }
        commands
            .entity(bug)
            .remove::<ChainedTo>()
            .insert(DespawnOutOfRange);
        attach_shadow(
            commands,
            models,
            bug,
            Vec2::splat(LADYBUG_SHADOW_SCALE),
            false,
        );
    }
    forget_terrain_item(&mut commands.entity(state.posts));
    commands.queue(explode_geometry(cage, CAGE_SHARDS));
    commands.entity(cage).despawn();
}

/// Where a freed ladybug is after `dt` seconds of flight: her new height,
/// rise speed and turn. Port of the flying part of `MoveLadyBug`.
fn fly(ladybug: &mut LadyBug, height: f32, dt: f32) -> f32 {
    ladybug.rise_speed += LADYBUG_RISE_ACCELERATION * dt;
    ladybug.yaw += LADYBUG_SPIN_RATE * dt;
    let height = height + ladybug.rise_speed * dt;
    if height - ladybug.start_height > LADYBUG_ALTITUDE_BEFORE_CULLING {
        ladybug.cullable = true;
    }
    height
}

/// How visible a freed ladybug's shadow is at a height above her start.
fn shadow_opacity(altitude: f32) -> f32 {
    (1.0 - altitude / LADYBUG_ALTITUDE_FOR_SHADOW).max(0.0)
}

/// Flies the freed ladybugs away. Port of `MoveLadyBug`; going out of the
/// item window is [`DespawnOutOfRange`].
fn move_ladybugs(
    time: Res<Time>,
    mut commands: Commands,
    culling: EnemyCulling,
    mut bugs: Query<(Entity, &mut LadyBug, &mut Transform)>,
    mut animators: Query<&mut SkeletonAnimator>,
    shadows: Query<(Entity, &ShadowOf)>,
) {
    let dt = time.delta_secs();
    for (entity, mut ladybug, mut transform) in &mut bugs {
        if !ladybug.freed {
            continue;
        }
        if ladybug.cullable && culling.is_culled(transform.translation, ladybug.radius) {
            commands.entity(entity).despawn();
            continue;
        }
        let Ok(mut animator) = animators.get_mut(ladybug.model) else {
            continue;
        };
        if animator.has_stopped {
            animator.morph_to(anim::FLY, FLY_MORPH_RATE);
        }
        if animator.anim != anim::FLY {
            continue;
        }
        transform.translation.y = fly(&mut ladybug, transform.translation.y, dt);
        transform.rotation = Quat::from_rotation_y(ladybug.yaw);
        let opacity = shadow_opacity(transform.translation.y - ladybug.start_height);
        for (shadow, owner) in &shadows {
            if owner.0 == entity {
                commands.entity(shadow).insert(MaterialOverride {
                    opacity,
                    ..default()
                });
            }
        }
        // Sound: EFFECT_RESCUE, following her.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ladybug() -> LadyBug {
        LadyBug {
            model: Entity::PLACEHOLDER,
            freed: true,
            rise_speed: 0.0,
            start_height: 100.0,
            yaw: 0.0,
            radius: 50.0,
            cullable: false,
        }
    }

    #[test]
    fn a_freed_ladybug_rises_faster_and_faster() {
        let mut bug = ladybug();
        let mut height = bug.start_height;
        height = fly(&mut bug, height, 1.0);
        assert_eq!(height, 200.0);
        height = fly(&mut bug, height, 1.0);
        assert_eq!(height, 400.0);
        assert_eq!(bug.yaw, 2.0 * LADYBUG_SPIN_RATE);
        assert!(!bug.cullable);
        // Only once she is high enough can she be culled.
        for _ in 0..6 {
            height = fly(&mut bug, height, 1.0);
        }
        assert!(height - bug.start_height > LADYBUG_ALTITUDE_BEFORE_CULLING);
        assert!(bug.cullable);
    }

    #[test]
    fn her_shadow_fades_as_she_rises() {
        assert_eq!(shadow_opacity(0.0), 1.0);
        assert_eq!(shadow_opacity(LADYBUG_ALTITUDE_FOR_SHADOW / 2.0), 0.5);
        assert_eq!(shadow_opacity(LADYBUG_ALTITUDE_FOR_SHADOW * 2.0), 0.0);
    }
}
