//! The slug: a spiked enemy that crawls round its spline for the whole
//! level.
//!
//! Port of original/src/Enemies/Enemy_Slug.c. It never leaves its spline,
//! and it can't be hurt or kicked: it only hurts the player who touches
//! it, through the player's own collision (`CTYPE_SPIKED`). `KillEnemy`
//! has no case for it, so a hurt that empties its health does nothing,
//! and the kick and the ball don't answer for it either.

use bevy::prelude::*;

use super::crawling::{
    Crawler, CrawlerBody, CrawlingShape, CrawlingWorld, add_crawling, crawl_on_spline,
    make_joints_global,
};
use super::{EnemyKind, EnemySkeleton, EnemySpawner};
use crate::collision::{CollisionKind, SolidSides};
use crate::items::kind;
use crate::skeleton::{SkeletonRig, SkeletonType};
use crate::splines::{OnSpline, RegisterSplineItemKind, SplineItemSpawn, SplineSystems, Splines};
use crate::terrain::TerrainMap;

pub struct SlugPlugin;

impl Plugin for SlugPlugin {
    fn build(&self, app: &mut App) {
        add_crawling(app);
        app.register_spline_item_kind(kind::SLUG, prime_slug)
            .add_systems(FixedUpdate, move_slugs.in_set(SplineSystems::Move));
    }
}

const SLUG_HEALTH: f32 = 1.0;
/// Damage to the player on contact (`SLUG_DAMAGE`).
const SLUG_DAMAGE: f32 = 0.1;
const SLUG_SCALE: f32 = 3.0;
/// Speed along the spline, in baked points per second.
const SLUG_SPEED: f32 = 90.0;

/// How the slug's body lies on its spline (`SLUG_STRETCH`,
/// `SLUG_FOOT_OFFSET`, `SLUG_COLLISIONBOX_SIZE`, `SLUG_SCALE`).
const SLUG_SHAPE: CrawlingShape = CrawlingShape {
    joint_spacing: 30.0,
    foot_offset: 0.0,
    box_size: 40.0,
    scale: SLUG_SCALE,
};

/// A slug.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Slug;

/// Port of `PrimeEnemy_Slug` and `SetSlugCollisionInfo`
/// (original/src/Enemies/Enemy_Slug.c). Its boxes, one per joint, are set
/// by its first move in the window. It starts on its inching animation
/// (`SLUG_ANIM_INCH`, animation 0), which global joints never play.
fn prime_slug(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let Some(slug) = enemies.spawn(
        EnemySkeleton::new(
            EnemyKind::Slug,
            SkeletonType::Slug,
            spawn.position,
            SLUG_SCALE,
        )
        .on_spline(OnSpline::new(spawn.spline, spawn.placement, SLUG_SPEED))
        .foot_offset(SLUG_SHAPE.foot_offset)
        .with_kinds(CollisionKind::Spiked)
        .solid(SolidSides::TOUCHABLE)
        .health(SLUG_HEALTH)
        .damage(SLUG_DAMAGE),
    ) else {
        return false;
    };
    let mut slug = enemies.commands().entity(slug);
    slug.insert((Slug, Crawler::new(SLUG_SHAPE)));
    make_joints_global(&mut slug);
    true
}

/// Port of `MoveSlugOnSpline` (original/src/Enemies/Enemy_Slug.c).
fn move_slugs(
    time: Res<Time>,
    splines: Option<Res<Splines>>,
    map: Res<TerrainMap>,
    rigs: Query<&SkeletonRig>,
    mut slugs: Query<CrawlerBody, With<Slug>>,
) {
    let Some(splines) = splines else {
        return;
    };
    let world = CrawlingWorld {
        splines: &splines,
        map: &map,
        dt: time.delta_secs(),
    };
    for mut body in &mut slugs {
        let rig = rigs.get(body.model.0).ok();
        crawl_on_spline(&mut body, &world, rig);
    }
}
