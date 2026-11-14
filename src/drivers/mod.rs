//! Built-in drivers and observers.
//!
//! Each driver and observer registers through [`crate::core::registry`], the
//! same way an external one would.

use crate::core::registry::Registry;

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "log")]
pub mod log;

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

/// Registers every observer enabled at build time.
pub fn register_observers(registry: &mut Registry) -> Result<(), String> {
    #[cfg(feature = "log")]
    {
        log::register(registry)?;
    }
    #[cfg(not(feature = "log"))]
    {
        let _ = registry;
    }
    Ok(())
}
