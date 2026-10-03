use bevy::ecs::system::RunSystemOnce;

use super::*;

/// A standing roach with its model and a player, for the message
/// handlers.
fn world_with_roach() -> (World, Entity, Entity, Entity) {
    let mut world = World::new();
    world.init_resource::<Messages<EnemyKicked>>();
    world.init_resource::<Messages<BallHitEnemy>>();
    world.init_resource::<Messages<EnemyKilled>>();
    let model = world.spawn(SkeletonAnimator::default()).id();
    let roach = world
        .spawn((
            RoachBrain::default(),
            Velocity::default(),
            Health(ROACH_HEALTH),
            EnemyModel(model),
        ))
        .id();
    let player = world
        .spawn((Player, Velocity(Vec3::new(100.0, 50.0, -200.0))))
        .id();
    (world, roach, model, player)
}

fn kick(world: &mut World, player: Entity, roach: Entity, damage: f32) {
    world.write_message(EnemyKicked {
        player,
        enemy: roach,
        direction: Vec2::NEG_Y,
        damage,
    });
    world
        .run_system_once(kick_roaches)
        .expect("the system runs");
    world.resource_mut::<Messages<EnemyKicked>>().clear();
}

#[test]
fn states_play_their_animations() {
    assert_eq!(RoachState::Stand.anim(), 0);
    assert_eq!(RoachState::Walk.anim(), 1);
    assert_eq!(RoachState::OnButt.anim(), 2);
    assert_eq!(RoachState::Death.anim(), 3);
}

#[test]
fn the_add_guard_counts_only_roaches_unless_told_to_always_add() {
    assert!(may_add_roach([0; 4], 0));
    assert!(may_add_roach([0; 4], MAX_ROACHES - 1));
    assert!(!may_add_roach([0; 4], MAX_ROACHES));
    assert!(may_add_roach([0, 0, 0, 1], MAX_ROACHES + 5));
}

#[test]
fn gas_comes_every_four_tenths_of_a_second_or_a_little_sooner() {
    let mut random = GameRandom::default();
    let mut timer = 0.0;
    let dt = 0.01;
    let mut puffs = Vec::new();
    for tick in 0..300 {
        if gas_due(&mut timer, dt, &mut random) {
            puffs.push(tick);
        }
    }
    assert!((39..=40).contains(&puffs[0]));
    for pair in puffs.windows(2) {
        let gap = pair[1] - pair[0];
        assert!((30..=41).contains(&gap), "gap {gap}");
    }
}

#[test]
fn a_kick_knocks_the_roach_on_its_butt_once_and_slows_the_player() {
    let (mut world, roach, model, player) = world_with_roach();
    kick(&mut world, player, roach, 0.4);
    let brain = *world.get::<RoachBrain>(roach).expect("a roach");
    assert_eq!(brain.state, RoachState::OnButt);
    assert_eq!(brain.butt_timer, BUTT_TIME);
    assert_eq!(
        **world.get::<Velocity>(roach).expect("a velocity"),
        Vec3::new(0.0, KICK_KNOCK_RISE, -KICK_KNOCK_SPEED)
    );
    assert_eq!(world.get::<Health>(roach), Some(&Health(0.6)));
    let animator = world.get::<SkeletonAnimator>(model).expect("an animator");
    assert_eq!(animator.anim, RoachState::OnButt.anim());
    assert!(animator.is_morphing());
    let slowed = Vec3::new(100.0, 50.0, -200.0) * PLAYER_SLOWDOWN;
    assert!((**world.get::<Velocity>(player).expect("a velocity") - slowed).length() < 1e-4);

    // Already down, a second kick does nothing.
    kick(&mut world, player, roach, 0.4);
    assert_eq!(world.get::<Health>(roach), Some(&Health(0.6)));
    assert!((**world.get::<Velocity>(player).expect("a velocity") - slowed).length() < 1e-4);
}

#[test]
fn a_knock_that_takes_the_last_health_kills_the_roach() {
    let (mut world, roach, model, player) = world_with_roach();
    kick(&mut world, player, roach, 1.0);
    assert_eq!(
        world.get::<RoachBrain>(roach).map(|b| b.state),
        Some(RoachState::Death)
    );
    assert_eq!(
        world.get::<CollisionLayers>(roach).map(|l| l.memberships),
        Some(LayerMask::from(CollisionKind::Misc))
    );
    assert_eq!(
        world.get::<SkeletonAnimator>(model).map(|a| a.anim),
        Some(RoachState::Death.anim())
    );
}

#[test]
fn only_a_fast_ball_knocks_a_roach_down() {
    let (mut world, roach, _, player) = world_with_roach();
    let hit = |world: &mut World, speed: f32| {
        world.write_message(BallHitEnemy {
            player,
            enemy: roach,
            ball_velocity: Vec3::new(speed, 100.0, 0.0),
            ball_speed: speed,
        });
        world
            .run_system_once(ball_hit_roaches)
            .expect("the system runs");
        world.resource_mut::<Messages<BallHitEnemy>>().clear();
    };
    hit(&mut world, ROACH_KNOCKDOWN_SPEED);
    assert_eq!(
        world.get::<RoachBrain>(roach).map(|b| b.state),
        Some(RoachState::Stand)
    );
    hit(&mut world, 2000.0);
    assert_eq!(
        world.get::<RoachBrain>(roach).map(|b| b.state),
        Some(RoachState::OnButt)
    );
    assert_eq!(
        **world.get::<Velocity>(roach).expect("a velocity"),
        Vec3::new(1600.0, 80.0 + BALL_KNOCK_RISE, 0.0)
    );
    assert_eq!(world.get::<Health>(roach), Some(&Health(0.5)));
}

#[test]
fn a_kill_plays_the_death_once() {
    let (mut world, roach, model, _) = world_with_roach();
    world.write_message(EnemyKilled {
        enemy: roach,
        knock: Vec3::ZERO,
    });
    world
        .run_system_once(kill_hurt_roaches)
        .expect("the system runs");
    world.resource_mut::<Messages<EnemyKilled>>().clear();
    assert_eq!(
        world.get::<RoachBrain>(roach).map(|b| b.state),
        Some(RoachState::Death)
    );
    // A later kill doesn't start the death over.
    world
        .get_mut::<SkeletonAnimator>(model)
        .expect("an animator")
        .time = 5.0;
    world.write_message(EnemyKilled {
        enemy: roach,
        knock: Vec3::ZERO,
    });
    world
        .run_system_once(kill_hurt_roaches)
        .expect("the system runs");
    assert_eq!(
        world.get::<SkeletonAnimator>(model).map(|a| a.time),
        Some(5.0)
    );
}
