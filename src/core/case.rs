//! Case files: the YAML model, loading, discovery and variable substitution.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::core::paths;
use jsonpath_rust::JsonPath;
use serde::Deserialize;
use serde_json::{Map, Value};

/// A test case as written in a case file.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// Human-readable case name.
    pub name: String,
    /// Variables defined for the whole case.
    pub vars: BTreeMap<String, String>,
    /// Project the case belongs to, if it declares one.
    pub project: Option<String>,
    /// Suite the case belongs to, if it declares one.
    pub suite: Option<String>,
    /// Free-form layer of the case, if it declares one.
    pub layer: Option<String>,
    /// Tags of the case, in declaration order. Empty when it has none.
    pub tags: Vec<String>,
    /// Steps, executed in order until one fails.
    pub steps: Vec<Step>,
}

/// One step of a case.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// Registered step name, for example `sample.call`.
    pub action: String,
    /// Arguments passed to the step.
    pub args: Value,
    /// Assertions evaluated against the step result.
    pub expect: Vec<Expectation>,
    /// Values copied out of the step result into case variables.
    pub save: Vec<SavedValue>,
}

/// One `expect` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Expectation {
    /// JSON path into the step result.
    pub path: String,
    /// How the selected value is compared.
    pub matcher: Matcher,
}

/// The comparison applied by an expectation.
#[derive(Debug, Clone, PartialEq)]
pub enum Matcher {
    /// The selected value equals the expected JSON value.
    Equals(Value),
    /// The selected value differs from the expected JSON value.
    NotEquals(Value),
    /// A string contains a substring, or an array contains a value.
    Contains(Value),
    /// A value is present (`true`) or absent (`false`).
    Exists(bool),
}

impl Matcher {
    /// The name used in reports and in case files.
    pub fn comparison(&self) -> &'static str {
        match self {
            Self::Equals(_) => "equals",
            Self::NotEquals(_) => "not_equals",
            Self::Contains(_) => "contains",
            Self::Exists(_) => "exists",
        }
    }

    /// The expected value shown in reports.
    pub fn expected(&self) -> Value {
        match self {
            Self::Equals(value) | Self::NotEquals(value) | Self::Contains(value) => value.clone(),
            Self::Exists(wanted) => Value::Bool(*wanted),
        }
    }
}

/// A `save` entry: a variable name and the JSON path it comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedValue {
    /// Variable name used by later `${name}` references.
    pub name: String,
    /// JSON path into the step result.
    pub path: String,
}

/// A case together with the file it was loaded from.
#[derive(Debug, Clone)]
pub struct LoadedCase {
    /// Path shown in reports.
    pub path: PathBuf,
    /// The parsed case.
    pub case: Case,
}

/// A case file that could not be loaded or is not valid.
#[derive(Debug, Clone)]
pub struct CaseError {
    /// File the problem belongs to.
    pub file: PathBuf,
    /// What is wrong and, where the parser knows it, where.
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCase {
    name: String,
    #[serde(default)]
    vars: BTreeMap<String, Value>,
    project: Option<String>,
    suite: Option<String>,
    layer: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    steps: Vec<Map<String, Value>>,
}

/// Parses one case file's text.
///
/// Grammar errors carry the location reported by the YAML parser; semantic
/// errors name the step and the field they belong to.
pub fn parse(text: &str) -> Result<Case, String> {
    let raw: RawCase = serde_saphyr::from_str(text).map_err(|error| {
        let message = error.to_string();
        // The YAML parser prefixes its own message with `error: `; the caller
        // already prints one.
        message
            .strip_prefix("error: ")
            .unwrap_or(&message)
            .to_string()
    })?;

    let mut vars = BTreeMap::new();
    for (name, value) in raw.vars {
        let text = match value {
            Value::String(text) => text,
            Value::Number(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
            Value::Null => {
                return Err(format!(
                    "variable `{name}` has no value; use a string, number or boolean"
                ));
            }
            Value::Array(_) | Value::Object(_) => {
                return Err(format!(
                    "variable `{name}` must be a string, number or boolean"
                ));
            }
        };
        vars.insert(name, text);
    }

    let mut steps = Vec::with_capacity(raw.steps.len());
    for (index, step) in raw.steps.into_iter().enumerate() {
        steps.push(parse_step(index + 1, step)?);
    }
    if steps.is_empty() {
        return Err("the case has no steps; add at least one step under `steps`".to_string());
    }

    Ok(Case {
        name: raw.name,
        vars,
        project: classification(raw.project, "project")?,
        suite: classification(raw.suite, "suite")?,
        layer: classification(raw.layer, "layer")?,
        tags: classification_tags(raw.tags)?,
        steps,
    })
}

/// Checks one optional classification value.
///
/// A value is used as written and never interpolated, so an empty value is
/// always a mistake rather than a reference to a variable.
fn classification(value: Option<String>, field: &str) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(text) if text.trim().is_empty() => Err(format!(
            "`{field}` is empty; give it a value or remove the field"
        )),
        Some(text) => Ok(Some(text)),
    }
}

/// Checks the tag list: every tag needs a value and may appear only once.
fn classification_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = BTreeSet::new();
    for tag in &tags {
        if tag.trim().is_empty() {
            return Err("`tags` contains an empty tag; give it a value or remove it".to_string());
        }
        if !seen.insert(tag.as_str()) {
            return Err(format!("`tags` contains `{tag}` twice; keep each tag once"));
        }
    }
    Ok(tags)
}

fn parse_step(index: usize, mut map: Map<String, Value>) -> Result<Step, String> {
    let expect = map.remove("expect");
    let save = map.remove("save");

    let (action, args) = match map.len() {
        1 => map
            .into_iter()
            .next()
            .ok_or_else(|| format!("step {index}: expected one step name"))?,
        0 => {
            return Err(format!(
                "step {index}: expected one step name such as `sample.call`, found none"
            ));
        }
        _ => {
            let names: Vec<String> = map.keys().cloned().collect();
            return Err(format!(
                "step {index}: expected exactly one step name, found {}: {}",
                names.len(),
                names.join(", ")
            ));
        }
    };

    if !args.is_object() {
        return Err(format!(
            "step {index}: `{action}` takes a mapping of arguments, found {}",
            kind_name(&args)
        ));
    }

    let expect = match expect {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut expectations = Vec::with_capacity(items.len());
            for (position, item) in items.iter().enumerate() {
                expectations.push(parse_expectation(index, position + 1, item)?);
            }
            expectations
        }
        Some(other) => {
            return Err(format!(
                "step {index}: `expect` must be a list of assertions, found {}",
                kind_name(&other)
            ));
        }
    };

    let save = match save {
        None => Vec::new(),
        Some(Value::Object(entries)) => {
            let mut saved = Vec::with_capacity(entries.len());
            for (name, value) in entries {
                let Value::String(path) = value else {
                    return Err(format!(
                        "step {index}: the `save` path for `{name}` must be a string, found {}",
                        kind_name(&value)
                    ));
                };
                validate_path(&path).map_err(|message| {
                    format!("step {index}: the `save` path for `{name}` is invalid: {message}")
                })?;
                saved.push(SavedValue { name, path });
            }
            saved
        }
        Some(other) => {
            return Err(format!(
                "step {index}: `save` must be a mapping of variable names to JSON paths, found {}",
                kind_name(&other)
            ));
        }
    };

    Ok(Step {
        action,
        args,
        expect,
        save,
    })
}

const COMPARISONS: [&str; 4] = ["equals", "not_equals", "contains", "exists"];

fn parse_expectation(step: usize, position: usize, value: &Value) -> Result<Expectation, String> {
    let Value::Object(entries) = value else {
        return Err(format!(
            "step {step}: assertion {position} must be a mapping with `path` and one comparison, found {}",
            kind_name(value)
        ));
    };
    let mut entries = entries.clone();

    let path = match entries.remove("path") {
        Some(Value::String(path)) => path,
        Some(other) => {
            return Err(format!(
                "step {step}: assertion {position}: `path` must be a string, found {}",
                kind_name(&other)
            ));
        }
        None => {
            return Err(format!(
                "step {step}: assertion {position}: `path` is required"
            ));
        }
    };
    validate_path(&path).map_err(|message| {
        format!("step {step}: assertion {position}: `{path}` is not a valid JSON path: {message}")
    })?;

    let mut found = Vec::new();
    for comparison in COMPARISONS {
        if let Some(value) = entries.remove(comparison) {
            found.push((comparison, value));
        }
    }
    if !entries.is_empty() {
        let unknown: Vec<String> = entries.keys().cloned().collect();
        return Err(format!(
            "step {step}: assertion {position}: unknown field(s) {}; use one of {}",
            unknown.join(", "),
            COMPARISONS.join(", ")
        ));
    }
    let (comparison, expected) = match found.len() {
        0 => {
            return Err(format!(
                "step {step}: assertion {position}: expected one of {}",
                COMPARISONS.join(", ")
            ));
        }
        1 => found.remove(0),
        _ => {
            let names: Vec<&str> = found.iter().map(|(name, _)| *name).collect();
            return Err(format!(
                "step {step}: assertion {position}: expected exactly one comparison, found {}",
                names.join(", ")
            ));
        }
    };

    let matcher = match comparison {
        "equals" => Matcher::Equals(expected),
        "not_equals" => Matcher::NotEquals(expected),
        "contains" => {
            if !expected.is_string() && !expected.is_array() {
                return Err(format!(
                    "step {step}: assertion {position}: `contains` compares a string or an array, found {}; use `equals` for other values",
                    kind_name(&expected)
                ));
            }
            Matcher::Contains(expected)
        }
        "exists" => match expected {
            Value::Bool(wanted) => Matcher::Exists(wanted),
            other => {
                return Err(format!(
                    "step {step}: assertion {position}: `exists` takes true or false, found {}",
                    kind_name(&other)
                ));
            }
        },
        other => {
            return Err(format!(
                "step {step}: assertion {position}: `{other}` is not a comparison; use one of {}",
                COMPARISONS.join(", ")
            ));
        }
    };

    Ok(Expectation { path, matcher })
}

fn validate_path(path: &str) -> Result<(), String> {
    Value::Null
        .query(path)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Describes a JSON value for an error message.
pub fn kind_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a mapping",
    }
}

/// Expands every `${name}` reference inside `value`.
///
/// The substitution is textual and applies to strings only, so a variable may
/// not change the type of a value. `vars` supplies the defined names.
pub fn interpolate(value: &Value, vars: &BTreeMap<String, String>) -> Result<Value, String> {
    match value {
        Value::String(text) => Ok(Value::String(interpolate_str(text, vars)?)),
        Value::Array(items) => {
            let mut expanded = Vec::with_capacity(items.len());
            for item in items {
                expanded.push(interpolate(item, vars)?);
            }
            Ok(Value::Array(expanded))
        }
        Value::Object(entries) => {
            let mut expanded = Map::new();
            for (key, item) in entries {
                expanded.insert(key.clone(), interpolate(item, vars)?);
            }
            Ok(Value::Object(expanded))
        }
        other => Ok(other.clone()),
    }
}

fn interpolate_str(input: &str, vars: &BTreeMap<String, String>) -> Result<String, String> {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!(
                "unterminated variable reference in `{input}`; write `${{name}}`"
            ));
        };
        let name = &after[..end];
        if name.is_empty() {
            return Err(format!(
                "empty variable name in `{input}`; write `${{name}}`"
            ));
        }
        let Some(value) = vars.get(name) else {
            return Err(format!(
                "undefined variable `${{{name}}}`; define it under `vars:` or save it in an earlier step"
            ));
        };
        output.push_str(value);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Expands case files and directories into a sorted list of case files.
///
/// A directory is searched recursively for `.yaml` and `.yml` files. Symlinks
/// are skipped so that a link cycle cannot loop forever.
pub fn discover(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, CaseError> {
    let mut files = Vec::new();
    for input in inputs {
        let metadata = std::fs::metadata(input).map_err(|error| CaseError {
            file: input.clone(),
            message: format!(
                "could not read `{}`: {error}; check that the path exists",
                paths::display(input)
            ),
        })?;
        if metadata.is_dir() {
            collect_yaml(input, &mut files)?;
        } else {
            files.push(input.clone());
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn collect_yaml(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), CaseError> {
    let entries = std::fs::read_dir(dir).map_err(|error| CaseError {
        file: dir.to_path_buf(),
        message: format!("could not list `{}`: {error}", paths::display(dir)),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| CaseError {
            file: dir.to_path_buf(),
            message: format!(
                "could not read an entry of `{}`: {error}",
                paths::display(dir)
            ),
        })?;
        let file_type = entry.file_type().map_err(|error| CaseError {
            file: entry.path(),
            message: format!(
                "could not inspect `{}`: {error}",
                paths::display(&entry.path())
            ),
        })?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_yaml(&path, files)?;
        } else if file_type.is_file() && is_yaml(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_yaml(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yaml") || extension.eq_ignore_ascii_case("yml")
        })
}

/// Loads and parses one case file.
pub fn load_file(path: &Path) -> Result<LoadedCase, CaseError> {
    let text = std::fs::read_to_string(path).map_err(|error| CaseError {
        file: path.to_path_buf(),
        message: format!(
            "could not read the case file: {error}; check the path and that the file is UTF-8"
        ),
    })?;
    let case = parse(&text).map_err(|message| CaseError {
        file: path.to_path_buf(),
        message,
    })?;
    Ok(LoadedCase {
        path: path.to_path_buf(),
        case,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case_text() -> &'static str {
        r#"
name: Example
vars:
  port: 8080
steps:
  - sample.call:
      target: "service:${port}"
    expect:
      - path: $.status
        equals: 200
    save:
      token: $.body.token
"#
    }

    #[test]
    fn parses_a_case_with_vars_expectations_and_saves() {
        let case = parse(case_text()).expect("the case must parse");

        assert_eq!(case.name, "Example");
        assert_eq!(case.vars.get("port").map(String::as_str), Some("8080"));
        assert_eq!(case.steps.len(), 1);
        assert_eq!(case.steps[0].action, "sample.call");
        assert_eq!(case.steps[0].expect.len(), 1);
        assert_eq!(case.steps[0].expect[0].matcher.comparison(), "equals");
        assert_eq!(case.steps[0].save[0].name, "token");
    }

    #[test]
    fn parses_the_optional_classification() {
        let case = parse(
            "name: Example\nproject: shop\nsuite: smoke\nlayer: api\ntags:\n  - smoke\n  - fast\nsteps:\n  - sample.call:\n      target: local\n",
        )
        .expect("the case must parse");

        assert_eq!(case.project.as_deref(), Some("shop"));
        assert_eq!(case.suite.as_deref(), Some("smoke"));
        assert_eq!(case.layer.as_deref(), Some("api"));
        assert_eq!(case.tags, vec!["smoke", "fast"]);
    }

    #[test]
    fn a_case_without_classification_leaves_every_field_empty() {
        let case = parse("name: Example\nsteps:\n  - sample.call:\n      target: local\n")
            .expect("the case must parse");

        assert_eq!(case.project, None);
        assert_eq!(case.suite, None);
        assert_eq!(case.layer, None);
        assert!(case.tags.is_empty());
    }

    #[test]
    fn classification_values_are_not_interpolated() {
        let case = parse(
            "name: Example\nproject: \"${project}\"\nsteps:\n  - sample.call:\n      target: local\n",
        )
        .expect("the case must parse");

        assert_eq!(case.project.as_deref(), Some("${project}"));
    }

    #[test]
    fn rejects_a_duplicate_tag_and_names_it() {
        let error = parse(
            "name: Broken\ntags:\n  - smoke\n  - smoke\nsteps:\n  - sample.call:\n      target: local\n",
        )
        .expect_err("the case must be rejected");

        assert!(error.contains("smoke"), "got: {error}");
        assert!(error.contains("twice"), "got: {error}");
    }

    #[test]
    fn rejects_an_empty_classification_value() {
        for field in ["project", "suite", "layer"] {
            let text = format!(
                "name: Broken\n{field}: \"\"\nsteps:\n  - sample.call:\n      target: local\n"
            );
            let error = parse(&text).expect_err("the case must be rejected");

            assert!(error.contains(field), "got: {error}");
            assert!(error.contains("empty"), "got: {error}");
        }
    }

    #[test]
    fn rejects_an_empty_tag() {
        let error =
            parse("name: Broken\ntags:\n  - \"\"\nsteps:\n  - sample.call:\n      target: local\n")
                .expect_err("the case must be rejected");

        assert!(error.contains("tags"), "got: {error}");
    }

    #[test]
    fn rejects_an_unknown_top_level_field() {
        let error =
            parse("name: Broken\nnaem: typo\nsteps:\n  - sample.call:\n      target: local\n")
                .expect_err("the case must be rejected");

        assert!(error.contains("naem"), "got: {error}");
        assert!(error.contains("line 2"), "got: {error}");
    }

    #[test]
    fn rejects_a_step_without_a_step_name() {
        let error = parse("name: Broken\nsteps:\n  - expect: []\n").expect_err("must be rejected");

        assert!(error.contains("step 1"), "got: {error}");
    }

    #[test]
    fn rejects_a_step_with_two_step_names() {
        let error = parse(
            "name: Broken\nsteps:\n  - sample.call:\n      target: local\n    other.step: {}\n",
        )
        .expect_err("must be rejected");

        assert!(error.contains("sample.call"), "got: {error}");
        assert!(error.contains("other.step"), "got: {error}");
    }

    #[test]
    fn rejects_a_contains_comparison_against_a_number() {
        let error = parse(
            "name: Broken\nsteps:\n  - sample.call:\n      target: local\n    expect:\n      - path: $.a\n        contains: 3\n",
        )
        .expect_err("must be rejected");

        assert!(error.contains("contains"), "got: {error}");
    }

    #[test]
    fn rejects_an_invalid_json_path() {
        let error = parse(
            "name: Broken\nsteps:\n  - sample.call:\n      target: local\n    expect:\n      - path: $[\n        exists: true\n",
        )
        .expect_err("must be rejected");

        assert!(error.contains("JSON path"), "got: {error}");
    }

    #[test]
    fn interpolates_strings_and_reports_undefined_variables() {
        let vars = BTreeMap::from([("name".to_string(), "alice".to_string())]);
        let value = serde_json::json!({"who": "hi ${name}", "nested": ["${name}"]});

        let expanded = interpolate(&value, &vars).expect("the value must expand");
        assert_eq!(
            expanded,
            serde_json::json!({"who": "hi alice", "nested": ["alice"]})
        );

        let error = interpolate(&serde_json::json!("${missing}"), &vars)
            .expect_err("the reference must be rejected");
        assert!(error.contains("missing"), "got: {error}");
    }

    #[test]
    fn discovers_yaml_files_recursively_in_order() {
        let root = std::env::temp_dir().join(format!("ta-core-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("nested")).expect("create the fixture tree");
        std::fs::write(root.join("b.yaml"), "name: b\nsteps: []\n").expect("write b");
        std::fs::write(root.join("a.yml"), "name: a\nsteps: []\n").expect("write a");
        std::fs::write(root.join("nested/c.yaml"), "name: c\nsteps: []\n").expect("write c");
        std::fs::write(root.join("ignored.txt"), "not yaml").expect("write the ignored file");

        let files = discover(std::slice::from_ref(&root)).expect("discovery must succeed");
        let names: Vec<String> = files
            .iter()
            .map(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();

        assert_eq!(names, vec!["a.yml", "b.yaml", "c.yaml"]);

        let _ = std::fs::remove_dir_all(&root);
    }
}
