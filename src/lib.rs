//! jedimem -- team memory for coding agents, stored as files in your repo.
pub mod compiler;
pub mod config;
pub mod importers;
pub mod memory;
pub mod migrate;
pub mod redact;
pub mod repo;
pub mod store;
pub mod update;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
