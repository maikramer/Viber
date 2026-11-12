//! Leitura de `.bsn` para o DSL de mundo do Viber.
//!
//! O documento não é aplicado como ECS do jogo: cada entidade traz um
//! [`WorldNode`] com a tag/attrs/texto originais. O loader reconstrói os
//! [`XmlNode`] e o `parse_world`/spawner existentes fazem o resto. As
//! componentes nativas do Jackdaw (`Transform`, `Name`, `GltfSource`, luzes)
//! são traduzidas de volta para atributos XML — é isso que faz o editor servir
//! para mover props e ajustar luzes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use bevy::math::{Quat, Vec3};
use jackdaw_bsn::{BsnPatch, BsnStructData, BsnValue, SceneBsnAst, parse_bsn_text};

use crate::bsn::components::{WorldNode, WorldProp};
use crate::bsn::pose::{Pose, format_f32, format_vec3, rotation_to_direction, write_pose_attrs};
use crate::xml::include::{
    INCLUDE_POSE_KEYS, LoadedWorld, MAX_INCLUDE_DEPTH, resolve_include_path,
};
use crate::xml::{NodeOrigin, XmlNode};

const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const GLTF_SOURCE: &str = "jackdaw_scene_types::types::GltfSource";
const POINT_LIGHT: &str = "bevy_light::point_light::PointLight";
const RECT_LIGHT: &str = "bevy_light::rect_light::RectLight";
const DIRECTIONAL_LIGHT: &str = "bevy_light::directional_light::DirectionalLight";

/// Lê um mundo `.bsn` a partir da raiz do projeto.
pub fn load_bsn_world(path: &Path) -> Result<LoadedWorld> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("world file not found: {}", path.display()))?;
    let root_dir = crate::project::root_of(&canonical);
    load_bsn_world_in(&canonical, &root_dir)
}

/// [`load_bsn_world`] com uma raiz explícita para os `<Include src="/…">`.
pub fn load_bsn_world_in(path: &Path, root_dir: &Path) -> Result<LoadedWorld> {
    let path = path
        .canonicalize()
        .with_context(|| format!("world file not found: {}", path.display()))?;
    let root_dir = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf());
    let mut loader = Loader {
        root_dir,
        stack: vec![path.clone()],
    };
    let (root_tag, root_attrs, nodes) = loader.load_document(&path)?;
    let _ = root_tag;
    Ok(LoadedWorld { root_attrs, nodes })
}

struct Loader {
    root_dir: PathBuf,
    stack: Vec<PathBuf>,
}

/// Root tag/attrs of a `.bsn` document plus its converted children.
type DocumentParts = (String, Vec<(String, String)>, Vec<XmlNode>);

impl Loader {
    fn load_document(&mut self, path: &Path) -> Result<DocumentParts> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let ast = parse_bsn_text(&text).map_err(|err| anyhow!("{}: {err}", path.display()))?;
        if ast.roots.is_empty() {
            bail!("{}: documento .bsn sem entidades", path.display());
        }

        let mut root_tag = "world".to_string();
        let mut root_attrs = Vec::new();
        let mut has_world_root = false;
        let mut nodes = Vec::new();
        for (index, &root) in ast.roots.iter().enumerate() {
            let mut prefix = vec![index as u32];
            if let Some(world) = world_node(&ast, root)
                && (world.tag.eq_ignore_ascii_case("world")
                    || world.tag.eq_ignore_ascii_case("scene"))
            {
                root_tag = world.tag.clone();
                root_attrs = world
                    .attrs
                    .iter()
                    .map(|prop| (prop.key.clone(), prop.value.clone()))
                    .collect();
                has_world_root = true;
                nodes.extend(self.convert_children(&ast, root, path, &mut prefix)?);
                continue;
            }
            nodes.extend(self.convert_node(&ast, root, path, &mut prefix)?);
        }
        if !has_world_root && root_attrs.is_empty() {
            // Sem nó `<world>`: todos os roots são conteúdo do mundo.
            root_tag = "world".to_string();
        }
        Ok((root_tag, root_attrs, nodes))
    }

    fn convert_children(
        &mut self,
        ast: &SceneBsnAst,
        patches_entity: bevy::ecs::entity::Entity,
        current_path: &Path,
        prefix: &mut Vec<u32>,
    ) -> Result<Vec<XmlNode>> {
        let children = ast.get_children_ast(patches_entity);
        let mut out = Vec::with_capacity(children.len());
        for (index, child) in children.into_iter().enumerate() {
            prefix.push(index as u32);
            out.extend(self.convert_node(ast, child, current_path, prefix)?);
            prefix.pop();
        }
        Ok(out)
    }

    fn convert_node(
        &mut self,
        ast: &SceneBsnAst,
        patches_entity: bevy::ecs::entity::Entity,
        current_path: &Path,
        prefix: &mut Vec<u32>,
    ) -> Result<Vec<XmlNode>> {
        let Some(world) = world_node(ast, patches_entity) else {
            // Entidades criadas no editor sem `WorldNode` não fazem parte do
            // DSL; ignorar é melhor do que inventar uma tag.
            return Ok(Vec::new());
        };
        let tag = world.tag.clone();
        let mut attrs: Vec<(String, String)> = world
            .attrs
            .iter()
            .map(|prop| (prop.key.clone(), prop.value.clone()))
            .collect();
        let origin = self.origin_for(&world, current_path, prefix);

        if tag.eq_ignore_ascii_case("include") {
            return self.expand_include(ast, &attrs, origin, current_path);
        }

        if let Some(name) = ast.get_name(patches_entity) {
            set_attr(&mut attrs, "name", name.to_string());
        }
        apply_transform_and_previews(ast, patches_entity, &tag, world.pose, &mut attrs);

        let children = self.convert_children(ast, patches_entity, current_path, prefix)?;
        Ok(vec![XmlNode {
            tag,
            attrs,
            text: world.text.clone(),
            children,
            origin: Some(Arc::new(origin)),
        }])
    }

    fn expand_include(
        &mut self,
        _ast: &SceneBsnAst,
        attrs: &[(String, String)],
        origin: NodeOrigin,
        current_path: &Path,
    ) -> Result<Vec<XmlNode>> {
        let src = attr(attrs, "src")
            .map(str::trim)
            .filter(|src| !src.is_empty())
            .ok_or_else(|| anyhow!("<Include> sem src em {}", origin.file.display()))?;
        let dir = current_path.parent().unwrap_or(&self.root_dir);
        let mut resolved = resolve_include_path(src, dir, &self.root_dir);
        resolved = prefer_bsn(resolved);

        let canonical = resolved
            .canonicalize()
            .with_context(|| format!("include inexistente: {}", resolved.display()))?;
        if self.stack.contains(&canonical) {
            bail!(
                "ciclo de includes: {}",
                self.stack
                    .iter()
                    .map(|path| path.display().to_string())
                    .chain(std::iter::once(canonical.display().to_string()))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            );
        }
        if self.stack.len() > MAX_INCLUDE_DEPTH {
            bail!(
                "include depth exceeds {MAX_INCLUDE_DEPTH}: {}",
                canonical.display()
            );
        }

        self.stack.push(canonical.clone());
        let included = if is_bsn(&canonical) {
            self.load_document(&canonical)?.2
        } else {
            crate::xml::include::load_world_in(&canonical, &self.root_dir)?.nodes
        };
        self.stack.pop();

        // Mesma regra do XML: só há wrapper quando o include tem pose; o
        // `name` acompanha o wrapper.
        let mut wrapper_attrs: Vec<(String, String)> = attrs
            .iter()
            .filter(|(key, _)| INCLUDE_POSE_KEYS.contains(&key.as_str()))
            .cloned()
            .collect();
        if wrapper_attrs.is_empty() {
            return Ok(included);
        }
        if !wrapper_attrs.iter().any(|(key, _)| key == "name")
            && let Some(name) = attr(attrs, "name")
        {
            wrapper_attrs.push(("name".into(), name.to_owned()));
        }
        let wrapper = XmlNode {
            tag: "Group".to_string(),
            attrs: wrapper_attrs,
            text: String::new(),
            children: included,
            origin: Some(Arc::new(origin)),
        };
        Ok(vec![wrapper])
    }

    fn origin_for(&self, world: &WorldNode, current_path: &Path, prefix: &[u32]) -> NodeOrigin {
        if !world.origin_file.is_empty() {
            let path = self.root_dir.join(&world.origin_file);
            let path = path.canonicalize().unwrap_or(path);
            return NodeOrigin {
                file: Arc::from(path.as_path()),
                path: world.origin_path.clone(),
            };
        }
        NodeOrigin {
            file: Arc::from(current_path),
            path: prefix.to_vec(),
        }
    }
}

fn prefer_bsn(path: PathBuf) -> PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("bsn") => path,
        Some("xml") => {
            let bsn = path.with_extension("bsn");
            if bsn.is_file() { bsn } else { path }
        }
        _ => {
            let bsn = path.with_extension("bsn");
            if bsn.is_file() {
                bsn
            } else {
                let xml = path.with_extension("xml");
                if xml.is_file() { xml } else { path }
            }
        }
    }
}

fn is_bsn(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bsn"))
}

fn world_node(ast: &SceneBsnAst, patches_entity: bevy::ecs::entity::Entity) -> Option<WorldNode> {
    let data = struct_patch(ast, patches_entity, WORLD_NODE)?;
    let tag = field_str(data, "tag")?.to_string();
    let attrs = match field(data, "attrs") {
        Some(BsnValue::List(values)) => values
            .iter()
            .filter_map(|value| {
                let prop = as_struct(value)?;
                Some(WorldProp {
                    key: field_str(prop, "key")?.to_string(),
                    value: field_str(prop, "value").unwrap_or_default().to_string(),
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(WorldNode {
        tag,
        attrs,
        text: field_str(data, "text").unwrap_or_default().to_string(),
        origin_file: field_str(data, "origin_file")
            .unwrap_or_default()
            .to_string(),
        origin_path: field(data, "origin_path")
            .and_then(as_list)
            .map(|values| values.iter().filter_map(as_u32).collect())
            .unwrap_or_default(),
        pose: field(data, "pose").and_then(as_bool).unwrap_or(false),
    })
}

fn apply_transform_and_previews(
    ast: &SceneBsnAst,
    patches_entity: bevy::ecs::entity::Entity,
    tag: &str,
    pose_authoritative: bool,
    attrs: &mut Vec<(String, String)>,
) {
    let transform = transform_patch(ast, patches_entity);
    if tag.eq_ignore_ascii_case("directionallight") {
        if let Some((_, rotation, _)) = transform {
            let direction = rotation_to_direction(rotation);
            set_attr(attrs, "direction", format_vec3(direction));
        }
    } else if pose_authoritative && let Some((translation, rotation, scale)) = transform {
        write_pose_attrs(
            attrs,
            Pose {
                translation,
                rotation,
                scale,
            },
        );
    }

    if let Some(data) = struct_patch(ast, patches_entity, GLTF_SOURCE)
        && let Some(path) = field_str(data, "path")
    {
        let normalized = path.trim_start_matches('/');
        let current = attr(attrs, "url").map(str::to_string);
        let same = current
            .as_deref()
            .map(|url| url.trim_start_matches('/') == normalized)
            .unwrap_or(false);
        if !same {
            let url = if current.as_deref().is_some_and(|url| url.starts_with('/')) {
                format!("/{normalized}")
            } else {
                normalized.to_string()
            };
            set_attr(attrs, "url", url);
        }
    }

    if let Some(data) = struct_patch(ast, patches_entity, POINT_LIGHT) {
        if let Some(color) = field(data, "color").and_then(color_rgb) {
            set_attr(attrs, "color", hex_color(color));
        }
        if let Some(value) = field_f64(data, "intensity") {
            set_attr(attrs, "intensity", format_f32(value as f32));
        }
        if let Some(value) = field_f64(data, "radius") {
            set_attr(attrs, "radius", format_f32(value as f32));
        }
        if let Some(value) = field_bool(data, "shadow_maps_enabled") {
            set_attr(attrs, "shadows", value.to_string());
        }
    }

    if let Some(data) = struct_patch(ast, patches_entity, RECT_LIGHT) {
        if let Some(color) = field(data, "color").and_then(color_rgb) {
            set_attr(attrs, "color", hex_color(color));
        }
        for (field_name, attr_name) in [
            ("intensity", "intensity"),
            ("range", "range"),
            ("width", "width"),
            ("height", "height"),
        ] {
            if let Some(value) = field_f64(data, field_name) {
                set_attr(attrs, attr_name, format_f32(value as f32));
            }
        }
    }

    if let Some(data) = struct_patch(ast, patches_entity, DIRECTIONAL_LIGHT) {
        if let Some(color) = field(data, "color").and_then(color_rgb) {
            set_attr(attrs, "color", hex_color(color));
        }
        if let Some(value) = field_f64(data, "illuminance") {
            set_attr(attrs, "illuminance", format_f32(value as f32));
        }
        if let Some(value) = field_bool(data, "shadow_maps_enabled") {
            set_attr(attrs, "shadows", value.to_string());
        }
    }
}

fn transform_patch(
    ast: &SceneBsnAst,
    patches_entity: bevy::ecs::entity::Entity,
) -> Option<(Vec3, Quat, Vec3)> {
    let data = struct_patch(ast, patches_entity, TRANSFORM)?;
    let translation = field(data, "translation")
        .and_then(as_vec3)
        .unwrap_or(Vec3::ZERO);
    let rotation = field(data, "rotation")
        .and_then(as_quat)
        .unwrap_or(Quat::IDENTITY);
    let scale = field(data, "scale").and_then(as_vec3).unwrap_or(Vec3::ONE);
    Some((translation, rotation, scale))
}

fn struct_patch<'a>(
    ast: &'a SceneBsnAst,
    patches_entity: bevy::ecs::entity::Entity,
    type_path: &str,
) -> Option<&'a BsnStructData> {
    let patch = ast.find_patch_by_type_path(patches_entity, type_path)?;
    match ast.get_patch(patch)? {
        BsnPatch::Struct(data) => Some(data),
        _ => None,
    }
}

fn field<'a>(data: &'a BsnStructData, name: &str) -> Option<&'a BsnValue> {
    data.fields
        .0
        .iter()
        .find(|field| field.name == name)
        .map(|field| &field.value)
}

fn field_str<'a>(data: &'a BsnStructData, name: &str) -> Option<&'a str> {
    field(data, name).and_then(as_str)
}

fn field_f64(data: &BsnStructData, name: &str) -> Option<f64> {
    field(data, name).and_then(as_f64)
}

fn field_bool(data: &BsnStructData, name: &str) -> Option<bool> {
    field(data, name).and_then(as_bool)
}

fn as_struct(value: &BsnValue) -> Option<&BsnStructData> {
    match value {
        BsnValue::Struct(data) => Some(data),
        _ => None,
    }
}

fn as_str(value: &BsnValue) -> Option<&str> {
    match value {
        BsnValue::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn as_f64(value: &BsnValue) -> Option<f64> {
    match value {
        BsnValue::Float(value) => Some(*value),
        BsnValue::Int(value) => Some(*value as f64),
        _ => None,
    }
}

fn as_bool(value: &BsnValue) -> Option<bool> {
    match value {
        BsnValue::Bool(value) => Some(*value),
        _ => None,
    }
}

fn as_u32(value: &BsnValue) -> Option<u32> {
    match value {
        BsnValue::Int(value) => u32::try_from(*value).ok(),
        _ => None,
    }
}

fn as_list(value: &BsnValue) -> Option<&Vec<BsnValue>> {
    match value {
        BsnValue::List(values) => Some(values),
        _ => None,
    }
}

fn as_vec3(value: &BsnValue) -> Option<Vec3> {
    let data = as_struct(value)?;
    Some(Vec3::new(
        field_f64(data, "x")? as f32,
        field_f64(data, "y")? as f32,
        field_f64(data, "z")? as f32,
    ))
}

fn as_quat(value: &BsnValue) -> Option<Quat> {
    let data = as_struct(value)?;
    Some(Quat::from_xyzw(
        field_f64(data, "x")? as f32,
        field_f64(data, "y")? as f32,
        field_f64(data, "z")? as f32,
        field_f64(data, "w")? as f32,
    ))
}

fn color_rgb(value: &BsnValue) -> Option<[f32; 3]> {
    match value {
        BsnValue::Struct(data)
            if data.type_path.ends_with("Srgba") || data.type_path.ends_with("Srgb") =>
        {
            Some([
                field_f64(data, "red")? as f32,
                field_f64(data, "green")? as f32,
                field_f64(data, "blue")? as f32,
            ])
        }
        BsnValue::Type(path) if path.ends_with("WHITE") => Some([1.0, 1.0, 1.0]),
        BsnValue::Type(path) if path.ends_with("BLACK") => Some([0.0, 0.0, 0.0]),
        _ => None,
    }
}

fn hex_color([red, green, blue]: [f32; 3]) -> String {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(red),
        channel(green),
        channel(blue)
    )
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn set_attr(attrs: &mut Vec<(String, String)>, key: &str, value: String) {
    if let Some((_, current)) = attrs.iter_mut().find(|(name, _)| name == key) {
        *current = value;
    } else {
        attrs.push((key.to_string(), value));
    }
}
