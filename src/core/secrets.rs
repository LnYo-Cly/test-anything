//! The values read from the environment during a run, and their redaction.
//!
//! A value is a secret because of where it came from — a `${env:NAME}`
//! reference — not because the case file declared it one. The set is filled
//! while the cases are validated, which happens before the first case runs, so
//! every output written after that point can be redacted with the whole set.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// What replaces a secret in output and in a failure bundle.
pub const MARKER: &str = "***";

/// The secret values of one run.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    values: BTreeSet<String>,
}

impl Secrets {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one resolved environment value.
    ///
    /// An empty value is not a secret: replacing every empty string would
    /// rewrite the whole output.
    pub fn record(&mut self, value: &str) {
        if !value.is_empty() {
            self.values.insert(value.to_string());
        }
    }

    /// Whether nothing has been read from the environment yet.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Replaces every occurrence of every secret with [`MARKER`].
    pub fn redact(&self, text: &str) -> String {
        if self.values.is_empty() {
            return text.to_string();
        }
        // Longest first, so a secret that contains another one is replaced as a
        // whole instead of leaving a fragment behind.
        let mut ordered: Vec<&str> = self.values.iter().map(String::as_str).collect();
        ordered.sort_by_key(|value| std::cmp::Reverse(value.len()));
        let mut redacted = text.to_string();
        for value in ordered {
            redacted = redacted.replace(value, MARKER);
        }
        redacted
    }

    /// Replaces every secret inside a JSON value.
    ///
    /// It runs on the tree rather than on the serialized text: a secret that
    /// contains `"` or `\` is escaped in the text, so a textual replacement
    /// would miss it. Numbers are covered too, because a service can echo a
    /// numeric token back as a number.
    pub fn redact_json(&self, value: &Value) -> Value {
        if self.values.is_empty() {
            return value.clone();
        }
        match value {
            Value::String(text) => Value::String(self.redact(text)),
            Value::Number(number) if self.values.contains(&number.to_string()) => {
                Value::String(MARKER.to_string())
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.redact_json(item)).collect())
            }
            Value::Object(entries) => {
                let mut redacted = Map::with_capacity(entries.len());
                for (key, item) in entries {
                    redacted.insert(self.redact(key), self.redact_json(item));
                }
                Value::Object(redacted)
            }
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_set_leaves_text_and_json_alone() {
        let secrets = Secrets::new();

        assert!(secrets.is_empty());
        assert_eq!(secrets.redact("nothing to hide"), "nothing to hide");
        assert_eq!(
            secrets.redact_json(&json!({"a": ["b", 1]})),
            json!({"a": ["b", 1]})
        );
    }

    #[test]
    fn every_occurrence_of_every_secret_is_replaced() {
        let mut secrets = Secrets::new();
        secrets.record("s3cr3t");
        secrets.record("token-1");

        assert_eq!(
            secrets.redact("a s3cr3t and token-1 and s3cr3t again"),
            "a *** and *** and *** again"
        );
    }

    #[test]
    fn an_empty_value_is_not_recorded() {
        let mut secrets = Secrets::new();
        secrets.record("");

        assert!(secrets.is_empty());
        assert_eq!(secrets.redact("unchanged"), "unchanged");
    }

    #[test]
    fn a_longer_secret_wins_over_a_shorter_one_it_contains() {
        let mut secrets = Secrets::new();
        secrets.record("abc");
        secrets.record("abcdef");

        assert_eq!(secrets.redact("abcdef"), "***");
    }

    #[test]
    fn json_strings_keys_and_numbers_are_redacted() {
        let mut secrets = Secrets::new();
        secrets.record("s3cr3t");
        secrets.record("4242");
        let value = json!({
            "s3cr3t": "a s3cr3t value",
            "token": "s3cr3t",
            "count": 4242,
            "list": ["s3cr3t", 1, true, null],
        });

        assert_eq!(
            secrets.redact_json(&value),
            json!({
                "***": "a *** value",
                "token": "***",
                "count": "***",
                "list": ["***", 1, true, null],
            })
        );
    }

    #[test]
    fn a_secret_that_needs_json_escaping_is_still_replaced() {
        let mut secrets = Secrets::new();
        secrets.record("a\"b\\c");
        let value = json!({"echo": "a\"b\\c"});

        let redacted = secrets.redact_json(&value);
        assert_eq!(redacted, json!({"echo": "***"}));
        assert!(
            !serde_json::to_string(&redacted)
                .expect("the redacted value must serialize")
                .contains("a\\\"b")
        );
    }
}
