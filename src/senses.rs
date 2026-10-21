//! Visão e audição das criaturas sem script.
//!
//! Não é uma behavior tree. Um barramento [`SenseBus`] recebe o que se ouve
//! (golpe, projétil, `viber.noise` / `viber.emit("noise")`). A visão é um
//! cone + um raycast, com orçamento por frame. A FSM nativa passa a
//! Wander → Alert → Chase → Search e volta ao wander. [`crate::ai::enemy_next_state`]
//! fica para quem não usa sentidos (os scripts).

use std::collections::HashMap;

use bevy::prelude::*;

use crate::ai::{AiLocomotion, EnemyCreature, EnemyState, WANDER_SPEED_FRACTION};

pub const SEE_RANGE: f32 = 18.0;
/// cos(60°): cone de visão para a frente.
pub const SEE_CONE_DOT: f32 = 0.5;
pub const HEAR_THRESHOLD: f32 = 1.0;
pub const HEAR_MELEE: f32 = 28.0;
pub const HEAR_SHOT: f32 = 36.0;
pub const ALERT_SECS: f32 = 0.4;
pub const SEARCH_SECS: f32 = 6.0;
pub const SENSE_RAYS_PER_FRAME: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SenseKind {
    See,
    Hear,
}

impl SenseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::See => "see",
            Self::Hear => "hear",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SenseEvent {
    pub kind: SenseKind,
    pub pos: Vec3,
    pub loudness: f32,
    pub from: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct SenseBus {
    pub events: Vec<SenseEvent>,
}

/// Último sentido por entidade, lido pelo Lua no mesmo frame (`viber.sense`, `on_sense`).
#[derive(Resource, Default)]
pub struct SenseInbox {
    pub pulses: HashMap<Entity, SenseKind>,
}

#[derive(Component, Debug, Clone)]
pub struct SenseMemory {
    pub alert_left: f32,
    pub search_left: f32,
    pub last_known: Vec2,
    pub saw: bool,
}

impl Default for SenseMemory {
    fn default() -> Self {
        Self {
            alert_left: 0.0,
            search_left: 0.0,
            last_known: Vec2::ZERO,
            saw: false,
        }
    }
}

pub fn hears(loudness: f32, dist: f32) -> bool {
    dist > 0.05 && loudness / dist >= HEAR_THRESHOLD
}

pub fn in_view(forward: Vec3, to_target: Vec3, range: f32) -> bool {
    let dist = to_target.length();
    if dist > range || dist < 1e-3 {
        return false;
    }
    let flat_f = Vec3::new(forward.x, 0.0, forward.z);
    let flat_t = Vec3::new(to_target.x, 0.0, to_target.z);
    if flat_f.length_squared() < 1e-6 || flat_t.length_squared() < 1e-6 {
        return false;
    }
    flat_f.normalize().dot(flat_t.normalize()) >= SEE_CONE_DOT
}

/// Passo da FSM com sentidos. Sem ver nem ouvir, Wander fica Wander
/// (não há perseguição por distância — isso é [`crate::ai::enemy_next_state`]).
pub fn sense_step(
    state: EnemyState,
    saw: bool,
    heard: bool,
    alert_left: f32,
    search_left: f32,
    dt: f32,
) -> (EnemyState, f32, f32) {
    match state {
        EnemyState::Wander if saw || heard => (EnemyState::Alert, ALERT_SECS, 0.0),
        EnemyState::Alert if saw => (EnemyState::Chase, 0.0, 0.0),
        EnemyState::Alert if alert_left <= dt => (EnemyState::Search, 0.0, SEARCH_SECS),
        EnemyState::Alert => (EnemyState::Alert, (alert_left - dt).max(0.0), 0.0),
        EnemyState::Chase if saw => (EnemyState::Chase, 0.0, 0.0),
        EnemyState::Chase => (EnemyState::Search, 0.0, SEARCH_SECS),
        EnemyState::Search if saw => (EnemyState::Chase, 0.0, 0.0),
        EnemyState::Search if search_left <= dt => (EnemyState::Wander, 0.0, 0.0),
        EnemyState::Search => (EnemyState::Search, 0.0, (search_left - dt).max(0.0)),
        EnemyState::Wander => (EnemyState::Wander, 0.0, 0.0),
    }
}

fn los_hit(world: &World, origin: Vec3, target: Vec3) -> Option<f32> {
    use bevy_rapier3d::prelude::{
        QueryFilter, RapierContextColliders, RapierContextSimulation, RapierQueryPipeline,
        RapierRigidBodySet,
    };
    use bevy_rapier3d::rapier::parry::query::DefaultQueryDispatcher;
    let dir = target - origin;
    let dist = dir.length();
    if dist < 1e-3 {
        return None;
    }
    let context = world
        .iter_entities()
        .find(|e| e.contains::<RapierContextSimulation>())
        .map(|e| e.id())?;
    let simulation = world.get::<RapierContextSimulation>(context)?;
    let colliders = world.get::<RapierContextColliders>(context)?;
    let bodies = world.get::<RapierRigidBodySet>(context)?;
    RapierQueryPipeline::new_scoped(
        &simulation.broad_phase,
        colliders,
        bodies,
        &QueryFilter::default(),
        &DefaultQueryDispatcher,
        |pipeline| pipeline.cast_ray(origin, dir / dist, dist, true),
    )
    .map(|(_, toi)| toi)
}

/// Som pontual (golpe, projétil, `viber.noise`) — o leitor corre antes da FSM.
#[derive(Message, Clone, Copy, Debug)]
pub struct HeardNoise {
    pub pos: Vec3,
    pub loudness: f32,
}

/// Impacto de projétil à espera de entrar no barramento (um frame).
#[derive(Component)]
pub struct NoisePing(pub Vec3);

/// Golpes e impactos entram no barramento como audição.
pub fn hear_combat(
    mut commands: Commands,
    mut bus: ResMut<SenseBus>,
    mut noises: MessageReader<HeardNoise>,
    hits: Query<&GlobalTransform, Added<crate::animation::HitReact>>,
    pings: Query<(Entity, &NoisePing)>,
) {
    for xf in &hits {
        bus.events.push(SenseEvent {
            kind: SenseKind::Hear,
            pos: xf.translation(),
            loudness: HEAR_MELEE,
            from: None,
        });
    }
    for (entity, ping) in &pings {
        bus.events.push(SenseEvent {
            kind: SenseKind::Hear,
            pos: ping.0,
            loudness: HEAR_SHOT,
            from: None,
        });
        commands.entity(entity).despawn();
    }
    for noise in noises.read() {
        bus.events.push(SenseEvent {
            kind: SenseKind::Hear,
            pos: noise.pos,
            loudness: noise.loudness,
            from: None,
        });
    }
}

/// FSM das criaturas sem script. Com linha de visão persegue; atrás de um
/// volume, não. Um som leva-a ao sítio e, se não vir ninguém, desiste.
pub fn sense_ai(world: &mut World) {
    if world.get_resource::<SenseBus>().is_none() {
        world.init_resource::<SenseBus>();
        world.init_resource::<SenseInbox>();
    }
    let dt = world.resource::<Time>().delta_secs().max(0.0);
    let events = std::mem::take(&mut world.resource_mut::<SenseBus>().events);
    let player = world
        .query_filtered::<&GlobalTransform, With<crate::player::Player>>()
        .iter(world)
        .next()
        .map(|g| g.translation());

    let mut rays = 0usize;
    let mut pending_loco: Vec<(Entity, Vec2, f32, Vec2)> = Vec::new();
    let mut pending_memory: Vec<Entity> = Vec::new();
    let mut pulses: Vec<(Entity, SenseKind)> = Vec::new();

    let enemies: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<EnemyCreature>,
            Without<crate::combat::Corpse>,
            Without<crate::luau::LuaScriptRef>,
            Without<crate::player::Player>,
        )>()
        .iter(world)
        .collect();

    for entity in enemies {
        if world.get::<SenseMemory>(entity).is_none() {
            pending_memory.push(entity);
            continue;
        }
        let Some(xf) = world.get::<GlobalTransform>(entity).map(|g| g.translation()) else {
            continue;
        };
        let rot = world
            .get::<GlobalTransform>(entity)
            .map(|g| g.rotation())
            .unwrap_or(Quat::IDENTITY);
        let forward = rot * Vec3::Z;
        let here = Vec2::new(xf.x, xf.z);

        let mut saw = false;
        let mut heard_pos: Option<Vec2> = None;
        if let Some(player) = player {
            let to = player - xf;
            if in_view(forward, to, SEE_RANGE) && rays < SENSE_RAYS_PER_FRAME {
                rays += 1;
                let origin = xf + Vec3::Y * 1.4;
                let aim = player + Vec3::Y * 1.0;
                let dist = origin.distance(aim);
                let blocked = los_hit(world, origin, aim)
                    .is_some_and(|toi| toi + crate::lockon::LOS_SLACK < dist);
                saw = !blocked;
            }
        }
        for event in &events {
            if event.kind != SenseKind::Hear {
                continue;
            }
            let dist = event.pos.distance(xf);
            if hears(event.loudness, dist) {
                heard_pos = Some(Vec2::new(event.pos.x, event.pos.z));
            }
        }

        let (state, alert_left, search_left, last_known, speed, home) = {
            let enemy = world.get::<EnemyCreature>(entity).unwrap();
            let memory = world.get::<SenseMemory>(entity).unwrap();
            let (state, alert_left, search_left) = sense_step(
                enemy.state,
                saw,
                heard_pos.is_some(),
                memory.alert_left,
                memory.search_left,
                dt,
            );
            let last = if saw {
                player.map(|p| Vec2::new(p.x, p.z)).unwrap_or(memory.last_known)
            } else if let Some(p) = heard_pos {
                p
            } else {
                memory.last_known
            };
            (state, alert_left, search_left, last, enemy.speed, enemy.home)
        };

        if let Some(mut enemy) = world.get_mut::<EnemyCreature>(entity) {
            enemy.state = state;
            if enemy.home.is_none() {
                enemy.home = Some(here);
            }
        }
        if let Some(mut memory) = world.get_mut::<SenseMemory>(entity) {
            memory.alert_left = alert_left;
            memory.search_left = search_left;
            memory.last_known = last_known;
            memory.saw = saw;
        }
        if saw {
            pulses.push((entity, SenseKind::See));
        } else if heard_pos.is_some() {
            pulses.push((entity, SenseKind::Hear));
        }

        let goal = match state {
            EnemyState::Chase => player.map(|p| Vec2::new(p.x, p.z)),
            EnemyState::Search => Some(last_known),
            EnemyState::Alert => None,
            EnemyState::Wander => home,
        };
        if let Some(goal) = goal {
            let spd = match state {
                EnemyState::Chase => speed,
                EnemyState::Search => speed * 0.7,
                EnemyState::Wander => speed * WANDER_SPEED_FRACTION,
                EnemyState::Alert => 0.0,
            };
            if state != EnemyState::Wander {
                pending_loco.push((entity, here, spd, goal));
            }
        }
        if state == EnemyState::Alert {
            let look = last_known - here;
            if let Some(mut loco) = world.get_mut::<AiLocomotion>(entity) {
                loco.halt();
                loco.look_at(look);
            } else {
                pending_loco.push((entity, here, 0.0, last_known));
            }
        }
    }

    for entity in pending_memory {
        world.entity_mut(entity).insert(SenseMemory::default());
    }
    for (entity, here, speed, goal) in pending_loco {
        if world.get::<AiLocomotion>(entity).is_none() {
            world.entity_mut(entity).insert(AiLocomotion::default());
        }
        if let Some(mut loco) = world.get_mut::<AiLocomotion>(entity) {
            if speed <= 0.0 {
                loco.halt();
                loco.look_at(goal - here);
            } else {
                loco.drive_to(here, goal, speed, dt.max(1e-3));
            }
        }
    }
    let mut inbox = world.resource_mut::<SenseInbox>();
    inbox.pulses.clear();
    for (entity, kind) in pulses {
        inbox.pulses.insert(entity, kind);
    }
}

pub struct SensesPlugin;

impl Plugin for SensesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SenseBus>()
            .init_resource::<SenseInbox>()
            .add_message::<HeardNoise>()
            .add_systems(Update, hear_combat.before(crate::ai::enemy_ai));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_of_distance_blocks_hearing_but_a_blow_at_twenty_metres_does_not() {
        assert!(!hears(HEAR_MELEE, 40.0));
        assert!(hears(HEAR_MELEE, 20.0));
    }

    #[test]
    fn vision_is_a_forward_cone() {
        let forward = Vec3::Z;
        assert!(in_view(forward, Vec3::new(0.0, 0.0, 8.0), SEE_RANGE));
        assert!(!in_view(forward, Vec3::new(8.0, 0.0, 0.0), SEE_RANGE));
        assert!(!in_view(forward, Vec3::new(0.0, 0.0, 40.0), SEE_RANGE));
    }

    #[test]
    fn lost_sight_goes_to_search_then_wander() {
        let (state, _, search) = sense_step(EnemyState::Chase, false, false, 0.0, 0.0, 0.1);
        assert_eq!(state, EnemyState::Search);
        assert!(search > 1.0);
        let (state, _, _) = sense_step(EnemyState::Search, false, false, 0.0, 0.05, 0.1);
        assert_eq!(state, EnemyState::Wander);
    }

    #[test]
    fn seeing_from_wander_alerts_before_the_chase() {
        let (state, alert, _) = sense_step(EnemyState::Wander, true, false, 0.0, 0.0, 0.016);
        assert_eq!(state, EnemyState::Alert);
        assert!(alert > 0.0);
        let (state, _, _) = sense_step(EnemyState::Alert, true, false, alert, 0.0, 0.016);
        assert_eq!(state, EnemyState::Chase);
    }
}
