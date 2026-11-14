//! Tests for the per-case session contract: one resource, one case, one close.
//!
//! The drivers here exist only in this test target. They are registered like any
//! other plugin, through the public registry, and never reach the binary.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use serde_json::{Value, json};
use serde_saphyr::Location;
use test_anything::core::case::{Case, LoadedCase, Observation, Step};
use test_anything::core::events::EventBus;
use test_anything::core::filter::Filters;
use test_anything::core::observer::{Observer, ObserverDefinition, ObserverError};
use test_anything::core::registry::{Registry, StepDefinition, StepError};
use test_anything::core::runner;
use test_anything::core::secrets::Secrets;
use test_anything::core::session::{Session, Sessions};

/// The number of sessions the fake driver created in the current test.
static CREATED: AtomicUsize = AtomicUsize::new(0);

/// Serializes the tests in this file, because they share the fake driver's
/// counters.
static TEST_LOCK: Mutex<()> = Mutex::new(());

fn test_guard() -> MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn reset_created() {
    CREATED.store(0, Ordering::SeqCst);
}

fn created() -> usize {
    CREATED.load(Ordering::SeqCst)
}

/// A resource the fake driver keeps across the steps of one case.
struct FakeSession {
    id: usize,
}

impl Session for FakeSession {
    fn close(&mut self) -> Result<(), String> {
        Ok(())
    }
}

const KIND: &str = "test.session";

fn open(_args: &Value, sessions: &mut Sessions) -> Result<Value, StepError> {
    let session = sessions
        .get_or_create(KIND, || {
            let id = CREATED.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(FakeSession { id })
        })
        .map_err(StepError::Execution)?;
    Ok(json!({ "id": session.id }))
}

fn read(_args: &Value, sessions: &mut Sessions) -> Result<Value, StepError> {
    let session = sessions
        .get::<FakeSession>(KIND)
        .map_err(StepError::Execution)?
        .ok_or_else(|| StepError::Execution("no session was opened".to_string()))?;
    Ok(json!({ "id": session.id }))
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

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register(StepDefinition {
            name: "test.open",
            description: "Opens the fake session on first use.",
            args_schema: json!({"type": "object"}),
            validate: |_| Ok(()),
            run: open,
        })
        .expect("the opening step must register");
    registry
        .register(StepDefinition {
            name: "test.read",
            description: "Reads the fake session.",
            args_schema: json!({"type": "object"}),
            validate: |_| Ok(()),
            run: read,
        })
        .expect("the reading step must register");
    registry
        .register_observer(ObserverDefinition {
            name: "test.session",
            description: "Reports the session the step opened.",
            args_schema: json!({"type": "object"}),
            validate: |_| Ok(()),
            create: |_| Ok(Box::new(SessionObserver)),
        })
        .expect("the session observer must register");
    registry
}

fn step(action: &str) -> Step {
    Step {
        action: action.to_string(),
        args: json!({}),
        expect: Vec::new(),
        save: Vec::new(),
        location: Location::UNKNOWN,
        args_location: Location::UNKNOWN,
        arg_locations: Vec::new(),
    }
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

fn loaded(case: Case) -> Vec<LoadedCase> {
    vec![LoadedCase {
        path: "case.yaml".into(),
        case,
    }]
}

fn run_case(case: Case) -> test_anything::core::report::RunOutcome {
    runner::run(
        &loaded(case),
        &registry(),
        &EventBus::new(),
        None,
        &Filters::default(),
        &mut Secrets::new(),
        1,
    )
}

#[test]
fn steps_and_an_observer_share_one_session_per_case() {
    let _guard = test_guard();
    reset_created();
    let mut case = case("shared", vec![step("test.open"), step("test.read")]);
    case.observe = vec![observation("page", "test.session")];

    let outcome = run_case(case);

    assert!(outcome.is_ok(), "{outcome:?}");
    let test_anything::core::report::RunOutcome::Completed(report) = outcome else {
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
}
