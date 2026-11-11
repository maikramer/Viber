//! Contrato de reflexão: os tipos de jogo da engine ficam visíveis ao BRP
//! (`bevy/query`, `bevy/get`, `bevy/mutate_components`) e ao acesso ECS genérico.
//!
//! Deliberadamente fora deste contrato:
//! - `RunMode` (`src/app.rs`): ficheiro de outro workstream; entra quando esse
//!   trabalho aterrar.
//! - `ui::modal::{UiModal, UiTabButton, UiTabPage}` e
//!   `ui::widgets::{UiSlider, UiCheck, UiInput}`: `src/ui/modal.rs` e
//!   `src/ui/widgets.rs` pertencem a outro workstream. Este contrato cobre-os
//!   quando os derives de lá aterrarem.
//! - Tipo internos de GPU/Rapier/VM Luau: os recursos existem no registo, mas os
//!   campos internos estão `#[reflect(ignore)]` (não são dados de jogo).

use bevy::ecs::reflect::{ReflectComponent, ReflectResource};
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectDeserializer, ReflectSerializer};
use bevy::reflect::{FromReflect, TypeRegistration, TypeRegistry};
use serde::de::DeserializeSeed;
use viber::reflect::ReflectPlugin;

fn registered<'a, T: TypePath>(registry: &'a TypeRegistry) -> &'a TypeRegistration {
    registry
        .get_with_type_path(T::type_path())
        .unwrap_or_else(|| panic!("{} não está no AppTypeRegistry", T::type_path()))
}

fn assert_component<T: TypePath>(registry: &TypeRegistry) {
    let registration = registered::<T>(registry);
    assert!(
        registration.data::<ReflectComponent>().is_some(),
        "{} está registado mas sem ReflectComponent",
        T::type_path()
    );
}

fn assert_resource<T: TypePath>(registry: &TypeRegistry) {
    let registration = registered::<T>(registry);
    assert!(
        registration.data::<ReflectResource>().is_some(),
        "{} está registado mas sem ReflectResource",
        T::type_path()
    );
}

/// Serializa com o `AppTypeRegistry`, deserializa para o tipo dinâmico e
/// reconstrói o tipo concreto — o mesmo caminho que o BRP usa.
fn roundtrip<T: Reflect + FromReflect + TypePath>(registry: &TypeRegistry, value: &T) -> T {
    let serializer = ReflectSerializer::new(value, registry);
    let json = serde_json::to_string(&serializer).expect("serializar valor refletido");
    let mut deserializer = serde_json::Deserializer::from_str(&json);
    let reflected = ReflectDeserializer::new(registry)
        .deserialize(&mut deserializer)
        .expect("deserializar valor refletido");
    T::from_reflect(reflected.as_ref()).expect("reconstruir o tipo concreto")
}

#[test]
fn curated_components_have_reflect_component() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(ReflectPlugin);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    // Gameplay
    assert_component::<viber::vitals::Health>(&registry);
    assert_component::<viber::vitals::Hostile>(&registry);
    assert_component::<viber::player::Player>(&registry);
    assert_component::<viber::ai::EnemyCreature>(&registry);
    assert_component::<viber::camera::BaseFov>(&registry);
    assert_component::<viber::recipes::spawn::OrbitCamera>(&registry);
    assert_component::<viber::animation::CharacterAnimator>(&registry);
    assert_component::<viber::luau::LuaScriptRef>(&registry);
    assert_component::<viber::luau::ScriptProps>(&registry);
    assert_component::<viber::luau::Groups>(&registry);
    assert_component::<viber::luau::SignalConnections>(&registry);
    assert_component::<viber::luau::ScriptActivation>(&registry);
    assert_component::<viber::luau::ScriptInteraction>(&registry);
    assert_component::<viber::scene_loader::ScenePortal>(&registry);
    assert_component::<viber::streaming::Streamable>(&registry);
    assert_component::<viber::physics::VoxelCollider>(&registry);
    assert_component::<viber::projectile::Projectile>(&registry);

    // UI declarativa (os tipos dos ficheiros de outro workstream ficam de fora).
    assert_component::<viber::ui::runtime::UiId>(&registry);
    assert_component::<viber::ui::runtime::UiTag>(&registry);
    assert_component::<viber::ui::runtime::UiClasses>(&registry);
    assert_component::<viber::ui::runtime::UiInlineStyle>(&registry);
    assert_component::<viber::ui::runtime::UiBar>(&registry);
    assert_component::<viber::ui::runtime::UiCooldown>(&registry);
    assert_component::<viber::ui::runtime::UiOrder>(&registry);
    assert_component::<viber::ui::runtime::UiBind>(&registry);
    assert_component::<viber::ui::runtime::UiDisabled>(&registry);
    assert_component::<viber::ui::runtime::UiPointerNone>(&registry);
}

#[test]
fn curated_resources_have_reflect_resource() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(ReflectPlugin);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    assert_resource::<viber::interact::InteractionFocus>(&registry);
    assert_resource::<viber::worldsys::DayCycleState>(&registry);
    assert_resource::<viber::worldsys::WeatherState>(&registry);
    assert_resource::<viber::save::SaveDir>(&registry);
    assert_resource::<viber::save::WorldSaveKey>(&registry);
    assert_resource::<viber::ui::runtime::UiClicks>(&registry);
    assert_resource::<viber::ui::runtime::UiRegistry>(&registry);
    assert_resource::<viber::camera::CameraShake>(&registry);
    assert_resource::<viber::camera::CameraSnapshot>(&registry);
    assert_resource::<viber::menus::MenusOpen>(&registry);
    assert_resource::<viber::input_map::ActionMap>(&registry);
    assert_resource::<viber::input_map::ActionState>(&registry);
    assert_resource::<viber::time_fx::HitStop>(&registry);
    assert_resource::<viber::locale::Locale>(&registry);
    assert_resource::<viber::senses::SenseBus>(&registry);
    assert_resource::<viber::luau::ScriptEventQueue>(&registry);
}

#[test]
fn plain_game_data_types_are_registered() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(ReflectPlugin);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    // Não são componentes nem recursos, mas viajam no save/histórico e são
    // úteis ao `bevy/get` sobre recursos que os contêm.
    registered::<viber::save::SaveGame>(&registry);
    registered::<viber::save::SaveSlotInfo>(&registry);
    registered::<viber::ai::EnemyState>(&registry);
    registered::<viber::input_map::Action>(&registry);
    registered::<viber::terrain::delta::DeltaPatch>(&registry);
    registered::<viber::scene_loader::ReturnPose>(&registry);
}

#[test]
fn health_round_trips_through_reflect_serde() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(ReflectPlugin);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    let health = viber::vitals::Health {
        current: 42.5,
        max: 100.0,
    };
    let back = roundtrip(&registry, &health);
    assert_eq!(back, health);
}

#[test]
fn ui_component_round_trips_through_reflect_serde() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(ReflectPlugin);
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    let id = viber::ui::runtime::UiId("hp-fill".into());
    let back = roundtrip(&registry, &id);
    assert_eq!(back.0, id.0);

    let classes = viber::ui::runtime::UiClasses(vec!["chip".into(), "gold".into()]);
    let back = roundtrip(&registry, &classes);
    assert_eq!(back.0, classes.0);
}
