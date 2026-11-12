//! Meshopt/KTX2 GLB decoder — root shim for `bevy_ext_gltf::meshopt`.
//!
//! The decoder, the multi-root asset reader and `load_gltf` live in the
//! `bevy_extensions` monorepo. The root keeps the path and the checkout-only
//! test helper.
pub use bevy_ext_gltf::meshopt::*;

/// The real asset pool of THIS checkout — integration-test helper only.
pub fn shared_asset_pool() -> Option<std::path::PathBuf> {
    let candidate = crate::vael_root()?.join("assets/public");
    (candidate.join("assets/meshes").is_dir()).then_some(candidate)
}
