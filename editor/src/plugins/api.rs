//! Construção do `ctx` que cada callback de plugin recebe.
//!
//! Tudo o que o plugin vê é uma cópia imutável do estado do editor. As ações
//! (`editor.*`) só ENFILEIRAM — o host aplica-as depois de todos os callbacks,
//! para não haver mutações a meio de uma iteração. As únicas exceções são as
//! prefs e o FS, que são serviços e não estado da cena.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::math::Vec3;
use mlua::{Lua, Table, Value};
use serde_json::Value as Json;
use viber::scene::ops::EditOp;

use super::fs::ProjectFs;
use super::json;
use super::{
    Action, AllNode, AttachmentSpec, CameraPose, LogLevel, NodeSnapshot, QueuedAction, ToastKind,
    push,
};

/// O projeto como o plugin o vê. Caminhos relativos à raiz, com `/`.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub root: PathBuf,
    pub name: String,
    pub main_scene: String,
    pub scenes_dir: String,
    pub scripts_dir: String,
    pub assets_roots: Vec<String>,
}

/// O documento aberto, em informação.
#[derive(Debug, Clone)]
pub struct DocInfo {
    pub scene: String,
    pub revision: u64,
    pub dirty: bool,
}

/// Tudo o que `make_ctx` precisa. Tudo owned/clonável para as closures do
/// Luau poderem capturar sem lifetimes.
pub struct ContextInput {
    pub plugin: String,
    pub nodes: Arc<Vec<NodeSnapshot>>,
    pub all_nodes: Arc<Vec<AllNode>>,
    pub player: Option<Vec3>,
    pub camera: Option<CameraPose>,
    pub selection: Option<String>,
    pub project: ProjectInfo,
    pub doc: DocInfo,
    pub fs: ProjectFs,
    pub prefs: Arc<Mutex<BTreeMap<String, BTreeMap<String, Json>>>>,
    pub queue: Arc<Mutex<Vec<QueuedAction>>>,
    pub denied_write: Arc<Vec<String>>,
    pub time: f64,
    pub tick: u64,
}

pub fn make_ctx(lua: &Lua, input: &ContextInput) -> mlua::Result<Table> {
    let ctx = lua.create_table()?;
    ctx.set("dt", super::TICK)?;
    ctx.set("time", input.time)?;
    ctx.set("tick", input.tick)?;

    // Nós com `groups=` (o contrato antigo, mantido para o grip preview).
    let nodes = lua.create_table()?;
    for (index, node) in input.nodes.iter().enumerate() {
        let table = lua.create_table()?;
        table.set("selector", node.selector.as_str())?;
        table.set("name", node.name.as_deref())?;
        table.set("tag", node.tag.as_str())?;
        let groups = lua.create_table()?;
        for group in &node.groups {
            groups.set(group.as_str(), true)?;
        }
        table.set("groups", groups)?;
        let props = lua.create_table()?;
        for (key, value) in &node.props {
            props.set(key.as_str(), value.to_lua(lua)?)?;
        }
        table.set("props", props)?;
        nodes.set(index + 1, table)?;
    }
    ctx.set("nodes", nodes)?;

    match input.player {
        Some(position) => {
            let table = lua.create_table()?;
            table.set("x", position.x)?;
            table.set("y", position.y)?;
            table.set("z", position.z)?;
            ctx.set("player", table)?;
        }
        None => ctx.set("player", Value::Nil)?,
    }
    match input.selection.as_deref() {
        Some(value) => ctx.set("selection", value)?,
        None => ctx.set("selection", Value::Nil)?,
    }
    match input.camera {
        Some(pose) => {
            let table = lua.create_table()?;
            let target = lua.create_table()?;
            target.set("x", pose.target.x)?;
            target.set("y", pose.target.y)?;
            target.set("z", pose.target.z)?;
            table.set("target", target)?;
            table.set("distance", pose.distance)?;
            table.set("pitch", pose.pitch)?;
            table.set("yaw", pose.yaw)?;
            ctx.set("camera", table)?;
        }
        None => ctx.set("camera", Value::Nil)?,
    }

    ctx.set("project", project_table(lua, &input.project)?)?;
    ctx.set("doc", doc_table(lua, &input.doc)?)?;
    ctx.set("editor", editor_table(lua, input)?)?;
    Ok(ctx)
}

fn project_table(lua: &Lua, project: &ProjectInfo) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("root", project.root.display().to_string())?;
    table.set("name", project.name.as_str())?;
    table.set("main_scene", project.main_scene.as_str())?;
    table.set("scenes_dir", project.scenes_dir.as_str())?;
    table.set("scripts_dir", project.scripts_dir.as_str())?;
    let roots = lua.create_table()?;
    for (index, root) in project.assets_roots.iter().enumerate() {
        roots.set(index + 1, root.as_str())?;
    }
    table.set("assets_roots", roots)?;
    Ok(table)
}

fn doc_table(lua: &Lua, doc: &DocInfo) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("scene", doc.scene.as_str())?;
    table.set("revision", doc.revision)?;
    table.set("dirty", doc.dirty)?;
    Ok(table)
}

fn editor_table(lua: &Lua, input: &ContextInput) -> mlua::Result<Table> {
    let editor = lua.create_table()?;

    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "log",
        lua.create_function(move |_, message: String| {
            push(
                &queue,
                &plugin,
                Action::Log {
                    level: LogLevel::Info,
                    message,
                },
            )
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "warn",
        lua.create_function(move |_, message: String| {
            push(
                &queue,
                &plugin,
                Action::Log {
                    level: LogLevel::Warn,
                    message,
                },
            )
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "error",
        lua.create_function(move |_, message: String| {
            push(
                &queue,
                &plugin,
                Action::Log {
                    level: LogLevel::Error,
                    message,
                },
            )
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "toast",
        lua.create_function(move |_, (kind, message): (Option<String>, String)| {
            let kind = match kind.as_deref() {
                Some("success") => ToastKind::Success,
                Some("warn") => ToastKind::Warn,
                Some("error") => ToastKind::Error,
                _ => ToastKind::Info,
            };
            push(&queue, &plugin, Action::Toast { kind, message })
        })?,
    )?;

    // ── anexos / câmara (compatível com o grip_preview antigo) ────────────
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "attach",
        lua.create_function(move |_, table: Table| {
            let spec = AttachmentSpec::from_lua(&table)?;
            push(&queue, &plugin, Action::Attach(spec))
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "detach",
        lua.create_function(move |_, slot: String| push(&queue, &plugin, Action::Detach(slot)))?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    let has_player = input.player.is_some();
    editor.set(
        "focus_player",
        lua.create_function(move |_, table: Table| {
            let number = |key: &str, default: f32| -> mlua::Result<f32> {
                Ok(table.get::<Option<f32>>(key)?.unwrap_or(default))
            };
            push(
                &queue,
                &plugin,
                Action::Focus {
                    distance: number("distance", 5.0)?,
                    height: number("height", 1.1)?,
                    pitch: number("pitch", 20.0)?,
                    yaw: number("yaw", 0.0)?,
                },
            )?;
            Ok(has_player)
        })?,
    )?;

    // ── leitura da cena ─────────────────────────────────────────────────
    let all_nodes = input.all_nodes.clone();
    editor.set(
        "node",
        lua.create_function(move |lua, selector: String| {
            all_nodes
                .iter()
                .find(|node| node.selector == selector)
                .map(|node| node_table(lua, node))
                .transpose()
        })?,
    )?;
    let all_nodes = input.all_nodes.clone();
    editor.set(
        "find_nodes",
        lua.create_function(move |lua, filter: Option<Table>| {
            let filter = Filter::from_lua(filter.as_ref())?;
            let table = lua.create_table()?;
            let mut count = 0usize;
            for node in all_nodes.iter() {
                if !filter.matches(node) {
                    continue;
                }
                count += 1;
                if count > filter.limit {
                    break;
                }
                table.set(count, node_table(lua, node)?)?;
            }
            Ok(table)
        })?,
    )?;

    // ── seleção ─────────────────────────────────────────────────────────
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "select",
        lua.create_function(move |_, selector: String| {
            push(&queue, &plugin, Action::Select(vec![selector]))
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "select_many",
        lua.create_function(move |_, selectors: Table| {
            let mut out = Vec::new();
            for index in 1..=selectors.raw_len() {
                let selector: String = selectors.raw_get(index)?;
                out.push(selector);
            }
            push(&queue, &plugin, Action::Select(out))
        })?,
    )?;

    // ── edições undoable ────────────────────────────────────────────────
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "set_attr",
        lua.create_function(move |_, (selector, key, value): (String, String, Value)| {
            let value = scalar(value)?;
            let mut attrs = BTreeMap::new();
            attrs.insert(key.clone(), value);
            push(
                &queue,
                &plugin,
                Action::Edit {
                    label: format!("plugin {plugin}: {key}"),
                    ops: vec![EditOp::Set {
                        node: selector,
                        attrs,
                    }],
                },
            )
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "set_props",
        lua.create_function(move |_, (selector, props): (String, Table)| {
            let mut attrs = BTreeMap::new();
            for pair in props.pairs::<Value, Value>() {
                let (key, value) = pair?;
                let Value::String(key) = key else {
                    return Err(mlua::Error::runtime("set_props: chaves têm de ser strings"));
                };
                let key = key
                    .to_str()
                    .map_err(|_| mlua::Error::runtime("set_props: chave inválida"))?;
                if key.is_empty()
                    || !key
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
                {
                    return Err(mlua::Error::runtime(format!(
                        "set_props: chave inválida `{key}`"
                    )));
                }
                attrs.insert(format!("prop-{key}"), scalar(value)?);
            }
            if attrs.is_empty() {
                return Ok(());
            }
            push(
                &queue,
                &plugin,
                Action::Edit {
                    label: format!("plugin {plugin}: props"),
                    ops: vec![EditOp::Set {
                        node: selector,
                        attrs,
                    }],
                },
            )
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "insert",
        lua.create_function(
            move |_, (parent, xml, index): (String, String, Option<usize>)| {
                push(
                    &queue,
                    &plugin,
                    Action::Edit {
                        label: format!("plugin {plugin}: insert"),
                        ops: vec![EditOp::Insert { parent, index, xml }],
                    },
                )
            },
        )?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "remove",
        lua.create_function(move |_, selector: String| {
            push(
                &queue,
                &plugin,
                Action::Edit {
                    label: format!("plugin {plugin}: remove"),
                    ops: vec![EditOp::Remove { node: selector }],
                },
            )
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "move",
        lua.create_function(
            move |_, (selector, parent, index): (String, String, Option<usize>)| {
                push(
                    &queue,
                    &plugin,
                    Action::Edit {
                        label: format!("plugin {plugin}: move"),
                        ops: vec![EditOp::Move {
                            node: selector,
                            parent,
                            index,
                        }],
                    },
                )
            },
        )?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "duplicate",
        lua.create_function(move |_, selector: String| {
            push(
                &queue,
                &plugin,
                Action::Edit {
                    label: format!("plugin {plugin}: duplicate"),
                    ops: vec![EditOp::Duplicate { node: selector }],
                },
            )
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "extract",
        lua.create_function(move |_, (selector, to): (String, Option<String>)| {
            push(
                &queue,
                &plugin,
                Action::Edit {
                    label: format!("plugin {plugin}: extract"),
                    ops: vec![EditOp::Extract { node: selector, to }],
                },
            )
        })?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "create_file",
        lua.create_function(
            move |_, (path, xml, title): (String, String, Option<String>)| {
                push(
                    &queue,
                    &plugin,
                    Action::Edit {
                        label: format!("plugin {plugin}: create {path}"),
                        ops: vec![EditOp::CreateFile { path, xml, title }],
                    },
                )
            },
        )?,
    )?;

    // ── comandos do editor ──────────────────────────────────────────────
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "save",
        lua.create_function(move |_, ()| push(&queue, &plugin, Action::Save))?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "open_scene",
        lua.create_function(move |_, path: String| push(&queue, &plugin, Action::OpenScene(path)))?,
    )?;
    let queue = input.queue.clone();
    let plugin = input.plugin.clone();
    editor.set(
        "open_panel",
        lua.create_function(move |_, id: String| push(&queue, &plugin, Action::OpenPanel(id)))?,
    )?;

    // ── FS ──────────────────────────────────────────────────────────────
    let fs = input.fs.clone();
    editor.set("fs", fs_table(lua, fs, input.denied_write.clone())?)?;

    // ── prefs ───────────────────────────────────────────────────────────
    editor.set(
        "prefs",
        prefs_table(lua, input.prefs.clone(), input.plugin.clone())?,
    )?;

    Ok(editor)
}

fn fs_table(lua: &Lua, fs: ProjectFs, denied: Arc<Vec<String>>) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let handle = fs.clone();
    table.set(
        "read",
        lua.create_function(move |_, path: String| {
            Ok(match handle.read(&path) {
                Ok(text) => (Some(text), None),
                Err(error) => (None, Some(format!("{error:#}"))),
            })
        })?,
    )?;

    let handle = fs.clone();
    let denied_read = denied.clone();
    table.set(
        "write",
        lua.create_function(move |_, (path, contents): (String, String)| {
            if let Some(reason) = denied_path(&path, &denied_read) {
                return Ok((false, Some(reason)));
            }
            Ok(match handle.write(&path, &contents) {
                Ok(()) => (true, None),
                Err(error) => (false, Some(format!("{error:#}"))),
            })
        })?,
    )?;

    let handle = fs.clone();
    table.set(
        "list",
        lua.create_function(move |lua, path: String| match handle.list(&path) {
            Ok(entries) => {
                let out = lua.create_table()?;
                for (index, entry) in entries.iter().enumerate() {
                    let row = lua.create_table()?;
                    row.set("name", entry.name.as_str())?;
                    row.set("path", entry.path.as_str())?;
                    row.set("dir", entry.dir)?;
                    out.set(index + 1, row)?;
                }
                Ok((Some(out), None))
            }
            Err(error) => Ok((None, Some(format!("{error:#}")))),
        })?,
    )?;

    let handle = fs.clone();
    table.set(
        "exists",
        lua.create_function(move |_, path: String| Ok(handle.exists(&path)))?,
    )?;

    let handle = fs;
    table.set(
        "mkdir",
        lua.create_function(move |_, path: String| {
            Ok(match handle.mkdir(&path) {
                Ok(()) => (true, None),
                Err(error) => (false, Some(format!("{error:#}"))),
            })
        })?,
    )?;

    let json_table = lua.create_table()?;
    json_table.set(
        "array",
        lua.create_function(|lua, values: Table| {
            let out = lua.create_table()?;
            for index in 1..=values.raw_len() {
                out.raw_set(index, values.raw_get::<Value>(index)?)?;
            }
            let meta = lua.create_table()?;
            meta.set("__json_array", true)?;
            out.set_metatable(Some(meta))?;
            Ok(out)
        })?,
    )?;
    json_table.set(
        "decode",
        lua.create_function(
            move |lua, text: String| match serde_json::from_str::<Json>(&text) {
                Ok(value) => match json::json_to_lua(lua, &value) {
                    Ok(value) => Ok((Some(value), None)),
                    Err(error) => Ok((None, Some(error))),
                },
                Err(error) => Ok((None, Some(format!("json: {error}")))),
            },
        )?,
    )?;
    json_table.set(
        "encode",
        lua.create_function(move |_, (value, pretty): (Value, Option<bool>)| {
            let value = match json::lua_to_json(value) {
                Ok(value) => value,
                Err(error) => return Ok((None, Some(error))),
            };
            let text = if pretty.unwrap_or(true) {
                serde_json::to_string_pretty(&value)
            } else {
                serde_json::to_string(&value)
            };
            Ok(match text {
                Ok(text) => (Some(text), None),
                Err(error) => (None, Some(format!("json: {error}"))),
            })
        })?,
    )?;
    table.set("json", json_table)?;
    Ok(table)
}

fn prefs_table(
    lua: &Lua,
    prefs: Arc<Mutex<BTreeMap<String, BTreeMap<String, Json>>>>,
    plugin: String,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let store = prefs.clone();
    let id = plugin.clone();
    table.set(
        "get",
        lua.create_function(move |lua, key: String| {
            let guard = store
                .lock()
                .map_err(|_| mlua::Error::runtime("prefs: lock envenenado"))?;
            let Some(value) = guard.get(&id).and_then(|map| map.get(&key)) else {
                return Ok(Value::Nil);
            };
            json::json_to_lua(lua, value)
                .map_err(|error| mlua::Error::runtime(format!("prefs: {error}")))
        })?,
    )?;

    table.set(
        "set",
        lua.create_function(move |_, (key, value): (String, Value)| {
            let mut guard = prefs
                .lock()
                .map_err(|_| mlua::Error::runtime("prefs: lock envenenado"))?;
            let map = guard.entry(plugin.clone()).or_default();
            if matches!(value, Value::Nil) {
                map.remove(&key);
                return Ok((true, None));
            }
            match json::lua_to_json(value) {
                Ok(value) => {
                    map.insert(key, value);
                    Ok((true, None))
                }
                Err(error) => Ok((false, Some(error))),
            }
        })?,
    )?;
    Ok(table)
}

fn node_table(lua: &Lua, node: &AllNode) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("selector", node.selector.as_str())?;
    table.set("name", node.name.as_deref())?;
    table.set("tag", node.tag.as_str())?;
    table.set("script", node.script.as_deref())?;
    let attrs = lua.create_table()?;
    for (key, value) in &node.attrs {
        attrs.set(key.as_str(), value.as_str())?;
    }
    table.set("attrs", attrs)?;
    let props = lua.create_table()?;
    for (key, value) in &node.props {
        props.set(key.as_str(), value.to_lua(lua)?)?;
    }
    table.set("props", props)?;
    Ok(table)
}

/// Um filtro de `editor.find_nodes`.
struct Filter {
    name: Option<String>,
    tag: Option<String>,
    script: Option<String>,
    attrs: Vec<(String, String)>,
    limit: usize,
}

impl Filter {
    fn from_lua(table: Option<&Table>) -> mlua::Result<Self> {
        let Some(table) = table else {
            return Ok(Self {
                name: None,
                tag: None,
                script: None,
                attrs: Vec::new(),
                limit: 200,
            });
        };
        let mut attrs = Vec::new();
        if let Some(values) = table.get::<Option<Table>>("attr")? {
            for pair in values.pairs::<Value, Value>() {
                let (key, value) = pair?;
                let Value::String(key) = key else {
                    return Err(mlua::Error::runtime(
                        "find_nodes: attr precisa de chaves string",
                    ));
                };
                let key = key
                    .to_str()
                    .map_err(|_| mlua::Error::runtime("find_nodes: chave inválida"))?
                    .to_owned();
                let value = scalar(value)?
                    .ok_or_else(|| mlua::Error::runtime("find_nodes: attr não pode ser nil"))?;
                attrs.push((key, value));
            }
        }
        Ok(Self {
            name: table.get::<Option<String>>("name")?,
            tag: table.get::<Option<String>>("tag")?,
            script: table.get::<Option<String>>("script")?,
            attrs,
            limit: table
                .get::<Option<usize>>("limit")?
                .unwrap_or(200)
                .clamp(1, 1000),
        })
    }

    fn matches(&self, node: &AllNode) -> bool {
        if let Some(name) = &self.name
            && node.name.as_deref() != Some(name.as_str())
        {
            return false;
        }
        if let Some(tag) = &self.tag
            && !node.tag.eq_ignore_ascii_case(tag)
        {
            return false;
        }
        if let Some(script) = &self.script
            && node.script.as_deref() != Some(script.as_str())
        {
            return false;
        }
        self.attrs.iter().all(|(key, value)| {
            node.attrs
                .iter()
                .any(|(candidate, current)| candidate == key && current == value)
        })
    }
}

/// `nil` remove o atributo; strings/números/bools viram o texto canónico.
fn scalar(value: Value) -> mlua::Result<Option<String>> {
    Ok(match value {
        Value::Nil => None,
        Value::String(text) => Some(
            text.to_str()
                .map_err(|_| mlua::Error::runtime("valor com bytes inválidos"))?
                .to_owned(),
        ),
        Value::Integer(number) => Some(number.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Boolean(value) => Some(value.to_string()),
        other => {
            return Err(mlua::Error::runtime(format!(
                "valor {} não é um escalar",
                other.type_name()
            )));
        }
    })
}

fn denied_path(path: &str, denied: &[String]) -> Option<String> {
    let path = path.trim_start_matches("./");
    if denied.iter().any(|entry| entry == path) {
        return Some(format!(
            "plugin não escreve `{path}`: usa as operações de cena/editor"
        ));
    }
    if path == "config.yaml" || path.starts_with("world.data/") || path.starts_with("assets/") {
        return Some(format!("plugin não escreve em `{path}`"));
    }
    None
}
