//! The Objects dock: the world's node tree, Godot-style. Rows show each node's own name
//! (`name`/`id`, the file it includes, else the friendly tag name — the icon carries the
//! type). `<Include>` nodes expand into the included file's nodes (sub-scenes); rows are
//! rebuilt only when the document, the expansion, the selection or the language changes.

use bevy::feathers::theme::ThemeBackgroundColor;
use bevy::feathers::tokens;
use bevy::input_focus::InputFocus;
use bevy::picking::events::PointerClick;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::doc::{NodeRef, SceneDoc};
use super::tree_drag::{DropZone, TreeDropHint};
use super::{Selection, TreeExpansion};
use crate::ui::icons::{self, fa};
use crate::ui::strings::{Lang, tag_display};
use crate::ui::theme;
use viber::xml::XmlNode;

/// Horizontal indent per depth level, in pixels.
const INDENT_PX: f32 = 14.0;
/// Rows beyond this many are cut (huge baked groups); expand a narrower branch instead.
const MAX_ROWS: usize = 2000;

/// The scrolling container the rows are spawned into.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SceneTreeBody;

/// One row of the tree.
#[derive(Component, Debug, Clone)]
pub struct TreeRow(pub NodeRef);

/// The disclosure chevron of a row.
#[derive(Component, Debug, Clone)]
pub struct TreeToggle(pub NodeRef);

/// Whether the rows exist yet (after that, Bevy change detection drives rebuilds).
#[derive(Resource, Debug, Default)]
pub struct TreeView {
    built: bool,
    /// Filtro de pesquisa (vazio = mostra tudo; casos em que coincide
    /// etiqueta/tag do nó, sem distinguir maiúsculas).
    pub filter: String,
    /// Sobe a cada mudança de filtro (um String não tem change-detection).
    filter_rev: u32,
    last_filter_rev: u32,
}

/// One flattened, visible row.
#[derive(Debug, Clone, PartialEq)]
pub struct RowSpec {
    pub node: NodeRef,
    pub depth: usize,
    /// The XML tag (drives the row's icon).
    pub tag: String,
    pub label: String,
    pub expandable: bool,
    pub expanded: bool,
    /// The row comes from an included file (shown dimmer, like Godot's instanced scenes).
    pub included: bool,
}

/// The row's label: the node's own name when it has one (the icon carries the type,
/// Godot-style), a humanized name for XML includes (the file icon already marks them as
/// files), the plain file name for models, else the friendly tag name.
fn friendly_node_label(lang: Lang, node: &XmlNode) -> String {
    for key in ["name", "id"] {
        if let Some(value) = node.attr(key) {
            return value.to_string();
        }
    }
    for key in ["src", "url", "model-url", "gltf"] {
        if let Some(value) = node.attr(key) {
            return file_label(value);
        }
    }
    tag_display(lang, &node.tag).to_string()
}

/// "room.xml" → "Room", "ground-wear.xml" → "Ground wear": XML files show their
/// humanized stem (separators become spaces, first letter up), without the extension.
/// Anything else — a GLB model, a URL — keeps its plain file name.
fn file_label(value: &str) -> String {
    let name = std::path::Path::new(value)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| value.to_string());
    if !name.to_ascii_lowercase().ends_with(".xml") {
        return name;
    }
    let stem = &name[..name.len() - ".xml".len()];
    let mut humanized = String::with_capacity(stem.len());
    for (index, ch) in stem.chars().enumerate() {
        match ch {
            '-' | '_' => humanized.push(' '),
            _ if index == 0 => humanized.extend(ch.to_uppercase()),
            _ => humanized.push(ch),
        }
    }
    let humanized = humanized.trim();
    if humanized.is_empty() {
        name.clone()
    } else {
        humanized.to_string()
    }
}

/// Flattens the visible part of the tree.
pub fn visible_rows(
    doc: &SceneDoc,
    expansion: &TreeExpansion,
    lang: Lang,
    filter: &str,
) -> Vec<RowSpec> {
    let mut rows = Vec::new();
    let mut walker = TreeWalker {
        doc,
        expansion,
        lang,
        rows: &mut rows,
        filter_mode: false,
    };
    if filter.trim().is_empty() {
        walker.push_children(0, &[], 0);
    } else {
        // com filtro: varre TUDO — inclusive ramos colapsados — e fica só o
        // que casa; depth 0, porque o depth alimenta o indent do row e um
        // sentinel gigante empurraria o conteúdo para fora do painel
        // `to_ascii_lowercase` não dobra acentos: «Historico» não casava com
        // «histórico» nem «PÁTIO» com «pátio» (os nomes do mundo são PT).
        let needle = filter.trim().to_lowercase();
        let mut all = Vec::new();
        let mut filtered = TreeWalker {
            doc,
            expansion,
            lang,
            rows: &mut all,
            filter_mode: true,
        };
        filtered.push_children(0, &[], 0);
        rows = all
            .into_iter()
            .filter(|row| {
                row.label.to_lowercase().contains(&needle)
                    || row.tag.to_lowercase().contains(&needle)
            })
            .collect();
    }
    rows.truncate(MAX_ROWS);
    rows
}

/// The recursion state of a tree flatten (the walker keeps `push_children`
/// at 3 arguments).
struct TreeWalker<'a> {
    doc: &'a SceneDoc,
    expansion: &'a TreeExpansion,
    lang: Lang,
    rows: &'a mut Vec<RowSpec>,
    /// Modo pesquisa: desce em TODOS os ramos, expandidos ou não.
    filter_mode: bool,
}

impl TreeWalker<'_> {
    fn push_children(&mut self, file: usize, parent: &[usize], depth: usize) {
        let doc = self.doc;
        let children = if parent.is_empty() {
            &doc.files[file].doc.children
        } else {
            match doc.node(&NodeRef {
                file,
                path: parent.to_vec(),
            }) {
                Some(node) => &node.children,
                None => return,
            }
        };
        for (index, child) in children.iter().enumerate() {
            if self.rows.len() >= MAX_ROWS {
                return;
            }
            let mut path = parent.to_vec();
            path.push(index);
            let node = NodeRef { file, path };
            let include = doc.include_target(&node);
            let expandable = !child.children.is_empty() || include.is_some();
            let expanded = expandable && self.expansion.0.contains(&node);
            self.rows.push(RowSpec {
                node: node.clone(),
                depth,
                tag: child.tag.clone(),
                label: friendly_node_label(self.lang, child),
                expandable,
                expanded,
                included: file != 0,
            });
            if self.filter_mode || expanded {
                match include {
                    Some(target) => self.push_children(target, &[], depth + 1),
                    None => self.push_children(file, &node.path, depth + 1),
                }
            }
        }
    }
}

/// Rebuilds the rows when the document, the expansion or the selection changed.
/// O input de renomeação inline.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct TreeRenameInput;

/// Nome actual a semear no `EditableText` (o widget nasce vazio).
#[derive(Component, Debug, Clone)]
pub(crate) struct RenameSeed(String);

/// Põe o nome actual no input de F2 no frame em que aparece — e dá-lhe o
/// FOCO: sem isto o F2 abria o campo mas as teclas continuavam a ir para o
/// viewport (era preciso clicar no input para escrever o nome).
pub fn prefill_rename(
    mut seeds: Query<(Entity, &RenameSeed, &mut bevy::text::EditableText)>,
    mut focus: Option<ResMut<bevy::input_focus::InputFocus>>,
    mut commands: Commands,
) {
    for (entity, seed, mut text) in &mut seeds {
        text.editor_mut().set_text(&seed.0);
        text.queue_edit(bevy::text::TextEdit::SelectAll);
        if let Some(focus) = focus.as_mut() {
            focus.set(entity, bevy::input_focus::FocusCause::Navigated);
        }
        commands.entity(entity).remove::<RenameSeed>();
    }
}

/// O nó em renomeação inline (F2 / menu de contexto). `.1` = o foco já esteve
/// no input (arma o commit-on-blur; evita commitar no frame de spawn).
#[derive(Resource, Debug, Default, Clone)]
pub struct RenameState(pub Option<NodeRef>, pub bool);

/// A caixa de pesquisa do dock Objects.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct TreeSearchInput;

/// Marks the Objects dock (keyboard navigation runs while the pointer is over it).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ObjectsDock;

/// Header actions: expand/collapse every branch.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ExpandAllButton;
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct CollapseAllButton;

/// The filter updates as you type (Enter/Esc still apply/clear it).
pub fn live_tree_filter(world: &mut World) {
    let Some(focused) = world.get_resource::<InputFocus>().and_then(InputFocus::get) else {
        return;
    };
    let mut input = focused;
    let is_search = loop {
        if world.get::<TreeSearchInput>(input).is_some() {
            break true;
        }
        match world.get::<ChildOf>(input).map(|child| child.parent()) {
            Some(parent) => input = parent,
            None => break false,
        }
    };
    if !is_search {
        return;
    }
    let Some(text) = crate::scene::input_text(world, input) else {
        return;
    };
    let wanted = text.trim().to_owned();
    let mut view = world.resource_mut::<TreeView>();
    if view.filter != wanted {
        view.filter = wanted;
        view.filter_rev += 1;
    }
}

/// Arrow-key navigation over the visible rows, active while the pointer is
/// over the Objects dock (so it never steals keys from the viewport/text).
#[allow(clippy::too_many_arguments)]
pub fn tree_keys(
    keys: Res<ButtonInput<KeyCode>>,
    focus: Option<Res<InputFocus>>,
    editable: Query<(), With<bevy::text::EditableText>>,
    hover: Res<bevy::picking::hover::HoverMap>,
    docks: Query<Entity, With<ObjectsDock>>,
    parents: Query<&ChildOf>,
    doc: Res<SceneDoc>,
    view: Res<TreeView>,
    mut expansion: ResMut<TreeExpansion>,
    mut selection: ResMut<Selection>,
    lang: Res<Lang>,
    add: Res<crate::scene::add_node::AddNodeOpen>,
    menu: Res<crate::ui::context_menu::ContextMenu>,
) {
    if add.0 || menu.open {
        return;
    }
    if focus
        .as_ref()
        .and_then(|focus| focus.get())
        .is_some_and(|entity| editable.contains(entity))
    {
        return;
    }
    let over_dock = docks.iter().any(|dock| {
        hover.values().flat_map(|hits| hits.keys()).any(|&entity| {
            entity == dock
                || parents
                    .iter_ancestors(entity)
                    .any(|ancestor| ancestor == dock)
        })
    });
    if !over_dock {
        return;
    }
    let rows = visible_rows(&doc, &expansion, *lang, &view.filter);
    if rows.is_empty() {
        return;
    }
    let current = selection
        .primary()
        .and_then(|primary| rows.iter().position(|row| &row.node == primary));
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let mut target = current;
    let mut handled = true;
    if keys.just_pressed(KeyCode::ArrowDown) {
        target = Some(current.map_or(0, |index| (index + 1).min(rows.len() - 1)));
    } else if keys.just_pressed(KeyCode::ArrowUp) {
        target = Some(current.map_or(rows.len() - 1, |index| index.saturating_sub(1)));
    } else if keys.just_pressed(KeyCode::Home) {
        target = Some(0);
    } else if keys.just_pressed(KeyCode::End) {
        target = Some(rows.len() - 1);
    } else if keys.just_pressed(KeyCode::ArrowLeft) {
        if let Some(index) = current {
            let row = &rows[index];
            if row.expanded {
                expansion.0.remove(&row.node);
            } else if let Some(parent) = row.node.parent().filter(|parent| !parent.is_root()) {
                target = rows.iter().position(|candidate| candidate.node == parent);
            }
        }
    } else if keys.just_pressed(KeyCode::ArrowRight) {
        if let Some(index) = current {
            let row = &rows[index];
            if row.expandable && !row.expanded {
                expansion.0.insert(row.node.clone());
            } else if row.expanded
                && rows
                    .get(index + 1)
                    .is_some_and(|child| child.depth > row.depth)
            {
                target = Some(index + 1);
            }
        }
    } else {
        handled = false;
    }
    if handled && let Some(index) = target {
        let node = rows[index].node.clone();
        if shift {
            selection.extend(&node);
        } else {
            selection.set_one(node);
        }
    }
}

pub fn expand_all_clicked(
    activate: On<Activate>,
    buttons: Query<(), With<ExpandAllButton>>,
    doc: Res<SceneDoc>,
    mut expansion: ResMut<TreeExpansion>,
) {
    if buttons.contains(activate.event_target()) {
        expansion.expand_all(&doc);
    }
}

pub fn collapse_all_clicked(
    activate: On<Activate>,
    buttons: Query<(), With<CollapseAllButton>>,
    mut expansion: ResMut<TreeExpansion>,
) {
    if buttons.contains(activate.event_target()) {
        expansion.collapse_all();
    }
}

/// Every node that has children (or an include), depth-first.
pub fn all_expandable(doc: &SceneDoc) -> Vec<NodeRef> {
    let mut out = Vec::new();
    let mut visited = std::collections::HashSet::new();
    visited.insert(0);
    collect_expandable(doc, 0, &[], &mut out, &mut visited);
    out.truncate(MAX_ROWS);
    out
}

fn collect_expandable(
    doc: &SceneDoc,
    file: usize,
    parent: &[usize],
    out: &mut Vec<NodeRef>,
    visited: &mut std::collections::HashSet<usize>,
) {
    let children = if parent.is_empty() {
        &doc.files[file].doc.children
    } else {
        match doc.node(&NodeRef {
            file,
            path: parent.to_vec(),
        }) {
            Some(node) => &node.children,
            None => return,
        }
    };
    for (index, child) in children.iter().enumerate() {
        if out.len() >= MAX_ROWS {
            return;
        }
        let mut path = parent.to_vec();
        path.push(index);
        let node = NodeRef {
            file,
            path: path.clone(),
        };
        let include = doc.include_target(&node);
        if !child.children.is_empty() || include.is_some() {
            out.push(node.clone());
        }
        match include {
            Some(target) => {
                if visited.insert(target) {
                    collect_expandable(doc, target, &[], out, visited);
                }
            }
            None => collect_expandable(doc, file, &path, out, visited),
        }
    }
}

/// Enter na pesquisa aplica o filtro; Esc limpa.
pub fn apply_tree_search(world: &mut World) {
    let Some(focused) = world
        .get_resource::<bevy::input_focus::InputFocus>()
        .and_then(|focus| focus.get())
    else {
        return;
    };
    let mut input = focused;
    let is_search = loop {
        if world.get::<TreeSearchInput>(input).is_some() {
            break true;
        }
        match world.get::<ChildOf>(input).map(|child| child.parent()) {
            Some(parent) => input = parent,
            None => break false,
        }
    };
    if !is_search {
        return;
    }
    let (enter, escape) = {
        let keys = world.resource::<ButtonInput<KeyCode>>();
        (
            keys.just_pressed(KeyCode::Enter),
            keys.just_pressed(KeyCode::Escape),
        )
    };
    let Some(text) = crate::scene::input_text(world, input) else {
        return;
    };
    if enter {
        let mut view = world.resource_mut::<TreeView>();
        view.filter = text.trim().to_owned();
        view.filter_rev += 1;
    } else if escape {
        world.resource_mut::<TreeView>().filter.clear();
        world.resource_mut::<TreeView>().filter_rev += 1;
        // Limpa também o CAMPO: só o filtro ficava vazio e a caixa continuava
        // a mostrar a pesquisa antiga (que voltava a filtrar no próximo foco).
        if let Some(mut text) = world.get_mut::<bevy::text::EditableText>(input) {
            text.clear();
        }
        if let Some(mut focus) = world.get_resource_mut::<bevy::input_focus::InputFocus>() {
            focus.clear();
        }
    }
}

/// Enter no input de renomeação escreve o `name` (uma op `set`); clicar fora
/// COMMITA (estilo Godot) — o modo não encrava e o texto digitado não se perde.
pub fn commit_rename(world: &mut World) {
    let Some(node) = world.resource::<RenameState>().0.clone() else {
        return;
    };
    let keys = world.resource::<ButtonInput<KeyCode>>();
    let enter = keys.just_pressed(KeyCode::Enter);
    let escape = keys.just_pressed(KeyCode::Escape);
    let focused = world
        .get_resource::<bevy::input_focus::InputFocus>()
        .and_then(|focus| focus.get());
    let on_input = focused.is_some_and(|entity| world.get::<TreeRenameInput>(entity).is_some());
    let (was_focused, armed) = {
        let rename = world.resource::<RenameState>();
        (on_input, rename.1)
    };
    if on_input {
        world
            .resource_mut::<RenameState>()
            .bypass_change_detection()
            .1 = true;
    }
    // blur (o foco foi para outro lado): commita o que lá estiver — o 1.º
    // frame com o foco NO input arma o blur-commit (evita commitar no frame
    // de spawn, antes de o foco chegar)
    let blur_commit = !was_focused && armed && !enter && !escape;
    if !blur_commit && !enter && !escape {
        return;
    }
    {
        let mut rename = world.resource_mut::<RenameState>();
        rename.0 = None;
        rename.1 = false;
    }
    if !blur_commit
        && let Some(mut focus) = world.get_resource_mut::<bevy::input_focus::InputFocus>()
    {
        focus.clear();
    }
    if !blur_commit && !enter {
        return;
    }
    apply_rename(world, node);
}

/// Escreve o texto corrente do input como `name` (ou `id` em tags UI — a
/// runtime de UI ignora `name`; era o F2 a criar atributos mortos).
fn apply_rename(world: &mut World, node: NodeRef) {
    let mut inputs = world.query_filtered::<&bevy::text::EditableText, With<TreeRenameInput>>();
    let Some(text) = inputs.iter(world).next() else {
        return;
    };
    let value = text.value().to_string().trim().to_owned();
    let doc = world.resource::<SceneDoc>().clone();
    let Some(xml) = doc.node(&node) else {
        return;
    };
    let key = if viber::ui::is_ui_tag(&xml.tag) {
        "id"
    } else {
        "name"
    };
    if value.is_empty() || value == xml.attr(key).unwrap_or_default() {
        return;
    }
    let selector = doc.selector(&node);
    let lang = *world.resource::<crate::ui::strings::Lang>();
    world.resource_mut::<crate::scene::PendingEdits>().push(
        format!(
            "{} {selector}",
            crate::ui::strings::pick(lang, "renomear", "rename")
        ),
        vec![viber::scene::ops::EditOp::Set {
            node: selector,
            attrs: [(key.to_owned(), Some(value))].into_iter().collect(),
        }],
    );
}

#[allow(clippy::too_many_arguments)]
pub fn render_tree(
    mut commands: Commands,
    rename: Res<RenameState>,
    doc: Res<SceneDoc>,
    expansion: Res<TreeExpansion>,
    selection: Res<Selection>,
    lang: Res<Lang>,
    hint: Res<TreeDropHint>,
    mut view: ResMut<TreeView>,
    bodies: Query<Entity, With<SceneTreeBody>>,
) {
    let Ok(body) = bodies.single() else {
        return;
    };
    let stale = !view.built
        || doc.is_changed()
        || expansion.is_changed()
        || selection.is_changed()
        || lang.is_changed()
        || view.filter_rev != view.last_filter_rev
        || rename.is_changed()
        || hint.is_changed();
    view.last_filter_rev = view.filter_rev;
    if !stale {
        return;
    }
    view.built = true;

    commands.entity(body).despawn_related::<Children>();
    let mut drag_rows: Vec<(Entity, NodeRef)> = Vec::new();
    for row in visible_rows(&doc, &expansion, *lang, &view.filter) {
        let selected = selection.0.contains(&row.node);
        let chevron = match (row.expandable, row.expanded) {
            (false, _) => ' ',
            (true, false) => fa::CHEVRON_RIGHT,
            (true, true) => fa::CHEVRON_DOWN,
        };
        let text_color = if row.included {
            Color::srgb(0.62, 0.78, 0.95)
        } else {
            Color::srgb(0.88, 0.88, 0.9)
        };
        let icon_color = if selected {
            Color::WHITE
        } else {
            icons::tag_color(&row.tag)
        };
        let mut row_node = Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(4),
            padding: UiRect::new(px(4.0 + row.depth as f32 * INDENT_PX), px(4), px(1), px(1)),
            ..default()
        };
        let mut hint_border = None;
        if hint.node.as_ref() == Some(&row.node) {
            match hint.zone {
                DropZone::Before => {
                    row_node.border = UiRect::top(px(2));
                    hint_border = Some(theme::ACCENT);
                }
                DropZone::After => {
                    row_node.border = UiRect::bottom(px(2));
                    hint_border = Some(theme::ACCENT);
                }
                DropZone::Into => {
                    row_node.border = UiRect::all(px(1));
                    hint_border = Some(theme::ACCENT);
                }
                DropZone::None => {}
            }
        }
        let mut row_entity = commands.spawn((TreeRow(row.node.clone()), row_node, ChildOf(body)));
        if selected {
            row_entity.insert(ThemeBackgroundColor(tokens::BUTTON_PRIMARY_BG));
        }
        if let Some(color) = hint_border {
            row_entity.insert(BorderColor::all(color));
        }
        drag_rows.push((row_entity.id(), row.node.clone()));
        row_entity
            .observe(select_row)
            .with_children(|row_children| {
                row_children
                    .spawn((
                        TreeToggle(row.node.clone()),
                        // sem filhos: texto vazio (um espaço na fonte de ícones
                        // desenha o glifo "em falta" ⊠)
                        if chevron == ' ' {
                            (
                                Text::default(),
                                TextFont::default(),
                                TextColor(icons::ICON_DIM),
                            )
                        } else {
                            icons::icon_text(chevron, 9.0, icons::ICON_DIM)
                        },
                        Node {
                            width: px(10),
                            ..default()
                        },
                    ))
                    .observe(toggle_row);
                row_children.spawn((
                    icons::icon_text(icons::tag_icon(&row.tag), 11.0, icon_color),
                    Node {
                        width: px(14),
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                if rename.0.as_ref() == Some(&row.node) {
                    // renomeação inline (F2): input no sítio do label. A
                    // chave tem de ser a MESMA do `apply_rename` (`id` nas
                    // tags de UI): ler sempre `name` abria o campo vazio e
                    // gravava o `name` por cima do `id`.
                    let xml = doc.node(&row.node);
                    let key = if xml.is_some_and(|node| viber::ui::is_ui_tag(&node.tag)) {
                        "id"
                    } else {
                        "name"
                    };
                    let current = xml
                        .and_then(|node| node.attr(key))
                        .unwrap_or_default()
                        .to_owned();
                    row_children
                        .spawn((
                            bevy::feathers::controls::FeathersTextInputContainer,
                            Node {
                                flex_grow: 1.0,
                                ..default()
                            },
                        ))
                        .with_children(|input| {
                            input.spawn((
                                bevy::feathers::controls::FeathersTextInput,
                                TreeRenameInput,
                                RenameSeed(current),
                            ));
                        });
                } else {
                    row_children.spawn((
                        Text::new(row.label.clone()),
                        TextFont::from_font_size(12.0),
                        TextColor(text_color),
                        Pickable::IGNORE,
                    ));
                }
            });
    }
    for (entity, node) in drag_rows {
        super::tree_drag::attach(&mut commands, entity, &node);
    }
}

fn select_row(
    click: On<PointerClick>,
    rows: Query<&TreeRow>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    mut selection: ResMut<Selection>,
    mut commands: Commands,
) {
    let Ok(row) = rows.get(click.event_target()) else {
        return;
    };
    // botão direito: menu de contexto (seleciona e abre)
    if click.button == PointerButton::Secondary
        && let Some(at) = windows.single().ok().and_then(Window::cursor_position)
    {
        let node = row.0.clone();
        if !selection.0.contains(&node) {
            selection.set_one(node.clone());
        }
        commands.queue(move |world: &mut World| {
            crate::ui::context_menu::open_for(world, Some(node.clone()), at);
        });
        return;
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if ctrl {
        selection.toggle(&row.0);
    } else if shift {
        selection.extend(&row.0);
    } else {
        selection.set_one(row.0.clone());
    }
}

fn toggle_row(
    mut click: On<PointerClick>,
    toggles: Query<&TreeToggle>,
    mut expansion: ResMut<TreeExpansion>,
) {
    if let Ok(toggle) = toggles.get(click.event_target()) {
        // The row underneath must not also select.
        click.propagate(false);
        if !expansion.0.remove(&toggle.0) {
            expansion.0.insert(toggle.0.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn doc(dir: &Path) -> SceneDoc {
        std::fs::write(
            dir.join("world.xml"),
            r#"<world>
  <Group name="props"><Entity name="crate" /><Entity name="barrel" /></Group>
  <Include src="part.xml" />
  <Sky />
</world>"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("part.xml"),
            r#"<world><Entity name="well" /></world>"#,
        )
        .unwrap();
        SceneDoc::load(&dir.join("world.xml")).unwrap()
    }

    #[test]
    fn collapsed_tree_shows_top_level_only() {
        let tmp = tempfile::tempdir().unwrap();
        let rows = visible_rows(&doc(tmp.path()), &TreeExpansion::default(), Lang::Pt, "");
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, ["props", "Part", "Céu"], "friendly PT labels");
        assert!(rows[0].expandable && rows[1].expandable && !rows[2].expandable);
    }

    #[test]
    fn expanding_groups_and_includes_nests_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = doc(tmp.path());
        let mut expansion = TreeExpansion::default();
        expansion.0.insert(NodeRef {
            file: 0,
            path: vec![0],
        });
        expansion.0.insert(NodeRef {
            file: 0,
            path: vec![1],
        });
        let rows = visible_rows(&doc, &expansion, Lang::Pt, "");
        let flat: Vec<_> = rows
            .iter()
            .map(|row| (row.depth, row.label.as_str(), row.included))
            .collect();
        assert_eq!(
            flat,
            [
                (0, "props", false),
                (1, "crate", false),
                (1, "barrel", false),
                (0, "Part", false),
                (1, "well", true),
                (0, "Céu", false),
            ]
        );
        assert_eq!(
            rows[4].node,
            NodeRef {
                file: 1,
                path: vec![0]
            }
        );
    }

    #[test]
    fn xml_includes_show_humanized_names_without_extension() {
        let mut node = XmlNode::new("Include");
        node.set_attr("src", "/world/paths/ground-wear.XML");
        assert_eq!(friendly_node_label(Lang::Pt, &node), "Ground wear");
        node.set_attr("src", "/world/landmarks/town.xml");
        assert_eq!(friendly_node_label(Lang::En, &node), "Town");
        // Ficheiros não-XML mantêm o nome (com extensão).
        let mut model = XmlNode::new("GltfScene");
        model.set_attr("model-url", "/assets/house.glb");
        assert_eq!(friendly_node_label(Lang::Pt, &model), "house.glb");
    }

    #[test]
    fn expand_all_covers_groups_and_includes_but_not_leaves() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = doc(tmp.path());
        let expanded = all_expandable(&doc);
        assert!(expanded.contains(&NodeRef {
            file: 0,
            path: vec![0]
        }));
        assert!(expanded.contains(&NodeRef {
            file: 0,
            path: vec![1]
        }));
        assert!(!expanded.contains(&NodeRef {
            file: 0,
            path: vec![2]
        }));
        let mut expansion = TreeExpansion::default();
        expansion.expand_all(&doc);
        let rows = visible_rows(&doc, &expansion, Lang::Pt, "");
        assert!(rows.iter().any(|row| row.label == "well" && row.included));
        expansion.collapse_all();
        assert_eq!(visible_rows(&doc, &expansion, Lang::Pt, "").len(), 3);
    }

    #[test]
    fn english_flips_friendly_tag_fallbacks() {
        let tmp = tempfile::tempdir().unwrap();
        let rows = visible_rows(&doc(tmp.path()), &TreeExpansion::default(), Lang::En, "");
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, ["props", "Part", "Sky"]);
    }

    #[test]
    fn the_filter_folds_accents() {
        // «CÉU» tem de casar com a etiqueta PT «Céu» (to_ascii_lowercase não
        // dobra o É).
        let tmp = tempfile::tempdir().unwrap();
        let rows = visible_rows(&doc(tmp.path()), &TreeExpansion::default(), Lang::Pt, "CÉU");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Céu");
    }
}
