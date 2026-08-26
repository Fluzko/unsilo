//! Fixture worlds and tree digests, shared by unit, integration and e2e tests.
//!
//! Fixtures are always generated from code, never copied from a real machine, so
//! no conversation content ever lands in the repository.

pub mod digest;
pub mod ids;
pub mod world;

pub use digest::{FileDigest, TreeDigest};
pub use world::{AccountBuilder, OrgBuilder, SessionSpec, World, WorldBuilder, scope_uuid};
