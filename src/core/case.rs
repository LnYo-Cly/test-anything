//! Case files: the YAML model, loading, discovery and variable substitution.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::core::paths;
use jsonpath_rust::JsonPath;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use serde_saphyr::{CroppedRegion, Error, Location, Options, Spanned};

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
    vars: BTreeMap<String, Spanned<Value>>,
    project: Option<String>,
    suite: Option<String>,
    layer: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    steps: Vec<Spanned<RawStep>>,
}

/// One step as written, with the location of every part a structural error can
/// name.
#[derive(Debug)]
struct RawStep {
    /// Keys other than `expect` and `save`, each with its value. A valid step
    /// has exactly one: the step name.
    fields: Vec<(String, Spanned<Value>)>,
    /// The `expect` value, if the step has one.
    expect: Option<Spanned<RawNodes>>,
    /// The `save` value, if the step has one.
    save: Option<Spanned<RawNodes>>,
}

impl<'de> Deserialize<'de> for RawStep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StepVisitor;

        impl<'de> Visitor<'de> for StepVisitor {
            type Value = RawStep;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a step mapping with one step name")
            }

            fn visit_map<A>(self, mut map: A) -> Result<RawStep, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut fields = Vec::new();
                let mut expect = None;
                let mut save = None;
                // The step name is chosen by the case, so the value type of each
                // key is decided here rather than by a struct: `expect` and
                // `save` must keep the location of every entry they contain.
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "expect" => expect = Some(map.next_value()?),
                        "save" => save = Some(map.next_value()?),
                        _ => fields.push((key, map.next_value()?)),
                    }
                }
                Ok(RawStep {
                    fields,
                    expect,
                    save,
                })
            }
        }

        deserializer.deserialize_map(StepVisitor)
    }
}

/// A raw `expect` or `save` value.
#[derive(Debug)]
enum RawNodes {
    /// A list, with the location of every entry.
    List(Vec<Spanned<Value>>),
    /// A mapping, with the location of every value.
    Map(Vec<(String, Spanned<Value>)>),
    /// Anything else, kept as the kind of value that was found.
    Other(&'static str),
}

impl RawNodes {
    /// The kind of value, for an error message.
    fn kind(&self) -> &'static str {
        match self {
            Self::List(_) => "a list",
            Self::Map(_) => "a mapping",
            Self::Other(kind) => kind,
        }
    }
}

impl<'de> Deserialize<'de> for RawNodes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct NodesVisitor;

        impl<'de> Visitor<'de> for NodesVisitor {
            type Value = RawNodes;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a list or a mapping")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<RawNodes, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<Spanned<Value>>()? {
                    items.push(item);
                }
                Ok(RawNodes::List(items))
            }

            fn visit_map<A>(self, mut map: A) -> Result<RawNodes, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    entries.push((key, map.next_value::<Spanned<Value>>()?));
                }
                Ok(RawNodes::Map(entries))
            }

            fn visit_bool<E>(self, _: bool) -> Result<RawNodes, E> {
                Ok(RawNodes::Other("a boolean"))
            }

            fn visit_i64<E>(self, _: i64) -> Result<RawNodes, E> {
                Ok(RawNodes::Other("a number"))
            }

            fn visit_u64<E>(self, _: u64) -> Result<RawNodes, E> {
                Ok(RawNodes::Other("a number"))
            }

            fn visit_f64<E>(self, _: f64) -> Result<RawNodes, E> {
                Ok(RawNodes::Other("a number"))
            }

            fn visit_str<E>(self, _: &str) -> Result<RawNodes, E> {
                Ok(RawNodes::Other("a string"))
            }

            fn visit_string<E>(self, _: String) -> Result<RawNodes, E> {
                Ok(RawNodes::Other("a string"))
            }

            fn visit_unit<E>(self) -> Result<RawNodes, E> {
                Ok(RawNodes::Other("null"))
            }

            fn visit_none<E>(self) -> Result<RawNodes, E> {
                Ok(RawNodes::Other("null"))
            }
        }

        deserializer.deserialize_any(NodesVisitor)
    }
}

/// A structural problem in a case file together with the node it belongs to.
#[derive(Debug)]
struct Problem {
    /// Where the offending node starts.
    location: Location,
    /// What is wrong.
    message: String,
}

impl Problem {
    /// Renders the problem the way the YAML parser reports a grammar error: a
    /// `line L column C:` prefix and a source snippet with a caret line.
    ///
    /// The parser already renders that layout, so a structural error reuses its
    /// renderer rather than keeping a second copy of the format.
    fn render(self, text: &str) -> String {
        let options = Options::default();
        let error = Error::WithSnippet {
            regions: vec![CroppedRegion::new(
                text,
                "<input>",
                1,
                text.lines().count(),
                self.location,
            )],
            crop_radius: options.crop_radius,
            error: Box::new(Error::Message {
                msg: self.message,
                location: self.location,
            }),
        };
        strip_error_prefix(&error.to_string())
    }
}

/// Drops the `error: ` prefix the YAML parser puts on its messages.
///
/// The caller prints its own prefix, so keeping the parser's would duplicate it.
fn strip_error_prefix(message: &str) -> String {
    message
        .strip_prefix("error: ")
        .unwrap_or(message)
        .to_string()
}

/// Parses one case file's text.
///
/// Errors carry the location of the node they belong to: the one the YAML
/// parser reports for a grammar error, and the one of the offending step,
/// assertion or `save` entry for a structural error.
pub fn parse(text: &str) -> Result<Case, String> {
    let raw: Spanned<RawCase> =
        serde_saphyr::from_str(text).map_err(|error| strip_error_prefix(&error.to_string()))?;
    build_case(raw).map_err(|problem| problem.render(text))
}

/// Turns the raw document into a case, adding the location to every problem.
fn build_case(raw: Spanned<RawCase>) -> Result<Case, Problem> {
    // Problems without a more precise node point at the document itself.
    let document = raw.referenced;
    let RawCase {
        name,
        vars,
        project,
        suite,
        layer,
        tags,
        steps: raw_steps,
    } = raw.value;

    let mut expanded = BTreeMap::new();
    for (name, value) in vars {
        let location = value.referenced;
        let text = match value.value {
            Value::String(text) => text,
            Value::Number(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
            Value::Null => {
                return Err(Problem {
                    location,
                    message: format!(
                        "variable `{name}` has no value; use a string, number or boolean"
                    ),
                });
            }
            Value::Array(_) | Value::Object(_) => {
                return Err(Problem {
                    location,
                    message: format!("variable `{name}` must be a string, number or boolean"),
                });
            }
        };
        expanded.insert(name, text);
    }

    let mut steps = Vec::with_capacity(raw_steps.len());
    for (index, step) in raw_steps.into_iter().enumerate() {
        steps.push(parse_step(index + 1, step)?);
    }
    if steps.is_empty() {
        return Err(Problem {
            location: document,
            message: "the case has no steps; add at least one step under `steps`".to_string(),
        });
    }

    Ok(Case {
        name,
        vars: expanded,
        project: classification(project, "project", document)?,
        suite: classification(suite, "suite", document)?,
        layer: classification(layer, "layer", document)?,
        tags: classification_tags(tags, document)?,
        steps,
    })
}

/// Checks one optional classification value.
///
/// A value is used as written and never interpolated, so an empty value is
/// always a mistake rather than a reference to a variable.
fn classification(
    value: Option<String>,
    field: &str,
    location: Location,
) -> Result<Option<String>, Problem> {
    match value {
        None => Ok(None),
        Some(text) if text.trim().is_empty() => Err(Problem {
            location,
            message: format!("`{field}` is empty; give it a value or remove the field"),
        }),
        Some(text) => Ok(Some(text)),
    }
}

/// Checks the tag list: every tag needs a value and may appear only once.
fn classification_tags(tags: Vec<String>, location: Location) -> Result<Vec<String>, Problem> {
    let mut seen = BTreeSet::new();
    for tag in &tags {
        if tag.trim().is_empty() {
            return Err(Problem {
                location,
                message: "`tags` contains an empty tag; give it a value or remove it".to_string(),
            });
        }
        if !seen.insert(tag.as_str()) {
            return Err(Problem {
                location,
                message: format!("`tags` contains `{tag}` twice; keep each tag once"),
            });
        }
    }
    Ok(tags)
}

fn parse_step(index: usize, step: Spanned<RawStep>) -> Result<Step, Problem> {
    let location = step.referenced;
    let RawStep {
        mut fields,
        expect,
        save,
    } = step.value;

    let (action, args) = match fields.len() {
        1 => {
            let Some((action, args)) = fields.pop() else {
                return Err(Problem {
                    location,
                    message: format!("step {index}: expected one step name"),
                });
            };
            (action, args)
        }
        0 => {
            return Err(Problem {
                location,
                message: format!(
                    "step {index}: expected one step name such as `sample.call`, found none"
                ),
            });
        }
        count => {
            let names: Vec<String> = fields.into_iter().map(|(name, _)| name).collect();
            return Err(Problem {
                location,
                message: format!(
                    "step {index}: expected exactly one step name, found {count}: {}",
                    names.join(", ")
                ),
            });
        }
    };

    if !args.value.is_object() {
        return Err(Problem {
            location: args.referenced,
            message: format!(
                "step {index}: `{action}` takes a mapping of arguments, found {}",
                kind_name(&args.value)
            ),
        });
    }

    let expect = match expect {
        None => Vec::new(),
        Some(node) => match node.value {
            RawNodes::List(items) => {
                let mut expectations = Vec::with_capacity(items.len());
                for (position, item) in items.iter().enumerate() {
                    expectations.push(parse_expectation(index, position + 1, item)?);
                }
                expectations
            }
            other => {
                return Err(Problem {
                    location: node.referenced,
                    message: format!(
                        "step {index}: `expect` must be a list of assertions, found {}",
                        other.kind()
                    ),
                });
            }
        },
    };

    let save = match save {
        None => Vec::new(),
        Some(node) => match node.value {
            RawNodes::Map(entries) => {
                let mut saved = Vec::with_capacity(entries.len());
                for (name, value) in entries {
                    let Value::String(path) = value.value else {
                        return Err(Problem {
                            location: value.referenced,
                            message: format!(
                                "step {index}: the `save` path for `{name}` must be a string, found {}",
                                kind_name(&value.value)
                            ),
                        });
                    };
                    validate_path(&path).map_err(|message| Problem {
                        location: value.referenced,
                        message: format!(
                            "step {index}: the `save` path for `{name}` is invalid: {message}"
                        ),
                    })?;
                    saved.push(SavedValue { name, path });
                }
                saved
            }
            other => {
                return Err(Problem {
                    location: node.referenced,
                    message: format!(
                        "step {index}: `save` must be a mapping of variable names to JSON paths, found {}",
                        other.kind()
                    ),
                });
            }
        },
    };

    Ok(Step {
        action,
        args: args.value,
        expect,
        save,
    })
}

const COMPARISONS: [&str; 4] = ["equals", "not_equals", "contains", "exists"];

fn parse_expectation(
    step: usize,
    position: usize,
    item: &Spanned<Value>,
) -> Result<Expectation, Problem> {
    let location = item.referenced;
    let Value::Object(entries) = &item.value else {
        return Err(Problem {
            location,
            message: format!(
                "step {step}: assertion {position} must be a mapping with `path` and one comparison, found {}",
                kind_name(&item.value)
            ),
        });
    };
    let mut entries = entries.clone();

    let path = match entries.remove("path") {
        Some(Value::String(path)) => path,
        Some(other) => {
            return Err(Problem {
                location,
                message: format!(
                    "step {step}: assertion {position}: `path` must be a string, found {}",
                    kind_name(&other)
                ),
            });
        }
        None => {
            return Err(Problem {
                location,
                message: format!("step {step}: assertion {position}: `path` is required"),
            });
        }
    };
    validate_path(&path).map_err(|message| Problem {
        location,
        message: format!(
            "step {step}: assertion {position}: `{path}` is not a valid JSON path: {message}"
        ),
    })?;

    let mut found = Vec::new();
    for comparison in COMPARISONS {
        if let Some(value) = entries.remove(comparison) {
            found.push((comparison, value));
        }
    }
    if !entries.is_empty() {
        let unknown: Vec<String> = entries.keys().cloned().collect();
        return Err(Problem {
            location,
            message: format!(
                "step {step}: assertion {position}: unknown field(s) {}; use one of {}",
                unknown.join(", "),
                COMPARISONS.join(", ")
            ),
        });
    }
    let (comparison, expected) = match found.len() {
        0 => {
            return Err(Problem {
                location,
                message: format!(
                    "step {step}: assertion {position}: expected one of {}",
                    COMPARISONS.join(", ")
                ),
            });
        }
        1 => found.remove(0),
        _ => {
            let names: Vec<&str> = found.iter().map(|(name, _)| *name).collect();
            return Err(Problem {
                location,
                message: format!(
                    "step {step}: assertion {position}: expected exactly one comparison, found {}",
                    names.join(", ")
                ),
            });
        }
    };

    let matcher = match comparison {
        "equals" => Matcher::Equals(expected),
        "not_equals" => Matcher::NotEquals(expected),
        "contains" => {
            if !expected.is_string() && !expected.is_array() {
                return Err(Problem {
                    location,
                    message: format!(
                        "step {step}: assertion {position}: `contains` compares a string or an array, found {}; use `equals` for other values",
                        kind_name(&expected)
                    ),
                });
            }
            Matcher::Contains(expected)
        }
        "exists" => match expected {
            Value::Bool(wanted) => Matcher::Exists(wanted),
            other => {
                return Err(Problem {
                    location,
                    message: format!(
                        "step {step}: assertion {position}: `exists` takes true or false, found {}",
                        kind_name(&other)
                    ),
                });
            }
        },
        other => {
            return Err(Problem {
                location,
                message: format!(
                    "step {step}: assertion {position}: `{other}` is not a comparison; use one of {}",
                    COMPARISONS.join(", ")
                ),
            });
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

/// Loads a `--vars-file`: a YAML mapping of names to string, number or boolean
/// values.
///
/// The values are read the same way a case's `vars:` are, so a number or a
/// boolean becomes its text form. A problem is reported with the file and, when
/// the parser knows it, the line and column of the offending value; the caller
/// decides the exit code.
pub fn load_vars_file(path: &Path) -> Result<Vec<(String, String)>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "could not read `{}`: {error}; check that the file exists and is UTF-8",
            paths::display(path)
        )
    })?;
    let raw: Spanned<BTreeMap<String, Spanned<Value>>> =
        serde_saphyr::from_str(&text).map_err(|error| {
            format!(
                "{}: {}",
                paths::display(path),
                strip_error_prefix(&error.to_string())
            )
        })?;

    let mut values = Vec::with_capacity(raw.value.len());
    for (name, value) in raw.value {
        let location = value.referenced;
        let text = match value.value {
            Value::String(text) => text,
            Value::Number(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
            Value::Null => {
                return Err(vars_file_problem(
                    path,
                    &text,
                    location,
                    format!("variable `{name}` has no value; use a string, number or boolean"),
                ));
            }
            Value::Array(_) | Value::Object(_) => {
                return Err(vars_file_problem(
                    path,
                    &text,
                    location,
                    format!("variable `{name}` must be a string, number or boolean"),
                ));
            }
        };
        values.push((name, text));
    }
    Ok(values)
}

/// Renders a problem of a `--vars-file` the way a case error is rendered: the
/// file name, then the location and source snippet when there is one.
fn vars_file_problem(path: &Path, source: &str, location: Location, message: String) -> String {
    format!(
        "{}: {}",
        paths::display(path),
        Problem { location, message }.render(source)
    )
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

    /// A case whose first step is `cli.run` followed by `extra`.
    ///
    /// The fixed first lines keep a fixture's own line numbers predictable.
    fn step_with(extra: &str) -> String {
        format!("name: Broken\nsteps:\n  - cli.run:\n      command: ta\n{extra}")
    }

    /// The typical structural mistakes of a case file and the node each one
    /// belongs to, as 1-based line and column numbers.
    fn structural_mistakes() -> Vec<(String, u64, u64, &'static str)> {
        vec![
            (
                step_with("    expect:\n      - path: $.exit_code\n        equal: 0\n"),
                6,
                9,
                "unknown field(s) equal",
            ),
            (
                step_with("    expect:\n      - equals: 0\n"),
                6,
                9,
                "`path` is required",
            ),
            (
                step_with(
                    "    expect:\n      - path: $.exit_code\n        equals: 0\n        contains: \"0\"\n",
                ),
                6,
                9,
                "expected exactly one comparison",
            ),
            (
                "name: Broken\nsteps:\n  - cli.run:\n      command: ta\n    other.step: {}\n"
                    .to_string(),
                3,
                5,
                "expected exactly one step name",
            ),
            (
                step_with("    save:\n      code: 3\n"),
                6,
                13,
                "must be a string",
            ),
            (
                step_with("    expect: 3\n"),
                5,
                13,
                "`expect` must be a list of assertions",
            ),
            (
                step_with("    save: 3\n"),
                5,
                11,
                "`save` must be a mapping",
            ),
        ]
    }

    #[test]
    fn a_structural_error_names_its_line_and_column_with_a_snippet() {
        for (text, line, column, fragment) in structural_mistakes() {
            let error = parse(&text).expect_err("the case must be rejected");

            assert!(
                error.starts_with(&format!("line {line} column {column}: ")),
                "the error must start with its location, got: {error}"
            );
            assert!(error.contains(fragment), "got: {error}");
            // The snippet is the one the YAML parser writes: a header naming the
            // position, the reported source line with its line number and a caret
            // under the column.
            assert!(
                error.contains(&format!(" --> <input>:{line}:{column}\n")),
                "the snippet header must name the position, got: {error}"
            );
            let source = text
                .lines()
                .nth(usize::try_from(line - 1).expect("a fixture line fits a usize"))
                .expect("the reported line must exist");
            assert!(
                error.contains(&format!("{line} | {source}\n")),
                "the snippet must quote line {line}, got: {error}"
            );
            assert!(
                error
                    .lines()
                    .any(|text| text.contains('|') && text.contains('^')),
                "the snippet must keep a caret line, got: {error}"
            );
        }
    }

    #[test]
    fn a_grammar_error_and_a_structural_error_share_the_snippet_layout() {
        let grammar = parse("name: Broken\nsteps:\n  - cli.run: [\n").expect_err("grammar error");
        let structural =
            parse(&step_with("    expect:\n      - equals: 0\n")).expect_err("structural error");

        for error in [grammar, structural] {
            assert!(error.contains(" --> <input>:"), "got: {error}");
            assert!(
                error
                    .lines()
                    .any(|text| text.contains('|') && text.contains('^')),
                "got: {error}"
            );
        }
    }

    #[test]
    fn a_variable_error_names_the_variable_line() {
        let error =
            parse("name: Broken\nvars:\n  port: [8080]\nsteps:\n  - cli.run:\n      command: ta\n")
                .expect_err("the variable must be rejected");

        assert!(error.contains("port"), "got: {error}");
        assert!(error.starts_with("line 3 column 9: "), "got: {error}");
    }

    #[test]
    fn a_case_without_steps_points_at_the_document() {
        let error = parse("name: Broken\nsteps: []\n").expect_err("the case must be rejected");

        assert!(error.contains("no steps"), "got: {error}");
        assert!(error.starts_with("line 1 column 1: "), "got: {error}");
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

    #[test]
    fn a_vars_file_reads_strings_numbers_and_booleans_as_text() {
        let dir = std::env::temp_dir().join(format!("ta-core-vars-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
        let path = dir.join("vars.yaml");
        std::fs::write(
            &path,
            "name: alice\nport: 8080\nratio: 1.5\nenabled: true\ndisabled: false\n",
        )
        .expect("write the vars file");

        let values = load_vars_file(&path).expect("the vars file must load");

        assert_eq!(
            values,
            vec![
                ("disabled".to_string(), "false".to_string()),
                ("enabled".to_string(), "true".to_string()),
                ("name".to_string(), "alice".to_string()),
                ("port".to_string(), "8080".to_string()),
                ("ratio".to_string(), "1.5".to_string()),
            ]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vars_file_value_that_is_not_scalar_names_its_line() {
        let dir = std::env::temp_dir().join(format!("ta-core-vars-type-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
        let path = dir.join("vars.yaml");
        std::fs::write(&path, "name: alice\ntoken:\n  - one\n").expect("write the vars file");

        let error = load_vars_file(&path).expect_err("the value must be rejected");

        assert!(error.contains("vars.yaml"), "got: {error}");
        assert!(error.contains("token"), "got: {error}");
        assert!(error.contains("line 3 column 3: "), "got: {error}");
        assert!(
            error.contains("must be a string, number or boolean"),
            "got: {error}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vars_file_that_is_not_a_mapping_is_reported_with_its_file() {
        let dir = std::env::temp_dir().join(format!("ta-core-vars-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
        let path = dir.join("vars.yaml");
        std::fs::write(&path, "- one\n- two\n").expect("write the vars file");

        let error = load_vars_file(&path).expect_err("the document must be rejected");

        assert!(error.contains("vars.yaml"), "got: {error}");
        assert!(error.contains("line 1"), "got: {error}");

        let missing = load_vars_file(&dir.join("nope.yaml")).expect_err("the file must be missing");
        assert!(missing.contains("nope.yaml"), "got: {missing}");
        assert!(
            missing.contains("check that the file exists"),
            "got: {missing}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
