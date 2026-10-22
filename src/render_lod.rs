//! Render LOD — distance culling and shadow budget for spawned instances.
//!
//! `simple-rpg` places ~9700 glTF scenes (6000 `<StaticSpawner>` props, 3200
//! `<Vegetation>` instances, 484 authored `<GltfScene>`), and every scene
//! expands into several entities: ~60k entities, all of them extracted,
//! frustum-checked and rendered into four shadow cascades every frame on a
//! 4 km map. Nothing beyond the fog is worth that.
//!
//! Two knobs, both mirroring what [`crate::luau::ScriptActivation`] already
//! does for AI:
//!
//! * [`CullDistance`] on a **scene root** hides the whole subtree past a
//!   radius. Visibility is inherited in Bevy, so toggling the root is enough
//!   — no per-mesh component and no propagation walk.
//! * [`NoShadowSubtree`] tags a root whose meshes must not cast. Unlike
//!   visibility, `NotShadowCaster` is read on the *mesh* entity, so this one
//!   does need a one-shot propagation pass once the scene has spawned.

use crate::profiler::{Group, timed};
use bevy::gltf::Gltf;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;

/// Hides the entity (and, by inheritance, its whole subtree) once the camera
/// is further than `radius` meters away.
///
/// The band between `radius * HYSTERESIS` and `radius` is sticky: an object
/// walking the boundary does not flicker on and off with camera jitter.
#[derive(Component, Debug, Clone, Copy)]
pub struct CullDistance {
    /// Distance (meters) past which the subtree stops rendering.
    pub radius: f32,
    /// O `Hidden` atual foi posto pelo cull. Um `Hidden` de outra origem
    /// (QA a esconder, um script) não é revertido quando a câmara chega
    /// perto.
    pub hidden_by_cull: bool,
}

impl CullDistance {
    pub fn new(radius: f32) -> Self {
        Self {
            radius,
            hidden_by_cull: false,
        }
    }
}

/// Raiz de uma `<GltfScene>` estática à espera de ganhar [`CullDistance`]
/// pelo tamanho, quando as malhas da cena tiverem `Aabb`.
#[derive(Component, Debug, Default)]
pub struct AutoCull;

/// Píxeis de altura (a 720p, FOV ~64°) abaixo dos quais um objeto sai de
/// cena: a distância de corte é `raio × AUTO_CULL_PER_RADIUS`.
///
/// O mundo estático cozia os spawners em `<GltfScene>` soltas SEM culling:
/// numa vista do vale desenhavam-se 600 arbustos e 200 cactos a centenas de
/// metros (sub-píxel), e o custo variava 3× entre sítios do mapa. Com o
/// corte pelo tamanho no ecrã um arbusto (r ≈ 0,6 m) sai aos ~85 m, um cacto
/// aos ~200 m, e uma muralha fica até ao teto.
pub const AUTO_CULL_PER_RADIUS: f32 = 144.0;
/// Piso e teto do corte automático (m), antes do `cull_scale` do preset.
pub const AUTO_CULL_MIN: f32 = 40.0;
pub const AUTO_CULL_MAX: f32 = 800.0;

/// Teto de triângulos de uma malha de tier distante (`*_lod2.glb`).
///
/// Vários LOD2 do pool chegavam a 8–21 k triângulos (o `moss_rock` tem os
/// três tiers iguais, 16 k; o lobo 21 k; o pilar 8,4 k) — a geração parou no
/// piso de costuras UV. Um LOD2 vive a ≥ 70–110 m, onde 2 500 triângulos
/// chegam para qualquer adereço: [`simplify_far_tiers`] corta-os ao carregar.
pub const FAR_TIER_MAX_TRIS: usize = 2500;

/// Índices simplificados para ≤ `target_tris`, ou `None` se não reduzir.
///
/// Primeiro a simplificação que respeita costuras (erro até 5 % da
/// extensão); se as costuras a travarem acima do dobro do alvo, a `sloppy`
/// (ignora topologia — a esta distância não se vê).
pub fn simplify_far_indices(
    positions: &[[f32; 3]],
    indices: &[u32],
    target_tris: usize,
) -> Option<Vec<u32>> {
    let current = indices.len() / 3;
    if current <= target_tris {
        return None;
    }
    let target = target_tris * 3;
    let mut out = meshopt::simplify_decoder(
        indices,
        positions,
        target,
        0.05,
        meshopt::SimplifyOptions::None,
        None,
    );
    if out.len() > target * 2 {
        out = meshopt::simplify_sloppy_decoder(indices, positions, target, 0.02, None);
    }
    (!out.is_empty() && out.len() < indices.len()).then_some(out)
}

/// Simplifica, ao carregar, as malhas de `*_lod2.glb` acima de
/// [`FAR_TIER_MAX_TRIS`]. Uma vez por asset (o `Added`): sem churn de upload.
pub fn simplify_far_tiers(
    mut events: MessageReader<AssetEvent<Mesh>>,
    // `Option`: apps mínimas (testes de LOD/animação) montam o plugin sem
    // AssetPlugin; a validação rígida abortava o sistema em vez de o saltar.
    server: Option<Res<AssetServer>>,
    meshes: Option<ResMut<Assets<Mesh>>>,
    mut total: Local<(usize, usize, usize)>,
) {
    let (Some(server), Some(mut meshes)) = (server, meshes) else {
        return;
    };
    let mut touched = false;
    for event in events.read() {
        let AssetEvent::Added { id } = event else {
            continue;
        };
        let is_far = server
            .get_path(*id)
            .is_some_and(|p| p.path().to_string_lossy().ends_with("_lod2.glb"));
        if !is_far {
            continue;
        }
        let Some(mesh) = meshes.get(*id) else {
            continue;
        };
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        let indices: Vec<u32> = match mesh.indices() {
            Some(bevy::mesh::Indices::U32(v)) => v.clone(),
            Some(bevy::mesh::Indices::U16(v)) => v.iter().map(|i| *i as u32).collect(),
            None => continue,
        };
        let Some(simplified) = simplify_far_indices(positions, &indices, FAR_TIER_MAX_TRIS) else {
            continue;
        };
        total.0 += 1;
        total.1 += indices.len() / 3;
        total.2 += simplified.len() / 3;
        if let Some(mut mesh) = meshes.get_mut(*id) {
            mesh.insert_indices(bevy::mesh::Indices::U32(simplified));
        }
        touched = true;
    }
    if touched {
        info!(
            "lod2: {} malhas simplificadas ({} → {} triângulos)",
            total.0, total.1, total.2
        );
    }
}

/// Distância de corte para um objeto de raio `radius` (m).
pub fn auto_cull_radius(radius: f32, scale: f32) -> f32 {
    (radius * auto_cull_per_radius()).clamp(AUTO_CULL_MIN, AUTO_CULL_MAX) * scale
}

/// [`AUTO_CULL_PER_RADIUS`], ou o equivalente a `VIBER_AUTO_CULL_PX`
/// píxeis de altura mínima (8 px = o default) — o knob do A/B.
fn auto_cull_per_radius() -> f32 {
    static FACTOR: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *FACTOR.get_or_init(|| {
        std::env::var("VIBER_AUTO_CULL_PX")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|px| *px > 0.0)
            .map_or(AUTO_CULL_PER_RADIUS, |px| AUTO_CULL_PER_RADIUS * 8.0 / px)
    })
}

/// Fraction of `radius` at which a hidden object becomes visible again.
const CULL_HYSTERESIS: f32 = 0.94;

/// Default cull radius (meters) for `<StaticSpawner>` props — trees, rocks,
/// crates. Far enough that a treeline still reads at the fog line.
pub const DEFAULT_STATIC_CULL: f32 = 320.0;

/// Default cull radius for `<Vegetation>` — grass and flowers, which are
/// sub-meter and stop contributing well before the props do.
pub const DEFAULT_VEGETATION_CULL: f32 = 80.0;

/// Default cull radius for `<DynamicSpawner>` creatures. Beyond this their
/// scripts are frozen anyway (`ScriptActivation`, 45 m by default).
pub const DEFAULT_DYNAMIC_CULL: f32 = 160.0;

/// Marks a scene root whose meshes must not cast shadows. Removed once the
/// propagation lands (the scene spawns asynchronously, so the marker waits).
#[derive(Component, Debug)]
pub struct NoShadowSubtree;

/// Mesh LOD ladder for one spawned instance.
///
/// The `simple-rpg` worlds already author `lod1-url` / `lod2-url` and the
/// `lod-threshold-near` / `lod-threshold-mid` cuts on every `<GLTFLoader>`
/// template — the old engine honored them, Viber dropped them as no-ops and drew
/// the hero mesh of every pine at 200 m. This component restores the ladder:
/// swapping `WorldAssetRoot` despawns the old scene subtree and spawns the
/// new one (`world_instance_spawner` reacts to `Changed<WorldAssetRoot>`),
/// so only one tier is resident per instance at a time — triangles *and*
/// VRAM, not just draw calls.
#[derive(Component, Debug, Clone)]
pub struct MeshLod {
    /// Scene per tier, coarsest last. Tier 0 is always present.
    pub tiers: Vec<Handle<bevy::world_serialization::WorldAsset>>,
    /// The glTF asset each tier's scene came from, parallel to `tiers`.
    ///
    /// Animation clips are per-gltf assets, so when a swap re-arms the
    /// animation bind ([`crate::animation::rearm_after_scene_swap`]) the new
    /// tier's own gltf has to travel with the swap.
    pub gltf_tiers: Vec<Handle<Gltf>>,
    /// Distance at which tier 0 gives way to tier 1.
    pub near: f32,
    /// Distance at which tier 1 gives way to tier 2.
    pub mid: f32,
    /// Tier currently attached to `WorldAssetRoot`.
    pub current: u8,
    /// Re-tag the fresh subtree as a non-caster after a swap.
    pub no_shadows: bool,
}

impl MeshLod {
    /// Tier index for `distance`, with a sticky band so an instance sitting
    /// on a threshold does not respawn its subtree every frame.
    fn tier_for(&self, distance: f32, current: u8) -> u8 {
        // `already_coarse`: the instance is *on* the coarser tier, so the
        // edge moves inward — it keeps the cheap mesh a little longer.
        // Otherwise the edge moves outward and the detailed mesh holds.
        let hysteresis = |edge: f32, already_coarse: bool| {
            if already_coarse {
                edge * (1.0 - LOD_HYSTERESIS)
            } else {
                edge * (1.0 + LOD_HYSTERESIS)
            }
        };
        let last = (self.tiers.len() as u8).saturating_sub(1);
        let near = hysteresis(self.near, current >= 1);
        let mid = hysteresis(self.mid, current >= 2);
        let wanted = if distance >= mid {
            2
        } else if distance >= near {
            1
        } else {
            0
        };
        wanted.min(last)
    }
}

/// Relative width of the sticky band around each LOD threshold.
const LOD_HYSTERESIS: f32 = 0.08;

/// Scene swaps allowed per frame. A swap despawns and respawns a subtree, so
/// a camera teleport (fast travel) must not try to re-tier 6000 props in one
/// frame — it drains over the next few instead.
///
/// Overridable with `VIBER_LOD_SWAP_BUDGET` (`0` disables the ladder without
/// disabling the culling): the two halves of this plugin have very different
/// costs, and telling them apart in a profile needs them to be separable.
/// Com o cross-fade de LOD os dois tiers convivem um instante, por isso o
/// teto desce: um teleporte não duplica a VRAM de 24 cenas ao mesmo tempo.
pub const MAX_LOD_SWAPS_PER_FRAME: usize = 12;
/// Quanto tempo (s) o tier antigo fica por cima, em dither temporal.
pub const LOD_CROSSFADE_SECS: f32 = 0.12;

/// Resolved swap budget: `VIBER_LOD_SWAP_BUDGET`, else the default.
fn swap_budget() -> usize {
    std::env::var("VIBER_LOD_SWAP_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(MAX_LOD_SWAPS_PER_FRAME)
}

/// Scene swaps performed on the last frame, and instances still waiting.
///
/// A swap is the expensive half of the ladder (despawn + respawn of a glTF
/// subtree); `pending` staying pegged means the budget is saturated and the
/// queue never drains, which reads as a permanent stutter while walking.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct MeshLodStats {
    /// Swaps applied on the last frame.
    pub swaps_last_frame: usize,
    /// Instances that wanted a different tier but ran out of budget.
    pub pending: usize,
}

/// Registers the render-LOD systems.
pub struct RenderLodPlugin;

impl bevy::app::Plugin for RenderLodPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.init_resource::<MeshLodStats>()
            // `simplify_far_tiers` lê `AssetEvent<Mesh>`; sem o registo aqui,
            // apps mínimas (o teste de swap de LOD) abortavam com "Message not
            // initialized". `add_message` é idempotente.
            .add_message::<AssetEvent<Mesh>>()
            .add_observer(retag_no_shadow_on_ready)
            .add_systems(bevy::app::PreUpdate, simplify_far_tiers)
            .add_systems(
                bevy::app::PostUpdate,
                (
                    timed(Group::Render, auto_cull_from_bounds),
                    timed(Group::Render, cull_distant_objects),
                    timed(Group::Render, update_mesh_lod),
                    timed(Group::Render, propagate_no_shadow),
                )
                    .chain()
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            );
    }
}

/// Re-marks a non-casting LOD instance once its NEW tier has spawned.
///
/// A swap only requests the scene: the scene spawner despawns the old
/// subtree and spawns the new one in the next frame's `SpawnScene`. Tagging
/// at swap time let [`propagate_no_shadow`] find the OLD (already tagged)
/// meshes and clear the marker, so every fresh tier cast shadows again.
/// `WorldInstanceReady` fires right after the spawn, before `PostUpdate`, so
/// the new meshes are tagged before they are ever extracted.
fn retag_no_shadow_on_ready(
    ready: On<bevy::world_serialization::WorldInstanceReady>,
    lods: Query<&MeshLod>,
    mut commands: Commands,
) {
    if lods.get(ready.entity).is_ok_and(|lod| lod.no_shadows) {
        commands.entity(ready.entity).try_insert(NoShadowSubtree);
    }
}

/// Swaps each instance's scene to the tier its camera distance calls for.
///
/// Only hidden-by-culling instances are skipped: a prop the player cannot see
/// does not deserve a respawn, and it re-tiers on the frame it comes back.
fn update_mesh_lod(
    editor: Query<&GlobalTransform, With<crate::editor_camera::EditorCamera>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut stats: ResMut<MeshLodStats>,
    boost: Option<Res<crate::cinematic::StreamingBoost>>,
    mut instances: Query<(
        &GlobalTransform,
        &mut MeshLod,
        &mut bevy::world_serialization::WorldAssetRoot,
        &Visibility,
    )>,
) {
    // Modo editor: a distância mede-se à câmara de editor (a de jogo está
    // inactiva mas continua na query — ver `active_camera_eye`).
    let Some(camera) = crate::editor_camera::active_camera_eye(&editor, &cameras) else {
        return;
    };
    let eye = camera.translation();
    // Corte de cinemática em curso (ecrã em negro): drenar a fila depressa.
    let mut budget = swap_budget()
        * if boost.as_deref().is_some_and(|b| b.0) {
            10
        } else {
            1
        };
    let mut swaps = 0usize;
    let mut pending = 0usize;
    for (transform, mut lod, mut root, visibility) in &mut instances {
        if *visibility == Visibility::Hidden {
            continue;
        }
        if budget == 0 {
            // Ainda conta o trabalho por fazer: é isto que distingue "a fila
            // drenou" de "a fila está saturada todos os frames".
            let distance = transform.translation().distance(eye);
            if lod.tier_for(distance, lod.current) != lod.current {
                pending += 1;
            }
            continue;
        }
        let distance = transform.translation().distance(eye);
        let wanted = lod.tier_for(distance, lod.current);
        if wanted == lod.current {
            continue;
        }
        let Some(scene) = lod.tiers.get(wanted as usize).cloned() else {
            continue;
        };
        // Mutating `WorldAssetRoot` is the swap: the spawner despawns the old
        // instance and spawns the new scene as children of this same entity,
        // so the transform, collider and script all survive untouched. The
        // animation bind does not survive — it re-arms off this very change
        // (see `crate::animation::rearm_after_scene_swap`).
        // The fresh subtree earns its `NotShadowCaster` tags again in
        // `retag_no_shadow_on_ready`, once it has actually spawned.
        root.0 = scene;
        lod.current = wanted;
        budget -= 1;
        swaps += 1;
    }
    stats.swaps_last_frame = swaps;
    stats.pending = pending;
}

/// Toggles `Visibility` on every [`CullDistance`] root from the camera range.
///
/// Runs every frame on purpose: instances stream in over many frames while
/// the spawner drains, so a camera-movement gate would leave fresh props
/// visible across the whole map until the player next moved. The work is a
/// squared-distance compare over ~10k roots, and the write is guarded so
/// change detection (and therefore visibility propagation) only fires on an
/// actual transition.
fn cull_distant_objects(
    editor: Query<&GlobalTransform, With<crate::editor_camera::EditorCamera>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut objects: Query<(&GlobalTransform, &mut CullDistance, &mut Visibility)>,
) {
    let Some(camera) = crate::editor_camera::active_camera_eye(&editor, &cameras) else {
        return;
    };
    let eye = camera.translation();
    for (transform, mut cull, mut visibility) in &mut objects {
        // Escondido por outra coisa: não é nosso para mostrar.
        if *visibility == Visibility::Hidden && !cull.hidden_by_cull {
            continue;
        }
        let radius = cull.radius.max(1.0);
        let distance_sq = transform.translation().distance_squared(eye);
        let wanted = match *visibility {
            // Visible: keep it until it passes the outer edge.
            Visibility::Hidden => {
                let show = radius * CULL_HYSTERESIS;
                if distance_sq <= show * show {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                }
            }
            _ => {
                if distance_sq <= radius * radius {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                }
            }
        };
        // Guarded: `DerefMut` on `Visibility` would mark every root changed
        // every frame and re-run visibility propagation over the whole tree.
        if *visibility != wanted {
            *visibility = wanted;
            cull.hidden_by_cull = wanted == Visibility::Hidden;
        }
    }
}

/// Dá [`CullDistance`] às raízes [`AutoCull`] assim que a cena tem malhas
/// com `Aabb`: o raio é o da maior esfera envolvente das malhas (em espaço
/// de mundo, com a escala da instância), medido à origem da raiz.
pub fn auto_cull_from_bounds(
    mut commands: Commands,
    roots: Query<(Entity, &GlobalTransform), With<AutoCull>>,
    children: Query<&Children>,
    meshes: Query<(&GlobalTransform, &bevy::camera::primitives::Aabb), With<Mesh3d>>,
    time: Res<Time<Real>>,
    mut report: Local<(f32, usize)>,
) {
    // Diagnóstico barato: a cada 10 s com raízes pendentes, quantas faltam e
    // quantas já ganharam corte (uma raiz sem malhas nunca resolve).
    report.0 += time.delta_secs();
    if report.0 >= 10.0 {
        report.0 = 0.0;
        let pending = roots.iter().count();
        if pending > 0 {
            info!("auto-cull: {} resolvidas, {pending} pendentes", report.1);
        }
    }
    let scale = crate::graphics::cull_scale();
    for (root, root_xf) in &roots {
        let origin = root_xf.translation();
        let mut radius = 0.0_f32;
        let mut found = false;
        for node in children.iter_descendants(root) {
            let Ok((xf, aabb)) = meshes.get(node) else {
                continue;
            };
            found = true;
            let center = xf.transform_point(Vec3::from(aabb.center));
            let extent = Vec3::from(aabb.half_extents) * xf.scale().abs();
            radius = radius.max(center.distance(origin) + extent.length());
        }
        if found {
            report.1 += 1;
            commands
                .entity(root)
                .remove::<AutoCull>()
                .try_insert(CullDistance::new(auto_cull_radius(radius, scale)));
        }
    }
}

/// Inserts [`NotShadowCaster`] on every mesh below a [`NoShadowSubtree`] root.
///
/// `NotShadowCaster` is queried on the entity that owns the `Mesh3d`, and
/// glTF scenes spawn their meshes as descendants a few frames after the root
/// exists — hence a marker that survives until the meshes show up.
fn propagate_no_shadow(
    mut commands: Commands,
    roots: Query<Entity, With<NoShadowSubtree>>,
    children: Query<&Children>,
    meshes: Query<Has<NotShadowCaster>, With<Mesh3d>>,
) {
    for root in &roots {
        let mut found = 0usize;
        for descendant in children.iter_descendants(root) {
            let Ok(already_tagged) = meshes.get(descendant) else {
                continue;
            };
            found += 1;
            if !already_tagged {
                commands.entity(descendant).insert(NotShadowCaster);
            }
        }
        // The marker clears as soon as the scene has *any* mesh — counting
        // only the newly tagged ones would keep re-walking the subtree of an
        // instance whose meshes were all tagged already (an LOD swap that
        // reused a cached scene).
        if found > 0 {
            commands.entity(root).remove::<NoShadowSubtree>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal app with the culling system and a camera at the origin.
    fn app_with_camera() -> App {
        let mut app = App::new();
        app.add_systems(Update, cull_distant_objects);
        app.world_mut().spawn((
            Camera3d::default(),
            GlobalTransform::from_translation(Vec3::ZERO),
        ));
        app
    }

    /// Uma grelha densa desce para o teto; uma malha já leve fica intacta.
    #[test]
    fn test_far_tier_simplification_hits_the_budget() {
        let n = 80;
        let mut positions = Vec::new();
        for z in 0..=n {
            for x in 0..=n {
                let (fx, fz) = (x as f32 / n as f32, z as f32 / n as f32);
                positions.push([fx, (fx * 6.0).sin() * 0.1 + (fz * 5.0).cos() * 0.1, fz]);
            }
        }
        let mut indices = Vec::new();
        let row = n as u32 + 1;
        for z in 0..n as u32 {
            for x in 0..n as u32 {
                let i = z * row + x;
                indices.extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
            }
        }
        assert_eq!(indices.len() / 3, 12_800);
        let out = simplify_far_indices(&positions, &indices, 2500).expect("reduz");
        assert!(out.len() / 3 <= 5000, "{} tri", out.len() / 3);
        assert!(simplify_far_indices(&positions, &indices[..300], 2500).is_none());
    }

    /// Arbusto sai cedo, muralha fica até ao teto, nada abaixo do piso.
    #[test]
    fn test_auto_cull_radius_follows_screen_size() {
        assert!((auto_cull_radius(0.6, 1.0) - 86.4).abs() < 0.1);
        assert_eq!(auto_cull_radius(0.1, 1.0), AUTO_CULL_MIN);
        assert_eq!(auto_cull_radius(20.0, 1.0), AUTO_CULL_MAX);
        assert!((auto_cull_radius(0.6, 0.7) - 60.48).abs() < 0.1);
    }

    /// Um `Hidden` posto por outra coisa (QA, script) não é revertido pelo
    /// cull quando a câmara se aproxima.
    #[test]
    fn test_cull_never_reveals_what_it_did_not_hide() {
        let mut app = app_with_camera();
        let entity = spawn_at(&mut app, 10.0, 100.0);
        app.world_mut()
            .entity_mut(entity)
            .insert(Visibility::Hidden);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Hidden
        );
        // E o que ELE escondeu volta.
        let far = spawn_at(&mut app, 500.0, 100.0);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(far).unwrap(),
            Visibility::Hidden
        );
        app.world_mut()
            .entity_mut(far)
            .insert(GlobalTransform::from_translation(Vec3::new(10.0, 0.0, 0.0)));
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(far).unwrap(),
            Visibility::Inherited
        );
    }

    fn spawn_at(app: &mut App, x: f32, radius: f32) -> Entity {
        app.world_mut()
            .spawn((
                GlobalTransform::from_translation(Vec3::new(x, 0.0, 0.0)),
                CullDistance::new(radius),
                Visibility::Inherited,
            ))
            .id()
    }

    #[test]
    fn test_object_inside_the_radius_stays_visible() {
        let mut app = app_with_camera();
        let entity = spawn_at(&mut app, 50.0, 100.0);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Inherited
        );
    }

    #[test]
    fn test_object_beyond_the_radius_is_hidden() {
        let mut app = app_with_camera();
        let entity = spawn_at(&mut app, 150.0, 100.0);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Hidden
        );
    }

    #[test]
    fn test_hysteresis_keeps_a_hidden_object_hidden_inside_the_band() {
        let mut app = app_with_camera();
        // 97 m: inside the 100 m radius, but above the 94 m re-show edge.
        let entity = spawn_at(&mut app, 150.0, 100.0);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Hidden
        );
        app.world_mut()
            .entity_mut(entity)
            .insert(GlobalTransform::from_translation(Vec3::new(97.0, 0.0, 0.0)));
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Hidden,
            "inside the hysteresis band a hidden object must stay hidden"
        );
        app.world_mut()
            .entity_mut(entity)
            .insert(GlobalTransform::from_translation(Vec3::new(90.0, 0.0, 0.0)));
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Inherited,
            "past the re-show edge it comes back"
        );
    }

    #[test]
    fn test_culling_without_a_camera_is_a_noop() {
        let mut app = App::new();
        app.add_systems(Update, cull_distant_objects);
        let entity = app
            .world_mut()
            .spawn((
                GlobalTransform::from_translation(Vec3::new(9999.0, 0.0, 0.0)),
                CullDistance::new(10.0),
                Visibility::Inherited,
            ))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Inherited
        );
    }

    #[test]
    fn test_no_shadow_marker_tags_mesh_descendants_and_clears() {
        let mut app = App::new();
        app.add_systems(Update, propagate_no_shadow);
        let mesh = app.world_mut().spawn(Mesh3d::default()).id();
        let root = app.world_mut().spawn(NoShadowSubtree).add_child(mesh).id();
        app.update();
        assert!(app.world().get::<NotShadowCaster>(mesh).is_some());
        assert!(
            app.world().get::<NoShadowSubtree>(root).is_none(),
            "the marker clears once the meshes are tagged"
        );
    }

    fn ladder(tiers: usize) -> MeshLod {
        MeshLod {
            tiers: (0..tiers).map(|_| Handle::default()).collect(),
            gltf_tiers: (0..tiers).map(|_| Handle::default()).collect(),
            near: 40.0,
            mid: 100.0,
            current: 0,
            no_shadows: false,
        }
    }

    #[test]
    fn test_tier_follows_the_authored_thresholds() {
        let lod = ladder(3);
        assert_eq!(lod.tier_for(10.0, 0), 0);
        assert_eq!(lod.tier_for(60.0, 0), 1);
        assert_eq!(lod.tier_for(200.0, 0), 2);
    }

    #[test]
    fn test_tier_is_clamped_to_the_tiers_the_template_authored() {
        // Only lod0 + lod1 exist: a far prop stops at tier 1 instead of
        // indexing past the end of the ladder.
        let lod = ladder(2);
        assert_eq!(lod.tier_for(500.0, 0), 1);
    }

    #[test]
    fn test_tier_hysteresis_holds_across_the_threshold() {
        let lod = ladder(3);
        // Coming from tier 0, the 40 m edge moves out to 43.2 m…
        assert_eq!(lod.tier_for(41.0, 0), 0);
        // …and coming back from tier 1 it moves in to 36.8 m.
        assert_eq!(lod.tier_for(38.0, 1), 1);
        assert_eq!(lod.tier_for(36.0, 1), 0);
    }

    /// The fresh tier of a non-casting instance is re-marked when its scene
    /// instance becomes ready — not at swap time, when only the old (already
    /// tagged) subtree exists.
    #[test]
    fn test_scene_ready_re_marks_non_casting_lod_instances() {
        let mut app = App::new();
        app.add_observer(retag_no_shadow_on_ready);
        let mut quiet = ladder(2);
        quiet.no_shadows = true;
        let caster = app.world_mut().spawn(ladder(2)).id();
        let non_caster = app.world_mut().spawn(quiet).id();
        let instance_id = {
            let world = app.world_mut();
            world.init_resource::<bevy::world_serialization::WorldInstanceSpawner>();
            world.resource_scope(
                |_, mut spawner: Mut<bevy::world_serialization::WorldInstanceSpawner>| {
                    spawner.spawn_as_child(Handle::default(), non_caster)
                },
            )
        };
        for entity in [caster, non_caster] {
            app.world_mut()
                .trigger(bevy::world_serialization::WorldInstanceReady {
                    entity,
                    instance_id,
                });
        }
        app.update();
        assert!(app.world().get::<NoShadowSubtree>(non_caster).is_some());
        assert!(app.world().get::<NoShadowSubtree>(caster).is_none());
    }

    #[test]
    fn test_no_shadow_marker_waits_for_the_scene_to_spawn() {
        let mut app = App::new();
        app.add_systems(Update, propagate_no_shadow);
        let root = app.world_mut().spawn(NoShadowSubtree).id();
        app.update();
        assert!(
            app.world().get::<NoShadowSubtree>(root).is_some(),
            "no meshes yet — the marker survives for a later frame"
        );
    }
}
