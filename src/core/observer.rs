//! The observer contract: read-only probes that collect evidence around a step.
//!
//! An observer watches one layer of the environment — a log file today, a
//! network capture or a database later — and reports what changed while one
//! step ran. The engine creates one instance per case, calls
//! [`Observer::start`] before a step and [`Observer::collect`] after it, and
//! merges the collected value into the step result under `observed.<name>`, so
//! an ordinary `expect` entry can assert on it.
//!
//! Observers are **read-only**. The contract does not give them a way to change
//! the case, the environment or the result of a step, and every built-in
//! observer only reads: it opens files for reading, never for writing.

use serde_json::Value;

/// Why an observer could not do its work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserverError {
    /// The arguments do not satisfy the observer's schema.
    InvalidArguments(String),
    /// The environment failed, for example a file that cannot be read.
    Environment(String),
}

impl ObserverError {
    /// The message shown to the user, without the observer's own name.
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidArguments(message) | Self::Environment(message) => message,
        }
    }
}

/// One observation of one case.
///
/// The engine calls [`Self::start`] before a step and [`Self::collect`] after
/// it. An implementation may keep whatever state it needs between the two
/// calls, and that state belongs to one case only: cases that run at the same
/// time never share an instance.
pub trait Observer: Send {
    /// Starts observing the next step.
    fn start(&mut self) -> Result<(), ObserverError>;

    /// Stops observing and returns what the step left behind.
    fn collect(&mut self) -> Result<Value, ObserverError>;
}

/// An observer implementation and the contract it declares to the engine.
#[derive(Clone)]
pub struct ObserverDefinition {
    /// Name used in case files, for example `log.file`.
    pub name: &'static str,
    /// One sentence describing what the observer reports.
    pub description: &'static str,
    /// JSON Schema for the observer's arguments.
    pub args_schema: Value,
    /// Checks arguments against the same contract as [`Self::args_schema`],
    /// returning a message the user can act on.
    pub validate: fn(&Value) -> Result<(), String>,
    /// Creates the observation of one case.
    pub create: fn(&Value) -> Result<Box<dyn Observer>, ObserverError>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_observer_error_keeps_the_message_it_was_given() {
        let error = ObserverError::Environment("could not read `app.log`".to_string());

        assert_eq!(error.message(), "could not read `app.log`");
    }
}
