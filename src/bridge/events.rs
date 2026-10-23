//! Event log estruturado do bridge — o complemento do ring de logs
//! ([`super::logs`]): enquanto o logs ring captura LINHAS de tracing, este
//! captura EVENTOS DE JOGO com estrutura (`{seq, time, kind, ...}`), para um
//! agente perguntar "houve dano?", "a quest virou ready?" sem grepar texto.
//!
//! Alimentado por [`collect_bridge_events`] (PostUpdate do plugin do bridge)
//! sobre os messages da engine + um detector de mudanças no `QuestLog` e de
//! HP→0 (mortes não têm message própria — são componentes).
//!
//! Cliente: `viber.debug.events(since_seq)` (a REPL leva os últimos 256) ou
//! `viber debug events --since N`.

use std::collections::VecDeque;

use bevy::prelude::*;
use serde_json::{Value as Json, json};

/// Eventos retidos no ring (o mais novo no fim).
pub const EVENT_BUFFER_CAPACITY: usize = 1000;
/// Eventos que viajam no snapshot da REPL (`DebugView.events`) — a cauda
/// mais recente chega sempre; `since_seq` filtra o resto.
pub const SNAPSHOT_TAIL: usize = 256;
/// Máximo de eventos de dano por frame (o combate apanha dezenas; o ring
/// não pode ser dominado por números flutuantes).
const DAMAGE_PER_FRAME_CAP: usize = 32;

/// Um evento de jogo estruturado.
#[derive(Debug, Clone)]
pub struct BridgeEvent {
    /// Monotónico por engine — o cursor do cliente (`--since`).
    pub seq: u64,
    /// Unix time (s, fracionário).
    pub time: f64,
    /// "damage" | "death" | "ui" | "toast". As regras de jogo (quests, XP,
    /// level-up, travel) vivem no Lua do jogo e não têm message na engine —
    /// NENHUM evento desses tipos é emitido (o `kind` é `&'static str`; um
    /// valor novo exige um produtor em [`collect_bridge_events`]).
    pub kind: &'static str,
    pub data: Json,
}

/// Ring partilhado (mesmo padrão do `logs` ring): escrito pelo coletor,
/// lido pelo handler da REPL/BRP.
#[derive(Resource)]
pub struct BridgeEventLog {
    buffer: VecDeque<BridgeEvent>,
    next_seq: u64,
}

impl Default for BridgeEventLog {
    fn default() -> Self {
        Self {
            buffer: VecDeque::new(),
            // `seq` começa em 1: o cursor documentado é "último seq visto" e o
            // default dos clientes é 0 (`> since`) — com `seq` 0 o PRIMEIRO
            // evento da sessão nunca era devolvido.
            next_seq: 1,
        }
    }
}

impl BridgeEventLog {
    pub fn push(&mut self, kind: &'static str, data: Json) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.buffer.push_back(BridgeEvent {
            seq,
            time,
            kind,
            data,
        });
        while self.buffer.len() > EVENT_BUFFER_CAPACITY {
            self.buffer.pop_front();
        }
    }

    /// Eventos com `seq > since` (cauda do ring).
    pub fn since(&self, since: u64) -> impl Iterator<Item = &BridgeEvent> {
        self.buffer.iter().filter(move |event| event.seq > since)
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Cauda para o snapshot da REPL (últimos [`SNAPSHOT_TAIL`]).
    pub fn tail_json(&self) -> Json {
        json!(
            self.buffer
                .iter()
                .rev()
                .take(SNAPSHOT_TAIL)
                .rev()
                .map(event_json)
                .collect::<Vec<_>>()
        )
    }
}

/// JSON de um evento (`{seq, time, kind, ...data}`).
pub fn event_json(event: &BridgeEvent) -> Json {
    let mut out = json!({
        "seq": event.seq,
        "time": event.time,
        "kind": event.kind,
    });
    if let (Some(obj), Json::Object(data)) = (out.as_object_mut(), &event.data) {
        for (key, value) in data {
            obj.insert(key.clone(), value.clone());
        }
    }
    out
}

/// Coleta os eventos do frame para o ring. Corre no PostUpdate — DEPOIS de
/// tudo o que escreveu eventos no Update; os `MessageReader` com `Local`
/// deduplicam entre frames.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn collect_bridge_events(
    mut log: ResMut<BridgeEventLog>,
    mut damage_numbers: MessageReader<crate::feedback::DamageNumberEvent>,
    mut ui_actions: MessageReader<crate::ui::actions::UiAction>,
    mut toasts: MessageReader<crate::luau::ScriptToast>,
    deaths: Query<(Entity, Option<&Name>, &crate::vitals::Health), Changed<crate::vitals::Health>>,
    // Entidades já reportadas mortas: `Changed<Health>` NÃO é "cruzou para
    // 0". Um `damage` num cadáver reescreve `current = 0` (o deref-mut marca
    // changed) e um `set_max_hp` também, o que emitia um "death" novo a cada
    // chamada — QA via mortes fantasma. Sai do conjunto se a entidade voltar
    // a ter HP > 0 (revive).
    mut already_dead: Local<std::collections::HashSet<Entity>>,
) {
    let mut damage_seen = 0usize;
    for event in damage_numbers.read() {
        damage_seen += 1;
        if damage_seen > DAMAGE_PER_FRAME_CAP {
            break;
        }
        log.push(
            "damage",
            json!({
                "text": event.text.clone(),
                "x": event.position.x,
                "y": event.position.y,
                "z": event.position.z,
            }),
        );
    }
    for event in ui_actions.read() {
        log.push(
            "ui",
            json!({ "name": event.name.clone(), "arg": event.arg.clone() }),
        );
    }
    for toast in toasts.read() {
        log.push("toast", json!({ "message": toast.0.clone() }));
    }
    // Mortes: só no CRUZAMENTO para HP 0 — um cadáver que continue a levar
    // `damage`/`set_max_hp` não repete o evento.
    for (entity, name, health) in &deaths {
        if health.current <= 0.0 {
            if !already_dead.insert(entity) {
                continue;
            }
            let mut data = json!({ "entity": entity.to_bits() });
            if let Some(name) = name {
                data["name"] = json!(name.to_string());
            }
            log.push("death", data);
        } else {
            already_dead.remove(&entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deaths(app: &App) -> usize {
        app.world()
            .resource::<BridgeEventLog>()
            .since(0)
            .filter(|event| event.kind == "death")
            .count()
    }

    /// O evento `death` é do CRUZAMENTO para 0, não de qualquer mudança num
    /// cadáver (`Changed<Health>` sozinho repetia-o a cada `damage`/
    /// `set_max_hp` num corpo já a 0).
    #[test]
    fn test_death_event_only_on_hp_crossing() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_message::<crate::feedback::DamageNumberEvent>()
            .add_message::<crate::ui::actions::UiAction>()
            .add_message::<crate::luau::ScriptToast>()
            .init_resource::<BridgeEventLog>()
            .add_systems(bevy::app::PostUpdate, collect_bridge_events);
        let entity = app
            .world_mut()
            .spawn(crate::vitals::Health {
                current: 10.0,
                max: 10.0,
            })
            .id();
        app.update();
        assert_eq!(deaths(&app), 0, "vivo não gera death");

        app.world_mut()
            .get_mut::<crate::vitals::Health>(entity)
            .unwrap()
            .current = 0.0;
        app.update();
        assert_eq!(deaths(&app), 1, "HP a 0 cruza uma vez");

        // Cadáver: mexer no máximo não pode repetir o evento.
        app.world_mut()
            .get_mut::<crate::vitals::Health>(entity)
            .unwrap()
            .max = 99.0;
        app.update();
        assert_eq!(deaths(&app), 1, "cadáver não repete o death");

        // Revive e morre outra vez: evento novo é legítimo.
        app.world_mut()
            .get_mut::<crate::vitals::Health>(entity)
            .unwrap()
            .current = 5.0;
        app.update();
        app.world_mut()
            .get_mut::<crate::vitals::Health>(entity)
            .unwrap()
            .current = 0.0;
        app.update();
        assert_eq!(deaths(&app), 2);
    }
}
