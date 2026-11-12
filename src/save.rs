//! Save/load & opções (loop 7 do port simple-rpg) — o análogo nativo do
//! `SaveLoadPlugin` original (localStorage + msgpackr):
//!
//! - **[`SaveGame`]**: envelope genérico (pose, volumes, cena, idioma,
//!   recortes de terreno) + `world_kv` (o `viber.game()` do Luau) + uma
//!   secção opaca `game` para o jogo. Nada de XP/ouro/quests hardcoded.
//! - **Disco**: `~/.local/share/viber/<world>.save.json` (fallback
//!   `./<world>.save.json` sem HOME).
//! - **UI**: no modal [Q], tab **Opções** — ↑↓ escolhe a linha (volumes
//!   master/música/sfx e Save/Load), ←/→ ajusta o volume ±10 %, [J] grava
//!   e [L] carrega.
//!
//! Os `defs` das quests são estáticos (embutidos) — só os ESTADOS viajam no
//! ficheiro, como os serializers por módulo da referência.

pub use bevy_save_slots::*;

use bevy::prelude::*;

use crate::luau::ScriptToast;
use crate::menus::MenusOpen;
use crate::music::AudioMixerSettings;
use crate::player::Player;

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
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
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
    if loader
        .as_deref()
        .is_some_and(crate::scene_loader::SceneLoader::busy)
    {
        return;
    }
    std::sync::Arc::make_mut(&mut runtime.deltas).import_patches(&pending.0);
    pending.0.clear();
}

/// Pedido de save/load vindo de SCRIPT (`viber.save()` / `viber.load()`) —
/// o `LuaScriptHost` não pode chamar o sistema de save de dentro do Luau, e o
/// host sem ficheiro `world_kv` não teria como persistir num mundo
/// `gameplay: none`. Consumido (e limpo) pelo `options_system`.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct SaveRequest {
    pub save: bool,
    pub load: bool,
    /// `None` = save legado; `0` = autosave; `1..` = slot manual.
    pub slot: Option<u32>,
}

/// Linha selecionada na tab Opções (0=master, 1=música, 2=sfx, 3=save,
/// 4=load).
#[derive(Debug, Clone, Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct OptionsRows {
    pub selected: usize,
}

pub const OPTIONS_ROWS: usize = 5;

/// Aplica volumes. O estado do JOGO (ouro, quests, talentos, …) viaja no
/// `world_kv`/secção `game` e é o `json_to_game` que o repõe em Lua; aqui só
/// se tocam os buses de áudio.
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
    action_owners: Option<Res<crate::ui::actions::UiActionOwners>>,
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
        // Ação reclamada por um script (`viber.own_action`): o handler nativo
        // cala-se e a lógica corre em Lua (a ação já foi encaminhada por
        // `forward_ui_actions`).
        if action_owners
            .as_deref()
            .is_some_and(|owners| owners.owns(&action.name))
        {
            continue;
        }
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
            // `level`/`gold` NÃO se copiam para o envelope: `SaveGame::level()`
            // e `gold()` lêem-nos do `world_kv` na altura de listar os slots.
            game.saved_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs_f64());
            game.playtime = game
                .world_kv
                .get("playtime_secs")
                .and_then(serde_json::Value::as_f64);
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

        // Um save MAIS NOVO que a engine: os campos que não conhecemos ficam
        // na secção opaca, não são apagados.
        let mut future: SaveGame =
            serde_json::from_str(r#"{"version": 99, "gold": 7, "new_field": [1]}"#).unwrap();
        assert_eq!(future.version, 99);
        assert_eq!(future.legacy.get("gold"), Some(&serde_json::json!(7)));
        future.migrate_legacy();
        assert_eq!(future.gold(), 7);
        assert!(future.legacy.is_empty());
        assert_eq!(
            future
                .game
                .as_ref()
                .and_then(|g| g.get("legacy"))
                .and_then(|l| l.get("new_field")),
            Some(&serde_json::json!([1])),
            "campos desconhecidos viajam para a secção do jogo"
        );
        let mut chopped: SaveGame =
            serde_json::from_str(r#"{"harvested":["inst-grp-18-0001"]}"#).unwrap();
        chopped.migrate_legacy();
        assert_eq!(
            chopped
                .game
                .as_ref()
                .and_then(|g| g.get("legacy"))
                .and_then(|l| l.get("harvested")),
            Some(&serde_json::json!(["inst-grp-18-0001"]))
        );
        assert!(legacy.legacy.is_empty());
    }

    #[test]
    fn test_save_roundtrip_preserves_pose_volumes_and_kv() {
        let mut game = capture([12.0, 25.0, -8.0], (0.8, 0.5, 0.9));
        game.scene = Some("main".into());
        game.came_from = Some(("wilds".into(), "gate".into()));
        game.world_kv
            .insert("item.gold".into(), serde_json::json!(80));
        game.game = Some(serde_json::json!({ "quests": { "wolves": 2 } }));
        let json = serde_json::to_string(&game).unwrap();
        assert!(
            !json.contains("\"legacy\""),
            "o campo de migração nunca é escrito"
        );
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
        assert_eq!(
            loaded.game,
            Some(serde_json::json!({ "quests": { "wolves": 2 } })),
            "a secção opaca do jogo faz roundtrip inteira"
        );
        assert_eq!(loaded.gold(), 0, "o capture não grava ouro no envelope");
    }

    #[test]
    fn test_partial_save_keeps_session_state() {
        let loaded: SaveGame = serde_json::from_str(r#"{ "gold": 5 }"#).unwrap();
        assert_eq!(loaded.position, None);
        assert_eq!(loaded.volumes, None);
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
        game.world_kv
            .insert("save_level".into(), serde_json::json!(7));
        game.world_kv
            .insert("save_gold".into(), serde_json::json!(123));
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

    /// Um save antigo (campos de RPG no topo) carrega, migra para a secção
    /// `game` e é relido pelo `world_kv` — nada é apagado.
    #[test]
    fn test_old_save_fields_migrate_into_the_game_section() {
        let json = r#"{ "xp": [10, 100], "health": [100.0, 100.0],
            "position": [0.0, 0.0, 0.0], "gold": 5, "wood": 0, "stone": 0,
            "items": {}, "quest_states": {}, "quest_done": [],
            "nota_marked": [], "volumes": [1.0, 1.0, 1.0] }"#;
        let mut loaded: SaveGame = serde_json::from_str(json).unwrap();
        loaded.migrate_legacy();
        assert_eq!(loaded.gold(), 5);
        assert_eq!(loaded.world_kv.get("wood"), Some(&serde_json::json!(0)));
        let legacy = loaded.game.as_ref().unwrap().get("legacy").unwrap();
        assert_eq!(legacy.get("xp"), Some(&serde_json::json!([10, 100])));
        assert_eq!(legacy.get("quest_states"), Some(&serde_json::json!({})));
        // Gravar por cima escreve só o schema novo: `quest_states` já não é
        // uma chave de topo (vive dentro de `game.legacy`).
        let json = serde_json::to_string(&loaded).unwrap();
        let top: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(top.get("quest_states").is_none(), "{json}");
    }

    #[test]
    fn test_save_to_disk_and_back() {
        let dir = std::env::temp_dir().join(format!("viber-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.json");
        let mut game = SaveGame::default();
        game.world_kv
            .insert("save_gold".into(), serde_json::json!(42));
        save_to_disk(&path, &game).unwrap();
        let loaded = load_from_disk(&path).unwrap();
        assert_eq!(loaded.gold(), 42);
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
        let mut game = SaveGame::default();
        game.world_kv
            .insert("save_gold".into(), serde_json::json!(42));
        save_to_disk(&path, &game).unwrap();
        // Sabotar o caminho do tmp com um diretório: File::create falha
        // ANTES do rename.
        let tmp = dir.join("s.json.tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let mut bad = SaveGame::default();
        bad.world_kv
            .insert("save_gold".into(), serde_json::json!(999));
        assert!(save_to_disk(&path, &bad).is_err());
        assert_eq!(
            load_from_disk(&path).unwrap().gold(),
            42,
            "save anterior sobrevive à falha"
        );
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&tmp).ok();
    }

    /// Sem recursos de RPG no mundo, o `capture` grava o envelope por omissão
    /// e o `world_kv` viaja — é a promessa do `viber.game()` persistir em
    /// qualquer preset.
    #[test]
    fn test_capture_and_apply_without_game_resources() {
        let mut game = capture([3.0, 1.5, -2.0], (1.0, 1.0, 1.0));
        game.world_kv
            .insert("score".to_string(), serde_json::json!(4));
        assert!(
            game.game.is_none(),
            "sem secção de jogo até o jogo escrever"
        );
        // Roundtrip por JSON: o world_kv volta e o apply não panica sem
        // recursos de jogo (só o mixer é aplicado).
        let json = serde_json::to_string(&game).unwrap();
        let loaded: SaveGame = serde_json::from_str(&json).unwrap();
        let mut mixer = AudioMixerSettings::default();
        apply_save(&loaded, &mut mixer);
        assert_eq!(loaded.world_kv.get("score"), Some(&serde_json::json!(4)));
        assert_eq!(loaded.position, Some([3.0, 1.5, -2.0]));
        assert!((mixer.master - 1.0).abs() < 1e-6);
    }

    /// Save ANTIGO (com campos de RPG no topo) carrega num mundo sem esses
    /// recursos sem erro: a migração preserva tudo e o apply só toca o mixer.
    #[test]
    fn test_legacy_game_save_loads_without_game_resources() {
        let json = r#"{ "xp": [10, 100], "health": [88.0, 100.0],
            "position": [1.0, 2.0, 3.0], "gold": 250, "wood": 3, "stone": 1,
            "items": {"potion": 2}, "quest_states": {"city_wolves": [1, []]},
            "quest_done": ["x"], "nota_marked": ["peaks-cairn-1"],
            "skill_learned": ["vitality1"], "skill_points": 2, "level": 3,
            "volumes": [0.5, 0.5, 0.5],
            "world_kv": {"score": 9} }"#;
        let mut loaded: SaveGame = serde_json::from_str(json).unwrap();
        loaded.migrate_legacy();
        let mut mixer = AudioMixerSettings::default();
        apply_save(&loaded, &mut mixer);
        assert_eq!(loaded.gold(), 250);
        assert_eq!(loaded.level(), 3);
        assert_eq!(loaded.world_kv.get("score"), Some(&serde_json::json!(9)));
        assert_eq!(
            loaded.world_kv.get("skill_points"),
            Some(&serde_json::json!(2))
        );
        assert!((mixer.master - 0.5).abs() < 1e-6);
        // O que não é escalar (inventário, quests) fica acessível na secção
        // opaca, com a forma original.
        let legacy = loaded.game.as_ref().unwrap().get("legacy").unwrap();
        assert_eq!(legacy.get("items"), Some(&serde_json::json!({"potion": 2})));
        assert_eq!(
            legacy.get("quest_states"),
            Some(&serde_json::json!({"city_wolves": [1, []]}))
        );
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
        app.world_mut().resource_mut::<PendingSavePatches>().0.push(
            crate::terrain::delta::DeltaPatch {
                min: [0.0, 0.0],
                step: [1.0, 1.0],
                cols: 2,
                rows: 2,
                data: vec![1.0, 2.0, 3.0, 4.0],
            },
        );
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
