//! Voxel terrain — root shim for `bevy_voxel_terrain`.
//!
//! The pure core (field/mods/index/transvoxel/arch/bridge/cave) lives in the
//! package. The ECS spawner, colliders, water/road authoring, cliffs, scatter
//! and the concrete `BrushGrid` remain here.
pub use bevy_voxel_terrain::*;

pub mod cliff;
pub mod riverbank;
pub mod scatter;
pub mod spawn;

pub use cliff::{CliffBand, build_cliff_mods, profile_offset};
pub use scatter::{RockFeaturesSpec, ScatterGuards, ScatterResult, ScatterStats, TakenDisc};
pub use spawn::{
    VoxelBoxSpec, VoxelChunk, VoxelLodShape, VoxelSpawnStats, build_box_mesh, column_boxes,
    lod_shape, spawn_voxel_columns,
};
pub(crate) use spawn::{spawn_box_entity, spawn_column};
