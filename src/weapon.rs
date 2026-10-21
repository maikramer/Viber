//! Arma na mão do herói — APRESENTAÇÃO.
//!
//! Anexa o GLB ao osso `hand_r`; não decide golpes nem dano (isso é Lua). O
//! ciclo de armas foi removido: o [V] do jogo pertence ao lock-on.

use bevy::prelude::*;

use crate::player::Player;

/// Escala de reprodução dos clips de ataque/colheita (1.4× = golpe legível).
pub const ATTACK_TIME_SCALE: f32 = 1.4;
/// Fração do clip em que o impacto aterra (fallback de clips sem tabela).
pub const SWING_IMPACT_FRACTION: f32 = 0.35;

#[derive(Debug, Clone, Resource, Default)]
pub struct HeldWeapon {
    /// Índice em [`WEAPON_TABLE`].
    pub idx: usize,
    /// Osso da mão do herói (preenchido na primeira detecção).
    pub bone: Option<Entity>,
    /// Entidade da arma atual (filha do osso).
    pub current: Option<Entity>,
    /// Nomes de nó já varridos (evita re-busca por frame).
    pub searched: bool,
}

/// (url, pos, rot XYZ rad, scale, rótulo)
#[allow(clippy::type_complexity)]
pub const WEAPON_TABLE: [(&str, [f32; 3], [f32; 3], f32, &str); 3] = [
    (
        "assets/meshes/props/sword_hero_lod0.glb",
        [0.12, 0.04, 0.04],
        [-1.33, 12.71, 0.96],
        1.0,
        "espada",
    ),
    (
        "assets/meshes/props/axe_lod0.glb",
        [0.23, 0.11, 0.01],
        [2.98, 12.71, std::f32::consts::FRAC_PI_2],
        1.0,
        "machado",
    ),
    (
        "assets/meshes/props/spear_lod0.glb",
        [0.2, 0.01, 0.04],
        [-1.33, 12.71, 0.96],
        1.0,
        "lança",
    ),
];

/// Espada na mão (sem ciclo): o app do jogo só precisa da apresentação.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn ensure_held_weapon(
    commands: Commands,
    asset_server: Res<AssetServer>,
    heroes: Query<Entity, With<Player>>,
    names: Query<&Name>,
    children: Query<&Children>,
    held: ResMut<HeldWeapon>,
) {
    sync_held_weapon(commands, asset_server, heroes, names, children, held);
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn sync_held_weapon(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    heroes: Query<Entity, With<Player>>,
    names: Query<&Name>,
    children: Query<&Children>,
    mut held: ResMut<HeldWeapon>,
) {
    if held.bone.is_none() && !held.searched {
        let Ok(hero) = heroes.single() else { return };
        held.bone = find_hand_bone(hero, &children, &names);
        // O GLB chega depois do primeiro frame. Só desiste quando a mão existe.
        held.searched = held.bone.is_some();
    }
    let Some(bone) = held.bone else { return };
    if held.current.is_some() {
        return;
    }
    let (url, pos, rot, scale, _label) = WEAPON_TABLE[held.idx.min(WEAPON_TABLE.len() - 1)];
    let handle = crate::meshopt::load_gltf(&asset_server, url.to_owned());
    let mut transform = Transform::from_translation(Vec3::new(pos[0], pos[1], pos[2]));
    transform.rotation = Quat::from_euler(EulerRot::XYZ, rot[0], rot[1], rot[2]);
    transform.scale = Vec3::splat(scale);
    let spawned = commands
        .spawn((
            transform,
            Visibility::Inherited,
            crate::recipes::spawn::GltfScenePending::single(handle),
        ))
        .id();
    commands.entity(bone).add_child(spawned);
    held.current = Some(spawned);
}

/// Procura o osso da mão por subárvore (mesmos candidates do held-item.ts).
/// `pub(crate)`: o harvest.rs anexa as ferramentas de colheita ao mesmo osso.
pub(crate) fn find_hand_bone(
    root: Entity,
    children: &Query<&Children>,
    names: &Query<&Name>,
) -> Option<Entity> {
    if let Ok(name) = names.get(root) {
        let name = name.to_ascii_lowercase();
        let is_hand = matches!(
            name.as_str(),
            "hand_r" | "righthand" | "right_hand" | "right hand"
        ) || (name.contains("hand_r") && !name.contains("finger"))
            || (name.contains("righthand") && !name.contains("finger"));
        if is_hand {
            return Some(root);
        }
    }
    for child in children.get(root).ok()?.iter() {
        if let Some(found) = find_hand_bone(child, children, names) {
            return Some(found);
        }
    }
    None
}
