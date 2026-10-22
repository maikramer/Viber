//! `VIBER_DRAW_DIAG=1` — o que cada vista desenha, em malhas e triângulos.
//!
//! O profiler mede tempos de passe, mas um frame preso na GPU com os passes
//! de pós a valer pouco precisa da pergunta seguinte: QUANTA geometria entra
//! na câmara e em cada cascata de sombra, e de que assets vem. A cada
//! [`PERIOD_S`] segundos loga, por vista, o nº de malhas visíveis, os
//! triângulos somados e o top de assets por triângulos (caminho do asset, ou
//! o `Name` da entidade para malhas procedurais).
//!
//! Desligado por omissão: varrer as listas de visibilidade custa ~1 ms.

use std::collections::HashMap;

use bevy::camera::visibility::{CascadesVisibleEntities, CubemapVisibleEntities, VisibleEntities};
use bevy::mesh::Mesh3d;
use bevy::prelude::*;

const PERIOD_S: f32 = 3.0;
const TOP: usize = 12;

pub struct DrawDiagPlugin;

impl Plugin for DrawDiagPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var("VIBER_DRAW_DIAG").is_ok_and(|v| v != "0") {
            app.add_systems(Last, (draw_diag, asset_churn));
        }
    }
}

/// Triângulos de uma malha (índices / 3, ou vértices / 3 sem índices).
fn triangles(mesh: &Mesh) -> usize {
    match mesh.indices() {
        Some(indices) => indices.len() / 3,
        None => mesh.count_vertices() / 3,
    }
}

#[derive(Default)]
struct Tally {
    meshes: usize,
    tris: usize,
    by_asset: HashMap<String, (usize, usize)>,
}

impl Tally {
    fn add(&mut self, label: String, tris: usize) {
        self.meshes += 1;
        self.tris += tris;
        let slot = self.by_asset.entry(label).or_default();
        slot.0 += 1;
        slot.1 += tris;
    }

    fn report(&self, view: &str) {
        info!(
            "draw-diag {view}: {} malhas, {:.2} M triângulos",
            self.meshes,
            self.tris as f64 / 1e6
        );
        let mut top: Vec<_> = self.by_asset.iter().collect();
        top.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        for (label, (count, tris)) in top.into_iter().take(TOP) {
            info!(
                "draw-diag {view}:   {:>8.1} k tri  {:>5}×  {label}",
                *tris as f64 / 1e3,
                count
            );
        }
    }
}

#[allow(clippy::type_complexity)]
fn draw_diag(
    time: Res<Time<Real>>,
    mut since: Local<f32>,
    cameras: Query<(&Camera, &VisibleEntities), With<Camera3d>>,
    directional: Query<&CascadesVisibleEntities>,
    points: Query<&CubemapVisibleEntities>,
    mesh3d: Query<(&Mesh3d, Option<&Name>, Option<&ChildOf>)>,
    names: Query<&Name>,
    meshes: Res<Assets<Mesh>>,
    std_mats: Query<&MeshMaterial3d<StandardMaterial>>,
    materials: Res<Assets<StandardMaterial>>,
) {
    *since += time.delta_secs();
    if *since < PERIOD_S {
        return;
    }
    *since = 0.0;

    let label_of = |entity: Entity| -> Option<(String, usize)> {
        let (mesh, name, parent) = mesh3d.get(entity).ok()?;
        let tris = meshes.get(&mesh.0).map(triangles).unwrap_or(0);
        let label = match mesh.0.path() {
            Some(path) => path.to_string(),
            None => name
                .map(|n| n.as_str().to_string())
                .or_else(|| {
                    parent
                        .and_then(|p| names.get(p.parent()).ok())
                        .map(|n| n.as_str().to_string())
                })
                .unwrap_or_else(|| "<procedural>".to_string()),
        };
        Some((label, tris))
    };
    let tally = |entities: &mut dyn Iterator<Item = Entity>| {
        let mut t = Tally::default();
        for entity in entities {
            if let Some((label, tris)) = label_of(entity) {
                t.add(label, tris);
            }
        }
        t
    };

    for (camera, visible) in &cameras {
        if !camera.is_active {
            continue;
        }
        tally(&mut visible.iter(std::any::TypeId::of::<Mesh3d>()).copied()).report("câmara");
        // O passe transparente: o que a câmara vê com material blended
        // (StandardMaterial com AlphaMode Blend/Add/Premultiplied/Multiply).
        let blended = |e: &Entity| {
            std_mats
                .get(*e)
                .ok()
                .and_then(|m| materials.get(&m.0))
                .is_some_and(|m| !matches!(m.alpha_mode, AlphaMode::Opaque | AlphaMode::Mask(_)))
        };
        tally(
            &mut visible
                .iter(std::any::TypeId::of::<Mesh3d>())
                .filter(|e| blended(e))
                .copied(),
        )
        .report("câmara/transparente");
    }
    for cascades in &directional {
        for per_view in cascades.entities.values() {
            for (i, cascade) in per_view.iter().enumerate() {
                tally(&mut cascade.entities.iter().copied()).report(&format!("cascata {i}"));
            }
        }
    }
    let mut cube = Tally::default();
    let mut lights = 0;
    for faces in &points {
        let mut any = false;
        for face in faces.iter() {
            any |= !face.entities.is_empty();
            for entity in &face.entities {
                if let Some((label, tris)) = label_of(*entity) {
                    cube.add(label, tris);
                }
            }
        }
        lights += usize::from(any);
    }
    if lights > 0 {
        cube.report(&format!("lanternas ({lights} com sombra, 6 faces)"));
    }
}

/// Eventos de asset por segundo: um material/malha/imagem mutado em loop
/// re-prepara-se todos os frames (bind group recriado, por vezes um frame
/// sem desenhar) — invisível no profiler, óbvio aqui.
fn asset_churn(
    time: Res<Time<Real>>,
    mut since: Local<f32>,
    mut counts: Local<[usize; 9]>,
    mut mats: MessageReader<AssetEvent<StandardMaterial>>,
    mut meshes: MessageReader<AssetEvent<Mesh>>,
    mut images: MessageReader<AssetEvent<Image>>,
    mut modified: Local<HashMap<AssetId<Mesh>, usize>>,
    owners: Query<(Entity, &Mesh3d)>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
) {
    fn tally(counts: &mut [usize], base: usize, kind: &AssetEvent<impl Asset>) {
        match kind {
            AssetEvent::Modified { .. } => counts[base] += 1,
            AssetEvent::Added { .. } => counts[base + 1] += 1,
            AssetEvent::Removed { .. } | AssetEvent::Unused { .. } => counts[base + 2] += 1,
            _ => {}
        }
    }
    for e in mats.read() {
        tally(&mut counts[..], 0, e);
    }
    for e in meshes.read() {
        tally(&mut counts[..], 3, e);
        if let AssetEvent::Modified { id } = e {
            *modified.entry(*id).or_default() += 1;
        }
    }
    for e in images.read() {
        tally(&mut counts[..], 6, e);
    }
    *since += time.delta_secs();
    if *since < 1.0 {
        return;
    }
    *since = 0.0;
    let c = *counts;
    info!(
        "draw-diag assets/s: material mod {} add {} rm {} | mesh mod {} add {} rm {} | image mod {} add {} rm {}",
        c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7], c[8]
    );
    *counts = [0; 9];
    // Quem são as malhas mutadas: o primeiro nome na cadeia de pais de uma
    // entidade que as desenha.
    let mut by_owner: HashMap<String, (usize, usize)> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for (entity, mesh) in &owners {
        let Some(n) = modified.get(&mesh.0.id()) else {
            continue;
        };
        if !seen.insert(mesh.0.id()) {
            continue;
        }
        let mut label = String::from("<sem nome>");
        let mut cursor = Some(entity);
        for _ in 0..8 {
            let Some(e) = cursor else { break };
            if let Ok(name) = names.get(e) {
                label = name.as_str().to_string();
                break;
            }
            cursor = parents.get(e).ok().map(|p| p.parent());
        }
        let slot = by_owner.entry(label).or_default();
        slot.0 += 1;
        slot.1 += n;
    }
    let mut top: Vec<_> = by_owner.into_iter().collect();
    top.sort_by(|a, b| b.1.1.cmp(&a.1.1));
    for (label, (meshes, events)) in top.into_iter().take(8) {
        info!("draw-diag   malhas mutadas: {events:>5} eventos  {meshes:>4} malhas  {label}");
    }
    modified.clear();
}
