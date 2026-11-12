//! Terrain declarativo do Viber — heightfield como input de um campo 3D.
//!
//! # Contratos
//!
//! * **O heightfield é input, não a forma.** O [`brush::BrushGrid`] é o termo
//!   base do SDF (`densidade(p) = p.y − base.sample(p.xz)`); o
//!   [`voxel::VoxelField`] combina-o em CSG com os mods 3D e o transvoxel
//!   mesha o zero do campo. A grelha não é renderizada nem colide
//!   directamente — é o alvo dos carves e a fonte dos registries.
//! * **Colunas, não chunks 2.5D.** Cada coluna de `chunk-size` metros é
//!   meshada por caixas voxel e colide por um trimesh Rapier com os mesmos
//!   triângulos do render (`physics.rs`), assado no swap atómico do LOD.
//! * **LOD por coluna** com histerese e clamp 2:1; o transvoxel faz as
//!   transições laterais sem saias.
//! * **Pads** achatam o heightfield com um SDF de rounded-rect e falloff
//!   smoothstep; o [`pads::ResolvedPad`] guarda a cota resolvida.
//! * **Duas vias de look:** o tint legado de altura/inclinação em vertex
//!   colors, ou o splat blend de [`splat`] + [`layer_material`] (13 slots de
//!   camada, material bindless único), que substitui o tint.
//! * **Edições vivas** ficam no overlay [`delta::DeltaGrid`]; nenhum
//!   consumidor lê a grelha crua em runtime.
//!
//! # XML
//!
//! ```xml
//! <Terrain heightmap="terrain/height.png" world-size="256" max-height="50"
//!          chunk-size="64" levels="3" collision-resolution="64"
//!          base-color="#ffffff" color-rock="#6b6560" />
//! <!-- ou o blend de camadas (substitui o tint; areia nas margens): -->
//! <Terrain layers="grass vale_grass dirt forest_floor gravel
//!          mountain_stone sand snow_peak" shore-width="6" />
//! <TerrainPad at="20 -10" size="24 16" falloff="8" corner-radius="4" />
//! ```

pub mod brush;
pub mod cliffs;
pub mod cut;
pub mod decal;
pub mod delta;
pub mod features;
pub mod heightmap;
pub mod layer_material;
pub mod mesh;
pub mod pads;
pub mod paint;
pub mod paths;
pub mod plateau;
pub mod plugin;
pub mod roads;
pub mod runtime;
pub mod shore_rocks;
pub mod spec;
pub mod splat;
pub mod voxel;
pub mod water;
pub mod water_fx;
pub mod water_material;

pub use brush::{BrushGrid, BrushMode};
pub use cliffs::{CliffMask, CliffProfile, CliffSide, CliffSpec, sharpen_terrain};
pub use cut::CutSpec;
pub use decal::{GroundDecalSpec, ground_decal_mesh};
pub use delta::{DeltaGrid, EditedBase, TerrainEdit, TerrainEditQueue};
pub use heightmap::HeightMapU16;
pub use layer_material::TerrainChunkMaterial;
pub use mesh::{ChunkMeshData, HeightField};
pub use pads::ResolvedPad;
pub use paths::{chaikin_smooth, resample};
pub use plateau::PlateauSpec;
pub use plugin::TerrainPlugin;
pub use roads::{RoadNetworkSpec, RoadPath, RoadProfile, RoadSpec, SegmentSpec, WaySpec};
pub use runtime::TerrainFeaturesPlugin;
pub use spec::{TerrainPadSpec, TerrainSpec, TerrainTint};
pub use voxel::{
    Bounds3, BoxMod, CapsuleMod, ChunkClass, ModIndex, ModOp, Span, VoxelChunkParams, VoxelField,
    VoxelMod, build_voxel_mesh,
};
pub use water::{LakeSpec, RiverSpec, WaterBody, WaterKind};
pub use water_fx::WaterFxPlugin;
pub use water_material::{WaterExtension, WaterMaterial, WaterSurfaceConfig};
