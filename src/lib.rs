//! Library target for the `ta` binary.
//!
//! The command-line entry point lives in `src/main.rs`. Modules that later
//! tasks build on, or that need unit tests, live here.

pub mod core;
pub mod drivers;
pub mod exit;

/// Dialect identifier of the case file schema.
///
/// It lives here rather than in `core` because the layering guard in
/// `tests/architecture.rs` refuses the `http` prefix anywhere under
/// `src/core/`, and this identifier begins with it. The value names a
/// specification; it is not a test target.
pub const CASE_SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";
