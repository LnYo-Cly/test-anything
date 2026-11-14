//! Tests for `da schema` and its agreement with the case parser.
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
fn da() -> &'static str {
    env!("CARGO_BIN_EXE_da")
}

fn run_da(args: &[&str]) -> Output {
    let work = TempDir::new("schema-cwd");
    Command::new(da())
        .current_dir(work.path())
        .args(args)
        .output()
        .expect("failed to run the da binary")
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
        let output = run_da(&["--output", "json", "schema"]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`da schema` must exit 0, stderr: {:?}",
            stderr(&output)
        );
        assert!(
            stderr(&output).is_empty(),
            "`da schema` must not write diagnostics, got: {:?}",
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
                "cli.run": {"command": "da", "args": ["--version"]},
                "expect": [{"path": "$.exit_code", "equals": 0}],
                "save": {"version": "$.stdout"}
            }
        ]
    })
}

#[test]
fn the_schema_is_one_json_object_in_both_output_modes() {
    let output = run_da(&["schema"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "`da schema` must exit 0, stderr: {:?}",
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
        for blank in ["", "   "] {
            let mut empty = instance.clone();
            empty[field] = json!(blank);
            assert!(
                !validator.is_valid(&empty),
                "the parser rejects a blank `{field}` ({blank:?}), so the schema must too"
            );
        }
    }

    let mut duplicate = instance.clone();
    duplicate["tags"] = json!(["smoke", "smoke"]);
    assert!(
        !validator.is_valid(&duplicate),
        "the parser rejects a duplicate tag, so the schema must too"
    );

    for blank in ["", "   "] {
        let mut empty_tag = instance.clone();
        empty_tag["tags"] = json!([blank]);
        assert!(
            !validator.is_valid(&empty_tag),
            "the parser rejects a blank tag ({blank:?}), so the schema must too"
        );
    }
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
            &format!("name: {yaml}\nsteps:\n  - cli.run:\n      command: da\n"),
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

        let output = run_da(&["lint", path.to_str().expect("the path must be UTF-8")]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "`da lint` must accept `name: {yaml}`, stderr: {:?}",
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
            "cli.run": {"command": "da"},
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
fn a_cli_timeout_of_zero_is_rejected_by_the_schema_and_by_validate() {
    let definition = step_definition("cli.run");
    assert_eq!(
        definition.args_schema["properties"]["timeout_ms"]["minimum"], 1,
        "the step schema must declare the bound validate enforces: {}",
        definition.args_schema
    );
    assert_eq!(
        schema()["$defs"]["cli.run"]["properties"]["timeout_ms"]["minimum"],
        1,
        "`da schema` must carry the same bound"
    );

    let zero = json!({"command": "da", "timeout_ms": 0});
    assert!(
        (definition.validate)(&zero).is_err(),
        "validate must reject `timeout_ms: 0`"
    );
    assert!(
        !jsonschema::is_valid(&definition.args_schema, &zero),
        "the schema must reject `timeout_ms: 0`: {}",
        definition.args_schema
    );
}

#[cfg(feature = "http")]
#[test]
fn an_http_timeout_of_zero_is_rejected_by_the_schema_and_by_validate() {
    let definition = step_definition("http.request");
    assert_eq!(
        definition.args_schema["properties"]["timeout_ms"]["minimum"], 1,
        "the step schema must declare the bound validate enforces: {}",
        definition.args_schema
    );
    assert_eq!(
        schema()["$defs"]["http.request"]["properties"]["timeout_ms"]["minimum"],
        1,
        "`da schema` must carry the same bound"
    );

    let zero = json!({"url": "http://127.0.0.1:1/health", "timeout_ms": 0});
    assert!(
        (definition.validate)(&zero).is_err(),
        "validate must reject `timeout_ms: 0`"
    );
    assert!(
        !jsonschema::is_valid(&definition.args_schema, &zero),
        "the schema must reject `timeout_ms: 0`: {}",
        definition.args_schema
    );
}

/// A registered step definition, for tests that compare the schema with
/// `validate`.
#[cfg(any(feature = "cli", feature = "http"))]
fn step_definition(name: &str) -> do_anything::core::registry::StepDefinition {
    built_in_registry()
        .get(name)
        .unwrap_or_else(|| panic!("`{name}` must be registered"))
        .clone()
}

/// Every registered step definition, keyed by name.
#[cfg(any(feature = "cli", feature = "db", feature = "http"))]
fn built_in_registry() -> do_anything::core::registry::Registry {
    let mut registry = do_anything::core::registry::Registry::new();
    do_anything::drivers::register(&mut registry).expect("the built-in drivers must register");
    do_anything::drivers::register_observers(&mut registry)
        .expect("the built-in observers must register");
    registry
}

/// Arguments each built-in step accepts, before one argument is overridden.
///
/// The values only have to pass `validate`; it checks arguments without
/// starting the program or opening a connection.
#[cfg(any(feature = "cli", feature = "db", feature = "http"))]
fn valid_arguments(step: &str) -> Value {
    match step {
        "cli.run" => json!({"command": "da"}),
        "http.request" => json!({"url": "http://127.0.0.1:1/health"}),
        "db.query" => json!({"url": "sqlite:app.db", "sql": "SELECT 1"}),
        other => panic!("add the valid arguments of `{other}` to this test"),
    }
}

/// The bounds of one argument schema that this test can turn into a value.
///
/// A bound that the schema can express but this list cannot construct is
/// caught by the assertion on the constructed value, which fails loudly.
#[cfg(any(feature = "cli", feature = "db", feature = "http"))]
fn declared_bounds(schema: &Value) -> Vec<(&'static str, Value)> {
    ["minimum", "minLength", "pattern"]
        .into_iter()
        .filter_map(|keyword| schema.get(keyword).map(|bound| (keyword, bound.clone())))
        .collect()
}

/// A value that violates one bound, and a value on the bound when one can be
/// constructed generically.
#[cfg(any(feature = "cli", feature = "db", feature = "http"))]
fn bound_values(keyword: &str, bound: &Value) -> (Value, Option<Value>) {
    match keyword {
        "minimum" => {
            let minimum = bound
                .as_i64()
                .unwrap_or_else(|| panic!("`minimum` must be an integer, got: {bound}"));
            (json!(minimum - 1), Some(json!(minimum)))
        }
        "minLength" => {
            let minimum = bound
                .as_u64()
                .unwrap_or_else(|| panic!("`minLength` must be an integer, got: {bound}"));
            let length = usize::try_from(minimum).unwrap_or(usize::MAX);
            (
                json!("x".repeat(length.saturating_sub(1))),
                Some(json!("x".repeat(length))),
            )
        }
        // No generic string matches an arbitrary pattern, so only the
        // violating side is constructed: the empty string is refused by every
        // pattern this project writes, and the assertion says so otherwise.
        "pattern" => (json!(""), None),
        other => panic!("add a value for the `{other}` bound to this test"),
    }
}

/// The schema and `validate` agree on every bound a step declares.
///
/// This is the regression gate for a class of drift that has now happened three
/// times: the schema accepted a value that the driver later refused (`name`, a
/// blank classification value, `timeout_ms: 0`). A step's argument schema and
/// its `validate` function are two contracts for the same arguments, and
/// neither may be looser than the other. The reverse direction, a value that
/// `validate` refuses while the schema accepts it, is covered by the two
/// `*_timeout_of_zero_*` tests above.
#[cfg(any(feature = "cli", feature = "db", feature = "http"))]
#[test]
fn every_declared_argument_bound_is_enforced_by_validate() {
    let registry = built_in_registry();
    let mut checked = 0;

    for name in registry.names() {
        let definition = registry
            .get(name)
            .expect("every listed step must be registered");
        let base = valid_arguments(name);
        assert!(
            (definition.validate)(&base).is_ok(),
            "the base arguments for `{name}` must be valid: {base}"
        );
        assert!(
            jsonschema::is_valid(&definition.args_schema, &base),
            "the schema must accept the base arguments for `{name}`: {base}"
        );

        let properties = definition.args_schema["properties"]
            .as_object()
            .unwrap_or_else(|| {
                panic!(
                    "`{name}` must declare its arguments as object properties: {}",
                    definition.args_schema
                )
            });
        for (argument, schema) in properties {
            for (keyword, bound) in declared_bounds(schema) {
                let (violating, allowed) = bound_values(keyword, &bound);

                let mut args = base.clone();
                args[argument] = violating;
                assert!(
                    !jsonschema::is_valid(&definition.args_schema, &args),
                    "the test must build a value that violates `{keyword}` for `{argument}` of `{name}`: {args}"
                );
                assert!(
                    (definition.validate)(&args).is_err(),
                    "`{name}` declares `{keyword}: {bound}` for `{argument}`, but validate accepts {args}"
                );

                // The value on the boundary itself must be accepted, so that
                // `da schema` does not promise a value that `da run` refuses.
                if let Some(allowed) = allowed {
                    let mut args = base.clone();
                    args[argument] = allowed;
                    assert!(
                        (definition.validate)(&args).is_ok(),
                        "`{name}` declares `{keyword}: {bound}` for `{argument}`, but validate rejects {args}"
                    );
                }
                checked += 1;
            }
        }
    }

    let drivers = usize::from(cfg!(feature = "cli")) + usize::from(cfg!(feature = "http"));
    assert!(
        checked >= drivers,
        "expected at least one bounded argument per built-in driver, checked {checked}"
    );
}

#[cfg(feature = "cli")]
#[test]
fn an_unknown_step_argument_is_rejected() {
    let validator = validator(schema());
    let instance = json!({
        "name": "Unknown argument",
        "steps": [{"cli.run": {"command": "da", "bogus": true}}]
    });

    assert!(
        !validator.is_valid(&instance),
        "an unknown step argument must not validate"
    );
}

/// The `observe` block and the observers behind it, as `da schema` describes
/// them.
#[cfg(all(feature = "log", feature = "cli"))]
#[test]
fn the_schema_describes_observe_and_every_registered_observer() {
    let schema = schema();
    let observe = &schema["properties"]["observe"];

    assert_eq!(observe["type"], "object");
    assert_eq!(
        observe["propertyNames"]["pattern"], "^[a-z0-9._-]+$",
        "the name becomes a key of `observed`, so the schema states the rule"
    );
    let branches = observe["additionalProperties"]["oneOf"]
        .as_array()
        .unwrap_or_else(|| panic!("registered observers must produce a oneOf, got: {observe}"));
    assert!(
        branches.iter().any(|branch| {
            branch["additionalProperties"] == false
                && branch["properties"]["log.file"]["$ref"] == "#/$defs/log.file"
        }),
        "`observe` must reference the observer definition, got: {branches:?}"
    );
    let definition = &schema["$defs"]["log.file"];
    assert_eq!(definition["type"], "object");
    assert_eq!(definition["properties"]["path"]["minLength"], 1);
    assert!(
        definition["description"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "the observer definition must keep its description, got: {definition}"
    );

    let validator = validator(schema);
    let valid = json!({
        "name": "Observed",
        "observe": {"app_log": {"log.file": {"path": "app.log"}}},
        "steps": [{"cli.run": {"command": "da"}}]
    });
    assert!(
        validator.is_valid(&valid),
        "a case that declares an observation must validate: {valid}"
    );

    for (label, observe, fragment) in [
        (
            "unknown observer",
            json!({"app_log": {"log.fil": {"path": "app.log"}}}),
            "log.fil",
        ),
        (
            "two observers",
            json!({"app_log": {"log.file": {"path": "a.log"}, "log.fil": {"path": "b.log"}}}),
            "log.fil",
        ),
        (
            "empty path",
            json!({"app_log": {"log.file": {"path": ""}}}),
            "path",
        ),
        (
            "unknown argument",
            json!({"app_log": {"log.file": {"path": "a.log", "location": "here"}}}),
            "location",
        ),
        (
            "name outside the rule",
            json!({"App_log": {"log.file": {"path": "a.log"}}}),
            "App_log",
        ),
    ] {
        let mut instance = valid.clone();
        instance["observe"] = observe;
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|error| error.to_string())
            .collect();
        assert!(
            !errors.is_empty(),
            "{label}: the schema must reject {instance}"
        );
        assert!(
            errors.iter().any(|error| error.contains(fragment)),
            "{label}: the error must mention `{fragment}`, got: {errors:?}"
        );
    }
}

/// The schema and `validate` agree on every bound an observer declares.
///
/// This is the same gate as [`every_declared_argument_bound_is_enforced_by_validate`],
/// applied to the second kind of extension point the registry holds.
#[cfg(all(feature = "log", any(feature = "cli", feature = "http")))]
#[test]
fn every_declared_observer_argument_bound_is_enforced_by_validate() {
    let registry = built_in_registry();
    let mut checked = 0;

    for name in registry.observer_names() {
        let definition = registry
            .observer(name)
            .expect("every listed observer must be registered");
        let base = observer_arguments(name);
        assert!(
            (definition.validate)(&base).is_ok(),
            "the base arguments for `{name}` must be valid: {base}"
        );
        assert!(
            jsonschema::is_valid(&definition.args_schema, &base),
            "the schema must accept the base arguments for `{name}`: {base}"
        );
        assert_eq!(
            schema()["$defs"][name]["properties"],
            definition.args_schema["properties"],
            "`da schema` must carry the observer's arguments"
        );

        let properties = definition.args_schema["properties"]
            .as_object()
            .unwrap_or_else(|| {
                panic!(
                    "`{name}` must declare its arguments as object properties: {}",
                    definition.args_schema
                )
            });
        for (argument, schema) in properties {
            for (keyword, bound) in declared_bounds(schema) {
                let (violating, allowed) = bound_values(keyword, &bound);

                let mut args = base.clone();
                args[argument] = violating;
                assert!(
                    !jsonschema::is_valid(&definition.args_schema, &args),
                    "the test must build a value that violates `{keyword}` for `{argument}` of `{name}`: {args}"
                );
                assert!(
                    (definition.validate)(&args).is_err(),
                    "`{name}` declares `{keyword}: {bound}` for `{argument}`, but validate accepts {args}"
                );

                if let Some(allowed) = allowed {
                    let mut args = base.clone();
                    args[argument] = allowed;
                    assert!(
                        (definition.validate)(&args).is_ok(),
                        "`{name}` declares `{keyword}: {bound}` for `{argument}`, but validate rejects {args}"
                    );
                }
                checked += 1;
            }
        }
    }

    assert!(
        checked > 0,
        "expected at least one bounded observer argument"
    );
}

/// Arguments each built-in observer accepts, before one is overridden.
#[cfg(all(feature = "log", any(feature = "cli", feature = "http")))]
fn observer_arguments(observer: &str) -> Value {
    match observer {
        "log.file" => json!({"path": "app.log"}),
        other => panic!("add the valid arguments of `{other}` to this test"),
    }
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
        // A build without the observer cannot describe a case that declares
        // one; the case itself is still checked by every build that has it.
        if instance.get("observe").is_some() && !describes_observations(schema()) {
            continue;
        }
        assert!(
            validator.is_valid(&instance),
            "the schema rejected the valid self case {}",
            file.display()
        );
    }

    for file in files_with_extension(root.join("tests/self/fixtures"), "case") {
        let text = fs::read_to_string(&file).expect("the fixture must be readable");
        if do_anything::core::case::parse(&text).is_err() {
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

/// Whether this build can describe an observation at all.
#[cfg(feature = "cli")]
fn describes_observations(schema: &Value) -> bool {
    schema["properties"]["observe"]["additionalProperties"] != Value::Bool(false)
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
                do_anything::core::case::parse(example).is_ok(),
                "an example in {doc} must parse as a case:
{example}"
            );
            let instance: Value =
                serde_saphyr::from_str(example).expect("the example must be valid YAML");
            // A build without the observer cannot describe an example that
            // declares one; every build that has the observer checks it.
            if instance.get("observe").is_some() && !describes_observations(schema()) {
                continue;
            }
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

#[cfg(not(any(feature = "cli", feature = "db", feature = "http")))]
#[test]
fn the_schema_is_valid_when_no_step_is_registered() {
    let schema = schema();
    assert_eq!(
        schema["properties"]["steps"]["items"],
        Value::Bool(false),
        "without registered steps no step item can be valid"
    );
    let definitions = schema["$defs"]
        .as_object()
        .expect("$defs must be an object");
    assert!(
        definitions.contains_key("expect") && definitions.contains_key("save"),
        "a build without steps keeps the shared assertion and save definitions"
    );
    assert!(
        !definitions.contains_key("cli.run") && !definitions.contains_key("http.request"),
        "a build without steps must not describe a step: {:?}",
        definitions.keys().collect::<Vec<_>>()
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
