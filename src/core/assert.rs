//! Domain-neutral expectations over a step's JSON result.

use jsonpath_rust::JsonPath;
use serde_json::Value;

use crate::core::case::{Expectation, Matcher, kind_name};

/// The outcome of one expectation.
#[derive(Debug, Clone, PartialEq)]
pub struct AssertionReport {
    /// JSON path that was evaluated.
    pub path: String,
    /// Comparison name, for example `equals`.
    pub comparison: String,
    /// Expected value.
    pub expected: Value,
    /// Selected value, or `None` when the path matched nothing.
    pub actual: Option<Value>,
    /// Whether the expectation passed.
    pub passed: bool,
    /// Extra detail when the comparison could not be made.
    pub message: Option<String>,
}

/// Evaluates one expectation against a step result.
pub fn evaluate(expectation: &Expectation, result: &Value) -> AssertionReport {
    let mut report = AssertionReport {
        path: expectation.path.clone(),
        comparison: expectation.matcher.comparison().to_string(),
        expected: expectation.matcher.expected(),
        actual: None,
        passed: false,
        message: None,
    };

    let selected = match result.query(&expectation.path) {
        Ok(values) => values,
        Err(error) => {
            report.message = Some(format!("the JSON path could not be evaluated: {error}"));
            return report;
        }
    };
    report.actual = selected.first().map(|value| (*value).clone());

    // Comparisons other than `exists` need exactly one selected value; a path
    // that matches nothing is reported instead of silently comparing null.
    let single = if selected.len() == 1 {
        Some(selected[0].clone())
    } else {
        None
    };

    match &expectation.matcher {
        Matcher::Exists(wanted) => {
            report.passed = if *wanted {
                !selected.is_empty()
            } else {
                selected.is_empty()
            };
        }
        Matcher::Equals(wanted) => match &single {
            Some(actual) => report.passed = actual == wanted,
            None => report.message = Some(match_count(selected.len())),
        },
        Matcher::NotEquals(wanted) => match &single {
            Some(actual) => report.passed = actual != wanted,
            None => report.message = Some(match_count(selected.len())),
        },
        Matcher::Contains(wanted) => match &single {
            Some(Value::String(actual)) => match wanted {
                Value::String(needle) => report.passed = actual.contains(needle.as_str()),
                other => {
                    report.message = Some(format!(
                        "`contains` needs a string to look for in a string, found {}",
                        kind_name(other)
                    ));
                }
            },
            Some(Value::Array(items)) => {
                report.passed = items.iter().any(|item| item == wanted);
            }
            Some(other) => {
                report.message = Some(format!(
                    "`contains` needs a string or a list, found {}",
                    kind_name(other)
                ));
            }
            None => report.message = Some(match_count(selected.len())),
        },
    }

    report
}

fn match_count(len: usize) -> String {
    if len == 0 {
        "the path matched no value".to_string()
    } else {
        format!("the path matched {len} values, expected one")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expectation(path: &str, matcher: Matcher) -> Expectation {
        Expectation {
            path: path.to_string(),
            matcher,
        }
    }

    fn result() -> Value {
        serde_json::json!({
            "status": 200,
            "body": {"name": "alice", "tags": ["admin", "beta"], "count": 2},
        })
    }

    #[test]
    fn equals_compares_the_selected_value() {
        let report = evaluate(
            &expectation("$.status", Matcher::Equals(Value::from(200))),
            &result(),
        );

        assert!(report.passed);
        assert_eq!(report.actual, Some(Value::from(200)));
        assert_eq!(report.comparison, "equals");
    }

    #[test]
    fn equals_fails_when_the_path_matches_nothing() {
        let report = evaluate(
            &expectation("$.body.missing", Matcher::Equals(Value::from(1))),
            &result(),
        );

        assert!(!report.passed);
        assert_eq!(report.actual, None);
        assert!(
            report
                .message
                .as_deref()
                .is_some_and(|message| message.contains("no value")),
            "got: {report:?}"
        );
    }

    #[test]
    fn not_equals_is_the_inverse_of_equals() {
        let report = evaluate(
            &expectation("$.status", Matcher::NotEquals(Value::from(500))),
            &result(),
        );

        assert!(report.passed);
    }

    #[test]
    fn contains_checks_substrings_and_list_members() {
        let substring = evaluate(
            &expectation("$.body.name", Matcher::Contains(Value::from("lic"))),
            &result(),
        );
        assert!(substring.passed);

        let member = evaluate(
            &expectation("$.body.tags", Matcher::Contains(Value::from("admin"))),
            &result(),
        );
        assert!(member.passed);

        let missing = evaluate(
            &expectation("$.body.tags", Matcher::Contains(Value::from("owner"))),
            &result(),
        );
        assert!(!missing.passed);
    }

    #[test]
    fn contains_reports_a_value_it_cannot_search() {
        let report = evaluate(
            &expectation("$.body.count", Matcher::Contains(Value::from(2))),
            &result(),
        );

        assert!(!report.passed);
        assert!(
            report
                .message
                .as_deref()
                .is_some_and(|message| message.contains("a number")),
            "got: {report:?}"
        );
    }

    #[test]
    fn exists_tests_presence_in_both_directions() {
        let present = evaluate(
            &expectation("$.body.name", Matcher::Exists(true)),
            &result(),
        );
        assert!(present.passed);

        let absent = evaluate(
            &expectation("$.body.missing", Matcher::Exists(false)),
            &result(),
        );
        assert!(absent.passed);

        let wrong = evaluate(
            &expectation("$.body.name", Matcher::Exists(false)),
            &result(),
        );
        assert!(!wrong.passed);
    }
}
