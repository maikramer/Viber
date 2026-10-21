//! Health bar dynamics: fill nodes mirror the player's `Health` every frame.

use bevy::prelude::*;

use crate::vitals::{Health, health_fraction};

/// Marker for the healthbar fill node (width mirrors the player's `Health`).
#[derive(Component)]
pub struct HudHealthFill;

/// Marker for the healthbar label node ("100/100").
#[derive(Component)]
pub struct HudHealthLabel;

/// Label text for the healthbar ("{cur}/{max}", hp rounded for display).
pub fn health_label_text(current: f32, max: f32) -> String {
    format!("{}/{}", current.round() as i32, max.round() as i32)
}

/// Mirror the hero's [`Health`] into the `healthbar` fill width and label.
/// Without a `Health` on the player it shows the default 100/100.
pub fn hud_health_sync(
    players: Query<&Health, With<crate::player::Player>>,
    mut fills: Query<&mut Node, With<HudHealthFill>>,
    mut labels: Query<&mut Text, With<HudHealthLabel>>,
) {
    let (current, max) = players
        .iter()
        .next()
        .map(|h| (h.current, h.max))
        .unwrap_or((100.0, 100.0));
    let percent = health_fraction(current, max) * 100.0;
    let wanted = Val::Percent(percent);
    for mut node in &mut fills {
        // Escrita gated: atribuir o mesmo valor mantinha o node em `Changed`
        // por frame (layout re-corrido à toa).
        if node.width != wanted {
            node.width = wanted;
        }
    }
    let text = health_label_text(current, max);
    for mut label in &mut labels {
        if label.0 != text {
            label.0 = text.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_label_text() {
        assert_eq!(health_label_text(100.0, 100.0), "100/100");
        assert_eq!(health_label_text(87.5, 100.0), "88/100"); // rounds
        assert_eq!(health_label_text(0.0, 100.0), "0/100");
    }
}
