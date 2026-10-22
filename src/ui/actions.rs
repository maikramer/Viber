//! Menu actions: what a button in the declarative UI actually *does*.
//!
//! Scripts raise an action by name — `viber.ui.action("learn", "vitality1")` —
//! and one handler per domain applies it. Keeping them here rather than in the
//! scripting module means the whole menu (structure, look, behaviour and the
//! gameplay it triggers) lives in `src/ui`, and a world can add a button
//! without touching the sandbox.
//!
//! Actions are messages, not direct mutation: a script runs mid-frame with the
//! Lua VM borrowed, so anything touching gameplay resources has to happen
//! afterwards.

use bevy::prelude::*;

/// One action raised by the UI.
#[derive(Debug, Clone, bevy::ecs::message::Message)]
pub struct UiAction {
    /// Action name (`save`, `load`, `buy`, … — o jogo interpreta).
    pub name: String,
    /// Its argument — a skill id, an item id, or empty.
    pub arg: String,
}

/// Ações da UI reclamadas por scripts (`viber.ui.own_action("buy")`): chegam
/// a Lua via `viber.events()` (`{type="ui_action"}`) e a lógica vive no jogo.
#[derive(Debug, Default, Resource)]
pub struct UiActionOwners(pub std::collections::HashSet<String>);

/// Encaminha TODAS as ações da UI para a fila de eventos dos scripts
/// (`viber.events()` → `{type="ui_action", name=..., arg=...}`). Os handlers
/// nativos correm à mesma — a ação é lida por ambos os consumidores.
pub fn forward_ui_actions(
    mut actions: bevy::ecs::message::MessageReader<UiAction>,
    mut events: Option<ResMut<crate::luau::ScriptEventQueue>>,
) {
    // `as_mut` pelo mesmo motivo do `apply_shop_actions`: a fila só fica
    // alterada quando há ação para encaminhar.
    let Some(events) = events.as_mut() else {
        return;
    };
    for action in actions.read() {
        events.push(crate::luau::ScriptGameEvent::UiAction {
            name: action.name.clone(),
            arg: action.arg.clone(),
        });
    }
}
