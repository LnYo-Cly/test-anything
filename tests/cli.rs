//! End-to-end tests that run the compiled `ta` binary.

use std::process::{Command, Output};

use serde_json::Value;

fn run_ta(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ta"))
        .args(args)
        .output()
        .expect("failed to run the ta binary")
}

fn exit_code(output: &Output) -> Option<i32> {
    output.status.code()
}

#[test]
fn version_prints_the_package_version() {
    let output = run_ta(&["--version"]);

    assert_eq!(exit_code(&output), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected the version in stdout, got: {stdout:?}"
    );
}

#[test]
fn run_exits_with_the_not_implemented_code() {
    let output = run_ta(&["run", "some.yaml"]);

    assert_eq!(exit_code(&output), Some(9));
    assert!(
        output.stdout.is_empty(),
        "standard output must stay clean in text mode, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not implemented"),
        "expected a human-readable message on stderr, got: {stderr:?}"
    );
}

#[test]
fn json_output_is_a_single_error_object() {
    let output = run_ta(&["--output", "json", "run", "some.yaml"]);

    assert_eq!(exit_code(&output), Some(9));
    let value: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must contain one JSON value");

    assert!(value.is_object(), "expected a JSON object, got: {value}");
    assert_eq!(value["ok"], Value::Bool(false));
    assert_eq!(value["error"]["code"], "not_implemented");
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "expected a non-empty error message, got: {value}"
    );
}

#[test]
fn unknown_subcommand_is_a_usage_error() {
    let output = run_ta(&["nosuch"]);

    assert_eq!(exit_code(&output), Some(2));
}
