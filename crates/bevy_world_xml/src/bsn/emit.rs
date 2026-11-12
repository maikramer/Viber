//! Emissor `.bsn` do DSL de mundo.
//!
//! O conversor lê o XML cru (`parse_file`) e escreve, por cada nó, uma
//! entidade com:
//!
//! - `#Name` quando há `name`;
//! - `Transform` (pose canónica) quando a tag usa pose;
//! - previews nativas para o viewport do Jackdaw: `GltfSource` e luzes;
//! - `WorldNode { tag, attrs, text, origin, pose }`.
//!
//! O jogo não usa as previews: o loader reconstrói `XmlNode` a partir do
//! `WorldNode` e usa o parser/spawner existentes.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use bevy::math::Vec3;

use crate::bsn::components::tag_uses_transform;
use crate::bsn::pose::{Pose, direction_to_rotation, format_f32, read_pose};
use crate::xml::values::{parse_bool, parse_color, parse_f32, parse_vec3};
use crate::xml::{NodeOrigin, XmlDocument, XmlNode};

const WORLD_NODE: &str = "bevy_world_xml::bsn::WorldNode";
const WORLD_PROP: &str = "bevy_world_xml::bsn::WorldProp";
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const GLTF_SOURCE: &str = "jackdaw_scene_types::types::GltfSource";
const POINT_LIGHT: &str = "bevy_light::point_light::PointLight";
const RECT_LIGHT: &str = "bevy_light::rect_light::RectLight";
const DIRECTIONAL_LIGHT: &str = "bevy_light::directional_light::DirectionalLight";

#[derive(Debug, Clone, Copy)]
pub struct EmitOptions {
    /// Emitir `GltfSource`/luzes nativas para o viewport do Jackdaw.
    pub preview: bool,
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self { preview: true }
    }
}

/// Emite um ficheiro `.bsn` a partir do XML cru (sem expandir includes).
pub fn emit_file(xml_path: &Path, root_dir: &Path, options: EmitOptions) -> Result<String> {
    let mut doc = crate::xml::parse_file(xml_path)?;
    crate::xml::stamp_origins(&mut doc.children, &std::sync::Arc::from(xml_path));
    let root_dir = root_dir
        .canonicalize()
        .unwrap_or_else(|_| root_dir.to_path_buf());
    emit_document(&doc, xml_path, &root_dir, options)
}

/// Emite um documento já parseado. `current_file` é o ficheiro que está a ser
/// escrito (origem dos nós sem `origin`).
pub fn emit_document(
    doc: &XmlDocument,
    current_file: &Path,
    root_dir: &Path,
    options: EmitOptions,
) -> Result<String> {
    let origin = OriginContext {
        current_file: current_file.to_path_buf(),
        root_dir: root_dir.to_path_buf(),
    };
    let mut out = emit_entity(
        &doc.root_tag,
        &doc.root_attrs,
        &doc.children,
        &doc.root_text,
        None,
        &origin,
        options,
        0,
    )?;
    out.push('\n');
    Ok(out)
}

struct OriginContext {
    current_file: PathBuf,
    root_dir: PathBuf,
}

#[allow(
    clippy::too_many_arguments,
    reason = "emissor de um nó: tag, attrs, filhos, texto, origem, opções e índice"
)]
fn emit_entity(
    tag: &str,
    attrs: &[(String, String)],
    children: &[XmlNode],
    text: &str,
    node_origin: Option<&NodeOrigin>,
    origin: &OriginContext,
    options: EmitOptions,
    index: u32,
) -> Result<String> {
    let mut out = String::new();
    if let Some(name) = attr(attrs, "name") {
        out.push_str(&format!("#\"{}\"\n", escape_string(name)));
    }

    let pose = if tag_uses_transform(tag) {
        pose_for_tag(tag, attrs)?
    } else {
        Pose::default()
    };
    out.push_str(&emit_transform(pose));

    if options.preview {
        if let Some(gltf) = emit_gltf_preview(tag, attrs) {
            out.push_str(&gltf);
        }
        if let Some(light) = emit_light_preview(tag, attrs)? {
            out.push_str(&light);
        }
    }

    let origin_file = node_origin
        .map(|origin_node| relative_path(origin_node.file.as_ref(), &origin.root_dir))
        .unwrap_or_else(|| {
            relative_path(
                origin
                    .current_file
                    .canonicalize()
                    .unwrap_or_else(|_| origin.current_file.clone())
                    .as_path(),
                &origin.root_dir,
            )
        });
    let origin_path = node_origin
        .map(|origin_node| origin_node.path.clone())
        .unwrap_or_else(|| vec![index]);
    out.push_str(&emit_world_node(
        tag,
        attrs,
        text,
        &origin_file,
        &origin_path,
        tag_uses_transform(tag),
    ));

    if !children.is_empty() {
        out.push_str("bevy_ecs::hierarchy::Children [\n");
        for (child_index, child) in children.iter().enumerate() {
            if child_index > 0 {
                out.push_str("    --\n");
            }
            let child_origin = child.origin.as_ref().map(|origin| origin.as_ref());
            out.push_str(&indent(
                &emit_entity(
                    &child.tag,
                    &child.attrs,
                    &child.children,
                    &child.text,
                    child_origin,
                    origin,
                    options,
                    child_index as u32,
                )?,
                1,
            ));
        }
        out.push_str("]\n");
    }
    Ok(out)
}

fn pose_for_tag(tag: &str, attrs: &[(String, String)]) -> Result<Pose> {
    if tag.eq_ignore_ascii_case("directionallight") {
        let direction = attr(attrs, "direction")
            .map(|value| parse_vec3(value, "<DirectionalLight direction>"))
            .transpose()?
            .map(Vec3::from)
            .unwrap_or(Vec3::new(-1.0, -1.0, -1.0));
        return Ok(Pose {
            rotation: direction_to_rotation(direction),
            ..Default::default()
        });
    }
    read_pose(attrs)
}

fn emit_transform(pose: Pose) -> String {
    format!(
        "{TRANSFORM} {{\n\
         \x20   translation: glam::Vec3 {{ x: {tx}, y: {ty}, z: {tz} }},\n\
         \x20   rotation: glam::Quat {{ x: {qx}, y: {qy}, z: {qz}, w: {qw} }},\n\
         \x20   scale: glam::Vec3 {{ x: {sx}, y: {sy}, z: {sz} }},\n\
         }}\n",
        tx = bsn_float(pose.translation.x),
        ty = bsn_float(pose.translation.y),
        tz = bsn_float(pose.translation.z),
        qx = bsn_float(pose.rotation.x),
        qy = bsn_float(pose.rotation.y),
        qz = bsn_float(pose.rotation.z),
        qw = bsn_float(pose.rotation.w),
        sx = bsn_float(pose.scale.x),
        sy = bsn_float(pose.scale.y),
        sz = bsn_float(pose.scale.z),
    )
}

fn emit_gltf_preview(tag: &str, attrs: &[(String, String)]) -> Option<String> {
    if !matches!(
        tag.to_ascii_lowercase().as_str(),
        "gltfscene" | "playergltf" | "gltfloader"
    ) {
        return None;
    }
    let url = attr(attrs, "url")?;
    let path = url.trim_start_matches('/');
    let path = path.to_string();
    Some(format!(
        "{GLTF_SOURCE} {{ path: \"{}\", scene_index: 0 }}\n",
        escape_string(&path)
    ))
}

fn emit_light_preview(tag: &str, attrs: &[(String, String)]) -> Result<Option<String>> {
    let color = light_color(attrs)?;
    let color_text = format!(
        "bevy_color::color::Color::Srgba {{ red: {}, green: {}, blue: {}, alpha: 1.0 }}",
        bsn_float(color[0]),
        bsn_float(color[1]),
        bsn_float(color[2]),
    );
    match tag.to_ascii_lowercase().as_str() {
        "pointlight" => {
            let intensity = float_attr(attrs, "intensity", 1200.0)?;
            let radius = float_attr(attrs, "radius", 0.0)?;
            let shadows = bool_attr(attrs, "shadows", false)?;
            Ok(Some(format!(
                "{POINT_LIGHT} {{\n\
                 \x20   color: {color_text},\n\
                 \x20   intensity: {intensity},\n\
                 \x20   radius: {radius},\n\
                 \x20   shadow_maps_enabled: {shadows},\n\
                 }}\n",
                intensity = bsn_float(intensity),
                radius = bsn_float(radius),
            )))
        }
        "rectlight" => {
            let intensity = float_attr(attrs, "intensity", 3000.0)?;
            let range = float_attr(attrs, "range", 12.0)?;
            let width = float_attr(attrs, "width", 2.0)?;
            let height = float_attr(attrs, "height", 1.0)?;
            Ok(Some(format!(
                "{RECT_LIGHT} {{\n\
                 \x20   color: {color_text},\n\
                 \x20   intensity: {intensity},\n\
                 \x20   range: {range},\n\
                 \x20   width: {width},\n\
                 \x20   height: {height},\n\
                 }}\n",
                intensity = bsn_float(intensity),
                range = bsn_float(range),
                width = bsn_float(width),
                height = bsn_float(height),
            )))
        }
        "directionallight" => {
            let illuminance = float_attr(attrs, "illuminance", 10_000.0)?;
            let shadows = bool_attr(attrs, "shadows", true)?;
            Ok(Some(format!(
                "{DIRECTIONAL_LIGHT} {{\n\
                 \x20   color: {color_text},\n\
                 \x20   illuminance: {illuminance},\n\
                 \x20   shadow_maps_enabled: {shadows},\n\
                 }}\n",
                illuminance = bsn_float(illuminance),
            )))
        }
        _ => Ok(None),
    }
}

fn emit_world_node(
    tag: &str,
    attrs: &[(String, String)],
    text: &str,
    origin_file: &str,
    origin_path: &[u32],
    pose: bool,
) -> String {
    let mut out = format!(
        "{WORLD_NODE} {{\n\
         \x20   tag: \"{}\",\n\
         \x20   attrs: [\n",
        escape_string(tag)
    );
    for (key, value) in attrs {
        out.push_str(&format!(
            "        {WORLD_PROP} {{ key: \"{}\", value: \"{}\" }},\n",
            escape_string(key),
            escape_string(value),
        ));
    }
    out.push_str("    ],\n");
    out.push_str(&format!(
        "    text: \"{}\",\n\
         \x20   origin_file: \"{}\",\n\
         \x20   origin_path: [{}],\n\
         \x20   pose: {},\n\
         }}\n",
        escape_string(text),
        escape_string(origin_file),
        origin_path
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        pose,
    ));
    out
}

fn light_color(attrs: &[(String, String)]) -> Result<[f32; 3]> {
    match attr(attrs, "color") {
        Some(value) => parse_color(value, "<light color>"),
        None => Ok([1.0, 1.0, 1.0]),
    }
}

fn float_attr(attrs: &[(String, String)], key: &str, default: f32) -> Result<f32> {
    match attr(attrs, key) {
        Some(value) => parse_f32(value, key),
        None => Ok(default),
    }
}

fn bool_attr(attrs: &[(String, String)], key: &str, default: bool) -> Result<bool> {
    match attr(attrs, key) {
        Some(value) => parse_bool(value, key),
        None => Ok(default),
    }
}

fn bsn_float(value: f32) -> String {
    let text = format_f32(value);
    if text.contains('.') || text.contains('e') || text.contains('E') {
        text
    } else {
        format!("{text}.0")
    }
}

fn relative_path(path: &Path, root: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    path.strip_prefix(&root)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn indent(text: &str, level: usize) -> String {
    let pad = "    ".repeat(level);
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn escape_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Lê e valida um `.bsn` emitido, devolvendo um erro com o caminho.
pub fn validate_emitted(path: &Path, text: &str) -> Result<()> {
    jackdaw_bsn::parse_bsn_text(text)
        .map(|_| ())
        .map_err(|err| anyhow!("{}: {err}", path.display()))
}

/// Emite todos os nós de um ficheiro e valida o resultado.
pub fn emit_file_checked(xml_path: &Path, root_dir: &Path, options: EmitOptions) -> Result<String> {
    let text = emit_file(xml_path, root_dir, options)?;
    validate_emitted(xml_path, &text)?;
    Ok(text)
}
