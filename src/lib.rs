pub mod annotations;
pub mod cli;
pub mod commands;
pub mod config;
pub mod context_output;
pub mod git;
pub mod graph;
pub mod index;
pub mod metadata;
pub mod migration;
pub mod model;
pub mod output;
pub mod repo;
pub mod review;
pub mod search;
pub mod source_span;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
