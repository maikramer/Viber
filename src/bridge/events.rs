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
    /// "hurt" | "damage" | "death" | "quest" | "ui" | "travel" | "toast"
    /// | "levelup"
    pub kind: &'static str,
    pub data: Json,
}

/// Ring partilhado (mesmo padrão do `logs` ring): escrito pelo coletor,
/// lido pelo handler da REPL/BRP.
#[derive(Resource, Default)]
pub struct BridgeEventLog {
    buffer: VecDeque<BridgeEvent>,
    next_seq: u64,
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
    // Mortes: HP acabou de cruzar para 0 (Changed evita repetir o cadáver).
    for (entity, name, health) in &deaths {
        if health.current <= 0.0 {
            let mut data = json!({ "entity": entity.to_bits() });
            if let Some(name) = name {
                data["name"] = json!(name.to_string());
            }
            log.push("death", data);
        }
    }
}
