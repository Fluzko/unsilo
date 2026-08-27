//! One module per command. Each returns a plain data structure; rendering lives
//! in `report`, so tests assert on values rather than on formatted text.

pub mod apply;
pub mod doctor;
pub mod find;
pub mod ingest;
pub mod label;
pub mod off;
pub mod restore;
pub mod snapshot;
