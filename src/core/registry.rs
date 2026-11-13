//! The step registry: step names, their argument schemas and their behavior.

use std::collections::BTreeMap;

use serde_json::Value;

/// Why a step could not produce a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepError {
    /// The arguments do not satisfy the step's schema.
    InvalidArguments(String),
    /// The environment failed, for example a refused connection.
    Environment(String),
    /// The step exceeded its time budget.
    Timeout(String),
    /// The step ran but could not finish its work.
    Execution(String),
}

/// A step implementation and the contract it declares to the engine.
#[derive(Clone)]
pub struct StepDefinition {
    /// Name used in case files, for example `sample.call`.
    pub name: &'static str,
    /// One sentence describing what the step does.
    pub description: &'static str,
    /// JSON Schema for the step's arguments.
    pub args_schema: Value,
    /// Checks arguments against the same contract as [`Self::args_schema`],
    /// returning a message the user can act on.
    pub validate: fn(&Value) -> Result<(), String>,
    /// Runs the step and returns its JSON result.
    pub run: fn(&Value) -> Result<Value, StepError>,
}

/// The steps available to the runner, keyed by name.
#[derive(Default)]
pub struct Registry {
    steps: BTreeMap<&'static str, StepDefinition>,
}

impl Registry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a step.
    ///
    /// Two steps with the same name would make a case ambiguous, so a
    /// duplicate is refused instead of silently replacing the first.
    pub fn register(&mut self, definition: StepDefinition) -> Result<(), String> {
        if self.steps.contains_key(definition.name) {
            return Err(format!(
                "step `{}` is already registered; every step name must be unique",
                definition.name
            ));
        }
        self.steps.insert(definition.name, definition);
        Ok(())
    }

    /// Looks a step up by name.
    pub fn get(&self, name: &str) -> Option<&StepDefinition> {
        self.steps.get(name)
    }

    /// Every registered step name, in order.
    pub fn names(&self) -> Vec<&'static str> {
        self.steps.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(name: &'static str) -> StepDefinition {
        StepDefinition {
            name,
            description: "Test step.",
            args_schema: serde_json::json!({"type": "object"}),
            validate: |_| Ok(()),
            run: |_| Ok(Value::Null),
        }
    }

    #[test]
    fn registers_and_looks_up_steps() {
        let mut registry = Registry::new();
        registry
            .register(definition("test.one"))
            .expect("the first registration must succeed");

        assert!(registry.get("test.one").is_some());
        assert!(registry.get("test.two").is_none());
        assert_eq!(registry.names(), vec!["test.one"]);
    }

    #[test]
    fn refuses_a_duplicate_name() {
        let mut registry = Registry::new();
        registry
            .register(definition("test.one"))
            .expect("the first registration must succeed");

        let error = registry
            .register(definition("test.one"))
            .expect_err("the duplicate must be refused");
        assert!(error.contains("test.one"), "got: {error}");
    }
}
