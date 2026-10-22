//! Editor preferences persisted between runs: dock sizes/visibility, the active
//! bottom tab, language, view options, snap steps, window geometry, the open
//! scene tabs and their camera poses.
//!
//! The file lives in `$XDG_CONFIG_HOME/viber/editor.json` (falling back to
//! `~/.config/viber/editor.json`). CLI flags still win: prefs are applied
//! before the `--lang` / `--tab` / `--views` startup systems.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window, WindowPosition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use viber::editor_camera::EditorCamState;

use crate::plugins::{ActivePluginPanel, PluginHost};
use crate::ui::layout::{ActiveDockTab, DockSizes, DockVisibility, TabKind};
use crate::ui::scene_tabs::{SceneTabs, TabCamera};
use crate::ui::strings::Lang;
use crate::ui::view_options::{CameraView, ViewOptions};
use crate::viewport::gizmo::SnapSettings;

/// Everything the editor remembers between sessions.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorPrefs {
    pub docks: DockPrefs,
    pub panels: PanelPrefs,
    pub active_tab: String,
    pub lang: Option<String>,
    pub view: ViewPrefs,
    pub snap: SnapPrefs,
    pub window: Option<WindowPrefs>,
    pub tabs: Vec<TabPrefs>,
    /// Estado persistido de cada plugin (`editor.prefs`), por id de plugin.
    pub plugin_state: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    /// Último painel de plugin aberto (`plugin:painel`).
    pub plugin_panel: Option<String>,
}

impl Default for EditorPrefs {
    fn default() -> Self {
        Self {
            docks: DockPrefs::default(),
            panels: PanelPrefs::default(),
            active_tab: TabKind::Console.key().to_owned(),
            lang: None,
            view: ViewPrefs::default(),
            snap: SnapPrefs::default(),
            window: None,
            tabs: Vec::new(),
            plugin_state: BTreeMap::new(),
            plugin_panel: None,
        }
    }
}

impl EditorPrefs {
    pub fn lang(&self) -> Lang {
        self.lang
            .as_deref()
            .and_then(Lang::parse)
            .unwrap_or_default()
    }

    pub fn camera_for(&self, path: &Path) -> Option<TabCamera> {
        let wanted = path.to_string_lossy();
        self.tabs
            .iter()
            .find(|tab| tab.path == wanted)
            .and_then(|tab| tab.camera)
            .map(TabCamera::from)
    }

    pub fn remember_camera(&mut self, path: &Path, camera: TabCamera) {
        let wanted = path.to_string_lossy();
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.path == wanted) {
            tab.camera = Some(camera.into());
        }
    }

    pub fn dock_sizes(&self) -> DockSizes {
        DockSizes {
            left: self.docks.left,
            right: self.docks.right,
            bottom: self.docks.bottom,
        }
    }

    pub fn dock_visibility(&self) -> DockVisibility {
        DockVisibility {
            left: self.panels.left,
            right: self.panels.right,
            bottom: self.panels.bottom,
        }
    }

    pub fn active_dock_tab(&self) -> ActiveDockTab {
        ActiveDockTab(TabKind::from_key(&self.active_tab).unwrap_or_default())
    }

    pub fn view_options(&self) -> ViewOptions {
        ViewOptions {
            grid: self.view.grid,
            colliders: self.view.colliders,
            ortho: self.view.ortho,
        }
    }

    pub fn camera_view(&self) -> CameraView {
        CameraView::from_key(&self.view.camera).unwrap_or_default()
    }

    pub fn snap_settings(&self) -> SnapSettings {
        SnapSettings {
            enabled: self.snap.enabled,
            move_m: self.snap.move_m,
            rotate_deg: self.snap.rotate_deg,
            scale: self.snap.scale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockPrefs {
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for DockPrefs {
    fn default() -> Self {
        let sizes = DockSizes::default();
        Self {
            left: sizes.left,
            right: sizes.right,
            bottom: sizes.bottom,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelPrefs {
    pub left: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Default for PanelPrefs {
    fn default() -> Self {
        Self {
            left: true,
            right: true,
            bottom: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewPrefs {
    pub grid: bool,
    pub colliders: bool,
    pub ortho: bool,
    pub camera: String,
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            grid: false,
            colliders: false,
            ortho: false,
            camera: CameraView::Perspective.key().to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapPrefs {
    pub enabled: bool,
    pub move_m: f32,
    pub rotate_deg: f32,
    pub scale: f32,
}

impl Default for SnapPrefs {
    fn default() -> Self {
        let snap = SnapSettings::default();
        Self {
            enabled: snap.enabled,
            move_m: snap.move_m,
            rotate_deg: snap.rotate_deg,
            scale: snap.scale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPrefs {
    pub width: u32,
    pub height: u32,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabPrefs {
    pub path: String,
    pub camera: Option<CameraPrefs>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraPrefs {
    pub target: [f32; 3],
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub distance: f32,
}

impl From<TabCamera> for CameraPrefs {
    fn from(camera: TabCamera) -> Self {
        Self {
            target: camera.target.to_array(),
            yaw_deg: camera.yaw_deg,
            pitch_deg: camera.pitch_deg,
            distance: camera.distance,
        }
    }
}

impl From<CameraPrefs> for TabCamera {
    fn from(camera: CameraPrefs) -> Self {
        Self {
            target: Vec3::from_array(camera.target),
            yaw_deg: camera.yaw_deg,
            pitch_deg: camera.pitch_deg,
            distance: camera.distance,
        }
    }
}

impl CameraPrefs {
    pub fn from_state(camera: &EditorCamState) -> Self {
        Self {
            target: camera.target.to_array(),
            yaw_deg: camera.yaw_deg,
            pitch_deg: camera.pitch_deg,
            distance: camera.distance,
        }
    }
}

/// Builds the snapshot written to disk from the live editor state.
#[allow(clippy::too_many_arguments)]
pub fn snapshot(
    docks: DockSizes,
    panels: DockVisibility,
    active: ActiveDockTab,
    lang: Lang,
    view: ViewOptions,
    camera_view: CameraView,
    snap: SnapSettings,
    tabs: &SceneTabs,
    camera: Option<&EditorCamState>,
    window: Option<&Window>,
    plugin_state: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    plugin_panel: Option<String>,
) -> EditorPrefs {
    let mut prefs = EditorPrefs {
        docks: DockPrefs {
            left: docks.left,
            right: docks.right,
            bottom: docks.bottom,
        },
        panels: PanelPrefs {
            left: panels.left,
            right: panels.right,
            bottom: panels.bottom,
        },
        active_tab: active.0.key().to_owned(),
        lang: Some(match lang {
            Lang::Pt => "pt".to_owned(),
            Lang::En => "en".to_owned(),
        }),
        view: ViewPrefs {
            grid: view.grid,
            colliders: view.colliders,
            ortho: view.ortho,
            camera: camera_view.key().to_owned(),
        },
        snap: SnapPrefs {
            enabled: snap.enabled,
            move_m: snap.move_m,
            rotate_deg: snap.rotate_deg,
            scale: snap.scale,
        },
        window: window.map(|window| {
            let (x, y) = match window.position {
                WindowPosition::At(position) => (Some(position.x), Some(position.y)),
                _ => (None, None),
            };
            WindowPrefs {
                width: window.resolution.width().round().max(1.0) as u32,
                height: window.resolution.height().round().max(1.0) as u32,
                x,
                y,
            }
        }),
        tabs: Vec::new(),
        plugin_state,
        plugin_panel,
    };
    for (index, tab) in tabs.tabs.iter().enumerate() {
        let pose = if index == tabs.active {
            camera
                .map(CameraPrefs::from_state)
                .or(tab.camera.map(Into::into))
        } else {
            tab.camera.map(Into::into)
        };
        prefs.tabs.push(TabPrefs {
            path: tab.path.to_string_lossy().into_owned(),
            camera: pose,
        });
    }
    prefs
}

/// `$XDG_CONFIG_HOME/viber/editor.json`, else `~/.config/viber/editor.json`.
pub fn path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir).join("viber").join("editor.json"));
    }
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("viber")
            .join("editor.json")
    })
}

pub fn load() -> EditorPrefs {
    path().map_or_else(EditorPrefs::default, |path| load_from(&path))
}

pub fn load_from(path: &Path) -> EditorPrefs {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(prefs) => prefs,
            Err(error) => {
                warn!(
                    "prefs: {} inválido ({error:#}) — a usar defaults",
                    path.display()
                );
                EditorPrefs::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => EditorPrefs::default(),
        Err(error) => {
            warn!(
                "prefs: {} ilegível ({error:#}) — a usar defaults",
                path.display()
            );
            EditorPrefs::default()
        }
    }
}

pub fn save(prefs: &EditorPrefs) -> anyhow::Result<()> {
    let Some(path) = path() else {
        anyhow::bail!("sem HOME/XDG_CONFIG_HOME para guardar as preferências");
    };
    save_to(&path, prefs)
}

pub fn save_to(path: &Path, prefs: &EditorPrefs) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(prefs)?;
    viber::scene::doc::write_atomic(path, &bytes)
}

/// Persists the current state immediately (window close, Ctrl+Q).
pub fn save_now(world: &mut World) {
    let Some(tabs) = world.get_resource::<SceneTabs>().cloned() else {
        return;
    };
    let docks = world
        .get_resource::<DockSizes>()
        .copied()
        .unwrap_or_default();
    let panels = world
        .get_resource::<DockVisibility>()
        .copied()
        .unwrap_or_default();
    let active = world
        .get_resource::<ActiveDockTab>()
        .copied()
        .unwrap_or_default();
    let lang = world.get_resource::<Lang>().copied().unwrap_or_default();
    let view = world
        .get_resource::<ViewOptions>()
        .copied()
        .unwrap_or_default();
    let camera_view = world
        .get_resource::<CameraView>()
        .copied()
        .unwrap_or_default();
    let snap = world
        .get_resource::<SnapSettings>()
        .copied()
        .unwrap_or_default();
    let camera = world.get_resource::<EditorCamState>().cloned();
    let window = {
        let mut query = world.query_filtered::<&Window, With<PrimaryWindow>>();
        query.iter(world).next().cloned()
    };
    let plugin_state = world
        .get_resource::<PluginHost>()
        .map(|host| host.plugin_state())
        .unwrap_or_default();
    let plugin_panel = world
        .get_resource::<ActivePluginPanel>()
        .and_then(|panel| panel.0.clone());
    let snapshot = snapshot(
        docks,
        panels,
        active,
        lang,
        view,
        camera_view,
        snap,
        &tabs,
        camera.as_ref(),
        window.as_ref(),
        plugin_state,
        plugin_panel,
    );
    if let Err(error) = save(&snapshot) {
        warn!("prefs: {error:#}");
    }
}

pub struct PrefsPlugin;

impl Plugin for PrefsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, apply_startup_window)
            .add_systems(Update, save_prefs);
    }
}

/// Restores the window geometry once the Bevy window exists.
fn apply_startup_window(
    prefs: Res<EditorPrefs>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let Some(saved) = &prefs.window else {
        return;
    };
    if saved.width >= 320 && saved.height >= 240 {
        window
            .resolution
            .set(saved.width as f32, saved.height as f32);
    }
    if let (Some(x), Some(y)) = (saved.x, saved.y) {
        window.position = WindowPosition::At(IVec2::new(x, y));
    }
}

/// Writes the snapshot at most every few seconds, and only when it changed.
#[allow(clippy::too_many_arguments)]
fn save_prefs(
    time: Res<Time<Real>>,
    mut elapsed: Local<f32>,
    mut last: Local<Option<EditorPrefs>>,
    docks: Res<DockSizes>,
    panels: Res<DockVisibility>,
    active: Res<ActiveDockTab>,
    lang: Res<Lang>,
    view: Res<ViewOptions>,
    camera_view: Res<CameraView>,
    snap: Res<SnapSettings>,
    tabs: Option<Res<SceneTabs>>,
    camera: Option<Res<EditorCamState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    plugin: Option<Res<PluginHost>>,
    plugin_panel: Option<Res<ActivePluginPanel>>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < 3.0 {
        return;
    }
    *elapsed = 0.0;
    let Some(tabs) = tabs else {
        return;
    };
    let snapshot = snapshot(
        *docks,
        *panels,
        *active,
        *lang,
        *view,
        *camera_view,
        *snap,
        &tabs,
        camera.as_deref(),
        windows.single().ok(),
        plugin
            .as_ref()
            .map(|host| host.plugin_state())
            .unwrap_or_default(),
        plugin_panel.and_then(|panel| panel.0.clone()),
    );
    if last.as_ref() == Some(&snapshot) {
        return;
    }
    match save(&snapshot) {
        Ok(()) => *last = Some(snapshot),
        Err(error) => warn!("prefs: {error:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::scene_tabs::SceneTab;

    #[test]
    fn prefs_round_trip_through_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("editor.json");
        let mut prefs = EditorPrefs::default();
        prefs.docks.left = 321.0;
        prefs.panels.bottom = false;
        prefs.active_tab = "history".to_owned();
        prefs.lang = Some("en".to_owned());
        prefs.snap.move_m = 0.25;
        prefs.tabs.push(TabPrefs {
            path: "/x/world.xml".to_owned(),
            camera: Some(CameraPrefs {
                target: [1.0, 2.0, 3.0],
                yaw_deg: 45.0,
                pitch_deg: 30.0,
                distance: 80.0,
            }),
        });
        save_to(&path, &prefs).unwrap();
        assert_eq!(load_from(&path), prefs);
        assert_eq!(prefs.lang(), Lang::En);
        assert_eq!(prefs.active_dock_tab().0, TabKind::History);
        assert!(!prefs.dock_visibility().bottom);
        assert_eq!(
            prefs
                .camera_for(Path::new("/x/world.xml"))
                .unwrap()
                .distance,
            80.0
        );
    }

    #[test]
    fn missing_or_invalid_files_fall_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.json");
        assert_eq!(load_from(&missing), EditorPrefs::default());
        let invalid = tmp.path().join("invalid.json");
        std::fs::write(&invalid, "{not json").unwrap();
        assert_eq!(load_from(&invalid), EditorPrefs::default());
    }

    #[test]
    fn snapshot_keeps_the_camera_of_the_active_tab() {
        let mut tabs = SceneTabs::default();
        tabs.tabs.push(SceneTab {
            path: PathBuf::from("/x/world.xml"),
            title: "World".into(),
            camera: None,
        });
        tabs.active = 0;
        let camera = EditorCamState {
            target: Vec3::new(4.0, 5.0, 6.0),
            distance: 42.0,
            ..Default::default()
        };
        let prefs = snapshot(
            DockSizes::default(),
            DockVisibility::default(),
            ActiveDockTab::default(),
            Lang::Pt,
            ViewOptions::default(),
            CameraView::default(),
            SnapSettings::default(),
            &tabs,
            Some(&camera),
            None,
            BTreeMap::new(),
            None,
        );
        let saved = prefs.camera_for(Path::new("/x/world.xml")).unwrap();
        assert_eq!(saved.target, Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(saved.distance, 42.0);
    }
}
