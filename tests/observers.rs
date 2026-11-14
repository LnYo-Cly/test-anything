//! End-to-end tests for observers, and for `log.file` in particular.
//!
//! Every test runs the compiled binary against a local HTTP server started by
//! the test. The server's `/write-log` route appends to a file while the
//! request is answered, so a case produces log lines during its own step
//! without depending on a shell, Python or any platform-specific command.

// The whole file needs the built-in log observer; a build without it compiles
// an empty test target.
#![cfg(feature = "log")]
// Every test is gated on the driver and features it needs, so a build with the
// observer but without a driver uses a subset of the helpers below.
#![allow(dead_code, unused_imports)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use support::{TempDir, TestServer};

/// The binary under test, as an absolute path.
fn da() -> &'static str {
    env!("CARGO_BIN_EXE_da")
}

/// Runs the binary with `dir` as its working directory.
fn run_da_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(da())
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run the da binary")
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

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout must contain exactly one JSON value")
}

/// A case that declares one `log.file` observer named `app_log`.
///
/// `step` is the YAML of one step, already indented for the `steps:` list, and
/// `assertions` is the YAML of its `expect` entries.
#[cfg(feature = "http")]
fn observed_case(name: &str, log: &Path, step: &str, assertions: &str) -> String {
    format!(
        r#"
name: {name}
observe:
  app_log:
    log.file:
      path: '{log}'
steps:
  - {step}
    expect:
{assertions}
"#,
        log = log.display()
    )
}

/// One `http.request` step that writes `text` to the log, appending or not.
#[cfg(feature = "http")]
fn write_log_step(server: &TestServer, log: &Path, text: &str, append: bool) -> String {
    format!(
        r#"http.request:
      method: POST
      url: {url}
      json:
        path: '{log}'
        text: "{text}"
        append: {append}"#,
        url = server.url("/write-log"),
        log = log.display(),
        text = text,
        append = append
    )
}

#[cfg(feature = "http")]
#[test]
fn only_the_lines_a_step_added_are_observed() {
    let server = TestServer::start();
    let dir = TempDir::new("observe-increment");
    let log = dir.path().join("app.log");
    // Text that already exists before the step must not be reported: the point
    // of the observer is the increment of this step.
    fs::write(&log, "startup line\n").expect("the log file must be written");

    let case = dir.write(
        "case.yaml",
        &observed_case(
            "Order is logged",
            &log,
            &write_log_step(&server, &log, "order created\\n", true),
            r#"      - path: $.observed.app_log.lines
        equals: ["order created"]
      - path: $.observed.app_log.text
        equals: "order created\n"
      - path: $.observed.app_log.truncated
        equals: false
      - path: $.observed.app_log.limited
        equals: false
"#,
        ),
    );

    let var = format!("da={}", da());
    let output = run_da_in(
        dir.path(),
        &["run", path_str(&case), "--no-bundle", "--var", &var],
    );
    assert_eq!(
        exit_code(&output),
        Some(0),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&output),
        stderr(&output)
    );
}

#[cfg(feature = "http")]
#[test]
fn a_log_file_that_does_not_exist_yields_an_empty_observation() {
    let server = TestServer::start();
    let dir = TempDir::new("observe-missing");
    let log = dir.path().join("app.log");

    let case = dir.write(
        "case.yaml",
        &observed_case(
            "Missing log",
            &log,
            &format!("http.request:\n      url: {}", server.url("/health")),
            r#"      - path: $.observed.app_log.lines
        equals: []
      - path: $.observed.app_log.text
        equals: ""
      - path: $.observed.app_log.truncated
        equals: false
"#,
        ),
    );

    let var = format!("da={}", da());
    let output = run_da_in(
        dir.path(),
        &["run", path_str(&case), "--no-bundle", "--var", &var],
    );
    assert_eq!(
        exit_code(&output),
        Some(0),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        !log.exists(),
        "an observer must not create the file it watches"
    );
}

#[cfg(feature = "http")]
#[test]
fn a_truncated_log_is_read_from_the_start_and_marked() {
    let server = TestServer::start();
    let dir = TempDir::new("observe-truncated");
    let log = dir.path().join("app.log");
    fs::write(&log, "a much longer line that disappears\n").expect("the log file must be written");

    let case = dir.write(
        "case.yaml",
        &observed_case(
            "Rotated log",
            &log,
            &write_log_step(&server, &log, "restarted\\n", false),
            r#"      - path: $.observed.app_log.lines
        equals: ["restarted"]
      - path: $.observed.app_log.truncated
        equals: true
      - path: $.observed.app_log.limited
        equals: false
"#,
        ),
    );

    let var = format!("da={}", da());
    let output = run_da_in(
        dir.path(),
        &["run", path_str(&case), "--no-bundle", "--var", &var],
    );
    assert_eq!(
        exit_code(&output),
        Some(0),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&output),
        stderr(&output)
    );
}

#[cfg(feature = "http")]
#[test]
fn a_log_larger_than_one_mebibyte_is_cut_off_and_marked() {
    let server = TestServer::start();
    let dir = TempDir::new("observe-limited");
    let log = dir.path().join("app.log");
    let text = "x".repeat(1024 * 1024 + 64);

    let case = dir.write(
        "case.yaml",
        &observed_case(
            "Huge log",
            &log,
            &write_log_step(&server, &log, &text, true),
            r#"      - path: $.observed.app_log.limited
        equals: true
      - path: $.observed.app_log.truncated
        equals: false
"#,
        ),
    );

    let output = run_da_in(
        dir.path(),
        &["--output", "json", "run", path_str(&case), "--no-bundle"],
    );
    assert_eq!(
        exit_code(&output),
        Some(0),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&output),
        stderr(&output)
    );
    let report = json_stdout(&output);
    let observed = &report["cases"][0]["steps"][0]["result"]["observed"]["app_log"];
    assert_eq!(observed["limited"], true, "{observed}");
    let text = observed["text"]
        .as_str()
        .unwrap_or_else(|| panic!("`text` must be a string: {observed}"));
    assert_eq!(
        text.len(),
        1024 * 1024,
        "the observation must stop at 1 MiB"
    );
}

#[cfg(feature = "http")]
#[test]
fn a_failed_assertion_on_observed_values_reports_and_bundles_them() {
    let server = TestServer::start();
    let dir = TempDir::new("observe-bundle");
    let log = dir.path().join("app.log");
    let bundles = dir.path().join("bundles");

    let case = dir.write(
        "case.yaml",
        &observed_case(
            "Wrong log line",
            &log,
            &write_log_step(&server, &log, "boom line\\n", true),
            r#"      - path: $.observed.app_log.lines
        equals: ["expected line"]
"#,
        ),
    );

    let output = run_da_in(
        dir.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(&case),
            "--bundle-dir",
            path_str(&bundles),
        ],
    );
    assert_eq!(
        exit_code(&output),
        Some(1),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&output),
        stderr(&output)
    );
    let report = json_stdout(&output);
    let step = &report["cases"][0]["steps"][0];
    assert_eq!(step["assertions"][0]["path"], "$.observed.app_log.lines");
    assert_eq!(
        step["assertions"][0]["actual"],
        serde_json::json!(["boom line"])
    );

    let bundle = report["cases"][0]["bundle"]
        .as_str()
        .unwrap_or_else(|| panic!("the failed case must name its bundle: {report}"))
        .to_string();
    let bundled: Value = serde_json::from_str(
        &fs::read_to_string(Path::new(&bundle).join("report.json"))
            .expect("report.json must be readable"),
    )
    .expect("report.json must be JSON");
    assert_eq!(
        bundled["steps"][0]["result"]["observed"]["app_log"]["lines"],
        serde_json::json!(["boom line"]),
        "the bundle must carry the observation: {bundled}"
    );
}

#[cfg(feature = "http")]
#[test]
fn two_parallel_cases_do_not_see_each_others_lines() {
    let server = TestServer::start();
    let dir = TempDir::new("observe-jobs");
    let first = dir.path().join("a.log");
    let second = dir.path().join("b.log");

    let expectations = |line: &str| {
        format!(
            r#"      - path: $.observed.app_log.lines
        equals: ["{line}"]
      - path: $.observed.app_log.text
        equals: "{line}\n"
"#
        )
    };
    dir.write(
        "a.yaml",
        &observed_case(
            "Case a",
            &first,
            &write_log_step(&server, &first, "from a\\n", true),
            &expectations("from a"),
        ),
    );
    dir.write(
        "b.yaml",
        &observed_case(
            "Case b",
            &second,
            &write_log_step(&server, &second, "from b\\n", true),
            &expectations("from b"),
        ),
    );

    let output = run_da_in(
        dir.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(dir.path()),
            "--jobs",
            "2",
            "--no-bundle",
        ],
    );
    assert_eq!(
        exit_code(&output),
        Some(0),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&output),
        stderr(&output)
    );
    let report = json_stdout(&output);
    assert_eq!(report["summary"]["cases"], 2);
}

#[cfg(feature = "cli")]
#[test]
fn an_unknown_observer_is_a_case_error_with_a_position() {
    let dir = TempDir::new("observe-unknown");
    let case = dir.write(
        "case.yaml",
        r#"
name: Unknown observer
observe:
  app_log:
    log.fil:
      path: app.log
steps:
  - cli.run:
      command: da
      args: ["--version"]
"#,
    );

    let run = run_da_in(dir.path(), &["run", path_str(&case)]);
    assert_eq!(
        exit_code(&run),
        Some(3),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&run),
        stderr(&run)
    );
    let message = stderr(&run);
    assert!(message.contains("log.fil"), "got: {message}");
    assert!(
        message.contains("line 5"),
        "an unknown observer must point at the observer name, got: {message}"
    );
    assert!(
        message.contains("log.file"),
        "the message must list the registered observers, got: {message}"
    );

    let lint = run_da_in(dir.path(), &["lint", path_str(&case)]);
    assert_eq!(exit_code(&lint), Some(3), "stdout: {:?}", stdout(&lint));
    let message = stdout(&lint);
    assert!(message.contains("log.fil"), "got: {message}");
    assert!(
        message.contains("line 5"),
        "lint must name the position too, got: {message}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn a_null_observe_block_is_a_case_error_with_a_position() {
    let dir = TempDir::new("observe-null");
    // A bare `observe:` is a null value; the schema refuses it, so the parser
    // must refuse it too instead of reading it as "no observations".
    let case = dir.write(
        "case.yaml",
        "name: Null observer\nobserve:\nsteps:\n  - cli.run:\n      command: da\n      args: [\"--version\"]\n",
    );

    let run = run_da_in(dir.path(), &["run", path_str(&case)]);
    assert_eq!(
        exit_code(&run),
        Some(3),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&run),
        stderr(&run)
    );
    let message = stderr(&run);
    assert!(message.contains("`observe`"), "got: {message}");
    assert!(message.contains("found null"), "got: {message}");
    assert!(
        message.contains("line 2"),
        "a null observe block must name its position, got: {message}"
    );

    let lint = run_da_in(dir.path(), &["lint", path_str(&case)]);
    assert_eq!(exit_code(&lint), Some(3), "stdout: {:?}", stdout(&lint));
    assert!(
        stdout(&lint).contains("found null"),
        "lint must report the same problem, got: {}",
        stdout(&lint)
    );
}

#[cfg(feature = "cli")]
#[test]
fn a_duplicate_observation_name_is_a_case_error_with_a_position() {
    let dir = TempDir::new("observe-duplicate");
    // A repeated name is a duplicate key of one YAML mapping, so the parser
    // refuses it before the observer is looked up.
    let case = dir.write(
        "case.yaml",
        r#"
name: Duplicate observer
observe:
  app_log:
    log.file:
      path: one.log
  app_log:
    log.file:
      path: two.log
steps:
  - cli.run:
      command: da
      args: ["--version"]
"#,
    );

    let run = run_da_in(dir.path(), &["run", path_str(&case)]);
    assert_eq!(
        exit_code(&run),
        Some(3),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&run),
        stderr(&run)
    );
    let message = stderr(&run);
    assert!(message.contains("app_log"), "got: {message}");
    assert!(
        message.contains("line 7"),
        "a duplicate name must name its position, got: {message}"
    );

    let lint = run_da_in(dir.path(), &["lint", path_str(&case)]);
    assert_eq!(exit_code(&lint), Some(3), "stdout: {:?}", stdout(&lint));
    assert!(stdout(&lint).contains("app_log"), "got: {}", stdout(&lint));
}

#[cfg(feature = "cli")]
#[test]
fn an_observer_argument_error_is_a_case_error_with_a_position() {
    let dir = TempDir::new("observe-args");

    for (label, args, fragment) in [
        ("empty-path", "path: \"\"", "path"),
        (
            "unknown-argument",
            "path: app.log\n      location: here",
            "location",
        ),
    ] {
        let case = dir.write(
            &format!("{label}.yaml"),
            &format!(
                r#"
name: Bad observer arguments
observe:
  app_log:
    log.file:
      {args}
steps:
  - cli.run:
      command: da
      args: ["--version"]
"#
            ),
        );

        let run = run_da_in(dir.path(), &["run", path_str(&case)]);
        assert_eq!(
            exit_code(&run),
            Some(3),
            "{label}: stdout: {:?}\nstderr: {:?}",
            stdout(&run),
            stderr(&run)
        );
        let message = stderr(&run);
        assert!(
            message.contains("app_log") && message.contains(fragment),
            "{label}: got: {message}"
        );
        assert!(
            message.contains("line "),
            "{label}: an argument error must name its position, got: {message}"
        );

        let lint = run_da_in(dir.path(), &["lint", path_str(&case)]);
        assert_eq!(
            exit_code(&lint),
            Some(3),
            "{label}: stdout: {:?}",
            stdout(&lint)
        );
        assert!(
            stdout(&lint).contains("app_log") && stdout(&lint).contains(fragment),
            "{label}: got: {}",
            stdout(&lint)
        );
    }
}

#[cfg(feature = "http")]
#[test]
fn an_observer_that_cannot_read_is_an_environment_error() {
    let server = TestServer::start();
    let dir = TempDir::new("observe-error");
    // A directory is not a regular file, so the observer refuses it before the
    // step runs, on every platform.
    let log = dir.path().join("not-a-file");
    fs::create_dir(&log).expect("the blocking directory must be created");

    let case = dir.write(
        "case.yaml",
        &observed_case(
            "Unreadable log",
            &log,
            &format!("http.request:\n      url: {}", server.url("/health")),
            r#"      - path: $.status
        equals: 200
"#,
        ),
    );

    let output = run_da_in(
        dir.path(),
        &["--output", "json", "run", path_str(&case), "--no-bundle"],
    );
    assert_eq!(
        exit_code(&output),
        Some(4),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&output),
        stderr(&output)
    );
    let report = json_stdout(&output);
    let step = &report["cases"][0]["steps"][0];
    assert_eq!(step["error"]["kind"], "environment", "{step}");
    let message = step["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("the error must carry a message: {step}"));
    assert!(message.contains("app_log"), "got: {message}");
    assert!(message.contains("log.file"), "got: {message}");
    assert!(
        message.contains("not a regular file"),
        "the reason must be the same on every platform, got: {message}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn an_empty_observation_block_is_accepted() {
    let dir = TempDir::new("observe-empty");
    let case = dir.write(
        "case.yaml",
        r#"
name: No observers
observe: {}
steps:
  - cli.run:
      command: ${da}
      args: ["--version"]
"#,
    );

    let var = format!("da={}", da());
    let output = run_da_in(
        dir.path(),
        &["run", path_str(&case), "--no-bundle", "--var", &var],
    );
    assert_eq!(
        exit_code(&output),
        Some(0),
        "stdout: {:?}\nstderr: {:?}",
        stdout(&output),
        stderr(&output)
    );
}

/// Every file named `name` anywhere below `dir`.
#[cfg(feature = "http")]
fn files_named(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_named(&path, name));
        } else if path.file_name().is_some_and(|file| file == name) {
            found.push(path);
        }
    }
    found
}
