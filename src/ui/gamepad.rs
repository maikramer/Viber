//! Navegação da UI declarativa por gamepad.
//!
//! O foco direccional deixou de ser um `pick_direction` à mão: o
//! [`AutoDirectionalNavigator`] do Bevy (sobre o `AutoDirectionalNavigation`
//! que cada nó focável carrega) escolhe o vizinho geométrico na direcção do
//! D-pad/stick, e o estado vive no `InputFocus` do `bevy_input_focus`. Este
//! módulo só traduz o gamepad para octantes e acciona o foco com A.
//!
//! Só corre com um menu/modal aberto (`MenusOpen::any()`); no mundo, o stick
//! pertence ao movimento do herói. [`UiGamepadFocus`] é o espelho compatível
//! que o resto da engine lê (e a classe CSS `focused` vem do `InputFocus`).

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::input_focus::InputFocusVisible;
use bevy::math::CompassOctant;
use bevy::prelude::*;
use bevy::ui::auto_directional_navigation::{AutoDirectionalNavigation, AutoDirectionalNavigator};
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::ui_widgets::{Activate, ToggleChecked};

use super::runtime::{UiDisabled, UiPointerNone};
use crate::menus::MenusOpen;
use crate::ui::interaction::Interaction;

/// Elemento com foco do gamepad. `None` = sem foco.
///
/// Espelho de `InputFocus` enquanto um menu está aberto; mantém-se o recurso
/// para os consumidores que já o liam e para o `viber.ui`/profiler.
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct UiGamepadFocus(pub Option<Entity>);

fn node_center(node: &ComputedNode, transform: &UiGlobalTransform) -> Vec2 {
    let (_, _, translation) = transform.to_scale_angle_translation();
    translation + node.size * 0.5
}

/// Converte o eixo do stick/D-pad num octante do `CompassOctant`.
///
/// O eixo Y da UI cresce para BAIXO; o stick para cima é Y positivo, por isso
/// o chamador inverte o Y antes de chegar aqui. Um eixo nulo em ambos os
/// sentidos não navega.
fn stick_octant(dx: f32, dy: f32) -> Option<CompassOctant> {
    let sign = |value: f32| {
        if value > 0.0 {
            1
        } else if value < 0.0 {
            -1
        } else {
            0
        }
    };
    let x = sign(dx);
    let y = sign(dy);
    Some(match (x, y) {
        (0, -1) => CompassOctant::North,
        (1, -1) => CompassOctant::NorthEast,
        (1, 0) => CompassOctant::East,
        (1, 1) => CompassOctant::SouthEast,
        (0, 1) => CompassOctant::South,
        (-1, 1) => CompassOctant::SouthWest,
        (-1, 0) => CompassOctant::West,
        (-1, -1) => CompassOctant::NorthWest,
        _ => return None,
    })
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn drive_ui_gamepad_nav(
    menus: Option<Res<MenusOpen>>,
    typing: Option<Res<super::widgets::UiFocusedInput>>,
    pads: Query<&Gamepad>,
    mut nav: AutoDirectionalNavigator,
    mut focus: ResMut<UiGamepadFocus>,
    mut focus_visible: ResMut<InputFocusVisible>,
    candidates: Query<
        (
            Entity,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&InheritedVisibility>,
            Option<&UiDisabled>,
            Option<&UiPointerNone>,
        ),
        With<AutoDirectionalNavigation>,
    >,
    mut interactions: Query<&mut Interaction>,
    mut stick_armed: Local<bool>,
    // Entidades cujo `Pressed` foi sintetizado por nós. Sem isto o botão
    // ficava pressionado para sempre (ninguém o solta sem um rato).
    mut synthetic: Local<Vec<Entity>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut commands: Commands,
) {
    // Solta no frame seguinte o que o gamepad pressionou. `collect_ui_clicks`
    // já correu depois do sistema no frame anterior e viu o `Changed`.
    if mouse.get_pressed().next().is_none() {
        for entity in synthetic.drain(..) {
            if let Ok(mut interaction) = interactions.get_mut(entity)
                && *interaction == Interaction::Pressed
            {
                *interaction = Interaction::None;
            }
        }
    }
    if !menus.as_deref().is_some_and(MenusOpen::any) {
        // Sair do menu tira o foco da UI — mas nunca o de um campo de texto
        // que viva fora dele (barra de fala do NLU) nem o de um nó que não
        // seja desta UI (o `InputFocus` é partilhado com o resto do Bevy).
        if typing.as_deref().is_none_or(|typing| typing.0.is_none()) {
            if let Some(current) = nav.input_focus()
                && candidates.contains(current)
            {
                nav.manual_directional_navigation.focus.clear();
            }
            focus.0 = None;
        }
        *stick_armed = false;
        return;
    }
    // Com um `<UiInput>` focado as teclas pertencem ao texto; o stick também
    // não deve arrancar o foco do campo (nem limpar o espelho).
    if typing.as_deref().is_some_and(|typing| typing.0.is_some()) {
        *stick_armed = false;
        return;
    }

    let mut live: Vec<(Entity, Vec2)> = Vec::new();
    for (entity, node, transform, visibility, disabled, pointer_none) in &candidates {
        if disabled.is_some() || pointer_none.is_some() {
            continue;
        }
        if visibility.is_some_and(|visible| !visible.get()) {
            continue;
        }
        if node.size().x <= 1.0 || node.size().y <= 1.0 {
            continue;
        }
        live.push((entity, node_center(node, transform)));
    }
    if live.is_empty() {
        // Menu sem focáveis: não há foco de gamepad para manter — mas o
        // `InputFocus` global pode pertencer a outra coisa.
        focus.0 = None;
        return;
    }

    // Sem comando ligado não há navegação nenhuma: o teclado tem o seu
    // próprio caminho (`InputFocus`/Tab, quando instalado).
    let Some(pad) = pads.iter().next() else {
        return;
    };

    let current = nav
        .input_focus()
        .filter(|entity| live.iter().any(|(candidate, _)| candidate == entity))
        .or_else(|| {
            // Sem foco, o primeiro da esquerda para a direita, de cima para
            // baixo. A classe entra já neste frame (o sync do foco corre
            // depois), e o A premido no mesmo frame em que o menu abriu não
            // se perde.
            live.iter()
                .min_by(|(_, a), (_, b)| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)))
                .map(|(entity, _)| *entity)
        });
    let Some(current) = current else {
        return;
    };
    if nav.input_focus() != Some(current) {
        nav.manual_directional_navigation
            .focus
            .set(current, bevy::input_focus::FocusCause::Navigated);
        // O anel `.focused` é de navegação, não de rato: o gamepad marca-o.
        focus_visible.0 = true;
    }
    focus.0 = Some(current);

    let (mut dx, mut dy) = (0.0f32, 0.0f32);
    if pad.just_pressed(GamepadButton::DPadUp) {
        dy -= 1.0;
    }
    if pad.just_pressed(GamepadButton::DPadDown) {
        dy += 1.0;
    }
    if pad.just_pressed(GamepadButton::DPadLeft) {
        dx -= 1.0;
    }
    if pad.just_pressed(GamepadButton::DPadRight) {
        dx += 1.0;
    }
    if dx == 0.0 && dy == 0.0 {
        let x = pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        let y = pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        let len = x.abs().max(y.abs());
        if len > 0.65 && !*stick_armed {
            *stick_armed = true;
            if x.abs() >= y.abs() {
                dx = x.signum();
            } else {
                // UI: Y cresce para BAIXO; stick para cima é Y positivo.
                dy = -y.signum();
            }
        } else if len < 0.3 {
            *stick_armed = false;
        }
    }

    if dx != 0.0 || dy != 0.0 {
        if let Some(octant) = stick_octant(dx, dy)
            && let Ok(entity) = nav.navigate(octant)
        {
            focus.0 = Some(entity);
            focus_visible.0 = true;
        }
        return;
    }

    // A acciona o foco. O `Activate` do Bevy cobre botões/abas (Enter/Espaço
    // quando o foco está lá) e o `ToggleChecked` cobre o `Checkbox`; o
    // `Interaction::Pressed` sintético mantém o `viber.ui.clicked`/`UiClicks`
    // dos elementos que não são widgets Bevy. `collect_ui_clicks` corre depois
    // deste sistema no mesmo frame e vê `Changed<Interaction>`.
    if pad.just_pressed(GamepadButton::South) {
        commands.trigger(Activate { entity: current });
        commands.trigger(ToggleChecked { entity: current });
        if let Ok(mut interaction) = interactions.get_mut(current) {
            *interaction = Interaction::Pressed;
            if !synthetic.contains(&current) {
                synthetic.push(current);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stick_maps_to_compass_octants() {
        assert_eq!(stick_octant(0.0, -1.0), Some(CompassOctant::North));
        assert_eq!(stick_octant(1.0, 0.0), Some(CompassOctant::East));
        assert_eq!(stick_octant(0.0, 1.0), Some(CompassOctant::South));
        assert_eq!(stick_octant(-1.0, 0.0), Some(CompassOctant::West));
        assert_eq!(stick_octant(1.0, -1.0), Some(CompassOctant::NorthEast));
        assert_eq!(stick_octant(-1.0, 1.0), Some(CompassOctant::SouthWest));
        assert_eq!(stick_octant(0.0, 0.0), None);
    }
}
