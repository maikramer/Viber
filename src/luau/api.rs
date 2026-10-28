//! Instalação da tabela `viber` (API de jogo dos scripts): percepção,
//! movimento com snap no terreno, IA primitiva, combate/progressão, quests,
//! vault, interação. Os grupos NOVOS vivem em módulos próprios e são
//! compostos aqui (`input`, `events`, `timers`, `entity`, `game`).

use bevy::prelude::*;
use mlua::{Lua, Table};

use super::commands::ScriptCommand;
use super::ctx::ScriptCtx;
use super::input::key_code_from_str;
use super::sfx::{SFX_NAME_REGISTRY, sfx_clip_from_str};
use super::{decision, entity, events, game, input, nlu, props, terrain, timers};

/// Installs the `viber` API table on the VM globals:
/// `log(msg)`, `time()`, `position()`, `set_position(x, y, z)`,
/// `distance_to_player()`.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let api = lua.create_table()?;

    // viber.log(msg) — engine log + ring buffer.
    api.set(
        "log",
        lua.create_function(|lua, msg: String| {
            let entity = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.entity)
                .map(|e| format!("{e:?}"))
                .unwrap_or_else(|| "pre-activate".into());
            info!(target: "viber::luau", "[{entity}] {msg}");
            if let Some(mut ctx) = lua.app_data_mut::<ScriptCtx>() {
                ctx.push_log(msg);
            }
            Ok(())
        })?,
    )?;

    // viber.time() — seconds since engine startup.
    api.set(
        "time",
        lua.create_function(|lua, ()| {
            let elapsed = lua
                .app_data_ref::<ScriptCtx>()
                .map(|ctx| ctx.elapsed)
                .unwrap_or_default();
            Ok(elapsed)
        })?,
    )?;

    // viber.position() -> x, y, z — start-of-frame snapshot.
    api.set(
        "position",
        lua.create_function(|lua, ()| {
            let origin = lua
                .app_data_ref::<ScriptCtx>()
                .map(|ctx| ctx.origin)
                .unwrap_or_default();
            Ok((origin.x, origin.y, origin.z))
        })?,
    )?;

    // viber.set_position(x, y, z) — queues a command applied post-frame.
    api.set(
        "set_position",
        lua.create_function(|lua, (x, y, z): (f32, f32, f32)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.set_position: coordenadas não finitas (NaN/inf)",
                ));
            }
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let Some(entity) = ctx.entity else {
                return Err(mlua::Error::runtime(
                    "viber.set_position outside on_update (no owning entity)",
                ));
            };
            ctx.pending.push((entity, Vec3::new(x, y, z)));
            Ok(())
        })?,
    )?;

    // viber.distance_to_player() -> number | nil (nil = no player).
    api.set(
        "distance_to_player",
        lua.create_function(|lua, ()| {
            let (origin, player) = lua
                .app_data_ref::<ScriptCtx>()
                .map(|ctx| (ctx.origin, ctx.player))
                .unwrap_or((Vec3::ZERO, None));
            Ok(player.map(|p| p.distance(origin)))
        })?,
    )?;

    // ── Estado por entidade ─────────────────────────────────────────
    // Chunks são partilhados entre entidades; o estado de CADA entidade
    // vive numa tabela separada (`states[entity_bits]`), criada à pressa.
    let state_fn = lua.create_function(|lua, ()| {
        let entity = lua
            .app_data_ref::<ScriptCtx>()
            .and_then(|ctx| ctx.entity)
            .ok_or_else(|| mlua::Error::runtime("viber.state fora de on_update"))?;
        let key = entity.to_bits() as i64;
        let states: Table = lua.named_registry_value("viber_states")?;
        let table = states.raw_get::<Table>(key).or_else(|_| {
            let fresh = lua.create_table()?;
            let _ = states.raw_set(key, fresh.clone());
            Ok::<Table, mlua::Error>(fresh)
        })?;
        Ok(table)
    })?;
    api.set("state", state_fn)?;

    // viber.self_name() -> string
    api.set(
        "self_name",
        lua.create_function(|lua, ()| {
            let name = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.entity)
                .map(|e| format!("{e:?}"))
                .unwrap_or_default();
            Ok(name)
        })?,
    )?;

    // viber.home() -> x, z — posição de SPAWN (centro do wander), lida
    // da tabela de estado gravada em `activate_at`; ctx.origin é a
    // posição ACTUAL e fazia scripts de leash orbitarem o sítio corrente.
    api.set(
        "home",
        lua.create_function(|lua, ()| {
            let (entity, fallback) = {
                let ctx = lua.app_data_ref::<ScriptCtx>();
                (
                    ctx.as_ref().and_then(|c| c.entity),
                    ctx.as_ref().map(|c| c.origin).unwrap_or_default(),
                )
            };
            if let Some(entity) = entity {
                let key = entity.to_bits() as i64;
                let states: Table = lua.named_registry_value("viber_states")?;
                if let Ok(state) = states.raw_get::<Table>(key)
                    && let Ok(home) = state.raw_get::<Table>("home")
                {
                    let x: f32 = home.raw_get("x")?;
                    let z: f32 = home.raw_get("z")?;
                    return Ok((x, z));
                }
            }
            Ok((fallback.x, fallback.z))
        })?,
    )?;

    // viber.player_position() -> x, y, z | nil
    api.set(
        "player_position",
        lua.create_function(|lua, ()| {
            let player = lua.app_data_ref::<ScriptCtx>().and_then(|ctx| ctx.player);
            match player {
                Some(p) => Ok((true, p.x, p.y, p.z)),
                None => Ok((false, 0.0, 0.0, 0.0)),
            }
        })?,
    )?;

    // viber.player_hp() -> ok, cur, max — snapshot do HP do herói
    // (o healer usa para recusar a cura com um gesto "no").
    api.set(
        "player_hp",
        lua.create_function(|lua, ()| {
            let hp = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.player_hp);
            match hp {
                Some((cur, max)) => Ok((true, cur, max)),
                None => Ok((false, 0.0, 0.0)),
            }
        })?,
    )?;

    // viber.player_id() -> bits | nil — a entidade do herói, para
    // `entity_damage`/`entity_heal` no próprio jogador.
    api.set(
        "player_id",
        lua.create_function(|lua, ()| {
            Ok(lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.player_bits))
        })?,
    )?;

    // viber.ground_below(x, y, z) -> y | nil — a superfície sólida mais
    // alta em ou abaixo de `y` nesta coluna. Acima do mundo = o topo;
    // dentro de uma gruta = o piso da gruta; sob um arco = o chão do
    // vão. `nil` quando não há terreno sólido abaixo. É a query que
    // deixa uma criatura andar num túnel sem ser sentada na colina por
    // cima (os snaps de move_towards/move_by continuam a usar o topo).
    api.set(
        "ground_below",
        lua.create_function(|lua, (x, y, z): (f32, f32, f32)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.ground_below: argumentos não finitos (NaN/inf)",
                ));
            }
            let ground = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.terrain.clone())
                .and_then(|reader| reader.voxel.surface_below(&reader.base(), x, z, y));
            Ok(ground)
        })?,
    )?;

    // viber.move_towards(x, z, speed) — passo deste frame na direção do
    // ponto; a engine senta o Y no terreno ao aplicar.
    api.set(
        "move_towards",
        lua.create_function(|lua, (x, z, speed): (f32, f32, f32)| {
            if !(x.is_finite() && z.is_finite() && speed.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.move_towards: argumentos não finitos (NaN/inf)",
                ));
            }
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let Some(entity) = ctx.entity else {
                return Err(mlua::Error::runtime("viber.move_towards fora de on_update"));
            };
            ctx.commands.push(ScriptCommand::MoveTowards {
                entity,
                goal: Vec2::new(x, z),
                speed,
            });
            Ok(())
        })?,
    )?;

    // viber.move_by(dx, dz) — passo relativo direto.
    api.set(
        "move_by",
        lua.create_function(|lua, (dx, dz): (f32, f32)| {
            if !(dx.is_finite() && dz.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.move_by: delta não finito (NaN/inf)",
                ));
            }
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let Some(entity) = ctx.entity else {
                return Err(mlua::Error::runtime("viber.move_by fora de on_update"));
            };
            ctx.commands
                .push(ScriptCommand::MoveBy(entity, Vec2::new(dx, dz)));
            Ok(())
        })?,
    )?;

    // viber.face_towards(x, z) / viber.face_player()
    let face_towards = |lua: &Lua| {
        lua.create_function(|lua, (x, z): (f32, f32)| {
            if !(x.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.face_towards: coordenadas não finitas (NaN/inf)",
                ));
            }
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let Some(entity) = ctx.entity else {
                return Err(mlua::Error::runtime("viber.face fora de on_update"));
            };
            let target = Vec3::new(x, ctx.origin.y, z);
            ctx.commands
                .push(ScriptCommand::FaceTowards(entity, target));
            Ok(())
        })
    };
    api.set("face_towards", face_towards(lua)?)?;
    api.set(
        "face_player",
        lua.create_function(|lua, ()| {
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let (Some(entity), Some(player)) = (ctx.entity, ctx.player) else {
                return Ok(());
            };
            ctx.commands
                .push(ScriptCommand::FaceTowards(entity, player));
            Ok(())
        })?,
    )?;

    // viber.on_road(x, z) / viber.in_water(x, z) — consultas ao mundo
    // carvado. Existiam na engine (`TerrainRuntime`) mas o Luau só tinha o
    // `ground_below`; sem elas um script não conseguia sequer perguntar
    // "estou na estrada?".
    api.set(
        "on_road",
        lua.create_function(|lua, (x, z): (f32, f32)| {
            let ctx = lua.app_data_ref::<ScriptCtx>();
            let Some(surfaces) = ctx.as_ref().and_then(|c| c.surfaces.as_ref()) else {
                return Ok(false);
            };
            Ok(surfaces.on_road(x, z))
        })?,
    )?;
    api.set(
        "in_water",
        lua.create_function(|lua, (x, z): (f32, f32)| {
            let ctx = lua.app_data_ref::<ScriptCtx>();
            let Some(surfaces) = ctx.as_ref().and_then(|c| c.surfaces.as_ref()) else {
                return Ok(false);
            };
            Ok(surfaces.in_water(x, z))
        })?,
    )?;
    // viber.road(x, z) -> {y, s, length, half_width, lateral, deck, heading, on} | nil
    // Projeção na estrada mais próxima, mesmo fora da fita. `s`/`length` em
    // metros. `y` é o tabuleiro quando o ponto está NUM vão suspenso; senão
    // é o terreno. Sem estradas = nil.
    api.set(
        "road",
        lua.create_function(|lua, (x, z): (f32, f32)| {
            if !(x.is_finite() && z.is_finite()) {
                return Ok(mlua::Value::Nil);
            }
            let (sample, ground) = {
                let ctx = lua
                    .app_data_ref::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded");
                let Some(sample) = ctx.surfaces.as_ref().and_then(|s| s.nearest_road(x, z)) else {
                    return Ok(mlua::Value::Nil);
                };
                let ground = ctx.terrain.as_ref().map(|terrain| {
                    let base = terrain.base();
                    if terrain.voxel.is_flat() {
                        crate::terrain::mesh::HeightField::sample(&base, x, z)
                    } else {
                        terrain.voxel.surface_top(&base, x, z)
                    }
                });
                (sample, ground)
            };
            let on = sample.center_dist <= sample.half_width + 1e-3;
            let y = if sample.deck && on {
                sample.deck_y
            } else {
                ground.unwrap_or(sample.deck_y)
            };
            let table = lua.create_table()?;
            table.raw_set("y", y)?;
            table.raw_set("s", sample.s)?;
            table.raw_set("length", sample.length)?;
            table.raw_set("half_width", sample.half_width)?;
            table.raw_set("lateral", sample.lateral)?;
            table.raw_set("deck", sample.deck && on)?;
            table.raw_set("heading", sample.heading)?;
            table.raw_set("on", on)?;
            table.raw_set("cx", sample.cx)?;
            table.raw_set("cz", sample.cz)?;
            Ok(mlua::Value::Table(table))
        })?,
    )?;
    // viber.road_at(s) -> o mesmo formato, no arco `s` (m) da estrada mais
    // longa. Serve o look-ahead da IA e a grelha. Sem estradas = nil.
    api.set(
        "road_at",
        lua.create_function(|lua, s: f32| {
            if !s.is_finite() {
                return Ok(mlua::Value::Nil);
            }
            let (sample, ground) = {
                let ctx = lua
                    .app_data_ref::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded");
                let Some(sample) = ctx
                    .surfaces
                    .as_ref()
                    .and_then(|surfaces| surfaces.road_at(s))
                else {
                    return Ok(mlua::Value::Nil);
                };
                let ground = ctx
                    .terrain
                    .as_ref()
                    .map(|terrain| terrain.sample(sample.cx, sample.cz));
                (sample, ground)
            };
            let y = if sample.deck {
                sample.deck_y
            } else {
                ground.unwrap_or(sample.deck_y)
            };
            let table = lua.create_table()?;
            table.raw_set("y", y)?;
            table.raw_set("s", sample.s)?;
            table.raw_set("length", sample.length)?;
            table.raw_set("half_width", sample.half_width)?;
            table.raw_set("lateral", sample.lateral)?;
            table.raw_set("deck", sample.deck)?;
            table.raw_set("heading", sample.heading)?;
            table.raw_set("on", true)?;
            table.raw_set("cx", sample.cx)?;
            table.raw_set("cz", sample.cz)?;
            table.raw_set("x", sample.cx)?;
            table.raw_set("z", sample.cz)?;
            Ok(mlua::Value::Table(table))
        })?,
    )?;
    // viber.set_pose(x, y, z, yaw [, id]) — pose LOCAL. Sem id = a própria
    // entidade. A escala mantém-se. Com own_system("locomotion") o herói
    // deixa de andar a pé.
    api.set(
        "set_pose",
        lua.create_function(
            |lua, (x, y, z, yaw, id): (f32, f32, f32, f32, Option<i64>)| {
                if !(x.is_finite() && y.is_finite() && z.is_finite() && yaw.is_finite()) {
                    return Err(mlua::Error::runtime(
                        "viber.set_pose: x/y/z/yaw têm de ser finitos",
                    ));
                }
                let mut ctx = lua
                    .app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded");
                let entity = match id {
                    Some(bits) => super::entity::entity_from_id(bits)?,
                    None => ctx.entity.ok_or_else(|| {
                        mlua::Error::runtime("viber.set_pose fora de on_update (sem id)")
                    })?,
                };
                ctx.commands.push(ScriptCommand::SetPose {
                    entity,
                    pos: Vec3::new(x, y, z),
                    yaw,
                });
                Ok(())
            },
        )?,
    )?;
    // viber.sound_loop(id, clip [, {pitch, volume, x, y, z}]) /
    // viber.sound_stop(id). `clip` = nome do registry ou caminho sob sfx_dir.
    api.set(
        "sound_loop",
        lua.create_function(|lua, (id, clip, opts): (String, String, Option<Table>)| {
            if id.is_empty() || clip.is_empty() {
                return Err(mlua::Error::runtime(
                    "viber.sound_loop: id e clip não podem ser vazios",
                ));
            }
            let mut pitch = 1.0_f64;
            let mut volume = 1.0_f32;
            let mut position = None;
            let origin_y = lua
                .app_data_ref::<ScriptCtx>()
                .map(|ctx| ctx.origin.y)
                .unwrap_or(0.0);
            if let Some(opts) = opts {
                if let Ok(value) = opts.get::<f64>("pitch") {
                    if !value.is_finite() || value <= 0.0 {
                        return Err(mlua::Error::runtime(
                            "viber.sound_loop: pitch tem de ser finito e > 0",
                        ));
                    }
                    pitch = value;
                }
                if let Ok(value) = opts.get::<f32>("volume") {
                    if !value.is_finite() || value < 0.0 {
                        return Err(mlua::Error::runtime(
                            "viber.sound_loop: volume tem de ser finito e >= 0",
                        ));
                    }
                    volume = value;
                }
                let x = opts.get::<f32>("x").ok();
                let y = opts.get::<f32>("y").ok();
                let z = opts.get::<f32>("z").ok();
                // A mesma regra do `viber.sound`: NaN/inf são ERRO (o resto
                // das primitivas com posição rejeita não-finitos) — aqui
                // caíam em silêncio e o loop tocava sem posição.
                for (axis, value) in [("x", x), ("y", y), ("z", z)] {
                    if value.is_some_and(|v| !v.is_finite()) {
                        return Err(mlua::Error::runtime(format!(
                            "viber.sound_loop: {axis} tem de ser finito (NaN/inf)"
                        )));
                    }
                }
                if let (Some(x), Some(z)) = (x, z) {
                    // Sem `y` = a cota da entidade (0 enterrava o loop nas
                    // colinas — o `viber.sound` já usava a origem).
                    let y = y.unwrap_or(origin_y);
                    position = Some(Vec3::new(x, y, z));
                }
            }
            let owner = lua
                .app_data_ref::<super::ctx::ScriptCtx>()
                .and_then(|ctx| ctx.entity);
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded")
                .commands
                .push(ScriptCommand::SoundLoop {
                    id,
                    clip,
                    pitch,
                    volume,
                    position,
                    owner,
                });
            Ok(())
        })?,
    )?;
    api.set(
        "sound_stop",
        lua.create_function(|lua, id: String| {
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded")
                .commands
                .push(ScriptCommand::SoundStop(id));
            Ok(())
        })?,
    )?;
    // viber.music.layer(nome | nil) — força a layer de BGM. nil devolve
    // a zona/combate automáticos.
    {
        let music = lua.create_table()?;
        music.set(
            "layer",
            lua.create_function(|lua, name: Option<String>| {
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded")
                    .commands
                    .push(ScriptCommand::MusicLayer(name));
                Ok(())
            })?,
        )?;
        // viber.music.combat(active [, boss]) — a REGRA de quando entra a
        // música de combate é do jogo (a engine só tem o estado e o crossfade).
        music.set(
            "combat",
            lua.create_function(|lua, (active, boss): (bool, Option<bool>)| {
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded")
                    .commands
                    .push(ScriptCommand::MusicCombat {
                        active,
                        boss: boss.unwrap_or(false),
                    });
                Ok(())
            })?,
        )?;
        api.set("music", music)?;
    }

    // viber.audio.* — primitivas de mistura/ambiente (o jogo decide quando).
    {
        let audio = lua.create_table()?;
        audio.set(
            "duck",
            lua.create_function(|lua, (bus, gain, secs): (String, f32, f32)| {
                if !gain.is_finite() || !secs.is_finite() || secs <= 0.0 {
                    return Err(mlua::Error::runtime(
                        "viber.audio.duck: gain finito e secs > 0",
                    ));
                }
                if crate::music::AudioBus::parse(&bus).is_none() {
                    return Err(mlua::Error::runtime(format!(
                        "viber.audio.duck: bus '{bus}' inválido (master|music|sfx|voice|ambience)"
                    )));
                }
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded")
                    .commands
                    .push(ScriptCommand::AudioDuck { bus, gain, secs });
                Ok(())
            })?,
        )?;
        audio.set(
            "reverb",
            lua.create_function(|lua, (preset, wet): (String, Option<f32>)| {
                if crate::music::ReverbPreset::parse(&preset).is_none() {
                    return Err(mlua::Error::runtime(format!(
                        "viber.audio.reverb: preset '{preset}' inválido (off|room|hall|cave)"
                    )));
                }
                if let Some(wet) = wet
                    && (!wet.is_finite() || !(0.0..=1.0).contains(&wet))
                {
                    return Err(mlua::Error::runtime(
                        "viber.audio.reverb: wet tem de estar em 0..=1",
                    ));
                }
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded")
                    .commands
                    .push(ScriptCommand::AudioReverb { preset, wet });
                Ok(())
            })?,
        )?;
        api.set("audio", audio)?;
    }

    // viber.set_locomotion(walk, run [, turn_rate]) — nominais explícitos
    // do rig. Só é preciso quando a auto-calibração pelas velocidades
    // comandadas não serve (ex. um rig cujo clip `walk` foi assado a uma
    // cadência diferente da velocidade a que o script o move).
    api.set(
        "set_locomotion",
        lua.create_function(|lua, (walk, run, turn_rate): (f32, f32, Option<f32>)| {
            if !(walk.is_finite() && run.is_finite()) || walk <= 0.0 || run <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.set_locomotion: walk/run têm de ser finitos e > 0",
                ));
            }
            if turn_rate.is_some_and(|t| !t.is_finite() || t <= 0.0) {
                return Err(mlua::Error::runtime(
                    "viber.set_locomotion: turn_rate tem de ser finito e > 0",
                ));
            }
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let Some(entity) = ctx.entity else {
                return Err(mlua::Error::runtime(
                    "viber.set_locomotion fora de on_update",
                ));
            };
            ctx.commands.push(ScriptCommand::SetLocomotion {
                entity,
                walk,
                run,
                turn_rate,
            });
            Ok(())
        })?,
    )?;

    // viber.gesture(name) — gesto one-shot no rig da entidade; o fuzzy
    // match contra os clips do GLB é feito na aplicação (pós-frame) e
    // alternativas separadas por vírgula são tentadas por ordem.
    api.set(
        "gesture",
        lua.create_function(|lua, name: String| {
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let Some(entity) = ctx.entity else {
                return Err(mlua::Error::runtime("viber.gesture fora de on_update"));
            };
            ctx.commands.push(ScriptCommand::Gesture {
                entity,
                name,
                speed: 1.0,
                lock: false,
            });
            Ok(())
        })?,
    )?;

    // viber.sound(clip) — SFX curto na posição da entidade (volume cai
    // com a distância à câmara). Nome inválido = erro de script (warn 1×).
    api.set(
        "sound",
        lua.create_function(|lua, (name, opts): (String, Option<mlua::Table>)| {
            let clip = sfx_clip_from_str(&name).ok_or_else(|| {
                let names: Vec<&str> = SFX_NAME_REGISTRY.iter().map(|(n, _)| *n).collect();
                mlua::Error::runtime(format!(
                    "clip de som desconhecido '{name}' ({})",
                    names.join(", ")
                ))
            })?;
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            // `{x=, y=, z=}` posiciona o SFX no mundo (golpe num prop); sem
            // overrides toca na origem da entidade que corre o script.
            let position = match opts {
                Some(opts) => {
                    let x = opts.raw_get::<Option<f32>>("x")?;
                    let y = opts.raw_get::<Option<f32>>("y")?;
                    let z = opts.raw_get::<Option<f32>>("z")?;
                    // Um NaN aqui seguia para o evento de SFX: a atenuação
                    // por distância dava NaN e o volume chegava NaN ao mixer
                    // (o resto das primitivas com posição rejeita não-finitos).
                    for (axis, value) in [("x", x), ("y", y), ("z", z)] {
                        if value.is_some_and(|v| !v.is_finite()) {
                            return Err(mlua::Error::runtime(format!(
                                "viber.sound('{name}'): {axis} tem de ser finito (NaN/inf)"
                            )));
                        }
                    }
                    if x.is_none() && y.is_none() && z.is_none() {
                        Some(ctx.origin)
                    } else {
                        Some(Vec3::new(
                            x.unwrap_or(ctx.origin.x),
                            y.unwrap_or(ctx.origin.y),
                            z.unwrap_or(ctx.origin.z),
                        ))
                    }
                }
                None => Some(ctx.origin),
            };
            ctx.commands.push(ScriptCommand::PlaySfx { clip, position });
            Ok(())
        })?,
    )?;

    // viber.lantern(intensity) — acende/reforça a lanterna do viajante (a
    // tocha equipada ilumina mesmo de DIA; 0 = só a lanterna automática
    // nocturna). `intensity` em lúmenes, o mesmo espaço da `LANTERN`.
    api.set(
        "lantern",
        lua.create_function(|lua, intensity: f32| {
            if !intensity.is_finite() {
                return Err(mlua::Error::runtime(
                    "viber.lantern: intensidade não finita (NaN/inf)",
                ));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::SetLantern(intensity.max(0.0)));
            Ok(())
        })?,
    )?;

    // viber.bone_offset(bone, rx, ry, rz) — rotação ADITIVA num osso do
    // herói, somada por cima da animação (idle/walk/run) todos os frames.
    // Serve para o braço da tocha ficar à frente sem perder a locomoção;
    // zeros (identidade) limpam a entrada.
    api.set(
        "bone_offset",
        lua.create_function(|lua, (bone, rx, ry, rz): (String, f32, f32, f32)| {
            if bone.trim().is_empty() {
                return Err(mlua::Error::runtime("viber.bone_offset: osso vazio"));
            }
            if !rx.is_finite() || !ry.is_finite() || !rz.is_finite() {
                return Err(mlua::Error::runtime(
                    "viber.bone_offset: rotação não finita (NaN/inf)",
                ));
            }
            let identity = rx.abs() < 1e-6 && ry.abs() < 1e-6 && rz.abs() < 1e-6;
            let rotation = if identity {
                Quat::IDENTITY
            } else {
                Quat::from_euler(EulerRot::XYZ, rx, ry, rz)
            };
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::SetBoneOffset {
                    bone: bone.trim().to_lowercase(),
                    rotation,
                });
            Ok(())
        })?,
    )?;

    // viber.attach(slot, url [, opts]) / viber.detach(slot [, id]) — prende
    // um GLB a um osso do rig. `url` vazio/nil solta o slot. Primitiva
    // genérica (volante, capacete, espada…): a engine só encontra o osso.
    // opts: {bone="hand_r", id, x, y, z, rx, ry, rz (rad), scale}.
    api.set(
        "attach",
        lua.create_function(
            |lua, (slot, url, opts): (String, Option<String>, Option<mlua::Table>)| {
                if slot.trim().is_empty() {
                    return Err(mlua::Error::runtime("viber.attach: slot vazio"));
                }
                let mut owner = None;
                let mut bone = "hand_r".to_string();
                let mut pos = Vec3::ZERO;
                let mut rot = Vec3::ZERO;
                let mut scale = 1.0f32;
                if let Some(opts) = opts {
                    if let Some(bits) = opts.raw_get::<Option<i64>>("id")? {
                        owner = Some(super::entity::entity_from_id(bits)?);
                    }
                    if let Some(name) = opts.raw_get::<Option<String>>("bone")? {
                        bone = name;
                    }
                    pos.x = opts.raw_get::<Option<f32>>("x")?.unwrap_or(0.0);
                    pos.y = opts.raw_get::<Option<f32>>("y")?.unwrap_or(0.0);
                    pos.z = opts.raw_get::<Option<f32>>("z")?.unwrap_or(0.0);
                    rot.x = opts.raw_get::<Option<f32>>("rx")?.unwrap_or(0.0);
                    rot.y = opts.raw_get::<Option<f32>>("ry")?.unwrap_or(0.0);
                    rot.z = opts.raw_get::<Option<f32>>("rz")?.unwrap_or(0.0);
                    scale = opts.raw_get::<Option<f32>>("scale")?.unwrap_or(1.0);
                }
                if !scale.is_finite() || scale <= 0.0 {
                    return Err(mlua::Error::runtime(
                        "viber.attach: scale tem de ser finito e > 0",
                    ));
                }
                // Um NaN em pos/rot envenena o `Transform` do prop para
                // sempre (o `f32::clamp` não o recupera). As restantes APIs
                // numéricas já rejeitam não-finitos.
                if !pos.is_finite() || !rot.is_finite() {
                    return Err(mlua::Error::runtime(
                        "viber.attach: x/y/z/rx/ry/rz têm de ser finitos",
                    ));
                }
                let url = url.filter(|u| !u.trim().is_empty());
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                    .commands
                    .push(ScriptCommand::Attach {
                        owner,
                        slot,
                        url,
                        bone,
                        pos,
                        rot,
                        scale,
                    });
                Ok(())
            },
        )?,
    )?;
    api.set(
        "detach",
        lua.create_function(|lua, (slot, id): (String, Option<i64>)| {
            let owner = match id {
                Some(bits) => Some(super::entity::entity_from_id(bits)?),
                None => None,
            };
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::Attach {
                    owner,
                    slot,
                    url: None,
                    bone: String::new(),
                    pos: Vec3::ZERO,
                    rot: Vec3::ZERO,
                    scale: 1.0,
                });
            Ok(())
        })?,
    )?;

    // viber.wander_target(radius) -> x, z — ponto determinístico ao redor
    // do home (mesma matemática da IA da engine, exposta ao script).
    api.set(
        "wander_target",
        lua.create_function(|lua, radius: f32| {
            if !radius.is_finite() {
                return Err(mlua::Error::runtime(
                    "viber.wander_target: raio não finito (NaN/inf)",
                ));
            }
            // Só os campos necessários (entity): clonar o ctx inteiro
            // (quest_states + vault) por chamada eram milhares de
            // alocações/frame com ~100 scripts activos.
            let entity = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|c| c.entity)
                .ok_or_else(|| mlua::Error::runtime("viber.wander_target fora de on_update"))?;
            let states: Table = lua.named_registry_value("viber_states")?;
            let key = entity.to_bits() as i64;
            let state: Table = states
                .raw_get::<Table>(key)
                .map_err(|_| mlua::Error::runtime("viber.wander_target antes de viber.state()"))?;
            let home: Table = state.raw_get("home").map_err(|_| {
                mlua::Error::runtime(
                    "viber.wander_target: o estado não tem 'home' (o top-level grava-o; o script apagou-o?)",
                )
            })?;
            let home = Vec2::new(
                home.raw_get("x").map_err(|_| {
                    mlua::Error::runtime("viber.wander_target: 'home' sem 'x'")
                })?,
                home.raw_get("z").map_err(|_| {
                    mlua::Error::runtime("viber.wander_target: 'home' sem 'z'")
                })?,
            );
            let picks: u64 = state.raw_get("picks").unwrap_or(0u64);
            state.raw_set("picks", picks + 1)?;
            let seed = crate::ai::enemy_seed(entity.to_bits() as u32, picks);
            let target = crate::ai::wander_target(home, radius, seed);
            Ok((target.x, target.y))
        })?,
    )?;

    api.set(
        "noise",
        lua.create_function(|lua, loudness: f32| {
            let mut ctx = lua.app_data_mut::<ScriptCtx>().expect("ScriptCtx");
            let pos = ctx.origin;
            ctx.commands.push(ScriptCommand::Noise {
                pos,
                loudness: if loudness.is_finite() {
                    loudness.max(0.0)
                } else {
                    0.0
                },
            });
            Ok(())
        })?,
    )?;
    api.set(
        "sense",
        lua.create_function(|lua, ()| {
            // Pulso do frame resolvido pela entidade CORRENTE: a tabela é
            // semeada uma vez por frame e cada hook traz o seu `ctx.entity`.
            let sense = lua.app_data_ref::<ScriptCtx>().and_then(|ctx| {
                let bits = ctx.entity?.to_bits() as i64;
                ctx.sense_by_entity.get(&bits).cloned()
            });
            Ok(sense)
        })?,
    )?;
    api.set(
        "action",
        lua.create_function(|lua, name: String| {
            // Array fixo de chaves estáticas semeado pelo frame — procura
            // linear em 6 entradas, sem mapas realoçados por frame.
            let pressed = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| {
                    ctx.action_pressed
                        .iter()
                        .find(|(key, _)| *key == name)
                        .map(|(_, pressed)| *pressed)
                })
                .unwrap_or(false);
            Ok(pressed)
        })?,
    )?;

    // viber.next_state(cur, dist, aggro, deaggro) -> "wander"|"chase"
    // A máquina wander↔chase da engine, exposta para os scripts comporem.
    api.set(
        "next_state",
        lua.create_function(|_, (cur, dist, aggro, deaggro): (String, f32, f32, f32)| {
            let state = crate::ai::EnemyState::parse(&cur).unwrap_or(crate::ai::EnemyState::Wander);
            Ok(crate::ai::enemy_next_state(dist, state, aggro, deaggro).as_str())
        })?,
    )?;
    // viber.fire_projectile(id [, x, y, z]) — dispara um projétil do
    // `<ProjectileTemplate id=…>` da boca da entidade para o peito do
    // herói (ou para o ponto dado). `false` quando não há alvo.
    api.set(
        "fire_projectile",
        lua.create_function(
            |lua, (template, x, y, z): (String, Option<f32>, Option<f32>, Option<f32>)| {
                let mut ctx = lua
                    .app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new");
                let target = match (x, y, z) {
                    (Some(x), Some(y), Some(z)) => Vec3::new(x, y, z),
                    (None, None, None) => match ctx.player {
                        Some(p) => p + Vec3::Y * crate::projectile::CHEST_HEIGHT,
                        None => return Ok(false),
                    },
                    _ => {
                        return Err(mlua::Error::runtime(
                            "viber.fire_projectile: alvo precisa de x, y e z",
                        ));
                    }
                };
                if !target.is_finite() {
                    return Err(mlua::Error::runtime(
                        "viber.fire_projectile: alvo não finito (NaN/inf)",
                    ));
                }
                let origin = ctx.origin + Vec3::Y * crate::projectile::MUZZLE_HEIGHT;
                let shooter = ctx.entity;
                ctx.commands.push(ScriptCommand::FireProjectile {
                    template,
                    origin,
                    target,
                    shooter,
                });
                Ok(true)
            },
        )?,
    )?;
    api.set(
        "topple",
        lua.create_function(|lua, id: Option<i64>| {
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let entity = match id {
                Some(bits) => super::entity::entity_from_id(bits)?,
                None => ctx
                    .entity
                    .ok_or_else(|| mlua::Error::runtime("viber.topple fora de on_update"))?,
            };
            ctx.commands.push(ScriptCommand::Topple { entity });
            Ok(())
        })?,
    )?;
    api.set(
        "entity_position",
        lua.create_function(|lua, id: Option<i64>| {
            let ctx = lua.app_data_ref::<ScriptCtx>();
            let Some(ctx) = ctx.as_ref() else {
                return Ok((false, 0.0, 0.0, 0.0));
            };
            let bits = match id {
                Some(bits) => bits,
                None => match ctx.entity {
                    Some(e) => e.to_bits() as i64,
                    None => return Ok((false, 0.0, 0.0, 0.0)),
                },
            };
            match ctx.pos_snapshot.get(&bits) {
                Some((_, pos)) => Ok((true, pos.x, pos.y, pos.z)),
                None => Ok((false, 0.0, 0.0, 0.0)),
            }
        })?,
    )?;
    // viber.hostiles([raio]) -> { {id, kind, x, y, z, hp, max, distance}, … }
    // Criaturas vivas de `enemies/` e `bosses/`, à volta do HERÓI (não da
    // entidade do script). Sem nome no spawner, `find_all("wolf")` é vazio.
    api.set(
        "hostiles",
        lua.create_function(|lua, radius: Option<f32>| {
            let radius = radius.unwrap_or(2.4);
            if !radius.is_finite() || radius <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.hostiles: raio tem de ser finito e > 0",
                ));
            }
            let out = lua.create_table()?;
            let ctx = lua.app_data_ref::<ScriptCtx>();
            let Some(ctx) = ctx.as_ref() else {
                return Ok(out);
            };
            let Some(player) = ctx.player else {
                return Ok(out);
            };
            let mut rows: Vec<&super::ctx::HostileSnap> = ctx
                .hostiles
                .iter()
                .filter(|h| h.pos.distance(player) <= radius)
                .collect();
            rows.sort_by(|a, b| {
                a.pos
                    .distance_squared(player)
                    .total_cmp(&b.pos.distance_squared(player))
            });
            for (i, h) in rows.iter().take(128).enumerate() {
                let row = lua.create_table()?;
                row.raw_set("id", h.id)?;
                row.raw_set("kind", h.kind.clone())?;
                row.raw_set("x", h.pos.x)?;
                row.raw_set("y", h.pos.y)?;
                row.raw_set("z", h.pos.z)?;
                row.raw_set("hp", h.hp)?;
                row.raw_set("max", h.max)?;
                row.raw_set("forward_x", h.forward.x)?;
                row.raw_set("forward_z", h.forward.y)?;
                row.raw_set("distance", h.pos.distance(player))?;
                out.raw_set(i + 1, row)?;
            }
            Ok(out)
        })?,
    )?;
    // viber.interact_range(base) -> alcance EFETIVO (o autorado escalado
    // pela configuração global de interação). O script compara distâncias
    // sem duplicar a escala.
    api.set(
        "interact_range",
        lua.create_function(|_, base: f32| {
            if !base.is_finite() || base < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.interact_range: base tem de ser finita e >= 0",
                ));
            }
            Ok(crate::interact::scaled_range(base))
        })?,
    )?;
    // viber.nearby(raio [, limite]) -> { {id, name, x, y, z, distance, props?}, … }
    // — entidades NOMEADAS à volta da entidade corrente, mais perto 1.º.
    // `props` são os dados autorais (`prop-*`, `destructible`, resource-*).
    api.set(
        "nearby",
        lua.create_function(|lua, (radius, limit): (f32, Option<usize>)| {
            let out = lua.create_table()?;
            if !radius.is_finite() || radius <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.nearby: raio tem de ser finito e > 0",
                ));
            }
            let ctx = lua.app_data_ref::<ScriptCtx>();
            let Some(ctx) = ctx.as_ref() else {
                return Ok(out);
            };
            // Filtrar ANTES de clonar: o snapshot tem todas as entidades
            // nomeadas (~79k no Vael) e clonar cada nome para depois o deitar
            // fora custava 3–4 ms por chamada — o `harvest.lua` chama isto
            // em todos os frames.
            // A PRÓPRIA entidade fica de fora: a distância é 0, era sempre a
            // primeira da lista, e um script que escolhe "o mais próximo"
            // interagia consigo (o `nearby` é "à volta", não "incluindo").
            let own = ctx.entity.map(|entity| entity.to_bits() as i64);
            let mut found: Vec<(f32, i64, &str, Vec3)> = ctx
                .pos_snapshot
                .iter()
                .filter_map(|(bits, (name, pos))| {
                    if Some(*bits) == own {
                        return None;
                    }
                    let d = pos.distance(ctx.origin);
                    (d <= radius).then_some((d, *bits, name.as_str(), *pos))
                })
                .collect();
            found.sort_by(|a, b| a.0.total_cmp(&b.0));
            let cap = limit.unwrap_or(16).min(64);
            for (i, (distance, bits, name, pos)) in found.into_iter().take(cap).enumerate() {
                let t = lua.create_table()?;
                t.raw_set("id", bits)?;
                t.raw_set("name", name)?;
                t.raw_set("x", pos.x)?;
                t.raw_set("y", pos.y)?;
                t.raw_set("z", pos.z)?;
                t.raw_set("distance", distance)?;
                if let Some(props) = ctx.props_snapshot.get(&bits) {
                    let table = lua.create_table()?;
                    for (key, value) in props.iter() {
                        table.raw_set(key.as_str(), value.as_str())?;
                    }
                    t.raw_set("props", table)?;
                }
                out.raw_set(i + 1, t)?;
            }
            Ok(out)
        })?,
    )?;
    // viber.player_nearby(raio [, limite]) — o mesmo que `nearby`, mas
    // centrado no HERÓI (controllers always-active não vivem no herói).
    api.set(
        "player_nearby",
        lua.create_function(|lua, (radius, limit): (f32, Option<usize>)| {
            let out = lua.create_table()?;
            if !radius.is_finite() || radius <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.player_nearby: raio tem de ser finito e > 0",
                ));
            }
            let ctx = lua.app_data_ref::<ScriptCtx>();
            let Some(ctx) = ctx.as_ref() else {
                return Ok(out);
            };
            let Some(origin) = ctx.player else {
                return Ok(out);
            };
            // Filtrar ANTES de clonar: o snapshot tem todas as entidades
            // nomeadas (~79k no Vael) e clonar cada nome para depois o deitar
            // fora custava 3–4 ms por chamada — o `harvest.lua` chama isto
            // em todos os frames.
            let mut found: Vec<(f32, i64, &str, Vec3)> = ctx
                .pos_snapshot
                .iter()
                .filter_map(|(bits, (name, pos))| {
                    let d = pos.distance(origin);
                    (d <= radius).then_some((d, *bits, name.as_str(), *pos))
                })
                .collect();
            found.sort_by(|a, b| a.0.total_cmp(&b.0));
            let cap = limit.unwrap_or(16).min(64);
            for (i, (distance, bits, name, pos)) in found.into_iter().take(cap).enumerate() {
                let t = lua.create_table()?;
                t.raw_set("id", bits)?;
                t.raw_set("name", name)?;
                t.raw_set("x", pos.x)?;
                t.raw_set("y", pos.y)?;
                t.raw_set("z", pos.z)?;
                t.raw_set("distance", distance)?;
                if let Some(props) = ctx.props_snapshot.get(&bits) {
                    let table = lua.create_table()?;
                    for (key, value) in props.iter() {
                        table.raw_set(key.as_str(), value.as_str())?;
                    }
                    t.raw_set("props", table)?;
                }
                out.raw_set(i + 1, t)?;
            }
            Ok(out)
        })?,
    )?;
    // viber.player_forward() -> dx, dz — forward do herói no plano.
    api.set(
        "player_forward",
        lua.create_function(|lua, ()| {
            let forward = lua
                .app_data_ref::<ScriptCtx>()
                .map(|c| c.player_forward)
                .unwrap_or_default();
            Ok((forward.x, forward.y))
        })?,
    )?;
    // viber.radial_damage(x, z, raio, dano [, opts]) — dano em área com
    // falloff linear; opts { knockback = força }. Mortes seguem a
    // paridade nativa (corpo, XP, quests, evento Kill).
    api.set(
        "radial_damage",
        lua.create_function(
            |lua, (x, z, radius, damage, opts): (f32, f32, f32, f32, Option<Table>)| {
                if !(x.is_finite() && z.is_finite() && radius.is_finite() && damage.is_finite())
                    || radius <= 0.0
                    || damage < 0.0
                {
                    return Err(mlua::Error::runtime(
                        "viber.radial_damage: valores têm de ser finitos (raio > 0, dano >= 0)",
                    ));
                }
                let knockback = opts
                    .and_then(|t| t.raw_get::<f32>("knockback").ok())
                    .unwrap_or(0.0);
                if !knockback.is_finite() || knockback < 0.0 {
                    return Err(mlua::Error::runtime(
                        "viber.radial_damage: knockback tem de ser finito e >= 0",
                    ));
                }
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                    .commands
                    .push(ScriptCommand::RadialDamage {
                        x,
                        z,
                        radius,
                        damage,
                        knockback,
                    });
                Ok(())
            },
        )?,
    )?;
    // viber.burst(preset, x, y, z [, count]) — preset validado contra a
    // biblioteca (`particles::PRESET_NAMES`).
    api.set(
        "burst",
        lua.create_function(
            |lua, (preset, x, y, z, count): (String, f32, f32, f32, Option<usize>)| {
                if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                    return Err(mlua::Error::runtime(
                        "viber.burst: coordenadas não finitas (NaN/inf)",
                    ));
                }
                // `sparks` é o preset de impacto do melee (fora da
                // biblioteca — ver `luau::fx::ScriptFx::burst`).
                if preset != "sparks" && !crate::particles::PRESET_NAMES.contains(&preset.as_str())
                {
                    return Err(mlua::Error::runtime(format!(
                        "viber.burst: preset desconhecido '{preset}' (válidos: {} sparks)",
                        crate::particles::PRESET_NAMES.join(", ")
                    )));
                }
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                    .commands
                    .push(ScriptCommand::Burst {
                        preset,
                        pos: Vec3::new(x, y, z),
                        count: count.unwrap_or(12),
                    });
                Ok(())
            },
        )?,
    )?;
    // viber.ring(x, z, raio [, cor "#hex"]) — anel de choque no chão.
    api.set(
        "ring",
        lua.create_function(
            |lua, (x, z, radius, color): (f32, f32, f32, Option<String>)| {
                if !(x.is_finite() && z.is_finite() && radius.is_finite()) || radius <= 0.0 {
                    return Err(mlua::Error::runtime(
                        "viber.ring: coordenadas finitas e raio > 0",
                    ));
                }
                let color = match color {
                    Some(hex) => Some(
                        crate::xml::values::parse_color(&hex, "viber.ring").map_err(|e| {
                            mlua::Error::runtime(format!("viber.ring: cor inválida ({e})"))
                        })?,
                    ),
                    None => None,
                };
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                    .commands
                    .push(ScriptCommand::Ring {
                        x,
                        z,
                        radius,
                        color,
                    });
                Ok(())
            },
        )?,
    )?;
    // viber.decal(url, x, y, z [, {nx, ny, nz, size, life, color}]) — marca
    // de impacto projectada numa superfície (chão/parede/rocha). A textura é
    // do jogo; a engine só desenha e desvanece.
    api.set(
        "decal",
        lua.create_function(
            |lua, (url, x, y, z, opts): (String, f32, f32, f32, Option<Table>)| {
                if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                    return Err(mlua::Error::runtime(
                        "viber.decal: coordenadas têm de ser finitas",
                    ));
                }
                if url.trim().is_empty() {
                    return Err(mlua::Error::runtime("viber.decal: url vazia"));
                }
                let (mut nx, mut ny, mut nz) = (0.0f32, 1.0f32, 0.0f32);
                let mut size = 0.7f32;
                let mut life = 1.5f32;
                let mut color = [1.0f32, 1.0, 1.0];
                if let Some(t) = opts {
                    nx = t.raw_get("nx").ok().unwrap_or(nx);
                    ny = t.raw_get("ny").ok().unwrap_or(ny);
                    nz = t.raw_get("nz").ok().unwrap_or(nz);
                    size = t.raw_get("size").ok().unwrap_or(size);
                    life = t.raw_get("life").ok().unwrap_or(life);
                    if let Some(hex) = t.raw_get::<Option<String>>("color").ok().flatten() {
                        color =
                            crate::xml::values::parse_color(&hex, "viber.decal").map_err(|e| {
                                mlua::Error::runtime(format!("viber.decal: cor inválida ({e})"))
                            })?;
                    }
                }
                if !(nx.is_finite() && ny.is_finite() && nz.is_finite())
                    || Vec3::new(nx, ny, nz).length_squared() < 1e-6
                {
                    nx = 0.0;
                    ny = 1.0;
                    nz = 0.0;
                }
                if !size.is_finite() || size <= 0.0 {
                    size = 0.7;
                }
                if !life.is_finite() || life <= 0.0 {
                    life = 1.5;
                }
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                    .commands
                    .push(ScriptCommand::Decal {
                        url,
                        position: Vec3::new(x, y, z),
                        normal: Vec3::new(nx, ny, nz),
                        size,
                        life,
                        color,
                    });
                Ok(())
            },
        )?,
    )?;
    api.set(
        "shake",
        lua.create_function(|lua, amount: f32| {
            if !amount.is_finite() || amount < 0.0 {
                return Err(mlua::Error::runtime("viber.shake: valor finito e >= 0"));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("seeded")
                .commands
                .push(ScriptCommand::Shake(amount));
            Ok(())
        })?,
    )?;
    api.set(
        "kick",
        lua.create_function(|lua, (x, y, z): (f32, f32, f32)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime("viber.kick: valores não finitos"));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("seeded")
                .commands
                .push(ScriptCommand::CameraKick(Vec3::new(x, y, z)));
            Ok(())
        })?,
    )?;
    api.set(
        "fov_kick",
        lua.create_function(|lua, deg: f32| {
            if !deg.is_finite() {
                return Err(mlua::Error::runtime("viber.fov_kick: valor não finito"));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("seeded")
                .commands
                .push(ScriptCommand::FovKick(deg));
            Ok(())
        })?,
    )?;
    api.set(
        "punch",
        lua.create_function(|lua, (stops, bloom): (f32, f32)| {
            if !(stops.is_finite() && bloom.is_finite()) {
                return Err(mlua::Error::runtime("viber.punch: valores não finitos"));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("seeded")
                .commands
                .push(ScriptCommand::Punch { stops, bloom });
            Ok(())
        })?,
    )?;
    api.set(
        "hit_stop",
        lua.create_function(|lua, secs: f32| {
            if !secs.is_finite() || secs < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.hit_stop: segundos finitos e >= 0",
                ));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("seeded")
                .commands
                .push(ScriptCommand::HitStop(secs));
            Ok(())
        })?,
    )?;
    api.set(
        "damage_number",
        lua.create_function(|lua, (text, opts): (String, Option<Table>)| {
            // Default: 1,8 m acima da entidade. Cada eixo dado SUBSTITUI o
            // default (um `{y = 3}` era ignorado por exigir x, y e z).
            let base = lua
                .app_data_ref::<ScriptCtx>()
                .map(|ctx| ctx.origin)
                .unwrap_or_default()
                + Vec3::Y * 1.8;
            let mut pos = base;
            let mut color = None;
            if let Some(t) = opts {
                let axis = |name: &str, fallback: f32| {
                    t.raw_get::<f32>(name)
                        .ok()
                        .filter(|value| value.is_finite())
                        .unwrap_or(fallback)
                };
                pos = Vec3::new(axis("x", base.x), axis("y", base.y), axis("z", base.z));
                if let Ok(Some(hex)) = t.raw_get::<Option<String>>("color") {
                    color = Some(
                        crate::xml::values::parse_color(&hex, "viber.damage_number").map_err(
                            |e| {
                                mlua::Error::runtime(format!(
                                    "viber.damage_number: cor inválida ({e})"
                                ))
                            },
                        )?,
                    );
                }
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("seeded")
                .commands
                .push(ScriptCommand::DamageNumber { text, pos, color });
            Ok(())
        })?,
    )?;
    // viber.play_clip(nome [, opts]) — clip de ação no rig (opts:
    // {speed = 1.0, id = bits}); generaliza o `viber.gesture` com
    // velocidade (mine/chop do rig do herói) e alvo.
    api.set(
        "play_clip",
        lua.create_function(|lua, (name, opts): (String, Option<Table>)| {
            let (speed, id, lock) = match opts {
                Some(t) => (
                    t.raw_get::<f32>("speed").ok().unwrap_or(1.0),
                    t.raw_get::<i64>("id").ok(),
                    t.raw_get::<bool>("lock").ok().unwrap_or(false),
                ),
                None => (1.0, None, false),
            };
            if !speed.is_finite() || speed <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.play_clip: speed tem de ser finito e > 0",
                ));
            }
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let entity = match id {
                Some(bits) => entity::entity_from_id(bits)?,
                None => ctx.entity.ok_or_else(|| {
                    mlua::Error::runtime("viber.play_clip fora de on_update (sem id)")
                })?,
            };
            ctx.commands.push(ScriptCommand::Gesture {
                entity,
                name,
                speed,
                lock,
            });
            Ok(())
        })?,
    )?;
    // viber.entity_despawn(id) — remove uma entidade arbitrária (o
    // `despawn_self` é o atalho para a própria).
    api.set(
        "entity_despawn",
        lua.create_function(|lua, id: i64| {
            let target = entity::entity_from_id(id)?;
            lua.app_data_mut::<ScriptCtx>()
                .expect("seeded")
                .commands
                .push(ScriptCommand::Despawn(target));
            Ok(())
        })?,
    )?;
    // viber.own_system(nome) — reclama um sistema NATIVO para Lua
    // (`dialogue`, `abilities`, `bomb`, `guard`, `hotbar`, `harvest`,
    // `locomotion`): o handler nativo correspondente cala e a lógica passa
    // a ser do script. Sem posse, o comportamento nativo é o de sempre.
    api.set(
        "own_system",
        lua.create_function(|lua, name: String| {
            if !super::ownership::KNOWN_SYSTEMS.contains(&name.as_str()) {
                return Err(mlua::Error::runtime(format!(
                    "viber.own_system: sistema desconhecido '{name}' (válidos: {})",
                    super::ownership::KNOWN_SYSTEMS.join(", ")
                )));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::OwnSystem(name));
            Ok(())
        })?,
    )?;
    // viber.release_system(nome) — devolve o sistema ao handler nativo (o
    // inverso de `own_system`): o script larga o corpo do herói e o
    // `player_movement` volta a andar com ele.
    api.set(
        "release_system",
        lua.create_function(|lua, name: String| {
            if !super::ownership::KNOWN_SYSTEMS.contains(&name.as_str()) {
                return Err(mlua::Error::runtime(format!(
                    "viber.release_system: sistema desconhecido '{name}' (válidos: {})",
                    super::ownership::KNOWN_SYSTEMS.join(", ")
                )));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::ReleaseSystem(name));
            Ok(())
        })?,
    )?;
    // viber.say(texto [, segundos]) — escreve no BALÃO nativo do HUD
    // (o mesmo do diálogo de quests). Sem `<DialogueBalloon>` no mundo é
    // no-op com warn 1×.
    api.set(
        "say",
        lua.create_function(|lua, (text, secs): (String, Option<f32>)| {
            let secs = secs.unwrap_or(0.0);
            if !secs.is_finite() || secs < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.say: segundos têm de ser finitos e >= 0",
                ));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::Say { text, secs });
            Ok(())
        })?,
    )?;

    // viber.save() / viber.load_save() — gravam/carregam o save do mundo
    // (o mesmo ficheiro do botão Guardar/Carregar do menu; funciona em
    // QUALQUER preset — um jogo sem RPG usa para persistir o game()
    // e a posição). O pedido é drenado no mesmo frame pelo save.
    // NOTA: `load_save` e não `load` — `viber.load` é o carregador de
    // MÓDULOS (`viber.load("lib/fsm.lua")`) desde a fase A; os dois
    // nomes colidiam e um tapava o outro.
    api.set(
        "save",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::SaveNow);
            Ok(())
        })?,
    )?;
    api.set(
        "load_save",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::LoadNow);
            Ok(())
        })?,
    )?;
    // viber.quit() — fecha o jogo (AppExit limpo; o save é responsabilidade
    // do jogo, tal como o botão Sair do menu).
    api.set(
        "quit",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::Quit);
            Ok(())
        })?,
    )?;

    // viber.save_slot(n) / viber.load_slot(n) — slots 0..=3 (0 = autosave).
    // O save legado (`viber.save()`) fica no path antigo.
    api.set(
        "save_slot",
        lua.create_function(|lua, slot: u32| {
            if slot > 3 {
                return Err(mlua::Error::runtime("viber.save_slot: slot fora de 0..=3"));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::SaveSlot(slot));
            Ok(())
        })?,
    )?;
    api.set(
        "load_slot",
        lua.create_function(|lua, slot: u32| {
            if slot > 3 {
                return Err(mlua::Error::runtime("viber.load_slot: slot fora de 0..=3"));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::LoadSlot(slot));
            Ok(())
        })?,
    )?;
    // viber.save_slots() — metadados dos 4 slots para o menu de continuar.
    // Lê 4 JSONs pequenos por chamada; o menu só chama ao abrir.
    api.set(
        "save_slots",
        lua.create_function(|lua, ()| {
            let (key, dir) = {
                let ctx = lua.app_data_ref::<ScriptCtx>();
                ctx.as_ref()
                    .map(|ctx| (ctx.world_key.clone(), ctx.save_dir.clone()))
                    .unwrap_or_default()
            };
            let list = crate::save::list_save_slots(key.as_deref(), dir.as_deref());
            let table = lua.create_table()?;
            for (index, slot) in list.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("slot", slot.slot)?;
                row.set("path", slot.path.as_str())?;
                row.set("exists", slot.exists)?;
                row.set("bytes", slot.bytes)?;
                row.set("saved_at", slot.saved_at)?;
                row.set("level", slot.level)?;
                row.set("gold", slot.gold)?;
                row.set("scene", slot.scene.as_deref())?;
                row.set("playtime", slot.playtime)?;
                row.set("title", slot.title.as_deref())?;
                table.set(index + 1, row)?;
            }
            Ok(table)
        })?,
    )?;

    // viber.spawn_prototype(nome, x, z [, opts]) — instancia um
    // `<Prototype>` do mundo. opts: {y = cota explícita (default: assenta
    // no terreno), on_spawned = fn(bits)}. O spawn real é pós-frame
    // (sistema exclusivo, `recipes::spawn::apply_script_spawns`).
    api.set(
        "spawn_prototype",
        lua.create_function(
            |lua, (name, x, z, opts): (String, f32, f32, Option<Table>)| {
                if !(x.is_finite() && z.is_finite()) {
                    return Err(mlua::Error::runtime(
                        "viber.spawn_prototype: coordenadas não finitas (NaN/inf)",
                    ));
                }
                let (y, on_spawned) = match opts {
                    Some(t) => (
                        t.raw_get::<f32>("y").ok(),
                        t.raw_get::<mlua::Function>("on_spawned").ok(),
                    ),
                    None => (None, None),
                };
                if let Some(y) = y
                    && !y.is_finite()
                {
                    return Err(mlua::Error::runtime(
                        "viber.spawn_prototype: y não finito (NaN/inf)",
                    ));
                }
                // Validado à fila: prototype desconhecido é ERRO de
                // script (o transbordo silencioso era o pior caso).
                let known = lua
                    .app_data_ref::<ScriptCtx>()
                    .map(|c| c.prototype_names.iter().any(|n| n == &name))
                    .unwrap_or(false);
                if !known {
                    return Err(mlua::Error::runtime(format!(
                        "viber.spawn_prototype: prototype desconhecido '{name}'"
                    )));
                }
                let mut ctx = lua
                    .app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new");
                let caller_path = ctx.path.clone();
                ctx.commands.push(ScriptCommand::SpawnPrototype {
                    name,
                    pos: Vec3::new(x, y.unwrap_or(0.0), z),
                    seat: y.is_none(),
                    on_spawned,
                    caller_path,
                });
                Ok(())
            },
        )?,
    )?;
    // viber.own_action(nome) — reclama uma ação da UI para Lua: o
    // handler nativo cala e a ação chega via `viber.events()` como
    // {type="ui_action", name=..., arg=...}. Sem dono, o comportamento
    // nativo é o de sempre (compat).
    api.set(
        "own_action",
        lua.create_function(|lua, name: String| {
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::OwnUiAction(name));
            Ok(())
        })?,
    )?;
    // viber.despawn_self() — a entidade se remove (árvore derrubada etc).
    api.set(
        "despawn_self",
        lua.create_function(|lua, ()| {
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let Some(entity) = ctx.entity else {
                return Err(mlua::Error::runtime("viber.despawn_self fora de on_update"));
            };
            ctx.commands.push(ScriptCommand::Despawn(entity));
            Ok(())
        })?,
    )?;
    // viber.change_scene(id[, entrada]) -> true | false, erro — troca de cena (fade,
    // descarrega a atual, carrega a outra; `entrada` = `<SceneEntry name>`, "@back" = o
    // portal por onde se entrou). Só em projetos com cenas (`project:` no config.yaml).
    api.set(
        "change_scene",
        lua.create_function(|lua, (scene, entry): (String, Option<String>)| {
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            if ctx.scene.is_none() {
                return Ok((
                    false,
                    Some(
                        "este mundo não é uma cena de um projeto (project: no config.yaml)"
                            .to_string(),
                    ),
                ));
            }
            let id = scene.trim().to_string();
            if !ctx.scene_ids.contains(&id) {
                return Ok((
                    false,
                    Some(format!(
                        "cena desconhecida '{id}' (cenas: {})",
                        ctx.scene_ids.join(", ")
                    )),
                ));
            }
            let via = ctx.self_name.clone();
            ctx.commands.push(ScriptCommand::ChangeScene {
                scene: id,
                entry,
                via,
            });
            Ok((true, None))
        })?,
    )?;
    // viber.scene_name() -> id da cena carregada (nil fora de um projeto com cenas).
    api.set(
        "scene_name",
        lua.create_function(|lua, ()| {
            Ok(lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.scene.clone()))
        })?,
    )?;
    // viber.scene_entry() -> nome da entrada por onde se chegou (nil = nenhuma).
    api.set(
        "scene_entry",
        lua.create_function(|lua, ()| {
            Ok(lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.scene_entry.clone()))
        })?,
    )?;
    api.set(
        "teleport_player",
        lua.create_function(|lua, (x, y, z, exclude): (f32, f32, f32, Option<i64>)| {
            if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.teleport_player: coordenadas não finitas (NaN/inf)",
                ));
            }
            // O `id` opcional NÃO entra no raio de aterragem: teleportar para
            // um poço/torre não pode pousar o herói em cima do próprio prop.
            let exclude = match exclude {
                Some(bits) => Some(super::entity::entity_from_id(bits)?),
                None => None,
            };
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::TeleportPlayer {
                    pos: Vec3::new(x, y, z),
                    exclude,
                });
            Ok(())
        })?,
    )?;

    // viber.push_player(dx, dz) — desloca o herói ~(dx, dz) metros pelo
    // character controller (colide com muros; o dash não atravessa paredes
    // como o teleporte). I-frames são regra do jogo (Lua).
    api.set(
        "push_player",
        lua.create_function(|lua, (dx, dz): (f32, f32)| {
            if !(dx.is_finite() && dz.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.push_player: valores não finitos (NaN/inf)",
                ));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::PushPlayer {
                    velocity: crate::physics_fx::push_velocity(Vec3::new(dx, 0.0, dz)),
                });
            Ok(())
        })?,
    )?;

    // viber.camera_free(ativo) — a câmara de JOGO passa a ser escrita pelo
    // script (`camera_pose`). `false` devolve a pose ao rig de terceira
    // pessoa no frame seguinte. O `OrbitCamera` continua a ser o dono da
    // câmara: post-FX, tone mapping e HUD não mudam.
    api.set(
        "camera_free",
        lua.create_function(|lua, active: bool| {
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::CameraFree(active));
            Ok(())
        })?,
    )?;
    // viber.camera_pose(olho_x, olho_y, olho_z, alvo_x, alvo_y, alvo_z) —
    // pose do modo livre. Sem `camera_free(true)` é inerte.
    api.set(
        "camera_pose",
        lua.create_function(
            |lua, (x, y, z, tx, ty, tz): (f32, f32, f32, f32, f32, f32)| {
                if !(x.is_finite()
                    && y.is_finite()
                    && z.is_finite()
                    && tx.is_finite()
                    && ty.is_finite()
                    && tz.is_finite())
                {
                    return Err(mlua::Error::runtime(
                        "viber.camera_pose: coordenadas não finitas (NaN/inf)",
                    ));
                }
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                    .commands
                    .push(ScriptCommand::CameraPose {
                        pos: Vec3::new(x, y, z),
                        look: Vec3::new(tx, ty, tz),
                    });
                Ok(())
            },
        )?,
    )?;
    // viber.camera_state() -> {x,y,z,yaw,pitch} — pose VIVA da câmara de
    // jogo. `yaw`/`pitch` são a direção de VISTA em graus (forward =
    // (sin yaw·cos pitch, sin pitch, cos yaw·cos pitch)), a mesma convenção
    // do `camera_pose`. `nil` sem câmara.
    api.set(
        "camera_state",
        lua.create_function(|lua, ()| {
            let Some((pos, yaw, pitch)) =
                lua.app_data_ref::<ScriptCtx>().and_then(|ctx| ctx.camera)
            else {
                return Ok(mlua::Value::Nil);
            };
            let table = lua.create_table()?;
            table.raw_set("x", pos.x)?;
            table.raw_set("y", pos.y)?;
            table.raw_set("z", pos.z)?;
            table.raw_set("yaw", yaw)?;
            table.raw_set("pitch", pitch)?;
            Ok(mlua::Value::Table(table))
        })?,
    )?;
    // viber.set_visible(visivel [, id]) — esconde/mostra a entidade e a
    // subárvore (a `Visibility` do Bevy propaga aos filhos com Inherited).
    api.set(
        "set_visible",
        lua.create_function(|lua, (visible, id): (bool, Option<i64>)| {
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded");
            let entity = match id {
                Some(bits) => super::entity::entity_from_id(bits)?,
                None => ctx.entity.ok_or_else(|| {
                    mlua::Error::runtime("viber.set_visible fora de on_update (sem id)")
                })?,
            };
            ctx.commands
                .push(ScriptCommand::SetVisible { entity, visible });
            Ok(())
        })?,
    )?;
    // viber.set_player_speed(mult) — multiplicador da velocidade base.
    api.set(
        "set_player_speed",
        lua.create_function(|lua, mult: f32| {
            if !mult.is_finite() || mult <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.set_player_speed: multiplicador finito e > 0",
                ));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::SetPlayerSpeed(mult.clamp(0.1, 5.0)));
            Ok(())
        })?,
    )?;
    // viber.clip_duration(nome) — duração do clip do herói (s), fuzzy pelo
    // nome normalizado. O jogo agenda swings/one-shots sem hardcodar o GLB.
    api.set(
        "clip_duration",
        lua.create_function(|lua, name: String| {
            let key = crate::animation::normalize_clip_name(&name);
            let ctx = lua.app_data_ref::<ScriptCtx>();
            Ok(ctx
                .as_ref()
                .and_then(|ctx| ctx.clip_durations.get(&key).copied()))
        })?,
    )?;
    // viber.entity_knockback(id, dx, dz, força) — impulso físico na entidade.
    // O jogo decide quando e quanto; a engine não conhece golpes.
    api.set(
        "entity_knockback",
        lua.create_function(|lua, (id, dx, dz, strength): (i64, f32, f32, f32)| {
            if !(dx.is_finite() && dz.is_finite() && strength.is_finite()) {
                return Err(mlua::Error::runtime(
                    "viber.entity_knockback: valores não finitos (NaN/inf)",
                ));
            }
            let dir = Vec3::new(dx, 0.0, dz).normalize_or_zero();
            let entity = super::entity::entity_from_id(id)?;
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::EntityKnockback {
                    entity,
                    dir,
                    strength: strength.max(0.0),
                });
            Ok(())
        })?,
    )?;
    // viber.hit_flash([id][, segundos]) — acende o flash branco de impacto.
    // Feedback visual genérico; não toca HP nem regras. `id` omitido = a
    // própria entidade (como o resto das vitais; a doc prometia-o e o
    // `i64` obrigatório era erro Lua).
    api.set(
        "hit_flash",
        lua.create_function(|lua, (id, secs): (Option<i64>, Option<f32>)| {
            let secs = secs.unwrap_or(crate::feedback::HIT_FLASH_SECS);
            if !secs.is_finite() || secs <= 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.hit_flash: segundos têm de ser finitos e > 0",
                ));
            }
            let entity = super::entity::resolve_entity(lua, id)?;
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::HitFlash {
                    entity,
                    secs: secs.min(2.0),
                });
            Ok(())
        })?,
    )?;

    // viber.corpse([id]) — marca a morte. A engine não agenda despawn nem XP:
    // o jogo chama isto, deixa o `ragdoll="true"` fazer o resto e decide
    // quando remover o corpo. Sem `id` = a própria entidade (a doc prometia-o
    // e o `i64` obrigatório era erro Lua).
    api.set(
        "corpse",
        lua.create_function(|lua, id: Option<i64>| {
            let entity = super::entity::resolve_entity(lua, id)?;
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::Corpse { entity });
            Ok(())
        })?,
    )?;

    // viber.a11y() -> {subtitle_scale, shake, hitstop, interact_hold}
    // viber.a11y_set{t} — o jogo desenha a acessibilidade; a engine só aplica.
    api.set(
        "a11y",
        lua.create_function(|lua, ()| {
            let a11y = lua
                .app_data_ref::<ScriptCtx>()
                .map(|ctx| ctx.a11y)
                .unwrap_or_default();
            let out = lua.create_table()?;
            out.raw_set("subtitle_scale", a11y.subtitle_scale)?;
            out.raw_set("shake", a11y.shake)?;
            out.raw_set("hitstop", a11y.hitstop)?;
            out.raw_set("interact_hold", a11y.interact_hold)?;
            Ok(out)
        })?,
    )?;
    api.set(
        "a11y_set",
        lua.create_function(|lua, opts: Table| {
            let subtitle_scale = opts.raw_get::<f32>("subtitle_scale").ok();
            if subtitle_scale.is_some_and(|value| !value.is_finite() || value <= 0.0) {
                return Err(mlua::Error::runtime(
                    "viber.a11y_set: subtitle_scale tem de ser finito e > 0",
                ));
            }
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::A11ySet {
                    subtitle_scale,
                    shake: opts.raw_get::<bool>("shake").ok(),
                    hitstop: opts.raw_get::<bool>("hitstop").ok(),
                    interact_hold: opts.raw_get::<bool>("interact_hold").ok(),
                });
            Ok(())
        })?,
    )?;

    // viber.net_role() -> "off" | "host" | "join" (VIBER_NET no arranque).
    api.set(
        "net_role",
        lua.create_function(|lua, ()| {
            Ok(lua
                .app_data_ref::<ScriptCtx>()
                .map(|ctx| ctx.net_role)
                .unwrap_or("off"))
        })?,
    )?;

    // ── UI / interação ──────────────────────────────────────────────
    api.set(
        "markers",
        lua.create_function(|lua, spec: mlua::Table| {
            fn point(row: &mlua::Table) -> Option<Vec3> {
                let x: f32 = row.raw_get("x").ok()?;
                let y: f32 = row.raw_get("y").ok()?;
                let z: f32 = row.raw_get("z").ok()?;
                (x.is_finite() && y.is_finite() && z.is_finite()).then_some(Vec3::new(x, y, z))
            }
            fn points(table: mlua::Table, cap: usize) -> mlua::Result<Vec<Vec3>> {
                let mut out = Vec::new();
                for row in table.sequence_values::<mlua::Table>() {
                    if out.len() >= cap {
                        break;
                    }
                    if let Some(pos) = point(&row?) {
                        out.push(pos);
                    }
                }
                Ok(out)
            }
            let mut blips = Vec::new();
            if let Ok(rows) = spec.raw_get::<mlua::Table>("blips") {
                for row in rows.sequence_values::<mlua::Table>() {
                    if blips.len() >= 32 {
                        break;
                    }
                    let row = row?;
                    let Some(pos) = point(&row) else { continue };
                    let tower: bool = row.raw_get("tower").unwrap_or(false);
                    blips.push((pos, tower));
                }
            }
            let beacons = spec
                .raw_get::<mlua::Table>("beacons")
                .ok()
                .map(|rows| points(rows, 16))
                .transpose()?
                .unwrap_or_default();
            let waypoint = spec
                .raw_get::<mlua::Table>("waypoint")
                .ok()
                .and_then(|row| {
                    let pos = point(&row)?;
                    let label: String = row.raw_get("label").unwrap_or_default();
                    Some((pos, label))
                });
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::SetMarkers {
                    blips,
                    beacons,
                    waypoint,
                });
            Ok(())
        })?,
    )?;
    api.set(
        "toast",
        lua.create_function(|lua, msg: String| {
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::Toast(msg));
            Ok(())
        })?,
    )?;
    api.set(
        "set_interaction",
        lua.create_function(|lua, (label, key, range): (String, String, Option<f32>)| {
            // Validado à fila (erro de script → warn 1×): a aplicação
            // pós-frame largava a tecla desconhecida EM SILÊNCIO — o
            // alvo de interação nunca aparecia sem diagnóstico.
            key_code_from_str(&key).ok_or_else(|| {
                mlua::Error::runtime(format!("viber.set_interaction: tecla desconhecida '{key}'"))
            })?;
            // O default é o alcance BASE autorado; a escala global entra
            // no `SetInteraction` (um só sítio).
            let range = range.unwrap_or(crate::interact::BASE_RANGE_M);
            // Negativo nunca ganha o foco (e o fallback de alcance do
            // `viber.interacted` também compara com ele): a interação ficava
            // morta em silêncio. Fora do intervalo = erro de script.
            if !range.is_finite() || range < 0.0 {
                return Err(mlua::Error::runtime(
                    "viber.set_interaction: range tem de ser finito e >= 0",
                ));
            }
            let mut ctx = lua
                .app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new");
            let Some(entity) = ctx.entity else {
                return Err(mlua::Error::runtime(
                    "viber.set_interaction fora de on_update",
                ));
            };
            ctx.commands.push(ScriptCommand::SetInteraction {
                entity,
                label,
                key,
                range,
            });
            Ok(())
        })?,
    )?;
    // viber.prompt(key, label) — prompt de interação de MUNDO. Um controller
    // publica o verbo do alvo atual (um prop que não é a própria entidade);
    // sem `label` (ou nil) limpa. É UI genérica: o JOGO decide quando.
    api.set(
        "prompt",
        lua.create_function(|lua, (key, label): (Option<String>, Option<String>)| {
            let (key, label) = match (key, label) {
                (Some(key), Some(label)) if !label.trim().is_empty() => (key, label),
                _ => (String::new(), String::new()),
            };
            lua.app_data_mut::<ScriptCtx>()
                .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                .commands
                .push(ScriptCommand::Prompt { key, label });
            Ok(())
        })?,
    )?;
    // viber.localize(texto) — resolve as chaves `@nome` do locale ativo.
    // `say`/`toast` já o fazem por linha inteira; isto serve quem compõe
    // (ex.: `Falante: @chave` num grafo de diálogo em Lua).
    api.set(
        "localize",
        lua.create_function(|lua, text: String| {
            let table = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.locale_table.clone());
            Ok(crate::dialogue::localized_lines(&text, table.as_deref()))
        })?,
    )?;
    // viber.locale.get()/set(lang) — idioma ativo em runtime. A tabela é
    // recarregada de `locale/<lang>.json`; `Say`/`toast`/UI resolvem as
    // chaves `@…` no frame seguinte.
    {
        let locale = lua.create_table()?;
        locale.set(
            "get",
            lua.create_function(|lua, ()| {
                Ok(lua
                    .app_data_ref::<ScriptCtx>()
                    .and_then(|ctx| ctx.locale_lang.clone()))
            })?,
        )?;
        locale.set(
            "set",
            lua.create_function(|lua, lang: String| {
                let lang = lang.trim().to_ascii_lowercase();
                if lang.is_empty()
                    || !lang
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(mlua::Error::runtime(
                        "viber.locale.set: idioma inválido (use 'pt', 'en', …)",
                    ));
                }
                lua.app_data_mut::<ScriptCtx>()
                    .expect("ScriptCtx app data seeded in LuaScriptHost::new")
                    .commands
                    .push(ScriptCommand::SetLocale(lang));
                Ok(())
            })?,
        )?;
        api.set("locale", locale)?;
    }
    // viber.interacted(key) -> bool — tecla pressionada NESTE frame E
    // ESTA entidade é o alvo mais próximo dessa tecla
    // (`interact::InteractionFocus`). O alcance sozinho não chega: dois
    // NPC sobrepostos reagiam ambos ao mesmo [E].
    api.set(
        "interacted",
        lua.create_function(|lua, key: String| {
            let code = key_code_from_str(&key)
                .ok_or_else(|| mlua::Error::runtime(format!("tecla desconhecida '{key}'")))?;
            let ctx = lua.app_data_ref::<ScriptCtx>();
            let Some(ctx) = ctx.as_ref() else {
                return Ok(false);
            };
            if ctx.ui_typing {
                return Ok(false);
            }
            if !ctx.just_pressed.contains(&code) {
                return Ok(false);
            }
            let Some(entity) = ctx.entity else {
                return Ok(false);
            };
            // Sem foco publicado ainda (1.º frame, ou entidade que ainda
            // não registou `set_interaction`): cai no teste de alcance de
            // sempre, para não perder a interação do frame de arranque.
            if let Some(winner) = ctx.interaction_focus.get(&code).copied() {
                return Ok(winner == entity);
            }
            Ok(ctx
                .player
                .map(|p| {
                    p.distance(ctx.origin)
                        <= ctx
                            .interaction_range
                            .unwrap_or_else(crate::interact::default_range)
                })
                .unwrap_or(false))
        })?,
    )?;

    input::install(lua, &api)?;
    events::install(lua, &api)?;
    terrain::install(lua, &api)?;
    timers::install(lua, &api)?;
    entity::install(lua, &api)?;
    game::install(lua, &api)?;
    props::install(lua, &api)?;
    nlu::install(lua, &api)?;
    decision::install(lua, &api)?;
    super::signals::install(lua, &api)?;

    lua.globals().set("viber", api)?;
    let states = lua.create_table()?;
    lua.set_named_registry_value("viber_states", states)?;
    // Filas de eventos por-path (`viber.events`): path → array de tabelas.
    // A presença da chave É a subscrição (criada na 1.ª chamada).
    let events = lua.create_table()?;
    lua.set_named_registry_value("viber_events", events)?;
    // Filas de timers (`viber.after`/`every`): id → {at, period?, func, owner, path}.
    lua.set_named_registry_value("viber_timers", lua.create_table()?)?;
    let seq = lua.create_table()?;
    seq.raw_set("next", 1i64)?;
    lua.set_named_registry_value("viber_timer_seq", seq)?;
    lua.set_named_registry_value("viber_timer_cancelled", lua.create_table()?)?;
    // Estado de JOGO world-scoped (`viber.game()`) e cache de módulos
    // (`viber.load`) — ambos sobrevivem ao hot-reload.
    lua.set_named_registry_value("viber_game", lua.create_table()?)?;
    lua.set_named_registry_value("viber_modules", lua.create_table()?)?;
    Ok(())
}
