//! Vegetação de um só mesh: tiles fundidos, no caminho da relva.
//!
//! Só entra quem tem [`SimpleFoliage`] (um `Mesh3d`, sem script, sem animação).
//! Árvores com vários meshes e props com collider ficam entidades. O gate é
//! `VIBER_FOLIAGE_BATCH=0` para desligar; ligado por omissão.

use std::collections::HashMap;

use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

pub const TILE: f32 = 16.0;

pub fn tile_of(x: f32, z: f32) -> IVec2 {
    IVec2::new((x / TILE).floor() as i32, (z / TILE).floor() as i32)
}

/// Um pé de vegetação que cabe num draw instanciado/fundido.
pub fn batch_wanted(mesh_count: usize, has_script: bool, has_animator: bool, has_collider: bool) -> bool {
    mesh_count == 1 && !has_script && !has_animator && !has_collider
}

pub fn batch_enabled() -> bool {
    !matches!(
        std::env::var("VIBER_FOLIAGE_BATCH").ok().as_deref(),
        Some("0" | "false" | "off")
    )
}

#[derive(Component)]
pub struct SimpleFoliage {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
}

#[derive(Component)]
pub struct FoliageTile {
    pub key: IVec2,
    pub mesh_id: AssetId<Mesh>,
}

/// Acrescenta um mesh transformado a buffers já em construção.
pub fn append_mesh(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    src_pos: &[[f32; 3]],
    src_nrm: &[[f32; 3]],
    src_uv: &[[f32; 2]],
    src_idx: &[u32],
    xform: Mat4,
) {
    let base = positions.len() as u32;
    let normal_mat = xform.inverse().transpose();
    for p in src_pos {
        let v = xform.transform_point3(Vec3::from_array(*p));
        positions.push(v.to_array());
    }
    for n in src_nrm {
        let v = normal_mat.transform_vector3(Vec3::from_array(*n)).normalize_or_zero();
        normals.push(v.to_array());
    }
    if normals.len() < positions.len() {
        normals.resize(positions.len(), [0.0, 1.0, 0.0]);
    }
    if src_uv.len() == src_pos.len() {
        uvs.extend_from_slice(src_uv);
    } else {
        uvs.resize(positions.len(), [0.0, 0.0]);
    }
    for i in src_idx {
        indices.push(base + *i);
    }
}

fn triples(values: &VertexAttributeValues) -> Option<Vec<[f32; 3]>> {
    match values {
        VertexAttributeValues::Float32x3(v) => Some(v.clone()),
        _ => None,
    }
}

pub fn batch_tiles(
    mut commands: Commands,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    meshes: Res<Assets<Mesh>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    foliage: Query<(Entity, &GlobalTransform, &SimpleFoliage, &Visibility)>,
    tiles: Query<(Entity, &FoliageTile)>,
) {
    if !batch_enabled() {
        return;
    }
    let Some(cam) = camera.iter().next() else {
        return;
    };
    let eye = cam.translation();
    let mut groups: HashMap<(IVec2, AssetId<Mesh>), Vec<(Entity, Mat4, Handle<Mesh>, Handle<StandardMaterial>)>> =
        HashMap::new();
    for (entity, xf, simple, vis) in &foliage {
        if *vis == Visibility::Hidden {
            continue;
        }
        let pos = xf.translation();
        if pos.distance(eye) > 80.0 {
            continue;
        }
        let key = (tile_of(pos.x, pos.z), simple.mesh.id());
        groups.entry(key).or_default().push((
            entity,
            xf.to_matrix(),
            simple.mesh.clone(),
            simple.material.clone(),
        ));
    }
    for ((key, mesh_id), members) in groups {
        if members.len() < 2 {
            continue;
        }
        if tiles.iter().any(|(_, tile)| tile.key == key && tile.mesh_id == mesh_id) {
            continue;
        }
        let Some(src) = meshes.get(&members[0].2) else {
            continue;
        };
        let Some(src_pos) = src.attribute(Mesh::ATTRIBUTE_POSITION).and_then(triples) else {
            continue;
        };
        let src_nrm = src
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .and_then(triples)
            .unwrap_or_default();
        let src_uv = match src.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(VertexAttributeValues::Float32x2(v)) => v.clone(),
            _ => Vec::new(),
        };
        let src_idx: Vec<u32> = match src.indices() {
            Some(Indices::U32(v)) => v.clone(),
            Some(Indices::U16(v)) => v.iter().map(|i| *i as u32).collect(),
            None => (0..src_pos.len() as u32).collect(),
        };
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        for (_, matrix, _, _) in &members {
            append_mesh(
                &mut positions,
                &mut normals,
                &mut uvs,
                &mut indices,
                &src_pos,
                &src_nrm,
                &src_uv,
                &src_idx,
                *matrix,
            );
        }
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U32(indices));
        let handle = mesh_assets.add(mesh);
        let material = members[0].3.clone();
        commands.spawn((
            FoliageTile { key, mesh_id },
            Mesh3d(handle),
            MeshMaterial3d(material),
            Transform::default(),
        ));
        for (entity, _, _, _) in &members {
            commands.entity(*entity).insert(Visibility::Hidden);
        }
    }
    let _ = tiles;
}

/// Impostor: para lá de 150 m e antes do cull, um cartaz sem sombra.
pub const IMPOSTOR_NEAR: f32 = 150.0;

pub fn impostor_band(distance: f32, cull: f32) -> bool {
    distance >= IMPOSTOR_NEAR && distance < cull
}

#[derive(Component)]
pub struct ImpostorCard {
    pub source: Entity,
}

pub fn impostors_enabled() -> bool {
    !matches!(
        std::env::var("VIBER_IMPOSTORS").ok().as_deref(),
        Some("0" | "false" | "off")
    )
}

pub struct FoliagePlugin;

impl Plugin for FoliagePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, batch_tiles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_single_bare_mesh_is_batched() {
        assert!(batch_wanted(1, false, false, false));
        assert!(!batch_wanted(2, false, false, false));
        assert!(!batch_wanted(1, true, false, false));
        assert!(!batch_wanted(1, false, false, true));
    }

    #[test]
    fn tiles_are_sixteen_metres() {
        assert_eq!(tile_of(0.0, 0.0), IVec2::ZERO);
        assert_eq!(tile_of(16.0, -1.0), IVec2::new(1, -1));
    }

    #[test]
    fn append_offsets_indices() {
        let mut p = Vec::new();
        let mut n = Vec::new();
        let mut u = Vec::new();
        let mut i = Vec::new();
        let src_p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let src_i = [0, 1, 2];
        append_mesh(
            &mut p, &mut n, &mut u, &mut i, &src_p, &[], &[], &src_i, Mat4::from_translation(Vec3::X),
        );
        append_mesh(
            &mut p, &mut n, &mut u, &mut i, &src_p, &[], &[], &src_i, Mat4::IDENTITY,
        );
        assert_eq!(p.len(), 6);
        assert_eq!(i[3], 3);
        assert!((p[0][0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn impostor_sits_between_lod_and_cull() {
        assert!(!impostor_band(80.0, 320.0));
        assert!(impostor_band(200.0, 320.0));
        assert!(!impostor_band(400.0, 320.0));
    }
}
