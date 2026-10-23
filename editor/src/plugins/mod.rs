//! Editor plugins — the project's `editor/plugins/*.lua`, run by the editor.
//!
//! The editor owns only GENERIC capabilities; game rules live in the project.
//! A plugin returns a table (a manifest) with lifecycle callbacks and,
//! optionally, UI panels, commands, node templates and a lint provider. The
//! host validates the shape, isolates errors and applies queued actions after
//! every callback has returned.
//!
//! The full contract is in `editor/PLUGINS.md`; the game example is
//! `editor/plugins/` do projeto.

mod api;
mod fs;
mod json;
mod manifest;
mod ui;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bevy::prelude::*;
use mlua::{Function, Lua, LuaOptions, RegistryKey, StdLib, Table};
use serde_json::Value as Json;
use viber::attachment::{AttachmentRequest, AttachmentRequests, AttachmentSlots, find_bone};
use viber::luau::exports::{PropValue, ScriptExports, parse_exports, prop_name};
use viber::player::Player;
use viber::project::Project;
use viber::scene::doc::{NodeRef, SceneDoc};
use viber::scene::ops::EditOp;

use api::{ContextInput, DocInfo, ProjectInfo};
use fs::ProjectFs;
use manifest::Manifest;
pub use manifest::NodeTemplateSpec;
pub use ui::{ListItem, Tone, UiNode};

/// Folder (relative to the project root) the editor scans for plugins.
pub const PLUGIN_DIR: &str = "editor/plugins";
/// How often `on_update` runs; 10 Hz is responsive for inspector tweaks and
/// cheap enough to rebuild the group snapshot.
pub(super) const TICK: f32 = 0.1;
/// How often the plugin folder is checked for edits (hot reload).
const RELOAD_PERIOD: f32 = 1.0;

pub struct EditorPluginsPlugin {
    /// Project root (the folder with `config.yaml`).
    pub root: PathBuf,
}

impl Plugin for EditorPluginsPlugin {
    fn build(&self, app: &mut App) {
        let dir = self.root.join(PLUGIN_DIR);
        let host = match PluginHost::load(&self.root, &dir) {
            Ok(host) => host,
            Err(error) => {
                warn!("editor plugins: {error:#}");
                PluginHost::empty(&self.root, dir)
            }
        };
        app.insert_resource(host)
            .init_resource::<PluginUiEvents>()
            .init_resource::<ActivePluginPanel>()
            .add_systems(Startup, apply_plugin_prefs)
            .add_systems(PostUpdate, run_plugins);
    }
}

/// The panel selected in the Plugins dock. `None` = first available.
#[derive(Resource, Debug, Default, Clone, PartialEq, Eq)]
pub struct ActivePluginPanel(pub Option<String>);

/// A preview attachment requested by a plugin.
#[derive(Debug, Clone, PartialEq)]
struct AttachmentSpec {
    slot: String,
    url: String,
    bone: String,
    pos: Vec3,
    rot: Vec3,
    scale: f32,
}

impl AttachmentSpec {
    fn from_lua(table: &Table) -> mlua::Result<Self> {
        let slot: String = table.get("slot")?;
        let url: String = table.get("url")?;
        if slot.trim().is_empty() || url.trim().is_empty() {
            return Err(mlua::Error::runtime(
                "editor.attach: `slot` e `url` não podem ser vazios",
            ));
        }
        let number =
            |key: &str| -> mlua::Result<f32> { Ok(table.get::<Option<f32>>(key)?.unwrap_or(0.0)) };
        let bone: String = table.get("bone")?;
        if bone.trim().is_empty() {
            return Err(mlua::Error::runtime(
                "editor.attach: `bone` não pode ser vazio",
            ));
        }
        let scale = table.get::<Option<f32>>("scale")?.unwrap_or(1.0);
        if !scale.is_finite() || scale <= 0.0 {
            return Err(mlua::Error::runtime(
                "editor.attach: `scale` tem de ser finito e > 0",
            ));
        }
        let spec = Self {
            slot,
            url,
            bone,
            pos: Vec3::new(number("x")?, number("y")?, number("z")?),
            rot: Vec3::new(number("rx")?, number("ry")?, number("rz")?),
            scale,
        };
        if !spec.pos.is_finite() || !spec.rot.is_finite() {
            return Err(mlua::Error::runtime(
                "editor.attach: posição/rotação tem de ser finita",
            ));
        }
        Ok(spec)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToastKind {
    Info,
    Success,
    Warn,
    Error,
}

impl From<ToastKind> for crate::ui::toast::ToastKind {
    fn from(kind: ToastKind) -> Self {
        match kind {
            ToastKind::Info => Self::Info,
            ToastKind::Success => Self::Success,
            ToastKind::Warn => Self::Warn,
            ToastKind::Error => Self::Error,
        }
    }
}

enum Action {
    Attach(AttachmentSpec),
    Detach(String),
    Focus {
        distance: f32,
        height: f32,
        pitch: f32,
        yaw: f32,
    },
    Log {
        level: LogLevel,
        message: String,
    },
    Toast {
        kind: ToastKind,
        message: String,
    },
    Edit {
        label: String,
        ops: Vec<EditOp>,
    },
    Select(Vec<String>),
    Save,
    OpenScene(String),
    OpenPanel(String),
}

struct QueuedAction {
    plugin: String,
    action: Action,
}

/// One node in `ctx.nodes` (only nodes with `groups=`).
#[derive(Debug, Clone)]
struct NodeSnapshot {
    selector: String,
    name: Option<String>,
    tag: String,
    groups: Vec<String>,
    props: Vec<(String, PropValue)>,
}

/// Every node, for `editor.node`/`editor.find_nodes`.
#[derive(Debug, Clone)]
struct AllNode {
    selector: String,
    name: Option<String>,
    tag: String,
    script: Option<String>,
    attrs: Vec<(String, String)>,
    props: Vec<(String, PropValue)>,
}

struct LoadedPlugin {
    name: String,
    table: RegistryKey,
    /// Source file, for error messages.
    path: PathBuf,
    manifest: Manifest,
    /// `on_start` still has to run (first tick or after a hot reload).
    started: bool,
}

#[derive(Clone, Copy)]
struct CameraPose {
    target: Vec3,
    distance: f32,
    pitch: f32,
    yaw: f32,
}

/// A UI event (click, edit, selection) delivered on the next tick.
#[derive(Debug, Clone)]
pub struct PluginUiEvent {
    pub plugin: String,
    pub panel: String,
    pub kind: String,
    pub id: String,
    pub value: Option<String>,
    pub index: Option<usize>,
}

#[derive(Resource, Debug, Default)]
pub struct PluginUiEvents(pub Vec<PluginUiEvent>);

/// A loaded panel, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelInfo {
    pub plugin: String,
    pub id: String,
    pub title: String,
}

impl PanelInfo {
    pub fn key(&self) -> String {
        format!("{}:{}", self.plugin, self.id)
    }
}

/// A plugin command, as the palette sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInfo {
    pub plugin: String,
    pub id: String,
    pub title: String,
    pub keywords: String,
}

impl CommandInfo {
    pub fn key(&self) -> String {
        format!("{}:{}", self.plugin, self.id)
    }
}

/// A node template offered by a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTemplateInfo {
    pub plugin: String,
    pub spec: NodeTemplateSpec,
}

/// A tree already rendered by a plugin this tick.
#[derive(Debug, Clone)]
pub struct RenderedPanel {
    pub plugin: String,
    pub id: String,
    pub tree: UiNode,
}

impl RenderedPanel {
    pub fn key(&self) -> String {
        format!("{}:{}", self.plugin, self.id)
    }
}

/// A finding returned by a plugin (Problems merges it with the engine lint).
#[derive(Debug, Clone)]
pub struct PluginFinding {
    pub plugin: String,
    pub severity: viber::lint::Severity,
    pub kind: String,
    pub message: String,
}

#[derive(Resource)]
pub struct PluginHost {
    dir: PathBuf,
    lua: Option<Lua>,
    plugins: Vec<LoadedPlugin>,
    snapshot: Arc<Vec<NodeSnapshot>>,
    all_nodes: Arc<Vec<AllNode>>,
    snapshot_revision: u64,
    /// Parsed `--@export` schemas, keyed by script path + mtime.
    export_cache: HashMap<PathBuf, (Option<SystemTime>, Arc<ScriptExports>)>,
    /// Slot → last requested attachment, for idempotence across ticks.
    last_attachments: HashMap<String, AttachmentSpec>,
    /// Errors already logged (a broken plugin must not spam 10×/s).
    warned: HashSet<String>,
    timer: Timer,
    reload_timer: Timer,
    /// Source file → (mtime, length), for hot reload.
    sources: HashMap<PathBuf, (Option<SystemTime>, u64)>,
    /// Plugin id → load error (the plugin is not in `plugins`).
    errors: HashMap<String, String>,
    prefs: Arc<Mutex<BTreeMap<String, BTreeMap<String, Json>>>>,
    rendered: Vec<RenderedPanel>,
    /// Actions requested by palette commands; applied on the next tick.
    pending_actions: Vec<QueuedAction>,
    project: ProjectInfo,
    fs: ProjectFs,
    denied_write: Arc<Vec<String>>,
    tick: u64,
    time: f64,
}

impl PluginHost {
    fn load(root: &Path, dir: &Path) -> anyhow::Result<Self> {
        // ALL_SAFE in Luau: no `os`/`io`/`package`; `require` would read disk
        // and is not needed for single-file plugins.
        let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())?;
        lua.set_memory_limit(64 * 1024 * 1024)?;
        lua.globals().set("require", mlua::Value::Nil)?;
        let mut host = Self::empty(root, dir.to_path_buf());
        host.lua = Some(lua);
        host.poll_sources();
        Ok(host)
    }

    fn empty(root: &Path, dir: PathBuf) -> Self {
        Self {
            dir,
            lua: None,
            plugins: Vec::new(),
            snapshot: Arc::new(Vec::new()),
            all_nodes: Arc::new(Vec::new()),
            snapshot_revision: u64::MAX,
            export_cache: HashMap::new(),
            last_attachments: HashMap::new(),
            warned: HashSet::new(),
            timer: Timer::from_seconds(TICK, TimerMode::Repeating),
            reload_timer: Timer::from_seconds(RELOAD_PERIOD, TimerMode::Repeating),
            sources: HashMap::new(),
            errors: HashMap::new(),
            prefs: Arc::new(Mutex::new(BTreeMap::new())),
            rendered: Vec::new(),
            pending_actions: Vec::new(),
            project: project_info(root),
            fs: ProjectFs::new(root),
            denied_write: Arc::new(Vec::new()),
            tick: 0,
            time: 0.0,
        }
    }

    /// Re-checks the plugin folder; only changed/new/broken files are reloaded.
    /// A file that fails to compile keeps the previous working version.
    fn poll_sources(&mut self) -> bool {
        let Some(lua) = self.lua.as_ref() else {
            return false;
        };
        let paths = scan_plugin_paths(&self.dir);
        let present: HashSet<PathBuf> = paths.iter().cloned().collect();
        let mut changed = false;
        // Ficheiros apagados/movidos saem PRIMEIRO. Um plugin renomeado
        // (foo.lua → bar.lua, mesmo `id`) era carregado com o antigo ainda na
        // lista, recusado como "id duplicado", e o antigo era removido a
        // seguir: o plugin desaparecia até ao próximo toque no ficheiro.
        let old = std::mem::take(&mut self.plugins);
        let (kept, removed): (Vec<_>, Vec<_>) = old
            .into_iter()
            .partition(|plugin| present.contains(&plugin.path));
        self.plugins = kept;
        for plugin in removed {
            let _ = lua.remove_registry_value(plugin.table);
            self.sources.remove(&plugin.path);
            changed = true;
        }
        // Uma assinatura de um ficheiro que já não existe não serve para nada
        // (e impedia recarregar um ficheiro recriado com o mesmo conteúdo).
        self.sources.retain(|path, _| present.contains(path));
        for path in paths {
            let signature = file_signature(&path);
            if self.sources.get(&path) == Some(&signature) {
                continue;
            }
            match load_plugin(lua, &path) {
                Ok(plugin) => {
                    if let Some(index) = self.plugins.iter().position(|other| other.path == path) {
                        let old = self.plugins.remove(index);
                        let _ = lua.remove_registry_value(old.table);
                    }
                    if self.plugins.iter().any(|other| other.name == plugin.name) {
                        let error = format!("id de plugin duplicado: {}", plugin.name);
                        self.errors.insert(plugin.name.clone(), error.clone());
                        self.sources.insert(path, signature);
                        let _ = lua.remove_registry_value(plugin.table);
                        if self.warned.insert(format!("dup:{}:{error}", plugin.name)) {
                            warn!("editor plugin {}: {error}", plugin.path.display());
                        }
                        continue;
                    }
                    self.errors.remove(&plugin.name);
                    self.plugins.push(plugin);
                    self.sources.insert(path, signature);
                    changed = true;
                }
                Err(error) => {
                    let name = path
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "plugin".into());
                    self.errors.insert(name.clone(), format!("{error:#}"));
                    self.sources.insert(path, signature);
                    if self.warned.insert(format!("load:{name}:{error:#}")) {
                        warn!("editor plugin {name}: {error:#}");
                    }
                }
            }
        }
        if changed {
            self.rendered.clear();
            info!(
                "editor plugins: {} carregado(s) de {}",
                self.plugins.len(),
                self.dir.display()
            );
        }
        changed
    }

    pub fn panels(&self) -> Vec<PanelInfo> {
        let mut out = Vec::new();
        for plugin in &self.plugins {
            for panel in &plugin.manifest.panels {
                out.push(PanelInfo {
                    plugin: plugin.name.clone(),
                    id: panel.id.clone(),
                    title: panel.title.clone(),
                });
            }
        }
        out
    }

    pub fn commands(&self) -> Vec<CommandInfo> {
        let mut out = Vec::new();
        for plugin in &self.plugins {
            for command in &plugin.manifest.commands {
                out.push(CommandInfo {
                    plugin: plugin.name.clone(),
                    id: command.id.clone(),
                    title: command.title.clone(),
                    keywords: command.keywords.clone(),
                });
            }
        }
        out
    }

    pub fn node_templates(&self) -> Vec<NodeTemplateInfo> {
        let mut out = Vec::new();
        for plugin in &self.plugins {
            for spec in &plugin.manifest.nodes {
                out.push(NodeTemplateInfo {
                    plugin: plugin.name.clone(),
                    spec: spec.clone(),
                });
            }
        }
        out
    }

    pub fn panel_tree(&self, key: &str) -> Option<&UiNode> {
        self.rendered
            .iter()
            .find(|panel| panel.key() == key)
            .map(|panel| &panel.tree)
    }

    pub fn errors(&self) -> &HashMap<String, String> {
        &self.errors
    }

    pub fn plugin_dir(&self) -> &Path {
        &self.dir
    }

    pub fn plugin_state(&self) -> BTreeMap<String, BTreeMap<String, Json>> {
        self.prefs
            .lock()
            .map(|prefs| prefs.clone())
            .unwrap_or_default()
    }

    pub fn has_panel(&self, key: &str) -> bool {
        self.panels().iter().any(|panel| panel.key() == key)
    }

    /// Smoke de runtime (testes): corre `on_start`/`on_update`/`render` de
    /// todos os plugins contra um documento real e devolve os erros.
    #[cfg(test)]
    pub fn smoke_render(&mut self, doc: &SceneDoc) -> Vec<String> {
        let Some(lua) = self.lua.as_ref() else {
            return vec!["sem VM Luau".into()];
        };
        let mut cache = HashMap::new();
        let (snapshot, all_nodes) = build_snapshots(doc, &mut cache);
        let snapshot = Arc::new(snapshot);
        let all_nodes = Arc::new(all_nodes);
        let denied = Arc::new(denied_paths(doc));
        let mut errors = Vec::new();
        for plugin in &self.plugins {
            let Ok(table) = lua.registry_value::<Table>(&plugin.table) else {
                continue;
            };
            let queue = Arc::new(Mutex::new(Vec::new()));
            let ctx = match api::make_ctx(
                lua,
                &ContextInput {
                    plugin: plugin.name.clone(),
                    nodes: snapshot.clone(),
                    all_nodes: all_nodes.clone(),
                    player: None,
                    camera: None,
                    selection: None,
                    project: self.project.clone(),
                    doc: DocInfo {
                        scene: doc.relative_path(0),
                        revision: doc.revision,
                        dirty: doc.is_dirty(),
                    },
                    fs: self.fs.clone(),
                    prefs: self.prefs.clone(),
                    queue,
                    denied_write: denied.clone(),
                    time: self.time,
                    tick: self.tick,
                },
            ) {
                Ok(ctx) => ctx,
                Err(error) => {
                    errors.push(format!("{}: ctx: {error}", plugin.name));
                    continue;
                }
            };
            for callback in ["on_start", "on_update"] {
                if let Ok(Some(function)) = table.get::<Option<Function>>(callback)
                    && let Err(error) = function.call::<()>(ctx.clone())
                {
                    errors.push(format!("{}: {callback}: {error}", plugin.name));
                }
            }
            let Some(panels) = table.get::<Option<Table>>("panels").ok().flatten() else {
                continue;
            };
            for panel in &plugin.manifest.panels {
                let Ok(Some(panel_table)) = panels.raw_get::<Option<Table>>(panel.index) else {
                    continue;
                };
                let Ok(Some(render)) = panel_table.get::<Option<Function>>("render") else {
                    continue;
                };
                match render.call::<mlua::Value>(ctx.clone()) {
                    Ok(value) => match UiNode::from_lua(value) {
                        Ok(tree) => {
                            if let Ok(Some(callback)) = table.get::<Option<Function>>("on_event") {
                                let mut events = Vec::new();
                                collect_events(&tree, &panel.id, &mut events);
                                for event in events {
                                    let Ok(lua_event) = lua.create_table() else {
                                        continue;
                                    };
                                    let _ = lua_event.set("kind", event.kind);
                                    let _ = lua_event.set("panel", event.panel.as_str());
                                    let _ = lua_event.set("id", event.id.as_str());
                                    let _ = lua_event.set("value", event.value.as_deref());
                                    let _ = lua_event.set("index", event.index);
                                    if let Err(error) =
                                        callback.call::<()>((ctx.clone(), lua_event))
                                    {
                                        errors.push(format!(
                                            "{} ({}): on_event {}: {error}",
                                            plugin.name, panel.id, event.id
                                        ));
                                    }
                                }
                                // Segundo render: apanha estados inválidos
                                // depois de uma edição de formulário.
                                if let Ok(Some(render)) =
                                    panel_table.get::<Option<Function>>("render")
                                    && let Ok(value) = render.call::<mlua::Value>(ctx.clone())
                                    && let Err(error) = UiNode::from_lua(value)
                                {
                                    errors.push(format!(
                                        "{} ({}): render depois de event: {error}",
                                        plugin.name, panel.id
                                    ));
                                }
                            }
                        }
                        Err(error) => {
                            errors.push(format!("{} ({}): {error}", plugin.name, panel.id));
                        }
                    },
                    Err(error) => {
                        errors.push(format!("{} ({}): render: {error}", plugin.name, panel.id));
                    }
                }
            }
            if let Ok(Some(command)) = table.get::<Option<Function>>("on_command") {
                for spec in &plugin.manifest.commands {
                    if let Err(error) = command.call::<()>((ctx.clone(), spec.id.clone())) {
                        errors.push(format!("{}: on_command {}: {error}", plugin.name, spec.id));
                    }
                }
            }
        }
        errors
    }

    /// Runs every plugin's `lint(ctx)` and collects the findings.
    pub fn lint(&self, doc: &SceneDoc) -> Vec<PluginFinding> {
        let Some(lua) = self.lua.as_ref() else {
            return Vec::new();
        };
        let mut cache = HashMap::new();
        let (snapshot, all_nodes) = build_snapshots(doc, &mut cache);
        let snapshot = Arc::new(snapshot);
        let all_nodes = Arc::new(all_nodes);
        let denied = Arc::new(denied_paths(doc));
        let mut findings = Vec::new();
        for plugin in &self.plugins {
            if !plugin.manifest.has_lint {
                continue;
            }
            let Ok(table) = lua.registry_value::<Table>(&plugin.table) else {
                continue;
            };
            let Ok(Some(lint)) = table.get::<Option<Function>>("lint") else {
                continue;
            };
            let queue = Arc::new(Mutex::new(Vec::new()));
            let ctx = match api::make_ctx(
                lua,
                &ContextInput {
                    plugin: plugin.name.clone(),
                    nodes: snapshot.clone(),
                    all_nodes: all_nodes.clone(),
                    player: None,
                    camera: None,
                    selection: None,
                    project: self.project.clone(),
                    doc: DocInfo {
                        scene: doc.relative_path(0),
                        revision: doc.revision,
                        dirty: doc.is_dirty(),
                    },
                    fs: self.fs.clone(),
                    prefs: self.prefs.clone(),
                    queue,
                    denied_write: denied.clone(),
                    time: self.time,
                    tick: self.tick,
                },
            ) {
                Ok(ctx) => ctx,
                Err(error) => {
                    warn!("editor plugin {}: lint ctx: {error}", plugin.name);
                    continue;
                }
            };
            match lint.call::<mlua::Value>(ctx) {
                Ok(mlua::Value::Table(rows)) => {
                    for index in 1..=rows.raw_len() {
                        let Ok(Some(row)) = rows.raw_get::<Option<Table>>(index) else {
                            continue;
                        };
                        let severity = row
                            .get::<Option<String>>("severity")
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| "warning".into());
                        let kind = row
                            .get::<Option<String>>("kind")
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| "plugin".into());
                        let message = row
                            .get::<Option<String>>("message")
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        if message.trim().is_empty() {
                            continue;
                        }
                        findings.push(PluginFinding {
                            plugin: plugin.name.clone(),
                            severity: if severity.eq_ignore_ascii_case("error") {
                                viber::lint::Severity::Error
                            } else {
                                viber::lint::Severity::Warning
                            },
                            kind,
                            message,
                        });
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    warn!("editor plugin {}: lint: {error}", plugin.name);
                    findings.push(PluginFinding {
                        plugin: plugin.name.clone(),
                        severity: viber::lint::Severity::Warning,
                        kind: "plugin-lint".into(),
                        message: format!("lint do plugin falhou: {error}"),
                    });
                }
            }
        }
        findings
    }
}

/// Runs a plugin command from the palette. Actions are queued and applied on
/// the next tick (where `run_plugins` already has the whole world).
pub fn invoke_command(world: &mut World, key: &str) {
    let Some((plugin_id, command_id)) = key.split_once(':') else {
        return;
    };
    let (project, fs, prefs, time, tick, lua_present) = {
        let host = world.resource::<PluginHost>();
        (
            host.project.clone(),
            host.fs.clone(),
            host.prefs.clone(),
            host.time,
            host.tick,
            host.lua.is_some(),
        )
    };
    if !lua_present {
        return;
    }
    let (snapshot, all_nodes, denied, selection, doc_info) = {
        let doc = world.resource::<SceneDoc>();
        let mut cache = HashMap::new();
        let (snapshot, all_nodes) = build_snapshots(doc, &mut cache);
        let selection = world
            .resource::<crate::scene::Selection>()
            .primary()
            .map(|node| doc.selector(node));
        (
            Arc::new(snapshot),
            Arc::new(all_nodes),
            Arc::new(denied_paths(doc)),
            selection,
            DocInfo {
                scene: doc.relative_path(0),
                revision: doc.revision,
                dirty: doc.is_dirty(),
            },
        )
    };
    let queue = Arc::new(Mutex::new(Vec::new()));
    {
        let host = world.resource::<PluginHost>();
        let Some(lua) = host.lua.as_ref() else {
            return;
        };
        let Some(plugin) = host.plugins.iter().find(|plugin| {
            plugin.name == plugin_id
                && plugin
                    .manifest
                    .commands
                    .iter()
                    .any(|command| command.id == command_id)
        }) else {
            return;
        };
        let Ok(table) = lua.registry_value::<Table>(&plugin.table) else {
            return;
        };
        let Ok(Some(command)) = table.get::<Option<Function>>("on_command") else {
            return;
        };
        let ctx = match api::make_ctx(
            lua,
            &ContextInput {
                plugin: plugin.name.clone(),
                nodes: snapshot,
                all_nodes,
                player: None,
                camera: None,
                selection,
                project,
                doc: doc_info,
                fs,
                prefs,
                queue: queue.clone(),
                denied_write: denied,
                time,
                tick,
            },
        ) {
            Ok(ctx) => ctx,
            Err(error) => {
                warn!("editor plugin {plugin_id}: command ctx: {error}");
                return;
            }
        };
        if let Err(error) = command.call::<()>((ctx, command_id.to_owned())) {
            warn!("editor plugin {plugin_id}: comando `{command_id}`: {error}");
        }
    }
    let actions = queue
        .lock()
        .map(|mut queue| std::mem::take(&mut *queue))
        .unwrap_or_default();
    if let Some(mut host) = world.get_resource_mut::<PluginHost>() {
        host.pending_actions.extend(actions);
    }
}

/// Copies the persisted plugin prefs into the host once the prefs resource exists.
fn apply_plugin_prefs(
    prefs: Option<Res<crate::prefs::EditorPrefs>>,
    host: ResMut<PluginHost>,
    mut panel: ResMut<ActivePluginPanel>,
) {
    let Some(prefs) = prefs else {
        return;
    };
    if let Ok(mut store) = host.prefs.lock() {
        *store = prefs.plugin_state.clone();
    }
    if let Some(key) = prefs.plugin_panel.clone()
        && host.has_panel(&key)
    {
        panel.0 = Some(key);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_plugins(
    time: Res<Time<Real>>,
    mut host: ResMut<PluginHost>,
    doc: Res<SceneDoc>,
    mut selection: ResMut<crate::scene::Selection>,
    mut pending: ResMut<crate::scene::PendingEdits>,
    mut requests: ResMut<AttachmentRequests>,
    slots: Res<AttachmentSlots>,
    players: Query<(Entity, &GlobalTransform), With<Player>>,
    children: Query<&Children>,
    names: Query<&Name>,
    mut camera: Option<ResMut<viber::editor_camera::EditorCamState>>,
    mut active_tab: Option<ResMut<crate::ui::layout::ActiveDockTab>>,
    mut active_panel: Option<ResMut<ActivePluginPanel>>,
    mut open_scene: Option<ResMut<crate::ui::scene_tabs::OpenSceneRequest>>,
    mut ui_events: Option<ResMut<PluginUiEvents>>,
    mut commands: Commands,
) {
    if host.lua.is_none() {
        return;
    }
    if !host.timer.tick(time.delta()).just_finished() {
        return;
    }
    host.tick = host.tick.wrapping_add(1);
    host.time = time.elapsed_secs_f64();
    let mut actions = std::mem::take(&mut host.pending_actions);

    if host.reload_timer.tick(time.delta()).just_finished() {
        host.poll_sources();
    }

    let player = players
        .iter()
        .next()
        .map(|(entity, transform)| (entity, transform.translation()));

    let PluginHost {
        lua,
        plugins,
        snapshot,
        all_nodes,
        snapshot_revision,
        export_cache,
        last_attachments,
        warned,
        rendered,
        project,
        fs,
        denied_write,
        prefs,
        tick,
        time: now,
        ..
    } = &mut *host;
    let Some(lua) = lua.as_ref() else {
        return;
    };

    if *snapshot_revision != doc.revision {
        let (grouped, all) = build_snapshots(&doc, export_cache);
        *snapshot = Arc::new(grouped);
        *all_nodes = Arc::new(all);
        *snapshot_revision = doc.revision;
        *denied_write = Arc::new(denied_paths(&doc));
    }

    let camera_pose = camera.as_ref().map(|camera| CameraPose {
        target: camera.target,
        distance: camera.distance,
        pitch: camera.pitch_deg,
        yaw: camera.yaw_deg,
    });
    let selected = selection.primary().map(|node| doc.selector(node));
    let events = ui_events
        .as_mut()
        .map(|events| std::mem::take(&mut events.0))
        .unwrap_or_default();
    let events: Vec<PluginUiEvent> = events.into_iter().take(256).collect();
    rendered.clear();

    let queue = Arc::new(Mutex::new(Vec::<QueuedAction>::new()));
    let doc_info = DocInfo {
        scene: doc.relative_path(0),
        revision: doc.revision,
        dirty: doc.is_dirty(),
    };

    for plugin in plugins.iter_mut() {
        let Ok(table) = lua.registry_value::<Table>(&plugin.table) else {
            continue;
        };
        let plugin_name = plugin.name.clone();
        let plugin_path = plugin.path.clone();
        let ctx = match api::make_ctx(
            lua,
            &ContextInput {
                plugin: plugin_name.clone(),
                nodes: snapshot.clone(),
                all_nodes: all_nodes.clone(),
                player: player.map(|(_, position)| position),
                camera: camera_pose,
                selection: selected.clone(),
                project: project.clone(),
                doc: doc_info.clone(),
                fs: fs.clone(),
                prefs: prefs.clone(),
                queue: queue.clone(),
                denied_write: denied_write.clone(),
                time: *now,
                tick: *tick,
            },
        ) {
            Ok(ctx) => ctx,
            Err(error) => {
                warn!("editor plugin {}: {error}", plugin_path.display());
                continue;
            }
        };

        if !plugin.started {
            plugin.started = true;
            if let Ok(Some(start)) = table.get::<Option<Function>>("on_start")
                && let Err(error) = start.call::<()>(ctx.clone())
                && warned.insert(format!("{plugin_name}:start:{error}"))
            {
                warn!("editor plugin {}: on_start: {error}", plugin_path.display());
            }
        }
        if let Ok(Some(update)) = table.get::<Option<Function>>("on_update")
            && let Err(error) = update.call::<()>(ctx.clone())
            && warned.insert(format!("{plugin_name}:{error}"))
        {
            warn!("editor plugin {}: {error}", plugin_path.display());
        }
        for event in events.iter().filter(|event| event.plugin == plugin_name) {
            let Ok(Some(callback)) = table.get::<Option<Function>>("on_event") else {
                continue;
            };
            let Ok(table_event) = lua.create_table() else {
                continue;
            };
            let _ = table_event.set("kind", event.kind.as_str());
            let _ = table_event.set("panel", event.panel.as_str());
            let _ = table_event.set("id", event.id.as_str());
            let _ = table_event.set("value", event.value.as_deref());
            let _ = table_event.set("index", event.index);
            if let Err(error) = callback.call::<()>((ctx.clone(), table_event))
                && warned.insert(format!("{plugin_name}:event:{error}"))
            {
                warn!("editor plugin {}: on_event: {error}", plugin_path.display());
            }
        }
        for panel in &plugin.manifest.panels {
            let tree = (|| -> Result<UiNode, String> {
                let panels = table
                    .get::<Option<Table>>("panels")
                    .map_err(|error| format!("panels: {error}"))?
                    .ok_or_else(|| "painéis desapareceram".to_owned())?;
                let panel_table = panels
                    .raw_get::<Option<Table>>(panel.index)
                    .map_err(|error| format!("panels[{}]: {error}", panel.index))?
                    .ok_or_else(|| format!("panels[{}] desapareceu", panel.index))?;
                let render = panel_table
                    .get::<Option<Function>>("render")
                    .map_err(|error| format!("render: {error}"))?
                    .ok_or_else(|| format!("painel `{}` sem render", panel.id))?;
                let value = render
                    .call::<mlua::Value>(ctx.clone())
                    .map_err(|error| error.to_string())?;
                UiNode::from_lua(value)
            })();
            let tree = match tree {
                Ok(tree) => tree,
                Err(error) => {
                    if warned.insert(format!("{plugin_name}:{}:{error}", panel.id)) {
                        warn!("editor plugin {plugin_name} ({}): {error}", panel.id);
                    }
                    UiNode::error(error)
                }
            };
            rendered.push(RenderedPanel {
                plugin: plugin_name.clone(),
                id: panel.id.clone(),
                tree,
            });
        }
    }

    if let Ok(mut queue) = queue.lock() {
        actions.extend(std::mem::take(&mut *queue));
    }
    for queued in actions {
        apply_action(
            queued,
            &doc,
            player,
            &mut pending,
            &mut selection,
            &mut requests,
            &slots,
            &children,
            &names,
            last_attachments,
            camera.as_deref_mut(),
            active_tab.as_deref_mut(),
            active_panel.as_deref_mut(),
            open_scene.as_deref_mut(),
            &mut commands,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_action(
    queued: QueuedAction,
    doc: &SceneDoc,
    player: Option<(Entity, Vec3)>,
    pending: &mut crate::scene::PendingEdits,
    selection: &mut crate::scene::Selection,
    requests: &mut AttachmentRequests,
    slots: &AttachmentSlots,
    children: &Query<&Children>,
    names: &Query<&Name>,
    last_attachments: &mut HashMap<String, AttachmentSpec>,
    camera: Option<&mut viber::editor_camera::EditorCamState>,
    active_tab: Option<&mut crate::ui::layout::ActiveDockTab>,
    active_panel: Option<&mut ActivePluginPanel>,
    open_scene: Option<&mut crate::ui::scene_tabs::OpenSceneRequest>,
    commands: &mut Commands,
) {
    let plugin = queued.plugin;
    match queued.action {
        Action::Attach(spec) => {
            let Some((owner, _)) = player else {
                return;
            };
            // The hero GLB loads asynchronously: without the bone the request
            // was dropped and retried at 10 Hz. Wait for it.
            if find_bone(owner, &spec.bone, children, names).is_none() {
                return;
            }
            let slot = format!("editor-plugin:{plugin}:{}", spec.slot);
            let attached = slots.0.contains_key(&(owner, slot.clone()));
            if last_attachments.get(&slot) != Some(&spec) || !attached {
                requests.0.push(AttachmentRequest {
                    owner: Some(owner),
                    slot: slot.clone(),
                    url: Some(spec.url.clone()),
                    bone: spec.bone.clone(),
                    pos: spec.pos,
                    rot: spec.rot,
                    scale: spec.scale.max(0.001),
                });
                last_attachments.insert(slot, spec);
            }
        }
        Action::Detach(slot) => {
            let slot = format!("editor-plugin:{plugin}:{slot}");
            if last_attachments.remove(&slot).is_some()
                && let Some((owner, _)) = player
            {
                requests.0.push(AttachmentRequest {
                    owner: Some(owner),
                    slot,
                    url: None,
                    bone: String::new(),
                    pos: Vec3::ZERO,
                    rot: Vec3::ZERO,
                    scale: 1.0,
                });
            }
        }
        Action::Focus {
            distance,
            height,
            pitch,
            yaw,
        } => {
            let (Some((_, position)), Some(camera)) = (player, camera) else {
                return;
            };
            if !distance.is_finite()
                || !height.is_finite()
                || !pitch.is_finite()
                || !yaw.is_finite()
            {
                return;
            }
            camera.target = position + Vec3::Y * height;
            camera.distance = distance.clamp(
                viber::editor_camera::MIN_DISTANCE,
                viber::editor_camera::MAX_DISTANCE,
            );
            camera.pitch_deg = pitch.clamp(
                viber::editor_camera::MIN_PITCH_DEG,
                viber::editor_camera::MAX_PITCH_DEG,
            );
            camera.yaw_deg = yaw;
        }
        Action::Log { level, message } => match level {
            LogLevel::Info => info!("[editor plugin {plugin}] {message}"),
            LogLevel::Warn => warn!("[editor plugin {plugin}] {message}"),
            LogLevel::Error => error!("[editor plugin {plugin}] {message}"),
        },
        Action::Toast { kind, message } => {
            let kind: crate::ui::toast::ToastKind = kind.into();
            commands.queue(move |world: &mut World| {
                crate::ui::toast::push(world, kind, message);
            });
        }
        Action::Edit { label, ops } => {
            pending.push(label, ops);
        }
        Action::Select(selectors) => {
            let nodes: Vec<NodeRef> = selectors
                .iter()
                .filter_map(|selector| doc.resolve(selector).ok())
                .collect();
            if !nodes.is_empty() {
                selection.0 = nodes;
            }
        }
        Action::Save => {
            commands.queue(|world: &mut World| crate::scene::save_world(world));
        }
        Action::OpenScene(path) => {
            if let Some(open) = open_scene {
                open.0 = Some(doc.root_dir.join(path));
            }
        }
        Action::OpenPanel(id) => {
            if let Some(tab) = active_tab {
                tab.0 = crate::ui::layout::TabKind::Plugins;
            }
            if let Some(panel) = active_panel {
                panel.0 = Some(format!("{plugin}:{id}"));
            }
        }
    }
}

/// Every node with a `groups=` attribute (the old contract) and every node
/// (for `editor.node`/`find_nodes`). Scripted nodes carry their resolved
/// `--@export` values, the same `viber.prop` does at runtime.
fn build_snapshots(
    doc: &SceneDoc,
    cache: &mut HashMap<PathBuf, (Option<SystemTime>, Arc<ScriptExports>)>,
) -> (Vec<NodeSnapshot>, Vec<AllNode>) {
    let mut grouped = Vec::new();
    let mut all = Vec::new();
    doc.walk(|node_ref, xml| {
        let script = xml.attr("script").map(str::to_owned);
        let props = script
            .as_deref()
            .map(|script| resolve_props(doc, script, &xml.attrs, cache))
            .unwrap_or_default();
        let selector = doc.selector(node_ref);
        if let Some(groups) = xml.attr("groups") {
            let groups: Vec<String> = groups.split_whitespace().map(str::to_owned).collect();
            if !groups.is_empty() {
                grouped.push(NodeSnapshot {
                    selector: selector.clone(),
                    name: xml.attr("name").map(str::to_owned),
                    tag: xml.tag.clone(),
                    groups,
                    props: props.clone(),
                });
            }
        }
        all.push(AllNode {
            selector,
            name: xml.attr("name").map(str::to_owned),
            tag: xml.tag.clone(),
            script,
            attrs: xml.attrs.clone(),
            props,
        });
    });
    for file in 0..doc.files.len() {
        let root = NodeRef::root(file);
        let file_doc = &doc.files[file];
        let attrs = file_doc.doc.root_attrs.clone();
        let script = attrs
            .iter()
            .find(|(key, _)| key == "script")
            .map(|(_, value)| value.clone());
        let props = script
            .as_deref()
            .map(|script| resolve_props(doc, script, &attrs, cache))
            .unwrap_or_default();
        all.push(AllNode {
            selector: doc.selector(&root),
            name: attrs
                .iter()
                .find(|(key, _)| key == "name")
                .map(|(_, value)| value.clone()),
            tag: file_doc.doc.root_tag.clone(),
            script,
            attrs,
            props,
        });
    }
    (grouped, all)
}

fn resolve_props(
    doc: &SceneDoc,
    script: &str,
    attrs: &[(String, String)],
    cache: &mut HashMap<PathBuf, (Option<SystemTime>, Arc<ScriptExports>)>,
) -> Vec<(String, PropValue)> {
    let path = viber::lint::resolve_script(&viber::scene::scripts_dir(doc), script);
    let modified = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    let exports = if let Some((cached_modified, exports)) = cache.get(&path)
        && *cached_modified == modified
    {
        exports.clone()
    } else {
        let Ok(source) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        let exports = Arc::new(parse_exports(&source));
        cache.insert(path, (modified, exports.clone()));
        exports
    };
    let raw: Vec<(String, String)> = attrs
        .iter()
        .filter_map(|(key, value)| prop_name(key).map(|name| (name.to_owned(), value.clone())))
        .collect();
    exports
        .resolve(&raw)
        .into_iter()
        .filter_map(|prop| prop.value.map(|value| (prop.def.name.clone(), value)))
        .collect()
}

fn denied_paths(doc: &SceneDoc) -> Vec<String> {
    (0..doc.files.len())
        .map(|file| doc.relative_path(file))
        .collect()
}

fn project_info(root: &Path) -> ProjectInfo {
    let project = Project::at(root);
    let config = viber::config::load(root).ok();
    ProjectInfo {
        root: root.to_path_buf(),
        name: config
            .as_ref()
            .and_then(|config| config.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| {
                root.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "project".into())
            }),
        main_scene: project
            .config
            .main_scene()
            .to_string_lossy()
            .replace('\\', "/"),
        scenes_dir: project
            .config
            .scenes_dir()
            .to_string_lossy()
            .replace('\\', "/"),
        scripts_dir: config
            .as_ref()
            .map(|config| config.game.scripts_dir.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| "scripts".into()),
        assets_roots: config
            .as_ref()
            .map(|config| {
                config
                    .assets
                    .roots
                    .iter()
                    .map(|path| path.to_string_lossy().replace('\\', "/"))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn scan_plugin_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext == "lua" || ext == "luau")
                })
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

fn file_signature(path: &Path) -> (Option<SystemTime>, u64) {
    let metadata = std::fs::metadata(path).ok();
    (
        metadata.as_ref().and_then(|meta| meta.modified().ok()),
        metadata.map(|meta| meta.len()).unwrap_or(0),
    )
}

fn load_plugin(lua: &Lua, path: &Path) -> anyhow::Result<LoadedPlugin> {
    let source = std::fs::read_to_string(path)?;
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plugin".into());
    let table: Table = lua.load(&source).set_name(stem.as_str()).eval()?;
    let manifest = Manifest::from_lua(&table, &stem).map_err(anyhow::Error::msg)?;
    let key = lua.create_registry_value(table)?;
    Ok(LoadedPlugin {
        name: manifest.id.clone(),
        table: key,
        path: path.to_path_buf(),
        manifest,
        started: false,
    })
}

fn push(queue: &Arc<Mutex<Vec<QueuedAction>>>, plugin: &str, action: Action) -> mlua::Result<()> {
    let mut queue = queue
        .lock()
        .map_err(|_| mlua::Error::runtime("editor plugin action queue poisoned"))?;
    queue.push(QueuedAction {
        plugin: plugin.to_owned(),
        action,
    });
    Ok(())
}

#[cfg(test)]
struct SmokeEvent {
    kind: &'static str,
    panel: String,
    id: String,
    value: Option<String>,
    index: Option<usize>,
}

/// Synthetic `change`/`select` events for the smoke test. Deliberately skips
/// `click`/`add`/`remove`: a smoke run must not write content.
#[cfg(test)]
fn collect_events(node: &UiNode, panel: &str, out: &mut Vec<SmokeEvent>) {
    if out.len() >= 200 {
        return;
    }
    match node {
        UiNode::Column { children, .. }
        | UiNode::Row { children, .. }
        | UiNode::Scroll { children, .. } => {
            for child in children {
                collect_events(child, panel, out);
            }
        }
        UiNode::Text { id, value, .. } => out.push(SmokeEvent {
            kind: "change",
            panel: panel.to_owned(),
            id: id.clone(),
            value: Some(value.clone()),
            index: None,
        }),
        UiNode::Number { id, value, .. } => out.push(SmokeEvent {
            kind: "change",
            panel: panel.to_owned(),
            id: id.clone(),
            value: Some(value.to_string()),
            index: None,
        }),
        UiNode::Checkbox { id, value, .. } => out.push(SmokeEvent {
            kind: "change",
            panel: panel.to_owned(),
            id: id.clone(),
            value: Some(value.to_string()),
            index: None,
        }),
        UiNode::Select {
            id, value, options, ..
        } => {
            let value = if value.is_empty() {
                options.first().map(|(value, _)| value.clone())
            } else {
                Some(value.clone())
            };
            if let Some(value) = value {
                out.push(SmokeEvent {
                    kind: "change",
                    panel: panel.to_owned(),
                    id: id.clone(),
                    value: Some(value),
                    index: None,
                });
            }
        }
        UiNode::List {
            id,
            selected,
            items,
            ..
        } => {
            if let Some(value) = selected
                .clone()
                .or_else(|| items.first().map(|item| item.id.clone()))
            {
                out.push(SmokeEvent {
                    kind: "select",
                    panel: panel.to_owned(),
                    id: id.clone(),
                    value: Some(value),
                    index: None,
                });
            }
        }
        UiNode::Button { id, enabled, .. } if *enabled && id.as_str() != "save" => {
            out.push(SmokeEvent {
                kind: "click",
                panel: panel.to_owned(),
                id: id.clone(),
                value: None,
                index: None,
            });
        }
        UiNode::TextLines { id, values, .. } => {
            for (index, value) in values.iter().enumerate() {
                out.push(SmokeEvent {
                    kind: "change",
                    panel: panel.to_owned(),
                    id: id.clone(),
                    value: Some(value.clone()),
                    index: Some(index),
                });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(code: &str) -> mlua::Result<AttachmentSpec> {
        let lua = Lua::new();
        let table: Table = lua.load(code).eval()?;
        AttachmentSpec::from_lua(&table)
    }

    #[test]
    fn attachment_spec_parses_and_rejects_missing_fields() {
        let parsed = spec(
            r#"return { slot="preview", url="x.glb", bone="bone_a", x=0.1, rx=1.5, scale=0.4 }"#,
        )
        .unwrap();
        assert_eq!(parsed.bone, "bone_a");
        assert!((parsed.pos.x - 0.1).abs() < 1e-6);
        assert!((parsed.rot.x - 1.5).abs() < 1e-6);
        assert!((parsed.scale - 0.4).abs() < 1e-6);

        assert!(spec(r#"return { url="x.glb", bone="bone_a" }"#).is_err());
        assert!(spec(r#"return { slot="preview", bone="bone_a" }"#).is_err());
        assert!(spec(r#"return { slot="preview", url="x.glb" }"#).is_err());
        assert!(spec(r#"return { slot="preview", url="x.glb", bone="b", scale=0 }"#).is_err());
    }

    /// Smoke headless: `PLUGIN_SMOKE_DIR=<projeto>/editor/plugins cargo test
    /// plugins::tests::smoke_loads_env_project -- --nocapture`. Sem a variável
    /// o teste salta (o editor não conhece nenhum projeto em particular).
    #[test]
    fn smoke_loads_env_project() {
        let Some(dir) = std::env::var_os("PLUGIN_SMOKE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let root = dir
            .parent()
            .and_then(Path::parent)
            .expect("PLUGIN_SMOKE_DIR deve ser <projeto>/editor/plugins")
            .to_path_buf();
        let host = PluginHost::load(&root, &dir).unwrap();
        assert!(
            host.errors().is_empty(),
            "plugins com erro: {:?}",
            host.errors()
        );
        assert!(!host.panels().is_empty(), "nenhum painel carregado");
        assert!(!host.commands().is_empty(), "nenhum comando carregado");
        let mut host = host;
        let world = root.join("world.xml");
        if world.is_file() {
            let doc = SceneDoc::load(&world).unwrap();
            let findings = host.lint(&doc);
            eprintln!("plugin lint: {} findings", findings.len());
            for finding in &findings {
                eprintln!(
                    "  {:?} {}: {}",
                    finding.severity, finding.kind, finding.message
                );
            }
            // O lint dos plugins pode avisar (conteúdo por resolver); o que não
            // pode é rebentar. Um erro de runtime vira finding `plugin-lint`.
            assert!(
                !findings.iter().any(|finding| finding.kind == "plugin-lint"),
                "lint dos plugins falhou: {findings:?}"
            );
            let errors = host.smoke_render(&doc);
            assert!(errors.is_empty(), "render dos plugins falhou: {errors:?}");
        }
    }

    /// Smoke dos loaders de conteúdo (rules/landmarks) contra um projeto real.
    /// Usa o mesmo `PLUGIN_SMOKE_DIR` do teste acima.
    #[test]
    fn smoke_rules_and_landmarks_from_env() {
        let Some(dir) = std::env::var_os("PLUGIN_SMOKE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let root = dir
            .parent()
            .and_then(Path::parent)
            .expect("PLUGIN_SMOKE_DIR deve ser <projeto>/editor/plugins")
            .to_path_buf();
        let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).unwrap();
        let viber = lua.create_table().unwrap();
        let project = root.clone();
        viber
            .set(
                "load_json",
                lua.create_function(move |lua, path: String| {
                    let text = std::fs::read_to_string(project.join(&path))
                        .map_err(mlua::Error::external)?;
                    let value: serde_json::Value =
                        serde_json::from_str(&text).map_err(mlua::Error::external)?;
                    json::json_to_lua(lua, &value).map_err(mlua::Error::runtime)
                })
                .unwrap(),
            )
            .unwrap();
        let project = root.clone();
        viber
            .set(
                "load",
                lua.create_function(move |lua, path: String| {
                    let source = std::fs::read_to_string(project.join("scripts").join(path))
                        .map_err(mlua::Error::external)?;
                    lua.load(&source).eval::<mlua::Value>()
                })
                .unwrap(),
            )
            .unwrap();
        viber
            .set(
                "game",
                lua.create_function(|lua, ()| lua.globals().get::<Table>("__game"))
                    .unwrap(),
            )
            .unwrap();
        viber
            .set(
                "log",
                lua.create_function(|_, message: String| {
                    eprintln!("lua log: {message}");
                    Ok(())
                })
                .unwrap(),
            )
            .unwrap();
        viber
            .set(
                "own_action",
                lua.create_function(|_, _: String| Ok(())).unwrap(),
            )
            .unwrap();
        viber
            .set(
                "sound",
                lua.create_function(|_, _: mlua::Value| Ok(())).unwrap(),
            )
            .unwrap();
        viber
            .set(
                "toast",
                lua.create_function(|_, _: mlua::Value| Ok(())).unwrap(),
            )
            .unwrap();
        viber
            .set(
                "player_position",
                lua.create_function(|_, ()| Ok((false, 0.0, 0.0, 0.0)))
                    .unwrap(),
            )
            .unwrap();
        viber
            .set(
                "events",
                lua.create_function(|lua, ()| lua.create_table()).unwrap(),
            )
            .unwrap();
        let ui = lua.create_table().unwrap();
        ui.set(
            "list",
            lua.create_function(|_, (_name, _rows): (String, Table)| Ok(()))
                .unwrap(),
        )
        .unwrap();
        viber.set("ui", ui).unwrap();
        lua.globals().set("viber", viber).unwrap();
        lua.globals()
            .set("__game", lua.create_table().unwrap())
            .unwrap();

        let source = std::fs::read_to_string(root.join("scripts/lib/rules.lua")).unwrap();
        let rules: Table = lua.load(&source).set_name("lib/rules.lua").eval().unwrap();
        let items: Table = rules.get::<Function>("items").unwrap().call(()).unwrap();
        assert_eq!(items.raw_len(), 19, "data/items.json");
        let quests: Table = rules
            .get::<Function>("quest_defs")
            .unwrap()
            .call(())
            .unwrap();
        assert_eq!(quests.raw_len(), 28, "quests/index.json + ficheiros");
        let skills: Table = rules.get::<Function>("skills").unwrap().call(()).unwrap();
        assert_eq!(skills.raw_len(), 8, "data/progression.json");
        let boss: i64 = rules
            .get::<Function>("kill_xp")
            .unwrap()
            .call("boss")
            .unwrap();
        assert_eq!(boss, 300);
        let potion: Table = rules
            .get::<Function>("item_def")
            .unwrap()
            .call("potion")
            .unwrap();
        assert_eq!(potion.get::<String>("label").unwrap(), "Poção");
        let vitality: Table = rules
            .get::<Function>("skill")
            .unwrap()
            .call("vitality1")
            .unwrap();
        assert_eq!(
            vitality
                .get::<Table>("effects")
                .unwrap()
                .get::<i64>("max_hp")
                .unwrap(),
            20
        );

        let source = std::fs::read_to_string(root.join("scripts/lib/landmarks.lua")).unwrap();
        let landmarks: Table = lua
            .load(&source)
            .set_name("lib/landmarks.lua")
            .eval()
            .unwrap();
        assert_eq!(landmarks.get::<i64>("CAMPFIRE_RANGE").unwrap(), 14);
        assert_eq!(landmarks.get::<Table>("LIST").unwrap().raw_len(), 22);
        let label: String = landmarks
            .get::<Function>("label")
            .unwrap()
            .call("forest-outpost-tower")
            .unwrap();
        assert_eq!(label, "Torre do Posto Avançado");

        // Loja e diário são os consumidores do registo de itens/talentos.
        let source = std::fs::read_to_string(root.join("scripts/game/shop.lua")).unwrap();
        let shop: Table = lua.load(&source).set_name("game/shop.lua").eval().unwrap();
        shop.get::<Function>("update")
            .unwrap()
            .call::<()>(0.0)
            .unwrap();

        let source = std::fs::read_to_string(root.join("scripts/ui/journal.lua")).unwrap();
        let journal: Table = lua.load(&source).set_name("ui/journal.lua").eval().unwrap();
        journal
            .get::<Function>("refresh")
            .unwrap()
            .call::<()>(())
            .unwrap();
    }
}
