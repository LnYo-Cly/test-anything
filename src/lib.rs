//! Library target for the `ta` binary.
//!
//! The command-line entry point lives in `src/main.rs`. Modules that later
//! tasks build on, or that need unit tests, live here.

pub mod core;
pub mod drivers;
pub mod exit;
