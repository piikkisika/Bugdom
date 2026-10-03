//! Splines: the closed paths that enemies, feet and platforms follow.
//!
//! Port of original/src/Terrain/SplineItems.c, and of the spline loading in
//! `ReadDataFromPlayfieldFile` (original/src/System/File.c).
//!
//! Each spline item kind has a prime system, registered with
//! [`RegisterSplineItemKind::register_spline_item_kind`] (the original's
//! `gSplineItemPrimeRoutines`). Unlike map items, spline items are all
//! spawned once, when the level starts, and stay for the whole level. While
//! an object is on a spline it carries [`OnSpline`]; when it is outside the
//! item window it is hidden and drops out of collision instead of being
//! despawned, as the original detaches it from the object list.

use avian3d::prelude::ColliderDisabled;
use bevy::ecs::system::SystemId;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bugdom_formats::terrain::{Spline as RawSpline, SplinePoint};

use crate::assets::terrain::TerrainAsset;
use crate::items::{ItemSystems, ItemWindow};
use crate::player::PlayerSystems;
use crate::state::{AppState, LevelAssets};
use crate::terrain::MAP_TO_WORLD;

pub struct SplinesPlugin;

impl Plugin for SplinesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SplineItemKinds>()
            .configure_sets(
                FixedUpdate,
                (SplineSystems::Visibility, SplineSystems::Move)
                    .chain()
                    .after(PlayerSystems::Move)
                    .before(ItemSystems::Window)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(
                OnEnter(AppState::InGame),
                (
                    // The original primes splines before the first items
                    // (`PrimeInitialTerrain`).
                    prime_splines
                        .after(PlayerSystems::Spawn)
                        .before(ItemSystems::Window),
                    // Spline objects start detached; the first update
                    // shows those inside the window.
                    update_spline_visibility.after(ItemSystems::Window),
                ),
            )
            .add_systems(
                FixedUpdate,
                update_spline_visibility.in_set(SplineSystems::Visibility),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SplineSystems {
    /// Hides or shows each object on a spline (`IsSplineItemVisible`),
    /// before it moves.
    Visibility,
    /// The spline item kinds' move systems (`MoveSplineObjects`) go here.
    /// They run after the other objects move, as in the original's main
    /// loop.
    Move,
}

/// The largest placement on a spline, so that indexing its points never
/// runs past the end (`MAX_PLACEMENT`).
pub const MAX_PLACEMENT: f32 = 1.0 - 1e-5;

/// How many nubs at each end of a spline are wrapped around to the other
/// end when baking, so the loop has no angular pinch at the seam
/// (`numWrapNubs` in `PatchSplineLoop`).
const WRAP_NUBS: usize = 3;
/// The farthest apart, in map pixels, a spline's end nubs can be for it to
/// loop (the assertion in `PatchSplineLoop`).
const MAX_LOOP_GAP: f32 = 20.0;
/// Baked points per map pixel of a span's (approximate) length
/// (`GetSplinePointsPerSpan`).
const POINTS_PER_MAP_PIXEL: f32 = 1.5;

/// A spline baked into points, in world x and z.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spline {
    /// The baked points. Objects move along these at a speed in points per
    /// second, so their spacing sets the timing (see
    /// [`points_per_span`]).
    pub points: Vec<Vec2>,
    /// The control points, in world x and z (for debugging).
    pub nubs: Vec<Vec2>,
}

impl Spline {
    /// The point at `placement` (0 is the start, 1 the end), interpolated
    /// between baked points and wrapping round from the last to the first,
    /// and the index of the point before it.
    ///
    /// Port of `GetCoordOnSpline` (original/src/Terrain/SplineItems.c). An
    /// empty spline gives the origin.
    pub fn coord_at(&self, placement: f32) -> (Vec2, usize) {
        let num_points = self.points.len();
        if num_points == 0 {
            return (Vec2::ZERO, 0);
        }
        let scaled = placement.clamp(0.0, MAX_PLACEMENT) * num_points as f32;
        let index1 = (scaled as usize).min(num_points - 1);
        let index2 = if index1 < num_points - 1 {
            index1 + 1
        } else {
            0
        };
        let frac = scaled - scaled.trunc();
        let p1 = self.points[index1];
        let p2 = self.points[index2];
        (p1 * (1.0 - frac) + p2 * frac, index1)
    }
}

/// The level's splines (`gSplineList`), baked and in world units.
#[derive(Resource, Debug, Clone, Default)]
pub struct Splines(pub Vec<Spline>);

impl Splines {
    /// Bakes the splines of a terrain file.
    ///
    /// Port of the spline part of `ReadDataFromPlayfieldFile`
    /// (original/src/System/File.c), which re-bakes each spline with
    /// `PatchSplineLoop`, and of the scaling in `PrimeSplines`.
    pub fn from_terrain(splines: &[RawSpline]) -> Self {
        Self(
            splines
                .iter()
                .enumerate()
                .map(|(i, spline)| {
                    let points = patch_spline_loop(spline).unwrap_or_else(|reason| {
                        if spline.nubs.len() >= 2 {
                            warn!("Spline {i} keeps its file points: {reason}");
                        }
                        spline.points.iter().map(|&p| to_vec(p)).collect()
                    });
                    Spline {
                        points: points.into_iter().map(|p| p * MAP_TO_WORLD).collect(),
                        nubs: spline
                            .nubs
                            .iter()
                            .map(|&p| to_vec(p) * MAP_TO_WORLD)
                            .collect(),
                    }
                })
                .collect(),
        )
    }

    pub fn get(&self, spline: usize) -> Option<&Spline> {
        self.0.get(spline)
    }
}

fn to_vec(p: SplinePoint) -> Vec2 {
    Vec2::new(p.x, p.z)
}

/// What a prime system is given for one spline item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SplineItemSpawn {
    /// The spline's index in [`Splines`]; put it in the item's [`OnSpline`].
    pub spline: usize,
    /// The item's index among its spline's items.
    pub item: usize,
    pub kind: u16,
    /// Where on the spline the item starts.
    pub placement: f32,
    /// World x and z of that place (`GetCoordOnSpline`).
    pub position: Vec2,
    pub params: [u8; 4],
    pub flags: u16,
}

/// The prime system of each spline item kind (`gSplineItemPrimeRoutines`).
/// A prime system returns whether it spawned something.
#[derive(Resource, Debug, Default)]
pub struct SplineItemKinds(HashMap<u16, SystemId<In<SplineItemSpawn>, bool>>);

pub trait RegisterSplineItemKind {
    /// Makes `system` spawn spline items of `kind` (the numbers in
    /// [`crate::items::kind`]).
    fn register_spline_item_kind<M>(
        &mut self,
        kind: u16,
        system: impl IntoSystem<In<SplineItemSpawn>, bool, M> + 'static,
    ) -> &mut Self;
}

impl RegisterSplineItemKind for App {
    fn register_spline_item_kind<M>(
        &mut self,
        kind: u16,
        system: impl IntoSystem<In<SplineItemSpawn>, bool, M> + 'static,
    ) -> &mut Self {
        let id = self.world_mut().register_system(system);
        // Created on first use, so that plugins can register kinds whatever
        // order they are built in.
        self.world_mut()
            .get_resource_or_init::<SplineItemKinds>()
            .0
            .insert(kind, id);
        self
    }
}

/// An object moving along a spline, which is in the original's spline
/// object list (`STATUS_BIT_ONSPLINE`, `SplineNum`, `SplinePlacement`).
///
/// Spline objects live for the whole level: their prime systems should not
/// give them `DespawnOutOfRange` until they leave the spline.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
#[require(Visibility, DespawnOnExit<AppState> = DespawnOnExit(AppState::InGame))]
pub struct OnSpline {
    /// Index into [`Splines`].
    pub spline: usize,
    /// Where on the spline: 0 is the start, [`MAX_PLACEMENT`] the end.
    pub placement: f32,
    /// Speed along the spline, in baked points per second. Negative goes
    /// backward.
    pub speed: f32,
    /// Whether a zigzagging object is going backward
    /// (`STATUS_BIT_REVERSESPLINE`).
    pub reverse: bool,
    /// Whether the object is on a supertile in the item window, as
    /// [`SplineSystems::Visibility`] last found it (the result of
    /// `IsSplineItemVisible`). Move systems skip work such as animation
    /// while it is false.
    pub visible: bool,
}

impl OnSpline {
    pub fn new(spline: usize, placement: f32, speed: f32) -> Self {
        Self {
            spline,
            placement,
            speed,
            reverse: false,
            visible: false,
        }
    }

    /// The world x and z of the object's place on its spline.
    /// Port of `GetObjectCoordOnSpline` (original/src/Terrain/SplineItems.c).
    pub fn position(&self, splines: &Splines) -> Vec2 {
        self.coord(splines).0
    }

    /// [`Self::position`] and the index of the baked point before it.
    pub fn coord(&self, splines: &Splines) -> (Vec2, usize) {
        splines
            .get(self.spline)
            .map_or((Vec2::ZERO, 0), |s| s.coord_at(self.placement))
    }

    /// Moves along the spline at `speed` for `dt` seconds, looping round at
    /// either end, and returns the new placement.
    ///
    /// Port of `IncreaseSplineIndex` (original/src/Terrain/SplineItems.c),
    /// with the speed per second instead of per frame.
    pub fn advance(&mut self, splines: &Splines, dt: f32) -> f32 {
        let Some(num_points) = self.num_points(splines) else {
            return self.placement;
        };
        let mut placement = self.placement + self.speed * dt / num_points;
        if placement > MAX_PLACEMENT {
            placement = (placement - 1.0).clamp(0.0, MAX_PLACEMENT);
        } else if placement < 0.0 {
            placement = (placement + 1.0).clamp(0.0, MAX_PLACEMENT);
        }
        self.placement = placement;
        placement
    }

    /// Moves along the spline at `speed` for `dt` seconds, turning round at
    /// either end, and returns the new placement.
    ///
    /// Port of `IncreaseSplineIndexZigZag`
    /// (original/src/Terrain/SplineItems.c), with the speed per second
    /// instead of per frame.
    pub fn advance_zigzag(&mut self, splines: &Splines, dt: f32) -> f32 {
        let Some(num_points) = self.num_points(splines) else {
            return self.placement;
        };
        let step = self.speed * dt / num_points;
        if self.reverse {
            self.placement -= step;
            if self.placement <= 0.0 {
                self.placement = 0.0;
                self.reverse = false;
            }
        } else {
            self.placement += step;
            if self.placement >= MAX_PLACEMENT {
                self.placement = MAX_PLACEMENT;
                self.reverse = true;
            }
        }
        self.placement
    }

    fn num_points(&self, splines: &Splines) -> Option<f32> {
        let len = splines.get(self.spline)?.points.len();
        (len > 0).then_some(len as f32)
    }
}

/// Takes an object off its spline, so it moves freely and comes into the
/// object list for good.
///
/// Port of the spline part of `DetachEnemyFromSpline`
/// (original/src/Enemies/Enemy.c): the object is shown and collides again,
/// and loses [`OnSpline`]. The caller gives it its free-roaming behaviour
/// (the original's `moveCall`) and, if it should go when out of range,
/// `DespawnOutOfRange`. Counting it as an enemy (`gNumEnemies`,
/// `gNumEnemyOfKind`) belongs to the enemy base (phase 3 design §2.2) and
/// is not done here.
pub fn detach_from_spline(entity: &mut EntityCommands) {
    entity
        .remove::<(OnSpline, ColliderDisabled)>()
        .insert(Visibility::Inherited);
}

/// Bakes the splines and runs each spline item's prime system.
/// Port of `PrimeSplines` (original/src/Terrain/SplineItems.c).
fn prime_splines(world: &mut World) {
    let level_assets = world.resource::<LevelAssets>().clone();
    let Some(terrain) = world
        .resource::<Assets<TerrainAsset>>()
        .get(&level_assets.terrain)
        .map(|t| t.0.clone())
    else {
        return;
    };
    let splines = Splines::from_terrain(&terrain.splines);
    let spawns: Vec<SplineItemSpawn> = terrain
        .splines
        .iter()
        .zip(&splines.0)
        .enumerate()
        .flat_map(|(s, (raw, baked))| {
            raw.items
                .iter()
                .enumerate()
                .map(move |(i, item)| SplineItemSpawn {
                    spline: s,
                    item: i,
                    kind: item.kind,
                    placement: item.placement,
                    position: baked.coord_at(item.placement).0,
                    params: item.params,
                    flags: item.flags,
                })
        })
        .collect();
    world.insert_resource(splines);

    for spawn in spawns {
        let Some(&system) = world.resource::<SplineItemKinds>().0.get(&spawn.kind) else {
            // Kinds without a prime system yet, and the original's
            // `NilPrime` kinds.
            continue;
        };
        if let Err(error) = world.run_system_with(system, spawn) {
            error!("Spline item kind {} failed to spawn: {error}", spawn.kind);
        }
    }
}

/// Shows the objects on splines whose supertile is in the item window and
/// hides the others, which also takes them out of collision.
///
/// Port of `IsSplineItemVisible` (original/src/Terrain/SplineItems.c). Its
/// shadow follows (see `objects::shadow`), and children inherit it as the
/// original's `ChainNode` does.
fn update_spline_visibility(
    mut commands: Commands,
    window: Option<Res<ItemWindow>>,
    mut objects: Query<(
        Entity,
        &Transform,
        &mut OnSpline,
        &mut Visibility,
        Has<ColliderDisabled>,
    )>,
) {
    let Some(window) = window else {
        return;
    };
    for (entity, transform, mut on_spline, mut visibility, disabled) in &mut objects {
        let visible = window.contains_point(transform.translation.xz());
        on_spline.visible = visible;
        if visible {
            visibility.set_if_neq(Visibility::Inherited);
            if disabled {
                commands.entity(entity).remove::<ColliderDisabled>();
            }
        } else {
            visibility.set_if_neq(Visibility::Hidden);
            if !disabled {
                commands.entity(entity).insert(ColliderDisabled);
            }
        }
    }
}

/// How many baked points each span between nubs gets, and the total.
///
/// Port of `GetSplinePointsPerSpan` (original/src/Terrain/SplineItems.c).
/// The distance is deliberately the crude `CalcQuickDistance`: it is what
/// the original editor used, and spline objects' timing depends on the
/// resulting point spacing.
pub fn points_per_span(nubs: &[Vec2]) -> (Vec<usize>, usize) {
    let mut spans = vec![0; nubs.len()];
    let mut total = 0;
    for (i, pair) in nubs.windows(2).enumerate() {
        let points = (POINTS_PER_MAP_PIXEL * quick_distance(pair[0], pair[1])) as usize;
        spans[i] = points;
        total += points;
    }
    // The final nub is a point of its own, as in the editor's splines.
    if let Some(last) = spans.last_mut() {
        *last = 1;
    }
    (spans, total + 1)
}

/// Port of `CalcQuickDistance` (original/src/QD3D/3DMath.c).
fn quick_distance(a: Vec2, b: Vec2) -> f32 {
    let d = (a - b).abs();
    if d.x > d.y {
        d.x + 0.375 * d.y
    } else {
        d.y + 0.375 * d.x
    }
}

/// Bakes a cubic spline through `nubs`, with `points_per_span[i]` points
/// between nub `i` and the next, plus the last nub if its entry is 1.
///
/// Port of `BakeSpline` (original/src/Terrain/SplineItems.c), which needs
/// at least four nubs.
fn bake_spline(nubs: &[Vec2], points_per_span: &[usize]) -> Vec<Vec2> {
    let n = nubs.len();
    debug_assert!(n >= 4 && points_per_span.len() == n);
    let third = 1.0 / 3.0;
    let mut h0 = vec![Vec2::ZERO; n];
    let mut h1 = vec![Vec2::ZERO; n];
    let mut h2 = vec![Vec2::ZERO; n];
    let mut h3 = vec![Vec2::ZERO; n];
    let mut a = vec![Vec2::ZERO; n];
    let mut b = vec![Vec2::ZERO; n];
    let mut c = vec![Vec2::ZERO; n];
    let d = nubs;

    // Solve the tridiagonal system for the second derivatives.
    for i in 0..n - 2 {
        h2[i] = Vec2::ONE;
        h3[i] = 3.0 * (d[i + 2] - 2.0 * d[i + 1] + d[i]);
    }
    h2[n - 3] = Vec2::ZERO;
    a[0] = Vec2::splat(4.0);
    h1[0] = h3[0] / a[0];
    for i in 1..n - 2 {
        let i1 = i - 1;
        h0[i1] = h2[i1] / a[i1];
        a[i] = 4.0 - h0[i1];
        h1[i] = (h3[i] - h1[i1]) / a[i];
    }
    b[n - 3] = h1[n - 3];
    for i in (0..n - 3).rev() {
        b[i] = h1[i] - h0[i] * b[i + 1];
    }
    for i in (1..n - 1).rev() {
        b[i] = b[i - 1];
    }
    b[0] = Vec2::ZERO;
    b[n - 1] = Vec2::ZERO;
    for i in 0..n - 1 {
        c[i] = (d[i + 1] - d[i]) - (2.0 * b[i] + b[i + 1]) * third;
        a[i] = (b[i + 1] - b[i]) * third;
    }

    let mut points = Vec::with_capacity(points_per_span.iter().sum());
    for nub in 0..n - 1 {
        let subdivisions = points_per_span[nub];
        for span_point in 0..subdivisions {
            let t = span_point as f32 / subdivisions as f32;
            points.push(((a[nub] * t + b[nub]) * t + c[nub]) * t + d[nub]);
        }
    }
    if points_per_span[n - 1] == 1 {
        points.push(nubs[n - 1]);
    }
    points
}

/// Re-bakes a spline from its nubs so that it loops without a seam, in map
/// pixels. Returns why not for splines it cannot loop, which keep the
/// editor's points.
///
/// Port of `PatchSplineLoop` (original/src/Terrain/SplineItems.c). The end
/// nubs are merged, and the nub list is wrapped round by three nubs at each
/// end so the curve is smooth across the seam; the wrapped spans get no
/// points.
fn patch_spline_loop(spline: &RawSpline) -> Result<Vec<Vec2>, &'static str> {
    let mut nubs: Vec<Vec2> = spline.nubs.iter().map(|&p| to_vec(p)).collect();
    let num_nubs = nubs.len();
    if num_nubs <= WRAP_NUBS {
        return Err("too few nubs to loop");
    }
    let (spans, _) = points_per_span(&nubs);

    let (first, last) = (nubs[0], nubs[num_nubs - 1]);
    if first.distance(last) >= MAX_LOOP_GAP {
        return Err("its end nubs are too far apart to loop");
    }
    let merged = (first + last) * 0.5;
    nubs[0] = merged;
    nubs[num_nubs - 1] = merged;

    let wrapped_len = num_nubs + 2 * WRAP_NUBS;
    let mut wrapped = vec![Vec2::ZERO; wrapped_len];
    let mut wrapped_spans = vec![0; wrapped_len];
    wrapped[WRAP_NUBS..WRAP_NUBS + num_nubs].copy_from_slice(&nubs);
    wrapped_spans[WRAP_NUBS..WRAP_NUBS + num_nubs].copy_from_slice(&spans);
    let modulo = |i: isize| i.rem_euclid(num_nubs as isize) as usize;
    for i in 0..WRAP_NUBS {
        wrapped[WRAP_NUBS - 1 - i] = nubs[modulo(-2 - i as isize)];
        wrapped[WRAP_NUBS + num_nubs + i] = nubs[modulo((1 + num_nubs + i) as isize)];
    }
    Ok(bake_spline(&wrapped, &wrapped_spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bugdom_formats::rsrc::ResourceFork;
    use bugdom_formats::terrain::{self, Terrain};

    const LEVELS: [&str; 10] = [
        "Training", "Lawn", "Pond", "Beach", "Flight", "Night", "BeeHive", "QueenBee", "AntHill",
        "AntKing",
    ];

    fn load(name: &str) -> Terrain {
        let path = bugdom_formats::original_data_dir().join(format!("Terrain/{name}.ter.rsrc"));
        let fork = ResourceFork::open(&path).expect("terrain file");
        terrain::parse(&fork).expect("terrain")
    }

    /// The checks `PatchSplineLoop` makes in debug builds: the re-baked
    /// splines stay close to the editor's.
    #[test]
    fn baked_splines_match_the_files() {
        let mut checked = 0;
        for name in LEVELS {
            let terrain = load(name);
            for (i, raw) in terrain.splines.iter().enumerate() {
                if raw.nubs.len() < 2 {
                    continue;
                }
                let nubs: Vec<Vec2> = raw.nubs.iter().map(|&p| to_vec(p)).collect();
                let (spans, total) = points_per_span(&nubs);
                let baked = patch_spline_loop(raw).expect("loops");
                assert_eq!(baked.len(), total, "{name} spline {i}");
                let old = raw.points.len();
                assert!(total <= old && old - total < 2, "{name} spline {i}");

                let (mut span, mut span_points) = (0, 0);
                for (p, (new, old)) in baked.iter().zip(&raw.points).enumerate() {
                    let drift = new.distance(to_vec(*old));
                    let limit = if span == 0 || span >= nubs.len() - 2 {
                        40.0
                    } else {
                        10.0
                    };
                    assert!(drift < limit, "{name} spline {i} point {p} drifts {drift}");
                    span_points += 1;
                    if span_points >= spans[span] {
                        span += 1;
                        span_points = 0;
                    }
                }
                // Seamless: the last point is the first.
                assert_eq!(baked.first(), baked.last(), "{name} spline {i}");
                checked += 1;
            }
        }
        assert!(checked > 20);
    }

    #[test]
    fn lawn_splines_are_scaled_to_world_units() {
        let terrain = load("Lawn");
        let splines = Splines::from_terrain(&terrain.splines);
        assert_eq!(splines.0.len(), 23);
        // Lawn's spline 16 is empty and stays so.
        assert!(splines.0[16].points.is_empty());
        let raw = &terrain.splines[0];
        let spline = &splines.0[0];
        let first_nub = (to_vec(raw.nubs[0]) + to_vec(*raw.nubs.last().unwrap())) * 0.5;
        assert!(spline.points[0].distance(first_nub * MAP_TO_WORLD) < 1e-3);
        assert_eq!(spline.nubs[1], to_vec(raw.nubs[1]) * MAP_TO_WORLD);
    }

    fn square() -> Splines {
        Splines(vec![Spline {
            points: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(10.0, 10.0),
                Vec2::new(0.0, 10.0),
            ],
            nubs: Vec::new(),
        }])
    }

    #[test]
    fn coords_interpolate_and_wrap_round() {
        let splines = square();
        let spline = &splines.0[0];
        assert_eq!(spline.coord_at(0.0), (Vec2::ZERO, 0));
        assert_eq!(spline.coord_at(0.125), (Vec2::new(5.0, 0.0), 0));
        assert_eq!(spline.coord_at(0.5), (Vec2::new(10.0, 10.0), 2));
        // Between the last point and the first.
        let (p, index) = spline.coord_at(0.875);
        assert_eq!(index, 3);
        assert!(p.distance(Vec2::new(0.0, 5.0)) < 1e-4);
        // Out of range placements are clamped.
        assert_eq!(spline.coord_at(-1.0).1, 0);
        assert_eq!(spline.coord_at(2.0).1, 3);
        assert!(spline.coord_at(2.0).0.distance(Vec2::ZERO) < 1e-3);
        assert_eq!(Spline::default().coord_at(0.5), (Vec2::ZERO, 0));
    }

    #[test]
    fn advancing_loops_at_both_ends() {
        let splines = square();
        // Two points per second on a four point spline.
        let mut on = OnSpline::new(0, 0.75, 2.0);
        assert_eq!(on.advance(&splines, 0.25), 0.875);
        assert_eq!(on.advance(&splines, 0.5), 0.125);
        assert_eq!(on.position(&splines), Vec2::new(5.0, 0.0));
        on.speed = -2.0;
        assert_eq!(on.advance(&splines, 0.5), 0.875);
    }

    #[test]
    fn zigzag_turns_round_at_the_ends() {
        let splines = square();
        let mut on = OnSpline::new(0, 0.75, 2.0);
        assert_eq!(on.advance_zigzag(&splines, 1.0), MAX_PLACEMENT);
        assert!(on.reverse);
        on.advance_zigzag(&splines, 0.5);
        assert!((on.placement - (MAX_PLACEMENT - 0.25)).abs() < 1e-6);
        on.advance_zigzag(&splines, 2.0);
        assert_eq!(on.placement, 0.0);
        assert!(!on.reverse);
        assert_eq!(on.advance_zigzag(&splines, 0.5), 0.25);
    }
}
