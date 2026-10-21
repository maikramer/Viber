//! Mapa de ações: teclado, gamepad e rebind gravado no save.
//!
//! Os sistemas de jogo lêem [`ActionState`]. Sem o recurso (apps de teste que
//! chamam o movimento à mão) o teclado antigo continua a valer.

use std::collections::HashMap;

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Jump,
    Sprint,
    Attack,
    Interact,
    Lock,
    Menu,
}

impl Action {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "jump" => Some(Self::Jump),
            "sprint" => Some(Self::Sprint),
            "attack" => Some(Self::Attack),
            "interact" => Some(Self::Interact),
            "lock" => Some(Self::Lock),
            "menu" => Some(Self::Menu),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jump => "jump",
            Self::Sprint => "sprint",
            Self::Attack => "attack",
            Self::Interact => "interact",
            Self::Lock => "lock",
            Self::Menu => "menu",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub key: Option<String>,
    pub pad: Option<String>,
}

#[derive(Resource, Clone, Debug)]
pub struct ActionMap {
    pub bindings: HashMap<Action, Binding>,
}

impl Default for ActionMap {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        let bind = |key: &str, pad: &str| Binding {
            key: Some(key.into()),
            pad: Some(pad.into()),
        };
        bindings.insert(Action::Jump, bind("space", "south"));
        bindings.insert(Action::Sprint, bind("shift", "left_trigger"));
        bindings.insert(Action::Attack, bind("j", "west"));
        bindings.insert(Action::Interact, bind("e", "east"));
        bindings.insert(Action::Lock, bind("z", "left_trigger2"));
        bindings.insert(Action::Menu, bind("q", "start"));
        Self { bindings }
    }
}

impl ActionMap {
    pub fn rebind_key(&mut self, action: Action, key: &str) {
        self.bindings.entry(action).or_insert(Binding {
            key: None,
            pad: None,
        }).key = Some(key.to_string());
    }
}

#[derive(Resource, Debug, Default, Clone)]
pub struct ActionState {
    pub move_x: f32,
    pub forward: f32,
    pub jump_just: bool,
    pub sprint: bool,
    pub attack_just: bool,
    pub interact_just: bool,
    pub lock_just: bool,
    pub menu_just: bool,
}

impl ActionState {
    pub fn pressed(&self, action: Action) -> bool {
        match action {
            Action::Jump => self.jump_just,
            Action::Sprint => self.sprint,
            Action::Attack => self.attack_just,
            Action::Interact => self.interact_just,
            Action::Lock => self.lock_just,
            Action::Menu => self.menu_just,
        }
    }
}

fn key_of(name: &str) -> Option<KeyCode> {
    crate::luau::input_code_from_str(name).and_then(|code| match code {
        crate::luau::InputCode::Key(key) => Some(key),
        _ => None,
    })
}

fn pad_of(name: &str) -> Option<GamepadButton> {
    match name {
        "south" => Some(GamepadButton::South),
        "east" => Some(GamepadButton::East),
        "west" => Some(GamepadButton::West),
        "north" => Some(GamepadButton::North),
        "start" => Some(GamepadButton::Start),
        "left_trigger" => Some(GamepadButton::LeftTrigger),
        "left_trigger2" => Some(GamepadButton::LeftTrigger2),
        _ => None,
    }
}

pub fn gather_actions(
    keys: Res<ButtonInput<KeyCode>>,
    map: Res<ActionMap>,
    pads: Query<&Gamepad>,
    mut state: ResMut<ActionState>,
) {
    let mut move_x = 0.0;
    let mut forward = 0.0;
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        move_x += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        move_x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        forward += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        forward -= 1.0;
    }
    for pad in &pads {
        let x = pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0);
        let y = pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0);
        if x.abs() > 0.2 {
            move_x = x;
        }
        if y.abs() > 0.2 {
            forward = y;
        }
    }
    let just = |action: Action| -> bool {
        let Some(binding) = map.bindings.get(&action) else {
            return false;
        };
        let key = binding
            .key
            .as_deref()
            .and_then(key_of)
            .is_some_and(|k| keys.just_pressed(k));
        let pad = binding.pad.as_deref().and_then(pad_of).is_some_and(|button| {
            pads.iter().any(|pad| pad.just_pressed(button))
        });
        key || pad
    };
    let held = |action: Action| -> bool {
        let Some(binding) = map.bindings.get(&action) else {
            return false;
        };
        let key = binding
            .key
            .as_deref()
            .and_then(key_of)
            .is_some_and(|k| keys.pressed(k));
        let pad = binding.pad.as_deref().and_then(pad_of).is_some_and(|button| {
            pads.iter().any(|pad| pad.pressed(button))
        });
        key || pad
    };
    *state = ActionState {
        move_x,
        forward,
        jump_just: just(Action::Jump),
        sprint: held(Action::Sprint),
        attack_just: just(Action::Attack),
        interact_just: just(Action::Interact),
        lock_just: just(Action::Lock),
        menu_just: just(Action::Menu),
    };
}

pub struct InputMapPlugin;

impl Plugin for InputMapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActionMap>()
            .init_resource::<ActionState>()
            .add_systems(PreUpdate, gather_actions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebind_replaces_only_the_key() {
        let mut map = ActionMap::default();
        map.rebind_key(Action::Jump, "f");
        assert_eq!(map.bindings[&Action::Jump].key.as_deref(), Some("f"));
        assert!(map.bindings[&Action::Jump].pad.is_some());
    }

    #[test]
    fn action_names_roundtrip() {
        for name in ["jump", "sprint", "attack", "interact", "lock", "menu"] {
            let action = Action::parse(name).unwrap();
            assert_eq!(action.as_str(), name);
        }
    }
}
