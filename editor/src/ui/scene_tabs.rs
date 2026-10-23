//! Scene tabs (Godot-style): one tab per open scene of the project, the active one is the
//! scene loaded in the viewport. Switching tabs saves the current scene and swaps the world
//! IN-PROCESS through the engine's scene loader (`viber::scene_loader` — the same unload/load
//! the game uses for `change_scene`), so a scene is its own universe here too: the interior
//! of a house is edited alone, not as a remote corner of the overworld.
//!
//! Agent parity: `viber.scene.change` over the bridge swaps the editor's scene as well — the
//! editor listens to [`SceneLoaded`] and opens/activates the matching tab.

use std::path::{Path, PathBuf};

use bevy::picking::events::PointerClick;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use viber::project::{Project, SceneTemplate};
use viber::scene_loader::{Arrival, ChangeScene, CurrentScene, SceneLoaded};

use super::icons::{self, fa};
use super::strings::{Lang, StrKey, save_failed, t};
use super::theme;
use super::tooltip::Tooltip;
use crate::scene::doc::SceneDoc;
use crate::scene::{EditStatus, Selection, TreeExpansion};

/// One open scene.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneTab {
    pub path: PathBuf,
    pub title: String,
    /// The editor camera when this tab was last left (restored on return).
    pub camera: Option<TabCamera>,
}

/// The orbit pose of the editor camera, per tab.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabCamera {
    pub target: Vec3,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub distance: f32,
}

/// The open scenes and the active one (the scene in the viewport).
#[derive(Resource, Debug, Default, Clone)]
pub struct SceneTabs {
    pub tabs: Vec<SceneTab>,
    pub active: usize,
    /// A cena REALMENTE carregada no viewport. `active` não serve: fechar a aba
    /// ativa desloca-o para a vizinha antes de a troca de cena chegar, e a
    /// comparação `tabs[active].path == evento` dizia "mesma cena" — a seleção
    /// e a expansão do mundo antigo sobreviviam para o novo documento.
    loaded: Option<PathBuf>,
    /// Re-render request (titles, dirty marker, language).
    rev: u64,
    rendered_rev: Option<u64>,
    /// Marcador "por gravar" de cada aba (o ficheiro da aba está sujo).
    rendered_dirty: Vec<bool>,
}

impl SceneTabs {
    fn touch(&mut self) {
        self.rev += 1;
    }

    /// Opens `path` (or activates its tab); returns the tab index.
    pub fn open(&mut self, path: &Path, title: String) -> usize {
        let path = canonical(path);
        let index = match self.tabs.iter().position(|tab| tab.path == path) {
            Some(index) => {
                if self.tabs[index].title != title {
                    self.tabs[index].title = title;
                }
                index
            }
            None => {
                self.tabs.push(SceneTab {
                    path,
                    title,
                    camera: None,
                });
                self.tabs.len() - 1
            }
        };
        self.active = index;
        self.touch();
        index
    }

    /// Closes a tab; the active tab cannot be closed while it is the only one.
    pub fn close(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return false;
        }
        self.tabs.remove(index);
        if self.active > index || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.touch();
        true
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The tab title of a scene: its `<world title>`, else the scene id, else the file stem.
fn scene_title(path: &Path) -> String {
    let project = Project::discover(path);
    project
        .scene_info(path)
        .and_then(|info| info.title.or(Some(info.id)))
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

/// The tab bar container.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SceneTabBar;

/// A tab button (its index).
#[derive(Component, Debug, Clone, Copy)]
struct TabButton(usize);

/// A tab's close button (its index).
#[derive(Component, Debug, Clone, Copy)]
struct TabClose(usize);

/// The "+" (new scene) button.
#[derive(Component, Debug, Clone, Copy)]
struct NewSceneButton;

/// Asks to open a scene file in a tab (Files dock, context menu). Processed by
/// [`process_open_requests`] (needs the whole `World`).
#[derive(Resource, Debug, Default)]
pub struct OpenSceneRequest(pub Option<PathBuf>);

pub struct SceneTabsPlugin;

impl Plugin for SceneTabsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneTabs>()
            .init_resource::<OpenSceneRequest>()
            .add_systems(
                Update,
                (
                    seed_first_tab,
                    on_scene_loaded,
                    render_tabs,
                    process_open_requests,
                )
                    .chain(),
            );
    }
}

/// The bar itself (empty; [`render_tabs`] fills it). Height matches the dock tab bars.
pub fn scene_tab_bar() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            height: px(28),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::End,
            column_gap: px(2),
            padding: UiRect::horizontal(px(4)),
            overflow: Overflow::clip(),
        }
        SceneTabBar
    }
}

/// First frame with a document: the world the editor opened becomes the first tab.
fn seed_first_tab(
    doc: Option<Res<SceneDoc>>,
    current: Option<Res<CurrentScene>>,
    prefs: Option<Res<crate::prefs::EditorPrefs>>,
    mut camera: Option<ResMut<viber::editor_camera::EditorCamState>>,
    mut tabs: ResMut<SceneTabs>,
) {
    if !tabs.tabs.is_empty() {
        return;
    }
    let Some(doc) = doc else {
        return;
    };
    // Restore the tabs of the last session (the scene files must still exist);
    // the world being opened becomes the active tab.
    if let Some(prefs) = prefs.as_deref() {
        for saved in &prefs.tabs {
            let path = PathBuf::from(&saved.path);
            if !path.is_file() {
                continue;
            }
            let path = canonical(&path);
            if tabs.tabs.iter().any(|tab| tab.path == path) {
                continue;
            }
            tabs.tabs.push(SceneTab {
                path: path.clone(),
                title: scene_title(&path),
                camera: saved.camera.map(Into::into),
            });
        }
    }
    let path = doc.world_path().to_path_buf();
    let title = current
        .and_then(|c| c.title.clone())
        .unwrap_or_else(|| scene_title(&path));
    tabs.open(&path, title);
    tabs.loaded = tabs.tabs.get(tabs.active).map(|tab| tab.path.clone());
    // The first scene never goes through `on_scene_loaded`'s "leaving" branch,
    // so restore its camera here.
    let active_path = tabs.tabs[tabs.active].path.clone();
    if let Some(camera) = camera.as_deref_mut()
        && let Some(pose) = prefs
            .as_deref()
            .and_then(|prefs| prefs.camera_for(&active_path))
    {
        camera.target = pose.target;
        camera.yaw_deg = pose.yaw_deg;
        camera.pitch_deg = pose.pitch_deg;
        camera.distance = pose.distance;
    }
}

/// A scene finished loading (tab switch, `viber.scene.change` from an agent, Ctrl+R):
/// activate its tab and drop every piece of editor state that pointed into the old scene.
#[allow(clippy::too_many_arguments)]
fn on_scene_loaded(
    mut loaded: MessageReader<SceneLoaded>,
    mut tabs: ResMut<SceneTabs>,
    mut commands: Commands,
    mut config: ResMut<crate::app::EditorConfig>,
    mut status: ResMut<EditStatus>,
    camera: Option<ResMut<viber::editor_camera::EditorCamState>>,
    terrain: Option<Res<viber::terrain::runtime::TerrainRuntime>>,
    lang: Res<Lang>,
    mut prefs: Option<ResMut<crate::prefs::EditorPrefs>>,
) {
    let Some(event) = loaded.read().last().cloned() else {
        return;
    };
    // A câmara (que a troca não mexe) ainda é a da cena que se deixa: fica
    // guardada nessa cena, venha a troca de uma aba, do dock Ficheiros ou de um
    // agente (`viber.scene.change`). O caminho CARREGADO é a referência — a aba
    // ativa pode já ser outra (aba ativa fechada) ou nem existir.
    let event_path = canonical(&event.path);
    let same_scene = tabs.loaded.as_deref() == Some(event_path.as_path());
    if !same_scene
        && let Some(loaded) = tabs.loaded.clone()
        && let Some(cam) = camera.as_deref()
    {
        let pose = TabCamera {
            target: cam.target,
            yaw_deg: cam.yaw_deg,
            pitch_deg: cam.pitch_deg,
            distance: cam.distance,
        };
        if let Some(tab) = tabs.tabs.iter_mut().find(|tab| tab.path == loaded) {
            tab.camera = Some(pose);
        }
        if let Some(prefs) = prefs.as_mut() {
            prefs.remember_camera(&loaded, pose);
        }
    }
    let title = scene_title(&event_path);
    let index = tabs.open(&event_path, title.clone());
    tabs.loaded = Some(event_path);
    // Reload da cena actual (Ctrl+R): a câmara ainda é esta. A aba só guarda a pose
    // ao SAIR, por isso `tab.camera` é None e o ramo "cena nunca vista" saltava para
    // a origem e largava a seleção.
    if !same_scene && let Some(mut camera) = camera {
        match tabs.tabs[index].camera {
            Some(pose) => {
                camera.target = pose.target;
                camera.yaw_deg = pose.yaw_deg;
                camera.pitch_deg = pose.pitch_deg;
                camera.distance = pose.distance;
            }
            None => {
                if let Some(pose) = prefs
                    .as_deref()
                    .and_then(|prefs| prefs.camera_for(&event.path))
                {
                    camera.target = pose.target;
                    camera.yaw_deg = pose.yaw_deg;
                    camera.pitch_deg = pose.pitch_deg;
                    camera.distance = pose.distance;
                } else {
                    let ground = terrain
                        .as_deref()
                        .map(|t| t.sample(0.0, 0.0))
                        .unwrap_or(0.0);
                    camera.target = Vec3::new(0.0, ground, 0.0);
                    camera.distance = 32.0;
                }
            }
        }
    }
    // Everything below addressed NODES or ENTITIES of the old scene.
    // Na mesma cena os NodeRef continuam válidos (o XML não mudou de identidade).
    if !same_scene {
        commands.insert_resource(Selection::default());
        commands.insert_resource(TreeExpansion::default());
        commands.insert_resource(crate::scene::tree::TreeView::default());
    }
    commands.insert_resource(crate::scene::PendingEdits::default());
    commands.insert_resource(crate::scene::tree_drag::TreeDrag::default());
    commands.insert_resource(crate::scene::tree::RenameState::default());
    commands.insert_resource(crate::scene::inspector::InspectorView::default());
    commands.insert_resource(crate::scene::add_node::AddNodeOpen::default());
    commands.insert_resource(crate::viewport::gizmo::SelectedEntity::default());
    // Drags/menus/tasks do mundo antigo não podem sobreviver ao swap: os
    // ids das entidades são reciclados e o commit escrevia no nó errado.
    commands.insert_resource(crate::viewport::gizmo::GizmoInput::default());
    commands.insert_resource(crate::viewport::rubber::RubberBand::default());
    // Os marcadores de "arrasto em curso" têm de SAIR (existem só durante um
    // drag). `insert_resource` deixava-os presentes e o gizmo/rubber ficavam
    // desligados: o primeiro clique no viewport era engolido e, com uma
    // feature selecionada em modo Select, o pick morria de vez.
    commands.remove_resource::<crate::viewport::rubber::RubberGrab>();
    commands.remove_resource::<crate::viewport::feature_handles::FeatureGrab>();
    commands.insert_resource(crate::ui::context_menu::ContextMenu::default());
    commands.insert_resource(super::problems::ProblemsTask::default());
    commands.insert_resource(crate::viewport::feature_handles::FeaturePoints::default());
    commands.insert_resource(crate::viewport::feature_handles::FeatureDrag::default());
    commands.insert_resource(crate::viewport::terrain_tool::Stroke::default());
    commands.insert_resource(crate::viewport::stamp_tool::StampStroke::default());
    // O alvo do pincel é um SELECTOR de um <Scatter> da cena antiga: no mundo
    // novo apontaria para outro nó (ou para nenhum) e cada dab falhava.
    commands.insert_resource(crate::viewport::brush_panel::BrushTarget::default());
    commands.insert_resource(super::problems::ProblemsState::default());
    commands.insert_resource(super::history::HistoryState::default());
    commands.insert_resource(super::biomes::BiomesState::default());
    commands.insert_resource(super::canvas::CanvasDrag::default());
    // The World Doc panel reads the bake of THIS scene (interiors have none).
    config.bake_dir = super::worlddoc::default_bake_dir(&event.path);
    commands.insert_resource(super::worlddoc::WorldDocState::default());
    status.0 = format!("{}: {title}", t(*lang, StrKey::SceneOpened));
}

/// O caminho de uma aba na forma de [`SceneDoc::dirty_paths`] (relativo à raiz, com `/`).
fn dirty_key(doc: &SceneDoc, path: &Path) -> String {
    path.strip_prefix(&doc.root_dir)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Rebuilds the tab buttons when the tabs, the language or the dirty state change.
#[allow(clippy::too_many_arguments)]
fn render_tabs(
    mut commands: Commands,
    mut tabs: ResMut<SceneTabs>,
    doc: Option<Res<SceneDoc>>,
    lang: Res<Lang>,
    bars: Query<Entity, With<SceneTabBar>>,
) {
    let Ok(bar) = bars.single() else {
        return;
    };
    // O marcador é POR aba: o ficheiro sujo pode ser o de uma aba em segundo
    // plano, e um ficheiro sujo que não é de nenhuma aba (o heightmap do
    // terreno) fica no marcador da ativa.
    let dirty_paths = doc
        .as_ref()
        .map(|doc| doc.dirty_paths())
        .unwrap_or_default();
    let mut per_tab_dirty: Vec<bool> = tabs
        .tabs
        .iter()
        .map(|tab| {
            doc.as_ref().is_some_and(|doc| {
                let key = dirty_key(doc, &tab.path);
                dirty_paths.iter().any(|dirty| *dirty == key)
            })
        })
        .collect();
    if (!dirty_paths.is_empty() && !per_tab_dirty.iter().any(|dirty| *dirty))
        && let Some(active) = per_tab_dirty.get_mut(tabs.active)
    {
        *active = true;
    }
    if tabs.rendered_rev == Some(tabs.rev)
        && tabs.rendered_dirty == per_tab_dirty
        && !lang.is_changed()
    {
        return;
    }
    tabs.rendered_rev = Some(tabs.rev);
    tabs.rendered_dirty = per_tab_dirty.clone();
    commands.entity(bar).despawn_related::<Children>();
    let closable = tabs.tabs.len() > 1;
    for (index, tab) in tabs.tabs.iter().enumerate() {
        let active = index == tabs.active;
        let dirty = per_tab_dirty.get(index).copied().unwrap_or(false);
        let label = if dirty {
            format!("{}  •", tab.title)
        } else {
            tab.title.clone()
        };
        let (bg, fg, border) = if active {
            (theme::BG_PANEL, theme::TEXT, theme::ACCENT)
        } else {
            (theme::BG_HEADER, theme::TEXT_DIM, theme::BG_HEADER)
        };
        commands
            .spawn((
                TabButton(index),
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(6),
                    height: px(24),
                    padding: UiRect::new(px(10), px(if closable { 4 } else { 10 }), px(0), px(0)),
                    border: UiRect::top(px(2)),
                    border_radius: BorderRadius::top(px(5)),
                    ..default()
                },
                BackgroundColor(bg),
                BorderColor::all(border),
                Tooltip(t(*lang, StrKey::SceneTabTip)),
                ChildOf(bar),
            ))
            .observe(tab_clicked)
            .with_children(|row| {
                row.spawn((
                    icons::icon_text(
                        fa::CUBES,
                        10.0,
                        if active {
                            theme::ACCENT
                        } else {
                            icons::ICON_DIM
                        },
                    ),
                    Pickable::IGNORE,
                ));
                row.spawn((
                    Text::new(label),
                    TextFont::from_font_size(theme::FONT_BODY),
                    TextColor(fg),
                    Pickable::IGNORE,
                ));
                if closable {
                    row.spawn((
                        TabClose(index),
                        icons::icon_text('\u{f00d}', 9.0, icons::ICON_DIM),
                        Node {
                            padding: UiRect::all(px(3)),
                            ..default()
                        },
                        Tooltip(t(*lang, StrKey::SceneCloseTip)),
                    ))
                    .observe(close_clicked);
                }
            });
    }
    commands
        .spawn((
            NewSceneButton,
            Node {
                align_items: AlignItems::Center,
                height: px(24),
                padding: UiRect::horizontal(px(8)),
                ..default()
            },
            Tooltip(t(*lang, StrKey::SceneNewTip)),
            ChildOf(bar),
        ))
        .observe(new_scene_clicked)
        .with_children(|row| {
            row.spawn((
                icons::icon_text(fa::PLUS, 11.0, icons::ICON_DIM),
                Pickable::IGNORE,
            ));
        });
}

fn tab_clicked(
    click: On<PointerClick>,
    buttons: Query<&TabButton>,
    tabs: Res<SceneTabs>,
    mut requests: ResMut<OpenSceneRequest>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(TabButton(index)) = buttons.get(click.event_target()) else {
        return;
    };
    if *index == tabs.active {
        return;
    }
    if let Some(tab) = tabs.tabs.get(*index) {
        requests.0 = Some(tab.path.clone());
    }
}

fn close_clicked(
    mut click: On<PointerClick>,
    buttons: Query<&TabClose>,
    mut tabs: ResMut<SceneTabs>,
    mut commands: Commands,
) {
    // Só o botão esquerdo fecha (o direito abria o mesmo caminho e fechava a aba).
    if click.button != PointerButton::Primary {
        return;
    }
    click.propagate(false);
    let Ok(TabClose(index)) = buttons.get(click.event_target()) else {
        return;
    };
    let index = *index;
    if index != tabs.active {
        // Uma aba de fundo não muda a cena do viewport.
        tabs.close(index);
        return;
    }
    // A aba ATIVA só fecha depois de gravar. O swap de cena é pedido depois e,
    // se a gravação falhasse no `process_open_requests`, a barra já tinha
    // perdido a aba da cena que continua aberta no viewport.
    let Some(path) = tabs.tabs.get(index).map(|tab| tab.path.clone()) else {
        return;
    };
    commands.queue(move |world: &mut World| close_active_tab(world, path));
}

/// Grava a cena ativa e só então fecha a aba e pede a vizinha.
fn close_active_tab(world: &mut World, path: PathBuf) {
    let lang = *world.resource::<Lang>();
    if let Err(error) = crate::scene::try_save_world(world) {
        let status = save_failed(lang, &error);
        crate::ui::toast::push(world, crate::ui::toast::ToastKind::Error, status.clone());
        world.resource_mut::<EditStatus>().0 = status;
        return;
    }
    let next = {
        let mut tabs = world.resource_mut::<SceneTabs>();
        let Some(index) = tabs.tabs.iter().position(|tab| tab.path == path) else {
            return;
        };
        if tabs.close(index) {
            Some(tabs.tabs[tabs.active].path.clone())
        } else {
            None
        }
    };
    if let Some(next) = next {
        world.resource_mut::<OpenSceneRequest>().0 = Some(next);
    }
}

fn new_scene_clicked(
    click: On<PointerClick>,
    doc: Res<SceneDoc>,
    mut requests: ResMut<OpenSceneRequest>,
    mut status: ResMut<EditStatus>,
    lang: Res<Lang>,
) {
    // Um clique direito no "+" criava um ficheiro de cena e trocava de cena.
    if click.button != PointerButton::Primary {
        return;
    }
    let project = Project::discover(doc.world_path());
    let mut n = 1;
    let path = loop {
        let id = format!("cena-{n}");
        match project.new_scene(&id, SceneTemplate::Empty) {
            Ok(path) => break path,
            Err(_) if n < 999 => n += 1,
            Err(error) => {
                status.0 = format!("{}: {error:#}", t(*lang, StrKey::SceneOpenFailed));
                return;
            }
        }
    };
    requests.0 = Some(path);
}

/// Opens the requested scene: saves the current one, then swaps in-process (project
/// scenes) or opens a new editor window (a world outside the project).
fn process_open_requests(world: &mut World) {
    let Some(path) = world.resource_mut::<OpenSceneRequest>().0.take() else {
        return;
    };
    let lang = *world.resource::<Lang>();
    let current = world.get_resource::<CurrentScene>().cloned();
    let id = current
        .as_ref()
        .filter(|c| c.switchable())
        .and_then(|c| Project::at(&c.root).scene_id(&path));
    let Some(id) = id else {
        // Not a scene of this project: the old behaviour — its own editor window.
        let opened = std::env::current_exe()
            .ok()
            .and_then(|exe| std::process::Command::new(exe).arg(&path).spawn().ok())
            .is_some();
        world.resource_mut::<EditStatus>().0 = if opened {
            t(lang, StrKey::SceneNotProject).to_string()
        } else {
            t(lang, StrKey::SceneOpenFailed).to_string()
        };
        return;
    };
    if let Err(error) = crate::scene::try_save_world(world) {
        let status = format!("{}: {error:#}", t(lang, StrKey::SceneOpenFailed));
        crate::ui::toast::push(world, crate::ui::toast::ToastKind::Error, status.clone());
        world.resource_mut::<EditStatus>().0 = status;
        return;
    }
    let request = ChangeScene::new(id, Arrival::Keep);
    if let Err(error) = viber::scene_loader::request_change(world, request) {
        world.resource_mut::<EditStatus>().0 =
            format!("{}: {error:#}", t(lang, StrKey::SceneOpenFailed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_open_activate_and_close() {
        let mut tabs = SceneTabs::default();
        let a = tabs.open(Path::new("/x/world.xml"), "Vila".into());
        let b = tabs.open(Path::new("/x/scenes/house.xml"), "Casa".into());
        assert_eq!((a, b, tabs.active), (0, 1, 1));
        // Reopening activates, never duplicates.
        assert_eq!(tabs.open(Path::new("/x/world.xml"), "Vila".into()), 0);
        assert_eq!(tabs.tabs.len(), 2);
        assert_eq!(tabs.active, 0);
        // Closing a tab before the active one keeps the same scene active.
        tabs.open(Path::new("/x/scenes/house.xml"), "Casa".into());
        assert!(tabs.close(0));
        assert_eq!(tabs.tabs[tabs.active].title, "Casa");
        // The last tab cannot be closed.
        assert!(!tabs.close(0));
    }
}
