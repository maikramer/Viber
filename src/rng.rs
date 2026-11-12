//! SplitMix64 — root shim for `bevy_engine_core::rng`.
//!
//! The implementation (and the "same seed, same world" contract) lives in the
//! core package; the root keeps the path so the engine and scripts do not
//! need to change.
pub use bevy_engine_core::rng::*;
