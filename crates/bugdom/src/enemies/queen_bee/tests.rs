use bevy::ecs::system::RunSystemOnce;
use bugdom_formats::terrain::Item;

use super::*;

const DT: f32 = 1.0 / 60.0;

fn base_item(x: u16, z: u16, id: u8) -> Item {
    Item {
        x,
        z,
        kind: kind::QUEEN_BEE,
        params: [id, 0, 0, 0],
        flags: 0,
    }
}

#[test]
fn the_bases_are_visited_in_order_of_their_numbers() {
    let other = Item {
        kind: kind::QUEEN_BEE + 1,
        ..base_item(0, 0, 9)
    };
    let items = TerrainItems::new(vec![
        base_item(10, 10, 0),
        other,
        base_item(30, 30, 2),
        base_item(20, 20, 1),
    ]);
    let mut bases = QueenBases::find(&items);
    assert_eq!(bases.bases.len(), 3);
    assert_eq!(bases.bases[0].position, Vec2::splat(10.0 * MAP_TO_WORLD));
    assert_eq!(bases.advance(), Some(Vec2::splat(20.0 * MAP_TO_WORLD)));
    assert_eq!(bases.advance(), Some(Vec2::splat(30.0 * MAP_TO_WORLD)));
    // Back to the first.
    assert_eq!(bases.advance(), Some(Vec2::splat(10.0 * MAP_TO_WORLD)));
    assert_eq!(bases.current, 0);

    // A missing number gives no base.
    let items = TerrainItems::new(vec![base_item(10, 10, 0), base_item(20, 20, 5)]);
    let mut bases = QueenBases::find(&items);
    assert_eq!(bases.advance(), None);
}

#[test]
fn the_queen_bee_level_has_the_queen_and_numbered_bases() {
    use bugdom_formats::rsrc::ResourceFork;
    let path = bugdom_formats::original_data_dir().join("Terrain/QueenBee.ter.rsrc");
    let fork = ResourceFork::open(&path).expect("terrain file");
    let terrain = bugdom_formats::terrain::parse(&fork).expect("terrain");
    let items = TerrainItems::new(terrain.items.clone());
    let bases = QueenBases::find(&items);
    assert!(bases.bases.len() > 1, "{bases:?}");
    assert_eq!(
        bases.bases.iter().filter(|b| b.id == 0).count(),
        1,
        "one item makes the queen"
    );
    // Every number up to the count has a base, so the queen always finds
    // her next one.
    let mut walk = bases.clone();
    for _ in 0..bases.bases.len() {
        assert!(walk.advance().is_some(), "{bases:?}");
    }
}

#[test]
fn the_flight_climbs_to_the_midpoint_hovers_and_lands() {
    let mut brain = QueenBeeBrain {
        state: QueenBeeState::Fly(FlyMode::ToMidpoint),
        midpoint: midpoint(Vec2::ZERO, Vec2::new(1000.0, 0.0), |_| 50.0),
        end_point: Vec2::new(1000.0, 0.0),
        ..default()
    };
    assert_eq!(brain.midpoint, Vec3::new(500.0, 50.0 + HOVER_HEIGHT, 0.0));
    let mut coord = Vec3::ZERO;
    let mut velocity = Vec3::ZERO;
    let check = fly(
        &mut brain,
        FlyMode::ToMidpoint,
        &mut coord,
        &mut velocity,
        None,
        DT,
    );
    assert!(!check, "still climbing");
    assert_eq!(velocity, Vec3::new(FLY_SPEED, FLY_SPEED, 0.0));

    let mut ticks = 0;
    while brain.state == QueenBeeState::Fly(FlyMode::ToMidpoint) && ticks < 1000 {
        fly(
            &mut brain,
            FlyMode::ToMidpoint,
            &mut coord,
            &mut velocity,
            None,
            DT,
        );
        ticks += 1;
    }
    assert_eq!(brain.state, QueenBeeState::Fly(FlyMode::Hover));
    assert_eq!(brain.timer, HOVER_TIME);

    while brain.state == QueenBeeState::Fly(FlyMode::Hover) && ticks < 2000 {
        assert!(fly(
            &mut brain,
            FlyMode::Hover,
            &mut coord,
            &mut velocity,
            None,
            DT
        ));
        ticks += 1;
    }
    assert_eq!(brain.state, QueenBeeState::Fly(FlyMode::Land));

    while brain.state == QueenBeeState::Fly(FlyMode::Land) && ticks < 4000 {
        fly(
            &mut brain,
            FlyMode::Land,
            &mut coord,
            &mut velocity,
            None,
            DT,
        );
        ticks += 1;
    }
    assert_eq!(brain.state, QueenBeeState::Wait);
    assert_eq!(brain.timer, LANDED_WAIT_TIME);
    assert_eq!(velocity, Vec3::ZERO);
    assert!(quick_distance(coord.xz(), brain.end_point) < BASE_REACHED_DIST);
}

/// A queen with her model and a player, for the message handlers.
fn world_with_queen(health: f32) -> (World, Entity, Entity, Entity) {
    let mut world = World::new();
    world.init_resource::<Messages<BallHitEnemy>>();
    world.init_resource::<Messages<EnemyKicked>>();
    world.init_resource::<Messages<EnemyKilled>>();
    world.init_resource::<ParticleGroups>();
    world.init_resource::<GameRandom>();
    let model = world.spawn(SkeletonAnimator::default()).id();
    let queen = world
        .spawn((
            QueenBeeBrain::default(),
            Transform::default(),
            EnemyModel(model),
            Velocity::default(),
            Health(health),
        ))
        .id();
    let player = world
        .spawn((Player, Velocity(Vec3::new(100.0, 50.0, 0.0))))
        .id();
    (world, queen, model, player)
}

fn ball_hit(player: Entity, enemy: Entity, speed: f32) -> BallHitEnemy {
    BallHitEnemy {
        player,
        enemy,
        ball_velocity: Vec3::new(0.0, 0.0, -speed),
        ball_speed: speed,
    }
}

fn brain(world: &World, queen: Entity) -> QueenBeeBrain {
    world
        .get::<QueenBeeBrain>(queen)
        .copied()
        .unwrap_or_default()
}

#[test]
fn only_a_fast_ball_knocks_the_queen_on_her_butt_once() {
    let (mut world, queen, model, player) = world_with_queen(QUEEN_BEE_HEALTH);
    world.write_message(ball_hit(player, queen, 1300.0));
    world
        .run_system_once(ball_hit_queen_bee)
        .expect("the system runs");
    assert_eq!(brain(&world, queen).state, QueenBeeState::Wait);

    world.write_message(ball_hit(player, queen, 1500.0));
    world.write_message(ball_hit(player, queen, 1500.0));
    world
        .run_system_once(ball_hit_queen_bee)
        .expect("the system runs");
    let b = brain(&world, queen);
    assert_eq!(b.state, QueenBeeState::OnButt);
    assert_eq!(b.butt_timer, BUTT_TIME);
    assert_eq!(
        world.get::<SkeletonAnimator>(model).map(|a| a.anim),
        Some(QueenBeeState::OnButt.anim())
    );
    assert_eq!(
        world.get::<Velocity>(queen).map(|v| v.0),
        Some(Vec3::new(0.0, KNOCK_RISE, -1500.0 * BALL_KNOCK_SHARE))
    );
    // Only the first knock took health and slowed the player.
    assert_eq!(
        world.get::<Health>(queen).map(|h| h.0),
        Some(QUEEN_BEE_HEALTH - BALL_DAMAGE)
    );
    assert_eq!(
        world.get::<Velocity>(player).map(|v| v.0),
        Some(Vec3::new(100.0, 50.0, 0.0) * PLAYER_SLOWDOWN)
    );
    assert_eq!(world.resource::<ParticleGroups>().iter().count(), 1);
}

#[test]
fn the_kick_knocks_her_along_the_kick() {
    let (mut world, queen, _, player) = world_with_queen(QUEEN_BEE_HEALTH);
    world.write_message(EnemyKicked {
        player,
        enemy: queen,
        direction: Vec2::new(1.0, 0.0),
        damage: crate::enemies::KICK_ENEMY_DAMAGE,
    });
    world
        .run_system_once(kick_queen_bee)
        .expect("the system runs");
    assert_eq!(brain(&world, queen).state, QueenBeeState::OnButt);
    assert_eq!(
        world.get::<Velocity>(queen).map(|v| v.0),
        Some(Vec3::new(KICK_KNOCK_SPEED, KNOCK_RISE, 0.0))
    );
}

#[test]
fn a_knock_that_takes_her_last_health_kills_her() {
    let (mut world, queen, model, player) = world_with_queen(0.5);
    world.write_message(ball_hit(player, queen, 1500.0));
    world
        .run_system_once(ball_hit_queen_bee)
        .expect("the system runs");
    let b = brain(&world, queen);
    assert_eq!(b.state, QueenBeeState::Death);
    assert_eq!(b.death_timer, DEATH_TIME);
    assert_eq!(
        world.get::<SkeletonAnimator>(model).map(|a| a.anim),
        Some(QueenBeeState::Death.anim())
    );
    assert_eq!(
        world.get::<CollisionLayers>(queen).map(|l| l.memberships),
        Some(CollisionKind::Misc.into())
    );
    // A dead queen can't be knocked again.
    world.write_message(ball_hit(player, queen, 1500.0));
    world
        .run_system_once(ball_hit_queen_bee)
        .expect("the system runs");
    assert_eq!(brain(&world, queen).state, QueenBeeState::Death);
}

#[test]
fn she_dies_once_when_killed() {
    let (mut world, queen, _, _) = world_with_queen(0.0);
    for _ in 0..2 {
        world.write_message(EnemyKilled {
            enemy: queen,
            knock: Vec3::ZERO,
        });
    }
    world
        .run_system_once(kill_hurt_queen_bee)
        .expect("the system runs");
    let b = brain(&world, queen);
    assert_eq!(b.state, QueenBeeState::Death);
    assert_eq!(b.death_timer, DEATH_TIME);
    // One group of sparks, for one death.
    assert_eq!(world.resource::<ParticleGroups>().iter().count(), 1);
}

#[test]
fn the_systems_have_no_conflicting_parameters() {
    fn check<M>(system: impl IntoSystem<(), (), M>) {
        let mut world = World::new();
        let mut system = IntoSystem::into_system(system);
        // Panics on parameters that conflict.
        system.initialize(&mut world);
    }
    check(ball_hit_queen_bee);
    check(kick_queen_bee);
    check(kill_hurt_queen_bee);
    check(move_queen_bee);
    check(spit::shoot_spit);
    check(spit::move_queen_spit);
}
