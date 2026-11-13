//! Executes loaded cases against a registry.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use jsonpath_rust::JsonPath;
use serde_json::Value;

use crate::core::assert::{self, AssertionReport};
use crate::core::case::{self, CaseError, LoadedCase, SavedValue, Step};
use crate::core::events::{Event, EventBus};
use crate::core::registry::{Registry, StepError};
use crate::core::report::{
    CaseReport, CaseStatus, Problem, ProblemKind, RunOutcome, RunReport, StepReport, StepStatus,
};

/// Validates every case, then runs them in the order given.
///
/// Validation covers step registration, argument schemas and variable
/// references. If anything is invalid no case runs at all, because a partially
/// executed run would make the result depend on case order.
pub fn run(cases: &[LoadedCase], registry: &Registry, bus: &EventBus) -> RunOutcome {
    let errors = preflight(cases, registry);
    if !errors.is_empty() {
        return RunOutcome::Rejected { errors };
    }

    let started = Instant::now();
    bus.emit(Event::RunStarted { cases: cases.len() });

    let mut reports = Vec::with_capacity(cases.len());
    for (index, loaded) in cases.iter().enumerate() {
        bus.emit(Event::CaseStarted {
            case: index,
            name: loaded.case.name.clone(),
        });
        let report = run_case(index, loaded, registry, bus);
        bus.emit(Event::CaseFinished {
            case: index,
            name: report.name.clone(),
            status: report.status,
            duration: report.duration,
        });
        reports.push(report);
    }

    let report = RunReport {
        cases: reports,
        duration: started.elapsed(),
    };
    bus.emit(Event::RunFinished {
        summary: report.summary(),
        duration: report.duration,
    });
    RunOutcome::Completed(report)
}

/// Checks everything that must hold before any case runs.
fn preflight(cases: &[LoadedCase], registry: &Registry) -> Vec<CaseError> {
    let names = registry.names();
    let hint = if names.is_empty() {
        "no steps are registered; build with a driver feature enabled".to_string()
    } else {
        format!(
            "registered steps: {}",
            names
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut errors = Vec::new();

    for loaded in cases {
        let mut vars = loaded.case.vars.clone();
        for (index, step) in loaded.case.steps.iter().enumerate() {
            let position = index + 1;
            match case::interpolate(&step.args, &vars) {
                Err(message) => errors.push(CaseError {
                    file: loaded.path.clone(),
                    message: format!("step {position}: {message}"),
                }),
                Ok(args) => match registry.get(&step.action) {
                    None => errors.push(CaseError {
                        file: loaded.path.clone(),
                        message: format!(
                            "step {position}: `{}` is not a registered step; {hint}",
                            step.action
                        ),
                    }),
                    Some(definition) => {
                        if let Err(message) = (definition.validate)(&args) {
                            errors.push(CaseError {
                                file: loaded.path.clone(),
                                message: format!("step {position}: `{}` {message}", step.action),
                            });
                        }
                    }
                },
            }
            for saved in &step.save {
                vars.insert(saved.name.clone(), format!("${{{}}}", saved.name));
            }
        }
    }

    errors
}

fn run_case(index: usize, loaded: &LoadedCase, registry: &Registry, bus: &EventBus) -> CaseReport {
    let started = Instant::now();
    let mut vars = loaded.case.vars.clone();
    let mut steps = Vec::with_capacity(loaded.case.steps.len());
    let mut stopped: Option<Problem> = None;

    for (position, step) in loaded.case.steps.iter().enumerate() {
        if stopped.is_some() {
            steps.push(StepReport {
                name: step.action.clone(),
                status: StepStatus::Skipped,
                duration: Duration::ZERO,
                result: None,
                assertions: Vec::new(),
                error: None,
            });
            continue;
        }

        bus.emit(Event::StepStarted {
            case: index,
            step: position,
            name: step.action.clone(),
        });
        let step_started = Instant::now();
        let mut report = StepReport {
            name: step.action.clone(),
            status: StepStatus::Passed,
            duration: Duration::ZERO,
            result: None,
            assertions: Vec::new(),
            error: None,
        };

        match execute_step(step, registry, &mut vars) {
            Ok((result, assertions)) => {
                let failed = assertions.iter().filter(|item| !item.passed).count();
                if failed == 0 {
                    report.status = StepStatus::Passed;
                } else {
                    report.status = StepStatus::Failed;
                    report.error = Some(Problem {
                        kind: ProblemKind::Assertion,
                        message: format!("{failed} of {} assertions failed", assertions.len()),
                    });
                }
                report.result = Some(result);
                report.assertions = assertions;
            }
            Err(problem) => {
                report.status = StepStatus::Error;
                report.error = Some(problem);
            }
        }

        report.duration = step_started.elapsed();
        bus.emit(Event::StepFinished {
            case: index,
            step: position,
            name: report.name.clone(),
            status: report.status,
            duration: report.duration,
        });
        if let Some(problem) = &report.error {
            stopped = Some(problem.clone());
        }
        steps.push(report);
    }

    let status = match stopped.as_ref().map(|problem| problem.kind) {
        None => CaseStatus::Passed,
        Some(ProblemKind::Assertion) => CaseStatus::Failed,
        Some(_) => CaseStatus::Error,
    };

    CaseReport {
        file: loaded.path.clone(),
        name: loaded.case.name.clone(),
        status,
        duration: started.elapsed(),
        steps,
        error: stopped,
    }
}

fn execute_step(
    step: &Step,
    registry: &Registry,
    vars: &mut BTreeMap<String, String>,
) -> Result<(Value, Vec<AssertionReport>), Problem> {
    let definition = registry.get(&step.action).ok_or_else(|| Problem {
        kind: ProblemKind::InvalidInput,
        message: format!("`{}` is not a registered step", step.action),
    })?;

    let args = case::interpolate(&step.args, vars).map_err(|message| Problem {
        kind: ProblemKind::Execution,
        message,
    })?;
    let result = (definition.run)(&args).map_err(problem_from_step_error)?;

    let assertions: Vec<AssertionReport> = step
        .expect
        .iter()
        .map(|expectation| assert::evaluate(expectation, &result))
        .collect();

    // A case that already failed an assertion is not allowed to change the
    // variables that later steps would see.
    if assertions.iter().all(|item| item.passed) {
        save_values(&step.save, &result, vars)?;
    }

    Ok((result, assertions))
}

fn save_values(
    saves: &[SavedValue],
    result: &Value,
    vars: &mut BTreeMap<String, String>,
) -> Result<(), Problem> {
    for saved in saves {
        let selected = result.query(&saved.path).map_err(|error| Problem {
            kind: ProblemKind::Execution,
            message: format!("could not save `{}`: {error}", saved.name),
        })?;
        match selected.as_slice() {
            [value] => {
                vars.insert(saved.name.clone(), variable_text(value));
            }
            [] => {
                return Err(Problem {
                    kind: ProblemKind::Execution,
                    message: format!(
                        "could not save `{}`: `{}` matched no value",
                        saved.name, saved.path
                    ),
                });
            }
            values => {
                return Err(Problem {
                    kind: ProblemKind::Execution,
                    message: format!(
                        "could not save `{}`: `{}` matched {} values, expected one",
                        saved.name,
                        saved.path,
                        values.len()
                    ),
                });
            }
        }
    }
    Ok(())
}

fn variable_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn problem_from_step_error(error: StepError) -> Problem {
    match error {
        StepError::InvalidArguments(message) => Problem {
            kind: ProblemKind::InvalidInput,
            message,
        },
        StepError::Environment(message) => Problem {
            kind: ProblemKind::Environment,
            message,
        },
        StepError::Timeout(message) => Problem {
            kind: ProblemKind::Timeout,
            message,
        },
        StepError::Execution(message) => Problem {
            kind: ProblemKind::Execution,
            message,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::case::Case;
    use crate::core::registry::StepDefinition;
    use serde_json::json;

    fn test_case(name: &str, steps: Vec<Step>) -> Case {
        Case {
            name: name.to_string(),
            vars: BTreeMap::new(),
            steps,
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry
            .register(StepDefinition {
                name: "test.echo",
                description: "Returns the `value` argument.",
                args_schema: json!({"type": "object"}),
                validate: |args: &Value| {
                    if args.get("value").is_some() {
                        Ok(())
                    } else {
                        Err("invalid arguments: `value` is required".to_string())
                    }
                },
                run: |args: &Value| Ok(args.get("value").cloned().unwrap_or(Value::Null)),
            })
            .expect("the test step must register");
        registry
            .register(StepDefinition {
                name: "test.fail",
                description: "Fails the environment.",
                args_schema: json!({"type": "object"}),
                validate: |_| Ok(()),
                run: |_| {
                    Err(StepError::Environment(
                        "the target refused the connection".to_string(),
                    ))
                },
            })
            .expect("the failing test step must register");
        registry
    }

    fn echo(value: Value, expect: Vec<crate::core::case::Expectation>) -> Step {
        Step {
            action: "test.echo".to_string(),
            args: json!({ "value": value }),
            expect,
            save: Vec::new(),
        }
    }

    fn loaded(steps: Vec<Step>) -> Vec<LoadedCase> {
        vec![LoadedCase {
            path: "case.yaml".into(),
            case: test_case("case", steps),
        }]
    }

    #[test]
    fn a_passing_case_returns_success() {
        let bus = EventBus::new();
        let outcome = run(&loaded(vec![echo(json!(1), Vec::new())]), &registry(), &bus);

        assert_eq!(outcome.exit_code(), crate::exit::ExitCode::Success);
        assert!(outcome.is_ok());
    }

    #[test]
    fn a_failed_assertion_stops_the_case_and_skips_the_rest() {
        let bus = EventBus::new();
        let mut failing = echo(json!("wrong"), Vec::new());
        failing.expect = vec![crate::core::case::Expectation {
            path: "$".to_string(),
            matcher: crate::core::case::Matcher::Equals(json!("right")),
        }];
        let outcome = run(
            &loaded(vec![failing, echo(json!(1), Vec::new())]),
            &registry(),
            &bus,
        );

        assert_eq!(outcome.exit_code(), crate::exit::ExitCode::TestFailure);
        let RunOutcome::Completed(report) = outcome else {
            panic!("the run must complete");
        };
        assert_eq!(report.cases[0].status, CaseStatus::Failed);
        assert_eq!(report.cases[0].steps[0].status, StepStatus::Failed);
        assert_eq!(report.cases[0].steps[1].status, StepStatus::Skipped);
    }

    #[test]
    fn an_environment_error_wins_over_a_failed_assertion() {
        let bus = EventBus::new();
        let mut failing = echo(json!("wrong"), Vec::new());
        failing.expect = vec![crate::core::case::Expectation {
            path: "$".to_string(),
            matcher: crate::core::case::Matcher::Equals(json!("right")),
        }];
        let broken = Step {
            action: "test.fail".to_string(),
            args: json!({}),
            expect: Vec::new(),
            save: Vec::new(),
        };
        let mut cases = loaded(vec![failing]);
        cases.push(LoadedCase {
            path: "other.yaml".into(),
            case: test_case("other", vec![broken]),
        });

        let outcome = run(&cases, &registry(), &bus);

        assert_eq!(outcome.exit_code(), crate::exit::ExitCode::Environment);
    }

    #[test]
    fn invalid_arguments_reject_the_run_before_any_step_runs() {
        let mut bad = echo(json!(1), Vec::new());
        bad.args = json!({});
        let mut cases = loaded(vec![bad]);
        cases.push(LoadedCase {
            path: "other.yaml".into(),
            case: test_case("other", vec![echo(json!(1), Vec::new())]),
        });
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&seen);
        let mut bus = EventBus::new();
        bus.subscribe(move |event| {
            recorder
                .lock()
                .expect("the recording mutex is poisoned")
                .push(format!("{event:?}"));
        });

        let outcome = run(&cases, &registry(), &bus);

        assert_eq!(outcome.exit_code(), crate::exit::ExitCode::InvalidCase);
        let RunOutcome::Rejected { errors } = outcome else {
            panic!("the run must be rejected");
        };
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].message.contains("value"),
            "got: {}",
            errors[0].message
        );
        assert!(
            seen.lock()
                .expect("the recording mutex is poisoned")
                .is_empty(),
            "no event may be emitted when the run is rejected"
        );
    }

    #[test]
    fn listeners_see_every_event_in_run_order() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&seen);
        let mut bus = EventBus::new();
        bus.subscribe(move |event| {
            let label = match event {
                Event::RunStarted { .. } => "run started".to_string(),
                Event::CaseStarted { .. } => "case started".to_string(),
                Event::StepStarted { name, .. } => format!("step started {name}"),
                Event::StepFinished { name, .. } => format!("step finished {name}"),
                Event::CaseFinished { .. } => "case finished".to_string(),
                Event::RunFinished { .. } => "run finished".to_string(),
            };
            recorder
                .lock()
                .expect("the recording mutex is poisoned")
                .push(label);
        });

        let outcome = run(
            &loaded(vec![echo(json!(1), Vec::new()), echo(json!(2), Vec::new())]),
            &registry(),
            &bus,
        );

        assert!(outcome.is_ok());
        assert_eq!(
            *seen.lock().expect("the recording mutex is poisoned"),
            vec![
                "run started",
                "case started",
                "step started test.echo",
                "step finished test.echo",
                "step started test.echo",
                "step finished test.echo",
                "case finished",
                "run finished",
            ]
        );
    }

    #[test]
    fn saved_values_feed_later_steps_and_failures_are_reported() {
        let bus = EventBus::new();
        let mut first = echo(json!({"token": "abc"}), Vec::new());
        first.save = vec![SavedValue {
            name: "token".to_string(),
            path: "$.token".to_string(),
        }];
        let second = Step {
            action: "test.echo".to_string(),
            args: json!({ "value": "${token}" }),
            expect: vec![crate::core::case::Expectation {
                path: "$".to_string(),
                matcher: crate::core::case::Matcher::Equals(json!("abc")),
            }],
            save: Vec::new(),
        };

        let outcome = run(&loaded(vec![first, second]), &registry(), &bus);

        assert!(outcome.is_ok(), "{outcome:?}");
    }
}
