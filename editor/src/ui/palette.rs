//! Ctrl+K: a fuzzy command palette over the editor's actions and open scene
//! tabs. Everything it runs is the same path the menus/shortcuts use.

use bevy::feathers::controls::{FeathersTextInput, FeathersTextInputContainer};
use bevy::feathers::display::caption;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::events::PointerClick;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;

use super::icons::{self, fa, icon};
use super::layout::{ActiveDockTab, DockSide, DockVisibility, TabKind};
use super::scene_tabs::{OpenSceneRequest, SceneTabs};
use super::strings::{Lang, StrKey, t};
use super::theme;
use super::view_options::{CameraView, ViewOptions};
use crate::bridge::{EngineStatus, ViberBridge};

/// Open/closed state of the palette.
#[derive(Resource, Debug, Default)]
pub struct PaletteOpen(pub bool);

/// The query and the highlighted row.
#[derive(Resource, Debug, Default)]
pub struct PaletteState {
    pub query: String,
    pub selected: usize,
}

/// The modal overlay.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PalettePanel;

/// The scrolling results.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PaletteBody;

/// The filter input (the widget that owns the text).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PaletteInput;

/// One clickable result row.
#[derive(Component, Debug, Clone)]
pub struct PaletteRow(pub Action);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Save,
    Reload,
    Undo,
    Redo,
    Duplicate,
    Delete,
    Copy,
    Paste,
    Rename,
    FrameSelection,
    FrameAll,
    ToggleGrid,
    ToggleColliders,
    ToggleOrtho,
    View(CameraView),
    AddNode,
    Play,
    PlayScene,
    PlayHere,
    Pause,
    Stop,
    BottomTab(TabKind),
    TogglePanel(DockSide),
    CheatSheet,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Command(Command),
    OpenTab { index: usize, title: String },
    PluginCommand(String),
}

struct Entry {
    action: Action,
    label: String,
    /// Both languages, so the palette matches "guardar" and "save".
    search: String,
}

pub struct PalettePlugin;

impl Plugin for PalettePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PaletteOpen>()
            .init_resource::<PaletteState>()
            .add_systems(
                Update,
                (
                    toggle_palette,
                    focus_palette_input,
                    apply_palette_input,
                    render_palette,
                )
                    .chain(),
            );
    }
}

fn entry(lang: Lang, action: Action, pt: &str, en: &str) -> Entry {
    Entry {
        action,
        label: if matches!(lang, Lang::En) { en } else { pt }.to_owned(),
        search: format!("{pt} {en}").to_ascii_lowercase(),
    }
}

fn tab_entry(lang: Lang, kind: TabKind) -> (&'static str, &'static str) {
    match kind {
        TabKind::Console => (t(lang, StrKey::TabConsole), "console"),
        TabKind::Logs => (t(lang, StrKey::TabLogs), "logs"),
        TabKind::Profiler => (t(lang, StrKey::TabProfiler), "profiler"),
        TabKind::WorldDoc => (t(lang, StrKey::TabWorldDoc), "world"),
        TabKind::Files => (t(lang, StrKey::TabFiles), "files"),
        TabKind::History => (t(lang, StrKey::TabHistory), "history"),
        TabKind::Game => (t(lang, StrKey::TabGame), "game"),
        TabKind::Plugins => (t(lang, StrKey::TabPlugins), "plugins"),
        TabKind::Problems => (t(lang, StrKey::TabProblems), "problems"),
        TabKind::Search => (t(lang, StrKey::TabSearch), "search"),
    }
}

fn entries(lang: Lang, tabs: &SceneTabs, commands: &[crate::plugins::CommandInfo]) -> Vec<Entry> {
    let en = matches!(lang, Lang::En);
    let mut out = Vec::new();
    macro_rules! push {
        ($command:expr, $pt:expr, $en_label:expr) => {
            out.push(entry(lang, Action::Command($command), $pt, $en_label));
        };
    }
    push!(Command::Save, "Guardar", "Save");
    push!(Command::Reload, "Recarregar do disco", "Reload from disk");
    push!(Command::Undo, "Desfazer", "Undo");
    push!(Command::Redo, "Refazer", "Redo");
    push!(Command::Duplicate, "Duplicar", "Duplicate");
    push!(Command::Delete, "Apagar", "Delete");
    push!(Command::Copy, "Copiar", "Copy");
    push!(Command::Paste, "Colar", "Paste");
    push!(Command::Rename, "Renomear", "Rename");
    push!(
        Command::FrameSelection,
        "Enquadrar a seleção",
        "Frame selection"
    );
    push!(Command::FrameAll, "Enquadrar tudo", "Frame everything");
    push!(Command::ToggleGrid, "Grelha", "Grid");
    push!(Command::ToggleColliders, "Colliders", "Colliders");
    push!(
        Command::ToggleOrtho,
        "Vista ortográfica",
        "Orthographic view"
    );
    push!(
        Command::View(CameraView::Perspective),
        "Vista perspetiva",
        "Perspective view"
    );
    push!(
        Command::View(CameraView::Front),
        "Vista de frente",
        "Front view"
    );
    push!(Command::View(CameraView::Top), "Vista de topo", "Top view");
    push!(
        Command::View(CameraView::Right),
        "Vista da direita",
        "Right view"
    );
    push!(Command::AddNode, "Adicionar nó", "Add node");
    push!(Command::Play, "Jogar", "Play");
    push!(
        Command::PlayScene,
        "Jogar a cena ativa",
        "Play the active scene"
    );
    push!(Command::PlayHere, "Jogar daqui", "Play from here");
    push!(Command::Pause, "Pausar / retomar", "Pause / resume");
    push!(Command::Stop, "Parar", "Stop");
    for kind in [
        TabKind::Console,
        TabKind::Logs,
        TabKind::Profiler,
        TabKind::WorldDoc,
        TabKind::Files,
        TabKind::History,
        TabKind::Game,
        TabKind::Plugins,
        TabKind::Problems,
        TabKind::Search,
    ] {
        let (label, key) = tab_entry(lang, kind);
        out.push(entry(
            lang,
            Action::Command(Command::BottomTab(kind)),
            &format!("Painel: {label}"),
            &format!("Panel: {label}"),
        ));
        let _ = key;
    }
    push!(
        Command::TogglePanel(DockSide::Left),
        "Painel Objetos",
        "Objects panel"
    );
    push!(
        Command::TogglePanel(DockSide::Right),
        "Painel Inspetor",
        "Inspector panel"
    );
    push!(
        Command::TogglePanel(DockSide::Bottom),
        "Dock inferior",
        "Bottom dock"
    );
    push!(Command::CheatSheet, "Atalhos", "Shortcuts");
    for (index, tab) in tabs.tabs.iter().enumerate() {
        out.push(Entry {
            action: Action::OpenTab {
                index,
                title: tab.title.clone(),
            },
            label: format!("{} {}", if en { "Tab:" } else { "Aba:" }, tab.title),
            search: tab.title.to_ascii_lowercase(),
        });
    }
    for command in commands {
        out.push(Entry {
            action: Action::PluginCommand(command.key()),
            label: format!("{} — {}", command.title, command.plugin),
            search: format!(
                "{} {} {} {}",
                command.title, command.keywords, command.id, command.plugin
            )
            .to_ascii_lowercase(),
        });
    }
    out
}

/// Case-insensitive subsequence match (`frm` matches `Frame selection`).
pub fn matches(query: &str, text: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    let mut chars = lower.chars();
    query
        .chars()
        .all(|wanted| chars.any(|candidate| candidate == wanted))
}

/// Substring hits first, then subsequence hits, stable otherwise.
fn filtered<'a>(query: &str, entries: &'a [Entry]) -> Vec<&'a Entry> {
    let needle = query.trim().to_ascii_lowercase();
    let mut hits: Vec<(usize, usize, &Entry)> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches(&needle, &entry.search))
        .map(|(index, entry)| {
            let position = if needle.is_empty() {
                0
            } else {
                entry.search.find(&needle).unwrap_or(usize::MAX)
            };
            (position, index, entry)
        })
        .collect();
    hits.sort_by_key(|(position, index, _)| (*position, *index));
    hits.into_iter().map(|(_, _, entry)| entry).collect()
}

/// Wraps the highlighted row.
pub fn next_selection(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let current = current.min(len - 1) as isize;
    (current + delta).rem_euclid(len as isize) as usize
}

fn toggle_palette(
    keys: Res<ButtonInput<KeyCode>>,
    focus: Option<Res<InputFocus>>,
    editable: Query<(), With<bevy::text::EditableText>>,
    mut open: ResMut<PaletteOpen>,
    mut state: ResMut<PaletteState>,
) {
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if ctrl && keys.just_pressed(KeyCode::KeyK) {
        open.0 = !open.0;
        if open.0 {
            state.query.clear();
            state.selected = 0;
        }
        return;
    }
    let typing = focus
        .as_ref()
        .and_then(|focus| focus.get())
        .is_some_and(|entity| editable.contains(entity));
    if open.0 && keys.just_pressed(KeyCode::Escape) && !typing {
        open.0 = false;
    }
}

/// Focuses the input on open and clears it when the palette closes.
fn focus_palette_input(open: Res<PaletteOpen>, mut was_open: Local<bool>, mut commands: Commands) {
    if open.0 && !*was_open {
        commands.queue(|world: &mut World| {
            let Some(entity) = world
                .query_filtered::<Entity, With<PaletteInput>>()
                .iter(world)
                .next()
            else {
                return;
            };
            if let Some(mut text) = world.get_mut::<bevy::text::EditableText>(entity) {
                text.editor_mut().set_text("");
            }
            if let Some(mut focus) = world.get_resource_mut::<InputFocus>() {
                focus.set(entity, FocusCause::Navigated);
            }
        });
    } else if !open.0 && *was_open {
        commands.queue(|world: &mut World| {
            let focused = world
                .get_resource::<InputFocus>()
                .and_then(|focus| focus.get());
            let is_palette =
                focused.is_some_and(|entity| world.get::<PaletteInput>(entity).is_some());
            if is_palette && let Some(mut focus) = world.get_resource_mut::<InputFocus>() {
                focus.clear();
            }
        });
    }
    *was_open = open.0;
}

/// Updates the query from the focused input and handles ↑/↓/Enter/Esc.
fn apply_palette_input(world: &mut World) {
    let open = world.resource::<PaletteOpen>().0;
    if !open {
        return;
    }
    let Some(focused) = world
        .get_resource::<InputFocus>()
        .and_then(|focus| focus.get())
    else {
        return;
    };
    if world.get::<PaletteInput>(focused).is_none() {
        return;
    }
    let text = crate::scene::input_text(world, focused).unwrap_or_default();
    let keys = world.resource::<ButtonInput<KeyCode>>();
    let (enter, escape, up, down) = (
        keys.just_pressed(KeyCode::Enter),
        keys.just_pressed(KeyCode::Escape),
        keys.just_pressed(KeyCode::ArrowUp),
        keys.just_pressed(KeyCode::ArrowDown),
    );
    if escape {
        world.resource_mut::<PaletteOpen>().0 = false;
        return;
    }
    let lang = *world.resource::<Lang>();
    let commands = world
        .get_resource::<crate::plugins::PluginHost>()
        .map(|host| host.commands())
        .unwrap_or_default();
    let entries = entries(lang, &world.resource::<SceneTabs>().clone(), &commands);
    let query = text.trim().to_owned();
    let hits = filtered(&query, &entries);
    let mut state = world.resource_mut::<PaletteState>();
    if state.query != query {
        state.query = query;
        state.selected = 0;
    }
    if down {
        state.selected = next_selection(state.selected, hits.len(), 1);
    } else if up {
        state.selected = next_selection(state.selected, hits.len(), -1);
    }
    let action = if enter {
        hits.get(state.selected).map(|entry| entry.action.clone())
    } else {
        None
    };
    let selected = state.selected;
    if let Some(action) = action {
        world.resource_mut::<PaletteOpen>().0 = false;
        execute(world, action);
    } else {
        // Keep the highlight valid when the query narrowed the list.
        let mut state = world.resource_mut::<PaletteState>();
        state.selected = selected.min(hits.len().saturating_sub(1));
    }
}

/// Runs a palette action through the same functions the shortcuts use.
fn execute(world: &mut World, action: Action) {
    match action {
        Action::OpenTab { index, .. } => {
            // A aba ativa já está no viewport: pedi-la de novo fazia um save +
            // troca/reload da MESMA cena (o caminho leva o `scene_loader` a
            // recarregar). Escolher a aba atual é um no-op.
            let path = {
                let tabs = world.resource::<SceneTabs>();
                if index == tabs.active {
                    return;
                }
                tabs.tabs.get(index).map(|tab| tab.path.clone())
            };
            if let Some(path) = path {
                world.resource_mut::<OpenSceneRequest>().0 = Some(path);
            }
        }
        Action::Command(command) => match command {
            Command::Save => crate::scene::save_world(world),
            Command::Reload => crate::app::reload_world(world),
            Command::Undo => crate::scene::undo_last(world),
            Command::Redo => crate::scene::redo_last(world),
            Command::Duplicate => crate::scene::duplicate_selection(world),
            Command::Delete => crate::scene::delete_selection(world),
            Command::Copy => crate::scene::copy_selection(world),
            Command::Paste => crate::scene::paste_clipboard(world),
            Command::Rename => crate::scene::begin_rename(world),
            Command::FrameSelection => {
                let nodes =
                    crate::scene::selection_roots(&world.resource::<crate::scene::Selection>().0);
                crate::viewport::gizmo::frame_nodes(world, &nodes);
            }
            Command::FrameAll => {
                crate::viewport::gizmo::frame_all(world);
            }
            Command::ToggleGrid => {
                world.resource_mut::<ViewOptions>().grid ^= true;
            }
            Command::ToggleColliders => {
                let value = {
                    let mut options = world.resource_mut::<ViewOptions>();
                    options.colliders = !options.colliders;
                    options.colliders
                };
                if matches!(
                    world.resource::<ViberBridge>().status,
                    EngineStatus::Connected { .. }
                ) {
                    let bridge = world.resource::<ViberBridge>();
                    crate::bridge::methods::colliders_toggle(&bridge.client, value).detach();
                }
            }
            Command::ToggleOrtho => {
                world.resource_mut::<ViewOptions>().ortho ^= true;
            }
            Command::View(view) => {
                world.insert_resource(view);
                if let Some(mut camera) =
                    world.get_resource_mut::<viber::editor_camera::EditorCamState>()
                {
                    super::view_options::set_camera_view(view, &mut camera);
                }
            }
            Command::AddNode => {
                world
                    .resource_mut::<crate::scene::add_node::AddNodeOpen>()
                    .0 = true;
            }
            Command::Play => super::toolbar::play(world),
            Command::PlayScene => super::toolbar::play_scene(world),
            Command::PlayHere => super::toolbar::play_from_here(world),
            Command::Pause => super::toolbar::toggle_pause(world),
            Command::Stop => super::toolbar::stop(world),
            Command::BottomTab(kind) => {
                world.resource_mut::<ActiveDockTab>().0 = kind;
            }
            Command::TogglePanel(side) => {
                world.resource_mut::<DockVisibility>().toggle(side);
            }
            Command::CheatSheet => {
                let mut open = world.resource_mut::<super::cheatsheet::CheatSheetOpen>();
                open.0 = !open.0;
            }
        },
        Action::PluginCommand(key) => crate::plugins::invoke_command(world, &key),
    }
}

#[allow(clippy::too_many_arguments)] // Bevy system: one parameter per resource/query.
fn render_palette(
    mut commands: Commands,
    open: Res<PaletteOpen>,
    state: Res<PaletteState>,
    lang: Res<Lang>,
    tabs: Res<SceneTabs>,
    host: Option<Res<crate::plugins::PluginHost>>,
    mut panels: Query<&mut Node, With<PalettePanel>>,
    bodies: Query<Entity, With<PaletteBody>>,
    mut built: Local<bool>,
) {
    let Ok(mut panel) = panels.single_mut() else {
        return;
    };
    let display = if open.0 { Display::Flex } else { Display::None };
    if panel.display != display {
        panel.display = display;
    }
    if !open.0 {
        *built = false;
        return;
    }
    if *built && !state.is_changed() && !lang.is_changed() && !tabs.is_changed() {
        return;
    }
    *built = true;
    let Ok(body) = bodies.single() else {
        return;
    };
    commands.entity(body).despawn_related::<Children>();
    let plugin_commands = host.map(|host| host.commands()).unwrap_or_default();
    let entries = entries(*lang, &tabs, &plugin_commands);
    let hits = filtered(&state.query, &entries);
    if hits.is_empty() {
        commands.spawn((
            Text::new(if matches!(*lang, Lang::En) {
                "No matching command"
            } else {
                "Sem comando correspondente"
            }),
            TextFont::from_font_size(12.0),
            TextColor(icons::ICON_DIM),
            Pickable::IGNORE,
            ChildOf(body),
        ));
        return;
    }
    for (index, entry) in hits.iter().enumerate() {
        let selected = index == state.selected.min(hits.len() - 1);
        let mut row = commands.spawn((
            PaletteRow(entry.action.clone()),
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding: UiRect::axes(px(8), px(5)),
                border_radius: BorderRadius::all(px(4)),
                ..default()
            },
            BackgroundColor(if selected { theme::ACCENT } else { Color::NONE }),
            ChildOf(body),
        ));
        row.observe(palette_row_clicked);
        row.with_children(|row| {
            row.spawn((
                Text::new(entry.label.clone()),
                TextFont::from_font_size(12.0),
                TextColor(if selected { Color::WHITE } else { theme::TEXT }),
                Pickable::IGNORE,
            ));
        });
    }
}

fn palette_row_clicked(
    click: On<PointerClick>,
    rows: Query<&PaletteRow>,
    mut open: ResMut<PaletteOpen>,
    mut commands: Commands,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(row) = rows.get(click.event_target()) else {
        return;
    };
    let action = row.0.clone();
    open.0 = false;
    commands.queue(move |world: &mut World| execute(world, action));
}

fn close_palette_on_backdrop(
    click: On<PointerClick>,
    panels: Query<(), With<PalettePanel>>,
    mut open: ResMut<PaletteOpen>,
) {
    if panels.contains(click.original_event_target()) {
        open.0 = false;
    }
}

/// The modal, spawned in the editor root.
pub fn palette_panel() -> impl Scene {
    bsn! {
        PalettePanel
        GlobalZIndex(66)
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            align_items: AlignItems::FlexStart,
            justify_content: JustifyContent::Center,
            padding: UiRect::top(px(96)),
            display: Display::None,
        }
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.28))
        on(close_palette_on_backdrop)
        Children [
            Node {
                width: px(540),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                padding: px(10),
                border: UiRect::all(px(1)),
                border_radius: px(8),
            }
            BackgroundColor(theme::BG_POPOVER)
            BorderColor::all(theme::BORDER_STRONG)
            Children [
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    flex_shrink: 0.0,
                }
                Children [
                    @icon(fa::MAGNIFYING_GLASS, 12.0, icons::ICON_DIM)
                    --
                    @FeathersTextInputContainer
                    Node { flex_grow: 1.0, min_width: px(0) }
                    Children [
                        @FeathersTextInput {
                            @visible_width: 24f32,
                        }
                        PaletteInput
                    ]
                ]
                --
                Node {
                    max_height: px(360),
                    overflow: Overflow::scroll_y(),
                    flex_direction: FlexDirection::Column,
                }
                PaletteBody
                Children []
                --
                @caption("↑↓  Enter  Esc") Pickable::IGNORE
                TextColor(icons::ICON_DIM)
            ]
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(lang: Lang) -> Vec<Entry> {
        entries(lang, &SceneTabs::default(), &[])
    }

    #[test]
    fn fuzzy_matching_accepts_subsequences_in_both_languages() {
        assert!(matches("save", "guardar save"));
        assert!(matches("grd", "guardar save"));
        assert!(matches("DUPL", "duplicar duplicate"));
        assert!(!matches("xyz", "guardar save"));
        assert!(matches("", "anything"));
    }

    #[test]
    fn substring_hits_rank_before_subsequence_hits() {
        let entries = sample(Lang::Pt);
        let hits = filtered("guardar", &entries);
        assert_eq!(hits[0].label, "Guardar");
        assert!(!hits.is_empty());
    }

    #[test]
    fn selection_wraps_and_clamps() {
        assert_eq!(next_selection(0, 3, -1), 2);
        assert_eq!(next_selection(2, 3, 1), 0);
        assert_eq!(next_selection(99, 3, 1), 0);
        assert_eq!(next_selection(0, 0, 1), 0);
    }

    #[test]
    fn palette_lists_scene_tabs() {
        let mut tabs = SceneTabs::default();
        tabs.tabs.push(super::super::scene_tabs::SceneTab {
            path: std::path::PathBuf::from("/x/world.xml"),
            title: "Vila".into(),
            camera: None,
        });
        let entries = entries(Lang::En, &tabs, &[]);
        assert!(
            entries
                .iter()
                .any(|entry| matches!(entry.action, Action::OpenTab { .. })
                    && entry.label.contains("Vila"))
        );
    }
}
