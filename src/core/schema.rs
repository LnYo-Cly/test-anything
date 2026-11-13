//! JSON Schema for the case file format.
//!
//! The schema mirrors the rules `case::parse` enforces: the top level has
//! exactly `name`, the four classification fields, `vars` and `steps`; a step
//! carries exactly one registered step name plus optional `expect` and `save`;
//! and every assertion uses one of the four comparisons. Step argument schemas
//! come from the registry, so a step's declared schema and the exported schema
//! cannot drift apart.

use serde_json::{Map, Value, json};

use crate::core::registry::{Registry, StepDefinition};

/// Builds a JSON Schema for one case file.
///
/// The schema is a draft 2020-12 document. It describes the shape of a case but
/// not the values that only the parser can check, such as whether a JSON path
/// is syntactically valid or whether a variable is defined.
pub fn case_schema(registry: &Registry) -> Value {
    let mut definitions = Map::new();
    definitions.insert("expect".to_string(), expectation_schema());
    definitions.insert("save".to_string(), save_schema());

    let mut branches = Vec::new();
    for name in registry.names() {
        let Some(definition) = registry.get(name) else {
            continue;
        };
        definitions.insert(name.to_string(), step_arguments_schema(definition));
        branches.push(step_branch(name));
    }

    // An empty `oneOf` is not a valid schema, and a build without registered
    // steps must still produce a legal document. `false` accepts no step,
    // which is exactly what `ta run` does with no driver.
    let step_items = if branches.is_empty() {
        Value::Bool(false)
    } else {
        json!({ "oneOf": branches })
    };

    json!({
        "$schema": crate::CASE_SCHEMA_DIALECT,
        "title": "ta case file",
        "description": "A declarative test case: a name, optional classification, optional variables and an ordered list of steps.",
        "type": "object",
        "additionalProperties": false,
        "required": ["name", "steps"],
        "properties": {
            "name": {
                // The parser reads the name as text and accepts a YAML
                // string, number or boolean scalar there.
                "type": ["string", "number", "boolean"],
                "description": "Human-readable case name shown in reports."
            },
            "project": {
                "type": "string",
                "pattern": "\\S",
                "description": "Project the case belongs to. Select it with `--project`."
            },
            "suite": {
                "type": "string",
                "pattern": "\\S",
                "description": "Suite the case belongs to. Select it with `--suite`."
            },
            "layer": {
                "type": "string",
                "pattern": "\\S",
                "description": "Free-form layer of the case, for example `unit`, `api`, `cli`, `ui`, `integration`, `e2e` or `db`. Select it with `--layer`."
            },
            "tags": {
                "type": "array",
                "description": "Tags of the case. Select them with `--tag` and exclude them with `--skip-tag`.",
                "uniqueItems": true,
                "items": {
                    "type": "string",
                    "pattern": "\\S"
                }
            },
            "vars": {
                "type": "object",
                "description": "Variables available to every step as ${name}. Values are converted to text.",
                "additionalProperties": {
                    "type": ["string", "number", "boolean"]
                }
            },
            "steps": {
                "type": "array",
                "description": "Steps executed in order until one fails.",
                "minItems": 1,
                "items": step_items
            }
        },
        "$defs": Value::Object(definitions)
    })
}

/// One allowed shape of a step: exactly one step name plus `expect` and `save`.
fn step_branch(name: &str) -> Value {
    let mut properties = Map::new();
    properties.insert(
        name.to_string(),
        json!({ "$ref": format!("#/$defs/{name}") }),
    );
    properties.insert("expect".to_string(), json!({ "$ref": "#/$defs/expect" }));
    properties.insert("save".to_string(), json!({ "$ref": "#/$defs/save" }));

    json!({
        "type": "object",
        "description": format!("A step that runs `{name}`."),
        "additionalProperties": false,
        "required": [name],
        "properties": Value::Object(properties)
    })
}

/// The step's own argument schema, with its description and no dialect header.
fn step_arguments_schema(definition: &StepDefinition) -> Value {
    let Value::Object(mut schema) = definition.args_schema.clone() else {
        // A boolean argument schema is already complete and cannot carry a
        // description. No built-in driver uses one.
        return definition.args_schema.clone();
    };
    schema.remove("$schema");
    schema.remove("title");
    schema.insert(
        "description".to_string(),
        Value::String(definition.description.to_string()),
    );
    Value::Object(schema)
}

/// The list of assertions allowed in `expect`.
fn expectation_schema() -> Value {
    json!({
        "type": "array",
        "description": "Assertions evaluated against the step result.",
        "items": {
            "oneOf": [
                comparison_branch("equals", Value::Bool(true)),
                comparison_branch("not_equals", Value::Bool(true)),
                comparison_branch("contains", json!({
                    "type": ["string", "array"],
                    "description": "A substring of a string or an element of an array."
                })),
                comparison_branch("exists", json!({
                    "type": "boolean",
                    "description": "`true` requires a value, `false` requires none."
                }))
            ]
        }
    })
}

/// One assertion shape: `path` plus exactly one comparison.
fn comparison_branch(comparison: &str, value_schema: Value) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "path".to_string(),
        json!({
            "type": "string",
            "description": "JSONPath expression into the step result, for example `$.body.token`."
        }),
    );
    properties.insert(comparison.to_string(), value_schema);

    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["path", comparison],
        "properties": Value::Object(properties)
    })
}

/// The mapping of variable names to JSON paths that `save` accepts.
fn save_schema() -> Value {
    json!({
        "type": "object",
        "description": "Copies values out of the step result into variables for later steps.",
        "additionalProperties": {
            "type": "string",
            "description": "JSONPath expression into the step result."
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::StepDefinition;
    use serde_json::json;

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry
            .register(StepDefinition {
                name: "test.echo",
                description: "Returns its arguments.",
                args_schema: json!({
                    "$schema": "dialect-placeholder",
                    "title": "Arguments",
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"value": {"type": "string"}}
                }),
                validate: |_| Ok(()),
                run: |_| Ok(Value::Null),
            })
            .expect("the test step must register");
        registry
    }

    #[test]
    fn the_case_shape_refuses_unknown_fields_and_empty_step_lists() {
        let schema = case_schema(&Registry::new());

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["name", "steps"]));
        assert_eq!(
            schema["properties"]["name"]["type"],
            json!(["string", "number", "boolean"]),
            "the parser accepts every YAML scalar as the case name"
        );
        assert_eq!(schema["properties"]["steps"]["minItems"], 1);
    }

    #[test]
    fn the_classification_fields_are_optional_strings_and_tags_are_unique() {
        let schema = case_schema(&Registry::new());
        let properties = &schema["properties"];

        for field in ["project", "suite", "layer"] {
            assert_eq!(properties[field]["type"], "string", "{field}");
            assert_eq!(properties[field]["pattern"], "\\S", "{field}");
        }
        assert_eq!(properties["tags"]["type"], "array");
        assert_eq!(properties["tags"]["uniqueItems"], true);
        assert_eq!(properties["tags"]["items"]["type"], "string");
        assert_eq!(properties["tags"]["items"]["pattern"], "\\S");
        assert!(
            schema["required"]
                .as_array()
                .is_some_and(|required| !required.iter().any(|field| {
                    matches!(field.as_str(), Some("project" | "suite" | "layer" | "tags"))
                })),
            "classification must stay optional: {}",
            schema["required"]
        );
    }

    #[test]
    fn a_build_without_steps_uses_false_as_the_step_item() {
        let schema = case_schema(&Registry::new());

        assert_eq!(schema["properties"]["steps"]["items"], Value::Bool(false));
        assert_eq!(
            schema["$defs"],
            json!({"expect": expectation_schema(), "save": save_schema()})
        );
    }

    #[test]
    fn every_step_is_a_definition_with_its_description_and_a_reference() {
        let schema = case_schema(&registry());
        let definition = &schema["$defs"]["test.echo"];

        assert_eq!(definition["type"], "object");
        assert_eq!(definition["description"], "Returns its arguments.");
        assert_eq!(definition["title"], Value::Null);
        assert_eq!(definition["$schema"], Value::Null);
        assert_eq!(definition["properties"]["value"]["type"], "string");

        let branches = schema["properties"]["steps"]["items"]["oneOf"]
            .as_array()
            .expect("registered steps must produce a oneOf");
        assert_eq!(branches.len(), 1);
        assert_eq!(
            branches[0]["properties"]["test.echo"]["$ref"],
            "#/$defs/test.echo"
        );
    }

    #[test]
    fn an_assertion_requires_a_path_and_exactly_one_comparison() {
        let expect = expectation_schema();
        let branches = expect["items"]["oneOf"]
            .as_array()
            .expect("the four comparisons must produce a oneOf");

        assert_eq!(branches.len(), 4);
        for branch in branches {
            assert_eq!(branch["additionalProperties"], false);
            assert_eq!(branch["required"][0], "path");
            assert_eq!(branch["required"].as_array().map(Vec::len), Some(2));
        }
    }
}
