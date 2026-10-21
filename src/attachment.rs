//! Anexar props a ossos de um rig — primitiva GENÉRICA.
//!
//! Um jogo de corrida usa isto para o volante, o capacete, uma bandeira; um
//! RPG para a espada na mão. A engine não conhece "arma" nem "ferramenta":
//! só encontra o osso pelo nome, instancia o GLB como filho e troca/remove
//! por SLOT. A regra (qual URL, qual osso, quando) é do script
//! (`viber.attach`/`viber.detach`).

use std::collections::HashMap;

use bevy::prelude::*;

use crate::recipes::spawn::GltfScenePending;

/// Pedido de attach/detach vindo de um script.
#[derive(Debug, Clone)]
pub struct AttachmentRequest {
    /// Dono; `None` = herói.
    pub owner: Option<Entity>,
    /// Slot lógico (`"weapon"`, `"helmet"`…): substitui o que lá estiver.
    pub slot: String,
    /// URL do GLB; `None` = detach.
    pub url: Option<String>,
    /// Nome do osso (`"hand_r"`, `"head"`…).
    pub bone: String,
    pub pos: Vec3,
    pub rot: Vec3,
    pub scale: f32,
}

/// Fila de pedidos (o runtime Luau empurra; `apply_attachments` consome).
#[derive(Debug, Default, Resource)]
pub struct AttachmentRequests(pub Vec<AttachmentRequest>);

/// `(dono, slot)` → entidade anexada (para trocar/remover sem órfãos).
#[derive(Debug, Default, Resource)]
pub struct AttachmentSlots(pub HashMap<(Entity, String), Entity>);

pub struct AttachmentPlugin;

impl Plugin for AttachmentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttachmentRequests>()
            .init_resource::<AttachmentSlots>()
            .add_systems(Update, apply_attachments);
    }
}

/// Aplica a fila. Um pedido `url = None` só remove o slot.
#[allow(clippy::type_complexity)]
pub fn apply_attachments(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut requests: ResMut<AttachmentRequests>,
    mut slots: ResMut<AttachmentSlots>,
    players: Query<Entity, With<crate::player::Player>>,
    names: Query<&Name>,
    children: Query<&Children>,
) {
    let queued = std::mem::take(&mut requests.0);
    for request in queued {
        let owner = request.owner.or_else(|| players.single().ok());
        let Some(owner) = owner else {
            continue;
        };
        let key = (owner, request.slot);
        if let Some(old) = slots.0.remove(&key) {
            commands.entity(old).try_despawn();
        }
        let Some(url) = request.url else {
            continue;
        };
        let Some(bone) = find_bone(owner, &request.bone, &children, &names) else {
            warn!(
                target: "viber::attachment",
                "osso '{}' não encontrado — '{}' não anexado",
                request.bone,
                url
            );
            continue;
        };
        let handle = crate::meshopt::load_gltf(&asset_server, url);
        let transform = Transform {
            translation: request.pos,
            rotation: Quat::from_euler(EulerRot::XYZ, request.rot.x, request.rot.y, request.rot.z),
            scale: Vec3::splat(request.scale.max(0.001)),
        };
        let spawned = commands
            .spawn((
                transform,
                Visibility::Inherited,
                GltfScenePending::single(handle),
            ))
            .id();
        commands.entity(bone).add_child(spawned);
        slots.0.insert(key, spawned);
    }
}

/// Procura um osso por nome normalizado (exato ou substring), com os alias
/// comuns de mão. Genérico: não assume `hand_r` nem rig humanoide.
fn find_bone(
    root: Entity,
    wanted: &str,
    children: &Query<&Children>,
    names: &Query<&Name>,
) -> Option<Entity> {
    let wanted = wanted.trim().to_ascii_lowercase();
    let mut aliases = vec![wanted.clone()];
    match wanted.as_str() {
        "hand_r" | "righthand" | "right_hand" => {
            aliases.extend(["righthand".into(), "right_hand".into(), "hand_r".into()]);
        }
        "hand_l" | "lefthand" | "left_hand" => {
            aliases.extend(["lefthand".into(), "left_hand".into(), "hand_l".into()]);
        }
        _ => {}
    }
    fn walk(
        entity: Entity,
        aliases: &[String],
        children: &Query<&Children>,
        names: &Query<&Name>,
    ) -> Option<Entity> {
        if let Ok(name) = names.get(entity) {
            let name = name.as_str().to_ascii_lowercase();
            // "hand_r" não pode casar com "hand_r_index"/"finger".
            let is_finger = name.contains("finger") || name.contains("index") && name.contains("hand");
            if !is_finger
                && aliases
                    .iter()
                    .any(|alias| name == *alias || name.contains(alias))
            {
                return Some(entity);
            }
        }
        for child in children.get(entity).ok()?.iter() {
            if let Some(found) = walk(child, aliases, children, names) {
                return Some(found);
            }
        }
        None
    }
    walk(root, &aliases, children, names)
}
