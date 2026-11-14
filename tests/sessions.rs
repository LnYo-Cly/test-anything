//! Tests for the per-case session contract: one resource, one case, one close.
//!
//! The drivers here exist only in this test target. They are registered like any
//! other plugin, through the public registry, and never reach the binary.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use do_anything::core::bundle::FailureBundles;
use do_anything::core::case::{Case, Expectation, LoadedCase, Matcher, Observation, Step};
use do_anything::core::events::EventBus;
use do_anything::core::filter::Filters;
use do_anything::core::observer::{Observer, ObserverDefinition, ObserverError};
use do_anything::core::output::{self, Format};
use do_anything::core::registry::{Registry, StepDefinition, StepError};
use do_anything::core::report::{CaseStatus, ProblemKind, RunOutcome, StepStatus};
use do_anything::core::runner;
use do_anything::core::secrets::Secrets;
use do_anything::core::session::{Session, Sessions};
use do_anything::exit::ExitCode;
use serde_json::{Value, json};
use serde_saphyr::Location;

mod support;

use support::TempDir;

type StepFn = fn(&Value, &mut Sessions) -> Result<Value, StepError>;
type CreateFn = fn(&Value) -> Result<Box<dyn Observer>, ObserverError>;

/// The number of sessions the fake driver created in the current test.
static CREATED: AtomicUsize = AtomicUsize::new(0);

/// The number of sessions the fake driver tried to close in the current test.
static CLOSED: AtomicUsize = AtomicUsize::new(0);

/// Serializes the tests in this file, because they share the fake driver's
/// counters.
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn test_guard() -> MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn reset_counts() {
    CREATED.store(0, Ordering::SeqCst);
    CLOSED.store(0, Ordering::SeqCst);
}

fn created() -> usize {
    CREATED.load(Ordering::SeqCst)
}

fn closed() -> usize {
    CLOSED.load(Ordering::SeqCst)
}

/// A resource the fake driver keeps across the steps of one case.
struct FakeSession {
    id: usize,
    label: String,
}

impl Session for FakeSession {
    fn close(&mut self) -> Result<(), String> {
        CLOSED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// A session whose close always fails, so the report has something to carry.
struct FailingSession {
    message: String,
}

impl Session for FailingSession {
    fn close(&mut self) -> Result<(), String> {
        CLOSED.fetch_add(1, Ordering::SeqCst);
        Err(self.message.clone())
    }
}

const KIND: &str = "test.session";
const BROKEN_KIND: &str = "test.broken";

fn open_session(args: &Value, sessions: &mut Sessions) -> Result<usize, StepError> {
    let label = args
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let session = sessions
        .get_or_create(KIND, || {
            let id = CREATED.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(FakeSession { id, label })
        })
        .map_err(StepError::Execution)?;
    Ok(session.id)
}

fn open(args: &Value, sessions: &mut Sessions) -> Result<Value, StepError> {
    let id = open_session(args, sessions)?;
    Ok(json!({ "id": id }))
}

fn read(_args: &Value, sessions: &mut Sessions) -> Result<Value, StepError> {
    let session = sessions
        .get::<FakeSession>(KIND)
        .map_err(StepError::Execution)?
        .ok_or_else(|| StepError::Execution("no session was opened".to_string()))?;
    Ok(json!({ "id": session.id }))
}

fn verify(args: &Value, sessions: &mut Sessions) -> Result<Value, StepError> {
    let expected = args
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let session = sessions
        .get::<FakeSession>(KIND)
        .map_err(StepError::Execution)?
        .ok_or_else(|| StepError::Execution("no session was opened".to_string()))?;
    if session.label != expected {
        return Err(StepError::Execution(format!(
            "case `{expected}` reached session `{}`, which belongs to another case",
            session.label
        )));
    }
    Ok(json!({ "id": session.id }))
}

fn open_then_fail(args: &Value, sessions: &mut Sessions) -> Result<Value, StepError> {
    let id = open_session(args, sessions)?;
    let message = format!("the step after opening session {id} failed");
    match args.get("error").and_then(Value::as_str) {
        Some("timeout") => Err(StepError::Timeout(message)),
        Some("execution") => Err(StepError::Execution(message)),
        _ => Err(StepError::Environment(message)),
    }
}

fn open_broken(args: &Value, sessions: &mut Sessions) -> Result<Value, StepError> {
    let message = args
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("the resource could not be released")
        .to_string();
    sessions
        .get_or_create(BROKEN_KIND, || {
            CREATED.fetch_add(1, Ordering::SeqCst);
            Ok(FailingSession { message })
        })
        .map_err(StepError::Execution)?;
    Ok(json!({ "opened": true }))
}

fn pass(_args: &Value, _sessions: &mut Sessions) -> Result<Value, StepError> {
    Ok(json!({ "ok": true }))
}

fn fail(_args: &Value, _sessions: &mut Sessions) -> Result<Value, StepError> {
    Err(StepError::Environment(
        "the target refused the connection".to_string(),
    ))
}

fn timeout(_args: &Value, _sessions: &mut Sessions) -> Result<Value, StepError> {
    Err(StepError::Timeout("the step timed out".to_string()))
}

fn assert_false(_args: &Value, _sessions: &mut Sessions) -> Result<Value, StepError> {
    Ok(json!({ "ok": false }))
}

fn wait(args: &Value, _sessions: &mut Sessions) -> Result<Value, StepError> {
    let millis = args.get("ms").and_then(Value::as_u64).unwrap_or(0);
    std::thread::sleep(Duration::from_millis(millis));
    Ok(json!({ "waited": millis }))
}

/// An observer that reports the session the step opened.
struct SessionObserver;

impl Observer for SessionObserver {
    fn start(&mut self, _sessions: &mut Sessions) -> Result<(), ObserverError> {
        Ok(())
    }

    fn collect(&mut self, sessions: &mut Sessions) -> Result<Value, ObserverError> {
        let session = sessions
            .get::<FakeSession>(KIND)
            .map_err(ObserverError::Environment)?
            .ok_or_else(|| ObserverError::Environment("no session was opened".to_string()))?;
        Ok(json!({ "id": session.id }))
    }
}

/// An observer that fails when it starts the second step.
struct FailOnSecondStart {
    starts: usize,
}

impl Observer for FailOnSecondStart {
    fn start(&mut self, _sessions: &mut Sessions) -> Result<(), ObserverError> {
        self.starts += 1;
        if self.starts == 2 {
            return Err(ObserverError::Environment(
                "the observer could not start".to_string(),
            ));
        }
        Ok(())
    }

    fn collect(&mut self, _sessions: &mut Sessions) -> Result<Value, ObserverError> {
        Ok(Value::Null)
    }
}

/// An observer that cannot collect.
struct BrokenCollect;

impl Observer for BrokenCollect {
    fn start(&mut self, _sessions: &mut Sessions) -> Result<(), ObserverError> {
        Ok(())
    }

    fn collect(&mut self, _sessions: &mut Sessions) -> Result<Value, ObserverError> {
        Err(ObserverError::Environment(
            "the observer could not collect".to_string(),
        ))
    }
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    let steps: [(&str, &str, StepFn); 10] = [
        ("test.open", "Opens the fake session on first use.", open),
        ("test.read", "Reads the fake session.", read),
        (
            "test.verify",
            "Asserts the fake session belongs to this case.",
            verify,
        ),
        (
            "test.open_then_fail",
            "Opens the fake session, then fails.",
            open_then_fail,
        ),
        (
            "test.open_broken",
            "Opens a session whose close fails.",
            open_broken,
        ),
        ("test.pass", "Succeeds.", pass),
        ("test.fail", "Fails the environment.", fail),
        ("test.timeout", "Times out.", timeout),
        ("test.assert_false", "Returns false.", assert_false),
        ("test.wait", "Waits.", wait),
    ];
    for (name, description, run) in steps {
        registry
            .register(StepDefinition {
                name,
                description,
                args_schema: json!({"type": "object"}),
                validate: |_| Ok(()),
                run,
            })
            .unwrap_or_else(|error| panic!("`{name}` must register: {error}"));
    }
    let observers: [(&str, &str, CreateFn); 3] = [
        (
            "test.session",
            "Reports the session the step opened.",
            |_| Ok(Box::new(SessionObserver)),
        ),
        (
            "test.fail_start",
            "Fails when the second step starts.",
            |_| Ok(Box::new(FailOnSecondStart { starts: 0 })),
        ),
        ("test.fail_collect", "Cannot collect.", |_| {
            Ok(Box::new(BrokenCollect))
        }),
    ];
    for (name, description, create) in observers {
        registry
            .register_observer(ObserverDefinition {
                name,
                description,
                args_schema: json!({"type": "object"}),
                validate: |_| Ok(()),
                create,
            })
            .unwrap_or_else(|error| panic!("`{name}` must register: {error}"));
    }
    registry
}

fn step(action: &str) -> Step {
    step_with(action, json!({}))
}

fn step_with(action: &str, args: Value) -> Step {
    Step {
        action: action.to_string(),
        args,
        expect: Vec::new(),
        save: Vec::new(),
        location: Location::UNKNOWN,
        args_location: Location::UNKNOWN,
        arg_locations: Vec::new(),
    }
}

fn failing_step() -> Step {
    let mut step = step("test.assert_false");
    step.expect = vec![Expectation {
        path: "$.ok".to_string(),
        matcher: Matcher::Equals(json!(true)),
    }];
    step
}

fn open_broken_step(message: &str) -> Step {
    step_with("test.open_broken", json!({ "message": message }))
}

fn wait_step(millis: u64) -> Step {
    step_with("test.wait", json!({ "ms": millis }))
}

fn observation(name: &str, kind: &str) -> Observation {
    Observation {
        name: name.to_string(),
        kind: kind.to_string(),
        args: json!({}),
        location: Location::UNKNOWN,
        args_location: Location::UNKNOWN,
    }
}

fn case(name: &str, steps: Vec<Step>) -> Case {
    Case {
        name: name.to_string(),
        vars: BTreeMap::new(),
        project: None,
        suite: None,
        layer: None,
        tags: Vec::new(),
        observe: Vec::new(),
        steps,
    }
}

fn loaded(case: Case) -> LoadedCase {
    LoadedCase {
        path: PathBuf::from("case.yaml"),
        case,
    }
}

fn run_cases(cases: Vec<LoadedCase>, jobs: usize) -> RunOutcome {
    runner::run(
        &cases,
        &registry(),
        &EventBus::new(),
        None,
        &Filters::default(),
        &mut Secrets::new(),
        jobs,
    )
}

fn run_case(case: Case) -> RunOutcome {
    run_cases(vec![loaded(case)], 1)
}

#[test]
fn steps_and_an_observer_share_one_session_per_case() {
    let _guard = test_guard();
    reset_counts();
    let mut case = case("shared", vec![step("test.open"), step("test.read")]);
    case.observe = vec![observation("page", "test.session")];

    let outcome = run_case(case);

    assert!(outcome.is_ok(), "{outcome:?}");
    let RunOutcome::Completed(report) = outcome else {
        panic!("the run must complete");
    };
    let first = report.cases[0].steps[0]
        .result
        .as_ref()
        .expect("the opening step must keep its result");
    let second = report.cases[0].steps[1]
        .result
        .as_ref()
        .expect("the reading step must keep its result");
    let observed = &second["observed"]["page"];
    assert_eq!(first["id"], second["id"], "both steps must see one session");
    assert_eq!(
        first["id"], observed["id"],
        "the observer must see the step's session"
    );
    assert_eq!(created(), 1, "the session must be created only once");
    assert_eq!(closed(), 1, "the session must close exactly once");
}

/// One way a case can end after it has opened a session.
struct EndPath {
    name: &'static str,
    case: Case,
    status: CaseStatus,
    exit: ExitCode,
    problem: ProblemKind,
    /// The step that must be reported as skipped after the failure.
    skipped: Option<usize>,
}

#[test]
fn every_end_path_closes_the_session_exactly_once_and_reports_a_close_failure() {
    let _guard = test_guard();

    let mut observer_start = case(
        "observer start",
        vec![open_broken_step("close failed"), step("test.pass")],
    );
    observer_start.observe = vec![observation("watch", "test.fail_start")];
    let mut observer_collect = case(
        "observer collect",
        vec![open_broken_step("close failed"), step("test.pass")],
    );
    observer_collect.observe = vec![observation("watch", "test.fail_collect")];

    let paths = vec![
        EndPath {
            name: "every step passes",
            case: case(
                "pass",
                vec![open_broken_step("close failed"), step("test.pass")],
            ),
            status: CaseStatus::Error,
            exit: ExitCode::Environment,
            problem: ProblemKind::Environment,
            skipped: None,
        },
        EndPath {
            name: "an assertion fails",
            case: case(
                "assertion",
                vec![
                    open_broken_step("close failed"),
                    failing_step(),
                    step("test.pass"),
                ],
            ),
            status: CaseStatus::Failed,
            exit: ExitCode::TestFailure,
            problem: ProblemKind::Assertion,
            skipped: Some(2),
        },
        EndPath {
            name: "a step fails",
            case: case(
                "step error",
                vec![
                    open_broken_step("close failed"),
                    step("test.fail"),
                    step("test.pass"),
                ],
            ),
            status: CaseStatus::Error,
            exit: ExitCode::Environment,
            problem: ProblemKind::Environment,
            skipped: Some(2),
        },
        EndPath {
            name: "a step times out",
            case: case(
                "timeout",
                vec![
                    open_broken_step("close failed"),
                    step("test.timeout"),
                    step("test.pass"),
                ],
            ),
            status: CaseStatus::Error,
            exit: ExitCode::Timeout,
            problem: ProblemKind::Timeout,
            skipped: Some(2),
        },
        EndPath {
            name: "an observer fails to start",
            case: observer_start,
            status: CaseStatus::Error,
            exit: ExitCode::Environment,
            problem: ProblemKind::Environment,
            skipped: None,
        },
        EndPath {
            name: "an observer fails to collect",
            case: observer_collect,
            status: CaseStatus::Error,
            exit: ExitCode::Environment,
            problem: ProblemKind::Environment,
            skipped: Some(1),
        },
    ];

    for path in paths {
        reset_counts();
        let outcome = run_case(path.case);

        assert_eq!(
            created(),
            1,
            "{}: the session must open exactly once",
            path.name
        );
        assert_eq!(
            closed(),
            1,
            "{}: the session must close exactly once",
            path.name
        );
        assert_eq!(
            outcome.exit_code(),
            path.exit,
            "{}: the case's own result must decide the exit code",
            path.name
        );
        let RunOutcome::Completed(report) = outcome else {
            panic!("{}: the run must complete", path.name);
        };
        let case = &report.cases[0];
        assert_eq!(case.status, path.status, "{}", path.name);
        let problem = case
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("{}: the case must carry its problem", path.name));
        assert_eq!(problem.kind, path.problem, "{}", path.name);
        if let Some(index) = path.skipped {
            assert_eq!(
                case.steps[index].status,
                StepStatus::Skipped,
                "{}: step {} must be skipped after the failure",
                path.name,
                index + 1
            );
        }
        assert_eq!(
            case.close_errors.len(),
            1,
            "{}: the close failure must be reported, got {:?}",
            path.name,
            case.close_errors
        );
        assert!(
            case.close_errors[0].contains("test.broken"),
            "{}: {:?}",
            path.name,
            case.close_errors
        );
        assert!(
            case.close_errors[0].contains("close failed"),
            "{}: {:?}",
            path.name,
            case.close_errors
        );
    }
}

#[test]
fn a_close_error_is_redacted_in_text_json_and_the_failure_bundle() {
    let _guard = test_guard();
    reset_counts();
    let dir = TempDir::new("close-error");
    let source = dir.write(
        "case.yaml",
        "name: close error\nsteps:\n  - test.open_broken:\n      message: s3cr3t-value\n",
    );
    let mut case = case(
        "close error",
        vec![open_broken_step(
            "the connection to s3cr3t-value was already gone",
        )],
    );
    case.name = "close error".to_string();
    let mut bundles = FailureBundles::new(dir.path().join("failures"));
    let mut secrets = Secrets::new();
    secrets.record("s3cr3t-value");

    let outcome = runner::run(
        &[LoadedCase { path: source, case }],
        &registry(),
        &EventBus::new(),
        Some(&mut bundles),
        &Filters::default(),
        &mut secrets,
        1,
    );

    assert_eq!(outcome.exit_code(), ExitCode::Environment, "{outcome:?}");
    let RunOutcome::Completed(report) = &outcome else {
        panic!("the run must complete");
    };
    let case = &report.cases[0];
    assert_eq!(case.status, CaseStatus::Error);
    assert_eq!(
        case.error.as_ref().map(|problem| problem.kind),
        Some(ProblemKind::Environment)
    );
    assert_eq!(case.close_errors.len(), 1);

    let text = output::render(&outcome, Format::Text, &secrets);
    assert!(
        !text.stdout.contains("s3cr3t-value"),
        "text output leaked: {}",
        text.stdout
    );
    assert!(text.stdout.contains("***"), "got: {}", text.stdout);
    assert!(text.stdout.contains("close:"), "got: {}", text.stdout);

    let json = output::render(&outcome, Format::Json, &secrets);
    assert!(
        !json.stdout.contains("s3cr3t-value"),
        "JSON output leaked: {}",
        json.stdout
    );
    let value: Value = serde_json::from_str(&json.stdout).expect("stdout must be JSON");
    assert_eq!(value["cases"][0]["status"], "error");
    assert_eq!(value["cases"][0]["error"]["kind"], "environment");
    let close_errors = value["cases"][0]["close_errors"]
        .as_array()
        .expect("the JSON report must carry the close errors");
    assert_eq!(close_errors.len(), 1);
    assert!(
        close_errors[0]
            .as_str()
            .is_some_and(|message| message.contains("***") && !message.contains("s3cr3t-value")),
        "got: {close_errors:?}"
    );

    let bundle = case
        .bundle
        .as_ref()
        .expect("the failed case must get a bundle");
    let report_text = std::fs::read_to_string(bundle.join("report.json"))
        .expect("the bundle report must be readable");
    assert!(
        !report_text.contains("s3cr3t-value"),
        "the bundle leaked: {report_text}"
    );
    assert!(
        report_text.contains("***"),
        "the bundle must show the marker"
    );
}

#[test]
fn sessions_are_not_shared_between_cases_under_jobs_4() {
    let _guard = test_guard();
    reset_counts();
    let cases: Vec<LoadedCase> = ["a", "b", "c", "d"]
        .into_iter()
        .map(|label| {
            let mut open = step("test.open");
            open.args = json!({ "label": label });
            let mut verify = step("test.verify");
            verify.args = json!({ "label": label });
            loaded(case(
                &format!("case {label}"),
                vec![open, wait_step(100), verify],
            ))
        })
        .collect();

    let outcome = run_cases(cases, 4);

    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(created(), 4, "each case must create its own session");
    assert_eq!(closed(), 4, "each case must close its own session");
    let RunOutcome::Completed(report) = outcome else {
        panic!("the run must complete");
    };
    let ids: std::collections::BTreeSet<u64> = report
        .cases
        .iter()
        .filter_map(|case| case.steps[2].result.as_ref()?.get("id")?.as_u64())
        .collect();
    assert_eq!(ids.len(), 4, "the four sessions must be distinct: {ids:?}");
}
