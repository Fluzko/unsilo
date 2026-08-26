//! Snapshots: a captured copy of Claude's state, or of Unsilo's own.

pub mod manifest;
pub mod read;
pub mod write;

pub use manifest::{Entry, EntryKind, Manifest, Scope};
pub use write::Options;
