//! Lock-on de câmara e strafe.
//!
//! O motor não lê a tecla: o jogo chama [`ScriptCommand::LockTarget`] /
//! [`ScriptCommand::CycleLock`] (Lua `viber.lock_target` / `viber.cycle_lock`).
//! Com [`LockOn`] no herói, a câmara fica atrás da linha herói→alvo e o
//! movimento passa a ser strafe relativo a essa linha.

use bevy::prelude::*;

use crate::camera::behind_yaw_deg;

/// Alcance (m) em que um hostil pode ser alvo.
pub const LOCK_RADIUS: f32 = 18.0;
/// A câmara aproxima-se desta fração da distância autorada.
pub const LOCK_DISTANCE_SCALE: f32 = 0.72;
/// Folga (m) para um hit no próprio alvo não contar como parede.
pub const LOS_SLACK: f32 = 0.45;

/// Alvo corrente do herói.
#[derive(Component, Debug, Clone, Copy)]
pub struct LockOn {
    pub target: Entity,
}

/// Pedidos vindos do Lua, aplicados antes do movimento.
#[derive(Resource, Debug, Default)]
pub struct LockRequests {
    /// `Some(None)` limpa; `Some(Some(e))` fixa.
    pub set: Option<Option<Entity>>,
    pub cycle: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct LockCandidate {
    pub entity: Entity,
    pub pos: Vec3,
}

/// `true` quando o raio chega ao alvo (ou não há mundo físico).
pub fn los_clear(hit_toi: Option<f32>, target_dist: f32) -> bool {
    match hit_toi {
        None => true,
        Some(toi) => toi + LOS_SLACK >= target_dist,
    }
}

/// Próximo alvo à volta do herói, por ângulo a partir do alvo actual.
pub fn cycle_lock(
    current: Option<Entity>,
    candidates: &[LockCandidate],
    origin: Vec3,
) -> Option<Entity> {
    if candidates.is_empty() {
        return None;
    }
    let mut ordered: Vec<&LockCandidate> = candidates.iter().collect();
    ordered.sort_by(|a, b| {
        let aa = (a.pos - origin).xz();
        let bb = (b.pos - origin).xz();
        aa.x.atan2(aa.y)
            .total_cmp(&bb.x.atan2(bb.y))
            .then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits()))
    });
    let start = current
        .and_then(|cur| ordered.iter().position(|c| c.entity == cur))
        .map(|i| (i + 1) % ordered.len())
        .unwrap_or(0);
    Some(ordered[start].entity)
}

/// Yaw de câmara que olha do herói para o alvo (atrás do herói).
pub fn lock_camera_yaw(hero: Vec3, target: Vec3) -> Option<f32> {
    let d = target - hero;
    let flat = Vec3::new(d.x, 0.0, d.z);
    if flat.length_squared() < 1e-6 {
        return None;
    }
    Some(behind_yaw_deg(flat.x, flat.z))
}

/// Direcção horizontal herói→alvo, para o strafe.
pub fn lock_flat_dir(hero: Vec3, target: Vec3) -> Option<Vec3> {
    let d = Vec3::new(target.x - hero.x, 0.0, target.z - hero.z);
    if d.length_squared() < 1e-6 {
        None
    } else {
        Some(d.normalize())
    }
}

fn cast_los(world: &World, origin: Vec3, target: Vec3) -> Option<f32> {
    use bevy_rapier3d::prelude::{
        QueryFilter, RapierContextColliders, RapierContextSimulation, RapierQueryPipeline,
        RapierRigidBodySet,
    };
    use bevy_rapier3d::rapier::parry::query::DefaultQueryDispatcher;
    let dir = target - origin;
    let dist = dir.length();
    if dist < 1e-3 {
        return None;
    }
    let context = world
        .iter_entities()
        .find(|e| e.contains::<RapierContextSimulation>())
        .map(|e| e.id())?;
    let simulation = world.get::<RapierContextSimulation>(context)?;
    let colliders = world.get::<RapierContextColliders>(context)?;
    let bodies = world.get::<RapierRigidBodySet>(context)?;
    RapierQueryPipeline::new_scoped(
        &simulation.broad_phase,
        colliders,
        bodies,
        &QueryFilter::default(),
        &DefaultQueryDispatcher,
        |pipeline| pipeline.cast_ray(origin, dir / dist, dist, true),
    )
    .map(|(_, toi)| toi)
}

/// Aplica pedidos e, no cycle, escolhe o hostil visível mais próximo do ângulo.
pub fn apply_lock(world: &mut World) {
    let request = world.resource::<LockRequests>().clone_request();
    if request.set.is_none() && !request.cycle {
        return;
    }
    world.resource_mut::<LockRequests>().set = None;
    world.resource_mut::<LockRequests>().cycle = false;

    let Some(player) = world
        .query_filtered::<Entity, With<crate::player::Player>>()
        .iter(world)
        .next()
    else {
        return;
    };
    let hero_pos = world
        .get::<GlobalTransform>(player)
        .map(|g| g.translation())
        .unwrap_or(Vec3::ZERO);

    if let Some(set) = request.set {
        match set {
            Some(target) if world.get_entity(target).is_ok() => {
                world.entity_mut(player).insert(LockOn { target });
            }
            _ => {
                world.entity_mut(player).remove::<LockOn>();
            }
        }
        return;
    }

    let mut candidates = Vec::new();
    let mut hostiles = world.query_filtered::<
        (Entity, &GlobalTransform),
        (With<crate::vitals::Health>, Without<crate::player::Player>),
    >();
    for (entity, xf) in hostiles.iter(world) {
        let pos = xf.translation();
        let dist = pos.distance(hero_pos);
        if dist > LOCK_RADIUS || dist < 0.2 {
            continue;
        }
        let origin = hero_pos + Vec3::Y * 1.5;
        let aim = pos + Vec3::Y * 1.0;
        if !los_clear(cast_los(world, origin, aim), origin.distance(aim)) {
            continue;
        }
        candidates.push(LockCandidate { entity, pos });
    }
    let current = world.get::<LockOn>(player).map(|l| l.target);
    match cycle_lock(current, &candidates, hero_pos) {
        Some(target) => {
            world.entity_mut(player).insert(LockOn { target });
        }
        None => {
            world.entity_mut(player).remove::<LockOn>();
        }
    }
}

trait CloneRequest {
    fn clone_request(&self) -> LockRequests;
}

impl CloneRequest for LockRequests {
    fn clone_request(&self) -> LockRequests {
        LockRequests {
            set: self.set,
            cycle: self.cycle,
        }
    }
}

pub struct LockOnPlugin;

impl Plugin for LockOnPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LockRequests>().add_systems(
            Update,
            apply_lock.before(crate::player::player_movement),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_treats_a_hit_on_the_target_as_clear() {
        assert!(los_clear(None, 10.0));
        assert!(los_clear(Some(9.7), 10.0));
        assert!(!los_clear(Some(3.0), 10.0));
    }

    #[test]
    fn cycle_walks_candidates_by_angle() {
        let mut world = World::new();
        let ea = world.spawn_empty().id();
        let eb = world.spawn_empty().id();
        let origin = Vec3::ZERO;
        let a = LockCandidate {
            entity: ea,
            pos: Vec3::new(0.0, 0.0, 5.0),
        };
        let b = LockCandidate {
            entity: eb,
            pos: Vec3::new(5.0, 0.0, 0.0),
        };
        let next = cycle_lock(Some(a.entity), &[a, b], origin);
        assert_eq!(next, Some(b.entity));
        let wrapped = cycle_lock(Some(b.entity), &[a, b], origin);
        assert_eq!(wrapped, Some(a.entity));
    }

    #[test]
    fn camera_yaw_looks_along_the_line_to_the_target() {
        let yaw = lock_camera_yaw(Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0)).unwrap();
        let (s, c) = yaw.to_radians().sin_cos();
        let forward = Vec3::new(-s, 0.0, -c);
        assert!(forward.dot(Vec3::Z) > 0.9);
    }
}
