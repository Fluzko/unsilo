//! Unsilo keeps one view of your Claude Code conversations across accounts.
//!
//! The library holds every decision; `main.rs` only parses arguments, builds an
//! [`env::Env`] and maps errors onto exit codes.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/Fluzko/unsilo/main/assets/logo.svg",
    html_favicon_url = "https://raw.githubusercontent.com/Fluzko/unsilo/main/assets/logo.svg"
)]

pub mod attribution;
pub mod claude;
pub mod cli;
pub mod env;
pub mod error;
pub mod filter;
pub mod fsx;
pub mod index;
pub mod ledger;
pub mod ops;
pub mod report;
pub mod snapshot;
pub mod store;

pub use env::Env;
pub use error::{Error, Result};

/// Reported by `doctor` so a bug report says which build produced the output.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
