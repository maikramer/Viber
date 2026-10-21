//! Anéis de residência do conteúdo.
//!
//! Quente (&lt;160 m): ECS completo. Morno (até 400 m): o script já congela
//! pelo raio de activação. Frio: a entidade com [`Streamable`] sai para um
//! registo e volta quando o herói se aproxima, com a pose e o `viber.state`
//! que deu para serializar. `stream="resident"` nunca arrefece.

use std::collections::HashMap;

use bevy::prelude::*;

pub const HOT_RADIUS: f32 = 160.0;
pub const COLD_RADIUS: f32 = 400.0;
/// Acorda um pouco antes do limite, para não piscar na fronteira.
pub const WAKE_RADIUS: f32 = 360.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    Hot,
    Warm,
    Cold,
}

pub fn ring_for(distance: f32) -> Ring {
    if distance < HOT_RADIUS {
        Ring::Hot
    } else if distance < COLD_RADIUS {
        Ring::Warm
    } else {
        Ring::Cold
    }
}

/// Pode ir para o registo frio. Sem isto (jogador, UI, terreno) fica.
#[derive(Component, Debug)]
pub struct Streamable;

/// Nunca arrefece (`stream="resident"`).
#[derive(Component, Debug)]
pub struct Resident;

#[derive(Clone, Debug)]
pub struct ColdRecord {
    pub key: String,
    /// Pose de MUNDO. A entidade pode ser um `<GltfScene>` filho de uma
    /// `<Entity>`; guardar o `Transform` local acordava a cena na origem.
    pub transform: Transform,
    pub name: Option<String>,
    pub scene: Option<Handle<bevy::world_serialization::WorldAsset>>,
    pub script: Option<String>,
    pub state_json: Option<String>,
    /// Proveniência do nó XML, para a entidade acordada poder voltar a dormir.
    pub source: crate::scene::SceneSource,
}

#[derive(Resource, Default)]
pub struct ColdStore {
    pub records: HashMap<String, ColdRecord>,
}

pub fn should_sleep(distance: f32, resident: bool, editor: bool) -> bool {
    !editor && !resident && ring_for(distance) == Ring::Cold
}

pub fn should_wake(distance: f32) -> bool {
    distance <= WAKE_RADIUS
}

fn source_key(origin: &crate::scene::SceneSource) -> String {
    let path = origin
        .0
        .path
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("/");
    format!("{}#{path}", origin.0.file.display())
}

pub fn stream_rings(
    mut commands: Commands,
    mut store: ResMut<ColdStore>,
    editor: Option<Res<crate::editor_camera::EditorCamState>>,
    players: Query<&GlobalTransform, With<crate::player::Player>>,
    mut host: Option<ResMut<crate::luau::LuaScriptHost>>,
    query: Query<(
        Entity,
        &GlobalTransform,
        &crate::scene::SceneSource,
        Option<&Name>,
        Option<&bevy::world_serialization::WorldAssetRoot>,
        Option<&crate::luau::LuaScriptRef>,
        Has<bevy::prelude::Mesh3d>,
        Has<crate::recipes::spawn::GltfScenePending>,
        Has<Resident>,
        Has<Streamable>,
    )>,
) {
    let Some(player) = players.iter().next() else {
        return;
    };
    let editing = editor.as_deref().is_some_and(|s| s.active);
    let hero = player.translation();
    let mut wake = Vec::new();
    for (key, record) in &store.records {
        let pos = record.transform.translation;
        if should_wake(pos.distance(hero)) {
            wake.push(key.clone());
        }
    }
    for key in wake {
        let Some(record) = store.records.remove(&key) else {
            continue;
        };
        let mut spawned = commands.spawn((
            Streamable,
            record.transform,
            Visibility::Inherited,
        ));
        if let Some(name) = record.name.clone() {
            spawned.insert(Name::new(name));
        }
        if let Some(scene) = record.scene.clone() {
            spawned.insert(bevy::world_serialization::WorldAssetRoot(scene));
        }
        if let Some(path) = record.script.clone() {
            spawned.insert(crate::luau::LuaScriptRef { path });
        }
        spawned.insert(record.source.clone());
        let id = spawned.id();
        if let (Some(host), Some(json)) = (host.as_mut(), record.state_json.as_deref()) {
            let _ = host.restore_entity_state(id, json);
        }
        let _ = key;
    }
    let mut sleep = Vec::new();
    for (entity, global, source, name, scene, script, has_mesh, pending, resident, streamable) in
        &query
    {
        if !streamable {
            continue;
        }
        // Só adormece o que sabe acordar: cena (WorldAssetRoot) ou entidade
        // sem visual. Um `<GltfScene>` ainda a carregar ou uma primitiva
        // (`Mesh3d`) não têm como ser reconstruídos e ficariam vazios.
        if scene.is_none() && (has_mesh || pending) {
            continue;
        }
        let distance = global.translation().distance(hero);
        if !should_sleep(distance, resident, editing) {
            continue;
        }
        let key = source_key(source);
        let state_json = script.and_then(|_| {
            host.as_ref()
                .and_then(|h| h.dump_entity_state(entity))
        });
        sleep.push((
            entity,
            ColdRecord {
                key: key.clone(),
                transform: global.compute_transform(),
                name: name.map(|n| n.as_str().to_string()),
                scene: scene.map(|s| s.0.clone()),
                script: script.map(|s| s.path.clone()),
                state_json,
                source: source.clone(),
            },
        ));
    }
    for (entity, record) in sleep {
        store.records.insert(record.key.clone(), record);
        commands.entity(entity).despawn();
    }
}

pub struct StreamingPlugin;

impl Plugin for StreamingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ColdStore>()
            .add_systems(Update, stream_rings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_split_hot_warm_and_cold() {
        assert_eq!(ring_for(10.0), Ring::Hot);
        assert_eq!(ring_for(200.0), Ring::Warm);
        assert_eq!(ring_for(500.0), Ring::Cold);
    }

    #[test]
    fn residents_and_the_editor_never_sleep() {
        assert!(!should_sleep(800.0, true, false));
        assert!(!should_sleep(800.0, false, true));
        assert!(should_sleep(800.0, false, false));
        assert!(should_wake(300.0));
        assert!(!should_wake(390.0));
    }

    #[test]
    fn a_named_npc_comes_back_inside_the_wake_radius() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.add_plugins(StreamingPlugin);
        app.world_mut().spawn((
            crate::player::Player::default(),
            Transform::from_xyz(0.0, 0.0, 0.0),
            GlobalTransform::from_xyz(0.0, 0.0, 0.0),
        ));
        let origin = crate::xml::NodeOrigin {
            file: std::sync::Arc::from(std::path::Path::new("world/npc.xml")),
            path: vec![0],
        };
        app.world_mut().spawn((
            Streamable,
            Name::new("mina"),
            Transform::from_xyz(0.0, 0.0, 500.0),
            GlobalTransform::from_xyz(0.0, 0.0, 500.0),
            crate::scene::SceneSource(std::sync::Arc::new(origin)),
        ));
        app.update();
        assert!(
            app.world().resource::<ColdStore>().records.contains_key("world/npc.xml#0"),
            "500 m adormece"
        );
        {
            let world = app.world_mut();
            let mut names = world.query::<&Name>();
            assert_eq!(names.iter(world).count(), 0);
        }
        let player = {
            let mut q = app
                .world_mut()
                .query_filtered::<Entity, With<crate::player::Player>>();
            let world = app.world();
            q.iter(world).next().unwrap()
        };
        app.world_mut()
            .entity_mut(player)
            .insert((
                Transform::from_xyz(0.0, 0.0, 500.0),
                GlobalTransform::from_xyz(0.0, 0.0, 500.0),
            ));
        app.update();
        let names: Vec<_> = app
            .world_mut()
            .query::<&Name>()
            .iter(app.world())
            .map(|n| n.as_str().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "mina"), "acorda com o nome: {names:?}");
        assert!(app.world().resource::<ColdStore>().records.is_empty());
    }
}
