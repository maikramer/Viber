//! day_tint para os materiais standard dos GltfScene — copas, casas, props
//! e personagens que os sistemas da relva/splat não alcançam (peça
//! exploração, r10 do gauntlet BOTW; mecanismo reportado e autorizado pela
//! lead).
//!
//! **Porquê**: `grass_daynight_tint` e `terrain_daynight_tint` escurecem os
//! MATERIAIS partilhados da relva e do splat ao anoitecer; os materiais dos
//! glTFs carregados por `<GltfScene>` não recebem passada nenhuma — ficam
//! com o albedo de dia sob o luar azul e leem-se como "recortes colados"
//! contra a relva em silhueta.
//!
//! **Como**: mesma curva `day_tint` da relva multiplicada no `base_color`
//! ORIGINAL de cada `StandardMaterial` (capturado à primeira vista e guardado,
//! para o tint nunca compor). Ficam de fora materiais `unlit` (marcadores) e
//! `emissive` ≠ preto (vidros de janela/lanterna têm de continuar acesos —
//! as pools quentes são as luzes de ponto, não o albedo).
//!
//! Seguro para materiais partilhados: a chave é o `AssetId` do material
//! (por ficheiro GLB — todas as instâncias da mesma copa partilham o tint,
//! como na referência).

use std::collections::HashMap;

use bevy::asset::AssetId;
use bevy::color::LinearRgba;
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;

use crate::grass::day_tint;
use crate::profiler::{Group, timed};

/// Originais de albedo capturados por material + último tint aplicado.
#[derive(Resource, Default)]
struct PropTintState {
    originals: HashMap<AssetId<StandardMaterial>, LinearRgba>,
    last_tint: Option<[f32; 3]>,
    /// Nº de materiais na última passada: um roster maior (glTF a carregar
    /// depois de o tint estabilizar) força uma passada fora do early-out de
    /// drift — sem isto, o prop novo ficava ao albedo de dia sob luar até o
    /// relógio mexer 1e-3.
    last_len: usize,
    throttle: f32,
    /// Passagem em curso: (roster elegível, tint a aplicar, cursor). A
    /// aplicação é ORÇAMENTADA por frame (ver `PROP_TINT_BUDGET`) — ver o
    /// mesmo padrão no `CHUNK_MATERIAL_WRITE_BUDGET` do terreno.
    sweep: Option<(Vec<AssetId<StandardMaterial>>, [f32; 3], usize)>,
}

pub struct PropTintPlugin;

impl Plugin for PropTintPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PropTintState>().add_systems(
            Update,
            (
                timed(Group::Fx, prop_daynight_tint),
                patch_transmissive_gltf,
            ),
        );
    }
}

/// Marcador inserido pelo spawn quando a URL do glTF contém "crystal"/"glass"
/// — os materiais da cena ganham transmissão especular quando o glTF carrega
/// (cristais que refratam o mundo + refletem o IBL do céu).
#[derive(Component)]
pub struct TransmissiveGltf;

/// Patch UMA VEZ por material dos glTFs marcados: `specular_transmission`
/// com IOR de quartzo e espessura de seixo — o
/// `ScreenSpaceTransmissionPlugin` (default na câmara 3D) refrata a cena por
/// trás e o env map do IBL preenche o resto. Sem o patch os cristais são
/// pedras azuis opacas.
fn patch_transmissive_gltf(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    children: Query<&Children>,
    roots: Query<(
        Entity,
        &bevy::world_serialization::WorldAssetRoot,
        &TransmissiveGltf,
    )>,
    mesh_materials: Query<&MeshMaterial3d<StandardMaterial>>,
    mut patched: Local<std::collections::HashSet<bevy::asset::AssetId<StandardMaterial>>>,
) {
    // O `WorldAssetRoot` só existe depois do gltf carregar e a cena spawnar
    // — a primeira passada vê a hierarquia completa; no fim o marcador sai
    // para não re-percorrer a árvore todos os frames.
    for (entity, _root, _marker) in roots.iter() {
        let mut stack = vec![entity];
        // Falso enquanto houver um material ainda não carregado: aí o
        // marcador FICA (o `WorldAssetRoot` aparece quando a CENA carrega;
        // os sub-assets — materiais — chegam em paralelo).
        let mut pending = false;
        while let Some(current) = stack.pop() {
            let Ok(kids) = children.get(current) else {
                continue;
            };
            for &kid in kids {
                stack.push(kid);
                let Ok(handle) = mesh_materials.get(kid) else {
                    continue;
                };
                if patched.contains(&handle.id()) {
                    continue;
                }
                // O `insert` no `patched` vinha ANTES do `get_mut`: um
                // material ainda em voo era dado como tratado e nunca mais
                // era revisto — o cristal ficava opaco para sempre.
                let Some(mut material) = materials.get_mut(handle.id()) else {
                    pending = true;
                    continue;
                };
                material.specular_transmission = 0.85;
                material.ior = 1.45;
                material.thickness = 0.6;
                material.perceptual_roughness = 0.05;
                patched.insert(handle.id());
            }
        }
        if !pending {
            commands.entity(entity).remove::<TransmissiveGltf>();
        }
    }
}

/// Reaplica o tint de dia sobre o albedo ORIGINAL preservando o alpha ATUAL
/// do material (não o do original): o fade de cadáveres
/// (`combat::corpse_fade_tick`) desce `base_color.alpha` e um tint com alpha
/// 1 fazia o corpo morto voltar a opaco no tick seguinte do day/night.
/// Puro para testes.
fn tinted_base_color(original: LinearRgba, tint: [f32; 3], current_alpha: f32) -> Color {
    Color::linear_rgb(
        original.red * tint[0],
        original.green * tint[1],
        original.blue * tint[2],
    )
    .with_alpha(current_alpha)
}

/// Materiais de prop tintados por FRAME durante uma passagem.
///
/// Cada `get_mut` num material bindless destrói e recria o bind group (TODO
/// explícito no upstream do bevy_pbr) — ver o gêmeo
/// `CHUNK_MATERIAL_WRITE_BUDGET` em `terrain/layer_material.rs`. O mundo tem
/// centenas de `StandardMaterial` e o amanhecer/anoitecer muda o tint de
/// forma CONTÍNUA durante minutos: sem orçamento, o sistema reescrevia todos
/// 4×/s nessa janela, com o pico dos bind groups num único frame.
const PROP_TINT_BUDGET: usize = 64;

/// Aplica a curva de noite da relva a todos os `StandardMaterial` (glTFs e
/// primitivas) que não sejam `unlit`/emissivos. O alvo recalcula-se ao
/// throttle de 0,25 s; a APLICAÇÃO é orçamentada por frame
/// ([`PROP_TINT_BUDGET`]) — parado, o custo é uma subtração.
fn prop_daynight_tint(
    mut state: ResMut<PropTintState>,
    time: Res<Time>,
    clock: Option<Res<crate::worldsys::DayCycleState>>,
    interior: Option<Res<crate::worldsys::InteriorLighting>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Passagem em curso: aplica o orçamento deste frame e avança o cursor.
    // (`mem::take` para poder tocar em `state.originals` dentro do loop.)
    let mut sweep = std::mem::take(&mut state.sweep);
    let mut finished_tint: Option<[f32; 3]> = None;
    if let Some((ids, tint, cursor)) = &mut sweep {
        let (start, end) = sweep_slice(ids.len(), *cursor, PROP_TINT_BUDGET);
        for id in ids[start..end].iter() {
            let Some(mut material) = materials.get_mut(*id) else {
                continue;
            };
            // Auto-iluminados ficam de fora: marcadores e vidros acesos. (O
            // filtro corre ao montar o roster; o re-check protege materiais
            // que ficaram emissivos desde então.)
            if material.unlit || material.emissive != LinearRgba::BLACK {
                continue;
            }
            let original = *state
                .originals
                .entry(*id)
                .or_insert_with(|| material.base_color.to_linear());
            let current_alpha = material.base_color.to_linear().alpha;
            material.base_color = tinted_base_color(original, *tint, current_alpha);
        }
        *cursor = end;
        if *cursor >= ids.len() {
            finished_tint = Some(*tint);
        }
    }
    state.sweep = if finished_tint.is_some() { None } else { sweep };
    if let Some(tint) = finished_tint {
        state.last_tint = Some(tint);
    }
    // Uma passagem em curso NUNCA é substituída: o guarda antigo
    // (`sweep.is_some() && throttle > 0`) reiniciava o cursor a 0 quando a
    // passagem demorava mais que o throttle (roster > 960 materiais a 64/
    // frame e 0,25 s = 15 frames) — o FIM da lista nunca era visitado e
    // esses props ficavam ao albedo de dia sob luar para sempre. Terminada a
    // passagem (None), o throttle decide a próxima.
    if state.sweep.is_some() {
        return;
    }

    state.throttle -= time.delta_secs();
    if state.throttle > 0.0 {
        return;
    }
    state.throttle = 0.25;

    // ids são Copy — o Vec termina o borrow imutável antes do get_mut.
    let ids: Vec<_> = materials.ids().collect();
    // Materiais que já não existem (troca de cena / glTF recarregado) não
    // podem ficar para sempre no mapa de originais: o asset é removido e o
    // `AssetId` nunca volta, mas a entrada ficava.
    state.originals.retain(|id, _| materials.contains(*id));
    let roster_changed = ids.len() != state.last_len;
    state.last_len = ids.len();

    // Dentro de um interior o tint fica de DIA: a sala é iluminada pelo
    // ambiente fixo (ver `worldsys::InteriorLighting`) e escurecer os
    // materiais com o relógio lá de fora deixava a divisão ilegível.
    let day = if interior.is_some_and(|state| state.active) {
        1.0
    } else {
        clock
            .as_deref()
            .map(|clock| {
                crate::worldsys::daylight_factor(
                    clock.minute_of_day,
                    clock.dawn_minute,
                    clock.dusk_minute,
                )
            })
            .unwrap_or(1.0)
    };
    let tint = day_tint(day);
    if let Some(last) = state.last_tint
        && !roster_changed
        && (tint[0] - last[0]).abs() < 1e-3
        && (tint[1] - last[1]).abs() < 1e-3
        && (tint[2] - last[2]).abs() < 1e-3
    {
        return;
    }

    // Monta o roster ELEGÍVEL uma vez (unlit/emissivos fora) e abre a
    // passagem orçamentada — o `get_mut` do filtro seria de leitura, mas o
    // roster filtrado poupa o cursor de saltar metade dos ids.
    let eligible: Vec<AssetId<StandardMaterial>> = ids
        .into_iter()
        .filter(|id| {
            materials
                .get(*id)
                .is_some_and(|material| !material.unlit && material.emissive == LinearRgba::BLACK)
        })
        .collect();
    state.sweep = Some((eligible, tint, 0));
}

/// Fatia `[início, fim)` de uma passagem orçamentada por frame — pura, para a
/// convergência (cada id visitado uma vez) ser testável.
fn sweep_slice(total: usize, cursor: usize, budget: usize) -> (usize, usize) {
    let start = cursor.min(total);
    (start, start.saturating_add(budget).min(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_at_noon_is_identity() {
        let noon = day_tint(1.0);
        assert!(noon.iter().all(|c| (c - 1.0).abs() < 1e-4));
    }

    #[test]
    fn tint_at_night_darkens_below_half() {
        let night = day_tint(0.0);
        assert!(night.iter().all(|c| *c < 0.5), "{night:?}");
    }

    /// Uma passagem orçamentada em curso não pode ser reiniciada: o guarda
    /// antigo só travava com `throttle > 0` e, num roster grande (a passagem
    /// demora mais que os 0,25 s do throttle), o cursor voltava a 0 e o FIM
    /// da lista nunca era tintado (props ao albedo de dia sob luar).
    #[test]
    fn sweep_in_progress_is_never_restarted() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<PropTintState>()
            .add_systems(Update, prop_daynight_tint);
        // Noite cheia: `daylight_factor(0, 330, 1170) == 0` ⇒ tint escuro.
        app.insert_resource(crate::worldsys::DayCycleState {
            minute_of_day: 0.0,
            minutes_per_real_second: 1.0,
            dawn_minute: 330.0,
            dusk_minute: 1170.0,
            ambient_day: 1.0,
            ambient_night: 0.2,
            ambient_reference: 1.0,
            drive_ambient: true,
            max_sun_elevation: 60.0,
            sun_azimuth_base: 205.0,
            min_sun_elevation: 8.0,
            day_speed_scale: 1.0,
            night_speed_scale: 1.0,
            night_cap_minute: 0.0,
        });
        let handles: Vec<_> = {
            let mut assets = app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>();
            (0..200)
                .map(|_| {
                    assets.add(StandardMaterial {
                        base_color: Color::WHITE,
                        ..Default::default()
                    })
                })
                .collect()
        };
        // 1.º update abre a passagem (cursor 0); os seguintes aplicam 64/frame.
        app.update();
        for _ in 0..8 {
            // Simula frames lentos: o throttle já devia ter expirado.
            app.world_mut().resource_mut::<PropTintState>().throttle = 0.0;
            app.update();
        }
        let night = day_tint(0.0);
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        for handle in &handles {
            let color = materials
                .get(handle.id())
                .expect("material ainda existe")
                .base_color
                .to_linear();
            for (channel, expected) in [(0, night[0]), (1, night[1]), (2, night[2])] {
                let got = [color.red, color.green, color.blue][channel];
                assert!(
                    (got - expected).abs() < 1e-3,
                    "material {handle:?} ficou sem o tint de noite: {color:?} \
                     (canal {channel}: {got} != {expected})"
                );
            }
        }
    }

    /// O material de um glTF pode chegar DEPOIS do `WorldAssetRoot` (os
    /// sub-assets carregam em paralelo). Marcar o id como "já tratado" antes
    /// de o `get_mut` ter sucesso deixava o cristal opaco para sempre.
    #[test]
    fn transmissive_patch_waits_for_the_material_asset() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, patch_transmissive_gltf);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        // Sub-asset ainda em voo: o handle existe (a cena referencia-o) mas
        // o asset não.
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .remove(handle.id());
        let root = app
            .world_mut()
            .spawn((
                bevy::world_serialization::WorldAssetRoot(Handle::default()),
                TransmissiveGltf,
            ))
            .id();
        let mesh = app
            .world_mut()
            .spawn(MeshMaterial3d(handle.clone()))
            .id();
        app.world_mut().entity_mut(root).add_child(mesh);
        app.update();
        assert!(
            app.world().get::<TransmissiveGltf>(root).is_some(),
            "sem o material, o marcador fica para a próxima passagem"
        );
        // O material chega (mesmo id, geracional — `remove` não recicla).
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .insert(handle.id(), StandardMaterial::default())
            .expect("id ainda válido");
        app.update();
        let material = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(handle.id())
            .expect("material carregado");
        assert_eq!(material.specular_transmission, 0.85);
        assert_eq!(material.ior, 1.45);
        assert!(
            app.world().get::<TransmissiveGltf>(root).is_none(),
            "tratado o material, o marcador sai"
        );
    }

    /// R2-G8: o tint preserva o alpha ATUAL do material — o fade de
    /// cadáveres desce o alpha e a reescrita com alpha 1 o desfazia (o
    /// corpo "acendia" de novo a cada tick do day/night, throttle 0,25 s).
    #[test]
    fn tinted_base_color_preserves_current_alpha() {
        let original = LinearRgba::rgb(0.8, 0.4, 0.2);
        let night = day_tint(0.0);
        // Cadáver a meio do fade (alpha 0.35): o tint de noite mantém o 0.35.
        let tinted = tinted_base_color(original, night, 0.35);
        let linear = tinted.to_linear();
        assert!(
            (linear.alpha - 0.35).abs() < 1e-5,
            "alpha {:#?}",
            linear.alpha
        );
        // RGB multiplicado pelo tint sobre o ORIGINAL (não compõe).
        assert!((linear.red - original.red * night[0]).abs() < 1e-5);
        assert!((linear.green - original.green * night[1]).abs() < 1e-5);
        assert!((linear.blue - original.blue * night[2]).abs() < 1e-5);
        // Dia (tint identidade): cor volta ao original, alpha corrente fica.
        let noon = day_tint(1.0);
        let back = tinted_base_color(original, noon, 0.9).to_linear();
        assert!((back.red - original.red).abs() < 1e-5);
        assert!((back.alpha - 0.9).abs() < 1e-5);
    }
}
