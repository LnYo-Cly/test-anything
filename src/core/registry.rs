//! The registry: step and observer names, their argument schemas and their
//! behavior.
//!
//! Steps and observers share the registry, the name rule and the shape of their
//! declaration, so a plugin registers both the same way. They are kept in
//! separate namespaces: a step named `sample.call` and an observer named
//! `sample.call` do not collide, because a case names a step in `steps:` and an
//! observer in `observe:`.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::core::observer::ObserverDefinition;
use crate::core::session::Sessions;

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
    ///
    /// The [`Sessions`] value belongs to the running case. A step that needs a
    /// long-lived resource asks it for the resource on first use and receives
    /// the same one on every later step of that case.
    pub run: fn(&Value, &mut Sessions) -> Result<Value, StepError>,
}

/// The steps and observers available to the runner, keyed by name.
#[derive(Default)]
pub struct Registry {
    steps: BTreeMap<&'static str, StepDefinition>,
    observers: BTreeMap<&'static str, ObserverDefinition>,
}

impl Registry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a step.
    ///
    /// Two steps with the same name would make a case ambiguous, so a
    /// duplicate is refused instead of silently replacing the first. The name
    /// itself is checked by [`validate_name`].
    pub fn register(&mut self, definition: StepDefinition) -> Result<(), String> {
        validate_name(definition.name)?;
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

    /// Adds an observer.
    ///
    /// An observer gets the same name rule and the same uniqueness rule as a
    /// step, but its own namespace: it is declared in `observe:`, never in
    /// `steps:`.
    pub fn register_observer(&mut self, definition: ObserverDefinition) -> Result<(), String> {
        validate_name(definition.name)?;
        if self.observers.contains_key(definition.name) {
            return Err(format!(
                "observer `{}` is already registered; every observer name must be unique",
                definition.name
            ));
        }
        self.observers.insert(definition.name, definition);
        Ok(())
    }

    /// Looks an observer up by name.
    pub fn observer(&self, name: &str) -> Option<&ObserverDefinition> {
        self.observers.get(name)
    }

    /// Every registered observer name, in order.
    pub fn observer_names(&self) -> Vec<&'static str> {
        self.observers.keys().copied().collect()
    }
}

/// Field names every step already gives a meaning to.
const RESERVED_NAMES: [&str; 2] = ["expect", "save"];

/// Checks the name a step registers under.
///
/// A name is written in every case that uses the step and becomes a key of the
/// schema's `$defs`, so it is limited to characters that need no escaping and
/// that read the same on every platform. The two fields a step already carries,
/// `expect` and `save`, cannot also name a step.
fn validate_name(name: &str) -> Result<(), String> {
    if RESERVED_NAMES.contains(&name) {
        return Err(format!(
            "step name `{name}` is reserved for the case format; choose another name"
        ));
    }
    if name.is_empty() {
        return Err(
            "step name is empty; use lowercase letters, digits, `.`, `_` and `-`".to_string(),
        );
    }
    if let Some(character) = name
        .chars()
        .find(|character| !is_allowed_name_character(*character))
    {
        return Err(format!(
            "step name `{name}` contains `{character}`; use only lowercase letters, digits, `.`, `_` and `-`"
        ));
    }
    Ok(())
}

/// Whether one character may appear in a name the registry holds.
///
/// The rule is shared with the names a case gives its observations, so a key of
/// `observed` reads the same way as a step or observer name.
pub fn is_allowed_name_character(character: char) -> bool {
    character.is_ascii_lowercase()
        || character.is_ascii_digit()
        || matches!(character, '.' | '_' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::observer::Observer;

    fn definition(name: &'static str) -> StepDefinition {
        StepDefinition {
            name,
            description: "Test step.",
            args_schema: serde_json::json!({"type": "object"}),
            validate: |_| Ok(()),
            run: |_, _| Ok(Value::Null),
        }
    }

    /// An observer that collects nothing.
    struct Silent;

    impl Observer for Silent {
        fn start(
            &mut self,
            _sessions: &mut Sessions,
        ) -> Result<(), crate::core::observer::ObserverError> {
            Ok(())
        }

        fn collect(
            &mut self,
            _sessions: &mut Sessions,
        ) -> Result<Value, crate::core::observer::ObserverError> {
            Ok(Value::Null)
        }
    }

    fn observer(name: &'static str) -> crate::core::observer::ObserverDefinition {
        crate::core::observer::ObserverDefinition {
            name,
            description: "Test observer.",
            args_schema: serde_json::json!({"type": "object"}),
            validate: |_| Ok(()),
            create: |_| Ok(Box::new(Silent)),
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

    #[test]
    fn accepts_the_characters_a_step_name_may_use() {
        for name in ["demo.call", "demo_call", "demo-call", "demo2.call", "a"] {
            let mut registry = Registry::new();
            registry
                .register(definition(name))
                .unwrap_or_else(|error| panic!("`{name}` must be accepted: {error}"));
        }
    }

    #[test]
    fn refuses_a_name_with_a_character_outside_the_rule() {
        for (name, offending) in [
            ("Demo.call", 'D'),
            ("demo/Call", '/'),
            ("demo~call", '~'),
            ("demo call", ' '),
            ("demo:call", ':'),
            ("demo.cäse", 'ä'),
            ("", '\0'),
        ] {
            let mut registry = Registry::new();
            let error = registry
                .register(definition(name))
                .expect_err("the name must be refused");

            assert!(
                error.contains("lowercase letters"),
                "the message must state the rule for `{name}`, got: {error}"
            );
            if !name.is_empty() {
                assert!(
                    error.contains(offending),
                    "the message must name `{offending}` in `{name}`, got: {error}"
                );
            }
        }
    }

    #[test]
    fn refuses_the_names_the_case_format_already_uses() {
        for name in ["expect", "save"] {
            let mut registry = Registry::new();
            let error = registry
                .register(definition(name))
                .expect_err("a reserved name must be refused");

            assert!(
                error.contains("reserved"),
                "the message must say the name is reserved, got: {error}"
            );
            assert!(error.contains(name), "got: {error}");
        }
    }

    #[test]
    fn observers_are_registered_and_looked_up_separately_from_steps() {
        let mut registry = Registry::new();
        // The two namespaces do not collide: one name can be a step and an
        // observer, because a case uses each in its own place.
        registry
            .register(definition("test.same"))
            .expect("the step must register");
        registry
            .register_observer(observer("test.same"))
            .expect("the observer must register");

        assert!(registry.get("test.same").is_some());
        assert!(registry.observer("test.same").is_some());
        assert!(registry.observer("test.other").is_none());
        assert_eq!(registry.names(), vec!["test.same"]);
        assert_eq!(registry.observer_names(), vec!["test.same"]);
    }

    #[test]
    fn refuses_a_duplicate_observer_name() {
        let mut registry = Registry::new();
        registry
            .register_observer(observer("test.watch"))
            .expect("the first registration must succeed");

        let error = registry
            .register_observer(observer("test.watch"))
            .expect_err("the duplicate must be refused");

        assert!(error.contains("test.watch"), "got: {error}");
        assert!(error.contains("observer"), "got: {error}");
    }

    #[test]
    fn an_observer_name_follows_the_step_name_rule() {
        for name in ["Demo.call", "demo/call", "demo call", ""] {
            let mut registry = Registry::new();
            let error = registry
                .register_observer(observer(name))
                .expect_err("the name must be refused");

            assert!(
                error.contains("lowercase letters"),
                "the message must state the rule for `{name}`, got: {error}"
            );
        }
    }
}
