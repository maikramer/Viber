//! Specs de partículas e pequenas utilidades de FX partilhadas pelo jogo.
//!
//! São receitas de apresentação (bursts de impacto, orientação de cast e um
//! roll determinístico). Nenhuma regra de combate vive aqui — o dano, a
// chain e o alvo são do Lua.

use bevy::prelude::*;

/// Spec de burst afinado por cima do preset.
pub fn impact_spec(
    preset: &str,
    size: (f32, f32),
    life: (f32, f32),
    speed: (f32, f32),
    color: Option<[f32; 3]>,
) -> crate::recipes::ParticleSpec {
    crate::recipes::ParticleSpec {
        preset: preset.to_string(),
        emission_rate: None,
        life: Some(life),
        speed: Some(speed),
        size: Some(size),
        color,
        shape_radius: None,
        looping: false,
        world_space: false,
    }
}

/// Sparks do impacto: poucas, pequenas, curtas.
pub fn hit_sparks_spec() -> crate::recipes::ParticleSpec {
    impact_spec("sparks", (0.04, 0.1), (0.16, 0.32), (1.4, 3.2), None)
}

/// Vira o MODELO para `dir` (XZ) de uma vez (slerp): snap de cast.
pub fn snap_facing(transform: &mut Transform, dir: Vec3, factor: f32) {
    if dir.length_squared() < 1e-6 {
        return;
    }
    let wanted = crate::player::facing_rotation(dir);
    transform.rotation = transform.rotation.slerp(wanted, factor.clamp(0.0, 1.0));
}

/// Roll pseudo-aleatório determinístico (0..1), derivado do tempo da engine.
pub fn pseudo_roll(time: &Time) -> f32 {
    (time.elapsed_secs_f64() * 7919.0).fract() as f32
}
