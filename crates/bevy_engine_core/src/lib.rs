//! Shared contracts for the `bevy_*` engine packages.
//!
//! This crate is the bottom of the dependency graph: it has no engine services
//! and no game types. Packages depend on it instead of reaching into each
//! other for a marker, a schedule set or a resource.
//!
//! The types here were moved out of the `viber` monolith in Phase 1; the root
//! re-exports them so existing `viber::…` paths keep working.

pub mod rng;
pub mod script_kind;

pub use rng::Rng;
pub use script_kind::{KILL_XP, kill_xp, script_kind};

use bevy::math::Vec3;
use bevy::prelude::*;

/// Marker for the entity driven by a player controller.
///
/// Engine services (terrain streaming, navigation, interaction) should query
/// this marker instead of the concrete controller component. A game attaches
/// it to whichever entity owns its controller; the Viber runtime attaches it
/// to `player::Player` and the Vael attaches it to its own controller.
#[derive(Debug, Default, Clone, Copy, Component, Reflect)]
#[reflect(Component, Default)]
pub struct PlayerBody;

/// UI/menu state that suppresses movement and gameplay input.
///
/// A game can extend the meaning of each flag; the engine only reads
/// [`Self::any`] to gate input and gameplay systems.
#[derive(Debug, Clone, Default, Resource, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::menus"]
pub struct MenusOpen {
    pub modal: bool,
    pub shop: bool,
    /// Full-screen travel/fast-travel panel.
    pub travel: bool,
    /// Regional map.
    pub map: bool,
    /// A cinematic is playing.
    pub cinematic: bool,
}

impl MenusOpen {
    pub fn any(&self) -> bool {
        self.modal || self.shop || self.travel || self.map || self.cinematic
    }
}

/// A toast requested by a script (`viber.toast`) or by gameplay code.
///
/// The HUD consumes it; the bridge logs it. Kept in `core` so the save layer
/// can load/save pending toasts without depending on the Luau runtime.
#[derive(Debug, Clone, bevy::ecs::message::Message)]
pub struct ScriptToast(pub String);

/// Bus volumes from `<AudioMixer master music sfx voice ambience>`.
///
/// The resource always exists: a derived `Default` with zeroed fields would
/// silence every bus, so [`Default`] is implemented explicitly with `1.0`.
#[derive(Debug, Clone, Resource, Reflect)]
#[reflect(Resource)]
// Preserva o TypePath do BRP/`viber.ecs` depois da extração.
#[type_path = "viber::music"]
pub struct AudioMixerSettings {
    pub master: f32,
    pub music: f32,
    pub sfx: f32,
    pub voice: f32,
    pub ambience: f32,
}

impl Default for AudioMixerSettings {
    fn default() -> Self {
        Self {
            master: 1.0,
            music: 1.0,
            sfx: 1.0,
            voice: 1.0,
            ambience: 1.0,
        }
    }
}

/// System ordering inside one UI frame: collect → build → bind → script →
/// style.
///
/// Explicit sets, because the order is the contract: a script has to see this
/// frame's bindings, and the restyle has to run after whatever the script
/// changed, or every class toggle would land a frame late.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum UiSet {
    /// Snapshot gameplay state into the UI data and list sources.
    Collect,
    /// Rebuild data-driven repeaters whose source changed.
    Build,
    /// Push bindings into elements and publish the script view.
    Bind,
    /// Apply the mutations scripts queued.
    Script,
    /// Recompute styles and mirror widget values into their child nodes.
    Style,
    /// The second half of styling (widget syncs and motion); the systems
    /// tuple in one chain has a ceiling and the style pass outgrew it.
    StyleLate,
}

/// Height query used by services that must not depend on the terrain crate.
pub trait TerrainQuery: Send + Sync + 'static {
    /// Terrain height at `(x, z)`, or `None` outside the field / over water.
    fn height_at(&self, x: f32, z: f32) -> Option<f32>;
}

/// Path query used by services that must not depend on the navigation crate.
pub trait NavQuery: Send + Sync + 'static {
    /// A world-space path from `from` to `to`, or `None` when there is no
    /// navmesh / no path.
    fn path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>>;
}
