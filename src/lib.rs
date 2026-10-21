//! Viber — native Bevy engine for AiGameKit declarative world XML.
//!
//! Library crate exposing the XML loader (`xml`), the recipe IR (`recipes`)
//! and gameplay modules (`terrain`); the `viber` binary is a thin CLI on top.

pub mod a11y;
pub mod adaptive_quality;
pub mod ai;
pub mod ambient;
pub mod audio_space;
pub mod animation;
pub mod app;
pub mod audit;
pub mod bake;
pub mod bridge;
pub mod camera;
pub mod cinematic;
pub mod cinematic_cli;
pub mod combat;
pub mod config;
pub mod dialogue;
pub mod dlss;
pub mod economy;
pub mod foliage;
pub mod editor_camera;
pub mod feedback;
pub mod graphics;
pub mod grass;
pub mod harvest;
pub mod hot_reload;
pub mod hud;
pub mod ibl;
pub mod ik;
pub mod input_map;
pub mod impact;
pub mod interact;
pub mod lint;
pub mod locale;
pub mod lockon;
pub mod luau;
pub mod menus;
pub mod meshopt;
pub mod music;
pub mod nav;
pub mod netplay;
pub mod particles;
pub mod physics;
pub mod physics_fx;
pub mod placement;
pub mod player;
pub mod postfx;
pub mod postfx_toggle;
pub mod probes;
pub mod profiler;
pub mod project;
pub mod projectile;
pub mod prop_tint;
pub mod prune;
pub mod quests;
pub mod ragdoll;
pub mod recipes;
pub mod render_lod;
pub mod rng;
pub mod save;
pub mod scaffold;
pub mod senses;
pub mod scene;
pub mod scene_loader;
pub mod session;
pub mod skills;
pub mod sky;
pub mod sky_nishita;
pub mod streaming;
pub mod spawn_gate;
pub mod spawner;
pub mod terrain;
pub mod textures;
pub mod trail;
pub mod travel;
pub mod ui;
pub mod vinst;
pub mod vitals;
pub mod water_ssr;
pub mod worldsys;
pub mod xml;

/// Checkout do jogo Vael, irmão deste repo.
///
/// `VAEL_ROOT` ganha. Sem a variável, procura `../Vael` a partir da raiz do
/// crate. Testes que leem o mundo do jogo saltam quando isto é `None`
/// (CI da engine sozinha).
pub fn vael_root() -> Option<std::path::PathBuf> {
    let candidate = match std::env::var_os("VAEL_ROOT") {
        Some(value) => std::path::PathBuf::from(value),
        None => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../Vael"),
    };
    let candidate = candidate.canonicalize().unwrap_or(candidate);
    candidate.is_dir().then_some(candidate)
}
