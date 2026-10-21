//! Grafo de diálogo opcional (`dialogues/<id>.json`).
//!
//! Sem ficheiro, o NPC continua nas três listas da quest. Com ficheiro, o [E]
//! avança nós e as escolhas são `1`..`9`. O cursor entra no save.

use std::collections::HashMap;
use std::path::Path;

use bevy::prelude::*;
use bevy_kira_audio::AudioControl;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct DialogueChoice {
    pub text: String,
    #[serde(default)]
    pub goto: Option<String>,
    #[serde(default)]
    pub quest_accept: bool,
    #[serde(default)]
    pub quest_turn_in: bool,
    #[serde(default)]
    pub lua: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DialogueNode {
    #[serde(default)]
    pub speaker: String,
    pub text: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub portrait: Option<String>,
    #[serde(default)]
    pub choices: Vec<DialogueChoice>,
}

#[derive(Clone, Debug, Deserialize)]
struct DialogueFile {
    #[serde(default = "default_start")]
    pub start: String,
    pub nodes: HashMap<String, DialogueNode>,
}

fn default_start() -> String {
    "start".into()
}

#[derive(Clone, Debug)]
pub struct DialogueGraph {
    pub start: String,
    pub nodes: HashMap<String, DialogueNode>,
}

impl DialogueGraph {
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: DialogueFile = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if !file.nodes.contains_key(&file.start) {
            return Err(format!("nó inicial '{}' não existe", file.start));
        }
        Ok(Self {
            start: file.start,
            nodes: file.nodes,
        })
    }
}

#[derive(Resource, Default, Clone)]
pub struct DialogueBook {
    pub graphs: HashMap<String, DialogueGraph>,
}

impl DialogueBook {
    pub fn load_dir(dir: &Path) -> Self {
        let mut graphs = HashMap::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Self { graphs };
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| DialogueGraph::parse(&t)) {
                Ok(graph) => {
                    graphs.insert(id.to_string(), graph);
                }
                Err(error) => warn!("diálogo {id}: {error}"),
            }
        }
        Self { graphs }
    }

    pub fn has(&self, id: &str) -> bool {
        self.graphs.contains_key(id)
    }
}

/// Nó corrente por id de diálogo.
#[derive(Resource, Default, Clone, Debug)]
pub struct DialogueSessions {
    pub node: HashMap<String, String>,
}

/// O que o balão deve mostrar depois de um [E] ou de um dígito.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownLine {
    pub text: String,
    pub voice: Option<String>,
    pub portrait: Option<String>,
    pub accept: bool,
    pub turn_in: bool,
    pub lua: Option<String>,
}

/// Avança o grafo. `digit` é a escolha 1-based; `None` no primeiro [E] mostra o nó.
pub fn advance(
    graph: &DialogueGraph,
    sessions: &mut DialogueSessions,
    id: &str,
    digit: Option<usize>,
) -> Option<ShownLine> {
    let current = sessions
        .node
        .get(id)
        .cloned()
        .unwrap_or_else(|| graph.start.clone());
    let node = graph.nodes.get(&current)?;
    if let Some(n) = digit {
        let choice = node.choices.get(n.saturating_sub(1))?;
        let next = choice
            .goto
            .clone()
            .unwrap_or_else(|| current.clone());
        if graph.nodes.contains_key(&next) {
            sessions.node.insert(id.to_string(), next.clone());
        }
        let shown = graph.nodes.get(sessions.node.get(id).unwrap_or(&next));
        let text = shown
            .map(|n| render_node(n))
            .unwrap_or_else(|| choice.text.clone());
        return Some(ShownLine {
            text,
            voice: shown.and_then(|n| n.voice.clone()).or(None),
            portrait: shown.and_then(|n| n.portrait.clone()),
            accept: choice.quest_accept,
            turn_in: choice.quest_turn_in,
            lua: choice.lua.clone(),
        });
    }
    sessions.node.insert(id.to_string(), current);
    Some(ShownLine {
        text: render_node(node),
        voice: node.voice.clone(),
        portrait: node.portrait.clone(),
        accept: false,
        turn_in: false,
        lua: None,
    })
}

fn render_node(node: &DialogueNode) -> String {
    let mut body = if node.speaker.is_empty() {
        node.text.clone()
    } else {
        format!("{}: {}", node.speaker, node.text)
    };
    for (i, choice) in node.choices.iter().enumerate() {
        body.push_str(&format!("\n{}. {}", i + 1, choice.text));
    }
    body
}

/// Resolve um texto `@chave` contra a tabela de locale; sem tabela devolve o original.
pub fn localized<'a>(text: &'a str, table: Option<&HashMap<String, String>>) -> String {
    let Some(key) = text.strip_prefix('@') else {
        return text.to_string();
    };
    table
        .and_then(|t| t.get(key).cloned())
        .unwrap_or_else(|| text.to_string())
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct DialogueAccess<'w, 's> {
    pub book: Option<Res<'w, DialogueBook>>,
    pub sessions: Option<ResMut<'w, DialogueSessions>>,
    pub hold: Local<'s, f32>,
}

/// Clip de voz por tocar (caminho de asset). O envelope da mandíbula abre junto.
#[derive(Component)]
pub struct PendingVoice(pub String);

fn play_pending_voices(
    mut commands: Commands,
    voices: Query<(Entity, &PendingVoice)>,
    server: Option<Res<AssetServer>>,
    sfx: Option<Res<bevy_kira_audio::AudioChannel<crate::music::SfxBus>>>,
    mut envelope: Option<ResMut<crate::ik::VoiceEnvelope>>,
) {
    let (Some(server), Some(sfx)) = (server, sfx) else {
        return;
    };
    for (entity, voice) in &voices {
        let handle: Handle<bevy_kira_audio::AudioSource> = server.load(voice.0.clone());
        sfx.play(handle);
        if let Some(envelope) = envelope.as_mut() {
            envelope.level = 1.0;
        }
        commands.entity(entity).despawn();
    }
}

pub struct DialoguePlugin {
    pub dir: std::path::PathBuf,
}

impl Plugin for DialoguePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DialogueBook::load_dir(&self.dir))
            .init_resource::<DialogueSessions>()
            .add_systems(Update, play_pending_voices);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DialogueGraph {
        DialogueGraph::parse(
            r#"{
                "start": "hi",
                "nodes": {
                    "hi": {
                        "speaker": "Mina",
                        "text": "Trazes a carta?",
                        "voice": "npc/ask.ogg",
                        "portrait": "portraits/mina.png",
                        "choices": [
                            {"text": "Sim", "quest_turn_in": true, "goto": "yes"},
                            {"text": "Agora não", "goto": "no"}
                        ]
                    },
                    "yes": {"speaker": "Mina", "text": "Obrigada."},
                    "no": {"speaker": "Mina", "text": "Volta quando puderes."}
                }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn choices_branch_and_the_refusal_does_not_turn_the_quest_in() {
        let graph = sample();
        let mut sessions = DialogueSessions::default();
        let open = advance(&graph, &mut sessions, "carta", None).unwrap();
        assert!(open.text.contains("Trazes a carta"));
        assert!(open.text.contains("1. Sim"));
        assert!(!open.turn_in);
        let yes = advance(&graph, &mut sessions, "carta", Some(1)).unwrap();
        assert!(yes.turn_in);
        assert!(yes.text.contains("Obrigada"));
        let mut sessions = DialogueSessions::default();
        advance(&graph, &mut sessions, "carta", None).unwrap();
        let no = advance(&graph, &mut sessions, "carta", Some(2)).unwrap();
        assert!(!no.turn_in);
        assert!(no.text.contains("Volta"));
    }

    #[test]
    fn missing_locale_key_falls_back_to_the_source_string() {
        assert_eq!(localized("Olá", None), "Olá");
        assert_eq!(localized("@missing", None), "@missing");
        let mut table = HashMap::new();
        table.insert("hi".into(), "Hello".into());
        assert_eq!(localized("@hi", Some(&table)), "Hello");
    }
}
