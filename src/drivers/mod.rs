//! Built-in drivers.
//!
//! Each driver registers its steps through [`crate::core::registry`], the same
//! way an external driver would.

use crate::core::registry::Registry;

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "http")]
pub mod http;

/// Registers every driver enabled at build time.
pub fn register(registry: &mut Registry) -> Result<(), String> {
    #[cfg(feature = "cli")]
    {
        cli::register(registry)?;
    }
    #[cfg(feature = "http")]
    {
        http::register(registry)?;
    }
    #[cfg(not(any(feature = "cli", feature = "http")))]
    {
        let _ = registry;
    }
    Ok(())
}
