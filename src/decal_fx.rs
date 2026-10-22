//! Decals de impacto genéricos (`<Decal>` futuro / `viber.decal`).
//!
//! O terreno já tinha `<GroundDecal>` (manchas drapeadas no heightfield),
//! mas um golpe precisa de deixar marca em QUALQUER superfície — parede,
//! rocha, chão. O Bevy 0.20 traz `ForwardDecal` (quad projectado, sem
//! sombras); aqui ele é exposto como uma fila de pedidos e um fade curto.
//!
//! A engine não conhece "sangue" nem "faísca": recebe textura, pose, tamanho
//! e duração. Quem decide o efeito é o jogo.

use bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt};
use bevy::prelude::*;

/// Tecto de decals vivos. São transparentes e cada um tem o seu material;
/// um combate intenso não pode criar centenas.
pub const MAX_DECALS: usize = 48;

/// Pedido de decal vindo de um script (`viber.decal`).
#[derive(Debug, Clone)]
pub struct DecalRequest {
    pub url: String,
    pub position: Vec3,
    pub normal: Vec3,
    pub size: f32,
    pub life: f32,
    pub color: Color,
}

/// Fila drenada uma vez por frame; sem o recurso o comando é no-op.
#[derive(Debug, Default, Resource)]
pub struct DecalQueue(pub Vec<DecalRequest>);

/// Decal com tempo de vida; o material é único (o alpha anima por instância).
#[derive(Debug, Component)]
pub struct DecalFade {
    pub age: f32,
    pub life: f32,
    pub base: Color,
    pub material: Handle<ForwardDecalMaterial<StandardMaterial>>,
}

fn decal_rotation(normal: Vec3) -> Quat {
    let normal = normal.try_normalize().unwrap_or(Vec3::Y);
    Quat::from_rotation_arc(Vec3::Y, normal)
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_decals(
    mut commands: Commands,
    mut queue: ResMut<DecalQueue>,
    server: Res<AssetServer>,
    mut materials: ResMut<Assets<ForwardDecalMaterial<StandardMaterial>>>,
    live: Query<Entity, With<DecalFade>>,
) {
    if queue.0.is_empty() {
        return;
    }
    let mut room = MAX_DECALS.saturating_sub(live.iter().count());
    for request in queue.0.drain(..) {
        if room == 0 || request.url.trim().is_empty() {
            continue;
        }
        let life = if request.life.is_finite() && request.life > 0.0 {
            request.life
        } else {
            1.5
        };
        let size = if request.size.is_finite() && request.size > 0.0 {
            request.size.clamp(0.05, 8.0)
        } else {
            0.7
        };
        let material = materials.add(ForwardDecalMaterial {
            base: StandardMaterial {
                base_color: request.color,
                // `AssetServer::load` não aceita o prefixo `/` que os XML
                // usam para os roots; o bridge do UI já o tira também.
                base_color_texture: Some(
                    server.load(request.url.trim_start_matches('/').to_string()),
                ),
                alpha_mode: AlphaMode::Blend,
                depth_bias: 0.02,
                cull_mode: None,
                ..Default::default()
            },
            extension: ForwardDecalMaterialExt {
                depth_fade_factor: 0.35,
            },
        });
        commands.spawn((
            Name::new("fx:decal"),
            Transform::from_translation(request.position)
                .with_rotation(decal_rotation(request.normal))
                .with_scale(Vec3::splat(size)),
            MeshMaterial3d(material.clone()),
            ForwardDecal,
            DecalFade {
                age: 0.0,
                life,
                base: request.color,
                material,
            },
        ));
        room -= 1;
    }
}

pub fn fade_decals(
    time: Res<Time>,
    mut commands: Commands,
    mut decals: Query<(Entity, &mut DecalFade)>,
    mut materials: ResMut<Assets<ForwardDecalMaterial<StandardMaterial>>>,
) {
    let dt = time.delta_secs();
    for (entity, mut decal) in &mut decals {
        decal.age += dt;
        let k = (decal.age / decal.life).clamp(0.0, 1.0);
        if k >= 1.0 {
            commands.entity(entity).despawn();
            materials.remove(&decal.material);
            continue;
        }
        // Fade nos últimos 55 % da vida: o impacto lê-se primeiro.
        let alpha = if k < 0.45 {
            1.0
        } else {
            1.0 - (k - 0.45) / 0.55
        };
        let rgba = decal.base.to_srgba();
        if let Some(mut material) = materials.get_mut(&decal.material) {
            material.base.base_color =
                Color::srgba(rgba.red, rgba.green, rgba.blue, rgba.alpha * alpha);
        }
    }
}

pub struct DecalFxPlugin;

impl Plugin for DecalFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DecalQueue>()
            .add_systems(Update, (spawn_decals, fade_decals));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decal_rotation_maps_y_to_the_normal() {
        let q = decal_rotation(Vec3::Y);
        assert!((q * Vec3::Y - Vec3::Y).length() < 1e-5);
        let q = decal_rotation(Vec3::X);
        assert!((q * Vec3::Y - Vec3::X).length() < 1e-5);
        // Normal degenerada cai no chão em vez de gerar NaN.
        let q = decal_rotation(Vec3::ZERO);
        assert!((q * Vec3::Y - Vec3::Y).length() < 1e-5);
    }
}
