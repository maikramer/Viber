//! Marcas de HUD que um script publica (`viber.markers`).
//!
//! A bússola, o minimapa e a seta de rumo leem daqui. O que é marco, missão
//! ou destino é decisão do jogo — a engine só desenha posições.

use bevy::prelude::*;

/// Ponto no mundo com um rótulo (seta de rumo).
#[derive(Debug, Clone)]
pub struct HudPoint {
    pub position: Vec3,
    pub label: String,
}

/// Marco no minimapa/bússola. `tower` escolhe o desenho de atalaia.
#[derive(Debug, Clone)]
pub struct HudBlip {
    pub position: Vec3,
    pub tower: bool,
}

/// Snapshot do frame. Substituído por inteiro a cada `viber.markers`.
#[derive(Debug, Clone, Resource, Default)]
pub struct HudMarkers {
    pub blips: Vec<HudBlip>,
    pub beacons: Vec<Vec3>,
    pub waypoint: Option<HudPoint>,
}
