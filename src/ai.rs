//! Enemy AI — deterministic wander + player chase over the carved terrain.
//!
//! Creatures carry [`EnemyCreature`] (authored stats + current state); the
//! patrol bookkeeping lives in [`WanderState`], auto-inserted by [`enemy_ai`]
//! on the first tick. Heights come from the rendered-surface sampler
//! ([`TerrainRuntime::sample_mesh_surface`]) every tick (no physics bodies)
//! and facing reuses the player's
//! +Z-forward convention via [`crate::player::facing_rotation`].
//!
//! Respawn é regra do JOGO: quem quiser trazer uma criatura de volta spawna-a
//! com `viber.spawn_prototype` ou pelo spawner. A engine não mantém fila de
//! respawn nem política de distância ao player.
//!
//! Wiring: `app.add_plugins(crate::ai::AiPlugin);` (see [`AiPlugin`]).

#[cfg(test)]
use std::sync::Arc;

use bevy::math::Vec3;
use bevy::prelude::*;

use crate::profiler::{Group, timed};
use crate::terrain::runtime::TerrainRuntime;

/// Wander speed as a fraction of the authored chase speed.
pub const WANDER_SPEED_FRACTION: f32 = 0.4;
/// Chase keeps aggro until this multiple of `aggro_radius` (hysteresis band).
pub const DEAGGRO_HYSTERESIS: f32 = 1.8;
/// Seconds a wanderer keeps the same patrol target before picking a new one.
pub const WANDER_RETARGET_SECS: f32 = 4.0;
/// Patrol targets are picked inside this radius around `home`.
pub const WANDER_RADIUS: f32 = 8.0;
/// "Arrived" distance for a patrol target (triggers an early retarget).
pub const WANDER_ARRIVE_DIST: f32 = 0.5;

/// Below this planar speed (m/s) a character counts as standing still, and an
/// explicit look-at (`viber.face_player`) is allowed to own the facing.
///
/// Above it the body always points where it is *going*. This one rule is what
/// kills the "anda de lado": the scripts call `face_player()` and
/// `move_towards(patrol_point)` in the same frame, and the look-at used to win
/// unconditionally — body aimed at the hero, translation going sideways.
pub const FACING_MOVE_SPEED: f32 = 0.25;

/// Velocity below which a decelerating character is simply stopped, so it does
/// not creep forever on a vanishing remainder.
pub const STOP_EPSILON: f32 = 0.02;

/// Velocity-space locomotion shared by the Rust FSM ([`enemy_ai`]) and the
/// Luau scripts (`viber.move_towards` / `move_by` / `face_towards`).
///
/// Producers state a *desire* for the frame ([`Self::drive`], [`Self::look_at`])
/// and [`apply_ai_locomotion`] is the single consumer that turns it into
/// movement: it ramps the velocity toward the desire at the profile's
/// acceleration, steps the transform, snaps Y to the carved surface and turns
/// the yaw at the profile's turn rate. Before this existed both producers wrote
/// `Transform::translation` and `Transform::rotation` outright — full speed on
/// the first frame and a hard 180° snap on every retarget.
///
/// The desire is a *one-frame* contract: the consumer clears it. A producer
/// that stops asking decelerates to a stop instead of freezing mid-stride.
#[derive(Debug, Clone, Copy, Component, Default)]
pub struct AiLocomotion {
    /// Current planar velocity (m/s). Integrated, never assigned by producers.
    pub velocity: Vec2,
    /// Desired planar velocity (m/s) for this frame.
    desired: Vec2,
    /// A producer asked for something this frame (distinguishes "stand still"
    /// from "nobody is driving me").
    driven: bool,
    /// One-frame look-at direction (XZ), honoured only below
    /// [`FACING_MOVE_SPEED`].
    facing: Option<Vec2>,
    /// Where the producer is actually trying to GO, when it knows.
    ///
    /// The desired velocity alone is not enough for a pathfinder: projecting it
    /// forward only ever names a point on the straight line the character
    /// cannot take, which is precisely the line the wall is on. `viber.move_towards`
    /// and the FSM both know their destination, so they say it, and the
    /// navigation bridge paths to the real thing.
    goal: Option<Vec2>,
}

impl AiLocomotion {
    /// Asks for a planar velocity (m/s) this frame, with no stated destination.
    ///
    /// Use [`Self::drive_to`] whenever the destination IS known: without it the
    /// navigation bridge can only guess where the character is heading.
    pub fn drive(&mut self, desired: Vec2) {
        self.desired = desired;
        self.driven = true;
        self.goal = None;
    }

    /// Asks to travel toward `goal` at `speed` (m/s), from `here`.
    ///
    /// Never asks for more speed than closes the remaining gap this frame, so
    /// arrival is the same as it always was.
    pub fn drive_to(&mut self, here: Vec2, goal: Vec2, speed: f32, dt: f32) {
        let offset = goal - here;
        if offset.length_squared() <= 1e-8 || speed <= 0.0 {
            self.halt();
            self.goal = Some(goal);
            return;
        }
        let reach = offset.length() / dt.max(1e-4);
        self.desired = offset.normalize() * speed.min(reach);
        self.driven = true;
        self.goal = Some(goal);
    }

    /// Asks to stand still this frame (decelerating, not freezing).
    pub fn halt(&mut self) {
        self.desired = Vec2::ZERO;
        self.driven = true;
        self.goal = None;
    }

    /// Asks to look toward a direction, if the character is slow enough to be
    /// allowed to (see [`FACING_MOVE_SPEED`]).
    pub fn look_at(&mut self, dir: Vec2) {
        if dir.length_squared() > 1e-8 {
            self.facing = Some(dir.normalize());
        }
    }

    /// Current planar speed (m/s).
    pub fn speed(&self) -> f32 {
        self.velocity.length()
    }

    /// The velocity asked for this frame, before the acceleration ramp.
    ///
    /// The navigation bridge reads it to know where the character *wants* to
    /// go, replaces it with a navigated answer, and hands it back — which is
    /// why the scripts never learn that pathfinding exists.
    pub fn desired(&self) -> Vec2 {
        self.desired
    }

    /// A producer asked for something this frame.
    pub fn is_driven(&self) -> bool {
        self.driven
    }

    /// Where the producer is trying to go, when it said so.
    pub fn goal(&self) -> Option<Vec2> {
        self.goal
    }
}

/// Behaviour state of one [`EnemyCreature`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnemyState {
    /// Patrol around `home`; the [`Default`].
    #[default]
    Wander,
    /// Viu ou ouviu: vira-se para o ponto, ainda sem correr.
    Alert,
    /// Run at the player while it can see them.
    Chase,
    /// Perdeu o alvo: vai ao último ponto e desiste.
    Search,
}

impl EnemyState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wander => "wander",
            Self::Alert => "alert",
            Self::Chase => "chase",
            Self::Search => "search",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "wander" => Some(Self::Wander),
            "alert" => Some(Self::Alert),
            "chase" => Some(Self::Chase),
            "search" => Some(Self::Search),
            _ => None,
        }
    }
}

/// A spawned creature: authored stats plus its current behaviour state.
///
/// `Default` is Wander with `home` unset — [`enemy_ai`] latches the real home
/// to the spawn position on the first tick.
#[derive(Debug, Clone, Component)]
pub struct EnemyCreature {
    /// Chase speed in m/s (wander runs at [`WANDER_SPEED_FRACTION`] × this).
    pub speed: f32,
    /// Player distance that flips Wander → Chase.
    pub aggro_radius: f32,
    /// Chase stops (attack range) once this close to the player.
    pub attack_radius: f32,
    /// Patrol anchor (XZ); `None` until [`enemy_ai`] latches it to the spawn
    /// position on the first tick — um sentinela `Vec2::ZERO` re-latchava o
    /// home a cada tick em criaturas spawnadas em (0,0) (patrulha derivava).
    pub home: Option<Vec2>,
    pub state: EnemyState,
}

impl Default for EnemyCreature {
    fn default() -> Self {
        Self {
            speed: 2.0,
            aggro_radius: 18.0,
            attack_radius: 2.2,
            home: None,
            state: EnemyState::Wander,
        }
    }
}

/// Patrol bookkeeping for one wanderer, auto-inserted by [`enemy_ai`].
#[derive(Debug, Clone, Component, Default)]
pub struct WanderState {
    /// Current patrol target (XZ).
    pub target: Vec2,
    /// `Time.elapsed_secs()` when a new target must be picked.
    pub next_pick_at: f32,
    /// Retarget count — feeds the deterministic per-pick seed.
    pub picks: u64,
}

/// Deterministic seed for one retarget of one entity: a hash of the entity
/// index and the pick counter (which advances with arrival/time triggers).
pub fn enemy_seed(entity_index: u32, picks: u64) -> u64 {
    (entity_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ picks.wrapping_mul(0xBF58_476D_1CE4_E5B9)
}

/// Patrol target: uniform point in the disc `home ± radius`, via the shared
/// SplitMix64 RNG (`crate::spawner::Rng`) — same seed, same target.
pub fn wander_target(home: Vec2, radius: f32, rng_seed: u64) -> Vec2 {
    let mut rng = crate::spawner::Rng::new(rng_seed);
    home + rng.unit_disc() * radius
}

/// State transition with the deaggro hysteresis band: Wander → Chase inside
/// `aggro`, Chase → Wander only past `deaggro` (> `aggro`, typically
/// `aggro × [`DEAGGRO_HYSTERESIS`]`); in between the state is sticky.
pub fn enemy_next_state(dist: f32, state: EnemyState, aggro: f32, deaggro: f32) -> EnemyState {
    match state {
        EnemyState::Wander if dist < aggro => EnemyState::Chase,
        EnemyState::Chase if dist > deaggro => EnemyState::Wander,
        other => other,
    }
}

/// Clamp an XZ position to the world disc of radius `limit` (Y passes
/// through — heights come from the terrain sampler, not the border).
pub fn clamp_to_world(pos: Vec3, limit: f32) -> Vec3 {
    let dist_sq = pos.x * pos.x + pos.z * pos.z;
    if dist_sq > limit * limit {
        let scale = limit / dist_sq.sqrt();
        Vec3::new(pos.x * scale, pos.y, pos.z * scale)
    } else {
        pos
    }
}

/// Criaturas sem script: visão, audição e a FSM Alert/Chase/Search.
/// Sem estímulo ficam no sítio (não há perseguição por distância plana).
pub fn enemy_ai(world: &mut World) {
    crate::senses::sense_ai(world);
}

/// System set of [`apply_ai_locomotion`], so anything that has to write the
/// locomotion ask (the navigation bridge) can order itself before the single
/// consumer without naming the function.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AiLocomotionSystems;

/// The single consumer of [`AiLocomotion`]: ramps velocity, steps the
/// transform, snaps Y to the carved surface and turns the yaw.
///
/// Runs in `PostUpdate` before `TransformSystems::Propagate`, so every producer
/// in `Update` (the FSM, the Luau command queue) has already had its say and
/// the movement lands in the same frame's `GlobalTransform`.
///
/// Three things it does that the old direct-write path did not:
///
/// * **Acceleration.** Velocity chases the desire at `profile.accel` instead of
///   jumping to it. The animation driver measures displacement through a
///   `SPEED_SMOOTH_TAU` low-pass; a step function into that filter is what made
///   the clip lag behind the body on every start and stop.
/// * **Turn rate.** Yaw slerps toward the target at `profile.turn_rate`
///   (reusing the hero's [`crate::player::facing_slerp_factor`]) instead of
///   snapping. A wander retarget no longer spins the creature in one frame.
/// * **Facing follows motion.** Above [`FACING_MOVE_SPEED`] the body points
///   along its own velocity; an explicit look-at only wins when the character
///   is effectively stopped.
#[allow(clippy::type_complexity)]
pub fn apply_ai_locomotion(
    time: Res<Time>,
    runtime: Option<Res<TerrainRuntime>>,
    interior: Option<Res<crate::worldsys::InteriorSceneConfig>>,
    mut movers: Query<
        (
            &mut Transform,
            &GlobalTransform,
            &mut AiLocomotion,
            Option<&crate::animation::LocomotionProfile>,
        ),
        Without<crate::corpse::Corpse>,
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let world_limit = runtime.as_deref().map(|rt| rt.spec.world_size * 0.5);
    for (mut transform, global, mut loco, profile) in &mut movers {
        let profile = profile.copied().unwrap_or_default();
        // O passo e o terreno são MUNDO; o `Transform` é LOCAL. `offset` é o
        // que o pai acrescenta — um NPC dentro do grupo `city` tem local y=0 e
        // world y≈38,7. Grupos de cena são translações puras; com um pai
        // rodado ou escalado isto seria uma aproximação (a mesma que o antigo
        // aplicador de `MoveBy` fazia).
        let offset = global.translation() - transform.translation;
        // Nobody drove this frame: coast to a stop rather than hold the last
        // velocity forever (a script whose `on_update` was skipped by the AI
        // LOD must not keep walking).
        if !loco.driven {
            loco.desired = Vec2::ZERO;
        }
        let delta = loco.desired - loco.velocity;
        let max_change = profile.accel * dt;
        loco.velocity += delta.clamp_length_max(max_change);
        if loco.desired == Vec2::ZERO && loco.velocity.length() < STOP_EPSILON {
            loco.velocity = Vec2::ZERO;
        }

        let step = loco.velocity * dt;
        if step.length_squared() > 1e-12 {
            let world = transform.translation + offset;
            let mut x = world.x + step.x;
            let mut z = world.z + step.y;
            // A bolsa de interior vive DECLARADAMENTE fora do disco do mundo
            // (ver `InteriorSceneConfig`): clampar ali teleportava os NPCs da
            // sala para a borda do vale — a mesma exceção (pela posição ATUAL)
            // que o knockback já faz. `limit > 0` cobre um `world-size`
            // degenerado, que colapsava todo o movimento na origem.
            let in_interior = interior
                .as_deref()
                .is_some_and(|scene| scene.contains(world.x, world.z));
            if let Some(limit) = world_limit.filter(|limit| !in_interior && *limit > 0.0) {
                let clamped = clamp_to_world(Vec3::new(x, world.y, z), limit);
                x = clamped.x;
                z = clamped.z;
            }
            // PISO SOB A CRIATURA (Y conhecido → surface_below): sob um
            // cliff/arco fica SOB a rocha em vez de ser teletransportada para o
            // topo do mundo. Sem terreno (testes headless sem bootstrap) o Y
            // fica como está. FORA do heightfield (interior, cena sem terreno)
            // os samplers saturam para a borda — não é o chão da criatura.
            let y = match runtime.as_deref() {
                Some(rt) if rt.in_field(x, z) => {
                    // `None` = não há piso sólido na coluna: o destino está
                    // enterrado em rocha que desce até ao fundo (falésia,
                    // encosta). O fallback antigo (`sample_mesh_surface`, o
                    // TOPO do mundo) teleportava a criatura para cima do
                    // penhasco a cada passo contra a parede. Sem collider
                    // próprio, recusa-se o passo — a criatura encosta, como
                    // o herói com o character controller.
                    rt.surface_below(x, z, world.y + crate::player::GROUND_PROBE)
                }
                _ => Some(world.y),
            };
            if let Some(y) = y {
                transform.translation = Vec3::new(x, y, z) - offset;
            }
        }

        let target_dir = if loco.speed() > FACING_MOVE_SPEED {
            Some(loco.velocity.normalize())
        } else {
            loco.facing
        };
        if let Some(dir) = target_dir {
            let target = crate::player::facing_rotation(Vec3::new(dir.x, 0.0, dir.y));
            let t = crate::player::facing_slerp_factor(
                transform.rotation,
                target,
                profile.turn_rate,
                dt,
            );
            transform.rotation = transform.rotation.slerp(target, t);
        }

        // One-frame contract: the desire has been consumed.
        loco.desired = Vec2::ZERO;
        loco.driven = false;
        loco.facing = None;
        loco.goal = None;
    }
}

/// HP default para as criaturas da FSM Rust (spawner dinâmico SEM script
/// Luau): nasciam sem `Health` e ficavam imunes ao strike/bomba — e o
/// respawn (abaixo) nunca disparava. Scriptadas recebem vitals via
/// `combat::ensure_creature_vitals` (hostis apenas).
#[allow(clippy::type_complexity)]
pub fn ensure_fsm_vitals(
    mut commands: Commands,
    creatures: Query<
        (Entity, &EnemyCreature),
        (
            Without<crate::vitals::Health>,
            Without<crate::player::Player>,
        ),
    >,
) {
    for (entity, _) in &creatures {
        commands
            .entity(entity)
            .insert(crate::vitals::Health::default());
    }
}

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        // Sem este plugin o `hear_combat` nunca corria (inimigos surdos) e
        // os `NoisePing` dos impactos ficavam para sempre no mundo.
        app.add_plugins(crate::senses::SensesPlugin);
        app.add_message::<crate::ambient::SfxEvent>()
            .add_systems(Update, (timed(Group::Ai, enemy_ai), ensure_fsm_vitals))
            // O consumidor único do [`AiLocomotion`] corre DEPOIS de todos os
            // produtores do `Update` (a FSM aqui, a fila de comandos Luau no
            // `LuauPlugin`) e ANTES da propagação de transforms, para o passo
            // do frame chegar ao `GlobalTransform` do mesmo frame.
            .add_systems(
                bevy::app::PostUpdate,
                timed(Group::Ai, apply_ai_locomotion)
                    .in_set(AiLocomotionSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal headless app with just the locomotion consumer and a hand-driven
    /// clock. No terrain: the integrator leaves Y alone, which is what we want
    /// when the subject is XZ velocity and yaw.
    fn locomotion_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build().disable::<bevy::time::TimePlugin>())
            .add_plugins(bevy::transform::TransformPlugin);
        app.init_resource::<Time>();
        app.add_systems(
            bevy::app::PostUpdate,
            apply_ai_locomotion.before(bevy::transform::TransformSystems::Propagate),
        );
        app
    }

    fn tick(app: &mut App, millis: u64) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(millis));
        app.update();
    }

    /// Yaw never moves faster than the profile allows. Before this, every
    /// wander retarget span the creature through 180° inside one frame.
    #[test]
    fn test_yaw_never_turns_faster_than_the_profile_allows() {
        let mut app = locomotion_app();
        let profile = crate::animation::LocomotionProfile::authored(1.5, 4.0);
        let mut loco = AiLocomotion::default();
        // Already at speed, heading -Z; the desire is a full 180° reversal.
        loco.velocity = Vec2::new(0.0, -4.0);
        let entity = app
            .world_mut()
            .spawn((Transform::from_xyz(0.0, 0.0, 0.0), loco, profile))
            .id();

        let dt = 0.05_f32;
        let mut previous = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .rotation;
        for _ in 0..40 {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .drive(Vec2::new(0.0, 4.0));
            tick(&mut app, (dt * 1000.0) as u64);
            let now = app
                .world()
                .entity(entity)
                .get::<Transform>()
                .unwrap()
                .rotation;
            let swept = previous.angle_between(now);
            assert!(
                swept <= profile.turn_rate * dt + 1e-3,
                "yaw swept {swept:.3} rad in one frame, cap is {:.3}",
                profile.turn_rate * dt
            );
            previous = now;
        }
        // And it does get there: a capped turn is still a turn.
        let facing = previous * Vec3::Z;
        assert!(
            facing.z > 0.9,
            "after 2 s the body faces +Z, got {facing:?}"
        );
    }

    /// Velocity ramps at the profile's acceleration instead of jumping to the
    /// desire on the first frame — the step function into the animation's
    /// own low-pass is what made clips lag the body on every start.
    #[test]
    fn test_velocity_ramps_instead_of_jumping_to_the_desire() {
        let mut app = locomotion_app();
        let profile = crate::animation::LocomotionProfile::authored(1.5, 4.0);
        let entity = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                AiLocomotion::default(),
                profile,
            ))
            .id();

        let dt = 0.05_f32;
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<AiLocomotion>()
            .unwrap()
            .drive(Vec2::new(4.0, 0.0));
        tick(&mut app, 50);
        let speed = app
            .world()
            .entity(entity)
            .get::<AiLocomotion>()
            .unwrap()
            .speed();
        assert!(
            (speed - profile.accel * dt).abs() < 1e-3,
            "first frame reaches accel·dt = {:.2} m/s, got {speed:.2}",
            profile.accel * dt
        );

        // Keep driving: it converges on the desire and stays there.
        for _ in 0..20 {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .drive(Vec2::new(4.0, 0.0));
            tick(&mut app, 50);
        }
        let speed = app
            .world()
            .entity(entity)
            .get::<AiLocomotion>()
            .unwrap()
            .speed();
        assert!(
            (speed - 4.0).abs() < 1e-3,
            "settles at 4 m/s, got {speed:.2}"
        );

        // Stop driving: it coasts down instead of freezing mid-stride.
        for _ in 0..20 {
            tick(&mut app, 50);
        }
        let speed = app
            .world()
            .entity(entity)
            .get::<AiLocomotion>()
            .unwrap()
            .speed();
        assert_eq!(speed, 0.0, "an undriven character comes to a stop");
    }

    /// The rule that kills "anda de lado": a look-at issued while the body is
    /// moving does not steal the facing. Stopped, it does.
    #[test]
    fn test_a_look_at_only_wins_when_the_body_is_stopped() {
        let mut app = locomotion_app();
        let profile = crate::animation::LocomotionProfile::authored(1.5, 4.0);
        let entity = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                AiLocomotion::default(),
                profile,
            ))
            .id();

        // Walking along +X while being told to look along +Z, every frame —
        // exactly what the scripts do with move_towards + face_player.
        for _ in 0..60 {
            let mut entity_mut = app.world_mut().entity_mut(entity);
            let mut loco = entity_mut.get_mut::<AiLocomotion>().unwrap();
            loco.drive(Vec2::new(3.0, 0.0));
            loco.look_at(Vec2::new(0.0, 1.0));
            drop(entity_mut);
            tick(&mut app, 50);
        }
        let facing = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .rotation
            * Vec3::Z;
        assert!(
            facing.x > 0.9,
            "a moving body faces where it goes (+X), got {facing:?}"
        );

        // Now stop and keep asking to look along +Z: the look-at takes over.
        for _ in 0..60 {
            app.world_mut()
                .entity_mut(entity)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .look_at(Vec2::new(0.0, 1.0));
            tick(&mut app, 50);
        }
        let facing = app
            .world()
            .entity(entity)
            .get::<Transform>()
            .unwrap()
            .rotation
            * Vec3::Z;
        assert!(
            facing.z > 0.9,
            "a stopped body honours the look-at (+Z), got {facing:?}"
        );
    }

    /// A producer that knows where it is going says so, and the stated goal is
    /// what the navigation bridge paths to. Without it the only thing a
    /// pathfinder could aim at is a point on the straight line — which is the
    /// line the wall is on.
    #[test]
    fn a_stated_destination_survives_into_the_goal() {
        let mut loco = AiLocomotion::default();
        assert_eq!(loco.goal(), None, "nada pedido, nada declarado");

        loco.drive_to(Vec2::ZERO, Vec2::new(30.0, 0.0), 4.0, 0.05);
        assert_eq!(loco.goal(), Some(Vec2::new(30.0, 0.0)));
        assert!(
            (loco.desired().length() - 4.0).abs() < 1e-4,
            "pede a velocidade pedida"
        );
        assert!(loco.desired().x > 0.0, "aponta ao destino");

        // Arrival: never overshoot, and the goal stays stated so the bridge
        // knows the character is still trying to get there.
        loco.drive_to(Vec2::new(29.95, 0.0), Vec2::new(30.0, 0.0), 4.0, 0.05);
        assert!(
            loco.desired().length() <= 4.0 + 1e-4,
            "a chegada nunca pede mais do que fecha o intervalo"
        );

        // A bare velocity clears it — `viber.move_by` has no destination to
        // state, and inventing one would path to a lie.
        loco.drive(Vec2::new(1.0, 0.0));
        assert_eq!(loco.goal(), None);
    }

    /// A child of a translated group moves in WORLD space and keeps its local
    /// transform consistent — the townsfolk live under `<Group name="city">`.
    #[test]
    fn test_a_parented_character_moves_in_world_space() {
        let mut app = locomotion_app();
        let parent = app
            .world_mut()
            .spawn(Transform::from_xyz(100.0, 5.0, -50.0))
            .id();
        let child = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.0, 0.0),
                AiLocomotion::default(),
                crate::animation::LocomotionProfile::authored(1.5, 4.0),
            ))
            .id();
        app.world_mut().entity_mut(parent).add_child(child);
        app.update(); // propagate once so the child has a GlobalTransform

        for _ in 0..40 {
            app.world_mut()
                .entity_mut(child)
                .get_mut::<AiLocomotion>()
                .unwrap()
                .drive(Vec2::new(2.0, 0.0));
            tick(&mut app, 50);
        }
        let local = app
            .world()
            .entity(child)
            .get::<Transform>()
            .unwrap()
            .translation;
        let world = app
            .world()
            .entity(child)
            .get::<GlobalTransform>()
            .unwrap()
            .translation();
        assert!(
            local.x > 1.0,
            "the child advanced, local x = {:.2}",
            local.x
        );
        assert!(
            (world.x - (100.0 + local.x)).abs() < 1e-3,
            "world x tracks the parent offset: {world:?} vs local {local:?}"
        );
    }

    use crate::terrain::heightmap::HeightMapU16;
    use crate::terrain::spec::TerrainSpec;

    #[test]
    fn test_default_enemy_creature_values() {
        let enemy = EnemyCreature::default();
        assert_eq!(enemy.speed, 2.0);
        assert_eq!(enemy.aggro_radius, 18.0);
        assert_eq!(enemy.attack_radius, 2.2);
        assert_eq!(enemy.home, None, "home latches to the spawn on first tick");
        assert_eq!(enemy.state, EnemyState::Wander);
    }

    #[test]
    fn test_wander_target_is_deterministic_and_bounded() {
        let home = Vec2::new(40.0, -12.0);
        let a = wander_target(home, WANDER_RADIUS, 7);
        let b = wander_target(home, WANDER_RADIUS, 7);
        assert_eq!(a, b, "same seed → same patrol point");
        let c = wander_target(home, WANDER_RADIUS, 8);
        assert_ne!(a, c, "different seed → different patrol point");
        for target in [a, b, c] {
            assert!(
                (target - home).length() <= WANDER_RADIUS + 1e-3,
                "target {target:?} outside the patrol disc around {home:?}"
            );
        }
    }

    #[test]
    fn test_enemy_next_state_hysteresis() {
        let aggro = 18.0;
        let deaggro = aggro * DEAGGRO_HYSTERESIS; // 32.4
        // Inside aggro: either state engages the chase.
        assert_eq!(
            enemy_next_state(5.0, EnemyState::Wander, aggro, deaggro),
            EnemyState::Chase
        );
        assert_eq!(
            enemy_next_state(5.0, EnemyState::Chase, aggro, deaggro),
            EnemyState::Chase
        );
        // Hysteresis band (aggro..deaggro): state is sticky.
        let mid = (aggro + deaggro) * 0.5;
        assert_eq!(
            enemy_next_state(mid, EnemyState::Wander, aggro, deaggro),
            EnemyState::Wander
        );
        assert_eq!(
            enemy_next_state(mid, EnemyState::Chase, aggro, deaggro),
            EnemyState::Chase
        );
        // Past deaggro the chase drops; wander never engages far away.
        assert_eq!(
            enemy_next_state(60.0, EnemyState::Chase, aggro, deaggro),
            EnemyState::Wander
        );
        assert_eq!(
            enemy_next_state(60.0, EnemyState::Wander, aggro, deaggro),
            EnemyState::Wander
        );
        // No player → infinite distance never aggros.
        assert_eq!(
            enemy_next_state(f32::INFINITY, EnemyState::Wander, aggro, deaggro),
            EnemyState::Wander
        );
    }

    #[test]
    fn test_clamp_to_world_scales_xz_and_keeps_y() {
        let inside = clamp_to_world(Vec3::new(5.0, 3.0, -7.0), 100.0);
        assert_eq!(
            inside,
            Vec3::new(5.0, 3.0, -7.0),
            "inside the disc: untouched"
        );
        let outside = clamp_to_world(Vec3::new(30.0, 2.0, 40.0), 50.0);
        assert_eq!(outside.y, 2.0, "height passes through");
        let clamped_xz = Vec2::new(outside.x, outside.z);
        assert!(
            (clamped_xz.length() - 50.0).abs() < 1e-4,
            "XZ rescaled onto the limit: {clamped_xz:?}"
        );
        assert!(
            Vec2::new(30.0, 40.0).angle_to(clamped_xz).abs() < 1e-4,
            "direction preserved"
        );
        let zeroed = clamp_to_world(Vec3::new(9.0, 1.0, 9.0), 0.0);
        assert_eq!(
            Vec2::new(zeroed.x, zeroed.z),
            Vec2::ZERO,
            "limit 0 zeroes XZ"
        );
    }

    #[test]
    fn test_enemy_seed_varies_per_entity_and_pick() {
        assert_eq!(enemy_seed(3, 2), enemy_seed(3, 2), "deterministic");
        assert_ne!(enemy_seed(3, 2), enemy_seed(4, 2));
        assert_ne!(enemy_seed(3, 2), enemy_seed(3, 3));
    }

    /// Headless end-to-end: a procedural carved world + [`AiPlugin`], one
    /// wanderer and one player inside the aggro radius — the enemy must
    /// engage the chase, close distance and sit on the terrain height.
    #[test]
    fn test_enemy_ai_chases_player_over_terrain_headless() {
        let mut app = App::new();
        // TimePlugin is disabled: it would overwrite the clock with real
        // (microscopic) deltas every frame; we advance `Time` by hand instead.
        app.add_plugins(MinimalPlugins.build().disable::<bevy::time::TimePlugin>())
            .add_plugins(bevy::transform::TransformPlugin)
            .add_plugins(AiPlugin);
        app.init_resource::<Time>();
        let spec = TerrainSpec {
            world_size: 128.0,
            max_height: 40.0,
            seed: 5,
            ..TerrainSpec::default()
        };
        let map = HeightMapU16::procedural(&spec, spec.resolution.max(1) as usize);
        let grid = crate::terrain::brush::BrushGrid::from_height_map(
            &map,
            spec.world_size,
            spec.max_height,
            spec.height_smoothing,
        )
        .expect("grid builds");
        let runtime = TerrainRuntime {
            spec,
            grid: Arc::new(grid),
            water: vec![],
            roads: vec![],
            pads: vec![],
            voxel: Arc::new(crate::terrain::voxel::VoxelField::default()),
            deltas: std::sync::Arc::new(crate::terrain::delta::DeltaGrid::default()),
            paint: std::sync::Arc::new(crate::terrain::paint::PaintGrid::default()),
            splat_tuning: Default::default(),
        };
        app.insert_resource(runtime);

        app.world_mut().spawn((
            crate::player::Player::default(),
            Transform::from_xyz(5.0, 0.0, 0.0),
        ));
        app.world_mut()
            .spawn((EnemyCreature::default(), Transform::from_xyz(0.0, 0.0, 0.0)));

        // 20 × 0.25 s = 5 simulated seconds — enough to close 2.8 m at 2 m/s.
        for _ in 0..20 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(250));
            app.update();
        }

        let world = app.world_mut();
        let mut q = world.query::<(Entity, &EnemyCreature, &Transform)>();
        let (entity, enemy, transform) = q
            .iter(world)
            .next()
            .expect("enemy alive with its components");
        assert_eq!(
            enemy.state,
            EnemyState::Wander,
            "sem script não há perseguição"
        );
        let pos = transform.translation;
        assert!(
            pos.xz().distance(Vec2::ZERO) < 0.05,
            "a criatura fica onde nasceu, agora {pos:?}"
        );
        assert!(world.get::<WanderState>(entity).is_none());
    }

    /// Same world, no player: the creature stays a wanderer, moves at the
    /// wander pace and never strays past its patrol radius.
    #[test]
    fn test_enemy_ai_wanders_without_a_player_headless() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins.build().disable::<bevy::time::TimePlugin>())
            .add_plugins(bevy::transform::TransformPlugin)
            .add_plugins(AiPlugin);
        app.init_resource::<Time>();
        let spec = TerrainSpec {
            world_size: 128.0,
            max_height: 40.0,
            seed: 5,
            ..TerrainSpec::default()
        };
        let map = HeightMapU16::procedural(&spec, spec.resolution.max(1) as usize);
        let grid = crate::terrain::brush::BrushGrid::from_height_map(
            &map,
            spec.world_size,
            spec.max_height,
            spec.height_smoothing,
        )
        .expect("grid builds");
        app.insert_resource(TerrainRuntime {
            spec,
            grid: Arc::new(grid),
            water: vec![],
            roads: vec![],
            pads: vec![],
            voxel: Arc::new(crate::terrain::voxel::VoxelField::default()),
            deltas: std::sync::Arc::new(crate::terrain::delta::DeltaGrid::default()),
            paint: std::sync::Arc::new(crate::terrain::paint::PaintGrid::default()),
            splat_tuning: Default::default(),
        });
        let home = Vec2::new(40.0, -30.0);
        // Enemy B patrols around an in-world home…
        app.world_mut().spawn((
            EnemyCreature {
                home: Some(home),
                ..EnemyCreature::default()
            },
            Transform::from_xyz(home.x, 0.0, home.y),
        ));
        // …enemy A spawns outside the world disc to exercise clamp_to_world:
        // it ends up on the border, which is by design farther than the
        // patrol radius from its home.
        let outside = Vec2::new(200.0, 200.0);
        app.world_mut().spawn((
            EnemyCreature {
                home: Some(outside),
                ..EnemyCreature::default()
            },
            Transform::from_xyz(outside.x, 0.0, outside.y),
        ));

        // 30 × 0.25 s = 7.5 simulated seconds of patrol (two retargets).
        for _ in 0..30 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(250));
            app.update();
        }

        let world = app.world_mut();
        let mut q = world.query::<(&EnemyCreature, &Transform)>();
        let mut border = None;
        let mut patroller = None;
        for (enemy, transform) in q.iter(world) {
            if enemy.home == Some(outside) {
                border = Some((enemy, transform));
            } else {
                patroller = Some((enemy, transform));
            }
        }
        let (border_enemy, border_transform) = border.expect("border enemy alive");
        let (patrol_enemy, patrol_transform) = patroller.expect("patrolling enemy alive");
        assert_eq!(
            border_enemy.state,
            EnemyState::Wander,
            "no player → never chases"
        );
        assert_eq!(
            patrol_enemy.state,
            EnemyState::Wander,
            "no player → never chases"
        );

        assert_eq!(patrol_enemy.home, Some(home));
        let pos = Vec2::new(
            patrol_transform.translation.x,
            patrol_transform.translation.z,
        );
        assert!(
            pos.distance(home) < 0.05,
            "sem script não patrulha: {pos:?}"
        );
        let border_pos = Vec2::new(
            border_transform.translation.x,
            border_transform.translation.z,
        );
        assert!(
            border_pos.distance(outside) < 0.05,
            "sem script não é puxada para o disco: {border_pos:?}"
        );
    }
}
