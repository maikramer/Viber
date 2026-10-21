//! Hit-stop: congela o tempo VIRTUAL por alguns milissegundos no impacto.
//!
//! É um efeito de tempo genérico — a engine não sabe de golpes. O jogo pede
//! via `viber.hit_stop(segundos)`; o `A11y.hitstop` pode desligá-lo.

use bevy::prelude::*;

/// Escala do tempo virtual durante o hit-stop (45 %).
pub const HIT_STOP_SCALE: f32 = 0.45;
pub const HIT_STOP_NORMAL: f32 = 0.05;
pub const HIT_STOP_HEAVY: f32 = 0.07;
pub const HIT_STOP_KILL: f32 = 0.09;

/// Hit-stop em curso (tempo REAL; o resto do mundo corre a
/// [`HIT_STOP_SCALE`] da velocidade).
#[derive(Debug, Clone, Resource, Default)]
pub struct HitStop {
    pub timer: f32,
}

/// Velocidade base do tempo virtual, definida FORA do hit-stop (ex.: o
/// slow-mo do debug bridge via `viber.debug.set_time_scale`). O
/// [`hit_stop_system`] compõe `relative_speed = base × hit_stop_scale`
/// enquanto há hit-stop e não escreve nada fora dele — assim a base escolhida
/// por outra via persiste.
#[derive(Debug, Clone, Resource)]
pub struct BaseTimeScale(pub f32);

impl Default for BaseTimeScale {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Pede um hit-stop, mantendo o maior timer já em curso.
pub fn request_hit_stop(stop: &mut HitStop, duration: f32) {
    if duration.is_finite() && duration > stop.timer {
        stop.timer = duration;
    }
}

/// Escala do tempo virtual para um timer de hit-stop.
pub fn hit_stop_scale(timer: f32) -> f32 {
    if timer > 0.0 { HIT_STOP_SCALE } else { 1.0 }
}

/// Aplica/limpa o hit-stop no [`Time<Virtual>`].
pub fn hit_stop_system(
    real: Res<Time<Real>>,
    base: Res<BaseTimeScale>,
    mut stop: ResMut<HitStop>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut was_active: Local<bool>,
    a11y: Option<Res<crate::a11y::A11y>>,
) {
    if a11y.as_deref().is_some_and(|a| !crate::a11y::hitstop_allowed(a)) {
        stop.timer = 0.0;
    }
    let active = stop.timer > 0.0;
    if active {
        stop.timer -= real.delta_secs();
        if stop.timer <= 0.0 {
            stop.timer = 0.0;
        }
    }
    if active || *was_active {
        let scale = if active { HIT_STOP_SCALE } else { 1.0 };
        virtual_time.set_relative_speed(base.0 * scale);
    }
    *was_active = active;
}
