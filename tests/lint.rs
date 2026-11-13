//! End-to-end tests for `ta lint`.
//!
//! Every test runs the compiled binary. Cases that need a driver are gated on
//! the matching feature so that a build without drivers still has a green test
//! suite: such a build cannot accept any step, which is the point of the
//! no-driver schema test rather than a lint regression.

mod support;

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use support::TempDir;

/// The binary under test, as an absolute path.
fn ta() -> &'static str {
    env!("CARGO_BIN_EXE_ta")
}

/// Runs the binary with an empty working directory of its own.
fn run_ta(args: &[&str]) -> Output {
    let work = TempDir::new("lint-cwd");
    run_ta_in(work.path(), args)
}

/// Runs the binary with `dir` as its working directory.
fn run_ta_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(ta())
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run the ta binary")
}

fn exit_code(output: &Output) -> Option<i32> {
    output.status.code()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("test paths must be valid UTF-8")
}

/// A path the way `ta` reports it: `/` separated on every platform.
fn reported(path: &Path) -> String {
    path_str(path).replace('\\', "/")
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout must contain exactly one JSON value")
}

/// The `(file, message)` pairs of a lint or run JSON report.
#[cfg(feature = "cli")]
fn error_pairs(value: &Value) -> Vec<(String, String)> {
    value["errors"]
        .as_array()
        .expect("errors must be an array")
        .iter()
        .map(|error| {
            (
                error["file"]
                    .as_str()
                    .expect("every error names its file")
                    .to_string(),
                error["message"]
                    .as_str()
                    .expect("every error has a message")
                    .to_string(),
            )
        })
        .collect()
}

#[cfg(feature = "cli")]
#[test]
fn a_valid_case_exits_zero_and_reports_the_file_count() {
    let dir = TempDir::new("lint-valid");
    let case = dir.write(
        "valid.yaml",
        "name: Valid\nsteps:\n  - cli.run:\n      command: ta\n      args: [\"--version\"]\n",
    );

    let output = run_ta(&["lint", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("1 file") && stdout.contains("0 errors"),
        "the summary must name the files and errors checked, got: {stdout:?}"
    );
    assert!(stderr(&output).is_empty(), "got: {:?}", stderr(&output));
}

#[test]
fn a_syntax_error_exits_three_and_reports_its_location() {
    let dir = TempDir::new("lint-syntax");
    let case = dir.write("broken.yaml", "name: Broken\nsteps:\n  - cli.run: [\n");

    let output = run_ta(&["lint", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3), "stderr: {:?}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("line"),
        "a grammar error must name its location, got: {stdout:?}"
    );
    assert!(
        stdout.contains("broken.yaml"),
        "the message must name the file, got: {stdout:?}"
    );
    // The parser shows the source line under the error. Lint keeps that
    // snippet as the parser wrote it, because a caret line is what makes the
    // location readable.
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines.len() > 2,
        "the snippet must keep its line breaks, got: {stdout:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains('^')),
        "the snippet must keep the caret line, got: {stdout:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.trim_end().ends_with("- cli.run: [")),
        "the snippet must keep the source line on its own line, got: {stdout:?}"
    );
    assert_eq!(
        lines.last().copied(),
        Some("Checked 1 file: 1 error."),
        "the summary must stay the last line, got: {stdout:?}"
    );
}

/// A `/`-separated directory can be pasted back from the output on Windows.
#[test]
fn reported_paths_use_forward_slashes_only() {
    let dir = TempDir::new("lint-path-separators");
    dir.write("broken.yaml", "name: Broken\nsteps:\n  - cli.run: [\n");
    let given = path_str(dir.path()).replace('\\', "/");

    let text = run_ta(&["lint", &given]);
    assert_eq!(exit_code(&text), Some(3), "stderr: {:?}", stderr(&text));
    assert!(
        !stdout(&text).contains('\\'),
        "lint text must use `/` only, got: {:?}",
        stdout(&text)
    );

    let json = run_ta(&["--output", "json", "lint", &given]);
    let value = json_stdout(&json);
    for error in value["errors"].as_array().expect("errors must be an array") {
        let file = error["file"].as_str().expect("every error names a file");
        assert!(
            !file.contains('\\'),
            "the JSON `file` must use `/` only, got: {file}"
        );
    }
}

#[test]
fn an_unknown_field_exits_three_with_a_line_number() {
    let dir = TempDir::new("lint-unknown-field");
    let case = dir.write(
        "unknown.yaml",
        "name: Broken\nnaem: typo\nsteps:\n  - cli.run:\n      command: ta\n",
    );

    let output = run_ta(&["lint", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3));
    let stdout = stdout(&output);
    assert!(stdout.contains("naem"), "got: {stdout:?}");
    assert!(stdout.contains("line 2"), "got: {stdout:?}");
    assert!(
        stdout.contains(&format!("{}: ", reported(&case))),
        "every error must start with the file, got: {stdout:?}"
    );
}

#[test]
fn an_unregistered_step_exits_three() {
    let dir = TempDir::new("lint-unregistered");
    let case = dir.write(
        "unregistered.yaml",
        "name: Unknown\nsteps:\n  - shell.run:\n      command: echo hi\n",
    );

    let output = run_ta(&["lint", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3));
    assert!(
        stdout(&output).contains("shell.run"),
        "got: {:?}",
        stdout(&output)
    );
}

#[cfg(feature = "cli")]
#[test]
fn invalid_arguments_exit_three() {
    let dir = TempDir::new("lint-bad-args");
    let case = dir.write(
        "bad-args.yaml",
        "name: Bad arguments\nsteps:\n  - cli.run:\n      command: ta\n      timeout_ms: 0\n",
    );

    let output = run_ta(&["lint", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3));
    assert!(
        stdout(&output).contains("timeout_ms"),
        "got: {:?}",
        stdout(&output)
    );
}

#[cfg(feature = "cli")]
#[test]
fn an_undefined_variable_exits_three_and_var_defines_it() {
    let dir = TempDir::new("lint-var");
    let case = dir.write(
        "var.yaml",
        "name: Variable\nsteps:\n  - cli.run:\n      command: ${target}\n",
    );

    let without = run_ta(&["lint", path_str(&case)]);
    assert_eq!(exit_code(&without), Some(3));
    assert!(
        stdout(&without).contains("target"),
        "got: {:?}",
        stdout(&without)
    );

    let with = run_ta(&[
        "lint",
        path_str(&case),
        "--var",
        &format!("target={}", ta()),
    ]);
    assert_eq!(exit_code(&with), Some(0), "stderr: {:?}", stderr(&with));
}

#[test]
fn every_error_of_every_file_is_reported_once() {
    let dir = TempDir::new("lint-many");
    dir.write("a.yaml", "name: A\nsteps: [\n");
    dir.write(
        "b.yaml",
        "name: B\nnaem: typo\nsteps:\n  - cli.run:\n      command: ta\n",
    );
    dir.write(
        "c.yaml",
        "name: C\nsteps:\n  - shell.run:\n      command: echo hi\n",
    );

    let output = run_ta(&["lint", path_str(dir.path())]);

    assert_eq!(exit_code(&output), Some(3), "stderr: {:?}", stderr(&output));
    let stdout = stdout(&output);
    for name in ["a.yaml", "b.yaml", "c.yaml"] {
        assert!(stdout.contains(name), "missing {name} in: {stdout:?}");
    }
    assert!(stdout.contains("naem"), "got: {stdout:?}");
    assert!(stdout.contains("shell.run"), "got: {stdout:?}");
    assert!(
        stdout.contains("3 files") && stdout.contains("3 errors"),
        "the summary must count every file and error, got: {stdout:?}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn lint_does_not_execute_any_step() {
    let dir = TempDir::new("lint-no-side-effect");
    let bundle = dir.path().join("would-be-bundle");
    let inner = dir.write(
        "inner-failing.yaml",
        &format!(
            "name: Inner failing\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n    expect:\n      - path: $.exit_code\n        equals: 42\n",
            ta()
        ),
    );
    let outer = dir.write(
        "outer.yaml",
        &format!(
            "name: Would write a bundle\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"run\", '{}', \"--bundle-dir\", '{}']\n",
            ta(),
            inner.display(),
            bundle.display()
        ),
    );

    let linted = run_ta(&["lint", path_str(&outer)]);
    assert_eq!(exit_code(&linted), Some(0), "stderr: {:?}", stderr(&linted));
    assert!(
        !bundle.exists(),
        "lint executed the step and created {}",
        bundle.display()
    );

    // Prove the fixture really would create the bundle when it runs, so the
    // assertion above tests lint rather than a case that does nothing.
    let ran = run_ta_in(dir.path(), &["run", path_str(&outer), "--no-bundle"]);
    assert_eq!(exit_code(&ran), Some(0), "stderr: {:?}", stderr(&ran));
    assert!(
        bundle.exists(),
        "the fixture must write a bundle when run, so lint's silence is meaningful"
    );
}

#[cfg(feature = "cli")]
#[test]
fn lint_and_run_report_the_same_errors() {
    let dir = TempDir::new("lint-run-agree");
    dir.write(
        "a.yaml",
        "name: A\nsteps:\n  - cli.run:\n      command: ta\n      timeout_ms: 0\n",
    );
    dir.write(
        "b.yaml",
        "name: B\nsteps:\n  - shell.run:\n      command: echo hi\n",
    );

    let run = run_ta(&[
        "--output",
        "json",
        "run",
        path_str(dir.path()),
        "--no-bundle",
    ]);
    let lint = run_ta(&["--output", "json", "lint", path_str(dir.path())]);

    assert_eq!(exit_code(&run), Some(3), "stderr: {:?}", stderr(&run));
    assert_eq!(exit_code(&lint), Some(3));
    let run_value = json_stdout(&run);
    let lint_value = json_stdout(&lint);
    assert_eq!(
        error_pairs(&run_value),
        error_pairs(&lint_value),
        "lint must report the same errors as run"
    );
}

#[cfg(feature = "cli")]
#[test]
fn json_output_is_one_object_with_the_documented_fields() {
    let dir = TempDir::new("lint-json");
    let case = dir.write(
        "valid.yaml",
        "name: Valid\nsteps:\n  - cli.run:\n      command: ta\n",
    );

    let output = run_ta(&["--output", "json", "lint", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], Value::Bool(true));
    assert_eq!(value["files"], 1);
    assert_eq!(value["errors"], Value::Array(Vec::new()));
}

#[test]
fn json_output_reports_an_error_and_exits_three() {
    let dir = TempDir::new("lint-json-error");
    let case = dir.write("broken.yaml", "name: Broken\nnaem: typo\nsteps: []\n");

    let output = run_ta(&["--output", "json", "lint", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], Value::Bool(false));
    assert_eq!(value["files"], 1);
    let errors = value["errors"].as_array().expect("errors must be an array");
    assert_eq!(errors.len(), 1, "got: {value}");
    assert!(
        errors[0]["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("broken.yaml")),
        "got: {value}"
    );
    assert!(
        errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("naem")),
        "got: {value}"
    );
}

#[test]
fn a_missing_path_exits_three() {
    let output = run_ta(&["lint", "does-not-exist.yaml"]);

    assert_eq!(exit_code(&output), Some(3));
    assert!(
        stdout(&output).contains("does-not-exist.yaml"),
        "got: {:?}",
        stdout(&output)
    );
}

#[test]
fn a_directory_without_yaml_files_exits_three() {
    let dir = TempDir::new("lint-empty");

    let output = run_ta(&["lint", path_str(dir.path())]);

    assert_eq!(exit_code(&output), Some(3));
    assert!(
        stdout(&output).contains("no `.yaml` or `.yml` case files found"),
        "got: {:?}",
        stdout(&output)
    );
}

#[test]
fn a_malformed_var_exits_two_with_the_expected_form() {
    let output = run_ta(&["lint", "some.yaml", "--var", "novalue"]);

    assert_eq!(exit_code(&output), Some(2));
    assert!(
        stderr(&output).contains("--var name=value"),
        "got: {:?}",
        stderr(&output)
    );
}
