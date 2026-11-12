//! Registo de reflexão dos tipos públicos da engine.
//!
//! O [`ReflectPlugin`] põe no `AppTypeRegistry` os componentes e recursos que
//! o BRP (`bevy/query`, `bevy/get`, `bevy/mutate_components`) e o acesso ECS
//! genérico podem inspecionar. Só entram tipos com dados de jogo: caches
//! (Rapier, GPU, VM Luau, árvores de parse) ficam de fora por não terem valor
//! de inspeção — os campos internos desses recursos estão `#[reflect(ignore)]`.
//!
//! Os quatro módulos `ui/{widgets,modal,gamepad,events}.rs` são de outro
//! workstream e não são registados aqui (os seus derives entram quando esse
//! trabalho aterrar). `RunMode` (`src/app.rs`) também fica de fora pelo mesmo
//! motivo. Tipos privados (`BakeFeatureSnapshot`, `WaterSsrPipeline`, …) têm
//! `Reflect` mas não são alcançáveis a partir deste módulo — os donos podem
//! registá-los mais tarde.

use bevy::prelude::*;

/// Preenchimento de um campo `#[reflect(ignore)]` cujo tipo não implementa
/// `Default` (handles/pipelines de GPU). Só corre num `FromReflect`; reconstruir
/// esses recursos a partir de reflexão não é suportado.
pub(crate) fn unavailable<T>() -> T {
    panic!("campo ignorado sem valor reconstruível por reflexão")
}

/// Regista os tipos de jogo da engine no `AppTypeRegistry`.
pub struct ReflectPlugin;

impl Plugin for ReflectPlugin {
    fn build(&self, app: &mut App) {
        // Tipos da Bevy que os componentes da engine usam como campos; com
        // `MinimalPlugins` ninguém mais os regista.
        app.register_type::<Transform>()
            .register_type::<GlobalTransform>()
            .register_type::<Name>()
            .register_type::<Visibility>()
            .register_type::<Entity>()
            .register_type::<Vec2>()
            .register_type::<Vec3>()
            .register_type::<Quat>()
            .register_type::<Color>()
            .register_type::<KeyCode>();

        // ── gameplay ────────────────────────────────────────────────────
        app.register_type::<crate::vitals::Health>()
            .register_type::<crate::vitals::Hostile>()
            .register_type::<crate::player::Player>()
            .register_type::<crate::player::TeleportSettle>()
            .register_type::<crate::ai::AiLocomotion>()
            .register_type::<crate::ai::EnemyCreature>()
            .register_type::<crate::ai::WanderState>()
            .register_type::<crate::ai::EnemyState>()
            .register_type::<crate::interact::InteractionFocus>()
            .register_type::<crate::projectile::Projectile>()
            .register_type::<crate::projectile::ProjectileQueue>()
            .register_type::<crate::projectile::ProjectileRequest>()
            .register_type::<crate::projectile::ProjectileTemplate>()
            .register_type::<crate::projectile::Faction>()
            .register_type::<crate::spawn_gate::SpawnGated>()
            .register_type::<crate::spawn_gate::PendingSpawnGates>()
            .register_type::<crate::spawn_gate::GateSpec>()
            .register_type::<crate::attachment::AttachmentRequests>()
            .register_type::<crate::attachment::AttachmentSlots>()
            .register_type::<crate::attachment::AttachmentRequest>()
            .register_type::<crate::save::WorldSaveKey>()
            .register_type::<crate::save::SaveDir>()
            .register_type::<crate::save::SaveGame>()
            .register_type::<crate::save::SaveSlotInfo>();

        // ── Luau: ligação entidade↔script ──────────────────────────────
        app.register_type::<crate::luau::LuaScriptRef>()
            .register_type::<crate::luau::ScriptProps>()
            .register_type::<crate::luau::Groups>()
            .register_type::<crate::luau::SignalConnections>()
            .register_type::<crate::luau::ScriptActivation>()
            .register_type::<crate::luau::ScriptInteraction>()
            .register_type::<crate::luau::ScriptGameEvent>()
            .register_type::<crate::luau::ScriptEventQueue>()
            .register_type::<crate::luau::LuaScriptHost>()
            .register_type::<crate::luau::ScriptSystemOwners>();

        // ── animação ───────────────────────────────────────────────────
        app.register_type::<crate::animation::CharacterAnimator>()
            .register_type::<crate::animation::LocomotionProfile>()
            .register_type::<crate::animation::ManualAnimation>()
            .register_type::<crate::animation::AnimatedScene>()
            .register_type::<crate::animation::BoundAnimation>()
            .register_type::<crate::animation::HitReact>()
            .register_type::<crate::animation::HitReactCooldown>()
            .register_type::<crate::animation::FrozenAnimation>()
            .register_type::<crate::animation::AnimState>()
            .register_type::<crate::animation::LocomotionBlendNodes>();

        // ── câmara e entrada ───────────────────────────────────────────
        app.register_type::<crate::recipes::spawn::OrbitCamera>()
            .register_type::<crate::recipes::spawn::AutoOrbit>()
            .register_type::<crate::camera::CameraShake>()
            .register_type::<crate::camera::CameraFx>()
            .register_type::<crate::camera::CameraKick>()
            .register_type::<crate::camera::BaseFov>()
            .register_type::<crate::camera::ScriptCameraFree>()
            .register_type::<crate::camera::CameraSnapshot>()
            .register_type::<crate::input_map::ActionMap>()
            .register_type::<crate::input_map::ActionState>()
            .register_type::<crate::input_map::Action>()
            .register_type::<crate::input_map::Binding>()
            .register_type::<crate::time_fx::HitStop>()
            .register_type::<crate::time_fx::BaseTimeScale>();

        // ── UI declarativa ─────────────────────────────────────────────
        app.register_type::<crate::ui::runtime::UiId>()
            .register_type::<crate::ui::runtime::UiTag>()
            .register_type::<crate::ui::runtime::UiClasses>()
            .register_type::<crate::ui::runtime::UiInlineStyle>()
            .register_type::<crate::ui::runtime::UiOrder>()
            .register_type::<crate::ui::runtime::UiBind>()
            .register_type::<crate::ui::runtime::UiStyleDirty>()
            .register_type::<crate::ui::runtime::UiDisabled>()
            .register_type::<crate::ui::runtime::UiPointerNone>()
            .register_type::<crate::ui::runtime::UiRootSheets>()
            .register_type::<crate::ui::runtime::UiComputed>()
            .register_type::<crate::ui::runtime::UiPreviewState>()
            .register_type::<crate::ui::runtime::UiBar>()
            .register_type::<crate::ui::runtime::UiCooldown>()
            .register_type::<crate::ui::widgets::UiCheck>()
            .register_type::<crate::ui::widgets::UiSlider>()
            .register_type::<crate::ui::widgets::UiInput>()
            .register_type::<crate::ui::widgets::UiFocusedInput>()
            .register_type::<crate::ui::widgets::UiTooltipText>()
            .register_type::<crate::ui::widgets::UiTooltipLayer>()
            .register_type::<crate::ui::modal::UiModal>()
            .register_type::<crate::ui::modal::UiModalsOpen>()
            .register_type::<crate::ui::modal::UiTabButton>()
            .register_type::<crate::ui::modal::UiTabPage>()
            .register_type::<crate::ui::modal::UiTabs>()
            .register_type::<crate::ui::modal::UiScroll>()
            .register_type::<crate::ui::gamepad::UiGamepadFocus>()
            .register_type::<crate::ui::runtime::UiClicks>()
            .register_type::<crate::ui::runtime::UiRegistry>()
            .register_type::<crate::ui::runtime::UiTextMirror>()
            .register_type::<crate::ui::runtime::UiTextOverflowMirror>()
            .register_type::<crate::ui::interaction::Interaction>()
            .register_type::<crate::ui::interaction::Button>()
            .register_type::<crate::ui::anim::UiAnim>()
            .register_type::<crate::ui::anim::AnimKind>()
            .register_type::<crate::ui::fade::UiFade>()
            .register_type::<crate::ui::tween::UiTransitions>()
            .register_type::<crate::ui::tween::UiAnimation>()
            .register_type::<crate::ui::tween::StyleField>()
            .register_type::<crate::ui::tween::Easing>()
            .register_type::<crate::ui::tween::TweenValue>()
            .register_type::<crate::ui::tween::TweenRun>()
            .register_type::<crate::ui::style::StyleProps>()
            .register_type::<crate::ui::style::StyleState>()
            .register_type::<crate::ui::style::Measure>()
            .register_type::<crate::ui::style::Expr>()
            .register_type::<crate::ui::style::Decoration>()
            .register_type::<crate::ui::style::GradientSpec>()
            .register_type::<crate::ui::style::GradientStop>()
            .register_type::<crate::ui::style::LetterSpacingSpec>()
            .register_type::<crate::ui::style::TextTransform>()
            .register_type::<crate::ui::style::TextOverflowKind>()
            .register_type::<crate::ui::style::TransitionSpec>()
            .register_type::<crate::ui::style::Iterations>()
            .register_type::<crate::ui::style::AnimationSpec>()
            .register_type::<crate::ui::style::Keyframe>()
            .register_type::<crate::ui::style::Keyframes>()
            .register_type::<crate::ui::style::Compound>()
            .register_type::<crate::ui::style::Selector>()
            .register_type::<crate::ui::style::Combinator>()
            .register_type::<crate::ui::style::MediaCond>()
            .register_type::<crate::ui::style::Rule>()
            .register_type::<crate::ui::style::TextShadowSpec>()
            .register_type::<crate::ui::style::OutlineSpec>();

        // ── HUD e menus ────────────────────────────────────────────────
        app.register_type::<crate::hud::HudHidden>()
            .register_type::<crate::hud::HideInInterior>()
            .register_type::<crate::hud::markers::HudMarkers>()
            .register_type::<crate::hud::markers::HudPoint>()
            .register_type::<crate::hud::markers::HudBlip>()
            .register_type::<crate::menus::MenusOpen>()
            .register_type::<crate::locale::Locale>();

        // ── mundo: dia, clima, cenas, sentidos ─────────────────────────
        app.register_type::<crate::worldsys::DayCycleState>()
            .register_type::<crate::worldsys::DayCycleMode>()
            .register_type::<crate::worldsys::DayNightConfig>()
            .register_type::<crate::worldsys::WeatherState>()
            .register_type::<crate::worldsys::WeatherScheduler>()
            .register_type::<crate::scene_loader::CurrentScene>()
            .register_type::<crate::scene_loader::SceneIndex>()
            .register_type::<crate::scene_loader::ChangeScene>()
            .register_type::<crate::scene_loader::Arrival>()
            .register_type::<crate::scene_loader::LoaderPhase>()
            .register_type::<crate::scene_loader::SweepReport>()
            .register_type::<crate::scene_loader::ReturnPose>()
            .register_type::<crate::scene_loader::ScenePortal>()
            .register_type::<crate::scene::PrototypeInstance>()
            .register_type::<crate::scene::SceneSource>()
            .register_type::<crate::senses::SenseBus>()
            .register_type::<crate::senses::SenseInbox>()
            .register_type::<crate::senses::SenseMemory>()
            .register_type::<crate::senses::SenseEvent>()
            .register_type::<crate::senses::SenseKind>();

        // ── física e streaming ─────────────────────────────────────────
        app.register_type::<crate::physics::PendingCollider>()
            .register_type::<crate::physics::ColliderResolved>()
            .register_type::<crate::physics::VoxelCollider>()
            .register_type::<crate::physics::ColliderShape>()
            .register_type::<crate::physics::MeshAnchor>()
            .register_type::<crate::physics::TerrainCollisionStatus>()
            .register_type::<crate::physics::ColliderDebugToggle>()
            .register_type::<crate::streaming::Streamable>()
            .register_type::<crate::streaming::Resident>()
            .register_type::<crate::streaming::ColdStore>()
            .register_type::<crate::streaming::ColdRecord>()
            .register_type::<crate::spawner::SpawnExclusions>()
            .register_type::<crate::spawner::SpawnExclusion>();

        // ── terreno ────────────────────────────────────────────────────
        app.register_type::<crate::terrain::plugin::TerrainChunk>()
            .register_type::<crate::terrain::runtime::TerrainRuntime>()
            .register_type::<crate::terrain::runtime::TerrainSplat>()
            .register_type::<crate::terrain::runtime::TerrainChunkMaterials>()
            .register_type::<crate::terrain::runtime::TerrainTextureGate>()
            .register_type::<crate::terrain::runtime::PendingTerrainTextures>()
            .register_type::<crate::terrain::runtime::ChunkLayerMap>()
            .register_type::<crate::terrain::runtime::WatchedTexture>()
            .register_type::<crate::terrain::delta::TerrainEdit>()
            .register_type::<crate::terrain::delta::DeltaPatch>()
            .register_type::<crate::terrain::delta::TerrainEditQueue>()
            .register_type::<crate::terrain::water::WaterBody>()
            .register_type::<crate::terrain::water::WaterKind>()
            .register_type::<crate::terrain::water::CascadeInfo>()
            .register_type::<crate::terrain::water::LakeShape>()
            .register_type::<crate::terrain::roads::RoadPath>()
            .register_type::<crate::terrain::roads::RoadProfile>()
            .register_type::<crate::terrain::pads::ResolvedPad>()
            .register_type::<crate::terrain::splat::SplatTuning>()
            .register_type::<crate::terrain::layer_material::TerrainChunkConfig>()
            .register_type::<crate::terrain::voxel::spawn::VoxelBoxSpec>()
            .register_type::<crate::terrain::water_fx::WaterContact>()
            .register_type::<crate::terrain::water_fx::Ripple>()
            .register_type::<crate::terrain::water_fx::RippleAssets>()
            .register_type::<crate::terrain::cliffs::CliffMask>()
            .register_type::<crate::terrain::cliffs::CliffBaked>();

        // ── apresentação e áudio ───────────────────────────────────────
        app.register_type::<crate::render_lod::CullDistance>()
            .register_type::<crate::render_lod::CullRangeApplied>()
            .register_type::<crate::render_lod::AutoCull>()
            .register_type::<crate::render_lod::NoShadowSubtree>()
            .register_type::<crate::render_lod::MeshLod>()
            .register_type::<crate::render_lod::MeshLodStats>()
            .register_type::<crate::prop_tint::TransmissiveGltf>()
            .register_type::<crate::grass::GrassTile>()
            .register_type::<crate::grass::GrassSettings>()
            .register_type::<crate::grass::GrassField>()
            .register_type::<crate::foliage::SimpleFoliage>()
            .register_type::<crate::foliage::FoliageCandidate>()
            .register_type::<crate::foliage::FoliageTile>()
            .register_type::<crate::foliage::ImpostorCard>()
            .register_type::<crate::ambient::CurrentBiome>()
            .register_type::<crate::ambient::PointShadowsEnabled>()
            .register_type::<crate::ambient::SfxHandles>()
            .register_type::<crate::ambient::SfxClip>()
            .register_type::<crate::ambient::RainEmitter>()
            .register_type::<crate::ambient::RainAmbienceLoop>()
            .register_type::<crate::ambient::WaterAmbienceLoop>()
            .register_type::<crate::ambient::WaterfallLoop>()
            .register_type::<crate::ambient::TravellerLantern>()
            .register_type::<crate::ambient::TorchLightOverride>()
            .register_type::<crate::audio_space::SpatialOneShot>()
            .register_type::<crate::music::AudioDuck>()
            .register_type::<crate::music::AudioMixerSettings>()
            .register_type::<crate::music::MusicLayerOverride>()
            .register_type::<crate::music::AudioReverb>()
            .register_type::<crate::music::ReverbPreset>()
            .register_type::<crate::music::DuckLevel>()
            .register_type::<crate::decal_fx::DecalQueue>()
            .register_type::<crate::decal_fx::DecalFade>()
            .register_type::<crate::decal_fx::DecalRequest>()
            .register_type::<crate::impact::HitRecoil>()
            .register_type::<crate::impact::ImpactRing>()
            .register_type::<crate::feedback::HitFlash>()
            .register_type::<crate::cinematic::CinematicLibrary>()
            .register_type::<crate::cinematic::CinematicPlayback>()
            .register_type::<crate::cinematic::CinematicSpec>()
            .register_type::<crate::sky::SkyModelState>()
            .register_type::<crate::sky::SkyModel>()
            .register_type::<crate::sky_nishita::NishitaModel>()
            .register_type::<crate::adaptive_quality::AdaptiveQuality>()
            .register_type::<crate::adaptive_quality::QualityMode>()
            .register_type::<crate::frame_pacing::FramePacer>()
            .register_type::<crate::frame_pacing::PacingMode>();

        // ── rede, profiler e bridge ────────────────────────────────────
        app.register_type::<crate::netplay::NetLink>()
            .register_type::<crate::netplay::NetGhost>()
            .register_type::<crate::netplay::NetRole>()
            .register_type::<crate::profiler::ProfilerState>()
            .register_type::<crate::profiler::FrameCounter>()
            .register_type::<crate::profiler::FrameStats>()
            .register_type::<crate::profiler::HitchLog>()
            .register_type::<crate::profiler::Hitch>()
            .register_type::<crate::profiler::ProfilerExtras>()
            .register_type::<crate::profiler::timed::FrameHotspot>()
            .register_type::<crate::bridge::events::BridgeEventLog>()
            .register_type::<crate::bridge::events::BridgeEvent>()
            .register_type::<crate::bridge::selection::SelectedEntity>();
    }
}
