//! End-to-end tests that run the compiled `ta` binary.

mod support;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use support::{TempDir, TestServer, closed_port};

/// The binary under test, as an absolute path.
fn ta() -> &'static str {
    env!("CARGO_BIN_EXE_ta")
}

/// Runs the binary with an empty working directory of its own.
///
/// A run that fails writes a failure bundle under `.ta/failures` in the current
/// directory, so every run gets a directory that is removed with it instead of
/// filling the checkout of whoever runs the tests.
fn run_ta(args: &[&str]) -> Output {
    let work = TempDir::new("cwd");
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

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout must contain exactly one JSON value")
}

#[test]
fn version_prints_the_package_version() {
    let output = run_ta(&["--version"]);

    assert_eq!(exit_code(&output), Some(0));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected the version in stdout, got: {stdout:?}"
    );
}

#[test]
fn lint_still_reports_not_implemented() {
    let output = run_ta(&["lint", "some.yaml"]);

    assert_eq!(exit_code(&output), Some(9));
    assert!(
        output.stdout.is_empty(),
        "standard output must stay clean in text mode, got: {:?}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("not implemented"),
        "expected a human-readable message on stderr, got: {:?}",
        stderr(&output)
    );
}

#[test]
fn json_output_for_lint_is_a_single_error_object() {
    let output = run_ta(&["--output", "json", "lint", "some.yaml"]);

    assert_eq!(exit_code(&output), Some(9));
    let value = json_stdout(&output);

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

#[test]
fn a_case_that_passes_exits_zero_and_saves_variables() {
    let server = TestServer::start();
    let dir = TempDir::new("passing");
    let case = dir.write(
        "login.yaml",
        &format!(
            r#"
name: Login returns a token
steps:
  - http.request:
      method: POST
      url: {base}/login
      json: {{ user: alice, password: secret }}
    expect:
      - path: $.status
        equals: 200
      - path: $.body.token
        exists: true
    save:
      token: $.body.token
  - http.request:
      url: {base}/me
      headers: {{ Authorization: "Bearer ${{token}}" }}
    expect:
      - path: $.body.name
        equals: alice
"#,
            base = server.url("")
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("PASS"),
        "expected a passing line, got: {stdout:?}"
    );
    assert!(
        stdout.to_lowercase().contains("summary"),
        "expected a summary line, got: {stdout:?}"
    );

    let requests = server.requests();
    assert_eq!(
        requests.len(),
        2,
        "expected two requests, got: {requests:?}"
    );
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path(), "/login");
    assert_eq!(
        requests[1].headers.get("authorization").map(String::as_str),
        Some("Bearer t-123")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&requests[0].body).expect("the login body must be JSON"),
        serde_json::json!({"user": "alice", "password": "secret"})
    );
}

#[test]
fn a_failed_assertion_exits_one_and_reports_path_expected_and_actual() {
    let server = TestServer::start();
    let dir = TempDir::new("failing");
    let case = dir.write(
        "failing.yaml",
        &format!(
            r#"
name: Wrong status
steps:
  - http.request:
      url: {base}/health
    expect:
      - path: $.status
        equals: 201
      - path: $.body.status
        equals: ok
"#,
            base = server.url("")
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(1));
    let stdout = stdout(&output);
    assert!(
        stdout.contains("FAIL"),
        "expected a failing line, got: {stdout:?}"
    );
    assert!(
        stdout.contains("$.status"),
        "expected the path, got: {stdout:?}"
    );
    assert!(
        stdout.contains("201"),
        "expected the expected value, got: {stdout:?}"
    );
    assert!(
        stdout.contains("200"),
        "expected the actual value, got: {stdout:?}"
    );
}

#[test]
fn json_output_describes_cases_steps_and_assertions() {
    let server = TestServer::start();
    let dir = TempDir::new("json");
    let case = dir.write(
        "failing.yaml",
        &format!(
            r#"
name: Wrong status
steps:
  - http.request:
      url: {base}/health
    expect:
      - path: $.status
        equals: 201
      - path: $.body.status
        equals: ok
"#,
            base = server.url("")
        ),
    );

    let output = run_ta(&["--output", "json", "run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(1));
    let value = json_stdout(&output);

    assert_eq!(value["ok"], Value::Bool(false));
    assert_eq!(value["summary"]["cases"], 1);
    assert_eq!(value["summary"]["cases_passed"], 0);
    assert_eq!(value["summary"]["cases_failed"], 1);
    assert!(value["summary"]["duration_ms"].is_number());

    let case = &value["cases"][0];
    assert_eq!(case["name"], "Wrong status");
    assert!(
        case["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("failing.yaml")),
        "expected the case file, got: {case}"
    );
    assert_eq!(case["status"], "failed");
    assert!(case["duration_ms"].is_number());

    let step = &case["steps"][0];
    assert_eq!(step["name"], "http.request");
    assert_eq!(step["status"], "failed");
    assert!(step["duration_ms"].is_number());
    assert_eq!(step["result"]["status"], 200);

    let assertion = &step["assertions"][0];
    assert_eq!(assertion["path"], "$.status");
    assert_eq!(assertion["comparison"], "equals");
    assert_eq!(assertion["expected"], 201);
    assert_eq!(assertion["actual"], 200);
    assert_eq!(assertion["passed"], Value::Bool(false));

    let passing = &step["assertions"][1];
    assert_eq!(passing["passed"], Value::Bool(true));
    assert_eq!(passing["actual"], "ok");
}

#[test]
fn an_unreachable_target_exits_four() {
    let port = closed_port();
    let dir = TempDir::new("unreachable");
    let case = dir.write(
        "unreachable.yaml",
        &format!(
            r#"
name: Unreachable
steps:
  - http.request:
      url: http://127.0.0.1:{port}/health
"#
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(4), "stderr: {:?}", stderr(&output));
    assert!(
        stdout(&output).contains("ERROR"),
        "expected an error line, got: {:?}",
        stdout(&output)
    );
}

#[test]
fn a_step_that_exceeds_its_timeout_exits_five() {
    let server = TestServer::start();
    let dir = TempDir::new("timeout");
    let case = dir.write(
        "timeout.yaml",
        &format!(
            r#"
name: Too slow
steps:
  - http.request:
      url: {base}/slow
      timeout_ms: 50
"#,
            base = server.url("")
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(5), "stderr: {:?}", stderr(&output));
}

#[test]
fn an_unknown_field_exits_three_with_a_line_number() {
    let dir = TempDir::new("unknown-field");
    let case = dir.write(
        "unknown.yaml",
        "name: Broken case\nnaem: typo\nsteps:\n  - http.request:\n      url: http://127.0.0.1:1/health\n",
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("naem"),
        "expected the field name, got: {stderr:?}"
    );
    assert!(
        stderr.contains("line 2"),
        "expected a line number, got: {stderr:?}"
    );
    assert!(
        output.stdout.is_empty(),
        "nothing should be reported as a result, got: {:?}",
        stdout(&output)
    );
}

#[test]
fn an_unregistered_step_exits_three() {
    let dir = TempDir::new("unregistered");
    let case = dir.write(
        "unregistered.yaml",
        "name: Unknown step\nsteps:\n  - shell.run:\n      command: echo hi\n",
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("shell.run"),
        "expected the step name, got: {stderr:?}"
    );
    assert!(
        stderr.contains("http.request"),
        "expected the registered step names to be listed, got: {stderr:?}"
    );
}

#[test]
fn arguments_that_do_not_match_the_schema_exit_three() {
    let dir = TempDir::new("bad-args");
    let case = dir.write(
        "bad-args.yaml",
        "name: Bad arguments\nsteps:\n  - http.request:\n      metod: GET\n      url: http://127.0.0.1:1/health\n",
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("metod"),
        "expected the unknown argument, got: {stderr:?}"
    );
}

#[test]
fn an_undefined_variable_exits_three() {
    let dir = TempDir::new("undefined-var");
    let case = dir.write(
        "undefined.yaml",
        "name: Undefined variable\nsteps:\n  - http.request:\n      url: http://127.0.0.1:${port}/health\n",
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(3));
    let stderr = stderr(&output);
    assert!(
        stderr.contains("port"),
        "expected the variable name, got: {stderr:?}"
    );
}

#[test]
fn an_invalid_case_prevents_every_other_case_from_running() {
    let server = TestServer::start();
    let dir = TempDir::new("rejected");
    dir.write(
        "a-good.yaml",
        &format!(
            "name: Good\nsteps:\n  - http.request:\n      url: {base}/health\n",
            base = server.url("")
        ),
    );
    dir.write(
        "b-broken.yaml",
        "name: Broken\nsteps:\n  - shell.run:\n      command: echo hi\n",
    );

    let output = run_ta(&["run", path_str(dir.path())]);

    assert_eq!(exit_code(&output), Some(3));
    assert!(
        output.stdout.is_empty(),
        "no case should have run, got: {:?}",
        stdout(&output)
    );
    assert_eq!(
        server.request_count(),
        0,
        "the valid case must not run when any case is invalid"
    );
}

#[test]
fn a_directory_is_scanned_recursively_in_path_order() {
    let server = TestServer::start();
    let dir = TempDir::new("directory");
    let case = |label: &str| {
        format!(
            "name: Case {label}\nsteps:\n  - http.request:\n      url: {base}/health\n",
            base = server.url("")
        )
    };
    dir.write("b.yaml", &case("b"));
    dir.write("nested/c.yml", &case("c"));
    dir.write("a.yaml", &case("a"));

    let output = run_ta(&["--output", "json", "run", path_str(dir.path())]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
    let value = json_stdout(&output);
    let files: Vec<&str> = value["cases"]
        .as_array()
        .expect("cases must be an array")
        .iter()
        .filter_map(|case| case["file"].as_str())
        .collect();
    assert_eq!(files.len(), 3, "expected three cases, got: {files:?}");
    assert!(files[0].ends_with("a.yaml"), "got: {files:?}");
    assert!(files[1].ends_with("b.yaml"), "got: {files:?}");
    assert!(files[2].ends_with("c.yml"), "got: {files:?}");
}

#[test]
fn a_failed_step_stops_its_case_but_not_the_other_cases() {
    let server = TestServer::start();
    let dir = TempDir::new("stop-on-failure");
    dir.write(
        "a-failing.yaml",
        &format!(
            r#"
name: Failing
steps:
  - http.request:
      url: {base}/health
    expect:
      - path: $.status
        equals: 999
  - http.request:
      url: {base}/text
"#,
            base = server.url("")
        ),
    );
    dir.write(
        "b-passing.yaml",
        &format!(
            "name: Passing\nsteps:\n  - http.request:\n      url: {base}/health\n",
            base = server.url("")
        ),
    );

    let output = run_ta(&["--output", "json", "run", path_str(dir.path())]);

    assert_eq!(exit_code(&output), Some(1), "stderr: {:?}", stderr(&output));
    let value = json_stdout(&output);

    let failing = &value["cases"][0];
    assert_eq!(failing["name"], "Failing");
    assert_eq!(failing["status"], "failed");
    assert_eq!(failing["steps"][1]["status"], "skipped");
    assert_eq!(failing["steps"][1]["result"], Value::Null);

    let passing = &value["cases"][1];
    assert_eq!(passing["status"], "passed");
    assert_eq!(passing["steps"][0]["status"], "passed");

    assert_eq!(
        server.request_count(),
        2,
        "the skipped step must not send a request"
    );
}

#[test]
fn every_comparison_passes_when_the_result_matches() {
    let server = TestServer::start();
    let dir = TempDir::new("comparisons");
    let case = dir.write(
        "comparisons.yaml",
        &format!(
            r#"
name: Comparisons
steps:
  - http.request:
      url: {base}/health
    expect:
      - path: $.status
        not_equals: 500
      - path: $.body.status
        equals: ok
      - path: $.body.status
        contains: "o"
      - path: $.body.missing
        exists: false
      - path: $.body
        exists: true
"#,
            base = server.url("")
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
    assert!(
        stdout(&output).contains("PASS"),
        "got: {:?}",
        stdout(&output)
    );
}

#[test]
fn query_headers_and_json_bodies_reach_the_server() {
    let server = TestServer::start();
    let dir = TempDir::new("echo");
    let case = dir.write(
        "echo.yaml",
        &format!(
            r#"
name: Request shape
vars:
  page: "2"
steps:
  - http.request:
      method: POST
      url: {base}/echo
      headers:
        X-Token: abc
      query:
        page: ${{page}}
      json: {{ user: alice }}
    expect:
      - path: $.body.method
        equals: POST
      - path: $.body.query
        equals: page=2
      - path: $.body.headers['x-token']
        equals: abc
      - path: $.body.headers['content-type']
        equals: application/json
      - path: $.body.body
        equals: '{{"user":"alice"}}'
"#,
            base = server.url("")
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].query(), "page=2");
}

#[test]
fn a_non_success_status_is_part_of_the_result() {
    let server = TestServer::start();
    let dir = TempDir::new("status");
    let case = dir.write(
        "status.yaml",
        &format!(
            r#"
name: Server error is data
steps:
  - http.request:
      url: {base}/status/500
    expect:
      - path: $.status
        equals: 500
      - path: $.body.error
        equals: boom
"#,
            base = server.url("")
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
}

#[test]
fn a_text_body_is_a_string() {
    let server = TestServer::start();
    let dir = TempDir::new("text");
    let case = dir.write(
        "text.yaml",
        &format!(
            r#"
name: Text body
steps:
  - http.request:
      url: {base}/text
    expect:
      - path: $.body
        equals: hello world
"#,
            base = server.url("")
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
}

#[test]
fn a_missing_case_file_exits_three() {
    let output = run_ta(&["run", "does-not-exist.yaml"]);

    assert_eq!(exit_code(&output), Some(3));
    assert!(
        stderr(&output).contains("does-not-exist.yaml"),
        "expected the path in the message, got: {:?}",
        stderr(&output)
    );
}

/// Reads one chunk from a child pipe and then closes it.
///
/// The tests below write far more than a pipe buffer, so closing after the
/// first chunk makes the next write in the child fail with a broken pipe.
fn close_after_first_read(mut pipe: impl Read) {
    let mut buffer = [0_u8; 64];
    let read = pipe.read(&mut buffer).expect("failed to read the pipe");
    assert!(read > 0, "expected output before closing the pipe");
}

#[test]
fn a_closed_stdout_pipe_does_not_panic_and_keeps_the_run_exit_code() {
    let server = TestServer::start();
    let dir = TempDir::new("broken-stdout");
    // A failing assertion whose expected value is far larger than a pipe
    // buffer, so `ta` is still writing when the reader closes the pipe.
    let expected = "x".repeat(512 * 1024);
    let case = dir.write(
        "huge-expected.yaml",
        &format!(
            r#"
name: Huge expected value
steps:
  - http.request:
      url: {base}/health
    expect:
      - path: $.body.status
        equals: "{expected}"
"#,
            base = server.url("")
        ),
    );

    let mut child = Command::new(ta())
        .current_dir(dir.path())
        .args(["run", path_str(&case)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run the ta binary");

    close_after_first_read(child.stdout.take().expect("stdout must be piped"));
    let output = child
        .wait_with_output()
        .expect("failed to wait for the ta binary");

    let stderr = stderr(&output);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a closed pipe must keep the run exit code, got {:?} with stderr: {stderr:?}",
        output.status.code()
    );
    assert!(
        !stderr.contains("panicked"),
        "ta panicked instead of ending quietly: {stderr:?}"
    );
}

#[test]
fn a_closed_stderr_pipe_does_not_panic_and_keeps_the_invalid_case_exit_code() {
    let dir = TempDir::new("broken-stderr");
    // The error message echoes the invalid JSON path, so standard error is far
    // larger than a pipe buffer.
    let invalid_path = format!("$.{}[", "x".repeat(256 * 1024));
    let case = dir.write(
        "huge-path.yaml",
        &format!(
            r#"
name: Huge invalid path
steps:
  - http.request:
      url: http://127.0.0.1:1/health
    expect:
      - path: "{invalid_path}"
        exists: true
"#
        ),
    );

    let mut child = Command::new(ta())
        .current_dir(dir.path())
        .args(["run", path_str(&case)])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run the ta binary");

    close_after_first_read(child.stderr.take().expect("stderr must be piped"));
    let output = child
        .wait_with_output()
        .expect("failed to wait for the ta binary");

    let stderr = stderr(&output);
    assert_eq!(
        output.status.code(),
        Some(3),
        "a closed pipe must keep the invalid-case exit code, got {:?} with stderr: {stderr:?}",
        output.status.code()
    );
    assert!(
        !stderr.contains("panicked"),
        "ta panicked instead of ending quietly: {stderr:?}"
    );
}

/// A case whose only step runs `ta --version` and expects the wrong exit code.
///
/// The program under test is `ta` itself: the failure paths must not depend on
/// a system command such as `false` or `exit`, which differ across platforms.
fn wrong_expectation_case(name: &str) -> String {
    format!(
        r#"
name: {name}
steps:
  - cli.run:
      command: '{}'
      args: ["--version"]
    expect:
      - path: $.exit_code
        equals: 42
"#,
        ta()
    )
}

#[test]
fn a_cli_step_reports_exit_code_stdout_stderr_and_parsed_json() {
    let dir = TempDir::new("cli-step");
    let case = dir.write(
        "cli.yaml",
        &format!(
            r#"
name: CLI step
steps:
  - cli.run:
      command: '{}'
      args: ["--version"]
      stdin: "ignored input"
    expect:
      - path: $.exit_code
        equals: 0
      - path: $.stdout
        contains: "ta "
      - path: $.stderr
        equals: ""
      - path: $.json
        exists: false
  - cli.run:
      command: '{}'
      args: ["nosuch-subcommand"]
    expect:
      - path: $.exit_code
        equals: 2
      - path: $.stderr
        not_equals: ""
      - path: $.stdout
        equals: ""
      - path: $.json
        exists: false
"#,
            ta(),
            ta()
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
}

#[test]
fn a_cli_step_runs_in_the_requested_working_directory() {
    let dir = TempDir::new("cli-cwd");
    dir.write(
        "inner.yaml",
        &format!(
            "name: Inner\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n",
            ta()
        ),
    );
    let case = dir.write(
        "outer.yaml",
        &format!(
            r#"
name: Working directory
steps:
  - cli.run:
      command: '{}'
      args: ["run", "inner.yaml"]
      cwd: '{}'
    expect:
      - path: $.exit_code
        equals: 0
      - path: $.stdout
        contains: "PASS"
"#,
            ta(),
            dir.path().display()
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
}

#[test]
fn a_missing_program_exits_four_and_names_it() {
    let dir = TempDir::new("cli-missing");
    let case = dir.write(
        "missing.yaml",
        "name: Missing program\nsteps:\n  - cli.run:\n      command: ta-no-such-program-4ba42\n",
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(4), "stderr: {:?}", stderr(&output));
    assert!(
        stdout(&output).contains("ta-no-such-program-4ba42"),
        "the message must name the program, got: {:?}",
        stdout(&output)
    );
}

#[test]
fn a_cli_step_with_invalid_arguments_exits_three() {
    let dir = TempDir::new("cli-invalid-args");
    let unknown = dir.write(
        "unknown.yaml",
        &format!(
            "name: Unknown argument\nsteps:\n  - cli.run:\n      cmnd: '{}'\n      command: '{}'\n",
            ta(),
            ta()
        ),
    );
    let zero = dir.write(
        "zero.yaml",
        &format!(
            "name: Zero timeout\nsteps:\n  - cli.run:\n      command: '{}'\n      timeout_ms: 0\n",
            ta()
        ),
    );

    let unknown_output = run_ta(&["run", path_str(&unknown)]);
    assert_eq!(exit_code(&unknown_output), Some(3));
    assert!(
        stderr(&unknown_output).contains("cmnd"),
        "got: {:?}",
        stderr(&unknown_output)
    );

    let zero_output = run_ta(&["run", path_str(&zero)]);
    assert_eq!(exit_code(&zero_output), Some(3));
    assert!(
        stderr(&zero_output).contains("timeout_ms"),
        "got: {:?}",
        stderr(&zero_output)
    );
}

#[test]
fn a_cli_step_that_exceeds_its_timeout_exits_five() {
    let server = TestServer::start();
    let dir = TempDir::new("cli-timeout");
    // The child is `ta` itself, waiting on a local service that answers slowly.
    let slow = dir.write(
        "slow.yaml",
        &format!(
            "name: Slow\nsteps:\n  - http.request:\n      url: {base}/slow\n      timeout_ms: 10000\n",
            base = server.url("")
        ),
    );
    let case = dir.write(
        "timeout.yaml",
        &format!(
            r#"
name: Slow program
steps:
  - cli.run:
      command: '{}'
      args: ["run", '{}']
      timeout_ms: 150
"#,
            ta(),
            slow.display()
        ),
    );

    let started = std::time::Instant::now();
    let output = run_ta(&["run", path_str(&case)]);
    let elapsed = started.elapsed();

    assert_eq!(exit_code(&output), Some(5), "stderr: {:?}", stderr(&output));
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "the run must stop soon after the timeout, took {elapsed:?}"
    );
}

#[test]
fn a_cli_step_reads_more_output_than_a_pipe_buffer() {
    let dir = TempDir::new("cli-big-output");
    // Far more than the 64 KiB a pipe holds, so a reader that waits for the
    // process before draining deadlocks instead of finishing.
    let expected = "x".repeat(512 * 1024);
    let inner = dir.write(
        "inner.yaml",
        &format!(
            r#"
name: Huge output
steps:
  - cli.run:
      command: '{}'
      args: ["--version"]
    expect:
      - path: $.stdout
        equals: "{expected}"
"#,
            ta()
        ),
    );
    let case = dir.write(
        "outer.yaml",
        &format!(
            r#"
name: Big output
steps:
  - cli.run:
      command: '{}'
      args: ["run", '{}']
    expect:
      - path: $.exit_code
        equals: 1
      - path: $.stdout
        contains: "Huge output"
      - path: $.stderr
        equals: ""
"#,
            ta(),
            inner.display()
        ),
    );

    let output = run_ta(&["run", path_str(&case)]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
}

#[test]
fn a_failing_case_writes_a_complete_bundle() {
    let dir = TempDir::new("bundle");
    let case = dir.write("failing.yaml", &wrong_expectation_case("Wrong exit code"));
    let root = dir.path().join("failures");

    let output = run_ta(&[
        "--output",
        "json",
        "run",
        path_str(&case),
        "--bundle-dir",
        path_str(&root),
    ]);

    assert_eq!(exit_code(&output), Some(1), "stderr: {:?}", stderr(&output));
    let value = json_stdout(&output);
    let reported = value["cases"][0]["bundle"]
        .as_str()
        .expect("a failed case must report its bundle");
    let bundle = PathBuf::from(reported);
    assert!(
        bundle.starts_with(&root),
        "the bundle must live under the requested directory, got: {reported}"
    );

    for name in ["case.yaml", "report.json", "meta.json"] {
        assert!(
            bundle.join(name).is_file(),
            "the bundle must contain {name}: {reported}"
        );
    }

    let meta: Value = serde_json::from_str(
        &fs::read_to_string(bundle.join("meta.json")).expect("meta.json must be readable"),
    )
    .expect("meta.json must be JSON");
    assert_eq!(meta["schema_version"], 1);
    assert_eq!(meta["ta_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(meta["case_name"], "Wrong exit code");
    assert_eq!(meta["status"], "failed");
    assert!(
        meta["case_file"]
            .as_str()
            .is_some_and(|file| file.ends_with("failing.yaml")),
        "got: {meta}"
    );
    assert!(
        meta["created_at"].as_u64().is_some_and(|millis| millis > 0),
        "got: {meta}"
    );

    let report: Value = serde_json::from_str(
        &fs::read_to_string(bundle.join("report.json")).expect("report.json must be readable"),
    )
    .expect("report.json must be JSON");
    assert_eq!(report["name"], value["cases"][0]["name"]);
    assert_eq!(report["status"], "failed");
    assert_eq!(report["bundle"], value["cases"][0]["bundle"]);
    assert_eq!(report["steps"][0]["assertions"][0]["passed"], false);

    assert_eq!(
        fs::read(bundle.join("case.yaml")).expect("case.yaml must be readable"),
        fs::read(&case).expect("the case file must be readable"),
        "case.yaml must be a copy of the case file"
    );

    let run_dir = bundle.parent().expect("a bundle has a run directory");
    let entries: Vec<_> = fs::read_dir(run_dir)
        .expect("the run directory must be readable")
        .map(|entry| entry.expect("the entry must be readable").file_name())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "no staging directory may be left behind: {entries:?}"
    );
}

#[test]
fn a_passing_run_creates_no_bundle_directory() {
    let dir = TempDir::new("bundle-passing");
    let case = dir.write(
        "passing.yaml",
        &format!(
            "name: Passing\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n",
            ta()
        ),
    );
    let root = dir.path().join("failures");

    let output = run_ta(&[
        "--output",
        "json",
        "run",
        path_str(&case),
        "--bundle-dir",
        path_str(&root),
    ]);

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
    assert!(
        !root.exists(),
        "a passing run must not create the bundle directory"
    );
    let value = json_stdout(&output);
    assert_eq!(value["cases"][0]["bundle"], Value::Null);
}

#[test]
fn no_bundle_turns_the_failure_bundle_off() {
    let dir = TempDir::new("bundle-off");
    let case = dir.write("failing.yaml", &wrong_expectation_case("No bundle"));

    let output = run_ta_in(
        dir.path(),
        &["--output", "json", "run", path_str(&case), "--no-bundle"],
    );

    assert_eq!(exit_code(&output), Some(1), "stderr: {:?}", stderr(&output));
    assert!(
        !dir.path().join(".ta").exists(),
        "the default bundle directory must not be created"
    );
    let value = json_stdout(&output);
    assert_eq!(value["cases"][0]["bundle"], Value::Null);
}

#[test]
fn a_bundle_that_cannot_be_written_keeps_the_exit_code_and_is_reported() {
    let dir = TempDir::new("bundle-blocked");
    let case = dir.write("failing.yaml", &wrong_expectation_case("Blocked bundle"));
    // A file where the directory should be makes creating the run directory fail.
    let blocked = dir.path().join("blocked");
    fs::write(&blocked, "not a directory").expect("the fixture file must be written");

    let output = run_ta(&[
        "--output",
        "json",
        "run",
        path_str(&case),
        "--bundle-dir",
        path_str(&blocked),
    ]);

    assert_eq!(
        exit_code(&output),
        Some(1),
        "a bundle failure must not change the exit code; stderr: {:?}",
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(value["cases"][0]["bundle"], Value::Null);
    assert!(
        value["bundle_errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty()),
        "the failure must be reported in the JSON output, got: {value}"
    );
    assert!(
        stderr(&output).contains("failure bundle"),
        "the failure must be reported on stderr, got: {:?}",
        stderr(&output)
    );
}

#[test]
fn var_overrides_a_variable_from_the_case_file() {
    let dir = TempDir::new("var");
    let inner = dir.write(
        "inner.yaml",
        &format!(
            "name: Inner\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n",
            ta()
        ),
    );
    let case = dir.write(
        "vars.yaml",
        &format!(
            r#"
name: Variables
vars:
  target: does-not-exist.yaml
steps:
  - cli.run:
      command: '{}'
      args: ["run", "${{target}}"]
    expect:
      - path: $.exit_code
        equals: 0
"#,
            ta()
        ),
    );

    let without = run_ta(&["run", path_str(&case)]);
    assert_eq!(
        exit_code(&without),
        Some(1),
        "the case file's value points at a missing file, so the step exits 3; stdout: {:?}",
        stdout(&without)
    );

    let with = run_ta(&[
        "run",
        path_str(&case),
        "--var",
        &format!("target={}", inner.display()),
    ]);
    assert_eq!(exit_code(&with), Some(0), "stderr: {:?}", stderr(&with));
}

#[test]
fn var_can_define_a_variable_the_case_file_does_not() {
    let dir = TempDir::new("var-new");
    let inner = dir.write(
        "inner.yaml",
        &format!(
            "name: Inner\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n",
            ta()
        ),
    );
    let case = dir.write(
        "vars.yaml",
        &format!(
            r#"
name: Variables from the command line
steps:
  - cli.run:
      command: '{}'
      args: ["run", "${{target}}"]
    expect:
      - path: $.exit_code
        equals: 0
"#,
            ta()
        ),
    );

    let without = run_ta(&["run", path_str(&case)]);
    assert_eq!(exit_code(&without), Some(3));
    assert!(
        stderr(&without).contains("target"),
        "got: {:?}",
        stderr(&without)
    );

    let with = run_ta(&[
        "run",
        path_str(&case),
        "--var",
        &format!("target={}", inner.display()),
    ]);
    assert_eq!(exit_code(&with), Some(0), "stderr: {:?}", stderr(&with));
}

#[test]
fn a_malformed_var_exits_two_with_the_expected_form() {
    let dir = TempDir::new("var-malformed");
    let case = dir.write(
        "passing.yaml",
        &format!(
            "name: Passing\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n",
            ta()
        ),
    );

    let missing_equals = run_ta(&["run", path_str(&case), "--var", "novalue"]);
    assert_eq!(exit_code(&missing_equals), Some(2));
    assert!(
        stderr(&missing_equals).contains("--var name=value"),
        "the message must show the expected form, got: {:?}",
        stderr(&missing_equals)
    );

    let empty_name = run_ta(&["run", path_str(&case), "--var", "=value"]);
    assert_eq!(exit_code(&empty_name), Some(2));
    assert!(
        stderr(&empty_name).contains("--var name=value"),
        "the message must show the expected form, got: {:?}",
        stderr(&empty_name)
    );
}
