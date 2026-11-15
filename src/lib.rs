//! jg — a small, local-first, git-compatible version control system.
//!
//! Layout:
//! - [`core`]: repository engine (objects, refs, index, diff, merge). No I/O to the terminal.
//! - [`ops`]: one module per command; each returns a serializable report.
//! - [`transport`]: push/fetch over local paths and SSH.

pub mod core;
pub mod ops;
pub mod transport;
pub mod util;
