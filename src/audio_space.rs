//! Som 3D com oclusão barata.
//!
//! O kira spatial (feature `spatial`) trata da pan e da queda com a distância.
//! A oclusão é um raycast câmara→fonte, no máximo [`OCCLUSION_RAYS`] por frame:
//! um volume no caminho baixa o ganho. A música de zona fica 2D.

use bevy::prelude::*;

/// Raios de oclusão por frame (rodam pelos emitters activos).
pub const OCCLUSION_RAYS: usize = 8;
/// Ganho quando o raio acerta geometria antes da fonte.
pub const OCCLUDED_GAIN: f32 = 0.35;

#[derive(Component)]
pub struct SpatialOneShot {
    pub pos: Vec3,
    /// Ganho extra da oclusão, 1 = livre.
    pub occlusion: f32,
}

/// Ganho final: distância linear até `radius` vezes a oclusão.
pub fn spatial_gain(distance: f32, radius: f32, occlusion: f32) -> f32 {
    let atten = (1.0 - distance / radius.max(1.0)).clamp(0.0, 1.0);
    atten * occlusion.clamp(0.0, 1.0)
}

/// `hit` é a distância ao primeiro volume. Se for menor que a fonte, está ocluído.
pub fn occlusion_of(hit: Option<f32>, source_distance: f32) -> f32 {
    match hit {
        Some(toi) if toi + 0.3 < source_distance => OCCLUDED_GAIN,
        _ => 1.0,
    }
}

/// Índice do próximo emitter a testar (roda, não testa todos no mesmo frame).
pub fn next_ray_cursor(cursor: usize, len: usize, budget: usize) -> (usize, usize) {
    if len == 0 || budget == 0 {
        return (cursor, 0);
    }
    let n = budget.min(len);
    ((cursor + n) % len, n)
}

pub struct AudioSpacePlugin;

impl Plugin for AudioSpacePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy_kira_audio::SpatialAudioPlugin);
        app.insert_resource(bevy_kira_audio::DefaultSpatialRadius { radius: 40.0 });
        app.add_systems(PostUpdate, ensure_listener);
    }
}

fn ensure_listener(
    mut commands: Commands,
    cameras: Query<
        Entity,
        (
            With<Camera3d>,
            Without<bevy_kira_audio::SpatialAudioReceiver>,
        ),
    >,
    existing: Query<Entity, With<bevy_kira_audio::SpatialAudioReceiver>>,
) {
    if existing.iter().next().is_some() {
        return;
    }
    if let Some(camera) = cameras.iter().next() {
        commands
            .entity(camera)
            .insert(bevy_kira_audio::SpatialAudioReceiver);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wall_between_ear_and_source_ducks_the_gain() {
        let open = spatial_gain(10.0, 40.0, occlusion_of(None, 10.0));
        let blocked = spatial_gain(10.0, 40.0, occlusion_of(Some(2.0), 10.0));
        assert!(blocked < open);
        assert!(occlusion_of(Some(9.9), 10.0) > 0.9);
    }

    #[test]
    fn the_ray_budget_rotates_instead_of_restarting() {
        let (cursor, n) = next_ray_cursor(0, 20, OCCLUSION_RAYS);
        assert_eq!(n, OCCLUSION_RAYS);
        assert_eq!(cursor, OCCLUSION_RAYS);
        let (cursor, _) = next_ray_cursor(16, 20, OCCLUSION_RAYS);
        assert_eq!(cursor, 4);
    }
}
