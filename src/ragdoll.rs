//! Ragdoll curto na morte.
//!
//! Só corre em quem tem [`RagdollDeath`] (attr XML `ragdoll="true"`). Uma
//! cadeia reduzida — pélvis, tronco, cabeça e quatro membros — vira corpos
//! dinâmicos com juntas esféricas durante [`RAGDOLL_SECS`], e depois o fade
//! de [`crate::corpse::Corpse`] segue sozinho. Sem os ossos, o clip de morte
//! mantém-se.

use bevy::prelude::*;
use bevy_rapier3d::prelude::{
    Collider, GravityScale, ImpulseJoint, RigidBody, SphericalJointBuilder, Velocity,
};

use crate::animation::{CharacterAnimator, FrozenAnimation};
use crate::corpse::Corpse;

pub const RAGDOLL_SECS: f32 = 2.5;

/// Pedido autoral: esta criatura cai com volume em vez de só o clip.
#[derive(Component, Debug, Default)]
pub struct RagdollDeath;

/// Corpos da cadeia enquanto o ragdoll está vivo.
#[derive(Component)]
pub struct RagdollActive {
    pub until: f32,
    pub bodies: Vec<Entity>,
    /// Osso conduzido por cada corpo, paralelo a `bodies`.
    pub bones: Vec<Entity>,
}

/// `true` quando o tempo do ragdoll acabou.
pub fn ragdoll_expired(now: f32, until: f32) -> bool {
    now >= until
}

/// Ossos da cadeia e o pai de cada um (nome do pai, `None` = raiz).
pub fn chain_spec() -> &'static [(&'static str, Option<&'static str>)] {
    &[
        ("pelvis", None),
        ("spine", Some("pelvis")),
        ("head", Some("spine")),
        ("leftarm", Some("spine")),
        ("rightarm", Some("spine")),
        ("leftupleg", Some("pelvis")),
        ("rightupleg", Some("pelvis")),
    ]
}

fn kind_of(name: &str) -> Option<&'static str> {
    let n = crate::animation::normalize_clip_name(name);
    for (kind, _) in chain_spec() {
        if n == *kind || n.ends_with(kind) {
            return Some(*kind);
        }
    }
    // sinónimos
    if n.ends_with("hips") || n == "hips" {
        return Some("pelvis");
    }
    if n.ends_with("chest") || n.ends_with("spine1") || n == "spine" {
        return Some("spine");
    }
    if crate::ik::is_head(name) {
        return Some("head");
    }
    if n.ends_with("leftarm") || n.ends_with("leftupperarm") {
        return Some("leftarm");
    }
    if n.ends_with("rightarm") || n.ends_with("rightupperarm") {
        return Some("rightarm");
    }
    if crate::ik::is_left_hip(name) {
        return Some("leftupleg");
    }
    if crate::ik::is_right_hip(name) {
        return Some("rightupleg");
    }
    None
}

fn walk(
    root: Entity,
    children: &Query<&Children>,
    names: &Query<&Name>,
    out: &mut Vec<(Entity, String)>,
) {
    let Ok(kids) = children.get(root) else {
        return;
    };
    for child in kids.iter() {
        if let Ok(name) = names.get(child) {
            out.push((child, name.as_str().to_string()));
        }
        walk(child, children, names, out);
    }
}

pub fn start_ragdolls(
    mut commands: Commands,
    time: Res<Time>,
    corpses: Query<
        (Entity, &GlobalTransform, &CharacterAnimator),
        (Added<Corpse>, With<RagdollDeath>, Without<RagdollActive>),
    >,
    names: Query<&Name>,
    children: Query<&Children>,
    globals: Query<&GlobalTransform>,
) {
    for (entity, root_xf, animator) in &corpses {
        let mut found = Vec::new();
        walk(animator.player, &children, &names, &mut found);
        let mut by_kind: Vec<(&str, Entity, Vec3)> = Vec::new();
        for (bone, name) in &found {
            if let Some(kind) = kind_of(name)
                && let Ok(g) = globals.get(*bone)
                && !by_kind.iter().any(|(k, _, _)| *k == kind)
            {
                by_kind.push((kind, *bone, g.translation()));
            }
        }
        if by_kind.len() < 3 {
            continue;
        }
        // Não remover o handle AQUI: `freeze_ragdoll_graphs` corre depois no
        // chain e precisa de o ler para inserir `FrozenAnimation`. A remoção
        // antecipada (deferida) deixava o foot IK a mexer o ragdoll.
        let _ = root_xf;
        let mut bodies = Vec::new();
        let mut bones = Vec::new();
        let mut kind_body: Vec<(&str, Entity)> = Vec::new();
        for (kind, bone, pos) in &by_kind {
            let body = commands
                .spawn((
                    RagdollBodyOf(entity),
                    RigidBody::Dynamic,
                    Collider::capsule_y(0.18, 0.12),
                    GravityScale(1.0),
                    Velocity::default(),
                    Transform::from_translation(*pos),
                    Visibility::Hidden,
                ))
                .id();
            bodies.push(body);
            bones.push(*bone);
            kind_body.push((kind, body));
        }
        for (kind, _, _) in &by_kind {
            let Some((_, parent_kind)) = chain_spec().iter().find(|(k, _)| k == kind) else {
                continue;
            };
            let Some(parent_kind) = parent_kind else {
                continue;
            };
            let Some((_, child_body)) = kind_body.iter().find(|(k, _)| k == kind) else {
                continue;
            };
            let Some((_, parent_body)) = kind_body.iter().find(|(k, _)| *k == *parent_kind) else {
                continue;
            };
            let child_pos = by_kind
                .iter()
                .find(|(k, _, _)| k == kind)
                .map(|(_, _, p)| *p)
                .unwrap_or(Vec3::ZERO);
            let parent_pos = by_kind
                .iter()
                .find(|(k, _, _)| k == parent_kind)
                .map(|(_, _, p)| *p)
                .unwrap_or(Vec3::ZERO);
            let anchor = (child_pos - parent_pos) * 0.5;
            commands.entity(*child_body).insert(ImpulseJoint::new(
                *parent_body,
                SphericalJointBuilder::new()
                    .local_anchor1(anchor)
                    .local_anchor2(-anchor),
            ));
        }
        commands.entity(entity).insert(RagdollActive {
            until: time.elapsed_secs() + RAGDOLL_SECS,
            bodies,
            bones,
        });
    }
}

pub fn drive_ragdolls(
    mut commands: Commands,
    time: Res<Time>,
    mut rigs: Query<(Entity, &RagdollActive, &CharacterAnimator)>,
    bodies: Query<&GlobalTransform, Without<CharacterAnimator>>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    mut transforms: Query<&mut Transform>,
) {
    let now = time.elapsed_secs();
    for (entity, active, animator) in &mut rigs {
        if ragdoll_expired(now, active.until) {
            for body in &active.bodies {
                commands.entity(*body).despawn();
            }
            commands.entity(entity).remove::<RagdollActive>();
            // O fade do cadáver continua; o rig fica na última pose.
            let _ = animator;
            continue;
        }
        // Deslocamento MUNDO aplicado a cada osso neste frame. O `GlobalTransform`
        // do pai só é reconstruído no fim do schedule: converter o osso filho
        // contra a posição ANTIGA do pai (que este loop acabou de mover)
        // deslocava a cadeia inteira pelo passo do pai — a pélvis andava e o
        // tronco/cabeça/membros ficavam para trás. Os ossos chegam por ordem
        // pai→filho (depth-first do `start_ragdolls`), por isso o delta do pai
        // já está no map.
        let mut moved: std::collections::HashMap<Entity, Vec3> = std::collections::HashMap::new();
        for (body, bone) in active.bodies.iter().zip(active.bones.iter()) {
            let Ok(body_xf) = bodies.get(*body) else {
                continue;
            };
            let parent = parents
                .get(*bone)
                .map(|c| c.parent())
                .unwrap_or(animator.player);
            let Ok(parent_g) = globals.get(parent) else {
                continue;
            };
            let world = body_xf.translation();
            let parent_delta = moved.get(&parent).copied().unwrap_or(Vec3::ZERO);
            let local = parent_g
                .affine()
                .inverse()
                .transform_point3(world - parent_delta);
            if let Ok(mut tf) = transforms.get_mut(*bone) {
                tf.translation = local;
                let old = globals.get(*bone).map(|g| g.translation()).unwrap_or(world);
                moved.insert(*bone, world - old);
            }
        }
    }
}

/// Corpo de ragdoll ligado ao cadáver que o originou. Se o cadáver for
/// despawnado antes dos 2,5 s (corpse lifetime 1,4 s, troca de cena), o
/// `RagdollActive` desaparecia com ele e os corpos ficavam para sempre no
/// mundo da física.
#[derive(Component)]
pub struct RagdollBodyOf(pub Entity);

/// Recolhe corpos cujo dono já não tem ragdoll ativo.
pub fn reap_orphan_ragdoll_bodies(
    mut commands: Commands,
    bodies: Query<(Entity, &RagdollBodyOf)>,
    owners: Query<(), With<RagdollActive>>,
) {
    for (body, owner) in &bodies {
        if owners.get(owner.0).is_err() {
            commands.entity(body).despawn();
        }
    }
}

/// Garante que um rig em ragdoll não volta a ser animado pelo grafo.
pub fn freeze_ragdoll_graphs(
    mut commands: Commands,
    rigs: Query<&CharacterAnimator, With<RagdollActive>>,
    graphs: Query<&bevy::animation::graph::AnimationGraphHandle>,
) {
    for animator in &rigs {
        if let Ok(handle) = graphs.get(animator.player) {
            commands
                .entity(animator.player)
                .remove::<bevy::animation::graph::AnimationGraphHandle>()
                .insert(FrozenAnimation(handle.clone()));
        }
    }
}

pub struct RagdollPlugin;

impl Plugin for RagdollPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                start_ragdolls,
                freeze_ragdoll_graphs,
                drive_ragdolls,
                reap_orphan_ragdoll_bodies,
            )
                .chain(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_has_pelvis_spine_head_and_four_limbs() {
        let spec = chain_spec();
        assert_eq!(spec.len(), 7);
        assert_eq!(spec[0].0, "pelvis");
        assert!(spec.iter().any(|(k, _)| *k == "head"));
        assert!(spec.iter().any(|(k, _)| *k == "leftarm"));
    }

    #[test]
    fn expiry_is_inclusive_at_the_deadline() {
        assert!(!ragdoll_expired(1.0, 2.5));
        assert!(ragdoll_expired(2.5, 2.5));
    }

    #[test]
    fn marked_corpse_with_bones_spawns_bodies() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.add_systems(Update, start_ragdolls);
        let pelvis = app
            .world_mut()
            .spawn((Name::new("Pelvis"), Transform::from_xyz(0.0, 1.0, 0.0)))
            .id();
        let spine = app
            .world_mut()
            .spawn((
                Name::new("Spine"),
                Transform::from_xyz(0.0, 1.4, 0.0),
                ChildOf(pelvis),
            ))
            .id();
        let head = app
            .world_mut()
            .spawn((
                Name::new("Head"),
                Transform::from_xyz(0.0, 1.8, 0.0),
                ChildOf(spine),
            ))
            .id();
        let _ = head;
        let armature = app
            .world_mut()
            .spawn((
                Name::new("Armature"),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().entity_mut(pelvis).insert(ChildOf(armature));
        let creature = app
            .world_mut()
            .spawn((
                RagdollDeath,
                Corpse { timer: 8.0 },
                Transform::default(),
                GlobalTransform::default(),
                crate::animation::test_animator(armature),
            ))
            .id();
        app.update();
        let active = app.world().get::<RagdollActive>(creature);
        assert!(active.is_some(), "ossos suficientes abrem o ragdoll");
        assert!(active.unwrap().bodies.len() >= 3);
    }
}
