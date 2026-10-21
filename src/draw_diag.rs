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
            app.add_systems(Last, draw_diag);
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
                .or_else(|| parent.and_then(|p| names.get(p.parent()).ok()).map(|n| n.as_str().to_string()))
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
