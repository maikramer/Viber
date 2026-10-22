//! Vegetação e props estáticos: tiles fundidos, no caminho da relva.
//!
//! Entram [`SimpleFoliage`]s armados a partir de duas fontes:
//! * `<GltfScene>` estático sem script/física/props e com **um** mesh;
//! * instâncias de spawner/vegetação estáticas **sem ladder de LOD**, com
//!   qualquer número de meshes (tronco + folhas). A vegetação `.vinst` entra
//!   por aqui — era o maior conjunto de draw calls sem batch.
//!
//! O grupo é `(tile de 16 m, mesh, material)`: dois props com a mesma mesh e
//! materiais diferentes não partilham o draw. Os tiles fundidos têm cull com
//! histerese (`BATCH_CULL`) e morrem quando as fontes despawnam (troca de
//! cena/streaming). Gate: `VIBER_FOLIAGE_BATCH=0`.

use std::collections::HashMap;

use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

pub const TILE: f32 = 16.0;

pub fn tile_of(x: f32, z: f32) -> IVec2 {
    IVec2::new((x / TILE).floor() as i32, (z / TILE).floor() as i32)
}

/// Um pé de vegetação que cabe num draw instanciado/fundido.
pub fn batch_wanted(
    mesh_count: usize,
    has_script: bool,
    has_animator: bool,
    has_collider: bool,
) -> bool {
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

/// `<GltfScene>` estático sem script/física: candidato a entrar no batch.
/// [`arm_candidates`] confirma o resto (sem colisor em nenhum ancestral)
/// quando a cena do glTF já spawnou.
///
/// `multi` distingue os dois caminhos:
/// * `false` — `<GltfScene>` normal: só entra se a cena tiver UM mesh
///   (o comportamento conservador de sempre; há LOD por cima).
/// * `true` — instância de spawner/vegetação: a cena pode ter vários meshes
///   (tronco + folhas) e cada um entra no batch. A vegetação não tem ladder
///   de LOD, por isso não há troca de subárvore a invalidar o tile fundido.
#[derive(Component)]
pub struct FoliageCandidate {
    pub multi: bool,
}

#[derive(Component)]
pub struct FoliageTile {
    pub key: IVec2,
    pub mesh_id: AssetId<Mesh>,
    pub material_id: AssetId<StandardMaterial>,
    /// Meshes de origem escondidos por este tile. Ao fazer cull do tile,
    /// voltam a `Inherited` para poderem ser re-batchados ao aproximar.
    pub members: Vec<Entity>,
    /// Mesh fundido (para o remover do `Assets<Mesh>` no cull).
    pub mesh: Handle<Mesh>,
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
        let v = normal_mat
            .transform_vector3(Vec3::from_array(*n))
            .normalize_or_zero();
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

pub const BATCH_RADIUS: f32 = 80.0;
/// Histerese do cull dos tiles fundidos: um tile criado a 80 m não morre no
/// frame seguinte; só quando a câmara se afasta de vez. 96 m (e não 112)
/// mantém a vegetação perto do `CullDistance` de 80 m que a fonte já tinha.
pub const BATCH_CULL: f32 = 96.0;

fn tile_center(key: IVec2) -> Vec2 {
    Vec2::new(
        key.x as f32 * TILE + TILE * 0.5,
        key.y as f32 * TILE + TILE * 0.5,
    )
}

pub fn batch_tiles(
    mut commands: Commands,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    editor_camera: Query<&GlobalTransform, With<crate::editor_camera::EditorCamera>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    foliage: Query<(Entity, &GlobalTransform, &SimpleFoliage, &Visibility)>,
    live: Query<(), With<SimpleFoliage>>,
    tiles: Query<(Entity, &FoliageTile)>,
) {
    if !batch_enabled() {
        return;
    }
    // Em `--camera free` há duas Camera3d; a ordem da query escolhia a
    // câmara do editor e batchava a folhagem do lado errado.
    let Some(cam) = crate::editor_camera::active_camera_eye(&editor_camera, &camera) else {
        return;
    };
    let eye = cam.translation();
    let eye_xz = Vec2::new(eye.x, eye.z);

    // 1. Cull dos tiles fundidos que ficaram para trás. Sem isto, caminhar
    //    pelo mapa acumulava uma mesh fundida por tile para sempre e as
    //    fontes escondidas nunca voltavam a ser candidatas.
    let mut culled = Vec::new();
    for (entity, tile) in &tiles {
        // Fonte despawnada (troca de cena, streaming) ou câmara longe: o
        // tile fundido não pode sobreviver à cena que o gerou.
        let stale = tile.members.iter().any(|member| !live.contains(*member));
        if stale || tile_center(tile.key).distance(eye_xz) > BATCH_CULL {
            culled.push((entity, tile.members.clone(), tile.mesh.clone()));
        }
    }
    for (entity, members, mesh) in culled {
        mesh_assets.remove(&mesh);
        for member in members {
            commands.entity(member).try_insert(Visibility::Inherited);
        }
        commands.entity(entity).despawn();
    }

    // 2. Agrupa por (tile, mesh, material). O material entra na chave: dois
    //    props com a mesma mesh mas materiais distintos não podem partilhar
    //    o draw (o primeiro material pintava os dois).
    let mut groups: HashMap<
        (IVec2, AssetId<Mesh>, AssetId<StandardMaterial>),
        Vec<(Entity, Mat4, Handle<Mesh>, Handle<StandardMaterial>)>,
    > = HashMap::new();
    for (entity, xf, simple, vis) in &foliage {
        if *vis == Visibility::Hidden {
            continue;
        }
        let pos = xf.translation();
        if pos.distance(eye) > BATCH_RADIUS {
            continue;
        }
        let key = (
            tile_of(pos.x, pos.z),
            simple.mesh.id(),
            simple.material.id(),
        );
        groups.entry(key).or_default().push((
            entity,
            xf.to_matrix(),
            simple.mesh.clone(),
            simple.material.clone(),
        ));
    }
    for ((key, mesh_id, material_id), members) in groups {
        if members.len() < 2 {
            continue;
        }
        if tiles.iter().any(|(_, tile)| {
            tile.key == key && tile.mesh_id == mesh_id && tile.material_id == material_id
        }) {
            continue;
        }
        let Some(src) = mesh_assets.get(&members[0].2) else {
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
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U32(indices));
        let handle = mesh_assets.add(mesh);
        let material = members[0].3.clone();
        let member_entities: Vec<Entity> = members.iter().map(|(e, _, _, _)| *e).collect();
        commands.spawn((
            Name::new("batch:foliage"),
            FoliageTile {
                key,
                mesh_id,
                material_id,
                members: member_entities.clone(),
                mesh: handle.clone(),
            },
            Mesh3d(handle),
            MeshMaterial3d(material),
            Transform::default(),
        ));
        for entity in member_entities {
            commands.entity(entity).insert(Visibility::Hidden);
        }
    }
}

/// Confirma os candidatos quando a cena do glTF já resolveu: nenhum
/// ancestral com script/animador/colisor/corpo. No caminho `multi`
/// (spawners/vegetação) entra **cada** mesh da cena; no caminho normal
/// (`<GltfScene>`) continua a exigir um só mesh. Insere o [`SimpleFoliage`]
/// NO mesh (é ele que o batch esconde) e larga o marcador.
#[allow(clippy::type_complexity)]
pub fn arm_candidates(
    mut commands: Commands,
    candidates: Query<(Entity, &FoliageCandidate)>,
    ready: Query<(), With<bevy::world_serialization::WorldAssetRoot>>,
    children: Query<&Children>,
    meshes: Query<(Entity, &Mesh3d, &MeshMaterial3d<StandardMaterial>)>,
    parents: Query<&ChildOf>,
    blocked: Query<(
        Option<&crate::luau::LuaScriptRef>,
        Option<&crate::animation::CharacterAnimator>,
        Option<&bevy_rapier3d::prelude::Collider>,
        Option<&bevy_rapier3d::prelude::RigidBody>,
        Option<&crate::physics::PendingCollider>,
    )>,
) {
    fn collect_meshes(
        root: Entity,
        children: &Query<&Children>,
        meshes: &Query<(Entity, &Mesh3d, &MeshMaterial3d<StandardMaterial>)>,
        out: &mut Vec<(Entity, Handle<Mesh>, Handle<StandardMaterial>)>,
    ) {
        if let Ok((entity, mesh, material)) = meshes.get(root) {
            out.push((entity, mesh.0.clone(), material.0.clone()));
            return;
        }
        if let Ok(kids) = children.get(root) {
            for child in kids.iter() {
                collect_meshes(child, children, meshes, out);
            }
        }
    }

    fn blocked_ancestor(
        entity: Entity,
        parents: &Query<&ChildOf>,
        blocked: &Query<(
            Option<&crate::luau::LuaScriptRef>,
            Option<&crate::animation::CharacterAnimator>,
            Option<&bevy_rapier3d::prelude::Collider>,
            Option<&bevy_rapier3d::prelude::RigidBody>,
            Option<&crate::physics::PendingCollider>,
        )>,
    ) -> bool {
        let mut current = Some(entity);
        let mut depth = 0;
        while let Some(node) = current {
            if let Ok((script, animator, collider, body, pending)) = blocked.get(node)
                && (script.is_some()
                    || animator.is_some()
                    || collider.is_some()
                    || body.is_some()
                    || pending.is_some())
            {
                return true;
            }
            current = parents.get(node).ok().map(|c| c.parent());
            depth += 1;
            if depth > 64 {
                return true;
            }
        }
        false
    }

    for (entity, candidate) in &candidates {
        // A cena do glTF ainda não aterrou: tentar no próximo frame (o
        // marcador fica; sem isto o candidato nunca era confirmado).
        if ready.get(entity).is_err() {
            continue;
        }
        let mut found = Vec::new();
        collect_meshes(entity, &children, &meshes, &mut found);
        if found.is_empty() {
            // `WorldAssetRoot` nasce antes dos filhos do glTF. Sem esta
            // espera o candidato era largado no 1.º frame e a vegetação
            // nunca entrava no batch (o bug que o QA de 40 plantas apanhou).
            if children.get(entity).is_err() {
                continue;
            }
            commands.entity(entity).remove::<FoliageCandidate>();
            continue;
        }
        if !candidate.multi && found.len() != 1 {
            commands.entity(entity).remove::<FoliageCandidate>();
            continue;
        }
        if blocked_ancestor(entity, &parents, &blocked) {
            commands.entity(entity).remove::<FoliageCandidate>();
            continue;
        }
        for (mesh_entity, mesh, material) in found {
            commands
                .entity(mesh_entity)
                .insert(SimpleFoliage { mesh, material });
        }
        commands.entity(entity).remove::<FoliageCandidate>();
    }
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
        app.add_systems(Update, arm_candidates)
            .add_systems(PostUpdate, batch_tiles);
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
            &mut p,
            &mut n,
            &mut u,
            &mut i,
            &src_p,
            &[],
            &[],
            &src_i,
            Mat4::from_translation(Vec3::X),
        );
        append_mesh(
            &mut p,
            &mut n,
            &mut u,
            &mut i,
            &src_p,
            &[],
            &[],
            &src_i,
            Mat4::IDENTITY,
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

    #[test]
    fn batch_cull_has_hysteresis_over_the_radius() {
        assert!(
            BATCH_CULL > BATCH_RADIUS,
            "um tile criado a BATCH_RADIUS não pode morrer no frame seguinte"
        );
    }

    #[test]
    fn tile_center_is_the_middle_of_the_tile() {
        assert_eq!(
            tile_center(IVec2::new(0, 0)),
            Vec2::new(TILE * 0.5, TILE * 0.5)
        );
        assert_eq!(
            tile_center(IVec2::new(-1, 2)),
            Vec2::new(-TILE * 0.5, TILE * 2.5)
        );
    }

    fn scene_with_meshes(multi: bool, meshes: usize) -> (bevy::app::App, Entity, Vec<Entity>) {
        let mut app = bevy::app::App::new();
        app.add_systems(Update, arm_candidates);
        let root = app
            .world_mut()
            .spawn((
                bevy::world_serialization::WorldAssetRoot(Handle::<
                    bevy::world_serialization::WorldAsset,
                >::default()),
                FoliageCandidate { multi },
            ))
            .id();
        let mut children = Vec::new();
        for _ in 0..meshes {
            children.push(
                app.world_mut()
                    .spawn((
                        Transform::default(),
                        Visibility::Inherited,
                        Mesh3d(Handle::<Mesh>::default()),
                        MeshMaterial3d(Handle::<StandardMaterial>::default()),
                        ChildOf(root),
                    ))
                    .id(),
            );
        }
        (app, root, children)
    }

    #[test]
    fn multi_candidate_arms_every_mesh_of_the_scene() {
        let (mut app, _root, children) = scene_with_meshes(true, 3);
        app.update();
        for child in children {
            assert!(
                app.world().get::<SimpleFoliage>(child).is_some(),
                "cada mesh da cena multi entra no batch"
            );
        }
    }

    #[test]
    fn single_candidate_still_rejects_a_multi_mesh_scene() {
        let (mut app, root, children) = scene_with_meshes(false, 2);
        app.update();
        assert!(
            children
                .iter()
                .all(|child| app.world().get::<SimpleFoliage>(*child).is_none()),
            "o caminho <GltfScene> continua conservador (um mesh)"
        );
        assert!(
            app.world().get::<FoliageCandidate>(root).is_none(),
            "o candidato que não qualifica é largado"
        );
    }
}
