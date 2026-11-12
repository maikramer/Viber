//! Ponte entre o formato `.bsn` (Jackdaw) e o DSL de mundo do Viber.
//!
//! O `.bsn` é a fonte de verdade do mundo, mas o runtime continua a usar o
//! `parse_world`/spawner existentes: cada nó XML é preservado num
//! [`WorldNode`] e o loader reconstrói os `XmlNode`.
//!
//! A camada de editor (`GltfSource`, `Transform`, `Name`, luzes nativas) é
//! traduzida de volta para atributos, para o Jackdaw poder mover/editar o
//! mundo sem conhecer as tags do Viber.

pub mod compare;
pub mod components;
pub mod emit;
pub mod load;
pub mod pose;

pub use compare::worlds_equivalent;
pub use components::{WorldNode, WorldProp, tag_uses_transform};
pub use emit::{EmitOptions, emit_document, emit_file, emit_file_checked, validate_emitted};
pub use load::{load_bsn_world, load_bsn_world_in};
pub use pose::{Pose, read_pose, write_pose_attrs};
