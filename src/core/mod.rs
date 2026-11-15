//! Core repository engine. Nothing in here prints or parses CLI arguments;
//! commands live in `crate::ops`.

pub mod config;
pub mod diff;
pub mod history;
pub mod ignore;
pub mod index;
pub mod merge;
pub mod object;
pub mod odb;
pub mod oid;
pub mod refs;
pub mod repo;
pub mod revparse;
pub mod tree;
pub mod worktree;

pub use oid::Oid;
pub use repo::Repo;
