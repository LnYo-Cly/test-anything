//! End-to-end tests that run the compiled `ta` binary.

mod support;

use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use support::{TempDir, TestServer, closed_port};

fn run_ta(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ta"))
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

    let mut child = Command::new(env!("CARGO_BIN_EXE_ta"))
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

    let mut child = Command::new(env!("CARGO_BIN_EXE_ta"))
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
