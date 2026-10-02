use bevy::ecs::system::RunSystemOnce;

use super::items::{Flight, Landing, fly, launch};
use super::*;

const DT: f32 = 1.0 / 60.0;

#[test]
fn a_standing_ant_attacks_only_when_aimed_and_near() {
    let spear_ant = AntBrain {
        held: Some(Entity::PLACEHOLDER),
        ..default()
    };
    assert_eq!(
        standing_attack(&spear_ant, 0.01, 500.0),
        StandingAttack::ThrowSpear
    );
    assert_eq!(
        standing_attack(&spear_ant, 0.1, 500.0),
        StandingAttack::None
    );
    assert_eq!(
        standing_attack(&spear_ant, 0.01, 1000.0),
        StandingAttack::None
    );
    // Without its spear it waits.
    assert_eq!(
        standing_attack(&AntBrain::default(), 0.01, 500.0),
        StandingAttack::None
    );
    let rock_ant = AntBrain {
        rock_thrower: true,
        ..default()
    };
    assert_eq!(
        standing_attack(&rock_ant, 0.01, 500.0),
        StandingAttack::PickUpRock
    );
}

#[test]
fn a_spear_is_thrown_a_little_off_the_ants_aim_and_a_rock_is_lobbed() {
    // Yaw 0 faces −Z; the player is 500 units that way.
    let player = Vec3::new(0.0, 0.0, -500.0);
    let (velocity, flight) = launch(Carried::Spear, Vec3::ZERO, 0.0, player);
    let speed = 500.0 * 1.6;
    assert!((velocity - Vec3::new(-0.1f32.sin(), 0.0, -0.1f32.cos()) * speed).length() < 1e-2);
    assert!((flight.yaw - 0.1).abs() < 1e-6);
    assert!((flight.pitch - std::f32::consts::FRAC_PI_2).abs() < 1e-6);

    let (velocity, flight) = launch(Carried::Rock, Vec3::ZERO, 0.0, player);
    assert!((velocity - Vec3::new(0.0, 300.0, -speed)).length() < 1e-2);
    assert_eq!(flight.pitch, 0.0);
    assert_eq!(flight.yaw, 0.0);
}

#[test]
fn a_spear_tips_down_and_sticks_where_it_lands() {
    let mut flight = Flight {
        pitch: std::f32::consts::FRAC_PI_2,
        yaw: 0.0,
        in_ground: false,
    };
    let mut coord = Vec3::new(0.0, 100.0, 0.0);
    let mut velocity = Vec3::new(0.0, 0.0, -800.0);
    let landing = fly(
        Carried::Spear,
        &mut flight,
        &mut coord,
        &mut velocity,
        |_| 0.0,
        DT,
    );
    assert_eq!(landing, Landing::Flying);
    assert!((flight.pitch - (std::f32::consts::FRAC_PI_2 - 0.8 * DT)).abs() < 1e-6);
    assert!(velocity.y < 0.0);

    let mut landing = Landing::Flying;
    for _ in 0..600 {
        landing = fly(
            Carried::Spear,
            &mut flight,
            &mut coord,
            &mut velocity,
            |_| 0.0,
            DT,
        );
        if landing != Landing::Flying {
            break;
        }
    }
    assert_eq!(landing, Landing::Stuck);
    assert!(flight.in_ground);
    assert_eq!(coord.y, 0.0);
    assert_eq!(velocity, Vec3::ZERO);
}

#[test]
fn a_rock_breaks_when_it_lands() {
    let mut flight = Flight {
        pitch: 0.0,
        yaw: 0.0,
        in_ground: false,
    };
    let mut coord = Vec3::new(0.0, 10.0, 0.0);
    let mut velocity = Vec3::new(0.0, -600.0, 0.0);
    let landing = fly(
        Carried::Rock,
        &mut flight,
        &mut coord,
        &mut velocity,
        |_| 0.0,
        DT,
    );
    assert_eq!(landing, Landing::Shattered);
    assert!(flight.pitch > 0.0 && flight.yaw > 0.0);
}

/// An ant with its model and a player, for the message handlers.
fn world_with_ant(brain: AntBrain) -> (World, Entity, Entity, Entity) {
    let mut world = World::new();
    world.init_resource::<Messages<EnemyKicked>>();
    world.init_resource::<Messages<BallHitEnemy>>();
    world.init_resource::<Messages<EnemyKilled>>();
    let model = world.spawn(SkeletonAnimator::default()).id();
    let ant = world
        .spawn((
            brain,
            EnemyModel(model),
            Velocity::default(),
            Health(ANT_HEALTH),
            CollisionBoxes(vec![ant_box(ANT_HEAD_OFFSET)]),
        ))
        .id();
    let player = world
        .spawn((Player, Velocity(Vec3::new(100.0, 0.0, 0.0))))
        .id();
    (world, ant, model, player)
}

fn kick(player: Entity, enemy: Entity) -> EnemyKicked {
    EnemyKicked {
        player,
        enemy,
        direction: Vec2::NEG_Y,
        damage: crate::enemies::KICK_ENEMY_DAMAGE,
    }
}

#[test]
fn a_kick_knocks_an_ant_on_its_butt_once() {
    let (mut world, ant, model, player) = world_with_ant(AntBrain::default());
    world.write_message(kick(player, ant));
    world.write_message(kick(player, ant));
    world.run_system_once(kick_ants).expect("the system runs");

    let brain = world.get::<AntBrain>(ant).copied().unwrap_or_default();
    assert_eq!(brain.state, AntState::FallOnButt);
    assert_eq!(brain.butt_timer, BUTT_TIME);
    assert!(!brain.dying);
    let animator = world.get::<SkeletonAnimator>(model);
    assert_eq!(animator.map(|a| a.anim), Some(AntState::FallOnButt.anim()));
    assert_eq!(
        world.get::<Velocity>(ant).map(|v| v.0),
        Some(Vec3::new(0.0, KICK_SPEED, -KICK_SPEED))
    );
    // Only the first kick took health and slowed the player.
    let health = world.get::<Health>(ant).map(|h| h.0).unwrap_or_default();
    assert!((health - (ANT_HEALTH - crate::enemies::KICK_ENEMY_DAMAGE)).abs() < 1e-6);
    let slowed = world.get::<Velocity>(player).map(|v| v.0);
    assert_eq!(slowed, Some(Vec3::new(100.0 * PLAYER_SLOWDOWN, 0.0, 0.0)));
    let top = world
        .get::<CollisionBoxes>(ant)
        .and_then(|b| b.0.first().map(|b| b.top));
    assert_eq!(top, Some(ANT_HEAD_OFFSET / 2.0));
}

#[test]
fn a_slow_ball_doesnt_knock_an_ant_down() {
    let (mut world, ant, _, player) = world_with_ant(AntBrain::default());
    world.write_message(BallHitEnemy {
        player,
        enemy: ant,
        ball_velocity: Vec3::new(0.0, 0.0, -1000.0),
        ball_speed: 1000.0,
    });
    world
        .run_system_once(ball_hit_ants)
        .expect("the system runs");
    assert_eq!(
        world.get::<AntBrain>(ant).map(|b| b.state),
        Some(AntState::Stand)
    );

    world.write_message(BallHitEnemy {
        player,
        enemy: ant,
        ball_velocity: Vec3::new(0.0, 0.0, -1500.0),
        ball_speed: 1500.0,
    });
    world
        .run_system_once(ball_hit_ants)
        .expect("the system runs");
    assert_eq!(
        world.get::<AntBrain>(ant).map(|b| b.state),
        Some(AntState::FallOnButt)
    );
    assert_eq!(
        world.get::<Velocity>(ant).map(|v| v.0),
        Some(Vec3::new(0.0, 250.0, -1200.0))
    );
}

#[test]
fn a_knock_that_takes_the_last_health_kills_the_ant() {
    let (mut world, ant, _, player) = world_with_ant(AntBrain::default());
    world.entity_mut(ant).insert(Health(0.3));
    world.write_message(kick(player, ant));
    world.run_system_once(kick_ants).expect("the system runs");
    let brain = world.get::<AntBrain>(ant).copied().unwrap_or_default();
    assert_eq!(brain.state, AntState::FallOnButt);
    assert!(brain.dying);
    assert_eq!(
        world.get::<CollisionLayers>(ant).map(|l| l.memberships),
        Some(CollisionKind::Misc.into())
    );
}

#[test]
fn a_rock_thrower_knocked_down_drops_its_rock() {
    let (mut world, ant, _, player) = world_with_ant(AntBrain {
        rock_thrower: true,
        ..default()
    });
    let rock = world.spawn_empty().id();
    if let Some(mut brain) = world.get_mut::<AntBrain>(ant) {
        brain.held = Some(rock);
    }
    world.write_message(kick(player, ant));
    world.run_system_once(kick_ants).expect("the system runs");
    assert_eq!(world.get::<AntBrain>(ant).and_then(|b| b.held), None);
    assert!(world.get_entity(rock).is_err());
}

#[test]
fn a_killed_ant_falls_on_its_butt_to_die_once() {
    let (mut world, ant, model, _) = world_with_ant(AntBrain {
        state: AntState::Walk,
        ..default()
    });
    for _ in 0..2 {
        world.write_message(EnemyKilled {
            enemy: ant,
            knock: Vec3::ZERO,
        });
    }
    world
        .run_system_once(kill_hurt_ants)
        .expect("the system runs");
    let brain = world.get::<AntBrain>(ant).copied().unwrap_or_default();
    assert_eq!(brain.state, AntState::FallOnButt);
    assert!(brain.dying);
    assert_eq!(brain.butt_timer, BUTT_TIME);
    assert_eq!(
        world.get::<SkeletonAnimator>(model).map(|a| a.anim),
        Some(AntState::FallOnButt.anim())
    );
}

#[test]
fn ants_are_on_the_lawn_and_in_the_ant_hill() {
    use bugdom_formats::rsrc::ResourceFork;
    for name in ["Lawn", "AntHill"] {
        let path = bugdom_formats::original_data_dir().join(format!("Terrain/{name}.ter.rsrc"));
        let fork = ResourceFork::open(&path).expect("terrain file");
        let terrain = bugdom_formats::terrain::parse(&fork).expect("terrain");
        let items = terrain.items.iter().filter(|i| i.kind == kind::ANT).count();
        let on_splines = terrain
            .splines
            .iter()
            .flat_map(|s| &s.items)
            .filter(|i| i.kind == kind::ANT)
            .count();
        assert!(items + on_splines > 0, "no ants in {name}");
    }
}
