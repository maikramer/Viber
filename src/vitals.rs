//! Saúde GENÉRICA de entidade (`Health`) e helpers puros de dano/cura.
//!
//! XP, níveis, talentos, veneno e morte são regras do JOGO (Lua no Vael,
//! `scripts/lib/rules.lua`). A engine só guarda um pool de HP que qualquer
//! sistema pode ler/escrever: criaturas, veículos, props com vida, o herói.

use bevy::prelude::*;

use crate::player::Player;

/// Default HP pool (also the HUD fallback when no `Health` exists yet).
pub const DEFAULT_HEALTH: f32 = 100.0;

/// HP pool, clamped to `0..=max` by [`apply_damage`] / [`heal_full`].
#[derive(Debug, Clone, Copy, PartialEq, Component)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            current: DEFAULT_HEALTH,
            max: DEFAULT_HEALTH,
        }
    }
}

/// Alvo hostil genérico. O mundo autora `hostile="true"`; o spawner de
/// criaturas (`<Creature role="enemy">`) também a insere. A engine NÃO infere
/// hostilidade pelo path do script.
#[derive(Debug, Clone, Copy, Default, Component)]
pub struct Hostile;

/// Garante `Health` no herói (o dano dos scripts e o HUD dependem disto).
pub fn ensure_player_vitals(
    players: Query<(Entity, Option<&Health>), With<Player>>,
    mut commands: Commands,
) {
    for (entity, health) in &players {
        if health.is_none() {
            commands.entity(entity).insert(Health::default());
        }
    }
}

/// Rede de segurança para um hostil autorado sem `health="…"`.
pub fn ensure_creature_vitals(
    creatures: Query<Entity, (With<Hostile>, Without<Health>, Without<Player>)>,
    mut commands: Commands,
) {
    for entity in &creatures {
        commands.entity(entity).insert(Health::default());
    }
}

/// Health bar fraction in `0..=1` for the UI fill (`0.0` when `max <= 0`).
pub fn health_fraction(current: f32, max: f32) -> f32 {
    if max <= 0.0 {
        return 0.0;
    }
    (current / max).clamp(0.0, 1.0)
}

/// Applies damage clamped to `0..=max`; negative `amount` (a heal) clamps at
/// `max` as well, so the pool never leaves its range.
///
/// `NaN`/`±inf` são ignorados: um dano não finito (via script) propagava-se ao
/// HP e o save gravava `null` — ilegível para sempre.
pub fn apply_damage(health: &mut Health, amount: f32) {
    if !amount.is_finite() {
        return;
    }
    health.current = (health.current - amount).clamp(0.0, health.max);
}

/// Full restore (debug `N`).
pub fn heal_full(health: &mut Health) {
    health.current = health.max;
}

/// Dano em área com falloff linear: cheio no centro → metade na borda.
/// `None` fora do raio. Primitiva genérica usada por `viber.radial_damage`;
/// a política (quem, quanto, knockback) é do script.
pub fn radial_damage(distance: f32, radius: f32, max_damage: f32) -> Option<f32> {
    if distance > radius {
        return None;
    }
    Some(max_damage * (1.0 - 0.5 * (distance / radius)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn test_health_fraction_basic() {
        assert!(approx(health_fraction(75.0, 100.0), 0.75));
        assert!(approx(health_fraction(0.0, 100.0), 0.0));
        assert!(approx(health_fraction(100.0, 100.0), 1.0));
    }

    #[test]
    fn test_health_fraction_guards() {
        assert!(approx(health_fraction(50.0, 0.0), 0.0));
        assert!(approx(health_fraction(50.0, -10.0), 0.0));
        assert!(approx(health_fraction(120.0, 100.0), 1.0));
        assert!(approx(health_fraction(-5.0, 100.0), 0.0));
    }

    #[test]
    fn test_apply_damage_clamps_at_zero() {
        let mut hp = Health {
            current: 25.0,
            max: 100.0,
        };
        apply_damage(&mut hp, 10.0);
        apply_damage(&mut hp, 10.0);
        apply_damage(&mut hp, 10.0);
        apply_damage(&mut hp, 10.0);
        assert!(approx(hp.current, 0.0));
        apply_damage(&mut hp, 10.0);
        assert!(approx(hp.current, 0.0));
    }

    #[test]
    fn test_heal_full_and_overheal_clamp_at_max() {
        let mut hp = Health {
            current: 30.0,
            max: 100.0,
        };
        heal_full(&mut hp);
        assert!(approx(hp.current, 100.0));
        apply_damage(&mut hp, -50.0);
        assert!(approx(hp.current, 100.0));
    }

    #[test]
    fn test_radial_damage_falloff() {
        assert_eq!(radial_damage(6.0, 5.0, 10.0), None);
        assert!(approx(radial_damage(0.0, 5.0, 10.0).unwrap(), 10.0));
        assert!(approx(radial_damage(5.0, 5.0, 10.0).unwrap(), 5.0));
    }
}
