//! Renderer da aba Plugins: sub-abas por painel e uma árvore declarativa
//! (`UiNode`) por painel. O renderer é deliberadamente pequeno: os widgets são
//! Bevy/Feathers normais, os eventos voltam ao host como `PluginUiEvent` e o
//! rebuild só acontece quando a árvore muda.

use bevy::feathers::controls::{
    ButtonVariant, FeathersCheckbox, FeathersNumberInput, FeathersSelect, FeathersTextInput,
    FeathersTextInputContainer, FeathersToolButton, NumberInputPrecision, NumberInputStep,
    NumberInputValue, OptionIndex, list_rows_from_strings,
};
use bevy::feathers::display::caption;
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::{EditableText, TextEdit};
use bevy::ui_widgets::{Activate, ValueChange};

use crate::plugins::{
    ActivePluginPanel, ListItem, PanelInfo, PluginHost, PluginUiEvent, PluginUiEvents, Tone, UiNode,
};
use crate::ui::strings::{Lang, StrKey, t};
use crate::ui::theme;

/// Root of the Plugins dock body (only spawned when the tab is active).
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PluginPanelRoot;

/// The row of panel sub-tabs.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PluginTabStrip;

/// The container the active panel tree is spawned into.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct PluginPanelBody;

/// Sub-tab button.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct PluginTabButton {
    plugin: String,
    panel: String,
}

#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct PluginButton {
    plugin: String,
    panel: String,
    id: String,
}

#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct PluginListRow {
    plugin: String,
    panel: String,
    id: String,
    value: String,
}

#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct PluginCheckboxControl {
    plugin: String,
    panel: String,
    id: String,
}

#[derive(Component, Debug, Clone)]
struct PluginNumberControl {
    plugin: String,
    panel: String,
    id: String,
}

#[derive(Component, Debug, Clone)]
struct PluginSelectControl {
    plugin: String,
    panel: String,
    id: String,
    options: Vec<(String, String)>,
}

#[derive(Component, Debug, Clone)]
struct PluginTextInput {
    plugin: String,
    panel: String,
    id: String,
    index: Option<usize>,
    /// Last value seen by the host, to emit `change` only on diffs.
    last: String,
    prefilled: bool,
}

#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct PluginLineAdd {
    plugin: String,
    panel: String,
    id: String,
}

#[derive(Component, Debug, Clone, PartialEq, Eq)]
struct PluginLineRemove {
    plugin: String,
    panel: String,
    id: String,
    index: usize,
}

#[derive(Resource, Default)]
struct PluginPanelState {
    tabs_signature: String,
    tabs_lang: Option<Lang>,
    tabs_active: Option<String>,
    panel_key: Option<String>,
    tree: Option<UiNode>,
}

pub struct PluginPanelPlugin;

impl Plugin for PluginPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PluginPanelState>()
            .add_systems(
                Update,
                (
                    render_plugin_tabs,
                    render_plugin_panel,
                    prefill_plugin_inputs,
                    poll_plugin_inputs,
                )
                    .chain(),
            )
            .add_observer(plugin_tab_clicked)
            .add_observer(plugin_button_clicked)
            .add_observer(plugin_list_clicked)
            .add_observer(plugin_checkbox_changed)
            .add_observer(plugin_number_changed)
            .add_observer(plugin_select_changed)
            .add_observer(plugin_line_added)
            .add_observer(plugin_line_removed);
    }
}

/// The dock body, as a BSN scene (called from `layout::dock_tabs`).
pub fn plugin_panel_body() -> impl Scene {
    bsn! {
        @theme::pane()
        Children [
            @theme::pane_body()
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                min_height: px(0),
            }
            PluginPanelRoot
            Children [
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: px(2),
                    flex_wrap: FlexWrap::Wrap,
                    flex_shrink: 0.0,
                }
                PluginTabStrip
                Children []
                --
                Node {
                    flex_grow: 1.0,
                    min_height: px(0),
                    overflow: Overflow::scroll_y(),
                    flex_direction: FlexDirection::Column,
                }
                PluginPanelBody
                Children []
            ]
        ]
    }
}

fn render_plugin_tabs(
    mut commands: Commands,
    host: Option<Res<PluginHost>>,
    active: Res<ActivePluginPanel>,
    lang: Res<Lang>,
    strips: Query<Entity, With<PluginTabStrip>>,
    mut state: ResMut<PluginPanelState>,
) {
    let Some(host) = host else {
        return;
    };
    let panels = host.panels();
    let signature: String = panels
        .iter()
        .map(PanelInfo::key)
        .collect::<Vec<_>>()
        .join("|");
    if state.tabs_signature == signature
        && state.tabs_lang == Some(*lang)
        && state.tabs_active == active.0
    {
        return;
    }
    state.tabs_signature = signature;
    state.tabs_lang = Some(*lang);
    state.tabs_active = active.0.clone();
    let Ok(strip) = strips.single() else {
        return;
    };
    commands.entity(strip).despawn_related::<Children>();
    if panels.is_empty() {
        commands
            .spawn_scene(bsn! { @caption(t(*lang, StrKey::PluginsEmpty)) })
            .insert((TextColor(theme::TEXT_DIM), Pickable::IGNORE, ChildOf(strip)));
        return;
    }
    for panel in panels {
        let selected = active.0.as_deref() == Some(panel.key().as_str());
        commands
            .spawn_scene(bsn! {
                @FeathersToolButton {
                    @variant: {if selected { ButtonVariant::Primary } else { ButtonVariant::Plain }},
                    @caption: bsn! { @caption(panel.title.clone()) },
                }
            })
            .insert((
                PluginTabButton {
                    plugin: panel.plugin.clone(),
                    panel: panel.id.clone(),
                },
                ChildOf(strip),
            ))
            .observe(plugin_tab_clicked);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_plugin_panel(
    mut commands: Commands,
    host: Option<Res<PluginHost>>,
    mut active: ResMut<ActivePluginPanel>,
    lang: Res<Lang>,
    focus: Option<Res<InputFocus>>,
    inputs: Query<(), With<PluginTextInput>>,
    bodies: Query<Entity, With<PluginPanelBody>>,
    mut state: ResMut<PluginPanelState>,
) {
    let Some(host) = host else {
        return;
    };
    let panels = host.panels();
    let key = active
        .0
        .clone()
        .filter(|key| panels.iter().any(|panel| panel.key() == *key))
        .or_else(|| panels.first().map(PanelInfo::key));
    if active.0 != key {
        active.0 = key.clone();
    }
    let tree = key
        .as_deref()
        .and_then(|key| host.panel_tree(key).cloned())
        .unwrap_or_else(|| empty_tree(&host, *lang));

    let focused_input = focus
        .as_ref()
        .and_then(|focus| focus.get())
        .is_some_and(|entity| inputs.contains(entity));
    if focused_input && state.panel_key == key {
        return;
    }
    if state.panel_key == key && state.tree.as_ref() == Some(&tree) {
        return;
    }
    let Ok(body) = bodies.single() else {
        return;
    };
    commands.entity(body).despawn_related::<Children>();
    let (plugin, panel) = key
        .as_deref()
        .and_then(|key| key.split_once(':'))
        .unwrap_or(("", ""));
    spawn_tree(&mut commands, body, &tree, plugin, panel);
    state.panel_key = key;
    state.tree = Some(tree);
}

fn empty_tree(host: &PluginHost, lang: Lang) -> UiNode {
    let mut children = Vec::new();
    if !host.errors().is_empty() {
        children.push(UiNode::Heading(t(lang, StrKey::PluginsBroken).to_owned()));
        let mut errors: Vec<_> = host.errors().iter().collect();
        errors.sort_by_key(|(name, _)| name.as_str());
        for (name, error) in errors {
            children.push(UiNode::Label {
                text: format!("{name}: {error}"),
                tone: Tone::Error,
            });
        }
        children.push(UiNode::Separator);
    }
    children.push(UiNode::Label {
        text: t(lang, StrKey::PluginsEmpty).to_owned(),
        tone: Tone::Dim,
    });
    children.push(UiNode::Label {
        text: host.plugin_dir().display().to_string(),
        tone: Tone::Dim,
    });
    UiNode::Column { gap: 4.0, children }
}

fn spawn_tree(commands: &mut Commands, parent: Entity, node: &UiNode, plugin: &str, panel: &str) {
    match node {
        UiNode::Column { gap, children } => {
            let container = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: px(*gap),
                        width: percent(100),
                        ..default()
                    },
                    ChildOf(parent),
                ))
                .id();
            for child in children {
                spawn_tree(commands, container, child, plugin, panel);
            }
        }
        UiNode::Row { gap, children } => {
            let container = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: px(*gap),
                        flex_wrap: FlexWrap::Wrap,
                        width: percent(100),
                        ..default()
                    },
                    ChildOf(parent),
                ))
                .id();
            for child in children {
                spawn_tree(commands, container, child, plugin, panel);
            }
        }
        UiNode::Heading(text) => {
            commands.spawn((
                Text::new(text.clone()),
                TextFont::from_font_size(13.0),
                TextColor(theme::TEXT),
                Node {
                    margin: UiRect::top(px(2)),
                    ..default()
                },
                Pickable::IGNORE,
                ChildOf(parent),
            ));
        }
        UiNode::Label { text, tone } => {
            commands.spawn((
                Text::new(text.clone()),
                TextFont::from_font_size(12.0),
                TextColor(tone_color(*tone)),
                Node {
                    flex_wrap: FlexWrap::Wrap,
                    ..default()
                },
                Pickable::IGNORE,
                ChildOf(parent),
            ));
        }
        UiNode::Button {
            id,
            text,
            primary,
            enabled,
            tip,
        } => {
            if !enabled {
                commands.spawn((
                    Text::new(text.clone()),
                    TextFont::from_font_size(12.0),
                    TextColor(theme::TEXT_DIM),
                    Pickable::IGNORE,
                    ChildOf(parent),
                ));
                return;
            }
            let mut entity = commands.spawn_scene(bsn! {
                @FeathersToolButton {
                    @variant: {if *primary { ButtonVariant::Primary } else { ButtonVariant::Plain }},
                    @caption: bsn! { @caption(text.clone()) },
                }
            });
            entity.insert((
                PluginButton {
                    plugin: plugin.to_owned(),
                    panel: panel.to_owned(),
                    id: id.clone(),
                },
                ChildOf(parent),
            ));
            let _ = tip;
            entity.observe(plugin_button_clicked);
        }
        UiNode::Text {
            id,
            label,
            value,
            placeholder,
        } => {
            let row = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: px(6),
                        width: percent(100),
                        ..default()
                    },
                    ChildOf(parent),
                ))
                .id();
            spawn_label(commands, row, label.as_deref());
            let container = commands
                .spawn_scene(bsn! {
                    @FeathersTextInputContainer
                    Node { flex_grow: 1.0, min_width: px(0) }
                })
                .insert(ChildOf(row))
                .id();
            let _ = placeholder;
            commands.spawn_scene(bsn! { @FeathersTextInput }).insert((
                Text::new(value.clone()),
                PluginTextInput {
                    plugin: plugin.to_owned(),
                    panel: panel.to_owned(),
                    id: id.clone(),
                    index: None,
                    last: value.clone(),
                    prefilled: false,
                },
                ChildOf(container),
            ));
        }
        UiNode::Number {
            id,
            label,
            value,
            step,
            min,
            max,
        } => {
            let row = text_row(commands, parent);
            spawn_label(commands, row, label.as_deref());
            let mut entity = commands.spawn_scene(bsn! {
                @FeathersNumberInput
                Node { flex_grow: 1.0 }
            });
            let mut value = *value as f32;
            if let Some(min) = min {
                value = value.max(*min as f32);
            }
            if let Some(max) = max {
                value = value.min(*max as f32);
            }
            entity.insert((
                NumberInputValue::F32(value),
                NumberInputPrecision(2),
                NumberInputStep(*step),
                PluginNumberControl {
                    plugin: plugin.to_owned(),
                    panel: panel.to_owned(),
                    id: id.clone(),
                },
                ChildOf(row),
            ));
            entity.observe(plugin_number_changed);
        }
        UiNode::Checkbox { id, label, value } => {
            let row = text_row(commands, parent);
            spawn_label(commands, row, Some(label));
            let mut entity = commands.spawn_scene(bsn! { @FeathersCheckbox });
            entity.insert((
                PluginCheckboxControl {
                    plugin: plugin.to_owned(),
                    panel: panel.to_owned(),
                    id: id.clone(),
                },
                ChildOf(row),
            ));
            if *value {
                entity.insert(bevy::ui::Checked);
            }
            entity.observe(plugin_checkbox_changed);
        }
        UiNode::Select {
            id,
            label,
            value,
            options,
        } => {
            let row = text_row(commands, parent);
            spawn_label(commands, row, label.as_deref());
            let labels: Vec<String> = options.iter().map(|(_, label)| label.clone()).collect();
            let current = options.iter().position(|(option, _)| option == value);
            let list = list_rows_from_strings(labels.iter().map(String::as_str), current);
            let mut entity = commands.spawn_scene(bsn! {
                @FeathersSelect {
                    @options: {list},
                    @max_visible: 8usize,
                }
                Node { flex_grow: 1.0 }
            });
            entity.insert((
                PluginSelectControl {
                    plugin: plugin.to_owned(),
                    panel: panel.to_owned(),
                    id: id.clone(),
                    options: options.clone(),
                },
                ChildOf(row),
            ));
            entity.observe(plugin_select_changed);
        }
        UiNode::List {
            id,
            selected,
            items,
            height,
        } => {
            let container = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: px(2),
                        width: percent(100),
                        max_height: height.map(px).unwrap_or(px(180.0)),
                        overflow: Overflow::scroll_y(),
                        ..default()
                    },
                    ChildOf(parent),
                ))
                .id();
            for item in items {
                spawn_list_row(
                    commands,
                    container,
                    plugin,
                    panel,
                    id,
                    selected.as_deref(),
                    item,
                );
            }
        }
        UiNode::TextLines { id, label, values } => {
            let column = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: px(3),
                        width: percent(100),
                        ..default()
                    },
                    ChildOf(parent),
                ))
                .id();
            spawn_label(commands, column, label.as_deref());
            for (index, value) in values.iter().enumerate() {
                let row = commands
                    .spawn((
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: px(4),
                            width: percent(100),
                            ..default()
                        },
                        ChildOf(column),
                    ))
                    .id();
                let container = commands
                    .spawn_scene(bsn! {
                        @FeathersTextInputContainer
                        Node { flex_grow: 1.0, min_width: px(0) }
                    })
                    .insert(ChildOf(row))
                    .id();
                commands.spawn_scene(bsn! { @FeathersTextInput }).insert((
                    Text::new(value.clone()),
                    PluginTextInput {
                        plugin: plugin.to_owned(),
                        panel: panel.to_owned(),
                        id: id.clone(),
                        index: Some(index),
                        last: value.clone(),
                        prefilled: false,
                    },
                    ChildOf(container),
                ));
                commands
                    .spawn_scene(bsn! {
                        @FeathersToolButton {
                            @variant: ButtonVariant::Plain,
                            @caption: bsn! { @caption("×") },
                        }
                    })
                    .insert((
                        PluginLineRemove {
                            plugin: plugin.to_owned(),
                            panel: panel.to_owned(),
                            id: id.clone(),
                            index,
                        },
                        ChildOf(row),
                    ))
                    .observe(plugin_line_removed);
            }
            commands
                .spawn_scene(bsn! {
                    @FeathersToolButton {
                        @variant: ButtonVariant::Plain,
                        @caption: bsn! { @caption("+") },
                    }
                })
                .insert((
                    PluginLineAdd {
                        plugin: plugin.to_owned(),
                        panel: panel.to_owned(),
                        id: id.clone(),
                    },
                    ChildOf(column),
                ))
                .observe(plugin_line_added);
        }
        UiNode::Separator => {
            commands.spawn((
                Node {
                    width: percent(100),
                    height: px(1),
                    margin: UiRect::vertical(px(4)),
                    ..default()
                },
                BackgroundColor(theme::BORDER_STRONG),
                Pickable::IGNORE,
                ChildOf(parent),
            ));
        }
        UiNode::Scroll { height, children } => {
            let container = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        width: percent(100),
                        max_height: height.map(px).unwrap_or(px(260.0)),
                        overflow: Overflow::scroll_y(),
                        ..default()
                    },
                    ChildOf(parent),
                ))
                .id();
            for child in children {
                spawn_tree(commands, container, child, plugin, panel);
            }
        }
        UiNode::Spacer(size) => {
            commands.spawn((
                Node {
                    height: px(*size),
                    flex_shrink: 0.0,
                    ..default()
                },
                Pickable::IGNORE,
                ChildOf(parent),
            ));
        }
    }
}

fn spawn_label(commands: &mut Commands, parent: Entity, label: Option<&str>) {
    let Some(label) = label.filter(|label| !label.is_empty()) else {
        return;
    };
    commands.spawn((
        Text::new(label.to_owned()),
        TextFont::from_font_size(11.0),
        TextColor(theme::TEXT_DIM),
        Node {
            width: px(132),
            flex_shrink: 0.0,
            ..default()
        },
        Pickable::IGNORE,
        ChildOf(parent),
    ));
}

fn text_row(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(6),
                width: percent(100),
                ..default()
            },
            ChildOf(parent),
        ))
        .id()
}

fn spawn_list_row(
    commands: &mut Commands,
    parent: Entity,
    plugin: &str,
    panel: &str,
    id: &str,
    selected: Option<&str>,
    item: &ListItem,
) {
    let label = match &item.status {
        Some(status) if !status.is_empty() => format!("{}  ·  {status}", item.label),
        _ => item.label.clone(),
    };
    let primary = selected == Some(item.id.as_str());
    commands
        .spawn_scene(bsn! {
            @FeathersToolButton {
                @variant: {if primary { ButtonVariant::Primary } else { ButtonVariant::Plain }},
                @caption: bsn! { @caption(label) },
            }
            Node { width: percent(100), justify_content: JustifyContent::FlexStart }
        })
        .insert((
            PluginListRow {
                plugin: plugin.to_owned(),
                panel: panel.to_owned(),
                id: id.to_owned(),
                value: item.id.clone(),
            },
            ChildOf(parent),
        ))
        .observe(plugin_list_clicked);
}

fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Normal => theme::TEXT,
        Tone::Dim => theme::TEXT_DIM,
        Tone::Warn => Color::srgb(0.95, 0.75, 0.3),
        Tone::Error => Color::srgb(0.9, 0.35, 0.3),
    }
}

fn plugin_tab_clicked(
    activate: On<Activate>,
    tabs: Query<&PluginTabButton>,
    mut active: ResMut<ActivePluginPanel>,
) {
    let Ok(tab) = tabs.get(activate.event_target()) else {
        return;
    };
    active.0 = Some(format!("{}:{}", tab.plugin, tab.panel));
}

fn plugin_button_clicked(
    activate: On<Activate>,
    buttons: Query<&PluginButton>,
    mut events: ResMut<PluginUiEvents>,
) {
    let Ok(button) = buttons.get(activate.event_target()) else {
        return;
    };
    push_event(
        &mut events,
        &button.plugin,
        &button.panel,
        "click",
        &button.id,
        None,
        None,
    );
}

fn plugin_list_clicked(
    activate: On<Activate>,
    rows: Query<&PluginListRow>,
    mut events: ResMut<PluginUiEvents>,
) {
    let Ok(row) = rows.get(activate.event_target()) else {
        return;
    };
    push_event(
        &mut events,
        &row.plugin,
        &row.panel,
        "select",
        &row.id,
        Some(row.value.clone()),
        None,
    );
}

fn plugin_checkbox_changed(
    event: On<ValueChange<bool>>,
    controls: Query<&PluginCheckboxControl>,
    mut events: ResMut<PluginUiEvents>,
) {
    let Ok(control) = controls.get(event.source) else {
        return;
    };
    push_event(
        &mut events,
        &control.plugin,
        &control.panel,
        "change",
        &control.id,
        Some(event.value.to_string()),
        None,
    );
}

fn plugin_number_changed(
    event: On<ValueChange<f32>>,
    controls: Query<&PluginNumberControl>,
    mut events: ResMut<PluginUiEvents>,
) {
    if !event.is_final {
        return;
    }
    let Ok(control) = controls.get(event.source) else {
        return;
    };
    push_event(
        &mut events,
        &control.plugin,
        &control.panel,
        "change",
        &control.id,
        Some(event.value.to_string()),
        None,
    );
}

fn plugin_select_changed(
    event: On<ValueChange<Entity>>,
    controls: Query<&PluginSelectControl>,
    options: Query<&OptionIndex>,
    mut events: ResMut<PluginUiEvents>,
) {
    let Ok(control) = controls.get(event.source) else {
        return;
    };
    let Some(index) = options.get(event.value).ok().map(|index| index.0) else {
        return;
    };
    let Some((value, _)) = control.options.get(index) else {
        return;
    };
    push_event(
        &mut events,
        &control.plugin,
        &control.panel,
        "change",
        &control.id,
        Some(value.clone()),
        None,
    );
}

fn plugin_line_added(
    activate: On<Activate>,
    buttons: Query<&PluginLineAdd>,
    mut events: ResMut<PluginUiEvents>,
) {
    let Ok(button) = buttons.get(activate.event_target()) else {
        return;
    };
    push_event(
        &mut events,
        &button.plugin,
        &button.panel,
        "add",
        &button.id,
        None,
        None,
    );
}

fn plugin_line_removed(
    activate: On<Activate>,
    buttons: Query<&PluginLineRemove>,
    mut events: ResMut<PluginUiEvents>,
) {
    let Ok(button) = buttons.get(activate.event_target()) else {
        return;
    };
    push_event(
        &mut events,
        &button.plugin,
        &button.panel,
        "remove",
        &button.id,
        None,
        Some(button.index),
    );
}

fn push_event(
    events: &mut PluginUiEvents,
    plugin: &str,
    panel: &str,
    kind: &str,
    id: &str,
    value: Option<String>,
    index: Option<usize>,
) {
    if events.0.len() >= 256 {
        return;
    }
    events.0.push(PluginUiEvent {
        plugin: plugin.to_owned(),
        panel: panel.to_owned(),
        kind: kind.to_owned(),
        id: id.to_owned(),
        value,
        index,
    });
}

fn prefill_plugin_inputs(
    mut inputs: Query<(&mut PluginTextInput, &mut Text, Option<&mut EditableText>)>,
) {
    for (mut input, mut text, editable) in &mut inputs {
        if input.prefilled {
            continue;
        }
        let Some(mut editable) = editable else {
            continue;
        };
        input.prefilled = true;
        if text.0.is_empty() && !input.last.is_empty() {
            text.0 = input.last.clone();
        }
        editable.editor_mut().set_text(&input.last);
        editable.queue_edit(TextEdit::TextEnd(false));
    }
}

fn poll_plugin_inputs(
    mut inputs: Query<(&mut PluginTextInput, &EditableText)>,
    mut events: ResMut<PluginUiEvents>,
) {
    for (mut input, editable) in &mut inputs {
        if !input.prefilled {
            continue;
        }
        let value = editable.value().to_string();
        if value == input.last {
            continue;
        }
        input.last = value.clone();
        push_event(
            &mut events,
            &input.plugin,
            &input.panel,
            "change",
            &input.id,
            Some(value),
            input.index,
        );
    }
}
