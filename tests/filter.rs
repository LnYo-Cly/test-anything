//! End-to-end tests for case classification and `da run` filters.
//!
//! Every case here uses the `cli.run` step, so the whole target is gated on the
//! `cli` feature: a build without that driver cannot accept any case, which is
//! the point of the no-driver tests elsewhere rather than a filter regression.

#![cfg(feature = "cli")]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
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

/// A passing case that uses a registered step.
fn passing_case(name: &str, classification: &str) -> String {
    format!(
        "name: {name}\n{classification}steps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n",
        da()
    )
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
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout must contain exactly one JSON value: {error}\nstdout: {}\nstderr: {}",
            stdout(output),
            stderr(output)
        )
    })
}

/// The case names of a JSON run report, in run order.
fn case_names(value: &Value) -> Vec<String> {
    value["cases"]
        .as_array()
        .expect("cases must be an array")
        .iter()
        .map(|case| {
            case["name"]
                .as_str()
                .expect("every case has a name")
                .to_string()
        })
        .collect()
}

#[test]
fn each_filter_option_selects_the_matching_cases() {
    let work = TempDir::new("filter-each-option");
    work.write(
        "a.yaml",
        &passing_case(
            "api smoke",
            "project: shop\nsuite: smoke\nlayer: api\ntags: [smoke, fast]\n",
        ),
    );
    work.write(
        "b.yaml",
        &passing_case(
            "cli regression",
            "project: billing\nsuite: regression\nlayer: cli\ntags: [slow]\n",
        ),
    );

    let cases = [
        ("--project", "shop", vec!["api smoke"]),
        ("--suite", "smoke", vec!["api smoke"]),
        ("--layer", "cli", vec!["cli regression"]),
        ("--tag", "fast", vec!["api smoke"]),
    ];

    for (option, value, expected) in cases {
        let output = run_da_in(
            work.path(),
            &[
                "--output",
                "json",
                "run",
                path_str(work.path()),
                "--no-bundle",
                option,
                value,
            ],
        );

        assert_eq!(
            exit_code(&output),
            Some(0),
            "{option} {value} failed; stderr: {}",
            stderr(&output)
        );
        let report = json_stdout(&output);
        assert_eq!(case_names(&report), expected, "{option} {value}");
        assert_eq!(report["summary"]["filtered"], 1, "{option} {value}");
    }
}

#[test]
fn one_option_uses_or_and_different_options_use_and() {
    let work = TempDir::new("filter-combine");
    work.write(
        "a.yaml",
        &passing_case("api smoke", "layer: api\ntags: [smoke]\n"),
    );
    work.write(
        "b.yaml",
        &passing_case("cli smoke", "layer: cli\ntags: [smoke]\n"),
    );
    work.write(
        "c.yaml",
        &passing_case("api regression", "layer: api\ntags: [regression]\n"),
    );

    let output = run_da_in(
        work.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(work.path()),
            "--no-bundle",
            "--layer",
            "api",
            "--layer",
            "cli",
            "--tag",
            "smoke",
        ],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
    let report = json_stdout(&output);
    assert_eq!(case_names(&report), vec!["api smoke", "cli smoke"]);
    assert_eq!(report["summary"]["filtered"], 1);
    assert_eq!(report["summary"]["cases"], 2);
}

#[test]
fn skip_tag_excludes_a_case_that_another_filter_selected() {
    let work = TempDir::new("filter-skip-tag");
    work.write(
        "a.yaml",
        &passing_case("quick", "layer: api\ntags: [smoke]\n"),
    );
    work.write(
        "b.yaml",
        &passing_case("slow", "layer: api\ntags: [smoke, slow]\n"),
    );

    let output = run_da_in(
        work.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(work.path()),
            "--no-bundle",
            "--tag",
            "smoke",
            "--skip-tag",
            "slow",
        ],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
    let report = json_stdout(&output);
    assert_eq!(case_names(&report), vec!["quick"]);
    assert_eq!(report["summary"]["filtered"], 1);
}

#[test]
fn a_case_without_the_field_is_not_selected_by_that_field() {
    let work = TempDir::new("filter-missing-field");
    work.write("a.yaml", &passing_case("unclassified", ""));
    work.write("b.yaml", &passing_case("api", "layer: api\n"));

    let output = run_da_in(
        work.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(work.path()),
            "--no-bundle",
            "--layer",
            "api",
        ],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
    let report = json_stdout(&output);
    assert_eq!(case_names(&report), vec!["api"]);
    assert_eq!(report["summary"]["filtered"], 1);
}

#[test]
fn an_empty_selection_exits_two_and_lists_the_available_values() {
    let work = TempDir::new("filter-no-match");
    work.write(
        "a.yaml",
        &passing_case(
            "api smoke",
            "project: shop\nsuite: smoke\nlayer: api\ntags: [smoke, fast]\n",
        ),
    );

    let output = run_da_in(
        work.path(),
        &[
            "run",
            path_str(work.path()),
            "--no-bundle",
            "--tag",
            "missing",
        ],
    );

    assert_eq!(exit_code(&output), Some(2), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).is_empty(),
        "a rejected selection is a diagnostic, got: {}",
        stdout(&output)
    );
    let message = stderr(&output);
    assert!(message.contains("--tag \"missing\""), "got: {message}");
    for available in ["`smoke`", "`fast`", "`shop`", "`api`"] {
        assert!(
            message.contains(available),
            "missing {available} in {message}"
        );
    }

    let json = run_da_in(
        work.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(work.path()),
            "--no-bundle",
            "--tag",
            "missing",
        ],
    );

    assert_eq!(exit_code(&json), Some(2), "stderr: {}", stderr(&json));
    let report = json_stdout(&json);
    assert_eq!(report["ok"], false);
    assert_eq!(report["cases"], Value::Array(Vec::new()));
    assert_eq!(report["summary"]["cases"], 0);
    assert!(
        report["filter_error"]
            .as_str()
            .is_some_and(|message| message.contains("`smoke`")),
        "got: {report}"
    );
}

#[test]
fn a_filtered_out_invalid_case_still_exits_three() {
    let work = TempDir::new("filter-invalid");
    work.write("a-good.yaml", &passing_case("good", "layer: api\n"));
    work.write(
        "b-broken.yaml",
        "name: broken\nlayer: cli\nsteps:\n  - shell.run:\n      command: echo hi\n",
    );

    let unknown_step = run_da_in(
        work.path(),
        &[
            "run",
            path_str(work.path()),
            "--no-bundle",
            "--layer",
            "api",
        ],
    );

    assert_eq!(
        exit_code(&unknown_step),
        Some(3),
        "a filter must not hide an invalid case; stderr: {}",
        stderr(&unknown_step)
    );
    assert!(
        stderr(&unknown_step).contains("shell.run"),
        "got: {}",
        stderr(&unknown_step)
    );
    assert!(
        stdout(&unknown_step).is_empty(),
        "nothing may run, got: {}",
        stdout(&unknown_step)
    );

    work.write(
        "c-syntax.yaml",
        "name: unclosed\nlayer: cli\nsteps:\n  - cli.run: [\n",
    );
    let syntax = run_da_in(
        work.path(),
        &[
            "run",
            path_str(work.path()),
            "--no-bundle",
            "--layer",
            "api",
        ],
    );

    assert_eq!(
        exit_code(&syntax),
        Some(3),
        "a filter must not hide a syntax error; stderr: {}",
        stderr(&syntax)
    );
}

#[test]
fn duplicate_tags_and_empty_classification_values_exit_three() {
    let work = TempDir::new("filter-invalid-classification");
    work.write(
        "duplicate.yaml",
        &passing_case("duplicate", "tags: [smoke, smoke]\n"),
    );
    work.write(
        "empty-project.yaml",
        &passing_case("empty project", "project: \"\"\n"),
    );
    work.write(
        "empty-tag.yaml",
        &passing_case("empty tag", "tags: [\"\"]\n"),
    );

    let cases = [
        ("duplicate.yaml", "twice"),
        ("empty-project.yaml", "project"),
        ("empty-tag.yaml", "tags"),
    ];

    for (file, expected) in cases {
        let output = run_da_in(
            work.path(),
            &["run", path_str(&work.path().join(file)), "--no-bundle"],
        );

        assert_eq!(
            exit_code(&output),
            Some(3),
            "{file} must be invalid; stderr: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains(expected),
            "{file} must mention {expected}, got: {}",
            stderr(&output)
        );
    }
}

#[test]
fn json_output_carries_the_classification_of_each_selected_case() {
    let work = TempDir::new("filter-json-fields");
    work.write(
        "a.yaml",
        &passing_case(
            "classified",
            "project: shop\nsuite: smoke\nlayer: api\ntags: [smoke, fast]\n",
        ),
    );
    work.write("b.yaml", &passing_case("plain", ""));
    let path = work.path().join("a.yaml");

    let output = run_da_in(
        work.path(),
        &["--output", "json", "run", path_str(&path), "--no-bundle"],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
    let report = json_stdout(&output);
    let case = &report["cases"][0];
    assert_eq!(case["project"], "shop");
    assert_eq!(case["suite"], "smoke");
    assert_eq!(case["layer"], "api");
    assert_eq!(case["tags"], serde_json::json!(["smoke", "fast"]));
    assert_eq!(report["summary"]["filtered"], 0);
}

#[test]
fn a_plain_case_reports_null_classification_and_empty_tags() {
    let work = TempDir::new("filter-json-empty");
    work.write("plain.yaml", &passing_case("plain", ""));

    let output = run_da_in(
        work.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(work.path()),
            "--no-bundle",
        ],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
    let report = json_stdout(&output);
    let case = &report["cases"][0];
    assert_eq!(case["project"], Value::Null);
    assert_eq!(case["suite"], Value::Null);
    assert_eq!(case["layer"], Value::Null);
    assert_eq!(case["tags"], serde_json::json!([]));
}

#[test]
fn a_failure_bundle_meta_json_carries_the_classification() {
    let work = TempDir::new("filter-bundle");
    let failing = work.write(
        "failing.yaml",
        &format!(
            "name: failing\nproject: shop\nsuite: smoke\nlayer: cli\ntags: [smoke, fast]\nsteps:\n  - cli.run:\n      command: '{}'\n      args: [\"--version\"]\n    expect:\n      - path: $.exit_code\n        equals: 42\n",
            da()
        ),
    );
    let root: PathBuf = work.path().join("failures");

    let output = run_da_in(
        work.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(&failing),
            "--bundle-dir",
            path_str(&root),
        ],
    );

    assert_eq!(exit_code(&output), Some(1), "stderr: {}", stderr(&output));
    let report = json_stdout(&output);
    let bundle = PathBuf::from(
        report["cases"][0]["bundle"]
            .as_str()
            .expect("a failed case must report its bundle"),
    );
    let meta: Value = serde_json::from_str(
        &std::fs::read_to_string(bundle.join("meta.json")).expect("meta.json must be readable"),
    )
    .expect("meta.json must be JSON");

    assert_eq!(meta["schema_version"], 1);
    assert_eq!(meta["project"], "shop");
    assert_eq!(meta["suite"], "smoke");
    assert_eq!(meta["layer"], "cli");
    assert_eq!(meta["tags"], serde_json::json!(["smoke", "fast"]));
}

#[test]
fn lint_lists_the_classification_of_every_case() {
    let work = TempDir::new("filter-lint-cases");
    work.write(
        "a.yaml",
        &passing_case(
            "classified",
            "project: shop\nsuite: smoke\nlayer: api\ntags: [smoke]\n",
        ),
    );
    work.write("b.yaml", &passing_case("plain", ""));

    let output = run_da_in(
        work.path(),
        &["--output", "json", "lint", path_str(work.path())],
    );

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
    let report = json_stdout(&output);
    let cases = report["cases"]
        .as_array()
        .expect("lint must list the parsed cases");
    assert_eq!(cases.len(), 2, "got: {report}");

    let classified = cases
        .iter()
        .find(|case| case["name"] == "classified")
        .expect("the classified case must be listed");
    assert_eq!(classified["project"], "shop");
    assert_eq!(classified["suite"], "smoke");
    assert_eq!(classified["layer"], "api");
    assert_eq!(classified["tags"], serde_json::json!(["smoke"]));
    assert!(
        classified["file"]
            .as_str()
            .is_some_and(|file| file.ends_with("a.yaml")),
        "got: {classified}"
    );

    let plain = cases
        .iter()
        .find(|case| case["name"] == "plain")
        .expect("the plain case must be listed");
    assert_eq!(plain["project"], Value::Null);
    assert_eq!(plain["tags"], serde_json::json!([]));
}
