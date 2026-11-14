//! The `db.query` step.
//!
//! One read-only statement against a SQLite file or a PostgreSQL server. The
//! step owns no connection pool and keeps no state: every case opens, reads and
//! closes, so a query cannot see another case's session settings and a run
//! cannot hold a connection open after it ends.

use std::path::Path;
use std::time::Duration;

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::core::registry::{Registry, StepDefinition, StepError};

/// Name of the step this module registers.
pub const STEP_NAME: &str = "db.query";

const DEFAULT_MAX_ROWS: u64 = 1000;
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

mod postgres;
mod sqlite;

/// Registers the steps of this driver.
pub fn register(registry: &mut Registry) -> Result<(), String> {
    let args_schema = serde_json::to_value(schema_for!(DbQueryArgs))
        .map_err(|error| format!("could not build the schema of `{STEP_NAME}`: {error}"))?;
    registry.register(StepDefinition {
        name: STEP_NAME,
        description: "Runs one read-only SQL query on SQLite or PostgreSQL and returns `columns`, `rows`, `row_count` and `truncated`.",
        args_schema,
        validate,
        run,
    })
}

/// Arguments of `db.query`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DbQueryArgs {
    /// Connection string: `sqlite:<path>` for a SQLite file, `postgres://` or
    /// `postgresql://` for a PostgreSQL server.
    // The scheme is not a `pattern` in the schema: the documented way to pass a
    // URL is `${env:DATABASE_URL}`, which no pattern could accept. `check`
    // enforces the rule on the expanded value instead.
    url: String,
    /// One read-only SQL statement. `?` and `?1` bind SQLite parameters, `$1`
    /// binds PostgreSQL parameters.
    #[schemars(extend("minLength" = 1))]
    sql: String,
    /// Values bound to the statement's placeholders, in order. Each one is a
    /// string, a number, a boolean or null.
    #[serde(default)]
    params: Vec<Param>,
    /// Largest number of rows to return. Defaults to 1000.
    // The schema must enforce the same bound as `check`, so that a value the
    // schema accepts is not rejected later for this argument.
    #[schemars(extend("minimum" = 1))]
    #[serde(default = "default_max_rows")]
    max_rows: u64,
    /// Timeout for connecting and for the whole query, in milliseconds.
    /// Defaults to 30000.
    #[schemars(extend("minimum" = 1))]
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

fn default_max_rows() -> u64 {
    DEFAULT_MAX_ROWS
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

/// One value bound to a placeholder.
///
/// The cases are listed instead of reusing `serde_json::Value` so the argument
/// schema can say that an object or a list is not a parameter, and so binding
/// stays exhaustive: a new JSON type cannot be forgotten here.
///
/// The schema is inlined: the engine copies a step's argument schema into the
/// case schema's `$defs`, where a reference to a definition of its own would
/// point at nothing.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
enum Param {
    /// JSON null.
    Null(()),
    /// A boolean.
    Bool(bool),
    /// A whole number.
    Integer(i64),
    /// A number with a fractional part.
    Float(f64),
    /// A string.
    Text(String),
}

fn validate(args: &Value) -> Result<(), String> {
    check(&parse(args)?)
}

fn parse(args: &Value) -> Result<DbQueryArgs, String> {
    serde_json::from_value(args.clone()).map_err(|error| format!("invalid arguments: {error}"))
}

fn check(args: &DbQueryArgs) -> Result<(), String> {
    target(&args.url)?;
    if args.sql.trim().is_empty() {
        return Err("invalid arguments: `sql` must contain one statement".to_string());
    }
    if args.max_rows == 0 {
        return Err("invalid arguments: `max_rows` must be at least 1".to_string());
    }
    if args.timeout_ms == 0 {
        return Err("invalid arguments: `timeout_ms` must be at least 1".to_string());
    }
    Ok(())
}

/// Where a `url` points.
#[derive(Debug, PartialEq, Eq)]
enum Target<'a> {
    /// A SQLite database file, as written after the `sqlite:` prefix.
    Sqlite(&'a str),
    /// A PostgreSQL connection string, unchanged.
    Postgres(&'a str),
}

/// Reads the scheme of a `url`.
///
/// The message never repeats the URL: a PostgreSQL URL usually carries a
/// password, and an error is the wrong place to copy one into.
fn target(url: &str) -> Result<Target<'_>, String> {
    if let Some(path) = url.strip_prefix("sqlite:") {
        if path.is_empty() {
            return Err(
                "invalid arguments: `url` must name a file after `sqlite:`, for example `sqlite:app.db`"
                    .to_string(),
            );
        }
        return Ok(Target::Sqlite(path));
    }
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        return Ok(Target::Postgres(url));
    }
    Err(
        "invalid arguments: `url` must start with `sqlite:`, `postgres://` or `postgresql://`"
            .to_string(),
    )
}

fn run(args: &Value) -> Result<Value, StepError> {
    let args = parse(args).map_err(StepError::InvalidArguments)?;
    check(&args).map_err(StepError::InvalidArguments)?;
    let timeout = Duration::from_millis(args.timeout_ms);
    match target(&args.url).map_err(StepError::InvalidArguments)? {
        Target::Sqlite(path) => sqlite::query(
            Path::new(path),
            &args.sql,
            &args.params,
            args.max_rows,
            timeout,
        ),
        Target::Postgres(url) => {
            postgres::query(url, &args.sql, &args.params, args.max_rows, timeout)
        }
    }
}

/// Builds the step result from the rows both engines decoded.
fn result(columns: Vec<String>, rows: Vec<Map<String, Value>>, truncated: bool) -> Value {
    let row_count = rows.len();
    json!({
        "columns": columns,
        "rows": rows,
        "row_count": row_count,
        "truncated": truncated,
    })
}

/// The message every refused statement carries.
///
/// A case that tries to write, to send two statements or to run SQL the server
/// does not understand all end here, so the sentence names the one rule that
/// covers all three.
fn refused_statement(error: &impl std::fmt::Display) -> StepError {
    StepError::Environment(format!(
        "`{STEP_NAME}` runs one read-only statement and the database refused this one: {error}"
    ))
}

/// Renders bytes as lower-case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_documented_limits() {
        let args = parse(&json!({"url": "sqlite:app.db", "sql": "SELECT 1"}))
            .expect("the arguments must parse");

        assert!(args.params.is_empty());
        assert_eq!(args.max_rows, 1_000);
        assert_eq!(args.timeout_ms, 30_000);
    }

    #[test]
    fn rejects_an_unknown_argument() {
        let error = validate(&json!({
            "url": "sqlite:app.db",
            "sql": "SELECT 1",
            "rows": 10,
        }))
        .expect_err("the arguments must be refused");

        assert!(error.contains("rows"), "got: {error}");
    }

    #[test]
    fn rejects_a_url_whose_scheme_is_not_a_database() {
        for url in ["mysql://localhost/app", "app.db", "http://localhost/app"] {
            let error = validate(&json!({"url": url, "sql": "SELECT 1"}))
                .expect_err("the url must be refused");

            assert!(error.contains("sqlite:"), "got: {error}");
            assert!(error.contains("postgres://"), "got: {error}");
            assert!(
                !error.contains(url),
                "the message must not repeat the url, got: {error}"
            );
        }
    }

    #[test]
    fn reads_the_scheme_of_each_supported_url() {
        assert_eq!(
            target("sqlite:data/app.db").expect("a sqlite url"),
            Target::Sqlite("data/app.db")
        );
        assert_eq!(
            target("postgres://user@host/app").expect("a postgres url"),
            Target::Postgres("postgres://user@host/app")
        );
        assert_eq!(
            target("postgresql://user@host/app").expect("a postgres url"),
            Target::Postgres("postgresql://user@host/app")
        );
    }

    #[test]
    fn rejects_a_sqlite_url_without_a_path() {
        let error = validate(&json!({"url": "sqlite:", "sql": "SELECT 1"}))
            .expect_err("the url must be refused");

        assert!(error.contains("sqlite:app.db"), "got: {error}");
    }

    #[test]
    fn rejects_an_empty_or_blank_statement() {
        for sql in ["", " ", "\n\t"] {
            let error = validate(&json!({"url": "sqlite:app.db", "sql": sql}))
                .expect_err("the statement must be refused");

            assert!(error.contains("sql"), "got: {error}");
        }
    }

    #[test]
    fn rejects_a_zero_max_rows_or_timeout() {
        let error = validate(&json!({
            "url": "sqlite:app.db",
            "sql": "SELECT 1",
            "max_rows": 0,
        }))
        .expect_err("the arguments must be refused");
        assert!(error.contains("max_rows"), "got: {error}");

        let error = validate(&json!({
            "url": "sqlite:app.db",
            "sql": "SELECT 1",
            "timeout_ms": 0,
        }))
        .expect_err("the arguments must be refused");
        assert!(error.contains("timeout_ms"), "got: {error}");
    }

    #[test]
    fn accepts_only_scalar_parameters() {
        let args = parse(&json!({
            "url": "sqlite:app.db",
            "sql": "SELECT 1",
            "params": [null, true, 42, 1.5, "text"],
        }))
        .expect("scalar parameters must parse");

        assert!(matches!(args.params[0], Param::Null(())));
        assert!(matches!(args.params[1], Param::Bool(true)));
        assert!(matches!(args.params[2], Param::Integer(42)));
        assert!(matches!(args.params[3], Param::Float(value) if value == 1.5));
        assert!(matches!(args.params[4], Param::Text(ref text) if text == "text"));

        let schema = serde_json::to_value(schema_for!(DbQueryArgs))
            .expect("the argument schema must serialize");
        for params in [json!([[1, 2]]), json!([{"a": 1}])] {
            let arguments = json!({
                "url": "sqlite:app.db",
                "sql": "SELECT 1",
                "params": params,
            });
            assert!(
                !jsonschema::is_valid(&schema, &arguments),
                "the schema must refuse a non-scalar parameter: {arguments}"
            );
            validate(&arguments).expect_err("a non-scalar parameter must be refused");
        }
    }

    #[test]
    fn the_argument_schema_refuses_what_check_refuses() {
        let schema = serde_json::to_value(schema_for!(DbQueryArgs))
            .expect("the argument schema must serialize");

        for (field, value) in [
            ("sql", json!({"url": "sqlite:a.db", "sql": ""})),
            (
                "max_rows",
                json!({"url": "sqlite:a.db", "sql": "SELECT 1", "max_rows": 0}),
            ),
            (
                "timeout_ms",
                json!({"url": "sqlite:a.db", "sql": "SELECT 1", "timeout_ms": 0}),
            ),
        ] {
            assert!(
                !jsonschema::is_valid(&schema, &value),
                "the schema must refuse `{field}` in {value}"
            );
            let error = validate(&value).expect_err("validate must refuse the same arguments");
            assert!(error.contains(field), "got: {error}");
        }
    }

    #[test]
    fn the_argument_schema_accepts_an_environment_reference() {
        // The documented way to pass a connection string is `${env:...}`, and
        // the value of the variable is what carries a scheme. A schema that
        // demanded a scheme here would reject every case the guide recommends.
        let schema = serde_json::to_value(schema_for!(DbQueryArgs))
            .expect("the argument schema must serialize");
        let arguments = json!({"url": "${env:DATABASE_URL}", "sql": "SELECT 1"});

        assert!(
            jsonschema::is_valid(&schema, &arguments),
            "the schema must accept {arguments}"
        );
        parse(&arguments).expect("the arguments must parse");
    }

    #[test]
    fn a_refused_statement_says_the_step_is_read_only() {
        let error = refused_statement(&"attempt to write a readonly database");

        let StepError::Environment(message) = error else {
            panic!("a refused statement must be an environment error");
        };
        assert!(message.contains("read-only"), "got: {message}");
        assert!(message.contains("one"), "got: {message}");
    }

    #[test]
    fn bytes_are_rendered_as_lower_case_hexadecimal() {
        assert_eq!(hex(&[0x00, 0xff, 0x10]), "00ff10");
        assert_eq!(hex(&[]), "");
    }
}
