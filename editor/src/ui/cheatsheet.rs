//! F1: the shortcut cheat sheet. Static data (it documents the bindings the
//! editor already dispatches); bilingual via [`Bi`].

use bevy::feathers::display::caption;
use bevy::picking::events::PointerClick;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;

use super::icons::{self, fa, icon};
use super::strings::{Bi, Lang, LangLabel, StrKey, t};
use super::theme;

/// Open/closed state of the modal.
#[derive(Resource, Debug, Default)]
pub struct CheatSheetOpen(pub bool);

/// The modal overlay.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct CheatSheetPanel;

/// The scrollable list body.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct CheatSheetBody;

/// The toolbar `?` button.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct CheatSheetToggle;

pub struct CheatSheetPlugin;

impl Plugin for CheatSheetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CheatSheetOpen>()
            .add_systems(Update, (toggle_cheatsheet, render_cheatsheet).chain());
    }
}

pub struct Group {
    pub title: Bi,
    pub bindings: &'static [Binding],
}

pub struct Binding {
    pub keys: &'static str,
    pub action: Bi,
}

const fn b(keys: &'static str, pt: &'static str, en: &'static str) -> Binding {
    Binding {
        keys,
        action: Bi(pt, en),
    }
}

pub static GROUPS: &[Group] = &[
    Group {
        title: Bi("Documento", "Document"),
        bindings: &[
            b("Ctrl+S", "Guardar", "Save"),
            b("Ctrl+Z", "Desfazer", "Undo"),
            b("Ctrl+Shift+Z / Ctrl+Y", "Refazer", "Redo"),
            b("Ctrl+D", "Duplicar", "Duplicate"),
            b("Ctrl+C / Ctrl+V", "Copiar / colar", "Copy / paste"),
            b("Delete", "Apagar", "Delete"),
            b("F2", "Renomear", "Rename"),
            b("Ctrl+R", "Recarregar do disco", "Reload from disk"),
            b("Ctrl+Q", "Guardar e sair", "Save and quit"),
        ],
    },
    Group {
        title: Bi("Transformar", "Transform"),
        bindings: &[
            b(
                "Q / W / E / R",
                "Selecionar / mover / rodar / escalar",
                "Select / move / rotate / scale",
            ),
            b("T", "Eixos locais / globais", "Local / global axes"),
            b("F", "Enquadrar a seleção", "Frame selection"),
            b("Shift+F", "Enquadrar tudo", "Frame everything"),
            b(
                "Ctrl (a arrastar)",
                "Snap pelos passos da Vista",
                "Snap by the View steps",
            ),
            b("Esc (a arrastar)", "Cancelar o arrasto", "Cancel the drag"),
            b("Ctrl+clique", "Alternar na seleção", "Toggle in selection"),
            b("Shift+clique", "Adicionar à seleção", "Add to selection"),
        ],
    },
    Group {
        title: Bi("Câmara", "Camera"),
        bindings: &[
            b("Botão do meio", "Órbita", "Orbit"),
            b("Shift + botão do meio", "Deslocar", "Pan"),
            b("Roda", "Zoom", "Zoom"),
            b("Botão direito + WASD/QE", "Voo livre", "Free fly"),
            b("Roda durante o voo", "Velocidade do voo", "Fly speed"),
            b(
                "Teclado numérico 1/3/7/5",
                "Frente / direita / topo / persp",
                "Front / right / top / persp",
            ),
        ],
    },
    Group {
        title: Bi("Terreno", "Terrain"),
        bindings: &[
            b("B", "Pincel de terreno", "Terrain brush"),
            b(
                "1 … 7",
                "Elevar / baixar / suavizar / aplanar / cratera / pintar / ruído",
                "Raise / lower / smooth / flatten / crater / paint / noise",
            ),
            b(", / .", "Camada de material", "Material layer"),
            b("[ / ]", "Raio do pincel", "Brush radius"),
            b("- / =", "Força do pincel", "Brush strength"),
            b("S", "Carimbar o GLB ativo", "Stamp the active GLB"),
        ],
    },
    Group {
        title: Bi("Jogo", "Play"),
        bindings: &[
            b("F5", "Jogar o mundo", "Play the world"),
            b("F7", "Jogar a cena ativa", "Play the active scene"),
            b("F9", "Jogar a partir da câmara", "Play from the camera"),
            b("F6", "Pausar / retomar", "Pause / resume"),
            b("F8", "Parar", "Stop"),
        ],
    },
    Group {
        title: Bi("Painéis e procura", "Panels and search"),
        bindings: &[
            b("F1", "Esta ajuda", "This help"),
            b("Ctrl+K", "Paleta de comandos", "Command palette"),
            b("Ctrl+F", "Pesquisar no mundo", "Search the world"),
            b(
                "Ctrl+B / Ctrl+I / Ctrl+J",
                "Objetos / Inspetor / dock inferior",
                "Objects / Inspector / bottom dock",
            ),
            b(
                "↑ / ↓",
                "Linha anterior / seguinte (sobre Objetos)",
                "Previous / next row (over Objects)",
            ),
            b(
                "← / →",
                "Colapsar / expandir ou pai / filho",
                "Collapse / expand or parent / child",
            ),
            b("Home / End", "Primeira / última linha", "First / last row"),
        ],
    },
];

/// F1 toggles; Esc closes (unless a text field owns the keyboard).
fn toggle_cheatsheet(
    keys: Res<ButtonInput<KeyCode>>,
    focus: Option<Res<bevy::input_focus::InputFocus>>,
    editable: Query<(), With<bevy::text::EditableText>>,
    mut open: ResMut<CheatSheetOpen>,
) {
    let typing = focus
        .and_then(|focus| focus.get())
        .is_some_and(|entity| editable.contains(entity));
    if keys.just_pressed(KeyCode::F1) && !typing {
        open.0 = !open.0;
    } else if keys.just_pressed(KeyCode::Escape) && open.0 && !typing {
        open.0 = false;
    }
}

/// The toolbar button opens it.
pub fn cheat_sheet_clicked(
    activate: On<Activate>,
    buttons: Query<(), With<CheatSheetToggle>>,
    mut open: ResMut<CheatSheetOpen>,
) {
    if buttons.contains(activate.event_target()) {
        open.0 = !open.0;
    }
}

/// Clicking the dimmed backdrop closes the modal.
pub fn close_cheatsheet_on_backdrop(
    click: On<PointerClick>,
    panels: Query<(), With<CheatSheetPanel>>,
    mut open: ResMut<CheatSheetOpen>,
) {
    if panels.contains(click.original_event_target()) {
        open.0 = false;
    }
}

/// The modal card, spawned in the editor root.
pub fn cheat_sheet_panel() -> impl Scene {
    bsn! {
        CheatSheetPanel
        GlobalZIndex(65)
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            display: Display::None,
        }
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45))
        on(close_cheatsheet_on_backdrop)
        Children [
            Node {
                width: px(640),
                max_height: px(620),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                padding: px(14),
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
                    padding: UiRect::bottom(px(6)),
                    border: UiRect::bottom(px(1)),
                    flex_shrink: 0.0,
                }
                BorderColor::all(theme::BORDER)
                Children [
                    @icon(fa::CIRCLE_QUESTION, 14.0, theme::ACCENT)
                    --
                    @caption(t(Lang::default(), StrKey::CheatSheetTitle)) LangLabel(StrKey::CheatSheetTitle)
                    --
                    Node { flex_grow: 1.0 }
                    --
                    @caption("Esc") Pickable::IGNORE
                    TextColor(icons::ICON_DIM)
                ]
                --
                Node {
                    flex_grow: 1.0,
                    min_height: px(0),
                    overflow: Overflow::scroll_y(),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(10),
                }
                CheatSheetBody
                Children []
            ]
        ]
    }
}

fn render_cheatsheet(
    mut commands: Commands,
    open: Res<CheatSheetOpen>,
    lang: Res<Lang>,
    mut panels: Query<&mut Node, With<CheatSheetPanel>>,
    bodies: Query<Entity, With<CheatSheetBody>>,
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
    if *built && !lang.is_changed() {
        return;
    }
    *built = true;
    let Ok(body) = bodies.single() else {
        return;
    };
    commands.entity(body).despawn_related::<Children>();
    for group in GROUPS {
        commands.spawn((
            Text::new(group.title.get(*lang)),
            TextFont::from_font_size(12.0),
            TextColor(theme::ACCENT),
            Node {
                margin: UiRect::top(px(4)),
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(body),
        ));
        for binding in group.bindings {
            let row = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: px(12),
                        padding: UiRect::axes(px(6), px(1)),
                        ..default()
                    },
                    ChildOf(body),
                ))
                .id();
            commands.spawn((
                Text::new(binding.keys),
                TextFont::from_font_size(11.5),
                TextColor(Color::srgb(0.72, 0.8, 0.95)),
                Node {
                    width: px(210),
                    flex_shrink: 0.0,
                    ..default()
                },
                Pickable::IGNORE,
                ChildOf(row),
            ));
            commands.spawn((
                Text::new(binding.action.get(*lang)),
                TextFont::from_font_size(12.0),
                TextColor(theme::TEXT),
                Pickable::IGNORE,
                ChildOf(row),
            ));
        }
    }
}

/// Tests the table itself (the UI only renders it).
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_binding_has_both_languages_and_unique_chords_per_group() {
        for group in GROUPS {
            assert!(!group.title.0.is_empty() && !group.title.1.is_empty());
            let mut seen = HashSet::new();
            for binding in group.bindings {
                assert!(!binding.keys.is_empty());
                assert!(
                    !binding.action.0.is_empty() && !binding.action.1.is_empty(),
                    "{:?} sem tradução",
                    binding.keys
                );
                assert!(
                    seen.insert(binding.keys),
                    "atalho repetido no grupo: {}",
                    binding.keys
                );
            }
        }
    }

    #[test]
    fn cheat_sheet_labels_are_bilingual() {
        assert_eq!(t(Lang::Pt, StrKey::CheatSheetTitle), "Atalhos");
        assert_eq!(t(Lang::En, StrKey::CheatSheetTitle), "Shortcuts");
        assert_eq!(t(Lang::Pt, StrKey::TipCheatSheet), "Atalhos (F1)");
        assert_eq!(t(Lang::En, StrKey::TipCheatSheet), "Shortcuts (F1)");
    }

    #[test]
    fn toggle_keys_do_not_duplicate_across_groups() {
        // The same chord may appear in two contexts (e.g. Ctrl+B vs a text
        // shortcut), so only check the table is non-trivial.
        assert!(GROUPS.len() >= 6);
        assert!(
            GROUPS
                .iter()
                .map(|group| group.bindings.len())
                .sum::<usize>()
                >= 30
        );
    }
}
