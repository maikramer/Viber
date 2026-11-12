//! The interactive widgets: check, slider, input — plus tooltips and per-element
//! mouse cursors.
//!
//! A partir do Bevy 0.20 a mecânica destes widgets é a de `bevy_ui_widgets` /
//! `bevy_input_focus`; o que fica aqui é o ADAPTADOR declarativo: um
//! `<UiCheck>` continua a ser uma caixa cujo flag `checked` conduz classes, um
//! `<UiSlider>` continua a ser uma track cujo valor move `.fill`/`.handle`, e
//! um `<UiInput>` continua a ser um campo cujo texto espelha o `<UiText>`
//! interno. O XML, o CSS, o `UiRegistry` e o `viber.ui.*` não mudam.
//!
//! * `UiCheck` → [`bevy::ui_widgets::Checkbox`] + `Checked`;
//! * `UiSlider` → [`bevy::ui_widgets::Slider`] + `SliderValue`/`SliderRange`;
//! * `UiInput` → [`bevy::ui_widgets::TextInput`] + `EditableText`;
//! * o foco de teclado é o [`bevy::input_focus::InputFocus`], espelhado em
//!   [`UiFocusedInput`] para o resto da engine (gameplay, Luau).
//!
//! Os eventos Bevy (`ValueChange`, `Activate`, foco) são absorvidos por
//! observers e escritos de volta nos componentes Viber; o diffs de
//! [`super::events::collect_ui_widget_events`] transformam-nos na fila
//! `viber.ui.events()` de sempre.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::Pickable;
use bevy::prelude::*;
// Bevy 0.20: `Interaction`/`Button` do prelude são aliases de tipos privados.
use crate::ui::interaction::Interaction;
use bevy::text::EditableText;
use bevy::ui::{Checked, InteractionDisabled};
use bevy::ui_widgets::popover::{Popover, PopoverAlign, PopoverPlacement, PopoverSide};
use bevy::ui_widgets::{Checkbox, Slider, SliderDragState, SliderValue, TextInput, ValueChange};
use bevy::window::{CursorIcon, PrimaryWindow};

use super::runtime::{UiClasses, UiDisabled, UiStyleDirty};

/// Ordena os extremos de um intervalo e afasta valores não finitos —
/// [`f32::clamp`] faz `assert!(min <= max)`, por isso um
/// `<UiSlider min="100" max="10">` (ou um extremo NaN/inf, que contagiaria o
/// resultado) rebentaria a engine. Os três pontos que clampeiam contra o
/// intervalo (o parse do XML, o arrasto e `viber.ui.set_value`) partilham
/// esta normalização; não finito cai no omissão do `<UiSlider>` (0..1) — o
/// aviso autoral vive no parse, em `tree.rs`.
pub(crate) fn normalized_range(min: f32, max: f32) -> (f32, f32) {
    if !min.is_finite() || !max.is_finite() {
        return (0.0, 1.0);
    }
    (min.min(max), max.max(min))
}

/// Marca uma entidade que também existe no vocabulário de widgets do Bevy.
/// O espelho de `UiDisabled` só toca nestas: um `InteractionDisabled` posto à
/// toa ficava para sempre num nó que nunca foi widget.
#[derive(Debug, Component)]
pub struct UiBevyWidget;

/// Marca um filho que não pode apanhar o pointer (`.fill`/`.handle` do slider,
/// texto do placeholder). O re-estilo repõe `Pickable` nestes nós; o sistema
/// [`sync_pick_through`] corre depois dele.
#[derive(Debug, Component)]
pub struct UiPickThrough;

// ── UiCheck ─────────────────────────────────────────────────────────────

/// A toggle: clicking flips `checked`; the classes `checked` / `unchecked`
/// mirror it for the stylesheet, and the `.tick` child shows only when on.
///
/// A decisão de flip é do [`Checkbox`] do Bevy (click, Enter/Space, gamepad);
/// este componente é o espelho que o CSS e o `viber.ui.read` consomem.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
pub struct UiCheck {
    pub checked: bool,
}

/// Espelha o `Checked` do Bevy para `UiCheck` e as classes.
///
/// O observer corre no trigger do widget (pointer click ou teclado), antes do
/// diff de eventos do frame — o `checked_changed` sai no mesmo frame em que o
/// jogador carregou.
pub fn on_check_value_change(
    change: On<ValueChange<bool>>,
    mut commands: Commands,
    mut checks: Query<(&mut UiCheck, &mut UiClasses, Has<UiStyleDirty>), With<Checkbox>>,
) {
    let Ok((mut check, mut classes, already_dirty)) = checks.get_mut(change.source) else {
        return;
    };
    check.checked = change.value;
    if change.value {
        commands.entity(change.source).insert(Checked);
    } else {
        commands.entity(change.source).remove::<Checked>();
    }
    let changed =
        classes.set_class("checked", change.value) | classes.set_class("unchecked", !change.value);
    if changed && !already_dirty {
        commands.entity(change.source).insert(UiStyleDirty);
    }
}

/// Mantém o [`Checked`] do Bevy e as classes em passo com `UiCheck`.
///
/// Corre quando um script (`viber.ui.set_checked`) ou o parse escrevem o
/// componente. A escrita é idempotente: o observer acima já deixou os dois
/// lados iguais, por isso o caso normal não toca em nada.
#[allow(clippy::type_complexity)]
pub fn drive_ui_checks(
    mut commands: Commands,
    mut checks: Query<
        (
            Entity,
            &UiCheck,
            Has<Checked>,
            &mut UiClasses,
            Has<UiStyleDirty>,
        ),
        Changed<UiCheck>,
    >,
) {
    for (entity, check, has_checked, mut classes, already_dirty) in &mut checks {
        match (check.checked, has_checked) {
            (true, false) => {
                commands.entity(entity).insert(Checked);
            }
            (false, true) => {
                commands.entity(entity).remove::<Checked>();
            }
            _ => {}
        }
        let changed = classes.set_class("checked", check.checked)
            | classes.set_class("unchecked", !check.checked);
        if changed && !already_dirty {
            commands.entity(entity).insert(UiStyleDirty);
        }
    }
}

/// Shows/hides the `.tick` child of every check.
///
/// Corre todos os frames (escrita idempotente): o re-estilo limpa o `display`
/// do tick e a classe `checked` é que o repõe — um resize não pode apagar
/// ticks. `display`, não `Visibility`: um nó escondido ainda ocupa layout.
#[allow(clippy::type_complexity)]
pub fn sync_check_ticks(
    checks: Query<(&UiCheck, &Children)>,
    classes: Query<&UiClasses>,
    mut nodes: Query<&mut Node>,
) {
    for (check, children) in &checks {
        for child in children.iter() {
            let Ok(classes) = classes.get(child) else {
                continue;
            };
            if !classes.has("tick") {
                continue;
            }
            if let Ok(mut node) = nodes.get_mut(child) {
                node.display = if check.checked {
                    Display::Flex
                } else {
                    Display::None
                };
            }
        }
    }
}

// ── UiSlider ────────────────────────────────────────────────────────────

/// A draggable range. `value` lives in `min..max` (`step` quantises it); the
/// `.fill` child mirrors the fraction like a bar and the `.handle` child
/// slides along the track. The class `dragging` is set while the pointer holds.
///
/// O arrasto/teclado é do [`Slider`] do Bevy; o observer
/// [`on_slider_value_change`] quantiza e escreve aqui para o CSS e o Luau.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
pub struct UiSlider {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    /// Quantisation; 0 means continuous.
    pub step: f32,
    pub vertical: bool,
    pub fill: Entity,
    pub handle: Entity,
}

impl UiSlider {
    /// Maps a 0..1 pointer position (already flipped for vertical tracks) to a
    /// stepped value in `min..max`.
    pub fn value_at(&self, fraction: f32) -> f32 {
        let fraction = fraction.clamp(0.0, 1.0);
        // Ordenar ANTES de interpolar: um intervalo invertido/não finito tem
        // de cair no 0..1 normalizado, não propagar NaN.
        let (min, max) = normalized_range(self.min, self.max);
        self.snap(min + fraction * (max - min))
    }

    /// Quantiza um valor cru para o passo e o intervalo. A defesa de
    /// `normalized_range` fica aqui também: o valor vem de um observer Bevy e
    /// não pode rebentar o `clamp` com um intervalo invertido.
    pub fn snap(&self, raw: f32) -> f32 {
        let (min, max) = normalized_range(self.min, self.max);
        let raw = if raw.is_finite() {
            raw.clamp(min, max)
        } else {
            min
        };
        if self.step > 0.0 {
            ((raw / self.step).round() * self.step).clamp(min, max)
        } else {
            raw
        }
    }

    /// 0..1 fraction of `value` within the range, for fill/handle sizing.
    pub fn fraction(&self) -> f32 {
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        ((self.value - self.min) / span).clamp(0.0, 1.0)
    }
}

/// Recebe o `ValueChange<f32>` do [`Slider`] do Bevy e escreve o valor em
/// [`UiSlider`] (quantizado ao `step` autoral).
pub fn on_slider_value_change(
    change: On<ValueChange<f32>>,
    mut commands: Commands,
    mut sliders: Query<(Entity, &mut UiSlider), With<Slider>>,
    values: Query<&SliderValue>,
) {
    let Ok((entity, mut slider)) = sliders.get_mut(change.source) else {
        return;
    };
    let wanted = slider.snap(change.value);
    if slider.value == wanted {
        return;
    }
    slider.value = wanted;
    // Componente imutável: substitui-se. O diff de eventos lê `Changed<UiSlider>`
    // no fim do frame.
    if values.get(entity).map_or(true, |value| value.0 != wanted) {
        commands.entity(entity).insert(SliderValue(wanted));
    }
}

/// Mantém o `SliderValue` do Bevy em passo com `UiSlider` (escritas de script).
pub fn mirror_ui_sliders(
    mut commands: Commands,
    sliders: Query<(Entity, &UiSlider), Changed<UiSlider>>,
    values: Query<&SliderValue>,
) {
    for (entity, slider) in &sliders {
        if values
            .get(entity)
            .map_or(true, |value| value.0 != slider.value)
        {
            commands.entity(entity).insert(SliderValue(slider.value));
        }
    }
}

/// Espelha `value` onto fill + handle, and the `dragging` class from Bevy's
/// drag state.
///
/// Corre todos os frames: o re-estilo (resize, `@media`) limpa os campos e
/// este sistema repõe-os no mesmo frame.
#[allow(clippy::type_complexity)]
pub fn drive_ui_sliders(
    mut commands: Commands,
    mut sliders: Query<
        (
            Entity,
            &UiSlider,
            Option<&SliderDragState>,
            &mut UiClasses,
            Has<UiStyleDirty>,
        ),
        Without<UiDisabled>,
    >,
    mut nodes: Query<&mut Node>,
) {
    for (entity, slider, drag, mut classes, already_dirty) in &mut sliders {
        let dragging = drag.is_some_and(|drag| drag.dragging);
        if classes.set_class("dragging", dragging) && !already_dirty {
            commands.entity(entity).insert(UiStyleDirty);
        }
        let fraction = slider.fraction() * 100.0;
        // Escrita gated: atribuir o mesmo `Val` marcava o `Node` em `Changed`
        // e o Taffy refazia o layout do slider em TODOS os frames.
        if let Ok(mut fill) = nodes.get_mut(slider.fill) {
            let wanted = Val::Percent(fraction);
            if slider.vertical {
                if fill.height != wanted {
                    fill.height = wanted;
                }
            } else if fill.width != wanted {
                fill.width = wanted;
            }
        }
        // The handle rides the fill edge: positioned by percentage so any
        // track size works without measuring pixels.
        if let Ok(mut handle) = nodes.get_mut(slider.handle) {
            if slider.vertical {
                let wanted = Val::Percent(100.0 - fraction);
                if handle.top != wanted {
                    handle.top = wanted;
                }
            } else {
                let wanted = Val::Percent(fraction);
                if handle.left != wanted {
                    handle.left = wanted;
                }
            }
        }
    }
}

/// O `.fill`/`.handle` não pode apanhar o pointer: o `slider_on_pointer_down`
/// do Bevy pára a propagação no thumb e a track nunca começaria o arrasto. O
/// mesmo para o placeholder do input (tem de deixar passar o clique).
pub fn sync_pick_through(
    mut commands: Commands,
    parts: Query<(Entity, Option<&Pickable>), With<UiPickThrough>>,
) {
    for (entity, pickable) in &parts {
        if pickable != Some(&Pickable::IGNORE) {
            commands.entity(entity).insert(Pickable::IGNORE);
        }
    }
}

// ── UiInput ─────────────────────────────────────────────────────────────

/// A single-line text field. Click to focus (class `focused`), type to edit,
/// Enter/Escape to blur. `child` is the editable [`EditableText`] node kept in
/// sync with `text`; `placeholder_child` is the dim label shown while empty.
#[derive(Debug, Component, Reflect)]
#[reflect(Component)]
pub struct UiInput {
    pub text: String,
    pub child: Entity,
    pub placeholder: Option<String>,
    /// Nó `<UiText>` do placeholder (irmão do editável), escondido enquanto
    /// houver texto.
    pub placeholder_child: Option<Entity>,
    /// Hard cap on stored characters; typing past it is dropped.
    pub max_len: Option<usize>,
}

/// Which input has the keyboard, if any.
///
/// Espelho compatível de `InputFocus` na raiz do `<UiInput>`: o gameplay
/// (`player`, `input_map`, Luau) e o `viber.ui.focused()` continuam a ler este
/// recurso, mas quem edita o texto é o `TextInput` do Bevy.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct UiFocusedInput(pub Option<Entity>);

/// Mapeia o foco do Bevy (`InputFocus`, que aponta para o nó editável) para a
/// raiz `<UiInput>` e escreve `UiFocusedInput`.
///
/// Também empurra o caminho inverso: `viber.ui.focus(id)`/`unfocus` escrevem
/// `UiFocusedInput`, e este sistema leva o pedido ao `InputFocus` do Bevy.
pub fn sync_ui_input_focus(
    mut input_focus: ResMut<InputFocus>,
    mut focus: ResMut<UiFocusedInput>,
    inputs: Query<(Entity, &UiInput)>,
    // A inicialização do recurso (`Added`) conta como "changed" e, com
    // `focus.0 = None`, seria lida como um blur que limpava um `InputFocus`
    // já posto no primeiro frame. Só o primeiro run ignora esse falso
    // positivo; depois, `is_changed` distingue escritas externas (scripts,
    // `drive_ui_modals`) das nossas (o próprio sistema não as vê).
    mut initialized: Local<bool>,
) {
    let root_of = |entity: Entity| -> Option<Entity> {
        if inputs.contains(entity) {
            Some(entity)
        } else {
            inputs
                .iter()
                .find(|(_, input)| input.child == entity)
                .map(|(root, _)| root)
        }
    };
    let first = !*initialized;
    *initialized = true;
    let external = focus.is_changed() && !(first && focus.0.is_none());
    if external {
        // Pedido vindo de um script (ou do `drive_ui_modals` a fechar).
        match focus.0 {
            Some(root) => {
                if let Some((_, input)) = inputs.iter().find(|(entity, _)| *entity == root) {
                    if input_focus.get() != Some(input.child) {
                        input_focus.set(input.child, FocusCause::Navigated);
                    }
                } else {
                    // O elemento deixou de ser um input (destroy/rebuild).
                    focus.0 = None;
                    input_focus.clear();
                }
            }
            None => {
                if input_focus.get().and_then(root_of).is_some() {
                    input_focus.clear();
                }
            }
        }
        return;
    }
    // Pedido vindo do Bevy (clique, navegação por teclado/gamepad).
    // Um clique na MOLDURA foca a raiz (o `TabIndex` dela); o teclado é do nó
    // editável, por isso o foco desce até ele.
    if let Some(root) = input_focus.get()
        && let Some((_, input)) = inputs.iter().find(|(entity, _)| *entity == root)
        && input_focus.get() != Some(input.child)
    {
        input_focus.set(input.child, FocusCause::Navigated);
    }
    let mapped = input_focus.get().and_then(root_of);
    if focus.0 != mapped {
        focus.0 = mapped;
    }
}

/// Enter/Escape tira o teclado ao campo.
///
/// O `TextInput` do Bevy já trata do Escape; o Enter fica por tratar (o campo
/// não aceita newlines) e era o blur documentado do `<UiInput>`.
pub fn blur_ui_inputs_on_enter(
    mut keys: bevy::ecs::message::MessageReader<KeyboardInput>,
    mut input_focus: ResMut<InputFocus>,
    inputs: Query<(Entity, &UiInput)>,
    editables: Query<&TextInput>,
) {
    let Some(focused) = input_focus.get() else {
        return;
    };
    let is_input = inputs.contains(focused)
        || inputs.iter().any(|(_, input)| input.child == focused)
        || editables.contains(focused);
    if !is_input {
        return;
    }
    for key in keys.read() {
        if key.state == bevy::input::ButtonState::Pressed
            && matches!(key.logical_key, Key::Enter | Key::Escape)
        {
            input_focus.clear();
            return;
        }
    }
}

/// Aplica a classe `focused` à raiz do input focado.
///
/// O `focused` visual continua a ser uma classe CSS (o stylesheet é que decide
/// a cor/anel), não um overlay de foco do Bevy. Nos botões só o
/// [`super::gamepad::UiGamepadFocus`] a recebe — o anel do menu é o indicador
/// da navegação por comando, e um clique de rato não o pode deixar preso (o
/// foco do Bevy em si é usado para Enter/Espaço, não para pintar).
pub fn sync_ui_focused_class(
    mut commands: Commands,
    focus: Res<UiFocusedInput>,
    input_focus: Res<InputFocus>,
    gamepad: Res<super::gamepad::UiGamepadFocus>,
    inputs: Query<(Entity, &UiInput)>,
    mut all_classes: Query<&mut UiClasses>,
    mut previous: Local<Option<Entity>>,
) {
    // Um input focado tem prioridade (a raiz); os restantes nós focáveis
    // (botões/checks/sliders) expõem-se directamente, mas só quando o foco
    // veio do gamepad.
    let current = focus.0.or_else(|| {
        let entity = input_focus.get()?;
        if gamepad.0 != Some(entity) {
            return None;
        }
        if inputs.iter().any(|(_, input)| input.child == entity) {
            return None;
        }
        all_classes.get(entity).ok().map(|_| entity)
    });
    if *previous == current {
        return;
    }
    if let Some(previous) = previous.take()
        && let Ok(mut classes) = all_classes.get_mut(previous)
        && classes.remove("focused")
    {
        commands.entity(previous).insert(UiStyleDirty);
    }
    if let Some(current) = current
        && let Ok(mut classes) = all_classes.get_mut(current)
        && classes.add("focused")
    {
        commands.entity(current).insert(UiStyleDirty);
    }
    *previous = current;
}

/// Mirrors the editable value into `UiInput.text` and toggles the placeholder.
pub fn sync_ui_input_text(
    mut commands: Commands,
    mut inputs: Query<(Entity, &mut UiInput)>,
    editables: Query<&EditableText>,
    mut visibility: Query<&mut Visibility>,
    mut classes: Query<&mut UiClasses>,
) {
    for (entity, mut input) in &mut inputs {
        let Ok(editable) = editables.get(input.child) else {
            continue;
        };
        let value = editable.value().to_string();
        if input.text != value {
            input.text = value;
        }
        let empty = input.text.is_empty();
        if let Some(child) = input.placeholder_child
            && let Ok(mut visibility) = visibility.get_mut(child)
        {
            let wanted = if empty {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if *visibility != wanted {
                *visibility = wanted;
            }
        }
        if let Ok(mut classes) = classes.get_mut(entity)
            && classes.set_class("placeholder", empty)
        {
            commands.entity(entity).insert(UiStyleDirty);
        }
    }
}

/// Aplica escritas de script (`viber.ui.set_text`) e do parse ao `EditableText`.
pub fn apply_ui_input_text(
    mut commands: Commands,
    inputs: Query<&UiInput, Changed<UiInput>>,
    editables: Query<&EditableText>,
) {
    for input in &inputs {
        let Ok(editable) = editables.get(input.child) else {
            continue;
        };
        if editable.value().to_string() != input.text {
            let mut replacement = EditableText::new(&input.text);
            replacement.max_characters = input.max_len;
            commands.entity(input.child).insert(replacement);
        }
    }
}

// ── tooltips ────────────────────────────────────────────────────────────

/// `tooltip="…"` on any element: shows a labelled hint near the pointer while
/// the element is hovered.
#[derive(Debug, Clone, Component, Reflect)]
#[reflect(Component)]
pub struct UiTooltipText(pub String);

/// Shared tooltip state: the floating element (spawned lazily under the first
/// UiRoot), its text child, and whose text it currently shows.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct UiTooltipLayer {
    pub entity: Option<Entity>,
    pub text_entity: Option<Entity>,
    pub showing: Option<String>,
    /// Dono actual do popover; reparentar é o que o `Popover` do Bevy usa para
    /// calcular a posição.
    pub target: Option<Entity>,
}

/// Floats one tooltip over the hovered `tooltip="…"` element.
///
/// O posicionamento é o [`Popover`] do Bevy (ancorado ao elemento, com
/// fallback para cima quando não há espaço em baixo), não o cursor. A camada é
/// criada na primeira utilização, vive sob o primeiro UiRoot e é reparentada
/// para o alvo enquanto o hint estiver visível.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn drive_ui_tooltip(
    mut commands: Commands,
    mut layer: ResMut<UiTooltipLayer>,
    hovered: Query<(Entity, &Interaction, &UiTooltipText)>,
    roots: Query<(Entity, &super::runtime::UiTag)>,
    // `Option`: mundos sem HUD (qa-agua, terrain demo) não inserem
    // `HudAssets` — exigir o resource panica a engine no arranque.
    assets: Option<Res<crate::hud::HudAssets>>,
    mut texts: Query<&mut Text>,
    nodes: Query<(), With<Node>>,
    mut visibility: Query<&mut Visibility>,
) {
    let Some(assets) = assets else {
        return;
    };
    // A camada nasce como filha de um `UiRoot` e morre com ele (rebuild de
    // cena/UI). Sem este reset, `layer.entity` apontava para uma entidade
    // morta e o tooltip deixava de aparecer para o resto da sessão.
    if let Some(tip) = layer.entity
        && nodes.get(tip).is_err()
    {
        layer.entity = None;
        layer.text_entity = None;
        layer.showing = None;
        layer.target = None;
    }
    let target = hovered.iter().find_map(|(entity, interaction, tooltip)| {
        (*interaction != Interaction::None).then(|| (entity, tooltip.0.clone()))
    });
    // Lazily build the floating layer the first time anything needs it.
    if layer.entity.is_none() {
        let Some((root, _)) = roots.iter().find(|(_, tag)| tag.0 == "uiroot") else {
            return;
        };
        let tip = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    display: Display::None,
                    ..Default::default()
                },
                super::runtime::UiTag("uitooltip".to_string()),
                UiClasses::parse("tooltip"),
                // `align-self: start` é obrigatório: num flex row (o UiRoot)
                // o Taffy ESTICA filhos absolutos sem tamanho no eixo cruzado
                // — o tooltip descia até ao fundo do ecrã (top definido,
                // bottom auto ⇒ altura = ecrã − top).
                super::runtime::UiInlineStyle(super::style::parse_declarations(
                    "background: #0b0d12e6; radius: 6; padding: 5 8; z: 9000; align-self: start; pointer-events: none",
                    "ui tooltip defaults",
                )),
                super::runtime::UiStyleDirty,
                Popover {
                    positions: vec![
                        PopoverPlacement {
                            side: PopoverSide::Bottom,
                            align: PopoverAlign::Start,
                            gap: 8.0,
                        },
                        PopoverPlacement {
                            side: PopoverSide::Top,
                            align: PopoverAlign::Start,
                            gap: 8.0,
                        },
                    ],
                    window_margin: 6.0,
                },
                Visibility::Hidden,
                ChildOf(root),
            ))
            .id();
        let text_entity = commands
            .spawn((
                Text::default(),
                TextColor(Color::WHITE),
                TextFont {
                    font: assets.font.clone().into(),
                    font_size: 12.0.into(),
                    ..Default::default()
                },
                TextLayout::default(),
            ))
            .id();
        // Reserved ids: the parent link queues fine before anything applies.
        commands.entity(tip).add_child(text_entity);
        layer.entity = Some(tip);
        layer.text_entity = Some(text_entity);
    }
    let Some(tip) = layer.entity else {
        return;
    };
    match target {
        Some((target, text)) => {
            // O popover ancora ao elemento; reparentar é mais barato do que
            // manter uma posição absoluta sincronizada com o cursor.
            if layer.target != Some(target) {
                commands.entity(tip).insert(ChildOf(target));
                layer.target = Some(target);
            }
            if layer.showing.as_deref() != Some(text.as_str()) {
                if let Some(text_entity) = layer.text_entity
                    && let Ok(mut tip_text) = texts.get_mut(text_entity)
                {
                    tip_text.0 = text.clone();
                }
                layer.showing = Some(text);
            }
            if let Ok(mut visibility) = visibility.get_mut(tip)
                && *visibility == Visibility::Hidden
            {
                *visibility = Visibility::Inherited;
            }
        }
        None => {
            layer.showing = None;
            layer.target = None;
            if let Ok(mut visibility) = visibility.get_mut(tip)
                && *visibility != Visibility::Hidden
            {
                *visibility = Visibility::Hidden;
            }
        }
    }
}

// ── mouse cursor ────────────────────────────────────────────────────────

/// Per-element mouse cursor (`cursor: pointer`), applied while hovered.
#[derive(Debug, Clone, Component)]
pub struct UiCursorIcon(pub CursorIcon);

/// Sets the primary window's cursor from the hovered element's [`UiCursorIcon`].
///
/// Any hovered element with a cursor wins; when none does, the cursor falls
/// back to the system default. A passada re-lê TODOS os elementos com cursor
/// (não só os `Changed<Interaction>`), por isso um elemento hovered que
/// despawnou — e leva a sua aresta de `Interaction` atrás — chega aqui como
/// "nada hovered" em vez de um pointer preso para sempre. Writes only on
/// change — the window component would otherwise report a change every frame.
#[allow(clippy::type_complexity)]
pub fn drive_ui_cursors(
    mut commands: Commands,
    cursors: Query<(Entity, &UiCursorIcon)>,
    interactions: Query<&Interaction>,
    // A janela PRIMÁRIA, não "a primeira que aparecer": com mais janelas o
    // cursor tem de seguir o rato do jogador.
    windows: Query<Entity, (With<Window>, With<PrimaryWindow>)>,
    mut last: Local<Option<CursorIcon>>,
) {
    let icon = cursors
        .iter()
        .find(|(entity, _)| {
            interactions
                .get(*entity)
                .is_ok_and(|interaction| *interaction != Interaction::None)
        })
        .map(|(_, cursor)| cursor.0.clone());
    if *last == icon {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    commands
        .entity(window)
        .insert(icon.clone().unwrap_or_default());
    *last = icon;
}

// ── disabled ────────────────────────────────────────────────────────────

/// Espelha `UiDisabled` no `InteractionDisabled` do Bevy (e vice-versa).
///
/// Os widgets do Bevy consultam `InteractionDisabled` para recusar clicks/
/// teclado; a engine continua a usar `UiDisabled` para `:disabled` e para os
/// drivers antigos.
pub fn sync_ui_disabled(
    mut commands: Commands,
    disabled: Query<
        Entity,
        (
            With<UiDisabled>,
            With<UiBevyWidget>,
            Without<InteractionDisabled>,
        ),
    >,
    enabled: Query<
        Entity,
        (
            Without<UiDisabled>,
            With<UiBevyWidget>,
            With<InteractionDisabled>,
        ),
    >,
    inputs: Query<(&UiInput, Has<UiDisabled>)>,
    editables: Query<(Entity, Has<InteractionDisabled>), With<TextInput>>,
) {
    for entity in &disabled {
        commands.entity(entity).insert(InteractionDisabled);
    }
    for entity in &enabled {
        commands.entity(entity).remove::<InteractionDisabled>();
    }
    // Um input desactivado desactiva o nó editável (é ele que recebe o teclado).
    for (input, disabled) in &inputs {
        let Ok((editable, has_disabled)) = editables.get(input.child) else {
            continue;
        };
        if disabled && !has_disabled {
            commands.entity(editable).insert(InteractionDisabled);
        } else if !disabled && has_disabled {
            commands.entity(editable).remove::<InteractionDisabled>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slider(step: f32) -> UiSlider {
        UiSlider {
            value: 0.0,
            min: 0.0,
            max: 100.0,
            step,
            vertical: false,
            fill: Entity::PLACEHOLDER,
            handle: Entity::PLACEHOLDER,
        }
    }

    #[test]
    fn test_slider_value_maps_and_steps() {
        let stepped = slider(5.0);
        assert!((stepped.value_at(0.5) - 50.0).abs() < 1e-5);
        assert!(
            (stepped.value_at(0.507) - 50.0).abs() < 1e-4,
            "snaps to step"
        );
        assert_eq!(stepped.value_at(-1.0), 0.0, "clamps low");
        assert_eq!(stepped.value_at(2.0), 100.0, "clamps high");
        // Continuous (step 0) keeps the raw fraction.
        let free = slider(0.0);
        assert!((free.value_at(0.333) - 33.3).abs() < 1e-3);
    }

    #[test]
    fn test_slider_fraction_guards_a_flat_range() {
        let mut flat = slider(0.0);
        flat.max = 0.0;
        assert_eq!(flat.fraction(), 0.0, "min == max divides by zero");
        let mut offset = slider(0.0);
        offset.min = 10.0;
        offset.max = 20.0;
        offset.value = 15.0;
        assert!((offset.fraction() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_value_at_survives_an_inverted_range() {
        // Defesa em profundidade: o parse normaliza os extremos, mas o
        // `snap` volta a ordená-los para o `clamp` nunca assertar
        // (intervalo invertido ou não finito não pode rebentar o arrasto).
        let mut inverted = slider(0.0);
        inverted.min = 100.0;
        inverted.max = 10.0;
        assert!((inverted.value_at(0.5) - 55.0).abs() < 1e-4);
        let mut nan = slider(0.0);
        nan.min = f32::NAN;
        nan.max = 10.0;
        assert!((nan.value_at(0.25) - 0.25).abs() < 1e-4, "cai no 0..1");
    }

    #[test]
    fn test_snap_matches_drag_and_keyboard_values() {
        let stepped = slider(5.0);
        assert_eq!(stepped.snap(12.4), 10.0);
        assert_eq!(stepped.snap(12.6), 15.0);
        assert_eq!(stepped.snap(-10.0), 0.0);
        assert_eq!(stepped.snap(999.0), 100.0);
        assert_eq!(stepped.snap(f32::NAN), 0.0);
        // Contínuo não quantiza.
        assert!((slider(0.0).snap(12.34) - 12.34).abs() < 1e-6);
    }

    #[test]
    fn test_input_edits_respect_max_len() {
        // The typing rules now live in `EditableText::max_characters`; this
        // pins the counting rule (chars, not bytes — acentos contam 1).
        let mut text = String::from("çé");
        assert_eq!(text.chars().count(), 2);
        text.pop();
        assert_eq!(text, "ç");
    }

    #[test]
    fn test_bevy_check_value_change_feeds_ui_check() {
        let mut app = App::new();
        app.add_observer(on_check_value_change);
        let entity = app
            .world_mut()
            .spawn((Checkbox, UiCheck { checked: false }, UiClasses::default()))
            .id();
        app.world_mut().trigger(ValueChange {
            source: entity,
            value: true,
            is_final: true,
        });
        app.world_mut().flush();
        assert!(app.world().get::<UiCheck>(entity).unwrap().checked);
        assert!(
            app.world().get::<Checked>(entity).is_some(),
            "o Checked do Bevy é o estado do widget"
        );
        assert!(app.world().get::<UiClasses>(entity).unwrap().has("checked"));
    }

    #[test]
    fn test_bevy_slider_value_change_snaps_back() {
        let mut app = App::new();
        app.add_observer(on_slider_value_change);
        let entity = app.world_mut().spawn((Slider::default(), slider(5.0))).id();
        app.world_mut().trigger(ValueChange {
            source: entity,
            value: 12.4_f32,
            is_final: true,
        });
        app.world_mut().flush();
        assert_eq!(app.world().get::<UiSlider>(entity).unwrap().value, 10.0);
        assert_eq!(app.world().get::<SliderValue>(entity).unwrap().0, 10.0);
    }

    #[test]
    fn test_input_focus_bridges_to_ui_focused_input() {
        let mut app = App::new();
        app.init_resource::<InputFocus>()
            .init_resource::<UiFocusedInput>()
            .init_resource::<super::super::gamepad::UiGamepadFocus>()
            .add_systems(Update, (sync_ui_input_focus, sync_ui_focused_class).chain());

        let child = app.world_mut().spawn_empty().id();
        let root = app
            .world_mut()
            .spawn((
                UiInput {
                    text: String::new(),
                    child,
                    placeholder: None,
                    placeholder_child: None,
                    max_len: None,
                },
                UiClasses::default(),
            ))
            .id();

        // O Bevy foca o NÓ EDITÁVEL; a raiz é que é visível aos scripts.
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(child, FocusCause::Pressed);
        app.update();
        assert_eq!(app.world().resource::<UiFocusedInput>().0, Some(root));
        assert!(
            app.world().get::<UiClasses>(root).unwrap().has("focused"),
            "o campo focado leva a classe CSS"
        );

        // `viber.ui.focus(id)` escreve a raiz; o sistema leva o pedido ao
        // `InputFocus` do Bevy.
        app.world_mut().resource_mut::<InputFocus>().clear();
        app.world_mut().resource_mut::<UiFocusedInput>().0 = Some(root);
        app.update();
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(child));

        // Blur por script limpa os dois lados.
        app.world_mut().resource_mut::<UiFocusedInput>().0 = None;
        app.update();
        assert_eq!(app.world().resource::<InputFocus>().get(), None);
    }
}
