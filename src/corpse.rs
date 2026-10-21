//! Marca de morte/cadáver.
//!
//! A ENGINE NÃO DECIDE o que a morte significa: `viber.entity_damage` só
//! aplica dano e emite `kill`; o jogo toca o clip, marca `viber.corpse(id)`
//! e despawna quando quiser. `Corpse` serve de filtro para IA/projéteis não
//! continuarem a tratar o corpo como vivo.

use bevy::prelude::*;

/// TTL de referência de um cadáver (s). O despawn é do jogo.
pub const CORPSE_LIFETIME: f32 = 1.4;

/// Morto/em fade. Quem o tem fica fora das queries de alvo vivo.
#[derive(Debug, Clone, Component)]
pub struct Corpse {
    pub timer: f32,
}
