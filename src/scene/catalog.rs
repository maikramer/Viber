//! Catálogo "Add Node" (tags conhecidas com snippets válidos) e o drag-drop de `.glb` para o
//! viewport — Fase 3 do editor, com paridade: o diálogo do editor e um agente usam os MESMOS
//! builders (a op `insert` que daqui sai é a que `viber scene insert` aplicaria).

use std::path::Path;

use super::ops::EditOp;
use super::pose;

/// Grupo do diálogo Add Node (ordem de apresentação).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateGroup {
    Content,
    Shape,
    Light,
    Water,
    Feature,
}

impl TemplateGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            TemplateGroup::Content => "content",
            TemplateGroup::Shape => "shape",
            TemplateGroup::Light => "light",
            TemplateGroup::Water => "water",
            TemplateGroup::Feature => "feature",
        }
    }
}

/// Uma tag do catálogo com um snippet mínimo válido (parsa e spawna tal qual).
pub struct NodeTemplate {
    pub group: TemplateGroup,
    pub tag: &'static str,
    pub label: &'static str,
    pub snippet: &'static str,
}

/// O catálogo: só tags cujo snippet passa pelo parser (garantido por teste). Tags conhecidas
/// fora do catálogo continuam a inserir-se com o snippet genérico `<Tag />`.
///
/// Duas exceções documentadas (testadas em `tests.rs`): `<Include>` é expandido
/// ANTES do parse (`xml::include::load_world`) — o parser cru recusa-o com erro
/// explícito; `<ResourceNode>` é tag de SUBÁRVORE (só vive dentro do template de
/// um spawner) — ao nível do mundo é o no-op reportado pelo `analyze`.
pub const TEMPLATES: &[NodeTemplate] = &[
    // ── content ──────────────────────────────────────────────────────────
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "entity",
        label: "Entity (contentor de props)",
        snippet: r#"<Entity name="prop" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "entity",
        label: "Volume físico (collider / trigger)",
        snippet: r#"<Entity name="volume" collider="shape: box; size: 2 2 2; trigger: false" rigidbody="type: fixed" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "cinematic",
        label: "Cinematic (câmara + legendas — viber cinematic)",
        snippet: r#"<Cinematic id="nova-cena"><Fade at="0" duration="1" from="1" to="0" /><Shot duration="4" anchor="player" from="4 3 6" look="0 1.5 0" /><Line at="0.5" duration="3" speaker="Narrador">…</Line></Cinematic>"#,
    },
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "group",
        label: "Group (agrupar nós)",
        snippet: r#"<Group name="group" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "gltfscene",
        label: "GLB (GltfScene)",
        snippet: r#"<GltfScene url="/assets/meshes/model.glb" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "composition",
        label: "Composition (cascas de caixas + colisor composto)",
        snippet: r#"<Composition name="shell" collider="auto"><Cuboid half-size="1 0.5 1" translation="0 0.5 0" /></Composition>"#,
    },
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "include",
        label: "Include (composição por ficheiro)",
        snippet: r#"<Include src="/world/bonus.xml" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "playergltf",
        label: "PlayerGLTF (herói GLB + movimento nativo)",
        snippet: r#"<PlayerGLTF name="player" pos="0 150 0" model-url="/assets/meshes/characters/hero_lod0.glb" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Content,
        tag: "particlesystem",
        label: "ParticleSystem (FX de partículas)",
        snippet: r#"<ParticleSystem preset="fire" translation="0 0.5 0" particle-emitter="preset: fire; emission-rate: 30; shape-radius: 0.2" />"#,
    },
    // ── shape ────────────────────────────────────────────────────────────
    NodeTemplate {
        group: TemplateGroup::Shape,
        tag: "cuboid",
        label: "Cuboid",
        snippet: r#"<Cuboid half-size="0.5 0.5 0.5" translation="0 0.5 0" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Shape,
        tag: "box",
        label: "Box (alias Cuboid)",
        snippet: r#"<Box half-size="0.5 0.5 0.5" translation="0 0.5 0" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Shape,
        tag: "sphere",
        label: "Sphere",
        snippet: r#"<Sphere radius="0.6" translation="0 0.6 0" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Shape,
        tag: "cylinder",
        label: "Cylinder",
        snippet: r#"<Cylinder radius="0.4" half-height="0.8" translation="0 0.8 0" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Shape,
        tag: "plane",
        label: "Plane",
        snippet: r#"<Plane translation="0 0.01 0" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Shape,
        tag: "capsule",
        label: "Capsule",
        snippet: r#"<Capsule translation="0 0.8 0" />"#,
    },
    // ── light ────────────────────────────────────────────────────────────
    NodeTemplate {
        group: TemplateGroup::Light,
        tag: "pointlight",
        label: "PointLight",
        snippet: r#"<PointLight translation="0 3 0" intensity="1500" shadows="true" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Light,
        tag: "directionallight",
        label: "DirectionalLight",
        snippet: r#"<DirectionalLight />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Light,
        tag: "ambientlight",
        label: "AmbientLight",
        snippet: r#"<AmbientLight color="0xc8d8e8" brightness="110" />"#,
    },
    // ── water ────────────────────────────────────────────────────────────
    NodeTemplate {
        group: TemplateGroup::Water,
        tag: "lake",
        label: "Lake",
        snippet: r#"<Lake at="0 0" radius="18" depth="2" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Water,
        tag: "river",
        label: "River",
        snippet: r#"<River path="0 0 30 4 60 2" width="12" depth="2.5" />"#,
    },
    // ── feature ──────────────────────────────────────────────────────────
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "terrain",
        label: "Terrain (heightfield procedural + layers)",
        snippet: r#"<Terrain name="world" world-size="512" max-height="60" chunk-size="64" layers="grass vale_grass dirt dirt_trail" seed="7" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "terrainpad",
        label: "TerrainPad (achatar terreno)",
        snippet: r#"<TerrainPad at="0 0" size="40 40" falloff="20" corner-radius="12" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "road",
        label: "Road",
        snippet: r#"<Road path="0 0 20 6 40 8" width="6" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "road",
        label: "RaceTrack (pista autorada: cota + larguras por nó, viaduto)",
        snippet: r#"<Road name="circuit" closed="1" width="18" y="14 14 20 21 14" widths="22 20 16 16 20" viaduct-clearance="6" texture-url="/assets/textures/dirt_road/albedo.ktx2" path="0 0 200 -20 400 60 300 300 0 260" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "roadnetwork",
        label: "RoadNetwork (rede de Ways + Segments)",
        snippet: r#"<RoadNetwork default-width="6"><Way id="plaza" xz="0 0" /><Way id="gate" xz="40 20" /><Segment a="plaza" b="gate" /></RoadNetwork>"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "grounddecal",
        label: "GroundDecal (mancha no terreno)",
        snippet: r#"<GroundDecal at="12 -8" radius="6" texture="/assets/textures/dirt_road/albedo.ktx2" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "vegetation",
        label: "Vegetation (flores/erva densa instanciada)",
        snippet: r#"<Vegetation meshes="/assets/meshes/vegetation/plant_flatShort.glb" density-per-km2="8000" seed="21" region-min="-60 0 -60" region-max="60 0 60" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "scatter",
        label: "Scatter (receita do gerador — saída do bake)",
        snippet: r#"<Scatter id="scatter.rocks" profile="rock" count="24" seed="31" region-min="-60 0 -60" region-max="60 0 60" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "dynamicspawner",
        label: "DynamicSpawner (criaturas/props com template)",
        snippet: r#"<DynamicSpawner profile="creature" count="3" seed="11" region-min="-40 0 -40" region-max="40 0 40"><GameObject><GLTFLoader url="/assets/meshes/characters/npc_scout_lod0.glb" /></GameObject></DynamicSpawner>"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "resourcenode",
        label: "ResourceNode (colheita — dentro do template de um spawner)",
        snippet: r#"<ResourceNode kind="wood" yield="3" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "spawnexclusion",
        label: "SpawnExclusion (círculo sem spawn)",
        snippet: r#"<SpawnExclusion at="0 0" radius="24" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "spawngate",
        label: "SpawnGate (âncora de spawn do player)",
        snippet: r#"<SpawnGate target-entity="player" y-fallback="150" />"#,
    },
    NodeTemplate {
        group: TemplateGroup::Feature,
        tag: "navmesh",
        label: "NavMesh (config da engine)",
        snippet: r#"<NavMesh />"#,
    },
];

/// O catálogo completo (dialog Add Node).
pub fn templates() -> &'static [NodeTemplate] {
    TEMPLATES
}

/// O template de uma tag (sem distinguir maiúsculas), se estiver no catálogo.
pub fn template_for(tag: &str) -> Option<&'static NodeTemplate> {
    TEMPLATES
        .iter()
        .find(|template| template.tag.eq_ignore_ascii_case(tag))
}

/// O snippet a inserir para uma tag: o do catálogo, ou `<Tag />` genérico (tags desconhecidas
/// do parser degradam para no-op com aviso no `analyze` — nunca rebentam o mundo).
pub fn snippet_for(tag: &str) -> String {
    template_for(tag)
        .map(|template| template.snippet.to_string())
        .unwrap_or_else(|| format!("<{tag} />"))
}

/// A op do diálogo Add Node: insere o snippet da tag em `parent`.
pub fn add_node(parent: &str, tag: &str, index: Option<usize>) -> EditOp {
    EditOp::Insert {
        parent: parent.to_string(),
        index,
        xml: snippet_for(tag),
    }
}

/// Escapa um valor para ir dentro de `attr="…"`.
pub fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Nome único no documento. Dois carimbos do mesmo GLB não podem partilhar `name=`
/// (o seletor deixa de ser unívoco — o mesmo contrato do `viber scene`).
pub fn unique_entity_name(doc: &super::doc::SceneDoc, stem: &str) -> String {
    let stem: String = stem.chars().filter(|ch| !ch.is_control()).collect();
    let stem = if stem.is_empty() {
        "dropped".to_owned()
    } else {
        stem
    };
    if doc.find_by_attr("name", &stem).is_empty() {
        return stem;
    }
    for index in 2..10_000 {
        let candidate = format!("{stem}-{index}");
        if doc.find_by_attr("name", &candidate).is_empty() {
            return candidate;
        }
    }
    format!("{stem}-x")
}

/// A op de largar um `.glb` no mundo: `Entity` com o nome do ficheiro e o `GltfScene` filho,
/// posta em `world_pos` (coordenadas MUNDO — o raycast do terreno, ou o que um agente pedir)
/// convertidas para o referencial de `parent` ([`pose::world_frame`]). Pose canónica.
pub fn glb_drop(
    doc: &super::doc::SceneDoc,
    parent: &str,
    glb_path: &str,
    world_pos: [f32; 3],
) -> anyhow::Result<EditOp> {
    glb_drop_pose(doc, parent, glb_path, world_pos, 0.0, 1.0)
}

/// Como [`glb_drop`], com rotação Y (graus) e escala uniforme — o stamp tool.
pub fn glb_drop_pose(
    doc: &super::doc::SceneDoc,
    parent: &str,
    glb_path: &str,
    world_pos: [f32; 3],
    yaw_deg: f32,
    scale: f32,
) -> anyhow::Result<EditOp> {
    let parent_ref = doc.resolve(parent)?;
    let stem = Path::new(glb_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("dropped");
    let name = unique_entity_name(doc, stem);
    let url = suggest_url(glb_path);
    let name_xml = escape_attr(&name);
    let url_xml = escape_attr(&url);
    let local = pose::world_to_local(
        doc,
        &parent_ref,
        pose::Pose {
            translation: bevy::math::Vec3::from(world_pos),
            rotation: bevy::math::Quat::from_rotation_y(yaw_deg.to_radians()),
            scale: bevy::math::Vec3::splat(scale.max(0.01)),
        },
    );
    let translation = pose::format_vec3(local.translation);
    let euler = pose::format_euler(local.rotation);
    let scale_text = pose::format_scale(local.scale);
    Ok(EditOp::Insert {
        parent: parent.to_string(),
        index: None,
        xml: format!(
            r#"<Entity name="{name_xml}" translation="{translation}" euler="{euler}" scale="{scale_text}"><GltfScene url="{url_xml}" /></Entity>"#
        ),
    })
}

/// Instancia uma cena (`.xml` da pasta do mundo — ex. um ramo extraído com `extract`):
/// `<Include src="/cena.xml">` debaixo de `parent`, na posição MUNDO `at` convertida para o
/// referencial do pai (omissa: na origem do pai). O insert carrega o ficheiro no documento.
pub fn scene_instance(
    doc: &super::doc::SceneDoc,
    parent: &str,
    scene: &str,
    world_pos: Option<[f32; 3]>,
) -> anyhow::Result<EditOp> {
    let parent_ref = doc.resolve(parent)?;
    let rel = scene
        .trim()
        .trim_start_matches("./")
        .trim_start_matches('/');
    if !rel.ends_with(".xml") {
        anyhow::bail!("instance: `{scene}` não é um .xml");
    }
    // `..`/raízes escapavam da pasta do mundo (o `SceneDoc` carrega o
    // ficheiro directamente; só o `load_world` recusava no arranque). O
    // `<Include src="/…">` gerado tem de ficar dentro do projecto.
    let rel_path = std::path::Path::new(rel);
    let inside = rel_path.components().all(|component| {
        matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if rel.is_empty() || !inside {
        anyhow::bail!("instance: `{scene}` tem de ser relativo à pasta do mundo (sem `..`)");
    }
    if doc.file_by_path(rel).is_none() && !doc.root_dir.join(rel).is_file() {
        anyhow::bail!("instance: `{rel}` não existe na pasta do mundo");
    }
    let mut xml = format!(r#"<Include src="/{rel}""#);
    if let Some(world_pos) = world_pos {
        let local = pose::world_to_local(
            doc,
            &parent_ref,
            pose::Pose {
                translation: bevy::math::Vec3::from(world_pos),
                ..pose::Pose::default()
            },
        );
        xml.push_str(&format!(
            r#" translation="{}""#,
            pose::format_vec3(local.translation)
        ));
    }
    xml.push_str(" />");
    Ok(EditOp::Insert {
        parent: parent.to_string(),
        index: None,
        xml,
    })
}

/// O catálogo em JSON (o "menu" Add Node de um agente).
pub fn to_json() -> serde_json::Value {
    serde_json::Value::Array(
        TEMPLATES
            .iter()
            .map(|t| serde_json::json!({ "group": t.group.as_str(), "tag": t.tag, "label": t.label, "snippet": t.snippet }))
            .collect(),
    )
}

/// URL a escrever no XML para um caminho arrastado do sistema: se vive sob uma pasta
/// `public/` (o pool partilhado), vira o caminho `/assets/…` habitual dos mundos; senão
/// usa-se o caminho tal como arrived (absoluto ou relativo ao mundo).
pub fn suggest_url(glb_path: &str) -> String {
    let path = glb_path.replace('\\', "/");
    match path.rfind("/public/") {
        Some(index) => path[index + "/public".len()..].to_string(),
        None => path,
    }
}

/// "Mover para nova cena" (projetos com cenas, [`crate::project`]): os nós passam para uma
/// cena NOVA `scenes/<id>.xml` — um universo próprio, não um `<Include>` — e saem do mundo.
/// `recenter` é subtraído à translação de cada nó (a sala passa a viver na origem da cena);
/// `prepend` é XML acrescentado antes dos nós (a `<SceneEntry>`, o `<InteriorScene>`…).
/// Um só lote: `create_file` + `remove`s — o `inverse` desfaz tudo (o ficheiro e os nós).
pub fn extract_scene_ops(
    doc: &super::doc::SceneDoc,
    selectors: &[String],
    id: &str,
    recenter: [f32; 3],
    prepend: Option<&str>,
    title: Option<&str>,
) -> anyhow::Result<Vec<EditOp>> {
    let id = id.trim().trim_matches('/');
    if id.is_empty() || id == crate::project::MAIN_SCENE || id.contains("..") {
        anyhow::bail!("extract-scene: id de cena inválido `{id}`");
    }
    if selectors.is_empty() {
        anyhow::bail!("extract-scene: nenhum nó selecionado");
    }
    let project = crate::project::Project::at(&doc.root_dir);
    let scenes_rel = project.config.scenes_dir();
    let rel = format!(
        "{}/{id}.xml",
        scenes_rel.to_string_lossy().trim_end_matches('/')
    );
    let mut nodes = Vec::new();
    for selector in selectors {
        let node = doc.resolve(selector)?;
        if node.is_root() {
            anyhow::bail!("extract-scene: `{selector}` é a raiz de um ficheiro");
        }
        if !nodes.contains(&node) {
            nodes.push(node);
        }
    }
    // Pela ordem do documento (a ordem dos nós na cena nova).
    nodes.sort_by(|a, b| (a.file, &a.path).cmp(&(b.file, &b.path)));
    // Um nó DENTRO de outro selecionado viaja com ele (mesmo ficheiro, ou
    // ficheiro carregado por um `<Include>` da subárvore). Copiá-lo também à
    // parte duplicava o conteúdo na cena nova — o pai já o leva.
    let mut redundant = vec![false; nodes.len()];
    for (index, ancestor) in nodes.iter().enumerate() {
        let files = doc.subtree_files(ancestor);
        for (other, node) in nodes.iter().enumerate() {
            if index == other {
                continue;
            }
            let inside = if ancestor.file == node.file {
                node.path.len() > ancestor.path.len() && node.path.starts_with(&ancestor.path)
            } else {
                files.contains(&node.file)
            };
            if inside {
                redundant[other] = true;
            }
        }
    }
    let nodes: Vec<super::doc::NodeRef> = nodes
        .into_iter()
        .zip(redundant)
        .filter_map(|(node, skip)| (!skip).then_some(node))
        .collect();
    let mut moved = Vec::new();
    for node in &nodes {
        let mut xml = doc.node(node).expect("resolvido").clone();
        if recenter != [0.0; 3] {
            let recentered = |value: &str, ctx: &str| -> anyhow::Result<String> {
                // Em f64: `2603.4 - 2600` em f32 dava `3.3999`.
                // `translation="100"` é broadcast para os três eixos.
                // Subtrair só os tokens escritos deixava um número só,
                // que voltava a fazer broadcast no reload.
                let parsed = crate::xml::values::parse_vec3(value, ctx)?;
                Ok(parsed
                    .iter()
                    .zip(recenter)
                    .map(|(v, r)| {
                        let x = ((f64::from(*v) - f64::from(r)) * 1000.0).round() / 1000.0;
                        let x = if x == 0.0 { 0.0 } else { x };
                        format!("{x}")
                    })
                    .collect::<Vec<_>>()
                    .join(" "))
            };
            for key in ["translation", "pos"] {
                if let Some((_, value)) = xml.attrs.iter_mut().find(|(k, _)| k == key) {
                    *value = recentered(value, key)?;
                }
            }
            // `transform="pos: …; rotation: …"` guarda a translação noutro
            // sítio: sem este braço o nó extraído ficava com as coordenadas do
            // mundo antigo (a cena nova nasce na origem).
            if let Some((_, value)) = xml.attrs.iter_mut().find(|(k, _)| k == "transform") {
                let parts = crate::recipes::parse_component_string(value);
                if parts.iter().any(|(k, _)| k == "pos" || k == "position") {
                    let mut rebuilt = Vec::with_capacity(parts.len());
                    for (key, part) in &parts {
                        let part = if key == "pos" || key == "position" {
                            recentered(part, "transform pos")?
                        } else {
                            part.clone()
                        };
                        rebuilt.push(format!("{key}: {part}"));
                    }
                    *value = rebuilt.join("; ");
                }
            }
        }
        moved.push(xml);
    }
    let mut xml = String::new();
    if let Some(prepend) = prepend.map(str::trim).filter(|p| !p.is_empty()) {
        xml.push_str(prepend);
        xml.push('\n');
    }
    xml.push_str(&super::ops::fragment_xml(&moved));
    let mut ops = vec![EditOp::CreateFile {
        path: rel,
        xml,
        title: title.map(str::to_owned),
    }];
    // Remoções do fim para o início: os índices dos irmãos anteriores não mudam.
    for node in nodes.iter().rev() {
        ops.push(EditOp::Remove {
            node: doc.selector(node),
        });
    }
    Ok(ops)
}

/// Os nós de topo de `file` cuja translação XZ cai no retângulo `[x0, z0, x1, z1]` — a
/// seleção por região do `extract-scene --within`.
pub fn nodes_within(
    doc: &super::doc::SceneDoc,
    file: &str,
    rect: [f32; 4],
) -> anyhow::Result<Vec<String>> {
    let file_id = doc
        .file_by_path(file)
        .ok_or_else(|| anyhow::anyhow!("--within: `{file}` não faz parte do mundo"))?;
    let (x0, x1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
    let (z0, z1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
    let mut out = Vec::new();
    for (index, node) in doc.files[file_id].doc.children.iter().enumerate() {
        let Ok(pose) = super::pose::read_pose(node) else {
            continue;
        };
        let t = pose.translation;
        if t.x >= x0 && t.x <= x1 && t.z >= z0 && t.z <= z1 {
            out.push(doc.selector(&super::doc::NodeRef::new(file_id, vec![index])));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_attr_keeps_quotes_inside_the_attribute() {
        assert_eq!(escape_attr(r#"a"b&c<d"#), "a&quot;b&amp;c&lt;d");
        assert_eq!(escape_attr("barrel"), "barrel");
    }
}
