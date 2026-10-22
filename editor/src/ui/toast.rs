//! Transient notifications (save, errors) stacked above the status bar. The
//! status line keeps the last message; toasts make it visible without looking
//! at the footer.

use bevy::prelude::*;

use super::icons::{fa, icon_text};
use super::theme;

const TTL: f32 = 4.0;
const MAX: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub age: f32,
}

#[derive(Resource, Debug, Default)]
pub struct Toasts {
    pub items: Vec<Toast>,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ToastRoot;

pub struct ToastPlugin;

impl Plugin for ToastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Toasts>()
            .add_systems(Update, (expire_toasts, render_toasts).chain());
    }
}

/// Queues a notification (also usable from exclusive systems).
pub fn push(world: &mut World, kind: ToastKind, text: impl Into<String>) {
    let mut toasts = world.get_resource_or_insert_with(Toasts::default);
    toasts.items.push(Toast {
        text: text.into(),
        kind,
        age: 0.0,
    });
    let overflow = toasts.items.len().saturating_sub(MAX);
    if overflow > 0 {
        toasts.items.drain(0..overflow);
    }
}

fn expire_toasts(time: Res<Time<Real>>, mut toasts: ResMut<Toasts>) {
    if toasts.items.is_empty() {
        return;
    }
    let delta = time.delta_secs();
    for toast in &mut toasts.items {
        toast.age += delta;
    }
    toasts.items.retain(|toast| toast.age < TTL);
}

fn render_toasts(
    mut commands: Commands,
    toasts: Res<Toasts>,
    mut roots: Query<(Entity, &mut Node), With<ToastRoot>>,
    mut built: Local<bool>,
) {
    let Ok((root, mut node)) = roots.single_mut() else {
        return;
    };
    let display = if toasts.items.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    if node.display != display {
        node.display = display;
    }
    if !toasts.is_changed() && *built {
        return;
    }
    *built = true;
    commands.entity(root).despawn_related::<Children>();
    if toasts.items.is_empty() {
        return;
    }
    for toast in &toasts.items {
        let color = kind_color(toast.kind);
        commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    padding: UiRect::axes(px(12), px(7)),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(6)),
                    max_width: px(440),
                    ..default()
                },
                BackgroundColor(theme::BG_POPOVER),
                BorderColor::all(color),
                ChildOf(root),
            ))
            .with_children(|row| {
                row.spawn((
                    icon_text(kind_icon(toast.kind), 12.0, color),
                    Pickable::IGNORE,
                ));
                row.spawn((
                    Text::new(toast.text.clone()),
                    TextFont::from_font_size(12.0),
                    TextColor(theme::TEXT),
                    Pickable::IGNORE,
                ));
            });
    }
}

fn kind_icon(kind: ToastKind) -> char {
    match kind {
        ToastKind::Info => fa::INFO,
        ToastKind::Success => fa::CIRCLE_CHECK,
        ToastKind::Warn => fa::TRIANGLE_EXCLAMATION,
        ToastKind::Error => fa::CIRCLE_XMARK,
    }
}

fn kind_color(kind: ToastKind) -> Color {
    match kind {
        ToastKind::Info => Color::srgb(0.55, 0.78, 1.0),
        ToastKind::Success => Color::srgb(0.5, 0.88, 0.58),
        ToastKind::Warn => Color::srgb(0.95, 0.8, 0.4),
        ToastKind::Error => Color::srgb(0.95, 0.45, 0.45),
    }
}

/// The overlay, spawned in the editor root above the status bar.
pub fn toast_root() -> impl Scene {
    bsn! {
        ToastRoot
        GlobalZIndex(70)
        Node {
            position_type: PositionType::Absolute,
            right: px(12),
            bottom: px(theme::STATUS_PX + 10.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            row_gap: px(6),
            display: Display::None,
        }
        Children []
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_caps_and_expires() {
        let mut toasts = Toasts::default();
        for index in 0..(MAX + 3) {
            toasts.items.push(Toast {
                text: format!("t{index}"),
                kind: ToastKind::Info,
                age: 0.0,
            });
        }
        let overflow = toasts.items.len().saturating_sub(MAX);
        toasts.items.drain(0..overflow);
        assert_eq!(toasts.items.len(), MAX);
        assert_eq!(toasts.items[0].text, "t3");
        toasts.items[0].age = TTL;
        toasts.items.retain(|toast| toast.age < TTL);
        assert_eq!(toasts.items.len(), MAX - 1);
    }

    #[test]
    fn every_kind_has_a_distinct_icon_and_color() {
        let kinds = [
            ToastKind::Info,
            ToastKind::Success,
            ToastKind::Warn,
            ToastKind::Error,
        ];
        for (index, kind) in kinds.iter().enumerate() {
            for other in &kinds[index + 1..] {
                assert_ne!(kind_icon(*kind), kind_icon(*other));
                assert_ne!(kind_color(*kind), kind_color(*other));
            }
        }
    }
}
