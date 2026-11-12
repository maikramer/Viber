//! Declarative world XML and `.vinst` instances.
//!
//! This is the format layer: the element tree (`XmlNode`/`XmlDocument`), the
//! tolerant parser, include expansion, project discovery, typed value parsers
//! and the compact vegetation instance format. The YAML `config.yaml` and the
//! scene catalogs stay in higher layers.
//!
//! The types are re-exported by the `viber` root, so existing
//! `viber::xml::…` / `viber::project::…` / `viber::vinst::…` paths keep
//! working.

pub mod bsn;
pub mod project;
pub mod vinst;
pub mod xml;

pub use project::CONFIG_FILE;
pub use vinst::{decode, encode};
pub use xml::{NodeOrigin, XmlDocument, XmlNode};

use bevy::math::{Quat, Vec3};
use serde::{Deserialize, Serialize};

/// A writable terrain patch saved with the game (absolute heights over the
/// carved grid). The live `DeltaGrid` (rect merging, sampling) stays in the
/// runtime; this is only the on-disk record.
#[derive(Clone, Debug, Serialize, Deserialize, bevy::prelude::Reflect)]
// O save e o BRP registam este tipo; preservar o path depois da extração.
#[type_path = "viber::terrain::delta"]
pub struct DeltaPatch {
    pub min: [f32; 2],
    pub step: [f32; 2],
    pub cols: u32,
    pub rows: u32,
    pub data: Vec<f32>,
}

/// One placed template instance.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedInstance {
    pub position: Vec3,
    /// Heading alone (diagnostics/tests); the full rotation lives in
    /// [`Self::rotation`].
    pub yaw_deg: f32,
    /// Final world rotation: yaw and, with `align-to-terrain`, the partial tilt
    /// clamped toward the terrain normal.
    pub rotation: Quat,
    pub scale: Vec3,
    /// Index into the spawner's `template_urls` (and handles).
    pub template_index: usize,
}
