//! XML layer — root shim.
//!
//! The element tree, the tolerant parser, include expansion, XML emission,
//! the typed values and the pose/escape helpers live in `bevy_world_xml`.
//! The re-export keeps every existing `crate::xml::…` path working.

pub use bevy_world_xml::xml::*;
