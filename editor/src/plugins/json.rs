//! Conversão JSON ↔ tabelas Luau para a API dos plugins.
//!
//! Preferimos uma conversão explícita à feature `serialize` do mlua: o
//! contrato fica sob o nosso controlo (arrays com buracos são erro, tabelas
//! vazias viram objetos) e os limites de profundidade são os mesmos da UI.

use mlua::{Lua, Table, Value};
use serde_json::{Map, Number, Value as Json};

/// Profundidade máxima de uma tabela/valor convertido.
const MAX_DEPTH: usize = 64;
/// Tamanho máximo de uma string convertida (bytes).
const MAX_STRING: usize = 4 * 1024 * 1024;

/// Converte um valor Luau num valor JSON.
pub fn lua_to_json(value: Value) -> Result<Json, String> {
    convert_lua(value, 0)
}

fn convert_lua(value: Value, depth: usize) -> Result<Json, String> {
    if depth > MAX_DEPTH {
        return Err(format!("json: profundidade máxima excedida ({MAX_DEPTH})"));
    }
    Ok(match value {
        Value::Nil => Json::Null,
        Value::Boolean(value) => Json::Bool(value),
        Value::Integer(value) => Json::Number(value.into()),
        Value::Number(value) => Number::from_f64(value)
            .map(Json::Number)
            .ok_or_else(|| "json: número não finito (NaN/inf)".to_owned())?,
        Value::String(value) => {
            let text = value
                .to_str()
                .map_err(|_| "json: string com bytes inválidos (UTF-8)".to_owned())?;
            if text.len() > MAX_STRING {
                return Err("json: string demasiado longa".to_owned());
            }
            Json::String(text.to_owned())
        }
        Value::Table(table) => convert_table(&table, depth)?,
        other => {
            return Err(format!(
                "json: tipo {} não é representável em JSON",
                other.type_name()
            ));
        }
    })
}

fn convert_table(table: &Table, depth: usize) -> Result<Json, String> {
    let len = table.raw_len();
    let forced_array = table
        .metatable()
        .and_then(|meta| meta.get::<Option<bool>>("__json_array").ok().flatten())
        .unwrap_or(false);
    if forced_array && len == 0 {
        return Ok(Json::Array(Vec::new()));
    }
    if forced_array || len > 0 {
        // `raw_len` pára no primeiro buraco: um `{[1]='a',[3]='c'}` tem
        // len 1. Percorrer as chaves apanha o índice 3 e os arrays mistos.
        for pair in table.clone().pairs::<Value, Value>() {
            let (key, _) = pair.map_err(|error| format!("json: tabela: {error}"))?;
            match key {
                Value::Integer(index) if index >= 1 && (index as usize) <= len => {}
                Value::Integer(_) => {
                    return Err("json: array com buraco ou índice inválido".to_owned());
                }
                _ => {
                    return Err("json: array com chaves que não são índices".to_owned());
                }
            }
        }
        let mut items = Vec::with_capacity(len);
        for index in 1..=len {
            let value: Value = table
                .raw_get(index)
                .map_err(|error| format!("json: array[{index}]: {error}"))?;
            if matches!(value, Value::Nil) {
                return Err(format!("json: array com buraco no índice {index}"));
            }
            items.push(convert_lua(value, depth + 1)?);
        }
        return Ok(Json::Array(items));
    }
    let mut object = Map::new();
    for pair in table.clone().pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|error| format!("json: tabela: {error}"))?;
        let Value::String(key) = key else {
            return Err(
                "json: tabela com chave inteira mas sem array contíguo (buraco?)".to_owned(),
            );
        };
        let key = key
            .to_str()
            .map_err(|_| "json: chave com bytes inválidos (UTF-8)".to_owned())?
            .to_owned();
        object.insert(key, convert_lua(value, depth + 1)?);
    }
    Ok(Json::Object(object))
}

/// Converte um valor JSON num valor Luau.
pub fn json_to_lua(lua: &Lua, value: &Json) -> Result<Value, String> {
    convert_json(lua, value, 0)
}

fn convert_json(lua: &Lua, value: &Json, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!("json: profundidade máxima excedida ({MAX_DEPTH})"));
    }
    Ok(match value {
        Json::Null => Value::Nil,
        Json::Bool(value) => Value::Boolean(*value),
        Json::Number(value) => {
            if let Some(integer) = value.as_i64() {
                Value::Integer(integer)
            } else if let Some(unsigned) = value.as_u64() {
                if unsigned <= i64::MAX as u64 {
                    Value::Integer(unsigned as i64)
                } else {
                    Value::Number(unsigned as f64)
                }
            } else {
                Value::Number(
                    value
                        .as_f64()
                        .ok_or_else(|| "json: número inválido".to_owned())?,
                )
            }
        }
        Json::String(value) => Value::String(
            lua.create_string(value)
                .map_err(|error| format!("json: string: {error}"))?,
        ),
        Json::Array(items) => {
            let table = lua
                .create_table()
                .map_err(|error| format!("json: tabela: {error}"))?;
            for (index, item) in items.iter().enumerate() {
                table
                    .raw_set(index + 1, convert_json(lua, item, depth + 1)?)
                    .map_err(|error| format!("json: array: {error}"))?;
            }
            Value::Table(table)
        }
        Json::Object(object) => {
            let table = lua
                .create_table()
                .map_err(|error| format!("json: tabela: {error}"))?;
            for (key, item) in object {
                table
                    .raw_set(key.as_str(), convert_json(lua, item, depth + 1)?)
                    .map_err(|error| format!("json: objeto: {error}"))?;
            }
            Value::Table(table)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip(value: Json) {
        let lua = Lua::new();
        let lua_value = json_to_lua(&lua, &value).unwrap();
        let back = lua_to_json(lua_value).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn roundtrips_scalars_arrays_and_objects() {
        roundtrip(json!(null));
        roundtrip(json!(true));
        roundtrip(json!(42));
        roundtrip(json!(-7));
        roundtrip(json!(3.5));
        roundtrip(json!("olá"));
        roundtrip(json!([1, "two", false]));
        roundtrip(json!({"id": "q1", "objective": {"type": "kill", "count": 3}}));
    }

    #[test]
    fn empty_table_is_an_object() {
        let lua = Lua::new();
        let value: Value = lua.load("return {}").eval().unwrap();
        assert_eq!(lua_to_json(value).unwrap(), json!({}));
    }

    #[test]
    fn sparse_array_is_an_error() {
        let lua = Lua::new();
        let value: Value = lua.load("return { [1] = 'a', [3] = 'c' }").eval().unwrap();
        assert!(lua_to_json(value).is_err());
    }

    #[test]
    fn functions_and_nan_are_errors() {
        let lua = Lua::new();
        let value: Value = lua.load("return function() end").eval().unwrap();
        assert!(lua_to_json(value).is_err());
        let value: Value = lua.load("return 0/0").eval().unwrap();
        assert!(lua_to_json(value).is_err());
    }

    #[test]
    fn depth_is_bounded() {
        let lua = Lua::new();
        let mut source = String::from("local t = {}\nlocal c = t\n");
        for _ in 0..80 {
            source.push_str("c.child = {}\nc = c.child\n");
        }
        source.push_str("return t");
        let value: Value = lua.load(&source).eval().unwrap();
        assert!(lua_to_json(value).is_err());
    }
}
