//! The edited scene, on top of the engine's own editing API (`viber::scene`): the same
//! document, operations, history and live sync agents use through `viber scene` and the
//! `viber.scene.*` bridge. The editor adds no editing logic of its own — every change it makes
//! is an [`EditOp`] batch queued in [`PendingEdits`] and applied by `viber::scene::live`.

pub mod add_node;
pub mod inspector;
pub mod property;
pub mod script_props;
pub mod tree;
pub mod tree_drag;

pub use viber::scene::doc;

use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use std::collections::HashSet;
use viber::scene::live;
use viber::scene::ops::EditOp;

use crate::ui::strings::{
    Lang, delete_label, duplicate_label, edit_failed, failed, nothing_to_redo, nothing_to_undo,
    redo_prefix, save_failed, save_nothing, saved, undo_prefix,
};

use doc::{NodeRef, SceneDoc};

/// Texto vivo de um input. O `FeathersTextInput` do Bevy 0.20 guarda o que foi
/// digitado em `EditableText`; o componente `Text` ou não existe ou fica no
/// valor do spawn. Preferir `EditableText` — senão Enter vê o valor antigo.
pub fn input_text(world: &World, entity: Entity) -> Option<String> {
    if let Some(editable) = world.get::<bevy::text::EditableText>(entity) {
        return Some(editable.value().to_string());
    }
    world.get::<Text>(entity).map(|text| text.0.clone())
}

/// The node selected in the Objects dock or the viewport.
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct Selection(pub Vec<NodeRef>);

impl Selection {
    /// The primary node (the one the gizmo/inspector/canvas act on).
    pub fn primary(&self) -> Option<&NodeRef> {
        self.0.first()
    }
    pub fn set_one(&mut self, node: NodeRef) {
        self.0 = vec![node];
    }
    /// Ctrl-click: toggle membership (never empties via toggle of primary? Godot allows empty).
    pub fn toggle(&mut self, node: &NodeRef) {
        if let Some(index) = self.0.iter().position(|current| current == node) {
            self.0.remove(index);
        } else {
            self.0.push(node.clone());
        }
    }
    /// Shift-click: add (dedup).
    pub fn extend(&mut self, node: &NodeRef) {
        if !self.0.contains(node) {
            self.0.push(node.clone());
        }
    }
}

/// Expanded nodes of the Scene tree.
#[derive(Resource, Debug, Default, Clone)]
pub struct TreeExpansion(pub HashSet<NodeRef>);

impl TreeExpansion {
    pub fn collapse_all(&mut self) {
        self.0.clear();
    }

    /// Expands every branch that has children (includes expand into their file).
    pub fn expand_all(&mut self, doc: &SceneDoc) {
        self.0 = tree::all_expandable(doc).into_iter().collect();
    }
}

/// Edits requested by the UI this frame, applied in order by [`apply_pending_edits`].
#[derive(Resource, Debug, Default)]
pub struct PendingEdits(pub Vec<Edit>);

/// One undoable editor action.
#[derive(Debug, Clone)]
pub struct Edit {
    pub label: String,
    pub ops: Vec<EditOp>,
    /// Where the selection goes once the edit is applied (`None` keeps it).
    pub select_after: Option<Option<NodeRef>>,
    /// Select (and reveal) the first node the transaction inserted — drops, Add Node.
    pub select_inserted: bool,
    /// After the edit, select the node with this `name` (agrupar).
    pub select_name: Option<String>,
}

impl PendingEdits {
    pub fn push(&mut self, label: impl Into<String>, ops: Vec<EditOp>) {
        self.0.push(Edit {
            label: label.into(),
            ops,
            select_after: None,
            select_inserted: false,
            select_name: None,
        });
    }

    /// Like [`Self::push`], then selects and reveals the node the edit inserts.
    pub fn push_selecting_inserted(&mut self, label: impl Into<String>, ops: Vec<EditOp>) {
        self.0.push(Edit {
            label: label.into(),
            ops,
            select_after: None,
            select_inserted: true,
            select_name: None,
        });
    }
}

/// Nós da seleção que não são descendentes de outro selecionado. Copiar ou
/// duplicar o pai já leva os filhos — incluí-los outra vez duplica-os.
pub fn selection_roots(nodes: &[NodeRef]) -> Vec<NodeRef> {
    nodes
        .iter()
        .filter(|node| {
            !node.is_root()
                && !nodes.iter().any(|other| {
                    other.file == node.file
                        && other.path.len() < node.path.len()
                        && node.path.starts_with(&other.path)
                })
        })
        .cloned()
        .collect()
}

/// Índice da cópia do primário depois de duplicar também os irmãos anteriores
/// (cada um insere um nó à sua frente).
pub fn duplicate_copy_index(primary_index: usize, earlier_siblings: usize) -> usize {
    primary_index + 1 + earlier_siblings
}

/// Irmãos do mesmo pai, com índice menor, que também entram no lote.
pub fn earlier_sibling_copies(nodes: &[NodeRef], primary: &NodeRef) -> usize {
    let Some(depth) = primary.path.len().checked_sub(1) else {
        return 0;
    };
    let index = primary.path.last().copied().unwrap_or(0);
    nodes
        .iter()
        .filter(|node| {
            node.file == primary.file
                && node.path.len() == primary.path.len()
                && node.path[..depth] == primary.path[..depth]
                && node.path.last().copied().unwrap_or(0) < index
        })
        .count()
}

/// Onde colar: a seguir ao nó, ou no fim da raiz quando a seleção está vazia
/// ou é o próprio `<world>`.
pub fn paste_anchor(doc: &SceneDoc, anchor: Option<&NodeRef>) -> (NodeRef, usize) {
    match anchor {
        Some(node) if !node.is_root() => {
            let parent = node.parent().unwrap_or_else(|| NodeRef::root(node.file));
            (parent, node.path.last().copied().unwrap_or(0) + 1)
        }
        Some(node) => {
            let len = doc.children(node).map(|kids| kids.len()).unwrap_or(0);
            (node.clone(), len)
        }
        None => {
            let root = NodeRef::root(0);
            let len = doc.children(&root).map(|kids| kids.len()).unwrap_or(0);
            (root, len)
        }
    }
}

/// Ops `insert` da colagem — as mesmas que `viber scene insert`.
pub fn paste_ops(doc: &SceneDoc, anchor: Option<&NodeRef>, clipboard: &[String]) -> Vec<EditOp> {
    let (parent, mut index) = paste_anchor(doc, anchor);
    let parent_selector = doc.selector(&parent);
    clipboard
        .iter()
        .map(|xml| {
            let op = EditOp::Insert {
                parent: parent_selector.clone(),
                index: Some(index),
                xml: xml.clone(),
            };
            index += 1;
            op
        })
        .collect()
}

/// The last editing outcome, shown in the toolbar.
#[derive(Resource, Debug, Default, Clone)]
pub struct EditStatus(pub String);

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Selection>()
            .init_resource::<TreeExpansion>()
            .init_resource::<PendingEdits>()
            .init_resource::<EditStatus>()
            .add_plugins((add_node::AddNodeDialogPlugin, property::PropertyPlugin))
            .add_systems(
                Update,
                (
                    property::apply_property_changes,
                    property::prefill_text_fields,
                    property::commit_text_fields,
                    property::sync_section_toggles,
                    autosave,
                ),
            )
            .init_resource::<tree_drag::TreeDrag>()
            .init_resource::<tree_drag::TreeDropHint>()
            .init_resource::<tree::RenameState>()
            .init_resource::<tree::TreeView>()
            .init_resource::<inspector::InspectorView>()
            .add_systems(
                Update,
                (
                    shortcuts,
                    apply_pending_edits,
                    validate_selection,
                    tree::apply_tree_search,
                    tree::live_tree_filter,
                    tree::tree_keys,
                    tree::prefill_rename,
                    tree::commit_rename,
                    tree::render_tree,
                    inspector::render_inspector,
                    inspector::prefill_inputs,
                    inspector::commit_inputs,
                )
                    .chain(),
            );
    }
}

/// Copied node XMLs (Ctrl+C → Ctrl+V), one deep each.
#[derive(Resource, Debug, Default, Clone)]
pub struct NodeClipboard(pub Vec<String>);

fn paste_str(lang: crate::ui::strings::Lang) -> &'static str {
    crate::ui::strings::t(lang, crate::ui::strings::StrKey::EditPaste)
}

/// Keys that act on the document, unless a text field has focus:
/// Ctrl+Z undo · Ctrl+Shift+Z / Ctrl+Y redo · Ctrl+S save · Ctrl+D duplicate · Delete remove.
fn shortcuts(world: &mut World) {
    let typing = world
        .get_resource::<InputFocus>()
        .and_then(InputFocus::get)
        .is_some_and(|entity| world.get::<bevy::text::EditableText>(entity).is_some());
    let dialog = world.resource::<crate::scene::add_node::AddNodeOpen>().0
        || world
            .resource::<crate::ui::context_menu::ContextMenu>()
            .open;
    let keys = world.resource::<ButtonInput<KeyCode>>();
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let pressed = |key| keys.just_pressed(key);
    let undo = ctrl && pressed(KeyCode::KeyZ) && !shift;
    let redo = ctrl && ((pressed(KeyCode::KeyZ) && shift) || pressed(KeyCode::KeyY));
    let save = ctrl && pressed(KeyCode::KeyS);
    let duplicate = ctrl && pressed(KeyCode::KeyD);
    let reload = ctrl && pressed(KeyCode::KeyR);
    let delete = pressed(KeyCode::Delete);
    let copy = ctrl && pressed(KeyCode::KeyC);
    let paste = ctrl && pressed(KeyCode::KeyV);
    let rename = pressed(KeyCode::F2);
    let toggle_left = ctrl && pressed(KeyCode::KeyB);
    let toggle_right = ctrl && pressed(KeyCode::KeyI);
    let toggle_bottom = ctrl && pressed(KeyCode::KeyJ);
    let quit = ctrl && pressed(KeyCode::KeyQ);
    let close_tab = ctrl && pressed(KeyCode::KeyW);
    // Ctrl+S grava mesmo com um campo focado — não é um atalho do texto.
    if save {
        save_world(world);
    }
    // Ctrl+Z num campo desfaz o texto, não a cena.
    if typing {
        return;
    }
    if toggle_left {
        world
            .resource_mut::<crate::ui::layout::DockVisibility>()
            .toggle(crate::ui::layout::DockSide::Left);
    }
    if toggle_right {
        world
            .resource_mut::<crate::ui::layout::DockVisibility>()
            .toggle(crate::ui::layout::DockSide::Right);
    }
    if toggle_bottom {
        world
            .resource_mut::<crate::ui::layout::DockVisibility>()
            .toggle(crate::ui::layout::DockSide::Bottom);
    }
    if quit {
        // Do not leave on a failed save: the user would lose the edits.
        if let Err(error) = try_save_world(world) {
            let status = failed(*world.resource::<Lang>(), &error);
            crate::ui::toast::push(world, crate::ui::toast::ToastKind::Error, status.clone());
            world.resource_mut::<EditStatus>().0 = status;
            return;
        }
        crate::prefs::save_now(world);
        world.write_message(AppExit::Success);
        return;
    }
    if close_tab {
        // Save first: closing loses the tab entry, so a failed save must not
        // drop the only reference to the dirty scene.
        if let Err(error) = try_save_world(world) {
            let status = failed(*world.resource::<Lang>(), &error);
            world.resource_mut::<EditStatus>().0 = status;
            return;
        }
        let next = {
            let mut tabs = world.resource_mut::<crate::ui::scene_tabs::SceneTabs>();
            if tabs.tabs.len() <= 1 {
                None
            } else {
                let active = tabs.active;
                tabs.close(active)
                    .then(|| tabs.tabs[tabs.active].path.clone())
            }
        };
        if let Some(next) = next {
            world
                .resource_mut::<crate::ui::scene_tabs::OpenSceneRequest>()
                .0 = Some(next);
        }
        return;
    }
    if reload {
        crate::app::reload_world(world);
        return;
    }
    // Add Node / menu de contexto: undo e reload ficam; Delete não apaga a cena por baixo.
    if dialog {
        let lang = *world.resource::<Lang>();
        if undo {
            let result = live::undo(world).map(|tx| tx.map(|tx| undo_prefix(lang, &tx.label)));
            report(world, result, nothing_to_undo(lang));
        } else if redo {
            let result = live::redo(world).map(|tx| tx.map(|tx| redo_prefix(lang, &tx.label)));
            report(world, result, nothing_to_redo(lang));
        }
        return;
    }
    let lang = *world.resource::<Lang>();
    if undo {
        let result = live::undo(world).map(|tx| tx.map(|tx| undo_prefix(lang, &tx.label)));
        report(world, result, nothing_to_undo(lang));
    } else if redo {
        let result = live::redo(world).map(|tx| tx.map(|tx| redo_prefix(lang, &tx.label)));
        report(world, result, nothing_to_redo(lang));
    }
    if paste {
        paste_clipboard(world);
    }
    if duplicate {
        duplicate_selection(world);
    }
    if rename {
        begin_rename(world);
    }
    if copy {
        copy_selection(world);
    }
    if delete {
        delete_selection(world);
    }
}

/// The selected roots in descending document order: removing/duplicating a
/// previous sibling shifts the indices of the next ones.
fn editable_selection(world: &World) -> Vec<NodeRef> {
    let mut editable = selection_roots(&world.resource::<Selection>().0);
    editable.sort();
    editable.dedup();
    editable.reverse();
    editable
}

/// Ctrl+D / palette: duplicates every selected root in one transaction.
pub fn duplicate_selection(world: &mut World) {
    let lang = *world.resource::<Lang>();
    let selected = world.resource::<Selection>().0.clone();
    let Some(primary) = selected.first().cloned() else {
        return;
    };
    let selector = world.resource::<SceneDoc>().selector(&primary);
    let editable = editable_selection(world);
    if editable.is_empty() {
        return;
    }
    let ops: Vec<EditOp> = editable
        .iter()
        .map(|node| EditOp::Duplicate {
            node: world.resource::<SceneDoc>().selector(node),
        })
        .collect();
    let focus = if editable.iter().any(|node| node == &primary) {
        primary.clone()
    } else {
        editable
            .iter()
            .find(|node| node.file == primary.file && primary.path.starts_with(&node.path))
            .cloned()
            .unwrap_or_else(|| primary.clone())
    };
    let copy = focus.parent().map(|parent| {
        let index = duplicate_copy_index(
            focus.path.last().copied().unwrap_or(0),
            earlier_sibling_copies(&editable, &focus),
        );
        parent.child(index)
    });
    world.resource_mut::<PendingEdits>().0.push(Edit {
        label: duplicate_label(lang, &selector),
        ops,
        select_after: Some(copy),
        select_inserted: false,
        select_name: None,
    });
}

/// Delete / palette: removes every selected root in one transaction.
pub fn delete_selection(world: &mut World) {
    let lang = *world.resource::<Lang>();
    let selected = world.resource::<Selection>().0.clone();
    let Some(primary) = selected.first().cloned() else {
        return;
    };
    let selector = world.resource::<SceneDoc>().selector(&primary);
    let editable = editable_selection(world);
    if editable.is_empty() {
        return;
    }
    let ops: Vec<EditOp> = editable
        .iter()
        .map(|node| EditOp::Remove {
            node: world.resource::<SceneDoc>().selector(node),
        })
        .collect();
    world.resource_mut::<PendingEdits>().0.push(Edit {
        label: delete_label(lang, &selector),
        ops,
        select_after: Some(None),
        select_inserted: false,
        select_name: None,
    });
}

/// Ctrl+C / palette: one deep XML per selected root, in ascending order.
pub fn copy_selection(world: &mut World) {
    let mut ascending = editable_selection(world);
    ascending.reverse();
    let xmls: Vec<String> = ascending
        .iter()
        .filter_map(|node| world.resource::<SceneDoc>().node(node))
        .map(viber::xml::emit::emit_node)
        .collect();
    if !xmls.is_empty() {
        world.insert_resource(NodeClipboard(xmls));
    }
}

/// Ctrl+V / palette: pastes after the primary (or at the world root).
pub fn paste_clipboard(world: &mut World) {
    let lang = *world.resource::<Lang>();
    let anchor = world.resource::<Selection>().primary().cloned();
    let Some(clipboard) = world
        .get_resource::<NodeClipboard>()
        .filter(|clipboard| !clipboard.0.is_empty())
        .cloned()
    else {
        return;
    };
    let ops = paste_ops(world.resource::<SceneDoc>(), anchor.as_ref(), &clipboard.0);
    if ops.is_empty() {
        return;
    }
    let label = anchor
        .as_ref()
        .map(|node| {
            format!(
                "{} {}",
                paste_str(lang),
                world.resource::<SceneDoc>().selector(node)
            )
        })
        .unwrap_or_else(|| paste_str(lang).to_owned());
    world
        .resource_mut::<PendingEdits>()
        .push_selecting_inserted(label, ops);
}

/// F2 / palette: starts the inline rename of the primary.
pub fn begin_rename(world: &mut World) {
    let Some(primary) = world.resource::<Selection>().primary().cloned() else {
        return;
    };
    let mut rename = world.resource_mut::<tree::RenameState>();
    rename.0 = Some(primary);
    rename.1 = false;
}

/// Autosave (Fase A5): every 2 minutes with dirty files, saves silently.
pub fn autosave(world: &mut World) {
    world.get_resource_or_insert_with::<AutosaveTimer>(|| AutosaveTimer(0.0));
    let delta = world.resource::<Time<Real>>().delta_secs();
    world.resource_mut::<AutosaveTimer>().0 += delta;
    if world.resource::<AutosaveTimer>().0 < 120.0 {
        return;
    }
    world.resource_mut::<AutosaveTimer>().0 = 0.0;
    let dirty = world.resource::<SceneDoc>().dirty_paths();
    if dirty.is_empty() {
        return;
    }
    save_world(world);
    info!("autosave: {} file(s)", dirty.len());
}

/// O cronómetro do autosave (segundos desde o último).
#[derive(Resource, Debug, Default)]
pub struct AutosaveTimer(pub f32);

/// Writes every edited file and marks the history's save point.
pub fn try_save_world(world: &mut World) -> anyhow::Result<Vec<String>> {
    let files = live::save(world)?;
    if let Some(mut history) = world.get_resource_mut::<viber::scene::ops::History>() {
        history.mark_saved();
    }
    Ok(files)
}

/// Writes every edited file (Ctrl+S, the Save button, and before ▶ Play).
pub fn save_world(world: &mut World) {
    let lang = *world.resource::<Lang>();
    let status = match try_save_world(world) {
        Ok(files) if files.is_empty() => save_nothing(lang).to_owned(),
        Ok(files) => {
            let status = saved(lang, &files);
            crate::ui::toast::push(world, crate::ui::toast::ToastKind::Success, status.clone());
            status
        }
        Err(error) => {
            let status = save_failed(lang, &error);
            crate::ui::toast::push(world, crate::ui::toast::ToastKind::Error, status.clone());
            status
        }
    };
    info!("{status}");
    world.resource_mut::<EditStatus>().0 = status;
}

/// Undo through the toolbar/palette (same status text as Ctrl+Z).
pub fn undo_last(world: &mut World) {
    let lang = *world.resource::<Lang>();
    let result = live::undo(world).map(|tx| tx.map(|tx| undo_prefix(lang, &tx.label)));
    report(world, result, nothing_to_undo(lang));
}

/// Redo through the toolbar/palette (same status text as Ctrl+Shift+Z).
pub fn redo_last(world: &mut World) {
    let lang = *world.resource::<Lang>();
    let result = live::redo(world).map(|tx| tx.map(|tx| redo_prefix(lang, &tx.label)));
    report(world, result, nothing_to_redo(lang));
}

fn report(world: &mut World, result: anyhow::Result<Option<String>>, empty: &str) {
    let status = match result {
        Ok(Some(label)) => label,
        Ok(None) => empty.to_owned(),
        Err(error) => {
            let status = failed(*world.resource::<Lang>(), &error);
            crate::ui::toast::push(world, crate::ui::toast::ToastKind::Error, status.clone());
            status
        }
    };
    world.resource_mut::<EditStatus>().0 = status;
}

/// Applies the queued edits through the engine's live editing API (history + live sync).
pub fn apply_pending_edits(world: &mut World) {
    let edits = std::mem::take(&mut world.resource_mut::<PendingEdits>().0);
    for edit in edits {
        let status = match live::apply(world, &edit.label, &edit.ops) {
            Ok(tx) => {
                if let Some(selection) = edit.select_after {
                    world.resource_mut::<Selection>().0 = selection.into_iter().collect();
                }
                if let Some(name) = edit.select_name.clone() {
                    let found = world.resource::<SceneDoc>().find_by_attr("name", &name);
                    if let Some(node) = found.into_iter().next() {
                        world.resource_scope(|world, mut expansion: Mut<TreeExpansion>| {
                            reveal(world.resource::<SceneDoc>(), &node, &mut expansion);
                        });
                        world.resource_mut::<Selection>().set_one(node);
                    }
                }
                let inserted = tx.changes.iter().find_map(|change| match change {
                    viber::scene::ops::Change::Inserted { node } => Some(node.clone()),
                    _ => None,
                });
                if edit.select_inserted
                    && let Some(node) = inserted
                {
                    world.resource_scope(|world, mut expansion: Mut<TreeExpansion>| {
                        reveal(world.resource::<SceneDoc>(), &node, &mut expansion);
                    });
                    world.resource_mut::<Selection>().set_one(node);
                }
                edit.label
            }
            Err(error) => {
                warn!("{}: {error:#}", edit.label);
                edit_failed(*world.resource::<Lang>(), &edit.label, &error)
            }
        };
        world.resource_mut::<EditStatus>().0 = status;
    }
}

/// Drops a selection whose node no longer exists (deleted, or undone insertion).
fn validate_selection(doc: Res<SceneDoc>, mut selection: ResMut<Selection>) {
    if !doc.is_changed() {
        return;
    }
    let before = selection.0.len();
    selection
        .0
        .retain(|node| node.is_root() || doc.node(node).is_some());
    if selection.0.len() != before {
        // mudou (dispara re-render dos docks)
    }
}

/// Expands every ancestor of `node` in the Scene tree (including the `<Include>` chain), so a
/// node picked in the viewport is visible in the dock.
pub fn reveal(doc: &SceneDoc, node: &NodeRef, expansion: &mut TreeExpansion) {
    let mut current = node.clone();
    for _ in 0..256 {
        match current.parent() {
            Some(parent) if !parent.is_root() => {
                expansion.0.insert(parent.clone());
                current = parent;
            }
            _ => {
                let includer = (0..doc.files.len()).find_map(|file| {
                    doc.files[file]
                        .includes
                        .iter()
                        .find(|(_, target)| *target == current.file)
                        .map(|(path, _)| NodeRef::new(file, path.clone()))
                });
                let Some(include) = includer else {
                    return;
                };
                expansion.0.insert(include.clone());
                current = include;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(file: usize, path: &[usize]) -> NodeRef {
        NodeRef::new(file, path.to_vec())
    }

    #[test]
    fn selection_roots_drop_descendants() {
        let parent = node(0, &[1]);
        let child = node(0, &[1, 0]);
        let sibling = node(0, &[2]);
        let roots = selection_roots(&[child.clone(), parent.clone(), sibling.clone()]);
        assert_eq!(roots, vec![parent, sibling]);
    }

    #[test]
    fn duplicate_copy_index_counts_earlier_siblings() {
        let a = node(0, &[0]);
        let c = node(0, &[2]);
        assert_eq!(earlier_sibling_copies(&[a, c.clone()], &c), 1);
        assert_eq!(duplicate_copy_index(2, 1), 4);
        assert_eq!(duplicate_copy_index(2, 0), 3);
    }

    #[test]
    fn paste_on_empty_selection_appends_to_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("world.xml");
        std::fs::write(
            &world,
            r#"<world static="true"><Entity name="a" /></world>"#,
        )
        .unwrap();
        let doc = SceneDoc::load(&world).unwrap();
        let (parent, index) = paste_anchor(&doc, None);
        assert!(parent.is_root());
        assert_eq!(index, 1);
        let ops = paste_ops(&doc, None, &[r#"<Entity name="b" />"#.into()]);
        assert!(matches!(&ops[0], EditOp::Insert { index: Some(1), .. }));
    }
}
