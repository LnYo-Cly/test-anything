//! End-to-end tests for `--vars-file`, `${env:NAME}` and secret redaction.
//!
//! Every test runs the compiled binary and sets an environment variable only on
//! that child process, never on the test process itself. The secrets are
//! obvious fake values: nothing here is a real credential.

// Every test needs at least one driver, so a build without drivers compiles an
// empty test target instead of a file whose helpers are all unused.
#![cfg(any(feature = "http", feature = "cli"))]
// Each test is gated on the feature it needs, so a build with one driver uses a
// subset of the helpers below.
#![allow(dead_code)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use support::TempDir;

/// The binary under test, as an absolute path.
fn ta() -> &'static str {
    env!("CARGO_BIN_EXE_ta")
}

/// Runs the binary with `dir` as its working directory.
fn run_ta_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(ta())
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run the ta binary")
}

/// Runs the binary with `dir` as its working directory and extra environment
/// variables, which exist only in the child process.
fn run_ta_with_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(ta());
    command.current_dir(dir).args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("failed to run the ta binary")
}

/// Runs the binary with `dir` as its working directory and `name` removed from
/// the environment, so a value set by the machine running the tests cannot make
/// an "unset" test pass or fail by accident.
fn run_ta_without_env(dir: &Path, args: &[&str], name: &str) -> Output {
    Command::new(ta())
        .current_dir(dir)
        .args(args)
        .env_remove(name)
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

/// A case that passes, so it can stand in for a variable's value.
fn passing_case(name: &str) -> String {
    format!(
        "name: {name}\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n",
        ta()
    )
}

/// A case that fails an assertion, so its exit code is 1.
fn failing_case(name: &str) -> String {
    format!(
        "name: {name}\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n    expect:\n      - path: $.exit_code\n        equals: 1\n",
        ta()
    )
}

/// Every file under `dir`, with its bytes, so a test can search a whole bundle.
fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let entries = fs::read_dir(dir).unwrap_or_else(|error| {
        panic!("failed to list {}: {error}", dir.display());
    });
    for entry in entries {
        let entry = entry.expect("failed to read a bundle entry");
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            let bytes = fs::read(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
            files.push((path, bytes));
        }
    }
    files
}

/// The bytes of every file under every directory in `dirs`, joined, so one
/// search covers text output, JSON output and every failure bundle.
fn searchable_bytes(dirs: &[&Path]) -> Vec<u8> {
    let mut all = Vec::new();
    for dir in dirs {
        for (path, bytes) in files_under(dir) {
            all.extend_from_slice(path.to_string_lossy().as_bytes());
            all.extend_from_slice(&bytes);
        }
    }
    all
}

#[test]
#[cfg(feature = "cli")]
fn vars_files_override_the_case_and_each_other_in_order() {
    let dir = TempDir::new("vars-order");
    let first = dir.write("passing-one.yaml", &passing_case("Passing one"));
    let second = dir.write("failing.yaml", &failing_case("Failing"));
    let third = dir.write("passing-two.yaml", &passing_case("Passing two"));

    // The case's own value points at a file that does not exist, so the inner
    // `ta` exits 3 and the outer assertion fails: exit 0 therefore proves that
    // a later source overrode the case's value.
    let case = dir.write(
        "case.yaml",
        &format!(
            r#"
name: Variables from files
vars:
  target: does-not-exist.yaml
steps:
  - cli.run:
      command: '{}'
      args: ["run", "${{target}}", "--no-bundle"]
    expect:
      - path: $.exit_code
        equals: 0
"#,
            ta()
        ),
    );

    let from_case_only = run_ta_in(dir.path(), &["run", path_str(&case)]);
    assert_eq!(
        exit_code(&from_case_only),
        Some(1),
        "the case's own value must lose against every later source; stdout: {:?}",
        stdout(&from_case_only)
    );

    let one_file = dir.write("one.yaml", &format!("target: {}\n", first.display()));
    let from_file = run_ta_in(
        dir.path(),
        &["run", path_str(&case), "--vars-file", path_str(&one_file)],
    );
    assert_eq!(
        exit_code(&from_file),
        Some(0),
        "the first vars file must override `vars:`; stderr: {:?}",
        stderr(&from_file)
    );

    let two_file = dir.write("two.yaml", &format!("target: {}\n", second.display()));
    let from_two_files = run_ta_in(
        dir.path(),
        &[
            "run",
            path_str(&case),
            "--vars-file",
            path_str(&one_file),
            "--vars-file",
            path_str(&two_file),
        ],
    );
    assert_eq!(
        exit_code(&from_two_files),
        Some(1),
        "the second vars file must override the first; stdout: {:?}",
        stdout(&from_two_files)
    );

    let from_var = run_ta_in(
        dir.path(),
        &[
            "run",
            path_str(&case),
            "--vars-file",
            path_str(&one_file),
            "--vars-file",
            path_str(&two_file),
            "--var",
            &format!("target={}", third.display()),
        ],
    );
    assert_eq!(
        exit_code(&from_var),
        Some(0),
        "--var must override every vars file; stderr: {:?}",
        stderr(&from_var)
    );
}

#[test]
#[cfg(feature = "cli")]
fn a_vars_file_that_cannot_be_used_exits_two_and_names_the_reason() {
    let dir = TempDir::new("vars-broken");
    let case = dir.write("passing.yaml", &passing_case("Passing"));

    let missing = run_ta_in(
        dir.path(),
        &[
            "run",
            path_str(&case),
            "--vars-file",
            path_str(&dir.path().join("nope.yaml")),
        ],
    );
    assert_eq!(exit_code(&missing), Some(2), "{}", stderr(&missing));
    assert!(
        stderr(&missing).contains("nope.yaml"),
        "the message must name the file, got: {:?}",
        stderr(&missing)
    );

    let not_a_mapping = dir.write("list.yaml", "- one\n- two\n");
    let output = run_ta_in(
        dir.path(),
        &[
            "run",
            path_str(&case),
            "--vars-file",
            path_str(&not_a_mapping),
        ],
    );
    assert_eq!(exit_code(&output), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("list.yaml"),
        "the message must name the file, got: {:?}",
        stderr(&output)
    );

    let wrong_type = dir.write("wrong.yaml", "token:\n  - one\n  - two\n");
    let output = run_ta_in(
        dir.path(),
        &["run", path_str(&case), "--vars-file", path_str(&wrong_type)],
    );
    assert_eq!(exit_code(&output), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("wrong.yaml") && stderr(&output).contains("token"),
        "the message must name the file and the variable, got: {:?}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("line 2"),
        "a value error must carry its location, got: {:?}",
        stderr(&output)
    );

    let bad_grammar = dir.write("grammar.yaml", "token: [\n");
    let output = run_ta_in(
        dir.path(),
        &[
            "run",
            path_str(&case),
            "--vars-file",
            path_str(&bad_grammar),
        ],
    );
    assert_eq!(exit_code(&output), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("grammar.yaml"),
        "got: {:?}",
        stderr(&output)
    );
}

#[test]
#[cfg(feature = "cli")]
fn lint_accepts_a_vars_file_and_reports_the_same_problems() {
    let dir = TempDir::new("vars-lint");
    let case = dir.write(
        "case.yaml",
        &format!(
            r#"
name: Variables from a file
steps:
  - cli.run:
      command: '{}'
      args: ["run", "${{target}}", "--no-bundle"]
"#,
            ta()
        ),
    );
    let inner = dir.write("inner.yaml", &passing_case("Inner"));
    let vars = dir.write("vars.yaml", &format!("target: {}\n", inner.display()));

    let without = run_ta_in(dir.path(), &["lint", path_str(&case)]);
    assert_eq!(exit_code(&without), Some(3), "{}", stdout(&without));
    assert!(
        stdout(&without).contains("target"),
        "got: {:?}",
        stdout(&without)
    );

    let with = run_ta_in(
        dir.path(),
        &["lint", path_str(&case), "--vars-file", path_str(&vars)],
    );
    assert_eq!(exit_code(&with), Some(0), "{}", stdout(&with));
}

#[test]
#[cfg(feature = "cli")]
fn env_references_work_in_step_arguments_and_in_variable_values() {
    let dir = TempDir::new("env-refs");
    let case = dir.write(
        "case.yaml",
        &format!(
            r#"
name: Environment references
vars:
  from_vars: "${{env:TA_TEST_VARS_ENV}}"
steps:
  - cli.run:
      command: '{ta}'
      args: ["run", "missing-${{from_vars}}.yaml", "--no-bundle"]
    expect:
      - path: $.stderr
        contains: "missing-alpha.yaml"
  - cli.run:
      command: '{ta}'
      args: ["run", "direct-${{env:TA_TEST_STEP_ENV}}.yaml", "--no-bundle"]
    expect:
      - path: $.stderr
        contains: "direct-beta.yaml"
"#,
            ta = ta()
        ),
    );

    let output = run_ta_with_env(
        dir.path(),
        &["run", path_str(&case)],
        &[("TA_TEST_VARS_ENV", "alpha"), ("TA_TEST_STEP_ENV", "beta")],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
}

#[test]
#[cfg(feature = "cli")]
fn a_vars_file_value_can_reference_the_environment() {
    let dir = TempDir::new("env-vars-file");
    let case = dir.write(
        "case.yaml",
        &format!(
            r#"
name: Environment reference in a vars file
steps:
  - cli.run:
      command: '{}'
      args: ["run", "missing-${{target}}.yaml", "--no-bundle"]
    expect:
      - path: $.stderr
        contains: "missing-gamma.yaml"
"#,
            ta()
        ),
    );
    let vars = dir.write("vars.yaml", "target: \"${env:TA_TEST_FILE_ENV}\"\n");

    let output = run_ta_with_env(
        dir.path(),
        &["run", path_str(&case), "--vars-file", path_str(&vars)],
        &[("TA_TEST_FILE_ENV", "gamma")],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
}

#[test]
#[cfg(feature = "cli")]
fn an_unset_environment_variable_is_a_case_error_in_run_and_lint() {
    let dir = TempDir::new("env-unset");
    let case = dir.write(
        "case.yaml",
        &format!(
            r#"
name: Missing environment variable
steps:
  - cli.run:
      command: '{}'
      args: ["run", "missing-${{env:TA_TEST_UNSET_ENV}}.yaml", "--no-bundle"]
"#,
            ta()
        ),
    );

    let run = run_ta_without_env(dir.path(), &["run", path_str(&case)], "TA_TEST_UNSET_ENV");
    assert_eq!(exit_code(&run), Some(3), "stdout: {:?}", stdout(&run));
    assert!(
        stderr(&run).contains("TA_TEST_UNSET_ENV"),
        "the message must name the variable, got: {:?}",
        stderr(&run)
    );
    assert!(
        stderr(&run).contains("set it before running"),
        "the message must say what to do, got: {:?}",
        stderr(&run)
    );

    let lint = run_ta_without_env(dir.path(), &["lint", path_str(&case)], "TA_TEST_UNSET_ENV");
    assert_eq!(exit_code(&lint), Some(3), "stdout: {:?}", stdout(&lint));
    assert!(
        stdout(&lint).contains("TA_TEST_UNSET_ENV"),
        "lint must name the variable, got: {:?}",
        stdout(&lint)
    );

    let vars = dir.write("vars.yaml", "target: \"${env:TA_TEST_UNSET_ENV}\"\n");
    let from_vars_file = run_ta_without_env(
        dir.path(),
        &["run", path_str(&case), "--vars-file", path_str(&vars)],
        "TA_TEST_UNSET_ENV",
    );
    assert_eq!(
        exit_code(&from_vars_file),
        Some(3),
        "an unset reference in a vars file is a case error too; stderr: {:?}",
        stderr(&from_vars_file)
    );
    assert!(
        stderr(&from_vars_file).contains("TA_TEST_UNSET_ENV"),
        "got: {:?}",
        stderr(&from_vars_file)
    );
}

#[test]
#[cfg(feature = "cli")]
fn an_empty_environment_variable_counts_as_set() {
    let dir = TempDir::new("env-empty");
    let case = dir.write(
        "case.yaml",
        &format!(
            r#"
name: Empty environment variable
steps:
  - cli.run:
      command: '{}'
      args: ["run", "missing${{env:TA_TEST_EMPTY_ENV}}.yaml", "--no-bundle"]
    expect:
      - path: $.stderr
        contains: "missing.yaml"
"#,
            ta()
        ),
    );

    let output = run_ta_with_env(
        dir.path(),
        &["run", path_str(&case)],
        &[("TA_TEST_EMPTY_ENV", "")],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {:?}", stderr(&output));
}

#[test]
#[cfg(feature = "cli")]
fn an_invalid_environment_variable_name_is_a_case_error() {
    let dir = TempDir::new("env-invalid");

    for (label, reference) in [("empty", "${env:}"), ("bad", "${env:TA-BAD-NAME}")] {
        let case = dir.write(
            &format!("{label}.yaml"),
            &format!(
                "name: Invalid reference\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\", \"{reference}\"]\n",
                ta()
            ),
        );

        let output = run_ta_in(dir.path(), &["run", path_str(&case)]);
        assert_eq!(
            exit_code(&output),
            Some(3),
            "{label}: stdout: {:?}, stderr: {:?}",
            stdout(&output),
            stderr(&output)
        );
    }
}

#[test]
#[cfg(feature = "cli")]
fn a_variable_from_the_command_line_is_not_redacted() {
    let dir = TempDir::new("var-visible");
    let case = dir.write(
        "case.yaml",
        &format!(
            r#"
name: A visible variable
steps:
  - cli.run:
      command: '{}'
      args: ["run", "missing-${{probe}}.yaml", "--no-bundle"]
    expect:
      - path: $.stderr
        equals: "nothing"
"#,
            ta()
        ),
    );

    let output = run_ta_in(
        dir.path(),
        &["run", path_str(&case), "--var", "probe=visible-4242"],
    );

    assert_eq!(exit_code(&output), Some(1), "stderr: {:?}", stderr(&output));
    assert!(
        stdout(&output).contains("visible-4242"),
        "a `--var` value is not a secret and must stay readable, got: {:?}",
        stdout(&output)
    );
}

/// The gate for this task: a secret that reaches a step result and an error
/// message must not appear in any output channel or in any bundle file.
///
/// The case prints the secret on a child process's standard output (an inner
/// `ta` echoes it), puts it in a request header that the local server echoes
/// back, and then fails, so a bundle is written.
#[test]
#[cfg(all(feature = "cli", feature = "http"))]
fn a_secret_from_the_environment_never_reaches_output_or_a_bundle() {
    const SECRET: &str = "s3cr3t-TA-LEAK-4242";
    const NAME: &str = "TA_TEST_LEAK_SECRET";

    let server = support::TestServer::start();
    let dir = TempDir::new("leak-gate");

    // The inner case makes an inner `ta` print the secret on its standard
    // output: the outer step's result therefore carries it.
    let inner = dir.write(
        "inner.yaml",
        &format!(
            "name: Inner\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"run\", \"missing-${{probe}}.yaml\", \"--no-bundle\"]\n",
            ta()
        ),
    );
    let case = dir.write(
        "leak.yaml",
        &format!(
            r#"
name: Leaks the token
steps:
  - cli.run:
      command: '{ta}'
      args: ["--output", "json", "run", '{inner}', "--var", "probe=${{env:{name}}}", "--no-bundle"]
  - http.request:
      url: {base}/echo
      headers: {{ authorization: "Bearer ${{env:{name}}}" }}
    expect:
      - path: $.body.headers.authorization
        equals: "Bearer nope"
"#,
            ta = ta(),
            inner = inner.display(),
            name = NAME,
            base = server.url("")
        ),
    );

    let text_dir = dir.path().join("bundles-text");
    let text = run_ta_with_env(
        dir.path(),
        &["run", path_str(&case), "--bundle-dir", path_str(&text_dir)],
        &[(NAME, SECRET)],
    );
    assert_eq!(exit_code(&text), Some(1), "stderr: {:?}", stderr(&text));

    let json_dir = dir.path().join("bundles-json");
    let json = run_ta_with_env(
        dir.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(&case),
            "--bundle-dir",
            path_str(&json_dir),
        ],
        &[(NAME, SECRET)],
    );
    assert_eq!(exit_code(&json), Some(1), "stderr: {:?}", stderr(&json));

    let text_stdout = stdout(&text);
    let text_stderr = stderr(&text);
    let json_stdout = stdout(&json);
    let json_stderr = stderr(&json);
    for (channel, content) in [
        ("text stdout", &text_stdout),
        ("text stderr", &text_stderr),
        ("json stdout", &json_stdout),
        ("json stderr", &json_stderr),
    ] {
        assert!(
            !content.contains(SECRET),
            "{channel} leaked the secret: {content}"
        );
    }
    for (channel, content) in [("text stdout", &text_stdout), ("json stdout", &json_stdout)] {
        assert!(
            content.contains("***"),
            "{channel} must show the redaction marker: {content}"
        );
    }

    for dir in [&text_dir, &json_dir] {
        for (path, bytes) in files_under(dir) {
            let text = String::from_utf8_lossy(&bytes);
            assert!(
                !text.contains(SECRET),
                "{} leaked the secret: {text}",
                path.display()
            );
        }
    }
    let bundles = searchable_bytes(&[&text_dir, &json_dir]);
    assert!(
        String::from_utf8_lossy(&bundles).contains("***"),
        "the bundles must contain the redaction marker"
    );
}

/// The same gate for an error message: a preflight failure names the argument
/// that was rejected, and that argument contains the secret.
#[test]
#[cfg(feature = "http")]
fn a_secret_never_appears_in_an_error_message() {
    const SECRET: &str = "s3cr3t-TA-ERROR-7731";
    const NAME: &str = "TA_TEST_ERROR_SECRET";

    let dir = TempDir::new("leak-error");
    let case = dir.write(
        "bad-url.yaml",
        &format!(
            "name: Bad url\nsteps:\n  - http.request:\n      url: \"nope://${{env:{name}}}\"\n",
            name = NAME
        ),
    );

    let text = run_ta_with_env(dir.path(), &["run", path_str(&case)], &[(NAME, SECRET)]);
    assert_eq!(exit_code(&text), Some(3), "stdout: {:?}", stdout(&text));
    assert!(
        !stderr(&text).contains(SECRET),
        "text error leaked the secret: {:?}",
        stderr(&text)
    );
    assert!(
        stderr(&text).contains("***"),
        "text error must show the marker: {:?}",
        stderr(&text)
    );

    let json = run_ta_with_env(
        dir.path(),
        &["--output", "json", "run", path_str(&case)],
        &[(NAME, SECRET)],
    );
    assert_eq!(exit_code(&json), Some(3));
    assert!(
        !stdout(&json).contains(SECRET),
        "json error leaked the secret: {:?}",
        stdout(&json)
    );
    assert!(
        stdout(&json).contains("***"),
        "json error must show the marker: {:?}",
        stdout(&json)
    );
}
