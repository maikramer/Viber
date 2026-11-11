//! Estado de JOGO world-scoped e módulos partilhados:
//! - `viber.game()` → tabela ÚNICA por mundo (globals resetam no hot-reload;
//!   `game()` sobrevive, como `viber.state()` por entidade). É aqui que a
//!   lógica de jogo em Lua guarda o que era recurso Rust noutra vida.
//! - `viber.load("lib/x.lua")` → corre um chunk de `scripts/` em env próprio
//!   UMA vez por mundo e devolve o seu `return` (cache em `viber_modules`) —
//!   a base para fatorar FSM/bibliotecas partilhadas entre scripts.

use mlua::IntoLua;
use mlua::{Lua, Table};

use super::ctx::ScriptCtx;

/// Instala o grupo `game` na tabela `viber`.
pub(crate) fn install(lua: &Lua, api: &Table) -> mlua::Result<()> {
    // viber.game() -> tabela world-scoped.
    api.set(
        "game",
        lua.create_function(|lua, ()| {
            let game: Table = lua.named_registry_value("viber_game")?;
            Ok(game)
        })?,
    )?;

    // viber.load(path) -> valor devolvido pelo módulo (cacheado por path).
    api.set(
        "load",
        lua.create_function(|lua, path: String| {
            // Só caminhos RELATIVOS dentro de `scripts/` — `..` ou raiz
            // absoluta liam (e executavam) qualquer ficheiro do disco.
            let relative = std::path::Path::new(&path);
            let inside = relative.components().all(|c| {
                matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
            if path.is_empty() || !inside {
                return Err(mlua::Error::runtime(format!(
                    "viber.load('{path}'): só caminhos relativos a scripts/ (sem '..')"
                )));
            }
            // Chave do cache = caminho RESOLVIDO (sem o prefixo histórico
            // `scripts/`, que o loader aceita): a mesma chave em `modules` e
            // em `evict_module` (hot-reload) — senão as duas grafias criavam
            // dois módulos com estados separados e o reload não evictava.
            // Também NORMALIZA `.`/`//` (`./lib/x.lua` era outro módulo, e o
            // hot-reload só evictava a grafia do watcher — o estado antigo
            // ficava preso para sempre).
            let key: String = std::path::Path::new(path.strip_prefix("scripts/").unwrap_or(&path))
                .components()
                .filter_map(|component| match component {
                    std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                    // `..`/raiz já foram rejeitados acima; `.` cai.
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/");
            if key.is_empty() {
                return Err(mlua::Error::runtime(format!(
                    "viber.load('{path}'): caminho sem ficheiro"
                )));
            }
            let modules: Table = lua.named_registry_value("viber_modules")?;
            if let Ok(cached) = modules.raw_get::<mlua::Value>(key.clone())
                && !matches!(cached, mlua::Value::Nil)
            {
                return Ok(cached);
            }
            let dir = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.scripts_dir.clone())
                .ok_or_else(|| mlua::Error::runtime("viber.load sem scripts_dir"))?;
            let full = dir.join(&key);
            let code = std::fs::read_to_string(&full)
                .map_err(|e| mlua::Error::runtime(format!("viber.load('{path}'): {e}")))?;
            // Env sandbox próprio (mesma forma do chunk de entidade) — o
            // módulo não pode clobber globals de quem o carrega.
            let env = lua.create_table()?;
            let mt = lua.create_table()?;
            mt.set("__index", lua.globals())?;
            env.set_metatable(Some(mt))?;
            let chunk = lua
                .load(&code)
                .set_name(format!("={path}"))
                .set_environment(env)
                .into_function()?;
            let value: mlua::Value = chunk.call(())?;
            // Módulo sem `return` (só mexe em `viber.game()`) cacheia `true`,
            // como o `require` — com `nil` o cache falhava e o módulo
            // re-corria a CADA chamada.
            let value = match value {
                mlua::Value::Nil => mlua::Value::Boolean(true),
                other => other,
            };
            modules.raw_set(key, value.clone())?;
            Ok(value)
        })?,
    )?;

    // viber.load_json(caminho) -> valor. Caminho relativo à pasta do mundo
    // (pai de scripts/). Sem `..`. Objectos/arrays/números/strings/bool/null.
    api.set(
        "load_json",
        lua.create_function(|lua, path: String| {
            let relative = std::path::Path::new(&path);
            let inside = relative.components().all(|c| {
                matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
            if path.is_empty() || !inside {
                return Err(mlua::Error::runtime(format!(
                    "viber.load_json('{path}'): só caminhos relativos à pasta do mundo (sem '..')"
                )));
            }
            let scripts = lua
                .app_data_ref::<ScriptCtx>()
                .and_then(|ctx| ctx.scripts_dir.clone())
                .ok_or_else(|| mlua::Error::runtime("viber.load_json sem scripts_dir"))?;
            let world = scripts.parent().ok_or_else(|| {
                mlua::Error::runtime("viber.load_json: scripts/ sem pasta de mundo")
            })?;
            let full = world.join(&path);
            let text = std::fs::read_to_string(&full)
                .map_err(|e| mlua::Error::runtime(format!("viber.load_json('{path}'): {e}")))?;
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| mlua::Error::runtime(format!("viber.load_json('{path}'): {e}")))?;
            json_to_lua(lua, &value)
        })?,
    )?;
    Ok(())
}

/// `serde_json::Value` → valor Luau. Números inteiros que cabem em i64
/// entram como inteiros; o resto como float. `null` → nil.
pub(crate) fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        serde_json::Value::Null => mlua::Value::Nil,
        serde_json::Value::Bool(flag) => mlua::Value::Boolean(*flag),
        serde_json::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                mlua::Value::Integer(int)
            } else if let Some(float) = number.as_f64() {
                mlua::Value::Number(float)
            } else {
                mlua::Value::Nil
            }
        }
        serde_json::Value::String(text) => mlua::Value::String(lua.create_string(text)?),
        serde_json::Value::Array(items) => {
            let table = lua.create_table()?;
            for (index, item) in items.iter().enumerate() {
                table.raw_set(index + 1, json_to_lua(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
        serde_json::Value::Object(map) => {
            let table = lua.create_table()?;
            for (key, item) in map {
                table.raw_set(key.as_str(), json_to_lua(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
    })
}

/// Tira `path` do cache do `viber.load` (hot-reload de um módulo). Devolve
/// `true` se estava carregado.
pub fn evict_module(lua: &Lua, path: &str) -> bool {
    let Ok(modules) = lua.named_registry_value::<Table>("viber_modules") else {
        return false;
    };
    let cached = modules
        .raw_get::<mlua::Value>(path)
        .is_ok_and(|v| !matches!(v, mlua::Value::Nil));
    if cached {
        let _ = modules.raw_remove(path);
    }
    cached
}

/// Serializa o estado de jogo (`viber_game`) para JSON PLANO — só valores
/// primitivos (string/número/bool) são persistíveis; tabelas/funções são
/// IGNORADAS silenciosamente (o save é do jogo, não da VM).
pub fn game_to_json(lua: &Lua) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(game) = lua.named_registry_value::<Table>("viber_game") else {
        return out;
    };
    for pair in game.pairs::<String, mlua::Value>().flatten() {
        let value = match pair.1 {
            mlua::Value::Boolean(b) => serde_json::Value::Bool(b),
            mlua::Value::Integer(i) => serde_json::Value::Number(i.into()),
            mlua::Value::Number(n) => match serde_json::Number::from_f64(n) {
                Some(num) => serde_json::Value::Number(num),
                None => continue,
            },
            mlua::Value::String(st) => serde_json::Value::String(st.to_string_lossy()),
            _ => continue,
        };
        out.insert(pair.0, value);
    }
    out
}

/// Escreve chaves primitivas no `viber_game` (patch de estado).
///
/// Só `Bool`/`Number`/`String` entram; `Null` REMOVE a chave; arrays e
/// objectos são ignorados (o `viber.game()` é estado plano). Nada aqui é
/// lógica de jogo: quem compõe o patch sabe o que possui.
pub fn apply_patch(lua: &Lua, patch: &std::collections::BTreeMap<String, serde_json::Value>) {
    let Ok(game) = lua.named_registry_value::<Table>("viber_game") else {
        return;
    };
    for (key, value) in patch {
        match value {
            serde_json::Value::Bool(flag) => {
                let _ = game.raw_set(key.clone(), *flag);
            }
            serde_json::Value::Number(number) => {
                // Inteiro quando cabe em i64; o resto como float (como o load).
                if let Some(int) = number.as_i64() {
                    let _ = game.raw_set(key.clone(), int);
                } else if let Some(float) = number.as_f64() {
                    let _ = game.raw_set(key.clone(), float);
                }
            }
            serde_json::Value::String(text) => {
                let _ = game.raw_set(key.clone(), text.clone());
            }
            serde_json::Value::Null => {
                let _ = game.raw_remove(key.clone());
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {}
        }
    }
}

/// Repõe o JSON do save no `viber_game` (load). Chaves não primitivas no
/// ficheiro (editado à mão) são ignoradas — nunca partem o load.
pub fn json_to_game(lua: &Lua, kv: &std::collections::BTreeMap<String, serde_json::Value>) {
    let Ok(game) = lua.named_registry_value::<Table>("viber_game") else {
        return;
    };
    // Um load SUBSTITUI o estado do jogo. Sem limpar, as chaves que só
    // existem na sessão atual (quest aceite, item apanhado, marco assinado)
    // sobreviviam ao carregar um save anterior — `raw_set` só escrevia as
    // chaves do ficheiro. Recolhe primeiro: mutar durante `pairs` é UB.
    let stale: Vec<String> = game
        .clone()
        .pairs::<String, mlua::Value>()
        .flatten()
        .map(|(key, _)| key)
        .collect();
    for key in stale {
        let _ = game.raw_remove(key);
    }
    for (key, value) in kv {
        let lv = match value {
            serde_json::Value::Bool(b) => (*b).into_lua(lua),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    (i as f64).into_lua(lua)
                } else {
                    n.as_f64().unwrap_or(0.0).into_lua(lua)
                }
            }
            serde_json::Value::String(st) => st.clone().into_lua(lua),
            _ => continue,
        };
        if let Ok(lv) = lv {
            let _ = game.raw_set(key.clone(), lv);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    /// VM mínima com a tabela `viber_game` (sem a API `viber` instalada).
    fn lua_com_jogo() -> Lua {
        let lua = Lua::new();
        let game = lua.create_table().expect("tabela do jogo");
        lua.set_named_registry_value("viber_game", game)
            .expect("registar viber_game");
        lua
    }

    fn jogo(lua: &Lua) -> Table {
        lua.named_registry_value("viber_game").expect("viber_game")
    }

    #[test]
    fn patch_escreve_primitivos_e_le_de_volta() {
        let lua = lua_com_jogo();
        jogo(&lua).raw_set("hp", 10).unwrap();

        let mut patch = BTreeMap::new();
        patch.insert("hp".to_string(), json!(92.0));
        patch.insert("iframes".to_string(), json!(0.35));
        patch.insert("death_pending".to_string(), json!(true));
        patch.insert("save_title".to_string(), json!("Vael — nível 4"));
        apply_patch(&lua, &patch);

        let game = jogo(&lua);
        assert_eq!(game.raw_get::<f64>("hp").unwrap(), 92.0);
        assert_eq!(game.raw_get::<f64>("iframes").unwrap(), 0.35);
        assert!(game.raw_get::<bool>("death_pending").unwrap());
        assert_eq!(
            game.raw_get::<String>("save_title").unwrap(),
            "Vael — nível 4"
        );

        // O patch é idempotente e não toca nas outras chaves.
        apply_patch(&lua, &patch);
        assert_eq!(game.raw_get::<f64>("hp").unwrap(), 92.0);
    }

    #[test]
    fn null_remove_a_chave() {
        let lua = lua_com_jogo();
        jogo(&lua).raw_set("venom", 3.5).unwrap();
        jogo(&lua).raw_set("hp", 100).unwrap();

        let mut patch = BTreeMap::new();
        patch.insert("venom".to_string(), serde_json::Value::Null);
        apply_patch(&lua, &patch);

        let game = jogo(&lua);
        assert!(
            game.raw_get::<mlua::Value>("venom").unwrap().is_nil(),
            "null tem de remover"
        );
        assert_eq!(game.raw_get::<i64>("hp").unwrap(), 100, "as outras ficam");
    }

    #[test]
    fn arrays_e_objetos_sao_ignorados() {
        let lua = lua_com_jogo();
        jogo(&lua).raw_set("enemies", 1).unwrap();

        let mut patch = BTreeMap::new();
        patch.insert("enemies".to_string(), json!({ "wolf": 1 }));
        patch.insert("lista".to_string(), json!([1, 2, 3]));
        apply_patch(&lua, &patch);

        let game = jogo(&lua);
        assert_eq!(
            game.raw_get::<i64>("enemies").unwrap(),
            1,
            "um objecto não substitui a chave"
        );
        assert!(
            game.raw_get::<mlua::Value>("lista").unwrap().is_nil(),
            "um array não cria a chave"
        );
    }

    #[test]
    fn sem_tabela_do_jogo_nao_panica() {
        // VM sem `viber_game` (host a arrancar): o patch é um no-op.
        let lua = Lua::new();
        let mut patch = BTreeMap::new();
        patch.insert("hp".to_string(), json!(1.0));
        apply_patch(&lua, &patch);
    }
}
