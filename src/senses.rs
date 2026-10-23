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
/// Folga do raio de LOS (m): evita "bloqueado" por encostar à parede.
pub const LOS_SLACK: f32 = 0.45;

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

/// Um som é ouvido se a intensidade na distância chegar ao limiar.
///
/// Distância zero é o caso MAIS alto, não um caso inválido: a guarda antiga
/// (`dist > 0.05`) fazia a criatura ignorar um ruído exatamente em cima dela —
/// o impacto de uma bomba nos próprios pés, ou o `HitReact` dela própria, que
/// o `hear_combat` publica na posição da vítima. O piso de 5 cm é só para não
/// dividir por zero; não finito não é audível.
pub fn hears(loudness: f32, dist: f32) -> bool {
    loudness.is_finite()
        && dist.is_finite()
        && dist >= 0.0
        && loudness / dist.max(0.05) >= HEAR_THRESHOLD
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
        // Um som NOVO renova a busca: sem isto, um ruído ouvido no frame em
        // que o timer expirava era ignorado (o `last_known` era atualizado
        // mas a criatura desistia no frame seguinte).
        EnemyState::Search if heard => (EnemyState::Search, 0.0, SEARCH_SECS),
        EnemyState::Search if search_left <= dt => (EnemyState::Wander, 0.0, 0.0),
        EnemyState::Search => (EnemyState::Search, 0.0, (search_left - dt).max(0.0)),
        EnemyState::Wander => (EnemyState::Wander, 0.0, 0.0),
    }
}

fn los_hit(world: &World, origin: Vec3, target: Vec3) -> Option<f32> {
    use bevy_rapier3d::prelude::{
        QueryFilter, QueryFilterFlags, RapierContextColliders, RapierContextSimulation,
        RapierQueryPipeline, RapierRigidBodySet,
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
    // Sensores (volumes de loja/porta/água) não são geometria: um deles
    // entre a criatura e o herói cegava-a por dentro de uma zona invisível.
    // O character controller também os exclui.
    let filter = QueryFilter {
        flags: QueryFilterFlags::EXCLUDE_SENSORS,
        ..QueryFilter::default()
    };
    RapierQueryPipeline::new_scoped(
        &simulation.broad_phase,
        colliders,
        bodies,
        &filter,
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

/// Rotação do ponto de partida do orçamento de raios de visão.
///
/// Sem isto os primeiros [`SENSE_RAYS_PER_FRAME`] inimigos da query (ordem
/// estável) consumiam TODOS os raios em todos os frames: os restantes nunca
/// eram testados e ficavam cegos para sempre, mesmo de frente para o herói.
#[derive(Resource, Default)]
pub struct SenseCursor(pub usize);

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
    if world.get_resource::<SenseCursor>().is_none() {
        world.insert_resource(SenseCursor::default());
    }
    let dt = world.resource::<Time>().delta_secs().max(0.0);
    let events = std::mem::take(&mut world.resource_mut::<SenseBus>().events);
    // TODOS os heróis: com co-op, ver só o primeiro deixava o segundo
    // invisível (os inimigos perseguiam sempre o player 1).
    let players: Vec<Vec3> = world
        .query_filtered::<&GlobalTransform, With<crate::player::Player>>()
        .iter(world)
        .map(GlobalTransform::translation)
        .collect();

    let mut rays = 0usize;
    let mut pending_loco: Vec<(Entity, Vec2, f32, Vec2)> = Vec::new();
    let mut pending_memory: Vec<Entity> = Vec::new();
    let mut pulses: Vec<(Entity, SenseKind)> = Vec::new();

    let enemies: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<EnemyCreature>,
            Without<crate::corpse::Corpse>,
            Without<crate::luau::LuaScriptRef>,
            Without<crate::player::Player>,
        )>()
        .iter(world)
        .collect();

    // A janela de quem recebe raio roda um inimigo por frame; sem isto os
    // últimos da lista nunca eram testados (ver [`SenseCursor`]).
    let start = {
        let mut cursor = world.resource_mut::<SenseCursor>();
        let start = cursor.0;
        cursor.0 = cursor.0.wrapping_add(1);
        start
    };

    for offset in 0..enemies.len() {
        let entity = enemies[(start.wrapping_add(offset)) % enemies.len()];
        if world.get::<SenseMemory>(entity).is_none() {
            pending_memory.push(entity);
            continue;
        }
        let Some(xf) = world
            .get::<GlobalTransform>(entity)
            .map(|g| g.translation())
        else {
            continue;
        };
        let rot = world
            .get::<GlobalTransform>(entity)
            .map(|g| g.rotation())
            .unwrap_or(Quat::IDENTITY);
        let forward = rot * Vec3::Z;
        let here = Vec2::new(xf.x, xf.z);

        let mut saw = false;
        let mut seen_pos: Option<Vec3> = None;
        let mut heard_pos: Option<Vec2> = None;
        // Candidatos no cone/alcance, do mais perto ao mais longe: um segundo
        // player atrás de uma parede (ou do primeiro) ainda é visto se houver
        // orçamento de raios.
        let mut candidates: Vec<(f32, Vec3)> = players
            .iter()
            .filter(|player| in_view(forward, **player - xf, SEE_RANGE))
            .map(|player| (player.distance(xf), *player))
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, target) in candidates {
            if rays >= SENSE_RAYS_PER_FRAME {
                break;
            }
            rays += 1;
            let origin = xf + Vec3::Y * 1.4;
            let aim = target + Vec3::Y * 1.0;
            let dist = origin.distance(aim);
            let blocked = los_hit(world, origin, aim).is_some_and(|toi| toi + LOS_SLACK < dist);
            if !blocked {
                saw = true;
                seen_pos = Some(target);
                break;
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
            let last = if let Some(seen) = seen_pos {
                Vec2::new(seen.x, seen.z)
            } else if let Some(p) = heard_pos {
                p
            } else {
                memory.last_known
            };
            (
                state,
                alert_left,
                search_left,
                last,
                enemy.speed,
                enemy.home,
            )
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

        // Chase implica ter visto NESTE frame (`sense_step`); o fallback ao
        // primeiro herói só cobre um estado forçado por script/bridge.
        let goal = match state {
            EnemyState::Chase => seen_pos
                .map(|p| Vec2::new(p.x, p.z))
                .or_else(|| players.first().map(|p| Vec2::new(p.x, p.z))),
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
            .init_resource::<SenseCursor>()
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

    /// O ruído EM CIMA do ouvinte (bomba nos pés, o próprio HitReact) tem de
    /// ser ouvido — era o único ponto que a guarda `dist > 0.05` rejeitava.
    #[test]
    fn a_noise_at_the_listeners_feet_is_heard() {
        assert!(hears(HEAR_SHOT, 0.0));
        assert!(hears(HEAR_MELEE, 0.01));
        // Silêncio continua silêncio; lixo numérico não é som.
        assert!(!hears(0.0, 0.0));
        assert!(!hears(f32::NAN, 1.0));
        assert!(!hears(HEAR_SHOT, f32::NAN));
        assert!(!hears(HEAR_SHOT, -1.0));
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

    /// Um som NOVO renova a busca: no frame em que o timer ia expirar, o
    /// ruído tem de manter a criatura à procura, não mandá-la pastar.
    #[test]
    fn a_fresh_noise_extends_the_search() {
        let (state, _, search) = sense_step(EnemyState::Search, false, true, 0.0, 0.01, 0.1);
        assert_eq!(state, EnemyState::Search);
        assert!((search - SEARCH_SECS).abs() < 1e-5, "{search}");
        // Sem som, o mesmo timer expira.
        let (state, _, _) = sense_step(EnemyState::Search, false, false, 0.0, 0.01, 0.1);
        assert_eq!(state, EnemyState::Wander);
    }
}
