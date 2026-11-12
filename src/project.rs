//! Projeto + cenas — root shim for `bevy_world_xml::project`.
//!
//! The project/catalog discovery lives in the world-format package; the root
//! keeps the path so the engine, editor and CLI do not need to change.
pub use bevy_world_xml::project::*;
