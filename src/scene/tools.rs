//! Ferramentas geradoras: o que antes corria em runtime (spawners) passa a ser executado À
//! MÃO — por um agente (`viber tool scatter`, `viber.tool.scatter`) ou no ViberEditor — e o
//! resultado entra no documento como UMA transação de operações (desfazível, gravada no
//! ficheiro do nó, igual venha de onde vier).
//!
//! `scatter` regenera o `<Group generated-by="{id}">` de um `<Scatter id>`: recalcula as
//! colocações sobre o terreno ATUAL (o mesmo `compute_placements` e os mesmos gates do jogo),
//! com a ocupação das instâncias geradas pelos OUTROS scatters, e substitui o grupo antigo.
//! Um seletor de `<Vegetation instances="…vinst">` regenera o BINÁRIO pré-gerado (o XML não
//! muda — sem transação, sem undo; ver [`scatter`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use bevy::math::Affine3A;
use bevy::prelude::*;
use serde_json::{Value, json};

use super::SceneSource;
use super::doc::{NodeRef, SceneDoc};
use super::ops::{EditOp, fragment_xml};
use crate::spawner::{SpawnExclusions, SpawnOccupancy};
use crate::terrain::cliffs::CliffMask;
use crate::terrain::runtime::TerrainRuntime;
use crate::xml::NodeOrigin;

/// Diferença de cota (m) abaixo da qual `settle` não reescreve um nó.
const SETTLE_TOLERANCE: f32 = 0.02;

/// Raio de ocupação de uma instância já existente quando o template não declara pegada.
const DEFAULT_FOOTPRINT: f32 = 0.8;

/// O resultado de uma ferramenta: as operações a aplicar (numa transação) e um resumo.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub label: String,
    pub ops: Vec<EditOp>,
    pub placed: usize,
    pub warnings: Vec<String>,
    /// Campos extra do resumo (o scatter de `<Vegetation>` reporta o binário
    /// gravado e o `undo` que o History não cobre); `Null` nas restantes.
    pub info: Value,
    /// Binário já gravado pela ferramenta (scatter de `<Vegetation>`): o path e
    /// os bytes ANTERIORES (`None` = não existia) — o `run_offline` com
    /// `--dry-run` repõe-nos (o History não cobre ficheiros binários).
    pub undo_binary: Option<(PathBuf, Option<Vec<u8>>)>,
}

impl ToolOutput {
    pub fn summary(&self) -> Value {
        let mut summary = json!({
            "label": self.label,
            "placed": self.placed,
            "ops": self.ops.len(),
            "warnings": self.warnings,
        });
        if let (Value::Object(base), Value::Object(extra)) = (&mut summary, &self.info) {
            for (key, value) in extra {
                base.insert(key.clone(), value.clone());
            }
        }
        summary
    }
}

/// Regenera o resultado do gerador em `selector`. Precisa de um mundo com o terreno
/// carregado (`TerrainRuntime`) e o documento (`SceneDoc`) — a engine viva, o editor ou a App
/// headless de [`run_offline`].
///
/// Aceita um `<Scatter id>` (regenera o grupo `generated-by`, transação desfazível) ou um
/// `<Vegetation instances="…vinst">` (regenera o BINÁRIO pré-gerado: as colocações
/// recomputam, o `.vinst` é gravado ATÓMICO — e o XML não muda, logo não há EditOp nem undo
/// pelo History; `git` ou um novo scatter repõem).
pub fn scatter(world: &mut World, selector: &str) -> Result<ToolOutput> {
    let (node, recipe, existing) = {
        let doc = world
            .get_resource::<SceneDoc>()
            .ok_or_else(|| anyhow!("scatter: sem documento"))?;
        let node = doc.resolve(selector)?;
        let recipe = doc
            .node(&node)
            .cloned()
            .ok_or_else(|| anyhow!("scatter: `{selector}` não é um nó"))?;
        if recipe.tag.eq_ignore_ascii_case("vegetation") {
            return scatter_vegetation(world, recipe);
        }
        if !recipe.tag.eq_ignore_ascii_case("scatter") {
            bail!("scatter: `{selector}` é <{}>, não <Scatter>", recipe.tag);
        }
        let id = recipe
            .attr("id")
            .ok_or_else(|| anyhow!("scatter: <Scatter> sem `id`"))?;
        let existing = doc.find_by_attr("generated-by", id).into_iter().next();
        (node, recipe, existing)
    };
    let id = recipe.attr("id").expect("verificado").to_owned();
    let mut warnings = Vec::new();
    let mut spec = crate::recipes::parse_scatter(&recipe, &mut warnings)?;
    if spec.template_urls.is_empty() {
        bail!("scatter `{id}`: o template não tem glTF");
    }
    if spec.avoid_overlaps && spec.footprint_radius <= 0.0 {
        let roots = doc_asset_roots(world.resource::<SceneDoc>());
        spec.footprint_radius = auto_footprint(&spec.template_urls, &roots);
    }

    // Frames mundo das entidades vivas, por origem (para ocupação e para o referencial).
    let frames = live_frames(world);
    let (mut occupancy, others) = doc_occupancy(world, &frames, Some(&id));
    let doc = world.resource::<SceneDoc>();

    let runtime = world
        .get_resource::<TerrainRuntime>()
        .ok_or_else(|| anyhow!("scatter: o terreno não está carregado (TerrainRuntime)"))?;
    let cliffs = world.get_resource::<CliffMask>();
    let (instances, stats) = crate::placement::place(&spec, &mut occupancy, runtime, cliffs);
    if instances.is_empty() {
        warnings.push(format!("scatter `{id}`: 0 colocadas ({stats:?})"));
    }

    // O grupo gerado fica onde o antigo estava (ou logo a seguir à receita).
    // A raiz de um ficheiro (`fragmento.xml#`) não tem pai onde inserir: erro,
    // não `expect` (a receita pode ser a raiz de um include).
    let (parent, index, replace) = match &existing {
        Some(old) => {
            let (Some(parent), Some(index)) = (old.parent(), old.path.last().copied()) else {
                bail!("scatter `{id}`: o grupo gerado é a raiz do ficheiro — envolve-o num <Group>");
            };
            (parent, index, true)
        }
        None => {
            let (Some(parent), Some(index)) = (node.parent(), node.path.last().copied()) else {
                bail!("scatter `{id}`: a receita é a raiz do ficheiro — envolve-a num <Group>");
            };
            (parent, index + 1, false)
        }
    };
    let parent_world = node_frame(doc, &parent, &frames);
    let output = crate::bake::generated_group(
        &recipe,
        &id,
        &instances,
        &spec.template_urls,
        parent_world,
        &format!("inst-{id}"),
        0,
    );
    let parent_selector = doc.selector(&parent);
    let mut ops = vec![EditOp::Insert {
        parent: parent_selector,
        index: Some(index),
        xml: fragment_xml(std::slice::from_ref(&output)),
    }];
    if replace {
        // O antigo desceu um índice com a inserção.
        ops.push(EditOp::Remove {
            node: doc.selector(&parent.child(index + 1)),
        });
    }
    debug!(
        "scatter `{id}`: {} placed, {others} sólidos (gerados + autorais) na ocupação",
        instances.len()
    );
    Ok(ToolOutput {
        label: format!("scatter {id} ({} instâncias)", instances.len()),
        placed: instances.len(),
        ops,
        warnings,
        info: Value::Null,
        undo_binary: None,
    })
}

/// Pincel de VEGETAÇÃO: coloca instâncias de um `<Scatter>` RESTRICTAS à
/// região do cursor (`center`/`radius`, XZ mundo) — a mesma colocação da
/// ferramenta (`placement::place`), mas a contagem escala pela ÁREA do dab
/// (densidade→count quando o XML usa densidade; proporção quando usa count).
/// As instâncias APPEND no grupo gerado existente (sem o remover) — o undo do
/// pincel é a transação coalescida do traço.
pub fn scatter_in_region(
    world: &mut World,
    selector: &str,
    center: bevy::math::Vec2,
    radius: f32,
) -> Result<ToolOutput> {
    let (node, recipe, existing) = {
        let doc = world
            .get_resource::<SceneDoc>()
            .ok_or_else(|| anyhow!("scatter_brush: sem documento"))?;
        let node = doc.resolve(selector)?;
        let recipe = doc
            .node(&node)
            .cloned()
            .ok_or_else(|| anyhow!("scatter_brush: `{selector}` não é um nó"))?;
        if !recipe.tag.eq_ignore_ascii_case("scatter") {
            bail!(
                "scatter_brush: `{selector}` é <{}>, não <Scatter>",
                recipe.tag
            );
        }
        let id = recipe
            .attr("id")
            .ok_or_else(|| anyhow!("scatter_brush: <Scatter> sem `id`"))?;
        let existing = doc.find_by_attr("generated-by", id).into_iter().next();
        (node, recipe, existing)
    };
    let id = recipe.attr("id").expect("verificado").to_owned();
    let mut warnings = Vec::new();
    let mut spec = crate::recipes::parse_scatter(&recipe, &mut warnings)?;
    if spec.template_urls.is_empty() {
        bail!("scatter_brush `{id}`: o template não tem glTF");
    }
    if spec.avoid_overlaps && spec.footprint_radius <= 0.0 {
        let roots = doc_asset_roots(world.resource::<SceneDoc>());
        spec.footprint_radius = auto_footprint(&spec.template_urls, &roots);
    }
    // região = o dab; contagem escala pela fração de área
    let half = spec_region_half(&recipe, &mut warnings);
    let full_area = (half.x * 2.0).max(1.0) * (half.y * 2.0).max(1.0);
    let dab_area = (radius * 2.0).powi(2).min(full_area);
    let fraction = (dab_area / full_area).clamp(0.001, 1.0);
    spec.region_min = [center.x - radius, 0.0, center.y - radius];
    spec.region_max = [center.x + radius, 0.0, center.y + radius];
    if spec.count > 0 {
        spec.count = ((spec.count as f32) * fraction).round().max(1.0) as u32;
    }
    let frames = live_frames(world);
    let (mut occupancy, _others) = doc_occupancy(world, &frames, Some(&id));
    let runtime = world
        .get_resource::<TerrainRuntime>()
        .ok_or_else(|| anyhow!("scatter_brush: o terreno não está carregado"))?;
    let cliffs = world.get_resource::<CliffMask>();
    let (instances, stats) = crate::placement::place(&spec, &mut occupancy, runtime, cliffs);
    if instances.is_empty() {
        warnings.push(format!("scatter_brush `{id}`: 0 colocadas ({stats:?})"));
    }
    // APPEND no grupo gerado (ou cria-o a seguir à receita). Nó raiz: sem pai
    // onde inserir — erro claro em vez de panic (vem de seletor de agente).
    let doc = world.resource::<SceneDoc>().clone();
    let parent = match &existing {
        Some(group) if group.is_root() => bail!(
            "scatter_brush `{id}`: o grupo gerado é a raiz do ficheiro — envolve-o num <Group>"
        ),
        Some(group) => group.clone(),
        None => node.parent().ok_or_else(|| {
            anyhow!("scatter_brush `{id}`: a receita é a raiz do ficheiro — envolve-a num <Group>")
        })?,
    };
    let parent_world = node_frame(&doc, &parent, &frames);
    let name_start = existing
        .as_ref()
        .and_then(|group| doc.node(group))
        .map(|group| group.children.len())
        .unwrap_or(0);
    let output = crate::bake::generated_group(
        &recipe,
        &id,
        &instances,
        &spec.template_urls,
        parent_world,
        &format!("inst-{id}"),
        name_start,
    );
    let mut ops = Vec::new();
    if existing.is_some() {
        for child in &output.children {
            ops.push(EditOp::Insert {
                parent: doc.selector(&parent),
                index: None,
                xml: fragment_xml(std::slice::from_ref(child)),
            });
        }
    } else {
        // Sem grupo, o dab tem de criar o `<Group generated-by>`. Inserir só
        // os filhos deixava a receita sem grupo e o dab seguinte repetia
        // `inst-…-0000`.
        let scatter_parent = node.parent().ok_or_else(|| {
            anyhow!("scatter_brush `{id}`: a receita é a raiz do ficheiro — envolve-a num <Group>")
        })?;
        let index = node.path.last().copied().unwrap_or(0) + 1;
        ops.push(EditOp::Insert {
            parent: doc.selector(&scatter_parent),
            index: Some(index),
            xml: fragment_xml(std::slice::from_ref(&output)),
        });
    }
    Ok(ToolOutput {
        label: format!("pincel {id} ({} instâncias)", instances.len()),
        placed: instances.len(),
        ops,
        warnings,
        info: Value::Null,
        undo_binary: None,
    })
}

/// A MEIA-largura XZ da região declarada no `<Scatter>` (para escalar a
/// contagem por área). Ausente/zero = 100×100 m.
fn spec_region_half(recipe: &crate::xml::XmlNode, warnings: &mut Vec<String>) -> bevy::math::Vec2 {
    let spec = crate::recipes::parse_scatter(recipe, warnings).ok();
    let min = spec.as_ref().map(|s| s.region_min).unwrap_or([0.0; 3]);
    let max = spec.as_ref().map(|s| s.region_max).unwrap_or([0.0; 3]);
    let half_x = {
        let extent = (max[0] - min[0]).abs();
        if extent < 1e-3 { 50.0 } else { extent * 0.5 }
    };
    let half_z = {
        let extent = (max[2] - min[2]).abs();
        if extent < 1e-3 { 50.0 } else { extent * 0.5 }
    };
    bevy::math::Vec2::new(half_x, half_z)
}

/// Regenera o `.vinst` de um `<Vegetation instances>` sobre o terreno ATUAL — o mesmo
/// caminho do bake: `compute_placements` com a spec canónica do parse (densidade→count,
/// cap, pegada), a ocupação das exclusões + dos geradores do documento, e a gravação
/// ATÓMICA em [`crate::vinst::write_atomic`].
fn scatter_vegetation(world: &mut World, recipe: crate::xml::XmlNode) -> Result<ToolOutput> {
    let Some(rel) = recipe.attr("instances").map(str::to_owned) else {
        bail!(
            "scatter: `<Vegetation>` sem `instances` — só se regenera folhagem pré-gerada \
             (`viber bake` escreve o .vinst)"
        );
    };
    // A spec EXATA do arranque (density→count com cap, footprint 0.4, gates): re-parse do
    // nó pelo parser canónico em vez de duplicarmos os attrs aqui.
    let mut warnings = Vec::new();
    let parsed = crate::recipes::parse_world(&[], std::slice::from_ref(&recipe))?;
    warnings.extend(parsed.warnings);
    let spec = match parsed.entities.into_iter().next().map(|entity| entity.kind) {
        Some(crate::recipes::EntityKind::Vegetation { spec }) => spec.to_spawner_spec(),
        _ => bail!("scatter: o nó selecionado não parseia como <Vegetation>"),
    };

    let frames = live_frames(world);
    let (mut occupancy, others) = doc_occupancy(world, &frames, None);
    let runtime = world
        .get_resource::<TerrainRuntime>()
        .ok_or_else(|| anyhow!("scatter: o terreno não está carregado (TerrainRuntime)"))?;
    let cliffs = world.get_resource::<CliffMask>();
    let (instances, stats) = crate::placement::place(&spec, &mut occupancy, runtime, cliffs);
    if instances.is_empty() {
        warnings.push(format!(
            "scatter vegetation `{rel}`: 0 colocadas ({stats:?})"
        ));
    }

    let doc = world.resource::<SceneDoc>();
    // `instances` é relativo à pasta do mundo; um `/` inicial (forma das
    // asset roots) não pode fazer `Path::join` descartar a pasta — o
    // `spawn_parsed` já o aparava, esta ferramenta escrevia no filesystem
    // root.
    let path: PathBuf = doc.root_dir.join(rel.trim_start_matches('/'));
    let previous = if path.is_file() {
        Some(std::fs::read(&path).map_err(|e| anyhow!("ler {}: {e}", path.display()))?)
    } else {
        None
    };
    let before = previous
        .as_deref()
        .and_then(|bytes| crate::vinst::decode(bytes).ok())
        .map_or(0, |old: Vec<_>| old.len());
    crate::vinst::write_atomic(&path, &instances)?;
    debug!(
        "scatter vegetation `{rel}`: {before}→{} instâncias, {others} sólidos \
         (gerados + autorais) na ocupação",
        instances.len()
    );
    // O XML não muda (o attr `instances` já aponta ao ficheiro): ZERO EditOps, nada para
    // o History desfazer — o relatório diz-o explicitamente.
    Ok(ToolOutput {
        label: format!("scatter vegetation {rel} ({} instâncias)", instances.len()),
        placed: instances.len(),
        ops: Vec::new(),
        warnings,
        info: json!({
            "kind": "vegetation",
            "instances_file": rel,
            "path": path.display().to_string(),
            "before": before,
            "after": instances.len(),
            "undo": "binary vinst: não desfazível via History — git ou re-scatter",
        }),
        undo_binary: Some((path, previous)),
    })
}

/// A ocupação de partida de um scatter: as `<SpawnExclusion>` do mundo (o arranque
/// publica-as como recurso) mais TUDO o que já ocupa espaço no documento — as
/// instâncias dos outros geradores (`generated-by`, `self_id` exclui a do próprio) e os
/// props autorais com `<GltfScene>`. Sem os autorais, uma árvore gerada nascia dentro da
/// pedra de musgo que o autor pôs à mão (o caso real do pântano); sem a pegada REAL de
/// cada template, o registo de 0,8 m deixava as árvores grandes passarem umas pelas outras.
/// Devolve a ocupação e quantos sólidos entraram nela.
fn doc_occupancy(
    world: &mut World,
    frames: &HashMap<NodeOrigin, Affine3A>,
    self_id: Option<&str>,
) -> (SpawnOccupancy, usize) {
    let mut occupancy = SpawnOccupancy::new();
    if let Some(exclusions) = world.get_resource::<SpawnExclusions>() {
        for exclusion in &exclusions.0 {
            occupancy.register_exclusion(exclusion.center.x, exclusion.center.y, exclusion.radius);
        }
    }
    let doc = world.resource::<SceneDoc>();
    // A pegada vem do GLB em disco (o mesmo AABB que o audit usa); as roots do
    // config resolvem `/assets/...` para o pool. A cache evita reler o mesmo
    // template centenas de vezes.
    let roots = doc_asset_roots(doc);
    let mut footprints: HashMap<String, Option<f32>> = HashMap::new();
    let mut others = 0usize;
    doc.walk(|node, xml| {
        if !xml.tag.eq_ignore_ascii_case("gltfscene") {
            return;
        }
        // Templates de `<Scatter>`/`<Vegetation>` e definições de `<Prototype>` não são
        // entidades no sítio: quem os materializa é o grupo `generated-by` ou a `<Use>`
        // expandida. Registá-los aqui punha ocupação fantasma na origem do mundo.
        let mut generated_by: Option<String> = None;
        let mut current = Some(node.clone());
        while let Some(at) = current {
            let Some(ancestor) = doc.node(&at) else {
                break;
            };
            let tag = ancestor.tag.to_ascii_lowercase();
            if matches!(
                tag.as_str(),
                "scatter" | "staticspawner" | "dynamicspawner" | "vegetation" | "prototype"
            ) {
                return;
            }
            if generated_by.is_none() {
                generated_by = ancestor.attr("generated-by").map(str::to_owned);
            }
            if at.is_root() {
                break;
            }
            current = at.parent();
        }
        if generated_by
            .as_deref()
            .is_some_and(|by| Some(by) == self_id)
        {
            return;
        }
        let Some(url) = xml.attr("url") else {
            return;
        };
        let Some(frame) = frames.get(&doc.origin_of(node)) else {
            return;
        };
        let (scale, _, translation) = frame.to_scale_rotation_translation();
        let scale_xz = scale.x.abs().max(scale.z.abs());
        let footprint =
            footprint_for(&roots, url, &mut footprints).unwrap_or(DEFAULT_FOOTPRINT) * scale_xz;
        if footprint > 0.0 {
            occupancy.register(translation.x, translation.z, footprint);
            others += 1;
        }
    });
    (occupancy, others)
}

/// Roots de assets do jogo (pasta do mundo + extras do `config.yaml`), na mesma ordem
/// que o runtime e o audit usam. `config` ausente cai na pasta do mundo.
fn doc_asset_roots(doc: &SceneDoc) -> Vec<PathBuf> {
    crate::config::load(&doc.root_dir)
        .map(|config| config.asset_roots(&doc.root_dir))
        .unwrap_or_else(|_| vec![doc.root_dir.clone()])
}

/// Pegada de um `<Scatter>` sem `footprint-radius`: a MAIOR meia-largura entre os
/// templates (o mesmo critério do auto-footprint do runtime). O 0,8 fixo antigo
/// deixava carvalhos de 4 m de copa (meia-largura 2,05) passarem uns pelos outros.
/// `DEFAULT_FOOTPRINT` só quando nenhum GLB tem bounds.
pub(crate) fn auto_footprint(urls: &[String], roots: &[PathBuf]) -> f32 {
    let mut cache = HashMap::new();
    let mut largest = 0.0f32;
    for url in urls {
        if let Some(radius) = footprint_for(roots, url, &mut cache) {
            largest = largest.max(radius);
        }
    }
    if largest > 0.0 {
        largest
    } else {
        DEFAULT_FOOTPRINT
    }
}

/// Pegada XZ de um template glTF (maior meia-largura do AABB), lida do disco e cacheada
/// por URL. `None` = asset ausente/sem bounds: o chamador aplica o fallback conservador.
fn footprint_for(
    roots: &[PathBuf],
    url: &str,
    cache: &mut HashMap<String, Option<f32>>,
) -> Option<f32> {
    if let Some(hit) = cache.get(url) {
        return *hit;
    }
    let rel = url.trim_start_matches('/');
    let radius = roots
        .iter()
        .map(|root| root.join(rel))
        .find(|path| path.is_file())
        .and_then(|path| crate::glb_bounds::read(&path))
        .map(|bounds| bounds.footprint_radius());
    cache.insert(url.to_string(), radius);
    radius
}

/// Assenta no terreno ATUAL do documento (o heightmap com as pinceladas, gravadas ou não):
/// aplicado a um grupo (`<Group>`, o resultado de um scatter…) assenta os filhos diretos; a
/// outro nó, assenta-o a ele. O pivô vai para o chão em XZ mundo (+ `offset`); rotação e
/// escala ficam. Só documento — sem engine, exato num mundo estático (poses finais).
pub fn settle(doc: &mut SceneDoc, selector: &str, offset: f32) -> Result<ToolOutput> {
    let node = doc.resolve(selector)?;
    let targets: Vec<NodeRef> = {
        let xml = doc.node(&node).ok_or_else(|| {
            anyhow!("settle: `{selector}` é a raiz de um ficheiro — escolhe um nó")
        })?;
        let is_group = xml.tag.eq_ignore_ascii_case("group") && !xml.children.is_empty();
        if is_group {
            (0..xml.children.len())
                .map(|index| node.child(index))
                .collect()
        } else {
            vec![node.clone()]
        }
    };
    // Carrega o heightmap uma vez (falha clara num mundo sem terreno pré-gerado).
    doc.terrain_mut()?;
    let mut ops = Vec::new();
    let mut warnings = Vec::new();
    for target in &targets {
        let Some(parent) = target.parent() else {
            continue;
        };
        let xml = doc.node(target).expect("filho existente").clone();
        let Ok(local) = super::pose::read_pose(&xml) else {
            warnings.push(format!("{}: pose ilegível — saltado", doc.selector(target)));
            continue;
        };
        let frame = super::pose::world_frame(doc, &parent);
        let world = frame.transform_point3(local.translation);
        let ground = doc
            .terrain
            .as_ref()
            .expect("carregado")
            .sample(world.x, world.z)
            + offset;
        // 2 cm: o bake assenta pela superfície da MALHA, e a amostra bilinear do heightmap
        // difere uns milímetros — abaixo disto não é desassentamento, é ruído de diff.
        if (world.y - ground).abs() < SETTLE_TOLERANCE {
            continue;
        }
        let settled = frame
            .inverse()
            .transform_point3(bevy::math::Vec3::new(world.x, ground, world.z));
        let pose = super::pose::Pose {
            translation: settled,
            ..local
        };
        ops.push(super::pose::pose_op(
            &doc.selector(target),
            &xml,
            pose,
            super::pose::PoseParts::TRANSLATION,
        )?);
    }
    Ok(ToolOutput {
        label: format!("settle {selector} ({} nó(s))", ops.len()),
        placed: ops.len(),
        ops,
        warnings,
        info: Value::Null,
        undo_binary: None,
    })
}

/// Normaliza as poses para a forma canónica (`translation`/`euler` graus/`scale`, 4 casas,
/// sem aliases `pos`/`rotation`/`transform`) — em todo o mundo ou na subárvore de
/// `selector`. Só toca atributos de pose que JÁ existem e só quando a forma muda; a pose em
/// si fica igual. Uma transação: mundos antigos (ou bakeados antes do formatter) passam a dar
/// diffs limpos à primeira edição.
pub fn canonicalize(doc: &SceneDoc, selector: Option<&str>) -> Result<ToolOutput> {
    let scope = selector.map(|selector| doc.resolve(selector)).transpose()?;
    let mut targets = Vec::new();
    doc.walk(|node, xml| {
        let inside = scope
            .as_ref()
            .is_none_or(|scope| node.file == scope.file && node.path.starts_with(&scope.path));
        if inside {
            targets.push((node.clone(), xml.clone()));
        }
    });
    let mut ops = Vec::new();
    let mut warnings = Vec::new();
    for (node, xml) in targets {
        let has_pose = [
            "translation",
            "pos",
            "euler",
            "rotation",
            "scale",
            "transform",
        ]
        .iter()
        .any(|key| xml.attr(key).is_some());
        if !has_pose {
            continue;
        }
        let pose = match super::pose::read_pose(&xml) {
            Ok(pose) => pose,
            Err(error) => {
                warnings.push(format!("{}: {error:#}", doc.selector(&node)));
                continue;
            }
        };
        // Nenhuma parte "muda": o pose_op reescreve só o que não está canónico (aliases,
        // casas a mais) e deixa intacta uma rotação já canónica — idempotente por construção.
        let unchanged_parts = super::pose::PoseParts {
            translation: false,
            rotation: false,
            scale: false,
        };
        let op = super::pose::pose_op(&doc.selector(&node), &xml, pose, unchanged_parts)?;
        let EditOp::Set { attrs, .. } = &op else {
            continue;
        };
        let unchanged = attrs
            .iter()
            .all(|(key, value)| xml.attr(key) == value.as_deref());
        if !unchanged {
            ops.push(op);
        }
    }
    Ok(ToolOutput {
        label: format!("canonicalize ({} nó(s))", ops.len()),
        placed: ops.len(),
        ops,
        warnings,
        info: Value::Null,
        undo_binary: None,
    })
}

/// `GlobalTransform` das entidades vivas com proveniência, por origem (a primeira de cada).
fn live_frames(world: &mut World) -> HashMap<NodeOrigin, Affine3A> {
    let mut query = world.query::<(&SceneSource, &GlobalTransform)>();
    let mut frames = HashMap::new();
    for (source, global) in query.iter(world) {
        frames
            .entry((*source.0).clone())
            .or_insert_with(|| global.affine());
    }
    frames
}

/// Referencial mundo dos FILHOS de `node`: o frame da entidade dele ou do ancestral mais
/// próximo que tenha entidade; a raiz de um ficheiro incluído continua pelo `<Include>`.
pub fn node_frame(
    doc: &SceneDoc,
    node: &NodeRef,
    frames: &HashMap<NodeOrigin, Affine3A>,
) -> Affine3A {
    let mut current = node.clone();
    for _ in 0..256 {
        if !current.is_root() {
            if let Some(frame) = frames.get(&doc.origin_of(&current)) {
                return *frame;
            }
            current = current.parent().expect("não-raiz");
            continue;
        }
        let includer = (0..doc.files.len()).find_map(|file| {
            doc.files[file]
                .includes
                .iter()
                .find(|(_, target)| *target == current.file)
                .map(|(path, _)| NodeRef::new(file, path.clone()))
        });
        let Some(include) = includer else {
            return Affine3A::IDENTITY;
        };
        // A pose do `<Include>` é o referencial dos filhos. Saltar para o pai
        // gravava o scatter no espaço errado.
        if let Some(frame) = frames.get(&doc.origin_of(&include)) {
            return *frame;
        }
        match include.parent() {
            Some(parent) => current = parent,
            None => return Affine3A::IDENTITY,
        }
    }
    Affine3A::IDENTITY
}

/// `viber tool scatter` offline: monta a App headless do jogo sobre o mundo (spawn + terreno),
/// corre a ferramenta, aplica o lote ao documento e grava (salvo `dry_run`).
pub fn run_offline(world_path: &Path, selector: &str, dry_run: bool) -> Result<Value> {
    let world_path = std::path::absolute(world_path)?;
    let world_dir = crate::project::root_of(&world_path);
    let config = crate::config::load(&world_dir)?;
    let loaded = crate::xml::include::load_world(&world_path)?;
    let parsed = crate::recipes::parse_world(&loaded.root_attrs, &loaded.nodes)?;
    let mut app = crate::bake::headless_app(&world_path, &config, parsed)?;
    app.add_systems(
        bevy::app::Startup,
        (
            crate::recipes::spawn::startup,
            crate::terrain::runtime::bootstrap,
        )
            .chain(),
    );
    app.insert_resource(SceneDoc::load(&world_path)?);
    app.update();
    let output = scatter(app.world_mut(), selector)?;
    let mut doc = app
        .world_mut()
        .remove_resource::<SceneDoc>()
        .ok_or_else(|| anyhow!("documento"))?;
    let tx = super::ops::apply_batch(&mut doc, &output.label, &output.ops)?;
    let files: Vec<String> = doc.dirty_paths();
    if !dry_run {
        doc.save()?;
    } else if let Some((path, previous)) = &output.undo_binary {
        // `--dry-run` com um binário de por meio (scatter de `<Vegetation>`): a
        // ferramenta JÁ gravou o .vinst — repomos os bytes anteriores para o
        // dry-run ficar honesto (não havia History para o fazer).
        match previous {
            Some(bytes) => {
                std::fs::write(path, bytes).map_err(|e| anyhow!("repôr {}: {e}", path.display()))?
            }
            None => {
                std::fs::remove_file(path).map_err(|e| anyhow!("apagar {}: {e}", path.display()))?
            }
        }
    }
    let mut out = super::cli::transaction_json(&doc, &tx, &files, !dry_run);
    out["tool"] = output.summary();
    Ok(out)
}

/// Lista os geradores do mundo (o "menu" de ferramentas de um agente): as receitas
/// `<Scatter id>` e os `<Vegetation instances>` pré-gerados (regeneráveis pela MESMA
/// ferramenta `scatter` — a kind distingue-os no output).
pub fn list_scatters(doc: &SceneDoc) -> Value {
    let mut out = Vec::new();
    doc.walk(|node, xml| {
        let tag = xml.tag.to_ascii_lowercase();
        if tag == "scatter" {
            let id = xml.attr("id").unwrap_or_default();
            let generated = doc.find_by_attr("generated-by", id);
            out.push(json!({
                "kind": "scatter",
                "selector": doc.selector(node),
                "id": id,
                "output": generated.first().map(|g| doc.selector(g)),
                "instances": generated.first().and_then(|g| doc.children(g)).map_or(0, Vec::len),
            }));
        } else if tag == "vegetation" && xml.attr("instances").is_some() {
            let file = xml.attr("instances").unwrap_or_default();
            let count = std::fs::read(doc.root_dir.join(file.trim_start_matches('/')))
                .ok()
                .and_then(|bytes| crate::vinst::decode(&bytes).ok())
                .map_or(0, |instances: Vec<_>| instances.len());
            out.push(json!({
                "kind": "vegetation",
                "selector": doc.selector(node),
                "instances_file": file,
                "instances": count,
            }));
        }
    });
    json!(out)
}

/// `viber tool …` — ferramentas geradoras (a mesma lógica do bridge `viber.tool.*` e do editor).
#[derive(Debug, clap::Subcommand)]
pub enum ToolCommand {
    /// Lista os geradores `<Scatter>` do mundo (seletor, id, grupo gerado, nº de instâncias)
    List,
    /// Regenera um gerador sobre o terreno atual: `<Scatter id>` substitui o grupo
    /// gerado (uma transação); `<Vegetation instances>` regrava o binário .vinst
    /// (sem transação — o History não cobre binários)
    Scatter {
        /// Seletor do `<Scatter>`/`<Vegetation>` (`id=forest-pines`, `world/vegetation/forest.xml#4`)
        selector: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Normaliza as poses para a forma canónica (sem mudar a pose): diffs limpos daqui em diante
    Canonicalize {
        /// Só esta subárvore (omisso: o mundo todo)
        selector: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Assenta no terreno atual: um grupo assenta os filhos diretos, outro nó assenta-se a si
    /// (depois de editar o terreno, props/árvores ficam enterrados ou a flutuar)
    Settle {
        selector: String,
        /// Metros acima do chão (pivô na base = 0)
        #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
        offset: f32,
        #[arg(long)]
        dry_run: bool,
    },
}

/// O subcomando contra a engine a correr: método `viber.tool.*` + params.
pub fn live_request(command: &ToolCommand) -> Result<(&'static str, Value)> {
    Ok(match command {
        ToolCommand::List => ("viber.tool.list", json!({})),
        ToolCommand::Scatter { dry_run: true, .. } => {
            bail!("--dry-run não existe com --live (a engine aplicaria a edição); corre sem --live")
        }
        ToolCommand::Scatter { selector, .. } => {
            ("viber.tool.scatter", json!({ "selector": selector }))
        }
        ToolCommand::Settle { dry_run: true, .. }
        | ToolCommand::Canonicalize { dry_run: true, .. } => {
            bail!("--dry-run não existe com --live (a engine aplicaria a edição); corre sem --live")
        }
        ToolCommand::Canonicalize { selector, .. } => {
            ("viber.tool.canonicalize", json!({ "selector": selector }))
        }
        ToolCommand::Settle {
            selector, offset, ..
        } => (
            "viber.tool.settle",
            json!({ "selector": selector, "offset": offset }),
        ),
    })
}

/// O subcomando offline contra os ficheiros de `world`.
pub fn run_cli(world: &Path, command: ToolCommand) -> Result<Value> {
    match command {
        ToolCommand::List => Ok(list_scatters(&SceneDoc::load(world)?)),
        ToolCommand::Scatter { selector, dry_run } => run_offline(world, &selector, dry_run),
        ToolCommand::Canonicalize { selector, dry_run } => {
            let mut doc = SceneDoc::load(world)?;
            let output = canonicalize(&doc, selector.as_deref())?;
            let tx = super::ops::apply_batch(&mut doc, &output.label, &output.ops)?;
            let files = doc.dirty_paths();
            if !dry_run {
                doc.save()?;
            }
            let mut out = super::cli::transaction_json(&doc, &tx, &files, !dry_run);
            out["tool"] = output.summary();
            // O relatório de milhares de inversas não serve a ninguém num lote destes.
            out["inverse"] = json!(format!("{} ops (omitidas)", tx.inverse.len()));
            Ok(out)
        }
        ToolCommand::Settle {
            selector,
            offset,
            dry_run,
        } => {
            let mut doc = SceneDoc::load(world)?;
            let output = settle(&mut doc, &selector, offset)?;
            let tx = super::ops::apply_batch(&mut doc, &output.label, &output.ops)?;
            let files = doc.dirty_paths();
            if !dry_run {
                doc.save()?;
            }
            let mut out = super::cli::transaction_json(&doc, &tx, &files, !dry_run);
            out["tool"] = output.summary();
            Ok(out)
        }
    }
}
