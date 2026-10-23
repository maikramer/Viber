//! IK de dois ossos (pés) e look-at da cabeça.
//!
//! A função [`solve_two_bone`] é pura. O sistema só corre em rigs com
//! [`FootIk`] que não estão congelados ([`crate::animation::FrozenAnimation`]):
//! é o mesmo corte que já evita animar criaturas longe.

use bevy::prelude::*;

use crate::animation::{AnimState, CharacterAnimator, FrozenAnimation};

/// Folga (m) entre a planta do pé e a superfície amostrada.
pub const FOOT_CLEARANCE: f32 = 0.02;
/// Quanto o pé pode descer/subir (m) antes do IK desistir (não estica o joelho ao infinito).
pub const FOOT_MAX_ERROR: f32 = 0.45;
/// Peso do look-at da cabeça para o alvo de lock.
pub const HEAD_LOOK_WEIGHT: f32 = 0.35;
/// Ângulo máximo (rad) que a cabeça pode rodar para o alvo.
pub const HEAD_LOOK_MAX: f32 = 40.0_f32.to_radians();

/// Marca um personagem cujos pés assentam no terreno.
#[derive(Component, Debug, Default)]
pub struct FootIk;

/// Peso do IK de pés para um estado de locomoção. No ar e em one-shots é 0.
pub fn foot_ik_weight(state: Option<AnimState>, busy: bool) -> f32 {
    if busy {
        return 0.0;
    }
    match state {
        Some(AnimState::Idle | AnimState::Walk) => 1.0,
        Some(AnimState::Run) => 0.65,
        _ => 0.0,
    }
}

/// Alvo vertical do pé: a superfície, se o erro couber em [`FOOT_MAX_ERROR`].
pub fn plant_foot_y(animated_y: f32, surface_y: f32) -> Option<f32> {
    let target = surface_y + FOOT_CLEARANCE;
    ((animated_y - target).abs() <= FOOT_MAX_ERROR).then_some(target)
}

/// Dois ossos: `root` fica, devolve as novas posições de `mid` e `end`.
///
/// Comprimentos dos ossos preservados. `target` fora do alcance é clampado.
/// O polo é a projecção do joelho actual, para o joelho não virar para trás.
pub fn solve_two_bone(root: Vec3, mid: Vec3, end: Vec3, target: Vec3) -> (Vec3, Vec3) {
    let len_ab = mid.distance(root).max(1e-4);
    let len_bc = end.distance(mid).max(1e-4);
    let mut to_target = target - root;
    let reach = to_target.length();
    if reach < 1e-5 {
        to_target = (end - root).normalize_or_zero() * (len_ab - len_bc).abs().max(1e-3);
    }
    let max_reach = (len_ab + len_bc) * 0.999;
    let min_reach = (len_ab - len_bc).abs() + 1e-4;
    let dist = to_target
        .length()
        .clamp(min_reach, max_reach.max(min_reach));
    let dir = to_target.normalize_or_zero();
    let cos_a = ((len_ab * len_ab + dist * dist - len_bc * len_bc) / (2.0 * len_ab * dist))
        .clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).max(0.0).sqrt();
    let mut pole = {
        let n = dir.normalize_or_zero();
        let v = mid - root;
        v - n * v.dot(n)
    };
    if pole.length_squared() < 1e-8 {
        let helper = if dir.dot(Vec3::Y).abs() > 0.9 {
            Vec3::X
        } else {
            Vec3::Y
        };
        pole = dir.cross(helper);
    }
    let pole = pole.normalize_or_zero();
    let new_mid = root + dir * (len_ab * cos_a) + pole * (len_ab * sin_a);
    let new_end = root + dir * dist;
    (new_mid, new_end)
}

/// Nome normalizado casa com um osso (`leftfoot`, `mixamorig:LeftFoot`, …).
pub fn bone_matches(name: &str, needles: &[&str]) -> bool {
    let n = crate::animation::normalize_clip_name(name);
    needles
        .iter()
        .any(|needle| n == *needle || n.ends_with(needle))
}

pub fn is_left_foot(name: &str) -> bool {
    bone_matches(name, &["leftfoot", "footl", "lfoot", "lefttoe"])
        && !bone_matches(name, &["leftfootik", "ik"])
}

pub fn is_right_foot(name: &str) -> bool {
    bone_matches(name, &["rightfoot", "footr", "rfoot", "righttoe"])
        && !bone_matches(name, &["rightfootik", "ik"])
}

/// Nomes de perna dos rigs da pipeline: `LeftUpLeg`/`LeftLeg` (Mixamo) e
/// `thigh_l`/`calf_l`/`upperleg_l` (sufixo de lado, a convenção do herói e de
/// TODOS os personagens do Vael). Sem as variantes com o lado no FIM, o
/// `plant_pair` não encontrava a coxa/canela do herói e o IK dos pés não
/// corria — o pé só era encontrado (`foot_l` → `footl`), a cadeia não.
pub fn is_left_knee(name: &str) -> bool {
    bone_matches(
        name,
        &[
            "leftleg",
            "leftcalf",
            "leftshin",
            "leftlowerleg",
            "calfl",
            "shinl",
            "lowerlegl",
        ],
    )
}

pub fn is_right_knee(name: &str) -> bool {
    bone_matches(
        name,
        &[
            "rightleg",
            "rightcalf",
            "rightshin",
            "rightlowerleg",
            "calfr",
            "shinr",
            "lowerlegr",
        ],
    )
}

pub fn is_left_hip(name: &str) -> bool {
    bone_matches(
        name,
        &[
            "leftupleg",
            "leftthigh",
            "leftupperleg",
            "thighl",
            "upperlegl",
            "uplegl",
        ],
    )
}

pub fn is_right_hip(name: &str) -> bool {
    bone_matches(
        name,
        &[
            "rightupleg",
            "rightthigh",
            "rightupperleg",
            "thighr",
            "upperlegr",
            "uplegr",
        ],
    )
}

pub fn is_head(name: &str) -> bool {
    let n = crate::animation::normalize_clip_name(name);
    n == "head" || n.ends_with("head") && !n.contains("headtop")
}

pub fn is_jaw(name: &str) -> bool {
    bone_matches(name, &["jaw", "chin"])
}

/// Rotação local que alinha o osso de `from_world` para `to_world`.
pub fn local_rotation_toward(
    parent_world_rot: Quat,
    bone_world_rot: Quat,
    from_world: Vec3,
    to_world: Vec3,
) -> Quat {
    let from = from_world.normalize_or_zero();
    let to = to_world.normalize_or_zero();
    if from.length_squared() < 1e-8 || to.length_squared() < 1e-8 {
        return parent_world_rot.inverse() * bone_world_rot;
    }
    let delta = Quat::from_rotation_arc(from, to);
    parent_world_rot.inverse() * (delta * bone_world_rot)
}

/// Abertura da mandíbula (rad) a partir de um envelope 0..1.
pub fn jaw_pitch(envelope: f32) -> f32 {
    -0.35 * envelope.clamp(0.0, 1.0)
}

fn walk_bones(
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
        walk_bones(child, children, names, out);
    }
}

fn world_of(entity: Entity, globals: &Query<&GlobalTransform>) -> Option<Vec3> {
    globals.get(entity).ok().map(|g| g.translation())
}

fn apply_aim(
    entity: Entity,
    parent_rot: Quat,
    bone_world_rot: Quat,
    from: Vec3,
    to: Vec3,
    transforms: &mut Query<&mut Transform>,
) {
    let local = local_rotation_toward(parent_rot, bone_world_rot, from, to);
    if let Ok(mut tf) = transforms.get_mut(entity) {
        tf.rotation = local;
    }
}

/// Assenta os pés e, com lock-on, vira a cabeça. Corre depois da animação.
pub fn foot_ik_system(
    mut characters: Query<(&CharacterAnimator, &GlobalTransform), With<FootIk>>,
    names: Query<&Name>,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    mut transforms: Query<&mut Transform>,
    rigs: Query<Has<FrozenAnimation>>,
    terrain: Option<Res<crate::terrain::runtime::TerrainRuntime>>,
) {
    let Some(terrain) = terrain.as_deref() else {
        return;
    };
    for (animator, _xform) in &mut characters {
        if rigs.get(animator.player).unwrap_or(false) {
            continue;
        }
        let weight = foot_ik_weight(animator.state, animator.is_busy());
        let mut bones = Vec::new();
        walk_bones(animator.player, &children, &names, &mut bones);
        if weight > 0.0 {
            plant_pair(
                &bones,
                is_left_hip,
                is_left_knee,
                is_left_foot,
                terrain,
                weight,
                &parents,
                &globals,
                &mut transforms,
            );
            plant_pair(
                &bones,
                is_right_hip,
                is_right_knee,
                is_right_foot,
                terrain,
                weight,
                &parents,
                &globals,
                &mut transforms,
            );
        }
    }
}

fn plant_pair(
    bones: &[(Entity, String)],
    hip_pred: fn(&str) -> bool,
    knee_pred: fn(&str) -> bool,
    foot_pred: fn(&str) -> bool,
    terrain: &crate::terrain::runtime::TerrainRuntime,
    weight: f32,
    parents: &Query<&ChildOf>,
    globals: &Query<&GlobalTransform>,
    transforms: &mut Query<&mut Transform>,
) {
    let Some((hip, _)) = bones.iter().find(|(_, n)| hip_pred(n)) else {
        return;
    };
    let Some((knee, _)) = bones.iter().find(|(_, n)| knee_pred(n)) else {
        return;
    };
    let Some((foot, _)) = bones.iter().find(|(_, n)| foot_pred(n)) else {
        return;
    };
    let (Some(hip_p), Some(knee_p), Some(foot_p)) = (
        world_of(*hip, globals),
        world_of(*knee, globals),
        world_of(*foot, globals),
    ) else {
        return;
    };
    let surface = if terrain.in_field(foot_p.x, foot_p.z) {
        terrain
            .surface_below(foot_p.x, foot_p.z, foot_p.y + FOOT_MAX_ERROR)
            .unwrap_or_else(|| terrain.sample_mesh_surface(foot_p.x, foot_p.z))
    } else {
        return;
    };
    let Some(y) =
        plant_foot_y(foot_p.y, surface).map(|target| foot_p.y + (target - foot_p.y) * weight)
    else {
        return;
    };
    let goal = Vec3::new(foot_p.x, y, foot_p.z);
    let (new_knee, new_foot) = solve_two_bone(hip_p, knee_p, foot_p, goal);
    let hip_parent = parents.get(*hip).map(|c| c.parent()).unwrap_or(*hip);
    let (Ok(hip_global), Ok(knee_global)) = (globals.get(*hip), globals.get(*knee)) else {
        return;
    };
    let hip_parent_rot = globals
        .get(hip_parent)
        .map(|g| g.rotation())
        .unwrap_or_else(|_| hip_global.rotation());
    // `local_rotation_toward` trata os vetores como DIREÇÕES: passar posições
    // absolutas alinhava o osso com o ponto do mundo (o joelho dobrava ao
    // contrário perto da origem e o IK sumia longe dela).
    let hip_local = local_rotation_toward(
        hip_parent_rot,
        hip_global.rotation(),
        knee_p - hip_p,
        new_knee - hip_p,
    );
    if let Ok(mut tf) = transforms.get_mut(*hip) {
        tf.rotation = hip_local;
    }
    // A coxa acabou de rodar: o `GlobalTransform` do osso só é reconstruído
    // depois deste sistema, por isso ler o pai da canela daqui devolvia a
    // rotação ANTIGA. A canela herdava então o delta da coxa duas vezes (a
    // direção saía rodada e o pé não chegava ao alvo). O pai efetivo da
    // canela é a rotação NOVA da coxa.
    let knee_parent = parents.get(*knee).map(|c| c.parent()).unwrap_or(*hip);
    let knee_parent_rot = if knee_parent == *hip {
        hip_parent_rot * hip_local
    } else {
        globals
            .get(knee_parent)
            .map(|g| g.rotation())
            .unwrap_or_else(|_| knee_global.rotation())
    };
    apply_aim(
        *knee,
        knee_parent_rot,
        knee_global.rotation(),
        foot_p - knee_p,
        new_foot - knee_p,
        transforms,
    );
}

/// Envelope de voz que abre a mandíbula enquanto o clip toca.
#[derive(Resource, Default)]
pub struct VoiceEnvelope {
    pub level: f32,
}

pub fn decay_voice_envelope(time: Res<Time>, mut env: ResMut<VoiceEnvelope>) {
    let dt = time.delta_secs();
    env.level = (env.level - dt * 1.6).max(0.0);
}

pub fn jaw_system(
    env: Res<VoiceEnvelope>,
    animators: Query<&CharacterAnimator>,
    names: Query<&Name>,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    mut transforms: Query<&mut Transform>,
) {
    if env.level <= 0.01 {
        return;
    }
    let pitch = jaw_pitch(env.level);
    for animator in &animators {
        let mut bones = Vec::new();
        walk_bones(animator.player, &children, &names, &mut bones);
        let Some((jaw, _)) = bones.into_iter().find(|(_, n)| is_jaw(n)) else {
            continue;
        };
        let parent = parents
            .get(jaw)
            .map(|c| c.parent())
            .unwrap_or(animator.player);
        let Ok(parent_g) = globals.get(parent) else {
            continue;
        };
        let Ok(self_g) = globals.get(jaw) else {
            continue;
        };
        let extra = Quat::from_rotation_x(pitch);
        if let Ok(mut tf) = transforms.get_mut(jaw) {
            tf.rotation = parent_g.rotation().inverse() * (self_g.rotation() * extra);
        }
    }
}

/// Rotações ADITIVAS por osso do herói, aplicadas **depois** da animação
/// (idle/walk/run/ataques) e **antes** da propagação de transforms.
///
/// É a camada que deixa o braço segurar a tocha inclinado à frente sem
/// congelar a locomoção: o clipe escreve a pose, isto soma uma rotação local
/// por cima. A chave é o nome do osso (`"upperarm_r"`, `"clavicle_r"`, …);
/// `Quat::IDENTITY` remove a entrada.
#[derive(Debug, Clone, Default, Resource)]
pub struct PlayerBoneOffsets {
    pub entries: Vec<(String, Quat)>,
}

impl PlayerBoneOffsets {
    pub fn set(&mut self, bone: &str, rotation: Quat) {
        if let Some(entry) = self.entries.iter_mut().find(|(name, _)| name == bone) {
            entry.1 = rotation;
        } else {
            self.entries.push((bone.to_string(), rotation));
        }
    }

    pub fn clear(&mut self, bone: &str) {
        self.entries.retain(|(name, _)| name != bone);
    }
}

/// Soma as rotações de [`PlayerBoneOffsets`] aos ossos do herói. Corre no
/// `PostUpdate` depois de `AnimationSystems` (a pose do clipe já está
/// escrita) e antes de `TransformSystems::Propagate` (os filhos — mão,
/// lampião — vêem a rotação no mesmo frame).
/// Desce a partir da entidade `<PlayerGLTF>` até ao `CharacterAnimator`.
///
/// O `Player` vive no nó XML; o `CharacterAnimator`/`FootIk` são inseridos
/// no **root da cena glTF** (um descendente), por isso uma query
/// `With<Player>` no animator vinha VAZIA — era o motivo de o braço da
/// tocha não mexer. Isto atravessa a hierarquia e devolve o primeiro filho
/// com animator (os tiers de LOD são rebindados pelo `rearm`).
fn find_animator<'a>(
    root: Entity,
    children: &Query<&Children>,
    animators: &'a Query<(Entity, &CharacterAnimator)>,
) -> Option<&'a CharacterAnimator> {
    if let Ok((_, animator)) = animators.get(root) {
        return Some(animator);
    }
    for child in children.get(root).ok()?.iter() {
        if let Some(found) = find_animator(child, children, animators) {
            return Some(found);
        }
    }
    None
}

pub fn player_bone_offsets_system(
    offsets: Res<PlayerBoneOffsets>,
    players: Query<Entity, With<crate::player::Player>>,
    animators: Query<(Entity, &CharacterAnimator)>,
    names: Query<&Name>,
    children: Query<&Children>,
    rigs: Query<Has<FrozenAnimation>>,
    mut transforms: Query<&mut Transform>,
) {
    if offsets.entries.is_empty() {
        return;
    }
    let Some(player) = players.iter().next() else {
        return;
    };
    let Some(animator) = find_animator(player, &children, &animators) else {
        return;
    };
    if rigs.get(animator.player).unwrap_or(false) {
        return;
    }
    let mut bones = Vec::new();
    walk_bones(animator.player, &children, &names, &mut bones);
    for (bone, rotation) in &offsets.entries {
        // A chave vem do script (`upperarm_r`), mas `bone_matches` normaliza
        // o NOME do osso (`upperarmr`) — sem normalizar a agulha também, o
        // `viber.bone_offset` documentado nunca casava com nada.
        let needle = crate::animation::normalize_clip_name(bone);
        let Some((entity, _)) = bones
            .iter()
            .find(|(_, name)| bone_matches(name, &[needle.as_str()]))
        else {
            continue;
        };
        if let Ok(mut tf) = transforms.get_mut(*entity) {
            // Pós-multiplica no espaço LOCAL do osso: a rotação soma-se à
            // pose animada em vez de a substituir.
            tf.rotation = (tf.rotation * *rotation).normalize();
        }
    }
}

pub struct IkPlugin;

impl Plugin for IkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VoiceEnvelope>()
            .init_resource::<PlayerBoneOffsets>()
            .add_systems(
                PostUpdate,
                (
                    foot_ik_system,
                    player_bone_offsets_system,
                    decay_voice_envelope,
                    jaw_system,
                )
                    .chain()
                    .after(bevy::app::AnimationSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_bone_keeps_lengths_and_reaches() {
        let root = Vec3::ZERO;
        let mid = Vec3::new(0.0, -0.5, 0.1);
        let end = Vec3::new(0.0, -1.0, 0.0);
        let target = Vec3::new(0.3, -0.8, 0.2);
        let (new_mid, new_end) = solve_two_bone(root, mid, end, target);
        let len_ab = mid.distance(root);
        let len_bc = end.distance(mid);
        assert!((new_mid.distance(root) - len_ab).abs() < 1e-3);
        assert!((new_end.distance(new_mid) - len_bc).abs() < 1e-3);
        assert!(new_end.distance(target) < 0.05 || new_end.distance(root) <= len_ab + len_bc);
    }

    #[test]
    fn foot_weight_is_zero_in_the_air_and_full_when_walking() {
        assert_eq!(foot_ik_weight(Some(AnimState::Walk), false), 1.0);
        assert_eq!(foot_ik_weight(Some(AnimState::Jump), false), 0.0);
        assert_eq!(foot_ik_weight(Some(AnimState::Walk), true), 0.0);
    }

    #[test]
    fn plant_rejects_a_foot_floating_far_above_the_ground() {
        assert!(plant_foot_y(1.0, 0.9).is_some());
        assert!(plant_foot_y(3.0, 0.0).is_none());
    }

    #[test]
    fn bone_names_match_mixamo_and_plain() {
        assert!(is_left_foot("mixamorig:LeftFoot"));
        assert!(is_right_knee("RightLeg"));
        assert!(is_head("mixamorig:Head"));
        assert!(is_jaw("Jaw"));
        assert!(!is_left_foot("RightFoot"));
    }

    /// O rig da pipeline (`hero_lod0.glb`, lobos, NPCs) usa o lado no FIM:
    /// `thigh_l` / `calf_l` / `foot_l`. As variantes Mixamo continuam válidas.
    #[test]
    fn bone_names_match_the_pipeline_side_suffix_rigs() {
        assert!(is_left_hip("thigh_l"));
        assert!(is_right_hip("thigh_r"));
        assert!(is_left_knee("calf_l"));
        assert!(is_right_knee("calf_r"));
        assert!(is_left_foot("foot_l"));
        assert!(is_right_foot("foot_r"));
        // Lado não se cruza (a coxa esquerda não é a direita nem o pé).
        assert!(!is_left_hip("thigh_r"));
        assert!(!is_right_knee("calf_l"));
        assert!(!is_left_hip("foot_l"));
        // Variantes com o lado no início continuam a casar.
        assert!(is_left_hip("mixamorig:LeftUpLeg"));
        assert!(is_right_knee("RightLeg"));
    }

    #[test]
    fn jaw_opens_down_with_the_envelope() {
        assert_eq!(jaw_pitch(0.0), 0.0);
        assert!(jaw_pitch(1.0) < -0.3);
    }
}
