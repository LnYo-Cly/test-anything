//! Domain-neutral engine.
//!
//! This module holds the case model, the step registry, the event bus, the
//! runner, assertions and result rendering. It never mentions a concrete test
//! target: concrete steps are registered by the caller through
//! [`registry::Registry`]. A test in `tests/architecture.rs` enforces that.

pub mod assert;
pub mod bundle;
pub mod case;
pub mod events;
pub mod output;
pub mod registry;
pub mod report;
pub mod runner;
