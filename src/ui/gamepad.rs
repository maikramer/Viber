//! Navegação da UI declarativa por gamepad.
//!
//! A UI nasceu para rato/teclado: `Interaction` vinha do ponteiro e os
//! botões só respondiam a clique. Este módulo dá-lhe um foco de teclado
//! direccional (D-pad/stick esquerdo) e A para accionar — o mínimo para um
//! jogo de PC com comando não ficar preso no menu.
//!
//! Só corre com um menu/modal aberto (`MenusOpen::any()`); no mundo, o stick
//! pertence ao movimento do herói. O foco vive em [`UiGamepadFocus`] e a
//! classe CSS `focused` marca o botão — o jogo pode estilizá-la.

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};

use super::runtime::{UiClasses, UiDisabled, UiPointerNone};
use crate::menus::MenusOpen;
use crate::ui::interaction::{Button, Interaction};

/// Elemento com foco do gamepad. `None` = sem foco.
#[derive(Debug, Default, Resource)]
pub struct UiGamepadFocus(pub Option<Entity>);

fn node_center(node: &ComputedNode, transform: &UiGlobalTransform) -> Vec2 {
    let (_, _, translation) = transform.to_scale_angle_translation();
    translation + node.size * 0.5
}

/// Escolhe o candidato mais alinhado na direcção pedida: projecção no eixo
/// pretendido, penalizando o desvio perpendicular (um botão à frente e ao
/// lado perde para um mesmo à frente).
fn pick_direction(candidates: &[(Entity, Vec2)], from: Vec2, dir: Vec2) -> Option<Entity> {
    let mut best: Option<(Entity, f32)> = None;
    for (entity, center) in candidates {
        let delta = *center - from;
        let forward = delta.dot(dir);
        if forward <= 1.0 {
            continue; // atrás ou em cima do foco actual
        }
        let side = delta.dot(Vec2::new(-dir.y, dir.x)).abs();
        let score = forward + side * 1.8;
        if best.is_none_or(|(_, current)| score < current) {
            best = Some((*entity, score));
        }
    }
    best.map(|(entity, _)| entity)
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn drive_ui_gamepad_nav(
    menus: Option<Res<MenusOpen>>,
    pads: Query<&Gamepad>,
    mut focus: ResMut<UiGamepadFocus>,
    buttons: Query<
        (
            Entity,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&InheritedVisibility>,
            Option<&UiDisabled>,
            Option<&UiPointerNone>,
        ),
        With<Button>,
    >,
    mut classes: Query<&mut UiClasses>,
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
        if focus.0.take().is_some() {
            // Sair do menu: tira a classe do último foco.
            for (entity, _, _, _, _, _) in &buttons {
                if let Ok(mut classes) = classes.get_mut(entity) {
                    if classes.remove("focused") {
                        commands.entity(entity).insert(super::runtime::UiStyleDirty);
                    }
                }
            }
        }
        *stick_armed = false;
        return;
    }

    let mut candidates: Vec<(Entity, Vec2)> = Vec::new();
    for (entity, node, transform, visibility, disabled, pointer_none) in &buttons {
        if disabled.is_some() || pointer_none.is_some() {
            continue;
        }
        if visibility.is_some_and(|visible| !visible.get()) {
            continue;
        }
        if node.size.x <= 1.0 || node.size.y <= 1.0 {
            continue;
        }
        candidates.push((entity, node_center(node, transform)));
    }
    if candidates.is_empty() {
        // O foco anterior ainda pode existir (ficou `disabled`, por exemplo):
        // sem tirar a classe, o anel de foco ficava preso num botão morto.
        if let Some(previous) = focus.0.take()
            && let Ok(mut classes) = classes.get_mut(previous)
            && classes.remove("focused")
        {
            commands
                .entity(previous)
                .insert(super::runtime::UiStyleDirty);
        }
        return;
    }

    let Some(pad) = pads.iter().next() else {
        return;
    };
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

    // Foco actual pode ter desaparecido (modal fechou, lista reconstruída).
    if let Some(previous) = focus.0
        && !candidates.iter().any(|(e, _)| *e == previous)
    {
        focus.0 = None;
        if let Ok(mut classes) = classes.get_mut(previous)
            && classes.remove("focused")
        {
            commands
                .entity(previous)
                .insert(super::runtime::UiStyleDirty);
        }
    }

    // Sem foco: o primeiro da esquerda para a direita, de cima para baixo.
    // A classe entra JÁ neste frame (antes, o foco inicial só ficava visível
    // ao primeiro toque na direcção) e o fluxo continua, para o A premido no
    // mesmo frame em que o menu abriu não se perder.
    if focus.0.is_none() {
        focus.0 = candidates
            .iter()
            .min_by(|(_, a), (_, b)| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)))
            .map(|(entity, _)| *entity);
        if let Some(first) = focus.0
            && let Ok(mut classes) = classes.get_mut(first)
            && classes.add("focused")
        {
            commands.entity(first).insert(super::runtime::UiStyleDirty);
        }
    }

    let current = focus.0.and_then(|entity| {
        candidates
            .iter()
            .find(|(candidate, _)| *candidate == entity)
            .map(|(_, center)| *center)
    });
    let Some(from) = current else { return };

    if dx != 0.0 || dy != 0.0 {
        // O eixo do UI cresce para baixo (Y para baixo), logo o stick
        // esquerdo (Y para cima) é invertido ao construir a direcção.
        let dir = Vec2::new(dx, dy).normalize_or_zero();
        if let Some(next) = pick_direction(&candidates, from, dir) {
            if let Ok(mut classes) = classes.get_mut(next) {
                if classes.add("focused") {
                    commands.entity(next).insert(super::runtime::UiStyleDirty);
                }
            }
            if let Some(previous) = focus.0
                && previous != next
                && let Ok(mut classes) = classes.get_mut(previous)
            {
                if classes.remove("focused") {
                    commands
                        .entity(previous)
                        .insert(super::runtime::UiStyleDirty);
                }
            }
            focus.0 = Some(next);
        }
        return;
    }

    // A acciona o foco. `collect_ui_clicks` corre depois deste sistema no
    // mesmo frame e vê `Changed<Interaction>`.
    if pad.just_pressed(GamepadButton::South)
        && let Some(entity) = focus.0
        && let Ok(mut interaction) = interactions.get_mut(entity)
    {
        *interaction = Interaction::Pressed;
        // Enquanto o rato estiver premido a lista não é drenada; sem o
        // guard, premir A repetidamente acumulava a mesma entidade.
        if !synthetic.contains(&entity) {
            synthetic.push(entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_picks_forward_and_ignores_behind() {
        let a = Entity::from_raw_u32(1).unwrap();
        let b = Entity::from_raw_u32(2).unwrap();
        let c = Entity::from_raw_u32(3).unwrap();
        let from = Vec2::new(100.0, 100.0);
        let candidates = vec![
            (a, Vec2::new(100.0, 180.0)), // abaixo
            (b, Vec2::new(260.0, 100.0)), // à direita
            (c, Vec2::new(100.0, 20.0)),  // acima
        ];
        assert_eq!(
            pick_direction(&candidates, from, Vec2::new(0.0, 1.0)),
            Some(a),
            "baixo escolhe o que está abaixo"
        );
        assert_eq!(
            pick_direction(&candidates, from, Vec2::new(1.0, 0.0)),
            Some(b),
            "direita escolhe o que está à direita"
        );
        assert_eq!(
            pick_direction(&candidates, from, Vec2::new(0.0, -1.0)),
            Some(c),
            "cima escolhe o que está acima"
        );
        assert_eq!(
            pick_direction(&candidates, from, Vec2::new(-1.0, 0.0)),
            None,
            "nada à esquerda"
        );
    }
}
