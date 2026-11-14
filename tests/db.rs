//! End-to-end tests for the `db.query` step.
//!
//! Every SQLite test prepares its own database file with `rusqlite` in a
//! temporary directory and then runs the compiled binary against it, so no
//! database file is ever committed. The PostgreSQL tests need a server: they
//! read `TA_TEST_POSTGRES_URL` and print why they are skipped when it is unset.

// The whole file needs the built-in database step; a build without it compiles
// an empty test target.
#![cfg(feature = "db")]
// Every test uses a different subset of the helpers below.
#![allow(dead_code, unused_imports)]

mod support;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use serde_json::Value;
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

/// Runs the binary with `dir` as its working directory and extra variables.
fn run_ta_with_env(dir: &Path, args: &[&str], environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(ta());
    command.current_dir(dir).args(args);
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().expect("failed to run the ta binary")
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

/// Everything the run said, on either channel.
///
/// A run with `--output json` writes the whole report, including the message of
/// every problem, to standard output; standard error only carries diagnostics.
fn messages(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

/// The `rows` of a successful single-step run, as JSON.
fn rows_of(output: &Output) -> Value {
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!("stdout must be one JSON value: {error}\n{}", stdout(output))
    });
    parsed["cases"][0]["steps"][0]["result"]["rows"].clone()
}

/// Runs one case and returns its output.
fn run_case(dir: &TempDir, case: &str) -> Output {
    let path = dir.write("case.yaml", case);
    run_ta_in(
        dir.path(),
        &["--output", "json", "run", path_str(&path), "--no-bundle"],
    )
}

/// Statement used by the write gate, with the data it would change.
///
/// Every entry must be refused: a plain write in either case, a write hidden
/// behind a leading comment, a write inside a common table expression, and a
/// second statement behind a read.
const WRITE_STATEMENTS: &[&str] = &[
    "INSERT INTO items (id, name) VALUES (99, 'mallory')",
    "insert into items (id, name) values (98, 'mallory')",
    "UPDATE items SET name = 'changed' WHERE id = 1",
    "DELETE FROM items WHERE id = 1",
    "CREATE TABLE extra (id INTEGER)",
    "DROP TABLE items",
    "-- a comment first\nINSERT INTO items (id, name) VALUES (97, 'mallory')",
    "WITH gone AS (DELETE FROM items RETURNING *) SELECT * FROM gone",
    "SELECT 1; INSERT INTO items (id, name) VALUES (96, 'mallory')",
    "  \n\tDELETE FROM items\n",
];

/// Creates a SQLite database with the rows every test asserts on.
fn sqlite_fixture(path: &Path) {
    let connection = rusqlite::Connection::open(path).expect("the fixture database must open");
    connection
        .execute_batch(
            "CREATE TABLE items (
                 id INTEGER PRIMARY KEY,
                 name TEXT,
                 score REAL,
                 flag INTEGER,
                 note TEXT,
                 data BLOB
             );
             INSERT INTO items VALUES (1, 'alice', 1.5, 1, NULL, X'00ff');
             INSERT INTO items VALUES (2, 'bob', 2.25, 0, 'hello', NULL);
             INSERT INTO items VALUES (3, 'carol', 3.0, 1, 'bye', X'10');",
        )
        .expect("the fixture schema must be written");
}

/// The names and rows of the fixture table, read straight from the file.
fn sqlite_rows(path: &Path) -> Vec<(i64, String)> {
    let connection = rusqlite::Connection::open(path).expect("the fixture database must open");
    let mut statement = connection
        .prepare("SELECT id, name FROM items ORDER BY id")
        .expect("the fixture table must be readable");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("the fixture rows must be readable")
        .collect::<Result<Vec<_>, _>>()
        .expect("the fixture rows must decode")
}

fn fixture(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("app.db");
    sqlite_fixture(&path);
    path
}

#[test]
fn one_query_returns_columns_rows_and_the_json_types() {
    let dir = TempDir::new("db-types");
    fixture(&dir);
    let case = r#"
name: Reads the order rows
steps:
  - db.query:
      url: sqlite:app.db
      sql: SELECT id, name, score, flag, note, data FROM items ORDER BY id
    expect:
      - path: $.columns
        equals: [id, name, score, flag, note, data]
      - path: $.row_count
        equals: 3
      - path: $.truncated
        equals: false
      - path: $.rows[0].id
        equals: 1
      - path: $.rows[0].name
        equals: alice
      - path: $.rows[0].score
        equals: 1.5
      - path: $.rows[0].flag
        equals: 1
      - path: $.rows[0].note
        equals: null
      - path: $.rows[0].data
        equals: "00ff"
      - path: $.rows[1].data
        equals: null
"#;

    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn parameters_are_bound_in_order() {
    let dir = TempDir::new("db-params");
    fixture(&dir);
    let case = r#"
name: Binds parameters
steps:
  - db.query:
      url: sqlite:app.db
      sql: SELECT name, score, flag, note FROM items WHERE id = ?1
      params: [2]
    expect:
      - path: $.rows
        equals:
          - { name: bob, score: 2.25, flag: 0, note: hello }
"#;

    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));

    let case = r#"
name: Binds an anonymous parameter
steps:
  - db.query:
      url: sqlite:app.db
      sql: SELECT name FROM items WHERE name = ? OR id = ?
      params: [carol, 99]
    expect:
      - path: $.rows
        equals:
          - { name: carol }
"#;
    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn a_result_longer_than_max_rows_is_truncated() {
    let dir = TempDir::new("db-max-rows");
    fixture(&dir);
    let case = r#"
name: Truncates
steps:
  - db.query:
      url: sqlite:app.db
      sql: SELECT id FROM items ORDER BY id
      max_rows: 2
    expect:
      - path: $.row_count
        equals: 2
      - path: $.truncated
        equals: true
      - path: $.rows
        equals:
          - { id: 1 }
          - { id: 2 }
"#;

    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn a_missing_sqlite_file_is_an_environment_error_and_is_not_created() {
    let dir = TempDir::new("db-missing");
    let missing = dir.path().join("missing.db");

    let case = r#"
name: Reads a database that is not there
steps:
  - db.query:
      url: sqlite:missing.db
      sql: SELECT 1
"#;
    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(4), "stderr: {}", stderr(&output));
    assert!(
        messages(&output).contains("missing.db"),
        "the message must name the file: {}",
        messages(&output)
    );
    assert!(
        !missing.exists(),
        "a missing database must not be created: {}",
        missing.display()
    );
}

#[test]
fn every_write_statement_is_refused_and_the_data_is_unchanged() {
    let dir = TempDir::new("db-writes");
    let database = fixture(&dir);
    let before = sqlite_rows(&database);
    assert_eq!(before.len(), 3);

    for sql in WRITE_STATEMENTS {
        let case = format!(
            "name: Writes\nsteps:\n  - db.query:\n      url: sqlite:app.db\n      sql: {sql:?}\n"
        );
        let output = run_case(&dir, &case);

        assert_eq!(
            exit_code(&output),
            Some(4),
            "`{sql}` must be refused, stderr: {}\nstdout: {}",
            stderr(&output),
            stdout(&output)
        );
        assert!(
            messages(&output).contains("read-only") || messages(&output).contains("read only"),
            "the message must say the step is read-only for `{sql}`: {}",
            messages(&output)
        );
        assert_eq!(
            sqlite_rows(&database),
            before,
            "`{sql}` changed the database"
        );
    }
}

#[test]
fn a_statement_list_is_refused_before_anything_runs() {
    let dir = TempDir::new("db-multi");
    let database = fixture(&dir);

    let case = r#"
name: Two statements
steps:
  - db.query:
      url: sqlite:app.db
      sql: "SELECT id FROM items; DELETE FROM items"
"#;
    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(4), "stderr: {}", stderr(&output));
    assert_eq!(
        sqlite_rows(&database).len(),
        3,
        "the rows must be unchanged"
    );
}

#[test]
fn a_trailing_semicolon_and_comment_are_one_statement() {
    let dir = TempDir::new("db-trailing");
    fixture(&dir);
    let case = r#"
name: One statement with a trailing comment
steps:
  - db.query:
      url: sqlite:app.db
      sql: "SELECT id FROM items ORDER BY id; -- the end"
    expect:
      - path: $.row_count
        equals: 3
"#;

    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn a_url_with_an_unknown_scheme_is_invalid_arguments() {
    let dir = TempDir::new("db-scheme");
    let case = r#"
name: Unknown scheme
steps:
  - db.query:
      url: mysql://localhost/app
      sql: SELECT 1
"#;
    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(3), "stderr: {}", stderr(&output));
    assert!(
        messages(&output).contains("sqlite:") && messages(&output).contains("postgres://"),
        "the message must name the accepted schemes: {}",
        messages(&output)
    );
}

#[test]
fn a_parameter_that_is_not_a_scalar_is_invalid_arguments() {
    let dir = TempDir::new("db-param-type");
    fixture(&dir);
    let case = r#"
name: A list parameter
steps:
  - db.query:
      url: sqlite:app.db
      sql: SELECT id FROM items WHERE id = ?
      params: [[1, 2]]
"#;
    let output = run_case(&dir, case);

    assert_eq!(exit_code(&output), Some(3), "stderr: {}", stderr(&output));
}

#[test]
fn an_unreachable_postgres_server_is_an_environment_error() {
    let dir = TempDir::new("db-pg-refused");
    let port = support::closed_port();
    let case = format!(
        r#"
name: The database is not there
steps:
  - db.query:
      url: "postgres://ta:secret@127.0.0.1:{port}/app?sslmode=disable"
      sql: SELECT 1
      timeout_ms: 5000
"#
    );
    let output = run_case(&dir, &case);

    assert_eq!(exit_code(&output), Some(4), "stderr: {}", stderr(&output));
    assert!(
        messages(&output).contains("connect") || messages(&output).contains("connection"),
        "the message must say the connection failed: {}",
        messages(&output)
    );
}

#[test]
fn a_connection_string_from_the_environment_is_redacted_everywhere() {
    const SECRET: &str = "s3cr3t-TA-DB-4242";
    const NAME: &str = "TA_TEST_DB_SECRET_DIR";

    let dir = TempDir::new("db-secret");
    let secret_dir = dir.path().join(SECRET);
    std::fs::create_dir_all(&secret_dir).expect("the secret directory must be created");
    sqlite_fixture(&secret_dir.join("app.db"));

    // The second step fails on purpose — it names a database file that is not
    // there — so the path reaches a message, a failure bundle is written and
    // every channel can be searched. The URL is single-quoted: a Windows path
    // holds backslashes, which YAML would read as escapes inside double quotes.
    let case = r#"
name: Reads a database named by an environment variable
steps:
  - db.query:
      url: 'sqlite:${env:TA_TEST_DB_SECRET_DIR}/app.db'
      sql: SELECT name FROM items WHERE id = 1
  - db.query:
      url: 'sqlite:${env:TA_TEST_DB_SECRET_DIR}/missing.db'
      sql: SELECT 1
"#;
    let path = dir.write("case.yaml", case);
    let bundle = dir.path().join("bundles");

    let output = run_ta_with_env(
        dir.path(),
        &[
            "--output",
            "json",
            "run",
            path_str(&path),
            "--bundle-dir",
            path_str(&bundle),
        ],
        &[(NAME, path_str(&secret_dir))],
    );
    assert_eq!(exit_code(&output), Some(4), "stderr: {}", stderr(&output));

    for (channel, content) in [("stdout", stdout(&output)), ("stderr", stderr(&output))] {
        assert!(
            !content.contains(SECRET),
            "{channel} leaked the secret: {content}"
        );
    }
    assert!(
        stdout(&output).contains("***"),
        "the report must show the redaction marker: {}",
        stdout(&output)
    );

    let mut files = 0;
    for entry in walk(&bundle) {
        files += 1;
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        assert!(
            !text.contains(SECRET),
            "{} leaked the secret: {text}",
            entry.display()
        );
    }
    assert!(files > 0, "the failing run must write a failure bundle");
}

/// Every file below `dir`, recursively.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

// ---------------------------------------------------------------------------
// PostgreSQL. The server comes from `TA_TEST_POSTGRES_URL`; without it these
// tests skip and print why.
// ---------------------------------------------------------------------------

/// The PostgreSQL URL the tests run against, if one was configured.
fn postgres_url() -> Option<String> {
    match std::env::var("TA_TEST_POSTGRES_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => None,
    }
}

/// Prints the reason a PostgreSQL test is skipped, or returns the URL.
fn postgres_or_skip(test: &str) -> Option<String> {
    match postgres_url() {
        Some(url) => Some(url),
        None => {
            // The lint that forbids `eprintln!` covers this crate too, and
            // `cargo test -- --nocapture` is where the reason appears.
            let _ = writeln!(
                io::stderr(),
                "skipping `{test}`: TA_TEST_POSTGRES_URL is not set, so there is no PostgreSQL server to query"
            );
            None
        }
    }
}

/// The table the PostgreSQL tests read. It is created once per test process.
fn postgres_table(url: &str) -> &'static str {
    static TABLE: OnceLock<String> = OnceLock::new();
    TABLE.get_or_init(|| {
        let table = format!("ta_db_query_{}", std::process::id());
        let mut client = postgres::Client::connect(url, postgres::NoTls)
            .expect("the PostgreSQL test server must accept the connection");
        client
            .batch_execute(&format!(
                "DROP TABLE IF EXISTS {table};
                 CREATE TABLE {table} (
                     id INTEGER PRIMARY KEY,
                     name TEXT,
                     score DOUBLE PRECISION,
                     flag BOOLEAN,
                     note TEXT
                 );
                 INSERT INTO {table} VALUES (1, 'alice', 1.5, true, NULL);
                 INSERT INTO {table} VALUES (2, 'bob', 2.25, false, 'hello');
                 INSERT INTO {table} VALUES (3, 'carol', 3.0, true, 'bye');"
            ))
            .expect("the PostgreSQL fixture must be written");
        table
    })
}

/// The fixture rows, read straight from the server.
fn postgres_rows(url: &str, table: &str) -> Vec<(i32, String)> {
    let mut client =
        postgres::Client::connect(url, postgres::NoTls).expect("the test server must accept us");
    client
        .query(&format!("SELECT id, name FROM {table} ORDER BY id"), &[])
        .expect("the fixture must be readable")
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect()
}

#[test]
fn postgres_one_query_returns_columns_rows_and_types() {
    let Some(url) = postgres_or_skip("postgres_one_query_returns_columns_rows_and_types") else {
        return;
    };
    let table = postgres_table(&url);
    let dir = TempDir::new("db-pg-types");
    let case = format!(
        r#"
name: Reads the PostgreSQL rows
steps:
  - db.query:
      url: '{url}'
      sql: SELECT id, name, score, flag, note FROM {table} ORDER BY id
    expect:
      - path: $.columns
        equals: [id, name, score, flag, note]
      - path: $.row_count
        equals: 3
      - path: $.truncated
        equals: false
      - path: $.rows[0].id
        equals: 1
      - path: $.rows[0].name
        equals: alice
      - path: $.rows[0].score
        equals: 1.5
      - path: $.rows[0].flag
        equals: true
      - path: $.rows[0].note
        equals: null
"#
    );

    let output = run_case(&dir, &case);

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn postgres_parameters_are_bound_in_order() {
    let Some(url) = postgres_or_skip("postgres_parameters_are_bound_in_order") else {
        return;
    };
    let table = postgres_table(&url);
    let dir = TempDir::new("db-pg-params");
    let case = format!(
        r#"
name: Binds PostgreSQL parameters
steps:
  - db.query:
      url: '{url}'
      sql: SELECT name FROM {table} WHERE id = $1
      params: [2]
    expect:
      - path: $.rows
        equals:
          - {{ name: bob }}
"#
    );

    let output = run_case(&dir, &case);

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn postgres_max_rows_truncates() {
    let Some(url) = postgres_or_skip("postgres_max_rows_truncates") else {
        return;
    };
    let table = postgres_table(&url);
    let dir = TempDir::new("db-pg-max-rows");
    let case = format!(
        r#"
name: Truncates PostgreSQL rows
steps:
  - db.query:
      url: '{url}'
      sql: SELECT id FROM {table} ORDER BY id
      max_rows: 2
    expect:
      - path: $.row_count
        equals: 2
      - path: $.truncated
        equals: true
"#
    );

    let output = run_case(&dir, &case);

    assert_eq!(exit_code(&output), Some(0), "stderr: {}", stderr(&output));
}

#[test]
fn postgres_refuses_every_write_and_leaves_the_data_unchanged() {
    let Some(url) = postgres_or_skip("postgres_refuses_every_write_and_leaves_the_data_unchanged")
    else {
        return;
    };
    let table = postgres_table(&url);
    let before = postgres_rows(&url, table);
    assert_eq!(before.len(), 3);
    let dir = TempDir::new("db-pg-writes");

    for sql in WRITE_STATEMENTS {
        let sql = sql.replace("items", table);
        let case = format!(
            "name: Writes\nsteps:\n  - db.query:\n      url: '{url}'\n      sql: {sql:?}\n"
        );
        let output = run_case(&dir, &case);

        assert_eq!(
            exit_code(&output),
            Some(4),
            "`{sql}` must be refused, stderr: {}",
            stderr(&output)
        );
        assert_eq!(
            postgres_rows(&url, table),
            before,
            "`{sql}` changed the database"
        );
    }
}
