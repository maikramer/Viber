//! Registro de pads resolvidos, partilhado pelo carve e pelo scatter.
//!
//! Um pad declarativo (`<TerrainPad>`) pode ter a cota automática: o
//! [`ResolvedPad`] é o spec já com essa cota amostrada, guardado para que o
//! bootstrap, as queries e o `<RockFeatures>` vejam o mesmo disco.

use bevy::math::Vec2;

/// Resolved pad: the declarative spec with the auto height filled in.
#[derive(Debug, Clone, PartialEq, bevy::prelude::Reflect)]
pub struct ResolvedPad {
    pub at: Vec2,
    pub size: Vec2,
    pub falloff: f32,
    pub corner_radius: f32,
    /// Target height in meters (auto pads have it sampled from the center).
    pub height: f32,
}
