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
#[derive(Debug, Clone, Reflect)]
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
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct AttachmentRequests(pub Vec<AttachmentRequest>);

/// `(dono, slot)` → entidade anexada (para trocar/remover sem órfãos).
#[derive(Debug, Default, Resource, Reflect)]
#[reflect(Resource)]
pub struct AttachmentSlots(pub HashMap<(Entity, String), Entity>);

/// Quanto tempo um pedido espera pelo osso (o GLB do dono ainda a carregar)
/// antes de desistir com um aviso.
pub const ATTACH_BONE_WAIT_SECS: f32 = 15.0;

/// Pedidos cujo osso ainda não existe: o glTF do dono (herói, criatura) chega
/// alguns frames — ou segundos, num disco lento — depois do primeiro
/// `viber.attach`. A engine guarda-os e tenta de novo em cada frame, em vez de
/// o script repetir o pedido (cada repetição despachava e recarregava o GLB).
#[derive(Debug, Default, Resource)]
pub struct PendingAttachments(pub Vec<(Entity, AttachmentRequest, f32)>);

pub struct AttachmentPlugin;

impl Plugin for AttachmentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AttachmentRequests>()
            .init_resource::<AttachmentSlots>()
            .init_resource::<PendingAttachments>()
            .add_systems(Update, apply_attachments);
    }
}

/// Aplica a fila. Um pedido `url = None` só remove o slot.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn apply_attachments(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    time: Res<Time>,
    mut requests: ResMut<AttachmentRequests>,
    mut pending: ResMut<PendingAttachments>,
    mut slots: ResMut<AttachmentSlots>,
    players: Query<Entity, With<crate::player::Player>>,
    names: Query<&Name>,
    children: Query<&Children>,
    entities: Query<Entity>,
) {
    let queued = std::mem::take(&mut requests.0);
    if queued.is_empty() && pending.0.is_empty() {
        return;
    }
    // Donos (ou props) despawnados deixavam a entrada `(dono, slot) → entidade`
    // no mapa PARA SEMPRE — um clone por criatura equipada numa horda. A poda
    // corre só quando há pedidos (nunca por frame) e o `Query<Entity>` só
    // responde a `contains`, sem varrer o mundo.
    slots
        .0
        .retain(|(owner, _), child| entities.contains(*owner) && entities.contains(*child));
    // Pedidos à espera do osso vêm primeiro; um pedido novo para o mesmo
    // `(dono, slot)` substitui o que estava à espera (trocar de arma enquanto
    // o herói carrega não pode anexar a antiga depois da nova).
    let dt = time.delta_secs();
    let mut work: Vec<(Entity, AttachmentRequest, f32)> = Vec::new();
    for (owner, request, age) in std::mem::take(&mut pending.0) {
        if entities.contains(owner) {
            work.push((owner, request, age + dt));
        }
    }
    for request in queued {
        // `iter().next()` e não `single()`: com 2+ players (co-op) o
        // `single()` falhava e TODOS os pedidos sem `id` caíam em silêncio
        // — a espada/Tocha do herói nunca aparecia. O resto da engine já
        // usa o primeiro player nessa situação (`interact`, nav tile).
        let Some(owner) = request.owner.or_else(|| players.iter().next()) else {
            continue;
        };
        work.retain(|(o, r, _)| !(*o == owner && r.slot == request.slot));
        work.push((owner, request, 0.0));
    }
    for (owner, request, age) in work {
        let key = (owner, request.slot.clone());
        // O osso resolve-se ANTES de trocar o slot: falhar o osso não pode
        // deixar o dono sem o attachment ANTIGO — remover antes deixava a
        // troca de "mão para cabeça" (osso inexistente no rig) a despir a
        // tocha viva e não pôr nada em vez dela. Um detach (`url = None`)
        // continua a remover só o slot.
        let Some(url) = request.url.clone() else {
            if let Some(old) = slots.0.remove(&key) {
                commands.entity(old).try_despawn();
            }
            continue;
        };
        let Some(bone) = find_bone(owner, &request.bone, &children, &names) else {
            if age < ATTACH_BONE_WAIT_SECS {
                pending.0.push((owner, request, age));
            } else {
                warn!(
                    target: "viber::attachment",
                    "osso '{}' não encontrado em {:.0} s — '{}' não anexado (o anterior fica)",
                    request.bone,
                    ATTACH_BONE_WAIT_SECS,
                    url
                );
            }
            continue;
        };
        if let Some(old) = slots.0.remove(&key) {
            commands.entity(old).try_despawn();
        }
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
/// Público para quem precisa de saber se o osso já existe antes de pedir um
/// attach (o preview do editor espera a hierarquia do GLB carregar).
pub fn find_bone(
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
            let is_finger =
                name.contains("finger") || name.contains("index") && name.contains("hand");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_owner() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_asset::<bevy::gltf::Gltf>();
        app.add_plugins(AttachmentPlugin);
        let owner = app.world_mut().spawn((Transform::default(), Name::new("hero"))).id();
        (app, owner)
    }

    fn request(owner: Entity, url: &str) -> AttachmentRequest {
        AttachmentRequest {
            owner: Some(owner),
            slot: "weapon".into(),
            url: Some(url.into()),
            bone: "hand_r".into(),
            pos: Vec3::ZERO,
            rot: Vec3::ZERO,
            scale: 1.0,
        }
    }

    #[test]
    fn a_request_waits_for_the_bone_and_keeps_the_old_attachment() {
        let (mut app, owner) = app_with_owner();
        let old = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .resource_mut::<AttachmentSlots>()
            .0
            .insert((owner, "weapon".into()), old);
        app.world_mut()
            .resource_mut::<AttachmentRequests>()
            .0
            .push(request(owner, "sword.glb"));
        app.update();
        // Sem osso: o pedido fica à espera e a arma antiga continua na mão.
        assert_eq!(app.world().resource::<PendingAttachments>().0.len(), 1);
        assert_eq!(
            app.world().resource::<AttachmentSlots>().0.get(&(owner, "weapon".into())),
            Some(&old)
        );
        // O rig chega (glTF carregado): o pedido aplica-se sem o script repetir.
        let bone = app.world_mut().spawn(Name::new("hand_r")).id();
        app.world_mut().entity_mut(owner).add_child(bone);
        app.update();
        assert!(app.world().resource::<PendingAttachments>().0.is_empty());
        let now = app.world().resource::<AttachmentSlots>().0[&(owner, "weapon".into())];
        assert_ne!(now, old);
        assert_eq!(app.world().get::<ChildOf>(now).map(|c| c.parent()), Some(bone));
        assert!(app.world().get_entity(old).is_err());
    }

    #[test]
    fn a_newer_request_replaces_the_one_waiting_for_the_bone() {
        let (mut app, owner) = app_with_owner();
        app.world_mut()
            .resource_mut::<AttachmentRequests>()
            .0
            .push(request(owner, "sword.glb"));
        app.update();
        app.world_mut()
            .resource_mut::<AttachmentRequests>()
            .0
            .push(request(owner, "torch.glb"));
        app.update();
        let pending = &app.world().resource::<PendingAttachments>().0;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1.url.as_deref(), Some("torch.glb"));
    }
}
