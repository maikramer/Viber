//! Terrain mesh helpers — root shim for `bevy_voxel_terrain`.
//!
//! `HeightField`, `ChunkMeshData`, `TintParams` and `tint_vertex_color` live
//! in the voxel terrain package; the host implements `HeightField` for its
//! concrete grid. `TintParams::from(&TerrainTint)` lives in `terrain::spec`.
pub use bevy_voxel_terrain::{ChunkMeshData, HeightField, TintParams, tint_vertex_color};
