//! Pose (translação/rotação/escala) de um nó, com as mesmas regras do parser
//! de recipes do Viber.
//!
//! Vive aqui (e não no runtime) para o conversor e o loader `.bsn` usarem a
//! mesma leitura e escrita sem depender da engine.

use anyhow::{Result, anyhow, bail};
use bevy::math::{EulerRot, Quat, Vec3};

use crate::xml::values::{parse_f32, parse_vec3};

pub const POSE_KEYS: &[&str] = &[
    "translation",
    "pos",
    "euler",
    "rotation",
    "scale",
    "transform",
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

pub fn has_pose_attrs(attrs: &[(String, String)]) -> bool {
    attrs
        .iter()
        .any(|(key, _)| POSE_KEYS.contains(&key.as_str()))
}

/// Lê a pose de um nó. Atributos inválidos dão erro (como no parse do mundo).
pub fn read_pose(attrs: &[(String, String)]) -> Result<Pose> {
    let mut translation = Vec3::ZERO;
    let mut euler_deg: Option<Vec3> = None;
    let mut quat: Option<Quat> = None;
    let mut scale = Vec3::ONE;
    for (key, value) in attrs {
        match key.as_str() {
            "translation" | "pos" => translation = Vec3::from(parse_vec3(value, "pose")?),
            "euler" => euler_deg = Some(Vec3::from(parse_vec3(value, "pose")?)),
            "rotation" => quat = Some(parse_rotation(value)?),
            "scale" => scale = Vec3::from(parse_vec3(value, "pose")?),
            "transform" => {
                for (part, value) in parse_component_string(value) {
                    match part.as_str() {
                        "pos" | "position" => translation = Vec3::from(parse_vec3(&value, "pose")?),
                        // Nesta forma `rotation` são GRAUS, como no runtime.
                        "euler" | "rotation" => {
                            euler_deg = Some(Vec3::from(parse_vec3(&value, "pose")?))
                        }
                        "scale" => scale = Vec3::from(parse_vec3(&value, "pose")?),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    // Como no `build_transform` do spawn: o quaternião ganha ao euler.
    let rotation = quat.unwrap_or_else(|| match euler_deg {
        Some(e) => Quat::from_euler(
            EulerRot::XYZ,
            e.x.to_radians(),
            e.y.to_radians(),
            e.z.to_radians(),
        ),
        None => Quat::IDENTITY,
    });
    Ok(Pose {
        translation,
        rotation,
        scale,
    })
}

/// Escreve a pose na forma canónica (`translation`, `euler` em graus, `scale`),
/// removendo os aliases `pos`/`rotation`/`transform`.
pub fn write_pose_attrs(attrs: &mut Vec<(String, String)>, pose: Pose) {
    attrs.retain(|(key, _)| !POSE_KEYS.contains(&key.as_str()));
    let euler = pose.rotation.to_euler(EulerRot::XYZ);
    attrs.push(("translation".to_string(), format_vec3(pose.translation)));
    attrs.push((
        "euler".to_string(),
        format!(
            "{} {} {}",
            format_euler(euler.0.to_degrees()),
            format_euler(euler.1.to_degrees()),
            format_euler(euler.2.to_degrees()),
        ),
    ));
    attrs.push(("scale".to_string(), format_vec3(pose.scale)));
}

/// `rotation="x y z w"` (quat) ou `rotation="x y z"` (euler em RADIANOS).
fn parse_rotation(value: &str) -> Result<Quat> {
    let parts: Vec<f32> = value
        .split_whitespace()
        .map(|part| parse_f32(part, "rotation"))
        .collect::<Result<_>>()?;
    match parts.as_slice() {
        [x, y, z, w] => Ok(Quat::from_xyzw(*x, *y, *z, *w).normalize()),
        [x, y, z] => Ok(Quat::from_euler(EulerRot::XYZ, *x, *y, *z)),
        n => bail!(
            "rotation: expected 3 (radians euler) or 4 (quaternion) components, got {}: `{value}`",
            n.len()
        ),
    }
}

/// `pos: 1 2 3; euler: 0 90 0; scale: 1` (aqui `euler`/`rotation` são graus).
fn parse_component_string(value: &str) -> Vec<(String, String)> {
    value
        .split(';')
        .filter_map(|part| {
            let (key, value) = part.split_once(':')?;
            let key = key.trim().to_ascii_lowercase();
            if key.is_empty() {
                return None;
            }
            Some((key, value.trim().to_string()))
        })
        .collect()
}

pub fn format_vec3(value: Vec3) -> String {
    format!(
        "{} {} {}",
        format_f32(value.x),
        format_f32(value.y),
        format_f32(value.z)
    )
}

pub fn format_f32(value: f32) -> String {
    format_rounded(value, 6)
}

/// Ângulos: 3 casas chegam para o erro de ida-e-volta quat→euler (≈1e-5°) e
/// mantêm o XML legível (`90`, não `89.999969`).
fn format_euler(value: f32) -> String {
    format_rounded(value, 3)
}

fn format_rounded(value: f32, decimals: usize) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" {
        text = "0".to_string();
    }
    text
}

/// Converte uma direção (como no `<DirectionalLight direction="…">`) na
/// rotação que leva o `-Z` local até ela, igual ao spawn do Viber.
pub fn direction_to_rotation(direction: Vec3) -> Quat {
    let dir = if direction.length_squared() > f32::EPSILON {
        direction.normalize()
    } else {
        Vec3::NEG_Y
    };
    Quat::from_rotation_arc(Vec3::NEG_Z, dir)
}

/// Inverso de [`direction_to_rotation`]: a direção que a luz ilumina.
pub fn rotation_to_direction(rotation: Quat) -> Vec3 {
    (rotation * Vec3::NEG_Z).normalize_or_zero()
}

/// Erro de contexto para os chamadores que querem um `anyhow::Result`.
pub fn pose_error(err: anyhow::Error) -> anyhow::Error {
    anyhow!("pose inválida: {err}")
}
