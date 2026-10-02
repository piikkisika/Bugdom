//! The caterpillar: a spiked enemy that crawls round its spline for the
//! whole level.
//!
//! Port of original/src/Enemies/Enemy_Caterpiller.c. It never leaves its
//! spline, and it can't be hurt or kicked: it only hurts the player who
//! touches it, through the player's own collision (`CTYPE_SPIKED`).
//! `KillEnemy` has no case for it, so a hurt that empties its health does
//! nothing, and the kick and the ball don't answer for it either.

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

pub struct CaterpillarPlugin;

impl Plugin for CaterpillarPlugin {
    fn build(&self, app: &mut App) {
        add_crawling(app);
        app.register_spline_item_kind(kind::CATERPILLAR, prime_caterpillar)
            .add_systems(FixedUpdate, move_caterpillars.in_set(SplineSystems::Move));
    }
}

const CATERPILLER_HEALTH: f32 = 1.0;
/// Damage to the player on contact (`CATERPILLER_DAMAGE`).
const CATERPILLER_DAMAGE: f32 = 0.2;
const CATERPILLER_SCALE: f32 = 3.0;
/// Speed along the spline, in baked points per second
/// (`CATERPILLER_SPEED`).
const CATERPILLER_SPEED: f32 = 45.0;
/// How far below the floor its joints are, in units. Its origin is as far
/// *above* the floor (`Coord.y += CATERPILLER_FOOT_OFFSET`), as in the
/// original.
const CATERPILLER_FOOT_OFFSET: f32 = 30.0;

/// How the caterpillar's body lies on its spline (`CATERPILLER_STRETCH`,
/// `CATERPILLER_FOOT_OFFSET`, `CATERPILLER_COLLISIONBOX_SIZE`,
/// `CATERPILLER_SCALE`).
const CATERPILLER_SHAPE: CrawlingShape = CrawlingShape {
    joint_spacing: 40.0,
    foot_offset: CATERPILLER_FOOT_OFFSET,
    box_size: 90.0,
    scale: CATERPILLER_SCALE,
};

/// A caterpillar.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caterpillar;

/// Port of `PrimeEnemy_Caterpiller` and `SetCaterpillerCollisionInfo`
/// (original/src/Enemies/Enemy_Caterpiller.c). Its boxes, one per joint,
/// are set by its first move in the window. It starts on its inching
/// animation (`CATERPILLER_ANIM_INCH`, animation 0), which global joints
/// never play.
fn prime_caterpillar(In(spawn): In<SplineItemSpawn>, mut enemies: EnemySpawner) -> bool {
    let Some(caterpillar) = enemies.spawn(
        EnemySkeleton::new(
            EnemyKind::Caterpiller,
            SkeletonType::Caterpiller,
            spawn.position,
            CATERPILLER_SCALE,
        )
        .on_spline(OnSpline::new(
            spawn.spline,
            spawn.placement,
            CATERPILLER_SPEED,
        ))
        .foot_offset(-CATERPILLER_FOOT_OFFSET)
        .with_kinds(CollisionKind::Spiked)
        .solid(SolidSides::TOUCHABLE)
        .health(CATERPILLER_HEALTH)
        .damage(CATERPILLER_DAMAGE),
    ) else {
        return false;
    };
    let mut caterpillar = enemies.commands().entity(caterpillar);
    caterpillar.insert((Caterpillar, Crawler::new(CATERPILLER_SHAPE)));
    make_joints_global(&mut caterpillar);
    true
}

/// Port of `MoveCaterpillerOnSpline`
/// (original/src/Enemies/Enemy_Caterpiller.c).
fn move_caterpillars(
    time: Res<Time>,
    splines: Option<Res<Splines>>,
    map: Res<TerrainMap>,
    rigs: Query<&SkeletonRig>,
    mut caterpillars: Query<CrawlerBody, With<Caterpillar>>,
) {
    let Some(splines) = splines else {
        return;
    };
    let world = CrawlingWorld {
        splines: &splines,
        map: &map,
        dt: time.delta_secs(),
    };
    for mut body in &mut caterpillars {
        let rig = rigs.get(body.model.0).ok();
        crawl_on_spline(&mut body, &world, rig);
    }
}
