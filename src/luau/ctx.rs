//! Contexto injectado por chamada ([`ScriptCtx`] como app data da VM): o
//! snapshot de início-de-frame que as closures `viber.*` leem (entidade,
//! posições, teclas, quest/vault, terreno) e onde os comandos se acumulam.
//! Inclui os registos de superfície partilhados (estradas/água).

use bevy::prelude::*;

use super::commands::ScriptCommand;

/// Per-call context injected into the shared VM (Lua app data) by
/// [`luau_update`]. The `viber` closures read the snapshot and queue commands.
#[derive(Default, Debug)]
pub struct ScriptCtx {
    /// Entity currently running `on_update` (None outside script calls).
    pub entity: Option<Entity>,
    /// Path do script em execução (`viber.events` subscreve por path).
    /// Semeado por [`ScriptCtx::set_path`], que reaproveita o buffer — o
    /// `run_update` semeia-o para CADA script activo em TODOS os frames.
    pub path: Option<String>,
    /// Position snapshot of the current entity (start of frame).
    pub origin: Vec3,
    /// Player position snapshot, when a [`crate::player::Player`] exists.
    pub player: Option<Vec3>,
    /// Bits da entidade do herói (`viber.player_id`).
    pub player_bits: Option<i64>,
    /// Seconds since engine startup.
    pub elapsed: f64,
    /// Delta time passed to `on_update`.
    pub dt: f32,
    /// Queued `set_position` commands, drained by [`luau_update`].
    pub pending: Vec<(Entity, Vec3)>,
    /// Queued [`ScriptCommand`]s (drained e aplicado pós-frame).
    pub commands: Vec<ScriptCommand>,
    /// Teclas pressionadas neste frame (para `viber.interacted`).
    pub just_pressed: Vec<KeyCode>,
    /// Teclas PRESSIONADAS (held) neste frame (`viber.input.down`).
    pub keys_down: Vec<KeyCode>,
    /// Teclas largadas NESTE frame (`viber.input.released`).
    pub keys_released: Vec<KeyCode>,
    /// Rato just-pressed / held / just-released neste frame (`viber.input.*`
    /// com `"mouse1"`). Vazio quando não há `ButtonInput<MouseButton>` (apps
    /// mínimas de teste).
    pub mouse_pressed: Vec<bevy::input::mouse::MouseButton>,
    pub mouse_down: Vec<bevy::input::mouse::MouseButton>,
    pub mouse_released: Vec<bevy::input::mouse::MouseButton>,
    /// Snapshot `nome → bits` de TODAS as entidades nomeadas do mundo para
    /// `viber.find`/`viber.find_all` (vetores ordenados por bits de entidade:
    /// `find_all` devolve ordem estável entre frames/máquinas).
    pub named_entities: std::collections::HashMap<String, Vec<i64>>,
    /// Snapshot `(current, max)` do HP de cada entidade COM `Health` (sem o
    /// player — esse já vive em `player_hp`) para `viber.entity_hp(id)`.
    pub hp_snapshot: std::collections::HashMap<i64, (f32, f32)>,
    /// Props autorais (`prop-*`, `destructible`, resource-*) por entidade,
    /// para `viber.nearby` devolver `props` — DADOS que o jogo interpreta.
    pub props_snapshot: std::collections::HashMap<i64, std::sync::Arc<Vec<(String, String)>>>,
    /// Hostis vivos (`enemies/`/`bosses/`) para `viber.hostiles`. As criaturas
    /// de `<DynamicSpawner>` não têm nome — `find_all("wolf")` não as acha.
    pub hostiles: Vec<HostileSnap>,
    /// Posição de MUNDO das entidades nomeadas (`viber.entity_position`,
    /// `viber.nearby`) — o mesmo snapshot de nomes, com o GlobalTransform.
    pub pos_snapshot: std::collections::HashMap<i64, (String, Vec3)>,
    /// Nome de MUNDO da entidade que corre o script (`viber.entity_name()`
    /// sem id). Semeado por chamada em [`super::runtime::luau_update`] a
    /// partir da query `Name` (fora do snapshot — o nome próprio não pode
    /// depender de truncagem de snapshot).
    pub self_name: Option<String>,
    /// Props `prop-*` da entidade que corre o script (`viber.prop`), semeados
    /// por chamada como o `self_name`.
    pub self_props: Option<std::sync::Arc<Vec<(String, String)>>>,
    /// Esquema `--@export` de cada script carregado (por path) — atualizado
    /// pelo `LuaScriptHost::load_script` (e portanto pelo hot-reload).
    pub exports: std::collections::HashMap<String, std::sync::Arc<super::exports::ScriptExports>>,
    /// True enquanto corre o TOP-LEVEL de um chunk (partilhado por todas as
    /// instâncias) — `viber.prop` recusa-se aí.
    pub top_level: bool,
    /// Avisos já dados de valores `prop-*` inválidos (1× por script+prop+valor).
    pub prop_warned: std::collections::HashSet<String>,
    /// Snapshot `grupo → bits` (ordenados) das entidades com `groups=` —
    /// `viber.group`/`in_group` e os alvos `@grupo` das conexões.
    pub groups: std::collections::HashMap<String, Vec<i64>>,
    /// Id da cinemática a tocar (`viber.cinematic_playing()`), snapshot do frame.
    pub cinematic: Option<String>,
    /// Pulsos de sentido DESTE frame, por entidade (bits → `"see"`/`"hear"`).
    /// `viber.sense()` resolve pelo `ctx.entity` corrente: um `Option<String>`
    /// semeado por chamada ficava pendurado nos hooks (um timer/sinal de uma
    /// entidade lia o sentido de OUTRA, do último `on_update`). A chave é
    /// `'static` (`SenseKind::as_str`) — sem `to_string` por pulso por frame.
    pub sense_by_entity: std::collections::HashMap<i64, &'static str>,
    /// Ações do mapa de input que dispararam neste frame (`viber.action`).
    /// Array FIXO de chaves estáticas: o `HashMap<String, bool>` antigo
    /// realçava o mapa e seis strings em TODOS os frames.
    pub action_pressed: [(&'static str, bool); 6],
    /// `MenusOpen::any()` neste frame (`viber.menu_open`): menu, mapa,
    /// viagem rápida ou cinemática estão a roubar o input.
    pub menu_open: bool,
    /// Emissor do sinal/`call_group` em entrega (`viber.sender()`); `None` fora
    /// de um handler.
    pub sender: Option<i64>,
    /// Forward do herói no plano (x, z) — `viber.player_forward()`.
    pub player_forward: Vec2,
    /// Pose VIVA da câmara de jogo — `viber.camera_state()`: posição do olho
    /// e direção de vista (yaw/pitch em graus, convenção do `camera_pose`).
    pub camera: Option<(Vec3, f32, f32)>,
    /// Dir raiz dos scripts (`<mundo>/scripts`) — `viber.load` resolve
    /// módulos contra ele. Fixo por mundo (semeado no `LuaScriptHost::new`).
    pub scripts_dir: Option<std::path::PathBuf>,
    /// Um `UiInput` está focado (barra de fala do NLU): teclas digitadas não
    /// podem disparar [E]/dígitos/hotbar. O player_movement já lê o recurso
    /// directamente; aqui é o espelho para a API `viber.input`/`interacted`.
    pub ui_typing: bool,
    /// Chave do save do mundo (`world_save_key`) — `viber.save_slots` lê os
    /// metadados dos slots sem o jogo conhecer o path.
    pub world_key: Option<String>,
    /// `save.dir` do config.yaml (fallback histórico quando `None`).
    pub save_dir: Option<std::path::PathBuf>,
    /// Nomes dos `<Prototype>` do mundo (validação à fila de
    /// `viber.spawn_prototype` — erro de script em vez de falha silenciosa).
    pub prototype_names: Vec<String>,
    /// Ring of `viber.log` lines (capped) — also read by tests.
    pub logs: Vec<String>,
    /// Range de interação da entidade actual (`viber.set_interaction`) —
    /// `viber.interacted` respeita-o em vez de hardcodar 3,5 m.
    pub interaction_range: Option<f32>,
    /// Vencedor por tecla do frame (`interact::InteractionFocus`): só ele
    /// vê `viber.interacted(tecla)` a `true`.
    pub interaction_focus: std::collections::HashMap<KeyCode, Entity>,
    /// Snapshot do HP do herói `(current, max)` para `viber.player_hp`.
    pub player_hp: Option<(f32, f32)>,
    /// Duração de cada clip do herói (`viber.clip_duration`), nomes
    /// normalizados → segundos. O jogo agenda o fim do swing sem hardcodar
    /// durações que mudam com o GLB.
    pub clip_durations: std::collections::HashMap<String, f32>,
    /// Tabela de idioma ativa (`locale/<lang>.json`) — `viber.localize`
    /// resolve as chaves `@nome` fora do `say`/`toast`.
    pub locale_table: Option<std::sync::Arc<std::collections::HashMap<String, String>>>,
    /// Idioma ativo (`viber.locale.get`).
    pub locale_lang: Option<String>,
    /// Acessibilidade (`viber.a11y`): escala de legendas, shake, hit-stop e
    /// [E] por hold.
    pub a11y: A11ySnap,
    /// Papel de netplay (`viber.net_role`): "off" | "host" | "join". Semeados
    /// TODOS os frames a partir do [`crate::netplay::NetRole`] — estáticos,
    /// sem realocar a string.
    pub net_role: &'static str,
    /// Handle de leitura partilhado do terreno (`viber.ground_below`) —
    /// dois `Arc` clones; o terreno não muda pós-bootstrap, portanto o
    /// snapshot É o terreno (não nasce segunda fonte de altura).
    pub terrain: Option<crate::terrain::runtime::TerrainReader>,
    /// Registos de superfície partilhados (`viber.on_road`, `viber.in_water`).
    ///
    /// O [`crate::terrain::runtime::TerrainReader`] leva o campo de altura e o
    /// voxel, mas não as estradas nem os corpos de água — e o `TerrainRuntime`
    /// não é clonável por frame. Este `Arc` é construído UMA vez (o mundo
    /// carvado não muda depois do bootstrap) e depois custa um clone de
    /// ponteiro por frame.
    pub surfaces: Option<std::sync::Arc<SurfaceRegistries>>,
    /// Cena carregada (`viber.scene_name`) — `None` fora de um projeto com cenas.
    pub scene: Option<String>,
    /// Entrada por onde se chegou (`viber.scene_entry`).
    pub scene_entry: Option<String>,
    /// Ids das cenas do projeto (validação de `viber.change_scene`).
    pub scene_ids: std::sync::Arc<Vec<String>>,
}

/// Criatura hostil viva neste frame (`viber.hostiles`).
#[derive(Debug, Clone)]
pub struct HostileSnap {
    pub id: i64,
    pub kind: String,
    pub pos: Vec3,
    pub hp: f32,
    pub max: f32,
    /// Forward do modelo no plano (XZ, unitário) — o jogo usa-o para backstab.
    pub forward: Vec2,
}

/// Snapshot de [`crate::a11y::A11y`] para `viber.a11y()`.
#[derive(Debug, Clone, Copy)]
pub struct A11ySnap {
    pub subtitle_scale: f32,
    pub shake: bool,
    pub hitstop: bool,
    pub interact_hold: bool,
}

impl Default for A11ySnap {
    fn default() -> Self {
        Self {
            subtitle_scale: 1.0,
            shake: true,
            hitstop: true,
            interact_hold: false,
        }
    }
}

/// Estradas e água, partilhadas com os scripts.
#[derive(Debug, Default)]
pub struct SurfaceRegistries {
    pub roads: Vec<crate::terrain::roads::RoadPath>,
    pub water: Vec<crate::terrain::water::WaterBody>,
}

impl SurfaceRegistries {
    /// Ponto na fita de uma estrada (`isPointOnRoad`).
    pub fn on_road(&self, x: f32, z: f32) -> bool {
        let p = Vec2::new(x, z);
        self.roads.iter().any(|road| road.is_on_road(p))
    }

    /// Estrada mais próxima do ponto, mesmo fora da fita (`viber.road`).
    /// `None` quando o mundo não tem estradas.
    pub fn nearest_road(&self, x: f32, z: f32) -> Option<crate::terrain::roads::RoadSample> {
        let p = Vec2::new(x, z);
        self.roads
            .iter()
            .filter_map(|road| road.sample_at(p))
            .min_by(|a, b| a.center_dist.total_cmp(&b.center_dist))
    }

    /// Ponto na estrada mais longa a `s` metros de arco (`viber.road_at`).
    /// O comprimento é INVARIÁVEL pós-bootstrap: computá-lo dentro do
    /// `max_by` percorria as estações de cada estrada em CADA comparação
    /// (a IA de perseguição chama isto todos os frames).
    pub fn road_at(&self, s: f32) -> Option<crate::terrain::roads::RoadSample> {
        let (index, _) = self
            .roads
            .iter()
            .enumerate()
            .map(|(index, road)| (index, crate::terrain::paths::path_length(&road.stations)))
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        self.roads[index].point_at_arc(s)
    }

    /// Ponto dentro de uma zona de carve de água.
    pub fn in_water(&self, x: f32, z: f32) -> bool {
        let p = Vec2::new(x, z);
        self.water.iter().any(|body| body.contains(p))
    }
}

/// Cache do snapshot de superfícies, construída no primeiro frame com terreno.
///
/// Vive num `Local` do `luau_update` e não num `Resource`: ninguém mais a lê, e
/// as apps mínimas dos testes montam o sistema solto, sem plugin — um recurso
/// obrigatório partia-as todas.
pub type SurfaceCache = Option<std::sync::Arc<SurfaceRegistries>>;

impl ScriptCtx {
    /// Semeia o path do script em execução REAPROVEITANDO o buffer anterior:
    /// um `String` novo por entidade activa por frame era alocação pura (o
    /// path repet-se entre frames e entre entidades que partilham script).
    pub(crate) fn set_path(&mut self, path: &str) {
        let mut slot = self.path.take().unwrap_or_default();
        slot.clear();
        slot.push_str(path);
        self.path = Some(slot);
    }

    /// Semeia o nome próprio da entidade que vai correr (`None` = sem `Name`)
    /// com o mesmo reaproveitamento de buffer do [`ScriptCtx::set_path`] — o
    /// loop de `on_update` faz isto por entidade activa em TODOS os frames.
    pub(crate) fn set_self_name(&mut self, name: Option<&Name>) {
        let Some(name) = name else {
            self.self_name = None;
            return;
        };
        let mut slot = self.self_name.take().unwrap_or_default();
        slot.clear();
        slot.push_str(name.as_str());
        self.self_name = Some(slot);
    }

    const LOG_CAP: usize = 256;
    /// Cap por linha: um `viber.log` gigante não pode reter dezenas de MB
    /// no ring de 256 linhas (trunca em char boundary, igual ao bridge).
    const MAX_LOG_MESSAGE: usize = 8192;
    pub(crate) fn push_log(&mut self, mut line: String) {
        if line.len() > Self::MAX_LOG_MESSAGE {
            let mut end = Self::MAX_LOG_MESSAGE;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
            line.push('…');
        }
        if self.logs.len() >= Self::LOG_CAP {
            self.logs.remove(0);
        }
        self.logs.push(line);
    }
}
