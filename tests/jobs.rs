//! End-to-end tests for `--jobs`: cases that run at the same time.
//!
//! Every test runs the compiled binary. The cases that need to take time wait on
//! a local server that the test starts itself, so nothing here reaches the
//! network and no system command is required.

// Every case here starts a program, so a build without the `cli` driver has
// nothing to run: it compiles an empty test target instead of a file whose
// helpers would all be unused.
#![cfg(feature = "cli")]
// A build with one driver uses a subset of the helpers below.
#![allow(dead_code)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use support::TempDir;

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

/// Runs the binary with `dir` as its working directory and extra environment
/// variables, which exist only in the child process.
fn run_da_with_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(da());
    command.current_dir(dir).args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("failed to run the da binary")
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
    serde_json::from_str(&stdout(output)).unwrap_or_else(|error| {
        panic!(
            "standard output must be one JSON object: {error}\nstdout:\n{}\nstderr:\n{}",
            stdout(output),
            stderr(output)
        )
    })
}

/// A case that passes, so it never gets a failure bundle.
fn passing_case(name: &str) -> String {
    format!(
        "name: {name}\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n",
        da()
    )
}

/// A case that fails an assertion, so its status is `failed`.
fn failing_case(name: &str) -> String {
    format!(
        "name: {name}\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n    expect:\n      - path: $.exit_code\n        equals: 1\n",
        da()
    )
}

/// A case that could not run a step, so its status is `error`.
///
/// The program it names cannot exist, so the step reports an environment error
/// instead of a failed assertion, and the run's exit code is 4.
fn error_case(name: &str) -> String {
    format!(
        "name: {name}\nsteps:\n  - cli.run:\n      command: 'da-no-such-program-4242'\n      timeout_ms: 5000\n"
    )
}

/// The parts of a result that legitimately differ between two runs of the same
/// cases: how long a case or a step took, when a bundle was written, and the
/// timestamped run directory a bundle lives in. Everything else must match.
fn normalize(value: &Value) -> Value {
    match value {
        Value::Object(entries) => {
            let mut normalized = serde_json::Map::new();
            for (key, item) in entries {
                match key.as_str() {
                    "duration_ms" | "created_at" => continue,
                    "bundle" => {
                        let item = item
                            .as_str()
                            .map_or_else(|| item.clone(), |path| Value::from(last_segment(path)));
                        normalized.insert(key.clone(), item);
                    }
                    _ => {
                        normalized.insert(key.clone(), normalize(item));
                    }
                }
            }
            Value::Object(normalized)
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

/// The last `/`-separated part of a path, which is the bundle's own name.
fn last_segment(path: &str) -> String {
    path.rsplit('/').next().unwrap_or_default().to_string()
}

/// The directories directly under `dir`, sorted; empty when it does not exist.
fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return directories;
    };
    for entry in entries {
        let path = entry.expect("a bundle entry must be readable").path();
        if path.is_dir() {
            directories.push(path);
        }
    }
    directories.sort();
    directories
}

/// Every failure bundle under `dir`, keyed by its case directory name.
///
/// The run directory above the case directories carries a timestamp and a
/// process id, so it is dropped: what is left is comparable between two runs.
fn bundles(dir: &Path) -> BTreeMap<String, Value> {
    let mut found = BTreeMap::new();
    for run in subdirectories(dir) {
        for case in subdirectories(&run) {
            let name = case
                .file_name()
                .expect("a bundle directory has a name")
                .to_string_lossy()
                .into_owned();
            let read = |file: &str| -> Value {
                let path = case.join(file);
                let text = fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
                serde_json::from_str(&text)
                    .unwrap_or_else(|error| panic!("{} must be JSON: {error}", path.display()))
            };
            found.insert(
                name,
                json!({"meta": normalize(&read("meta.json")), "report": normalize(&read("report.json"))}),
            );
        }
    }
    found
}

/// The file names inside one bundle directory, sorted.
fn bundle_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to list {}: {error}", dir.display()))
        .map(|entry| {
            entry
                .expect("a bundle file must be readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
#[cfg(feature = "cli")]
fn a_job_count_below_one_or_not_a_number_exits_two() {
    let dir = TempDir::new("jobs-usage");
    let case = dir.write("passing.yaml", &passing_case("Passing"));

    // A value that is not a whole number at all, then one that is too small.
    // `-1` is also checked, but the parser may read it as an unknown option, so
    // its message is not required to name `--jobs`.
    for (value, names_the_option) in [
        ("0", true),
        ("two", true),
        ("1.5", true),
        ("", true),
        ("-1", false),
    ] {
        let output = run_da_in(dir.path(), &["run", path_str(&case), "--jobs", value]);
        assert_eq!(
            exit_code(&output),
            Some(2),
            "--jobs {value:?} must be a usage error; stdout: {:?}, stderr: {:?}",
            stdout(&output),
            stderr(&output)
        );
        if names_the_option {
            assert!(
                stderr(&output).contains("--jobs"),
                "--jobs {value:?}: the message must name the option, got: {:?}",
                stderr(&output)
            );
        }
        assert!(
            !stdout(&output).contains("PASS"),
            "--jobs {value:?}: no case may run, got: {:?}",
            stdout(&output)
        );
    }

    // `--jobs` belongs to `run` only.
    let lint = run_da_in(dir.path(), &["lint", path_str(&case), "--jobs", "2"]);
    assert_eq!(
        exit_code(&lint),
        Some(2),
        "`da lint --jobs` must be a usage error, stderr: {:?}",
        stderr(&lint)
    );

    let accepted = run_da_in(dir.path(), &["run", path_str(&case), "--jobs", "2"]);
    assert_eq!(
        exit_code(&accepted),
        Some(0),
        "a valid job count must run the cases, stderr: {:?}",
        stderr(&accepted)
    );
}

#[test]
#[cfg(feature = "cli")]
fn the_default_is_one_job() {
    let dir = TempDir::new("jobs-default");
    dir.write("cases/a-passes.yaml", &passing_case("A passes"));
    dir.write("cases/b-fails.yaml", &failing_case("B fails"));

    let without = run_da_in(
        dir.path(),
        &["--output", "json", "run", "cases", "--no-bundle"],
    );
    let one = run_da_in(
        dir.path(),
        &[
            "--output",
            "json",
            "run",
            "cases",
            "--jobs",
            "1",
            "--no-bundle",
        ],
    );

    assert_eq!(exit_code(&without), Some(1), "{}", stderr(&without));
    assert_eq!(exit_code(&one), Some(1), "{}", stderr(&one));
    assert_eq!(
        normalize(&json_stdout(&without)),
        normalize(&json_stdout(&one)),
        "omitting --jobs must be the same as --jobs 1"
    );
}

#[test]
#[cfg(feature = "cli")]
fn jobs_four_and_jobs_one_report_the_same_result() {
    let dir = TempDir::new("jobs-same");
    // Discovery order is path order, so the four statuses are known in advance.
    dir.write("cases/a-passes.yaml", &passing_case("A passes"));
    dir.write("cases/b-fails.yaml", &failing_case("B fails"));
    dir.write("cases/c-errors.yaml", &error_case("C errors"));
    dir.write("cases/d-passes.yaml", &passing_case("D passes"));
    let serial_bundles = dir.path().join("bundles-one");
    let parallel_bundles = dir.path().join("bundles-four");

    let serial = run_da_in(
        dir.path(),
        &[
            "--output",
            "json",
            "run",
            "cases",
            "--jobs",
            "1",
            "--bundle-dir",
            path_str(&serial_bundles),
        ],
    );
    let parallel = run_da_in(
        dir.path(),
        &[
            "--output",
            "json",
            "run",
            "cases",
            "--jobs",
            "4",
            "--bundle-dir",
            path_str(&parallel_bundles),
        ],
    );

    // The environment error is the most fundamental problem, so it decides the
    // exit code in both runs.
    assert_eq!(exit_code(&serial), Some(4), "stderr: {:?}", stderr(&serial));
    assert_eq!(
        exit_code(&parallel),
        exit_code(&serial),
        "the exit code must not depend on the job count; stderr: {:?}",
        stderr(&parallel)
    );

    let serial_json = normalize(&json_stdout(&serial));
    let parallel_json = normalize(&json_stdout(&parallel));
    assert_eq!(
        parallel_json, serial_json,
        "both runs must report the same cases, in the same order"
    );

    let names: Vec<&str> = parallel_json["cases"]
        .as_array()
        .expect("cases must be a list")
        .iter()
        .map(|case| case["name"].as_str().expect("a case has a name"))
        .collect();
    assert_eq!(
        names,
        vec!["A passes", "B fails", "C errors", "D passes"],
        "the cases must be reported in discovery order, not completion order"
    );
    let statuses: Vec<&str> = parallel_json["cases"]
        .as_array()
        .expect("cases must be a list")
        .iter()
        .map(|case| case["status"].as_str().expect("a case has a status"))
        .collect();
    assert_eq!(statuses, vec!["passed", "failed", "error", "passed"]);

    // The two non-passing cases get one bundle each, named after their position
    // in the run, and neither run loses or duplicates one.
    let four = bundles(&parallel_bundles);
    assert_eq!(
        four.keys().cloned().collect::<Vec<String>>(),
        vec!["1-B-fails".to_string(), "2-C-errors".to_string()],
        "one directory per non-passing case, numbered by run position"
    );
    for (name, bundle) in &four {
        let case = &name[2..];
        assert_eq!(bundle["meta"]["case_name"], json!(case.replace('-', " ")));
        assert_eq!(bundle["report"]["name"], json!(case.replace('-', " ")));
    }
    assert_eq!(
        four,
        bundles(&serial_bundles),
        "the bundles must hold the same files with the same content"
    );
    for case in subdirectories(&parallel_bundles) {
        for bundle in subdirectories(&case) {
            assert_eq!(
                bundle_files(&bundle),
                vec![
                    "case.yaml".to_string(),
                    "meta.json".to_string(),
                    "report.json".to_string()
                ],
                "a bundle must be complete: {}",
                bundle.display()
            );
        }
    }
}

/// Four cases that each wait about a second must overlap under `--jobs 4`.
///
/// The comparison is a ratio between two runs of the same binary on the same
/// machine, never an absolute ceiling: a loaded machine slows both runs down.
#[test]
#[cfg(all(feature = "cli", feature = "http"))]
fn jobs_four_finishes_much_sooner_than_jobs_one() {
    let server = support::TestServer::start();
    let dir = TempDir::new("jobs-timing");
    // The child is `da` itself, waiting on a local service that answers slowly.
    dir.write(
        "cases/slow.case",
        &format!(
            "name: Slow\nsteps:\n  - http.request:\n      url: {base}/sleep\n      timeout_ms: 20000\n",
            base = server.url("")
        ),
    );
    for name in ["a", "b", "c", "d"] {
        dir.write(
            &format!("cases/{name}.yaml"),
            &format!(
                r#"
name: Sleeps {name}
steps:
  - cli.run:
      command: '{da}'
      args: ["run", "cases/slow.case", "--no-bundle"]
    expect:
      - path: $.exit_code
        equals: 0
"#,
                da = da()
            ),
        );
    }

    let started = std::time::Instant::now();
    let serial = run_da_in(dir.path(), &["run", "cases", "--jobs", "1"]);
    let serial_elapsed = started.elapsed();

    let started = std::time::Instant::now();
    let parallel = run_da_in(dir.path(), &["run", "cases", "--jobs", "4"]);
    let parallel_elapsed = started.elapsed();

    assert_eq!(
        exit_code(&serial),
        Some(0),
        "stdout: {:?}, stderr: {:?}",
        stdout(&serial),
        stderr(&serial)
    );
    assert_eq!(
        exit_code(&parallel),
        Some(0),
        "stdout: {:?}, stderr: {:?}",
        stdout(&parallel),
        stderr(&parallel)
    );
    // The four waits cannot add up to less than a lower bound, however fast the
    // machine is, so this proves the serial run really waited four times.
    assert!(
        serial_elapsed >= support::SLEEP_ROUTE_DELAY * 2,
        "the serial run must wait for every case in turn, took {serial_elapsed:?}"
    );
    assert!(
        parallel_elapsed * 10 < serial_elapsed * 6,
        "four sleeping cases must overlap under --jobs 4: {parallel_elapsed:?} against {serial_elapsed:?}"
    );
}

/// The secret gate of the variable task, with the cases running at once.
///
/// Three cases leak the same environment value into a step result and an error
/// message and then fail, so each writes a bundle. Whatever a worker thread is
/// doing, no value that came from `${env:...}` may reach any output or any file.
#[test]
#[cfg(all(feature = "cli", feature = "http"))]
fn secrets_stay_redacted_when_cases_run_at_the_same_time() {
    const SECRET: &str = "s3cr3t-DA-JOBS-9191";
    const NAME: &str = "DA_TEST_JOBS_SECRET";

    let server = support::TestServer::start();
    let dir = TempDir::new("jobs-secrets");
    let inner = dir.write(
        "inner.yaml",
        &format!(
            "name: Inner\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"run\", \"missing-${{probe}}.yaml\", \"--no-bundle\"]\n",
            da()
        ),
    );
    for case in 0..3 {
        dir.write(
            &format!("cases/leak-{case}.yaml"),
            &format!(
                r#"
name: Leaks the token {case}
steps:
  - cli.run:
      command: '{da}'
      args: ["--output", "json", "run", '{inner}', "--var", "probe=${{env:{name}}}", "--no-bundle"]
  - http.request:
      url: {base}/echo
      headers: {{ authorization: "Bearer ${{env:{name}}}" }}
    expect:
      - path: $.body.headers.authorization
        equals: "Bearer nope"
"#,
                da = da(),
                inner = inner.display(),
                name = NAME,
                base = server.url("")
            ),
        );
    }
    let environment = [(NAME, SECRET)];

    let text_bundles = dir.path().join("bundles-text");
    let text = run_da_with_env(
        dir.path(),
        &[
            "run",
            "cases",
            "--jobs",
            "3",
            "--bundle-dir",
            path_str(&text_bundles),
        ],
        &environment,
    );
    assert_eq!(exit_code(&text), Some(1), "stderr: {:?}", stderr(&text));

    let json_bundles = dir.path().join("bundles-json");
    let json = run_da_with_env(
        dir.path(),
        &[
            "--output",
            "json",
            "run",
            "cases",
            "--jobs",
            "3",
            "--bundle-dir",
            path_str(&json_bundles),
        ],
        &environment,
    );
    assert_eq!(exit_code(&json), Some(1), "stderr: {:?}", stderr(&json));

    for (channel, content) in [
        ("text stdout", stdout(&text)),
        ("text stderr", stderr(&text)),
        ("json stdout", stdout(&json)),
        ("json stderr", stderr(&json)),
    ] {
        assert!(
            !content.contains(SECRET),
            "{channel} leaked the secret: {content}"
        );
    }
    for (channel, content) in [
        ("text stdout", stdout(&text)),
        ("json stdout", stdout(&json)),
    ] {
        assert!(
            content.contains("Bearer ***"),
            "{channel} must redact the header value: {content}"
        );
    }

    let value = json_stdout(&json);
    let cases = value["cases"].as_array().expect("cases must be a list");
    assert_eq!(cases.len(), 3, "every case must be reported: {value}");
    assert!(
        !cases.iter().any(|case| case["bundle"].is_null()),
        "each failing case must get its own bundle: {value}"
    );

    for dir in [&text_bundles, &json_bundles] {
        let bundles = bundles(dir);
        assert_eq!(bundles.len(), 3, "one bundle per failing case");
        for (name, bundle) in bundles {
            let text = bundle.to_string();
            assert!(!text.contains(SECRET), "bundle {name} leaked the secret");
            assert!(text.contains("***"), "bundle {name} must be redacted");
        }
    }
}
