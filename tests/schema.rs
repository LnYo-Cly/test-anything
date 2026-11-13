//! Tests for `ta schema` and its agreement with the case parser.
//!
//! `jsonschema` is a dev-dependency only: production code builds the schema
//! with `serde_json` and never validates anything at run time.

mod support;

#[cfg(feature = "cli")]
use std::fs;
#[cfg(feature = "cli")]
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::LazyLock;

use serde_json::{Value, json};
use support::TempDir;

/// The binary under test, as an absolute path.
fn ta() -> &'static str {
    env!("CARGO_BIN_EXE_ta")
}

fn run_ta(args: &[&str]) -> Output {
    let work = TempDir::new("schema-cwd");
    Command::new(ta())
        .current_dir(work.path())
        .args(args)
        .output()
        .expect("failed to run the ta binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The schema as `--output json` prints it, parsed once per test process.
fn schema() -> &'static Value {
    static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
        let output = run_ta(&["--output", "json", "schema"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`ta schema` must exit 0, stderr: {:?}",
            stderr(&output)
        );
        assert!(
            stderr(&output).is_empty(),
            "`ta schema` must not write diagnostics, got: {:?}",
            stderr(&output)
        );
        serde_json::from_slice(&output.stdout).expect("stdout must contain exactly one JSON value")
    });
    &SCHEMA
}

#[cfg(feature = "cli")]
fn validator(schema: &'static Value) -> jsonschema::Validator {
    jsonschema::validator_for(schema).expect("the emitted schema must compile")
}

/// A complete case that uses a registered step, as a JSON instance.
#[cfg(feature = "cli")]
fn valid_instance() -> Value {
    json!({
        "name": "Example",
        "project": "shop",
        "suite": "smoke",
        "layer": "api",
        "tags": ["smoke", "fast"],
        "vars": {"port": 8080, "verbose": true},
        "steps": [
            {
                "cli.run": {"command": "ta", "args": ["--version"]},
                "expect": [{"path": "$.exit_code", "equals": 0}],
                "save": {"version": "$.stdout"}
            }
        ]
    })
}

#[test]
fn the_schema_is_one_json_object_in_both_output_modes() {
    let output = run_ta(&["schema"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "`ta schema` must exit 0, stderr: {:?}",
        stderr(&output)
    );

    let text: Value = serde_json::from_str(&stdout(&output))
        .expect("text output must be one JSON object, pretty-printed or not");
    assert_eq!(text, *schema(), "both modes must describe the same schema");
    assert_eq!(
        text["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert!(text["$defs"].is_object(), "got: {text}");
    assert_eq!(text["type"], "object");
}

#[test]
fn the_schema_is_valid_against_the_2020_12_meta_schema() {
    jsonschema::meta::validate(schema()).expect("the schema must satisfy its own meta-schema");
}

#[cfg(feature = "cli")]
#[test]
fn the_classification_fields_match_the_parser() {
    let validator = validator(schema());
    let instance = valid_instance();
    assert!(
        validator.is_valid(&instance),
        "the parser accepts classification, so the schema must too"
    );

    for field in ["project", "suite", "layer"] {
        let mut empty = instance.clone();
        empty[field] = json!("");
        assert!(
            !validator.is_valid(&empty),
            "the parser rejects an empty `{field}`, so the schema must too"
        );
    }

    let mut duplicate = instance.clone();
    duplicate["tags"] = json!(["smoke", "smoke"]);
    assert!(
        !validator.is_valid(&duplicate),
        "the parser rejects a duplicate tag, so the schema must too"
    );

    let mut empty_tag = instance.clone();
    empty_tag["tags"] = json!([""]);
    assert!(
        !validator.is_valid(&empty_tag),
        "the parser rejects an empty tag, so the schema must too"
    );
}

#[cfg(feature = "cli")]
#[test]
fn an_unknown_top_level_field_is_rejected() {
    let validator = validator(schema());
    let mut instance = valid_instance();
    assert!(validator.is_valid(&instance), "the control case must pass");

    instance["naem"] = json!("typo");
    let errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|error| error.to_string())
        .collect();
    assert!(
        errors.iter().any(|error| error.contains("naem")),
        "the error must name the field, got: {errors:?}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn non_string_case_names_pass_the_schema_and_lint() {
    let dir = TempDir::new("schema-name-scalars");
    let validator = validator(schema());

    for (yaml, label) in [("123", "number"), ("true", "boolean")] {
        let path = dir.write(
            &format!("name-{label}.yaml"),
            &format!("name: {yaml}\nsteps:\n  - cli.run:\n      command: ta\n"),
        );
        let text = fs::read_to_string(&path).expect("the case must be readable");
        let instance: Value = serde_saphyr::from_str(&text).expect("the case must be valid YAML");
        assert!(
            !instance["name"].is_string(),
            "the regression must use a non-string YAML scalar, got: {instance}"
        );
        assert!(
            validator.is_valid(&instance),
            "the schema rejected a case name the parser accepts: {instance}"
        );

        let output = run_ta(&["lint", path.to_str().expect("the path must be UTF-8")]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`ta lint` must accept `name: {yaml}`, stderr: {:?}",
            stderr(&output)
        );
    }
}

#[test]
fn an_empty_step_list_is_rejected() {
    let instance = json!({"name": "No steps", "steps": []});

    assert!(
        !jsonschema::is_valid(schema(), &instance),
        "the parser requires at least one step, so the schema must too"
    );
}

#[cfg(feature = "cli")]
#[test]
fn an_unregistered_step_is_rejected() {
    let validator = validator(schema());
    let instance = json!({
        "name": "Unknown",
        "steps": [{"no.such.step": {}}]
    });

    assert!(
        !validator.is_valid(&instance),
        "an unregistered step must not validate"
    );
}

#[cfg(feature = "cli")]
#[test]
fn an_unknown_comparison_is_rejected() {
    let validator = validator(schema());
    let instance = json!({
        "name": "Unknown comparison",
        "steps": [{
            "cli.run": {"command": "ta"},
            "expect": [{"path": "$.exit_code", "matches": 0}]
        }]
    });

    assert!(
        !validator.is_valid(&instance),
        "an unknown comparison must not validate"
    );
}

#[cfg(feature = "cli")]
#[test]
fn an_unknown_step_argument_is_rejected() {
    let validator = validator(schema());
    let instance = json!({
        "name": "Unknown argument",
        "steps": [{"cli.run": {"command": "ta", "bogus": true}}]
    });

    assert!(
        !validator.is_valid(&instance),
        "an unknown step argument must not validate"
    );
}

#[cfg(feature = "cli")]
#[test]
fn the_schema_accepts_every_valid_repository_case() {
    let validator = validator(schema());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    for file in files_with_extension(root.join("tests/self"), "yaml") {
        let text = fs::read_to_string(&file).expect("the self case must be readable");
        let instance: Value =
            serde_saphyr::from_str(&text).expect("the self case must be valid YAML");
        assert!(
            validator.is_valid(&instance),
            "the schema rejected the valid self case {}",
            file.display()
        );
    }

    for file in files_with_extension(root.join("tests/self/fixtures"), "case") {
        let text = fs::read_to_string(&file).expect("the fixture must be readable");
        if test_anything::core::case::parse(&text).is_err() {
            let instance: Value =
                serde_saphyr::from_str(&text).expect("an invalid fixture is still valid YAML");
            assert!(
                !validator.is_valid(&instance),
                "the schema accepted the invalid fixture {}",
                file.display()
            );
            continue;
        }
        let instance: Value =
            serde_saphyr::from_str(&text).expect("a parsable fixture must be valid YAML");
        assert!(
            validator.is_valid(&instance),
            "the schema rejected the valid fixture {}",
            file.display()
        );
    }
}

#[cfg(all(feature = "http", feature = "cli"))]
#[test]
fn the_schema_accepts_every_case_example_in_the_docs() {
    let validator = validator(schema());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut checked = 0;
    for doc in ["README.md", "README.zh-CN.md", "docs/guide.md"] {
        let text = fs::read_to_string(root.join(doc)).expect("the document must be readable");
        for example in case_examples(&text) {
            assert!(
                test_anything::core::case::parse(example).is_ok(),
                "an example in {doc} must parse as a case:
{example}"
            );
            let instance: Value =
                serde_saphyr::from_str(example).expect("the example must be valid YAML");
            assert!(
                validator.is_valid(&instance),
                "the schema rejected an example in {doc}:
{example}"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 5,
        "expected the docs to hold case examples, found {checked}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn the_schema_embeds_each_registered_step_with_its_description() {
    let schema = schema();
    let definition = &schema["$defs"]["cli.run"];
    assert!(
        definition["description"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "the step definition must keep its description, got: {definition}"
    );
    assert_eq!(definition["type"], "object");

    let branches = schema["properties"]["steps"]["items"]["oneOf"]
        .as_array()
        .unwrap_or_else(|| panic!("the step schema must be a oneOf, got: {schema}"));
    assert!(
        branches.iter().any(|branch| {
            branch["properties"]["cli.run"]["$ref"]
                .as_str()
                .is_some_and(|reference| reference == "#/$defs/cli.run")
        }),
        "the steps schema must reference the step definition, got: {branches:?}"
    );
}

#[cfg(not(any(feature = "http", feature = "cli")))]
#[test]
fn the_schema_is_valid_when_no_step_is_registered() {
    let schema = schema();
    assert_eq!(
        schema["properties"]["steps"]["items"],
        Value::Bool(false),
        "without registered steps no step item can be valid"
    );
    assert_eq!(
        schema["$defs"]
            .as_object()
            .expect("$defs must be an object")
            .len(),
        2,
        "a build without steps keeps only the shared assertion and save definitions"
    );
    jsonschema::meta::validate(schema).expect("the no-driver schema must be a valid schema");

    let instance = json!({"name": "No driver", "steps": [{"anything": {}}]});
    assert!(
        !jsonschema::is_valid(schema, &instance),
        "a build without steps must not accept a case with a step"
    );
}

/// Every file directly inside `dir` with the given extension, sorted by path.
#[cfg(feature = "cli")]
fn files_with_extension(dir: PathBuf, extension: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("failed to list {}: {error}", dir.display()))
        .map(|entry| entry.expect("failed to read a directory entry").path())
        .filter(|path| path.extension().is_some_and(|value| value == extension))
        .collect();
    files.sort();
    files
}

/// Complete case files in a document: `yaml` blocks with a top-level `name:`.
///
/// Fragments that show only part of a case have no `name:` line and are skipped.
#[cfg(all(feature = "http", feature = "cli"))]
fn case_examples(text: &str) -> Vec<&str> {
    text.split(
        "```yaml
",
    )
    .skip(1)
    .filter_map(|rest| {
        rest.split_once(
            "
```",
        )
        .map(|(block, _)| block)
    })
    .filter(|block| block.lines().any(|line| line.starts_with("name:")))
    .collect()
}
