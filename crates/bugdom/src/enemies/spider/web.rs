//! The web a spider spits, and the sphere of web it wraps the player in.
//!
//! Port of `SpiderShootWeb`, `MoveWebBullet`, `DoTrig_WebBullet` and
//! `MoveWebSphere` (original/src/Enemies/Enemy_Spider.c).
//!
//! The web bullet is a harmless trigger that only the player sets off.
//! Touching it sends [`HoldPlayer`] with [`Hold::Webbed`] (the player's
//! side, `MovePlayerBug_Webbed`, is in the player plugin) and wraps the
//! player in a [`WebSphere`]. The sphere lets the player go
//! ([`ReleasePlayer`]) when its time is up, and goes away as soon as the
//! player is out of the web some other way.
//!
//! The sphere bursts into shards when it goes (`QD3D_ExplodeGeometry`);
//! shards aren't ported yet, so it just vanishes.

use std::f32::consts::PI;

use avian3d::prelude::TransformInterpolation;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

use super::SPIDER_SCALE;
use crate::collision::{
    CollisionBox, CollisionKind, SolidSides, Trigger, TriggerHit, solid_object,
};
use crate::items::DespawnOutOfRange;
use crate::objects::{ModelRef, ModelSpawner, ObjectMaterial, Shading};
use crate::physics::{PreviousPosition, Velocity};
use crate::player::{BugState, Dying, Hold, HoldPlayer, Player, PlayerForm, ReleasePlayer};
use crate::state::AppState;

/// How fast a web bullet flies, in units per second (`WEB_SPEED`).
pub const WEB_SPEED: f32 = 600.0;
/// A web bullet's size when spat (`SPIDER_SCALE * .1`).
const WEB_BULLET_START_SCALE: f32 = SPIDER_SCALE * 0.1;
/// How fast a web bullet grows, in scale per second.
const WEB_BULLET_GROWTH: f32 = 0.5;
/// How fast a web bullet spins about its z axis, in radians per second.
const WEB_BULLET_SPIN: f32 = 2.0;
/// How long a web bullet flies, in seconds (`Health = 1.0`).
const WEB_BULLET_LIFE: f32 = 1.0;
/// A web bullet fades out over the last third of its life: its opacity is
/// its remaining life times this, once that is under 1.
const WEB_BULLET_FADE: f32 = 3.0;
/// A web bullet's trigger box (`SetObjectCollisionBounds(newObj, 50, -100,
/// -50, 50, 50, -50)`).
const WEB_BULLET_BOX: CollisionBox = CollisionBox::new(50.0, -100.0, -50.0, 50.0, 50.0, -50.0);

/// The web sphere's size (`WEB_SPHERE_SCALE`).
const WEB_SPHERE_SCALE: f32 = 1.2;
/// How long the sphere holds the player, in seconds (`SPHERE_DURATION`).
pub const SPHERE_DURATION: f32 = 2.5;
/// How fast the sphere's x, y and z sizes wobble, in radians per second.
const SPHERE_WOBBLE_RATE: Vec3 = Vec3::new(6.0, 8.0, 5.0);
/// How much the sphere's size wobbles.
const SPHERE_WOBBLE: f32 = 0.1;

/// A web bullet in flight.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct WebBullet {
    /// Seconds left before it is gone (`Health`).
    pub life: f32,
    /// Its size (`SpecialF[3]`).
    pub scale: f32,
    /// Its heading (`Rot.y`).
    pub yaw: f32,
    /// Its spin about its own z axis (`Rot.z`).
    pub spin: f32,
    /// Its model, which carries its rotation and size.
    pub model: Option<Entity>,
}

impl WebBullet {
    fn new(yaw: f32) -> Self {
        Self {
            life: WEB_BULLET_LIFE,
            scale: WEB_BULLET_START_SCALE,
            yaw,
            spin: 0.0,
            model: None,
        }
    }

    /// Ages the bullet by `dt` seconds. Returns false once it has burned
    /// out, as `MoveWebBullet` deletes it before moving it.
    fn age(&mut self, dt: f32) -> bool {
        self.life -= dt;
        if self.life <= 0.0 {
            return false;
        }
        self.scale += dt * WEB_BULLET_GROWTH;
        self.spin += dt * WEB_BULLET_SPIN;
        true
    }

    /// Its opacity (`MakeObjectTransparent` over the end of its life).
    fn opacity(&self) -> f32 {
        (self.life * WEB_BULLET_FADE).min(1.0)
    }

    /// Its model's transform: turned `ROTXZY` (with no x turn, the spin
    /// about z, then the heading about y) and scaled.
    fn model_transform(&self) -> Transform {
        Transform::from_rotation(Quat::from_rotation_y(self.yaw) * Quat::from_rotation_z(self.spin))
            .with_scale(Vec3::splat(self.scale))
    }
}

/// The sphere of web around a webbed player.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct WebSphere {
    /// The player it holds.
    pub player: Entity,
    /// Seconds before it lets the player go (`Health`).
    pub life: f32,
    /// The wobble angles of its x, y and z sizes (`SpecialF[0..2]`).
    pub wobble: Vec3,
    /// Its model, which carries its size.
    pub model: Option<Entity>,
}

impl WebSphere {
    fn new(player: Entity) -> Self {
        Self {
            player,
            life: SPHERE_DURATION,
            wobble: Vec3::ZERO,
            model: None,
        }
    }

    /// Its size in x, y and z: x and z wobble with a sine, y with a cosine.
    fn scale(&self) -> Vec3 {
        let w = self.wobble;
        Vec3::splat(WEB_SPHERE_SCALE) + Vec3::new(w.x.sin(), w.y.cos(), w.z.sin()) * SPHERE_WOBBLE
    }
}

/// Makes the meshes below an entity glow (`STATUS_BIT_GLOW`, additive),
/// show their back faces (`STATUS_BIT_KEEPBACKFACES`) and fade
/// (`MakeObjectTransparent`). The meshes get materials of their own the
/// first time, so other objects of the same model are left alone.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct WebGlow {
    pub opacity: f32,
}

/// A web mesh's own material, and the alpha its model gave it.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct OwnWebMaterial {
    alpha: f32,
    applied: f32,
}

/// The web models of a level type, if it has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebModels {
    pub thread: ModelRef,
    pub bullet: ModelRef,
    pub sphere: ModelRef,
}

/// Spits a web bullet from `mouth`, ahead of a spider facing `yaw`.
///
/// Port of `SpiderShootWeb` (original/src/Enemies/Enemy_Spider.c).
pub fn shoot_web(
    commands: &mut Commands,
    models: &mut ModelSpawner,
    model: ModelRef,
    mouth: Vec3,
    yaw: f32,
) {
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let mut bullet = WebBullet::new(yaw + PI);
    let root = commands
        .spawn((
            Name::new("Web bullet"),
            Transform::from_translation(mouth),
            Visibility::default(),
            TransformInterpolation,
            PreviousPosition(mouth),
            Velocity(forward * WEB_SPEED),
            Trigger {
                sides: SolidSides::ALL,
                solid: false,
            },
            solid_object(
                vec![WEB_BULLET_BOX],
                [
                    CollisionKind::Trigger,
                    CollisionKind::PlayerTriggerOnly,
                    CollisionKind::AutoTarget,
                ],
                SolidSides::ALL,
            ),
            DespawnOutOfRange,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    bullet.model = models.spawn(
        commands,
        root,
        model,
        Shading::Unlit,
        bullet.model_transform(),
    );
    if let Some(model) = bullet.model {
        commands.entity(model).insert(WebGlow { opacity: 1.0 });
    }
    commands.entity(root).insert(bullet);
}

/// Flies the web bullets, growing, spinning and fading, until they burn
/// out. Leaving the item window (`TrackTerrainItem`) is
/// [`DespawnOutOfRange`].
///
/// Port of `MoveWebBullet` (original/src/Enemies/Enemy_Spider.c).
pub(super) fn move_web_bullets(
    time: Res<Time>,
    mut commands: Commands,
    mut bullets: Query<(Entity, &mut WebBullet, &mut Transform, &Velocity)>,
    mut models: Query<(&mut Transform, &mut WebGlow), Without<WebBullet>>,
) {
    let dt = time.delta_secs();
    for (entity, mut bullet, mut transform, velocity) in &mut bullets {
        if !bullet.age(dt) {
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation += **velocity * dt;
        if let Some(Ok((mut model, mut glow))) = bullet.model.map(|m| models.get_mut(m)) {
            *model = bullet.model_transform();
            let opacity = bullet.opacity();
            if glow.opacity != opacity {
                glow.opacity = opacity;
            }
        }
    }
}

/// The player that touches a web bullet is caught in the web, unless it
/// is caught already: the bullet goes, and a ball turns into the bug.
///
/// Port of `DoTrig_WebBullet` (original/src/Enemies/Enemy_Spider.c). The
/// hold itself is applied by the player's plugin. A dying player isn't
/// held (the original's dead bug has no collision to set the bullet off),
/// so it gets no sphere either.
#[allow(clippy::type_complexity)]
pub(super) fn web_bullet_hits(
    mut commands: Commands,
    mut models: ModelSpawner,
    level: Res<crate::level::CurrentLevel>,
    mut hits: MessageReader<TriggerHit>,
    bullets: Query<(), With<WebBullet>>,
    players: Query<
        (&Transform, &PlayerForm, Option<&BugState>, Has<Dying>),
        (With<Player>, Without<WebBullet>),
    >,
    mut holds: MessageWriter<HoldPlayer>,
) {
    let mut webbed = EntityHashSet::default();
    for hit in hits.read() {
        if !bullets.contains(hit.trigger) {
            continue;
        }
        commands.entity(hit.trigger).try_despawn();
        let Ok((transform, form, state, dying)) = players.get(hit.mover) else {
            continue;
        };
        if dying || is_webbed(*form, state.copied()) || !webbed.insert(hit.mover) {
            continue;
        }
        holds.write(HoldPlayer {
            player: hit.mover,
            hold: Hold::Webbed,
        });
        let Some(model) = super::web_models(level.def().level_type).map(|m| m.sphere) else {
            continue;
        };
        let mut sphere = WebSphere::new(hit.mover);
        let root = commands
            .spawn((
                Name::new("Web sphere"),
                Transform::from_translation(transform.translation),
                Visibility::default(),
                TransformInterpolation,
                DespawnOnExit(AppState::InGame),
            ))
            .id();
        sphere.model = models.spawn(
            &mut commands,
            root,
            model,
            Shading::Unlit,
            Transform::from_scale(sphere.scale()),
        );
        if let Some(model) = sphere.model {
            commands.entity(model).insert(WebGlow { opacity: 1.0 });
        }
        commands.entity(root).insert(sphere);
    }
}

/// Whether the player is the bug caught in a web (`PLAYER_ANIM_WEBBED`).
pub fn is_webbed(form: PlayerForm, state: Option<BugState>) -> bool {
    form == PlayerForm::Bug && state == Some(BugState::Webbed)
}

/// What a web sphere does this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SphereOutcome {
    /// It holds on.
    Holds,
    /// The player got out some other way (rolled into the ball, or is no
    /// longer webbed).
    PlayerGotOut,
    /// Its time is up: it lets the player go.
    TimedOut,
}

/// Port of the checks in `MoveWebSphere`, after its time has gone down.
fn sphere_outcome(player: Option<(PlayerForm, Option<BugState>)>, life: f32) -> SphereOutcome {
    match player {
        Some((form, state)) if is_webbed(form, state) => {
            if life <= 0.0 {
                SphereOutcome::TimedOut
            } else {
                SphereOutcome::Holds
            }
        }
        _ => SphereOutcome::PlayerGotOut,
    }
}

/// Keeps each web sphere on its player, wobbling, until the player gets
/// out or its time is up, when it lets the player stand.
///
/// Port of `MoveWebSphere` (original/src/Enemies/Enemy_Spider.c). It runs
/// once the holds have been applied, so that a new sphere finds its player
/// webbed.
pub(super) fn move_web_spheres(
    time: Res<Time>,
    mut commands: Commands,
    mut spheres: Query<(Entity, &mut WebSphere, &mut Transform)>,
    players: Query<
        (&Transform, &PlayerForm, Option<&BugState>),
        (With<Player>, Without<WebSphere>),
    >,
    mut models: Query<&mut Transform, (Without<WebSphere>, Without<Player>)>,
    mut releases: MessageWriter<ReleasePlayer>,
) {
    let dt = time.delta_secs();
    for (entity, mut sphere, mut transform) in &mut spheres {
        sphere.life -= dt;
        let player = players.get(sphere.player).ok();
        match sphere_outcome(player.map(|(_, f, s)| (*f, s.copied())), sphere.life) {
            SphereOutcome::Holds => {}
            SphereOutcome::TimedOut => {
                releases.write(ReleasePlayer {
                    player: sphere.player,
                    restore_collision: false,
                });
                commands.entity(entity).despawn();
                continue;
            }
            SphereOutcome::PlayerGotOut => {
                commands.entity(entity).despawn();
                continue;
            }
        }
        if let Some((player_transform, ..)) = player {
            transform.translation = player_transform.translation;
        }
        sphere.wobble += SPHERE_WOBBLE_RATE * dt;
        if let Some(Ok(mut model)) = sphere.model.map(|m| models.get_mut(m)) {
            model.scale = sphere.scale();
        }
    }
}

/// Applies each [`WebGlow`] to the meshes below its entity.
pub(super) fn apply_web_glow(
    mut commands: Commands,
    glows: Query<(Entity, &WebGlow)>,
    children: Query<&Children>,
    mut meshes: Query<(
        &mut MeshMaterial3d<ObjectMaterial>,
        Option<&mut OwnWebMaterial>,
    )>,
    mut materials: ResMut<Assets<ObjectMaterial>>,
) {
    for (root, glow) in &glows {
        for entity in children.iter_descendants(root) {
            let Ok((mut handle, own)) = meshes.get_mut(entity) else {
                continue;
            };
            match own {
                Some(mut own) => {
                    if own.applied == glow.opacity {
                        continue;
                    }
                    own.applied = glow.opacity;
                    if let Some(mut material) = materials.get_mut(&handle.0) {
                        material.base.base_color.set_alpha(own.alpha * glow.opacity);
                    }
                }
                None => {
                    let Some(mut material) = materials.get(&handle.0).cloned() else {
                        continue;
                    };
                    let base = &mut material.base;
                    let alpha = base.base_color.alpha();
                    base.base_color.set_alpha(alpha * glow.opacity);
                    base.alpha_mode = AlphaMode::Add;
                    base.cull_mode = None;
                    base.double_sided = true;
                    handle.0 = materials.add(material);
                    commands.entity(entity).insert(OwnWebMaterial {
                        alpha,
                        applied: glow.opacity,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_bullet_grows_spins_and_fades_before_burning_out() {
        let mut bullet = WebBullet::new(0.5);
        assert_eq!(bullet.opacity(), 1.0);
        let dt = 0.1;
        for _ in 0..7 {
            assert!(bullet.age(dt));
        }
        assert!((bullet.scale - (WEB_BULLET_START_SCALE + 0.35)).abs() < 1e-4);
        assert!((bullet.spin - 1.4).abs() < 1e-4);
        // A third of a second left: still fully opaque; then it fades.
        assert!((bullet.opacity() - 0.9).abs() < 1e-4);
        assert!(bullet.age(dt));
        assert!(bullet.opacity() < 0.7);
        assert!(bullet.age(dt));
        assert!(!bullet.age(dt + 0.01));
    }

    #[test]
    fn a_web_bullet_faces_back_at_its_spider() {
        let bullet = WebBullet::new(PI);
        let forward = bullet.model_transform().rotation * Vec3::NEG_Z;
        // Turned half round from a spider facing -Z (a heading of 0).
        assert!(forward.abs_diff_eq(Vec3::Z, 1e-5), "{forward}");
    }

    #[test]
    fn only_a_webbed_bug_counts_as_webbed() {
        assert!(is_webbed(PlayerForm::Bug, Some(BugState::Webbed)));
        assert!(!is_webbed(PlayerForm::Bug, Some(BugState::Stand)));
        assert!(!is_webbed(PlayerForm::Ball, Some(BugState::Webbed)));
        assert!(!is_webbed(PlayerForm::Bug, None));
    }

    #[test]
    fn the_sphere_holds_until_its_time_is_up_or_the_player_gets_out() {
        let webbed = Some((PlayerForm::Bug, Some(BugState::Webbed)));
        assert_eq!(sphere_outcome(webbed, 1.0), SphereOutcome::Holds);
        assert_eq!(sphere_outcome(webbed, 0.0), SphereOutcome::TimedOut);
        let ball = Some((PlayerForm::Ball, Some(BugState::Webbed)));
        assert_eq!(sphere_outcome(ball, 1.0), SphereOutcome::PlayerGotOut);
        let knocked = Some((PlayerForm::Bug, Some(BugState::KnockedOnButt)));
        assert_eq!(sphere_outcome(knocked, 0.0), SphereOutcome::PlayerGotOut);
        assert_eq!(sphere_outcome(None, 1.0), SphereOutcome::PlayerGotOut);
    }

    #[test]
    fn the_sphere_wobbles_around_its_size() {
        let mut sphere = WebSphere::new(Entity::PLACEHOLDER);
        // y starts at the top of its cosine.
        assert!(sphere.scale().abs_diff_eq(Vec3::new(1.2, 1.3, 1.2), 1e-5));
        sphere.wobble = Vec3::splat(PI / 2.0);
        assert!(sphere.scale().abs_diff_eq(Vec3::new(1.3, 1.2, 1.3), 1e-5));
    }

    #[test]
    fn the_sphere_lets_go_when_its_time_is_up() {
        use bevy::ecs::system::RunSystemOnce;
        use std::time::Duration;

        let mut world = World::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(1.0));
        world.insert_resource(time);
        world.init_resource::<Messages<ReleasePlayer>>();
        let player = world
            .spawn((
                Player,
                Transform::from_xyz(10.0, 20.0, 30.0),
                PlayerForm::Bug,
                BugState::Webbed,
            ))
            .id();
        let sphere = world
            .spawn((WebSphere::new(player), Transform::default()))
            .id();
        let run = |world: &mut World| {
            world
                .run_system_once(move_web_spheres)
                .expect("the system runs");
        };
        run(&mut world);
        assert_eq!(
            world.get::<Transform>(sphere).map(|t| t.translation),
            Some(Vec3::new(10.0, 20.0, 30.0))
        );
        run(&mut world);
        assert!(world.get_entity(sphere).is_ok());
        run(&mut world);
        assert!(world.get_entity(sphere).is_err());
        let releases: Vec<_> = world
            .resource_mut::<Messages<ReleasePlayer>>()
            .drain()
            .collect();
        assert_eq!(
            releases,
            [ReleasePlayer {
                player,
                restore_collision: false
            }]
        );
    }
}
