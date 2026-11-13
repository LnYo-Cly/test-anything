//! Renders run outcomes for people and for machines.
//!
//! This is the only place the JSON report is built, and the only place result
//! text is formatted, so the two views cannot drift apart.

use serde_json::{Map, Value, json};

use crate::core::assert::AssertionReport;
use crate::core::report::{CaseReport, CaseStatus, Problem, RunOutcome, StepReport, Summary};

/// How results are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One line per case, assertion details on failure, then a summary line.
    Text,
    /// Exactly one JSON object.
    Json,
}

/// The text `ta` writes to standard output and standard error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// Results.
    pub stdout: String,
    /// Diagnostics.
    pub stderr: String,
}

/// Renders an outcome in the requested format.
pub fn render(outcome: &RunOutcome, format: Format) -> Rendered {
    match format {
        Format::Json => Rendered {
            stdout: format!("{}\n", json_report(outcome)),
            stderr: String::new(),
        },
        Format::Text => text_report(outcome),
    }
}

fn text_report(outcome: &RunOutcome) -> Rendered {
    let RunOutcome::Completed(report) = outcome else {
        let RunOutcome::Rejected { errors } = outcome else {
            return Rendered::default();
        };
        let mut stderr = String::new();
        for error in errors {
            stderr.push_str(&format!(
                "error: {}: {}\n",
                error.file.display(),
                error.message
            ));
        }
        return Rendered {
            stdout: String::new(),
            stderr,
        };
    };

    let mut stdout = String::new();
    for case in &report.cases {
        stdout.push_str(&format!(
            "{} {} [{}] {} ms\n",
            label(case.status),
            case.file.display(),
            case.name,
            millis(case.duration)
        ));
        for step in &case.steps {
            if let Some(problem) = &step.error {
                stdout.push_str(&format!("  {}: {}\n", step.name, problem.message));
            }
            for assertion in &step.assertions {
                if !assertion.passed {
                    stdout.push_str(&format!("    {}\n", describe(assertion)));
                }
            }
        }
    }

    let summary = report.summary();
    stdout.push_str(&format!(
        "Summary: {} {} ({} passed, {} failed, {} errors), {} {} ({} passed, {} failed, {} errors, {} skipped), {} ms\n",
        summary.cases,
        noun(summary.cases, "case", "cases"),
        summary.cases_passed,
        summary.cases_failed,
        summary.cases_errored,
        summary.steps,
        noun(summary.steps, "step", "steps"),
        summary.steps_passed,
        summary.steps_failed,
        summary.steps_errored,
        summary.steps_skipped,
        millis(report.duration),
    ));

    Rendered {
        stdout,
        stderr: String::new(),
    }
}

/// Picks the singular or plural form for a count.
fn noun(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

fn label(status: CaseStatus) -> &'static str {
    match status {
        CaseStatus::Passed => "PASS",
        CaseStatus::Failed => "FAIL",
        CaseStatus::Error => "ERROR",
    }
}

fn describe(assertion: &AssertionReport) -> String {
    let actual = assertion
        .actual
        .as_ref()
        .map_or_else(|| "<missing>".to_string(), compact);
    let mut line = format!(
        "{} {} {} (actual {})",
        assertion.path,
        assertion.comparison,
        compact(&assertion.expected),
        actual
    );
    if let Some(message) = &assertion.message {
        line.push_str(": ");
        line.push_str(message);
    }
    line
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unprintable>".to_string())
}

fn json_report(outcome: &RunOutcome) -> Value {
    let mut object = Map::new();
    object.insert("ok".to_string(), Value::Bool(outcome.is_ok()));

    let (summary, duration) = match outcome {
        RunOutcome::Completed(report) => (report.summary(), report.duration),
        RunOutcome::Rejected { .. } => (Summary::default(), std::time::Duration::ZERO),
    };
    object.insert("summary".to_string(), summary_value(&summary, duration));

    match outcome {
        RunOutcome::Completed(report) => {
            object.insert(
                "cases".to_string(),
                Value::Array(report.cases.iter().map(case_value).collect()),
            );
            object.insert("errors".to_string(), Value::Array(Vec::new()));
        }
        RunOutcome::Rejected { errors } => {
            object.insert("cases".to_string(), Value::Array(Vec::new()));
            object.insert(
                "errors".to_string(),
                Value::Array(
                    errors
                        .iter()
                        .map(|error| {
                            json!({
                                "file": error.file.display().to_string(),
                                "message": error.message,
                                "kind": "invalid_input",
                            })
                        })
                        .collect(),
                ),
            );
        }
    }

    Value::Object(object)
}

fn summary_value(summary: &Summary, duration: std::time::Duration) -> Value {
    json!({
        "cases": summary.cases,
        "cases_passed": summary.cases_passed,
        "cases_failed": summary.cases_failed,
        "cases_errored": summary.cases_errored,
        "steps": summary.steps,
        "steps_passed": summary.steps_passed,
        "steps_failed": summary.steps_failed,
        "steps_errored": summary.steps_errored,
        "steps_skipped": summary.steps_skipped,
        "duration_ms": millis(duration),
    })
}

fn case_value(case: &CaseReport) -> Value {
    json!({
        "file": case.file.display().to_string(),
        "name": case.name,
        "status": case.status.as_str(),
        "duration_ms": millis(case.duration),
        "steps": case.steps.iter().map(step_value).collect::<Vec<Value>>(),
        "error": case.error.as_ref().map(problem_value),
    })
}

fn step_value(step: &StepReport) -> Value {
    json!({
        "name": step.name,
        "status": step.status.as_str(),
        "duration_ms": millis(step.duration),
        "result": step.result,
        "assertions": step.assertions.iter().map(assertion_value).collect::<Vec<Value>>(),
        "error": step.error.as_ref().map(problem_value),
    })
}

fn assertion_value(assertion: &AssertionReport) -> Value {
    json!({
        "path": assertion.path,
        "comparison": assertion.comparison,
        "expected": assertion.expected,
        "actual": assertion.actual,
        "passed": assertion.passed,
        "message": assertion.message,
    })
}

fn problem_value(problem: &Problem) -> Value {
    json!({
        "kind": problem.kind.as_str(),
        "message": problem.message,
    })
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::core::report::{ProblemKind, StepStatus};

    fn failing_outcome() -> RunOutcome {
        RunOutcome::Completed(crate::core::report::RunReport {
            cases: vec![CaseReport {
                file: PathBuf::from("cases/login.yaml"),
                name: "Login".to_string(),
                status: CaseStatus::Failed,
                duration: Duration::from_millis(3),
                steps: vec![StepReport {
                    name: "sample.call".to_string(),
                    status: StepStatus::Failed,
                    duration: Duration::from_millis(2),
                    result: Some(json!({"status": 200})),
                    assertions: vec![AssertionReport {
                        path: "$.status".to_string(),
                        comparison: "equals".to_string(),
                        expected: json!(201),
                        actual: Some(json!(200)),
                        passed: false,
                        message: None,
                    }],
                    error: Some(Problem {
                        kind: ProblemKind::Assertion,
                        message: "1 of 1 assertions failed".to_string(),
                    }),
                }],
                error: Some(Problem {
                    kind: ProblemKind::Assertion,
                    message: "1 of 1 assertions failed".to_string(),
                }),
            }],
            duration: Duration::from_millis(4),
        })
    }

    #[test]
    fn text_lists_the_path_the_expected_value_and_the_actual_value() {
        let rendered = render(&failing_outcome(), Format::Text);

        assert!(rendered.stderr.is_empty());
        assert!(
            rendered
                .stdout
                .contains("FAIL cases/login.yaml [Login] 3 ms")
        );
        assert!(
            rendered.stdout.contains("$.status equals 201 (actual 200)"),
            "got: {}",
            rendered.stdout
        );
        assert!(rendered.stdout.contains("Summary: 1 case"));
    }

    #[test]
    fn json_is_one_object_with_the_documented_fields() {
        let rendered = render(&failing_outcome(), Format::Json);
        let value: Value = serde_json::from_str(&rendered.stdout).expect("stdout must be JSON");

        assert_eq!(value["ok"], false);
        assert_eq!(value["summary"]["cases"], 1);
        assert_eq!(value["summary"]["cases_failed"], 1);
        assert_eq!(value["summary"]["steps_failed"], 1);
        assert_eq!(value["cases"][0]["duration_ms"], 3);
        assert_eq!(value["cases"][0]["steps"][0]["status"], "failed");
        assert_eq!(
            value["cases"][0]["steps"][0]["assertions"][0]["passed"],
            false
        );
        assert_eq!(
            value["cases"][0]["steps"][0]["assertions"][0]["actual"],
            200
        );
        assert_eq!(value["cases"][0]["steps"][0]["error"]["kind"], "assertion");
        assert_eq!(value["errors"], json!([]));
    }

    #[test]
    fn rejected_input_is_reported_on_stderr_and_as_json_errors() {
        let outcome = RunOutcome::Rejected {
            errors: vec![crate::core::case::CaseError {
                file: PathBuf::from("broken.yaml"),
                message: "line 2: unknown field".to_string(),
            }],
        };

        let text = render(&outcome, Format::Text);
        assert!(text.stdout.is_empty());
        assert!(text.stderr.contains("broken.yaml"), "got: {}", text.stderr);

        let json = render(&outcome, Format::Json);
        assert!(json.stderr.is_empty());
        let value: Value = serde_json::from_str(&json.stdout).expect("stdout must be JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["cases"], json!([]));
        assert_eq!(value["errors"][0]["file"], "broken.yaml");
    }
}
