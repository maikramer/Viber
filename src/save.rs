//! Save/load & opções (loop 7 do port simple-rpg) — o análogo nativo do
//! `SaveLoadPlugin` original (localStorage + msgpackr):
//!
//! - **[`SaveGame`]**: XP, posição, vault (recursos + itens), estados das
//!   quests e marcos da Nota — serializado em JSON.
//! - **Disco**: `~/.local/share/viber/<world>.save.json` (fallback
//!   `./<world>.save.json` sem HOME).
//! - **UI**: no modal [Q], tab **Opções** — ↑↓ escolhe a linha (volumes
//!   master/música/sfx e Save/Load), ←/→ ajusta o volume ±10 %, [J] grava
//!   e [L] carrega.
//!
//! Os `defs` das quests são estáticos (embutidos) — só os ESTADOS viajam no
//! ficheiro, como os serializers por módulo da referência.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::luau::ScriptToast;
use crate::menus::MenusOpen;
use crate::music::AudioMixerSettings;
use crate::player::Player;

/// Nome do save deste mundo ([`world_save_key`]), inserido pelo `run` a
/// partir do caminho do world.xml; `None` = o nome global histórico.
#[derive(Debug, Clone, Resource, Default)]
pub struct WorldSaveKey(pub Option<String>);

/// Chave do save de um ficheiro de mundo: a pasta do jogo para `world.xml`
/// (`simple-rpg/world.xml` → `simple-rpg`, o nome dos saves existentes) e
/// `pasta-ficheiro` para os restantes — mundos irmãos na mesma pasta
/// (`worlds/qa-agua.xml`, `worlds/qa-cliffs.xml`) deixam de partilhar o save.
/// O sufixo `.static` do stem é removido (`world.static.xml`, o default
/// pós-`viber bake`, partilha a chave do `world.xml` de onde veio — a
/// continuidade dos saves não pode depender do caminho de arranque; só o
/// sufixo exato conta, `qa-static.xml` fica `qa-static`).
/// O caminho é tornado absoluto: `viber run world.xml` a partir da pasta do
/// jogo tem `parent()` vazio e caía no nome global.
pub fn world_save_key(world_file: &Path) -> Option<String> {
    let abs = world_file
        .canonicalize()
        .or_else(|_| std::path::absolute(world_file))
        .ok()?;
    let dir = abs.parent()?.file_name()?.to_string_lossy().into_owned();
    let mut stem = abs.file_stem()?.to_string_lossy().into_owned();
    if let Some(base) = stem.strip_suffix(".static") {
        stem = base.to_owned();
    }
    Some(if stem.eq_ignore_ascii_case("world") {
        dir
    } else {
        format!("{dir}-{stem}")
    })
}

/// Diretório de saves do `config.yaml` do jogo (`save.dir`); `None` = o
/// histórico `~/.local/share/viber` (apps mínimas de teste).
#[derive(Debug, Clone, Resource, Default)]
pub struct SaveDir(pub Option<PathBuf>);

/// Nome do ficheiro de save fallback (quando o mundo é desconhecido).
pub const SAVE_FILENAME: &str = "simple-rpg.save.json";

/// Caminho do save: `save.dir` do config do jogo (fallback histórico
/// `~/.local/share/viber` via [`dirs::home_dir`] — semântica `$HOME` exata,
/// NÃO `XDG_DATA_HOME`, os saves históricos vivem aí; último fallback: cwd).
pub fn save_path() -> PathBuf {
    save_path_for(None, None)
}

/// Caminho do save do mundo (`world_key` = [`world_save_key`]): gravar no
/// mundo A e carregar no B nunca mais restaura o estado errado. Sem chave
/// conhecida, cai no nome global histórico.
pub fn save_path_for(world_key: Option<&str>, save_dir: Option<&Path>) -> PathBuf {
    let name = world_key
        .filter(|key| !key.is_empty())
        .map(|key| format!("{key}.save.json"))
        .unwrap_or_else(|| SAVE_FILENAME.to_string());
    let dir = match save_dir.map(Path::to_path_buf) {
        Some(dir) => {
            let _ = std::fs::create_dir_all(&dir);
            dir
        }
        None => dirs::home_dir()
            .map(|home| {
                let dir = home.join(".local/share/viber");
                let _ = std::fs::create_dir_all(&dir);
                dir
            })
            .unwrap_or_default(),
    };
    if dir.as_os_str().is_empty() {
        PathBuf::from(name)
    } else {
        dir.join(name)
    }
}

/// Caminho de um slot. `None` = save legado; `0` = autosave; `1..` = slots
/// manuais. O sufixo entra antes de `.save.json`, por isso o save antigo do
/// mundo continua no sítio e é lido pelo `load_save()` sem migração.
pub fn save_path_for_slot(
    world_key: Option<&str>,
    save_dir: Option<&Path>,
    slot: Option<u32>,
) -> PathBuf {
    let base = save_path_for(world_key, save_dir);
    let Some(slot) = slot else {
        return base;
    };
    let suffix = if slot == 0 {
        "auto".to_string()
    } else {
        format!("slot{slot}")
    };
    let file = base
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(SAVE_FILENAME);
    let name = file
        .strip_suffix(".save.json")
        .map(|stem| format!("{stem}.{suffix}.save.json"))
        .unwrap_or_else(|| format!("{file}.{suffix}"));
    base.with_file_name(name)
}

/// Sumário de um slot para o menu (o jogo não abre o JSON à mão).
#[derive(Debug, Clone, Serialize)]
pub struct SaveSlotInfo {
    pub slot: u32,
    pub path: String,
    pub exists: bool,
    pub bytes: u64,
    /// Unix seconds: campo do save, ou mtime do ficheiro nos saves antigos.
    pub saved_at: Option<f64>,
    pub level: u32,
    pub gold: u32,
    pub scene: Option<String>,
    pub playtime: Option<f64>,
    pub title: Option<String>,
}

/// Lê os metadados dos slots 0..=3 (0 = autosave) e, se existir, do save
/// legado (slot 4 — `viber.load_save()`, não `load_slot(4)`). Nunca falha:
/// um JSON corrompido lista o slot como existente mas sem nível/ouro.
pub fn list_save_slots(world_key: Option<&str>, save_dir: Option<&Path>) -> Vec<SaveSlotInfo> {
    fn info(slot: u32, path: std::path::PathBuf) -> SaveSlotInfo {
        let meta = std::fs::metadata(&path).ok();
        let exists = meta.is_some();
        let bytes = meta.as_ref().map_or(0, |m| m.len());
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        let game = exists
            .then(|| load_from_disk(&path).ok())
            .flatten()
            .unwrap_or_default();
        SaveSlotInfo {
            slot,
            path: path.display().to_string(),
            exists,
            bytes,
            saved_at: game.saved_at.or(mtime),
            level: game.level,
            gold: game.gold,
            scene: game.scene.clone(),
            playtime: game.playtime,
            title: game.title.clone(),
        }
    }

    let mut rows: Vec<SaveSlotInfo> = (0..=3)
        .map(|slot| info(slot, save_path_for_slot(world_key, save_dir, Some(slot))))
        .collect();
    let legacy = info(4, save_path_for(world_key, save_dir));
    if legacy.exists {
        rows.push(legacy);
    }
    rows
}

// ── estrutura do save ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SaveGame {
    /// Versão do schema do save. Saves antigos (sem o campo) carregam como 0;
    /// um save de versão MAIS NOVA que a engine [`SAVE_VERSION`] carrega na
    /// mesma (campos a mais são ignorados pelo serde) mas o load avisa — os
    /// campos novos perdem-se ao regravar por cima.
    #[serde(default)]
    pub version: u32,
    /// Unix seconds da gravação. Saves antigos não têm; o menu cai no mtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<f64>,
    /// Tempo de jogo acumulado (segundos), escrito pelo jogo em `world_kv`
    /// ou directamente aqui.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playtime: Option<f64>,
    /// Rótulo do slot (ex. "Discórdia — Capítulo I").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    // Campos nucleares com `#[serde(default)]`: um save antigo/parcial sem
    // `volumes`/`nota_marked`/`items`/… carrega em vez de ser recusado inteiro.
    // XP, vitais, posição e volumes são `Option`: o zero do tipo matava o
    // herói (health 0/0), punha-o na origem, parava os level-ups (next 0) e
    // silenciava o áudio. Ausentes, o load mantém o estado da sessão; o JSON
    // de `Some` é o mesmo array de sempre.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xp: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<(f32, f32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    #[serde(default)]
    pub gold: u32,
    #[serde(default)]
    pub wood: u32,
    #[serde(default)]
    pub stone: u32,
    #[serde(default)]
    pub items: HashMap<String, u32>,
    /// Estados das quests ativas: id → progresso + marcos visitados.
    #[serde(default)]
    pub quest_states: HashMap<String, (u32, Vec<String>)>,
    #[serde(default)]
    pub quest_done: Vec<String>,
    /// Marcos da Nota assinados.
    #[serde(default)]
    pub nota_marked: Vec<String>,
    /// Biomas revelados por atalaias de agrimensura (travel::RevealedBiomes)
    /// — sem isto, carregar voltava a esconder os marcos já cartografados.
    #[serde(default)]
    pub revealed_biomes: Vec<String>,
    /// Skills aprendidas (ids) + pontos disponíveis + nível — sem isto,
    /// carregar apagava a árvore comprada.
    #[serde(default)]
    pub skill_learned: Vec<String>,
    #[serde(default)]
    pub skill_points: u32,
    #[serde(default)]
    pub level: u32,
    /// Volumes (master, music, sfx) 0..=1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volumes: Option<(f32, f32, f32)>,
    /// Volumes dos buses novos (voice, ambience) 0..=1. Campo separado
    /// porque o save antigo é uma tupla de 3 e não pode mudar de forma.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volumes_extra: Option<(f32, f32)>,
    /// Estado de JOGO world-scoped do Luau (`viber.game()`), JSON plano
    /// (string/número/bool). É o gancho de persistência para lógica de jogo
    /// que vive em Lua — sem ele, scripts não conseguiam guardar nada.
    #[serde(default)]
    pub world_kv: std::collections::BTreeMap<String, serde_json::Value>,
    /// Cena onde o herói estava (projetos com cenas, `crate::project`); `position` é
    /// relativa a ELA. Ausente (saves v1) = a cena principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// `(cena, portal)` por onde se entrou na cena gravada — o `@back` da porta de saída.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub came_from: Option<(String, String)>,
    /// Idioma do save (`pt`, `en`); ausente = o do arranque.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// LEGADO: nomes de props colhidos. A colheita atual guarda a marca no
    /// `world_kv` do jogo; o campo fica só para ler saves antigos.
    #[serde(default)]
    pub harvested: Vec<String>,
    /// Nó corrente de cada diálogo com grafo (`dialogue id → node`).
    #[serde(default)]
    pub dialogue_nodes: std::collections::HashMap<String, String>,
    /// Recortes de terreno editados em jogo.
    #[serde(default)]
    pub terrain_patches: Vec<crate::terrain::delta::DeltaPatch>,
    /// Rebinds do mapa de ações (`ação → tecla`).
    #[serde(default)]
    pub action_keys: std::collections::HashMap<String, String>,
}

/// Versão do schema [`SaveGame`] que ESTA engine escreve. Incrementar ao
/// acrescentar campos cuja ausência muda a leitura (hoje tudo tem default —
/// o 1 existe para o save mais novo que a engine avisar no load).
pub const SAVE_VERSION: u32 = 2;

/// Captura pose, volumes e o envelope do ficheiro. Ouro, quests, talentos
/// e marcos vivem em `world_kv` (`viber.game()`), que o caller preenche.
pub fn capture(position: [f32; 3], volumes: (f32, f32, f32)) -> SaveGame {
    SaveGame {
        version: SAVE_VERSION,
        position: Some(position),
        volumes: Some(volumes),
        ..Default::default()
    }
}

/// Escreve o save em disco (JSON pretty) ATOMICAMENTE: escreve num `.tmp` NO
/// MESMO diretório, sincroniza e só então renomea — o rename é atómico no
/// mesmo filesystem, portanto um ENOSPC/kill -9/OOM da GPU a meio nunca
/// destrói o save anterior (é o único save, sem backup).
pub fn save_to_disk(path: &std::path::Path, game: &SaveGame) -> Result<(), String> {
    use std::io::Write;
    let json = serde_json::to_string_pretty(game).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    let write_tmp = || -> Result<(), String> {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(json.as_bytes())
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("{}: {e}", tmp.display()))
    };
    if let Err(e) = write_tmp() {
        let _ = std::fs::remove_file(&tmp); // não deixar .tmp órfão
        return Err(e);
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// Lê o save do disco.
pub fn load_from_disk(path: &std::path::Path) -> Result<SaveGame, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&json).map_err(|e| e.to_string())
}

// ── plugin ──────────────────────────────────────────────────────────────

pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OptionsRows>()
            .init_resource::<WorldSaveKey>()
            .init_resource::<SaveRequest>()
            .init_resource::<PendingSavePatches>()
            .add_systems(Update, options_system)
            // DEPOIS do driver da troca (Last): um pedido aplicado por commands
            // no Update só existe no `SceneLoader` a partir do schedule
            // seguinte — importar antes podia cair no runtime da cena VELHA no
            // próprio frame do load (o loader ainda parecia Idle).
            .add_systems(
                bevy::app::Last,
                apply_pending_save_patches.after(crate::scene_loader::drive_scene_loader),
            );
    }
}

/// Recortes de terreno de um save à espera do `TerrainRuntime` certo.
///
/// Importar logo no load punha-os no runtime da cena VELHA: num load que
/// troca de cena, a troca substitui o `TerrainRuntime` e os recortes
/// perdiam-se — ou caíam no terreno PEQUENO da cena errada (o menu tem o seu
/// próprio campo). Ficam pendentes; [`apply_pending_save_patches`] importa-os
/// no primeiro runtime vivo fora de uma troca (na mesma cena: no fim do
/// próprio frame do load).
#[derive(Debug, Default, Resource)]
pub struct PendingSavePatches(pub Vec<crate::terrain::delta::DeltaPatch>);

/// Consome [`PendingSavePatches`] assim que haja terreno vivo e nenhuma troca
/// em curso (`import_patches` sobe a revisão — o mesher volta a cozer as
/// colunas tocadas, valha importar cedo ou tarde).
pub fn apply_pending_save_patches(
    mut pending: ResMut<PendingSavePatches>,
    mut runtime: Option<ResMut<crate::terrain::runtime::TerrainRuntime>>,
    loader: Option<Res<crate::scene_loader::SceneLoader>>,
) {
    if pending.0.is_empty() {
        return;
    }
    let Some(runtime) = runtime.as_deref_mut() else {
        return; // sala sem terreno: espera pela volta ao campo principal
    };
    if loader.as_deref().is_some_and(crate::scene_loader::SceneLoader::busy) {
        return;
    }
    std::sync::Arc::make_mut(&mut runtime.deltas).import_patches(&pending.0);
    pending.0.clear();
}

/// Pedido de save/load vindo de SCRIPT (`viber.save()` / `viber.load()`) —
/// o `LuaScriptHost` não pode chamar o sistema de save de dentro do Luau, e o
/// host sem ficheiro `world_kv` não teria como persistir num mundo
/// `gameplay: none`. Consumido (e limpo) pelo `options_system`.
#[derive(Debug, Default, Resource)]
pub struct SaveRequest {
    pub save: bool,
    pub load: bool,
    /// `None` = save legado; `0` = autosave; `1..` = slot manual.
    pub slot: Option<u32>,
}

/// Linha selecionada na tab Opções (0=master, 1=música, 2=sfx, 3=save,
/// 4=load).
#[derive(Debug, Clone, Resource, Default)]
pub struct OptionsRows {
    pub selected: usize,
}

pub const OPTIONS_ROWS: usize = 5;

/// Aplica volumes. Campos de RPG no JSON antigo ficam no struct e não
/// entram em nenhum recurso — o jogo lê-os de `world_kv` se os tiver gravado.
pub fn apply_save(game: &SaveGame, mixer: &mut AudioMixerSettings) {
    if let Some((master, music, sfx)) = game.volumes {
        mixer.master = master.clamp(0.0, 1.0);
        mixer.music = music.clamp(0.0, 1.0);
        mixer.sfx = sfx.clamp(0.0, 1.0);
    }
    if let Some((voice, ambience)) = game.volumes_extra {
        mixer.voice = voice.clamp(0.0, 1.0);
        mixer.ambience = ambience.clamp(0.0, 1.0);
    }
}

/// Opções e gravação: linhas ↑↓, volumes ←→, [J] grava, [L] carrega — e os
/// mesmos dois efeitos disparados pelos botões do menu declarativo
/// (`viber.ui.action("save"|"load", "")`).
///
/// Sem throttle a montante do `just_pressed`: a tecla só é verdadeira 1
/// frame e o throttle (0,15 s) descartava ~90 % das pressões de [J]/[L].
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn options_system(
    keys: Res<ButtonInput<KeyCode>>,
    open: Res<MenusOpen>,
    ui_tabs: Option<Res<crate::ui::UiTabs>>,
    mut ui_actions: MessageReader<crate::ui::actions::UiAction>,
    mut rows: ResMut<OptionsRows>,
    mut mixer: ResMut<AudioMixerSettings>,
    mut locale: Option<ResMut<crate::locale::Locale>>,
    mut world_save_host_request: (
        Option<Res<WorldSaveKey>>,
        Option<Res<SaveDir>>,
        Option<ResMut<crate::luau::LuaScriptHost>>,
        Option<ResMut<SaveRequest>>,
        Commands,
        Option<Res<crate::scene_loader::CurrentScene>>,
        Option<Res<crate::terrain::runtime::TerrainRuntime>>,
        Option<Res<crate::input_map::ActionMap>>,
    ),
    mut heroes: Query<(Entity, &mut Transform, &mut Player), With<Player>>,
    mut toasts: MessageWriter<ScriptToast>,
    mut sfx: MessageWriter<crate::ambient::SfxEvent>,
) {
    // Os botões do menu funcionam sempre que o menu está aberto; as teclas
    // continuam a exigir o mesmo.
    let mut save_requested = false;
    let mut load_requested = false;
    for action in ui_actions.read() {
        match action.name.as_str() {
            "save" => save_requested = true,
            "load" => load_requested = true,
            _ => {}
        }
    }
    // `viber.save()` / `viber.load()` (scripts) valem sempre. O slot vem
    // junto: `None` = save legado, `0` = autosave, `1..` = manual.
    let (script_save, script_load, script_slot) = world_save_host_request
        .3
        .as_mut()
        .map(|r| {
            let trio = (r.save, r.load, r.slot);
            r.save = false;
            r.load = false;
            r.slot = None;
            trio
        })
        .unwrap_or((false, false, None));
    let save_requested = save_requested || script_save;
    let load_requested = load_requested || script_load;
    if !open.modal && !save_requested && !load_requested {
        return;
    }
    // Contrato: as TECLAS (↑↓ linhas, ←→ volumes, [J]/[L] save/load) só
    // valem na tab Sistema do modal — noutras tabs as setas/[J]/[L]
    // pertencem ao modal (na tab Talentos, ↑↓ mudava as linhas de opções E
    // a navegação em simultâneo). Os BOTÕES do menu declarativo (UiAction)
    // valem sempre — são cliques da própria tab Sistema.
    let on_system_tab = ui_tabs.as_deref().and_then(|tabs| tabs.selected("menu")) == Some("system");
    if on_system_tab {
        if keys.just_pressed(KeyCode::ArrowDown) {
            rows.selected = (rows.selected + 1) % OPTIONS_ROWS;
        }
        if keys.just_pressed(KeyCode::ArrowUp) {
            rows.selected = (rows.selected + OPTIONS_ROWS - 1) % OPTIONS_ROWS;
        }
        let left = keys.just_pressed(KeyCode::ArrowLeft);
        let right = keys.just_pressed(KeyCode::ArrowRight);
        if (left || right) && rows.selected < 3 {
            let delta = if right { 0.1 } else { -0.1 };
            match rows.selected {
                0 => mixer.master = (mixer.master + delta).clamp(0.0, 1.0),
                1 => mixer.music = (mixer.music + delta).clamp(0.0, 1.0),
                _ => mixer.sfx = (mixer.sfx + delta).clamp(0.0, 1.0),
            }
        }
    }
    let world_key = world_save_host_request
        .0
        .as_deref()
        .and_then(|w| w.0.as_deref());
    let save_dir = world_save_host_request
        .1
        .as_deref()
        .and_then(|d| d.0.as_deref());
    let mut host = world_save_host_request.2.as_deref_mut();
    if (on_system_tab && keys.just_pressed(KeyCode::KeyJ)) || save_requested {
        // O 1.º herói e não `single_mut()`: com 2+ heróis (co-op/mundos de QA)
        // o save recusava-se INTEIRO com "sem herói para gravar" — o resto da
        // engine (interact, attachment, projéteis, nav) já usa o primeiro
        // nessa situação.
        let hero_state = heroes
            .iter_mut()
            .next()
            .map(|(_, t, _player)| [t.translation.x, t.translation.y, t.translation.z]);
        if let Some(position) = hero_state {
            let mut game = capture(position, (mixer.master, mixer.music, mixer.sfx));
            game.volumes_extra = Some((mixer.voice, mixer.ambience));
            if let Some(host) = host.as_mut() {
                game.world_kv = crate::luau::game::game_to_json(&host.lua);
            }
            // Metadados que o menu lê sem abrir o JSON: hora da gravação,
            // tempo de jogo e rótulo (o jogo escreve-os em `viber.game()`).
            game.saved_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs_f64());
            game.playtime = game
                .world_kv
                .get("playtime_secs")
                .and_then(serde_json::Value::as_f64);
            // Metadados genéricos que um jogo pode publicar no `game()`.
            game.level = game
                .world_kv
                .get("save_level")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            game.gold = game
                .world_kv
                .get("save_gold")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            game.title = game
                .world_kv
                .get("save_title")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            game.locale = locale.as_deref().map(|locale| locale.lang.clone());
            if let Some(terrain) = world_save_host_request.6.as_deref() {
                game.terrain_patches = terrain.deltas.export_patches();
            }
            if let Some(map) = world_save_host_request.7.as_deref() {
                game.action_keys = map
                    .bindings
                    .iter()
                    .filter_map(|(action, binding)| {
                        binding
                            .key
                            .as_ref()
                            .map(|key| (action.as_str().to_string(), key.clone()))
                    })
                    .collect();
            }
            if let Some(scene) = world_save_host_request
                .5
                .as_deref()
                .filter(|scene| scene.switchable())
            {
                game.scene = scene.id.clone();
                game.came_from = scene.came_from.clone();
            }
            if let Err(e) =
                save_to_disk(&save_path_for_slot(world_key, save_dir, script_slot), &game)
            {
                toasts.write(ScriptToast(format!("Falha ao gravar: {e}")));
                sfx.write(crate::ambient::SfxEvent {
                    clip: crate::ambient::SfxClip::Error,
                    position: None,
                });
            } else {
                toasts.write(ScriptToast("Jogo gravado.".into()));
                sfx.write(crate::ambient::SfxEvent {
                    clip: crate::ambient::SfxClip::Save,
                    position: None,
                });
            }
        } else {
            toasts.write(ScriptToast("Falha ao gravar: sem herói para gravar".into()));
            sfx.write(crate::ambient::SfxEvent {
                clip: crate::ambient::SfxClip::Error,
                position: None,
            });
        }
    }
    if (on_system_tab && keys.just_pressed(KeyCode::KeyL)) || load_requested {
        match load_from_disk(&save_path_for_slot(world_key, save_dir, script_slot)) {
            Ok(game) => {
                if game.version > SAVE_VERSION {
                    bevy::log::warn!(
                        "save '{}' é da versão {} (esta engine escreve {}) — campos novos perdem-se se gravares por cima",
                        save_path_for_slot(world_key, save_dir, script_slot).display(),
                        game.version,
                        SAVE_VERSION
                    );
                }
                apply_save(&game, &mut mixer);
                // O idioma do save ganha ao do arranque; a tabela Lua é
                // recarregada no mesmo frame.
                if let Some(lang) = game.locale.clone() {
                    if let Some(locale) = locale.as_deref_mut() {
                        locale.set_lang(&lang);
                    }
                    if let Some(host) = host.as_mut()
                        && let Some(locale) = locale.as_deref()
                        && let Some(mut ctx) = host.lua.app_data_mut::<crate::luau::ScriptCtx>()
                    {
                        ctx.locale_table = Some(std::sync::Arc::new(locale.table.clone()));
                        ctx.locale_lang = Some(locale.lang.clone());
                    }
                }
                let patches = game.terrain_patches.clone();
                let keys = game.action_keys.clone();
                world_save_host_request.4.queue(move |world: &mut World| {
                    // Recortes de terreno PENDENTES (ver `PendingSavePatches`):
                    // importá-los aqui punha-os no runtime da cena velha, que a
                    // troca (se houver) substitui.
                    world.insert_resource(PendingSavePatches(patches));
                    if let Some(mut map) = world.get_resource_mut::<crate::input_map::ActionMap>() {
                        for (name, key) in keys {
                            if let Some(action) = crate::input_map::Action::parse(&name) {
                                map.rebind_key(action, &key);
                            }
                        }
                    }
                });
                if let Some(host) = host.as_mut() {
                    crate::luau::game::json_to_game(&host.lua, &game.world_kv);
                }
                // 1.º herói (ver o save): com 2+ heróis o load morria em
                // silêncio — "Jogo carregado." sem herói nenhum reposicionado.
                if let Some((entity, mut transform, mut player)) = heroes.iter_mut().next() {
                    let position = game.position.filter(|p| p.iter().all(|v| v.is_finite()));
                    let current = world_save_host_request
                        .5
                        .as_deref()
                        .filter(|scene| scene.switchable())
                        .cloned();
                    let scene_of_save = game
                        .scene
                        .clone()
                        .unwrap_or_else(|| crate::project::MAIN_SCENE.to_string());
                    if let Some(current) = &current
                        && current.id.as_deref() != Some(scene_of_save.as_str())
                    {
                        let mut request = crate::scene_loader::ChangeScene::new(
                            scene_of_save,
                            match position {
                                Some(p) => crate::scene_loader::Arrival::Position(p.into()),
                                None => crate::scene_loader::Arrival::Entry(None),
                            },
                        );
                        request.came_from = game.came_from.clone();
                        world_save_host_request.4.queue(move |world: &mut World| {
                            if let Err(error) = crate::scene_loader::request_change(world, request)
                            {
                                bevy::log::warn!(
                                    "load: troca para a cena do save falhou: {error:#}"
                                );
                            }
                        });
                    } else if let Some(position) = position {
                        let stale = game.scene.is_none()
                            && current.is_some()
                            && world_save_host_request
                                .6
                                .as_deref()
                                .is_some_and(|t| !t.in_field(position[0], position[2]));
                        if stale {
                            world_save_host_request.4.queue(|world: &mut World| {
                                if let Some((_, pose)) =
                                    crate::scene_loader::find_entry(world, None)
                                {
                                    crate::scene_loader::place_player(
                                        world,
                                        pose.translation,
                                        Some(pose.rotation),
                                    );
                                }
                            });
                        }
                        transform.translation = position.into();
                        // Mesma cena: o `came_from` do save tem de voltar
                        // também, senão o `@back` saía pelo portal da sessão
                        // atual em vez do que o save gravou.
                        let came_from = game.came_from.clone();
                        world_save_host_request.4.queue(move |world: &mut World| {
                            if let Some(mut current) =
                                world.get_resource_mut::<crate::scene_loader::CurrentScene>()
                            {
                                current.came_from = came_from;
                            }
                        });
                        crate::player::settle_after_teleport(
                            &mut world_save_host_request.4,
                            entity,
                            Some(&mut *player),
                        );
                    }
                }
                toasts.write(ScriptToast("Jogo carregado.".into()));
                sfx.write(crate::ambient::SfxEvent {
                    clip: crate::ambient::SfxClip::Load,
                    position: None,
                });
            }
            Err(e) => {
                toasts.write(ScriptToast(format!("Falha ao carregar: {e}")));
                sfx.write(crate::ambient::SfxEvent {
                    clip: crate::ambient::SfxClip::Error,
                    position: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_version_roundtrip_and_future_loads() {
        let game = capture([0.0; 3], (1.0, 1.0, 1.0));
        assert_eq!(game.version, SAVE_VERSION);
        let json = serde_json::to_string(&game).unwrap();
        assert!(json.contains("\"version\""), "{json}");

        let legacy: SaveGame = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.version, 0);

        let future: SaveGame = serde_json::from_str(r#"{"version": 99, "gold": 7}"#).unwrap();
        assert_eq!(future.version, 99);
        assert_eq!(future.gold, 7);
        let chopped: SaveGame =
            serde_json::from_str(r#"{"harvested":["inst-grp-18-0001"]}"#).unwrap();
        assert_eq!(chopped.harvested, vec!["inst-grp-18-0001".to_string()]);
        assert!(legacy.harvested.is_empty());
    }

    #[test]
    fn test_save_roundtrip_preserves_pose_volumes_and_kv() {
        let mut game = capture([12.0, 25.0, -8.0], (0.8, 0.5, 0.9));
        game.scene = Some("main".into());
        game.came_from = Some(("wilds".into(), "gate".into()));
        game.world_kv
            .insert("item.gold".into(), serde_json::json!(80));
        let json = serde_json::to_string(&game).unwrap();
        let loaded: SaveGame = serde_json::from_str(&json).unwrap();
        let mut mixer = AudioMixerSettings::default();
        apply_save(&loaded, &mut mixer);
        assert!((mixer.master - 0.8).abs() < 1e-6);
        assert_eq!(loaded.position, Some([12.0, 25.0, -8.0]));
        assert_eq!(loaded.scene.as_deref(), Some("main"));
        assert_eq!(loaded.came_from, Some(("wilds".into(), "gate".into())));
        assert_eq!(
            loaded.world_kv.get("item.gold"),
            Some(&serde_json::json!(80))
        );
        assert_eq!(loaded.gold, 0, "o capture já não grava ouro de recurso");
    }

    #[test]
    fn test_partial_save_keeps_session_state() {
        let loaded: SaveGame = serde_json::from_str(r#"{ "gold": 5 }"#).unwrap();
        assert_eq!(loaded.health, None);
        assert_eq!(loaded.xp, None);
        assert_eq!(loaded.position, None);
        assert_eq!(loaded.gold, 5, "o campo antigo ainda desserializa");
        let mut mixer = AudioMixerSettings {
            master: 0.6,
            music: 0.4,
            sfx: 0.8,
            ..AudioMixerSettings::default()
        };
        apply_save(&loaded, &mut mixer);
        assert_eq!((mixer.master, mixer.music, mixer.sfx), (0.6, 0.4, 0.8));
    }

    /// A chave é por ficheiro de mundo: `world.xml` fica com o nome da pasta
    /// (saves existentes) e irmãos na mesma pasta não colidem. O default
    /// pós-bake (`world.static.xml`) partilha a chave do `world.xml` de onde
    /// veio — só o sufixo exato `.static` do stem conta.
    #[test]
    fn test_world_save_key_per_world_file() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("meu-jogo");
        std::fs::create_dir_all(&game).unwrap();
        for name in [
            "world.xml",
            "world.static.xml",
            "qa-agua.xml",
            "qa-cliffs.xml",
            "qa-static.xml",
            "outpost.static.xml",
        ] {
            std::fs::write(game.join(name), "<world/>").unwrap();
        }
        assert_eq!(
            world_save_key(&game.join("world.xml")).as_deref(),
            Some("meu-jogo")
        );
        // Continuidade arranque antigo ↔ default pós-bake: mesma chave.
        assert_eq!(
            world_save_key(&game.join("world.static.xml")).as_deref(),
            Some("meu-jogo"),
            "world.static.xml partilha o save do world.xml"
        );
        assert_eq!(
            world_save_key(&game.join("qa-agua.xml")).as_deref(),
            Some("meu-jogo-qa-agua")
        );
        // "static" no meio do nome NÃO é o sufixo do stem.
        assert_eq!(
            world_save_key(&game.join("qa-static.xml")).as_deref(),
            Some("meu-jogo-qa-static")
        );
        // Um `.static` genuíno sem par: o sufixo sai, o resto do nome conta.
        assert_eq!(
            world_save_key(&game.join("outpost.static.xml")).as_deref(),
            Some("meu-jogo-outpost")
        );
        assert_ne!(
            world_save_key(&game.join("qa-agua.xml")),
            world_save_key(&game.join("qa-cliffs.xml"))
        );
        let save = save_path_for(Some("meu-jogo-qa-agua"), Some(dir.path()));
        assert_eq!(save, dir.path().join("meu-jogo-qa-agua.save.json"));
        assert_eq!(
            save_path_for(None, Some(dir.path())),
            dir.path().join(SAVE_FILENAME)
        );
    }

    /// Um save antigo (sem campos de skills) carrega sem falhar.
    #[test]
    fn test_slot_paths_and_listing() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = save_path_for(Some("vael"), Some(dir.path()));
        let auto = save_path_for_slot(Some("vael"), Some(dir.path()), Some(0));
        let slot1 = save_path_for_slot(Some("vael"), Some(dir.path()), Some(1));
        assert_eq!(
            save_path_for_slot(Some("vael"), Some(dir.path()), None),
            legacy,
            "sem slot continua no path legado"
        );
        assert_eq!(auto, dir.path().join("vael.auto.save.json"));
        assert_eq!(slot1, dir.path().join("vael.slot1.save.json"));

        let mut game = capture([1.0, 2.0, 3.0], (1.0, 1.0, 1.0));
        game.level = 7;
        game.gold = 123;
        game.saved_at = Some(42.0);
        game.playtime = Some(3_600.0);
        game.title = Some("Discórdia".into());
        save_to_disk(&slot1, &game).unwrap();

        let list = list_save_slots(Some("vael"), Some(dir.path()));
        assert_eq!(list.len(), 4, "autosave + 3 slots");
        let auto_row = list.iter().find(|row| row.slot == 0).unwrap();
        assert!(!auto_row.exists, "autosave ainda não existe");
        let row = list.iter().find(|row| row.slot == 1).unwrap();
        assert!(row.exists);
        assert_eq!(row.level, 7);
        assert_eq!(row.gold, 123);
        assert_eq!(row.saved_at, Some(42.0));
        assert_eq!(row.playtime, Some(3_600.0));
        assert_eq!(row.title.as_deref(), Some("Discórdia"));
    }

    /// Um save antigo (sem campos de skills) carrega sem falhar.
    #[test]
    fn test_old_save_without_skills_loads() {
        let json = r#"{ "xp": [10, 100], "health": [100.0, 100.0],
            "position": [0.0, 0.0, 0.0], "gold": 5, "wood": 0, "stone": 0,
            "items": {}, "quest_states": {}, "quest_done": [],
            "nota_marked": [], "volumes": [1.0, 1.0, 1.0] }"#;
        let loaded: SaveGame = serde_json::from_str(json).unwrap();
        assert!(loaded.skill_learned.is_empty());
        assert_eq!(loaded.skill_points, 0);
    }

    #[test]
    fn test_save_to_disk_and_back() {
        let dir = std::env::temp_dir().join(format!("viber-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.json");
        let game = SaveGame {
            gold: 42,
            ..Default::default()
        };
        save_to_disk(&path, &game).unwrap();
        let loaded = load_from_disk(&path).unwrap();
        assert_eq!(loaded.gold, 42);
        assert!(!dir.join("s.json.tmp").exists(), "sem .tmp órfão");
        std::fs::remove_file(&path).ok();
    }

    /// Escrita atómica: quando a escrita do `.tmp` falha (aqui, um diretório
    /// no lugar do tmp), o rename não acontece e o save ANTERIOR fica intacto.
    #[test]
    fn test_save_to_disk_failure_preserves_previous_save() {
        let dir = std::env::temp_dir().join(format!("viber-save-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.json");
        let game = SaveGame {
            gold: 42,
            ..Default::default()
        };
        save_to_disk(&path, &game).unwrap();
        // Sabotar o caminho do tmp com um diretório: File::create falha
        // ANTES do rename.
        let tmp = dir.join("s.json.tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let bad = SaveGame {
            gold: 999,
            ..Default::default()
        };
        assert!(save_to_disk(&path, &bad).is_err());
        assert_eq!(
            load_from_disk(&path).unwrap().gold,
            42,
            "save anterior sobrevive à falha"
        );
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&tmp).ok();
    }

    /// Preset `gameplay: none`: sem Vault/QuestLog/Nota/SkillTree o capture
    /// grava os campos por omissão e o `world_kv` viaja — é a promessa do
    /// `viber.game()` persistir em qualquer preset.
    #[test]
    fn test_capture_and_apply_without_rpg_resources() {
        let mut game = capture([3.0, 1.5, -2.0], (1.0, 1.0, 1.0));
        game.world_kv
            .insert("score".to_string(), serde_json::json!(4));
        assert_eq!(game.gold, 0);
        assert!(game.quest_states.is_empty());
        // Roundtrip por JSON: o world_kv volta e o apply não panica sem
        // recursos RPG (só o mixer é aplicado).
        let json = serde_json::to_string(&game).unwrap();
        let loaded: SaveGame = serde_json::from_str(&json).unwrap();
        let mut mixer = AudioMixerSettings::default();
        apply_save(&loaded, &mut mixer);
        assert_eq!(loaded.world_kv.get("score"), Some(&serde_json::json!(4)));
        assert_eq!(loaded.position, Some([3.0, 1.5, -2.0]));
        assert!((mixer.master - 1.0).abs() < 1e-6);
    }

    /// Save ANTIGO (com campos RPG) carrega num mundo `gameplay: none` sem
    /// erro: os campos extra são simplesmente ignorados pelo apply.
    #[test]
    fn test_rpg_save_loads_in_none_preset() {
        let json = r#"{ "xp": [10, 100], "health": [88.0, 100.0],
            "position": [1.0, 2.0, 3.0], "gold": 250, "wood": 3, "stone": 1,
            "items": {"potion": 2}, "quest_states": {"city_wolves": [1, []]},
            "quest_done": ["x"], "nota_marked": ["peaks-cairn-1"],
            "skill_learned": ["vitality1"], "skill_points": 2, "level": 3,
            "volumes": [0.5, 0.5, 0.5],
            "world_kv": {"score": 9} }"#;
        let loaded: SaveGame = serde_json::from_str(json).unwrap();
        let mut mixer = AudioMixerSettings::default();
        apply_save(&loaded, &mut mixer);
        assert_eq!(loaded.gold, 250);
        assert_eq!(loaded.world_kv.get("score"), Some(&serde_json::json!(9)));
        assert!((mixer.master - 0.5).abs() < 1e-6);
    }

    /// Os recortes de terreno de um load ESPERAM por terreno vivo: importá-los
    /// no runtime da cena velha perdia-os na troca (ou caía no campo pequeno
    /// da cena errada). Sem `TerrainRuntime` fica pendente; com um, importa e
    /// limpa.
    #[test]
    fn test_pending_save_patches_wait_for_a_live_terrain() {
        use bevy::ecs::system::RunSystemOnce as _;

        fn small_runtime() -> crate::terrain::runtime::TerrainRuntime {
            let spec = crate::terrain::spec::TerrainSpec {
                world_size: 128.0,
                max_height: 40.0,
                seed: 5,
                ..crate::terrain::spec::TerrainSpec::default()
            };
            let map = crate::terrain::heightmap::HeightMapU16::procedural(
                &spec,
                spec.resolution.max(1) as usize,
            );
            let grid = crate::terrain::brush::BrushGrid::from_height_map(
                &map,
                spec.world_size,
                spec.max_height,
                spec.height_smoothing,
            )
            .expect("grid builds");
            crate::terrain::runtime::TerrainRuntime {
                spec,
                grid: std::sync::Arc::new(grid),
                water: vec![],
                roads: vec![],
                pads: vec![],
                voxel: std::sync::Arc::new(crate::terrain::voxel::VoxelField::default()),
                deltas: std::sync::Arc::new(crate::terrain::delta::DeltaGrid::default()),
                paint: std::sync::Arc::new(crate::terrain::paint::PaintGrid::default()),
                splat_tuning: Default::default(),
            }
        }

        let mut app = App::new();
        app.init_resource::<PendingSavePatches>();
        app.world_mut()
            .resource_mut::<PendingSavePatches>()
            .0
            .push(crate::terrain::delta::DeltaPatch {
                min: [0.0, 0.0],
                step: [1.0, 1.0],
                cols: 2,
                rows: 2,
                data: vec![1.0, 2.0, 3.0, 4.0],
            });
        // Sem terreno vivo (interior/menu à espera): fica pendente.
        app.world_mut()
            .run_system_once(apply_pending_save_patches)
            .unwrap();
        assert_eq!(
            app.world().resource::<PendingSavePatches>().0.len(),
            1,
            "sem terreno os recortes esperam"
        );
        // Terreno vivo: importa e limpa o pendente.
        app.insert_resource(small_runtime());
        app.world_mut()
            .run_system_once(apply_pending_save_patches)
            .unwrap();
        assert!(app.world().resource::<PendingSavePatches>().0.is_empty());
        let runtime = app
            .world()
            .resource::<crate::terrain::runtime::TerrainRuntime>();
        assert_eq!(
            runtime.deltas.export_patches().len(),
            1,
            "o recorte entrou no runtime vivo"
        );
    }
}
