//! The SQLite half of `db.query`.
//!
//! Two independent layers keep a query from writing. The file is opened with
//! `SQLITE_OPEN_READONLY`, so the connection has no write permission at all and
//! a missing file is an error instead of a new database; the connection then
//! sets `PRAGMA query_only`, so a statement is refused even if a later change
//! opened the file writable. Removing either layer still refuses every write;
//! the tests in `tests/db.rs` prove both.

use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, ErrorCode, OpenFlags, params_from_iter};
use serde_json::{Map, Value, json};

use super::{Param, hex, refused_statement, result};
use crate::core::paths::display;
use crate::core::registry::StepError;

/// Runs `sql` against the SQLite file at `path`.
pub(super) fn query(
    path: &Path,
    sql: &str,
    params: &[Param],
    max_rows: u64,
    timeout: Duration,
) -> Result<Value, StepError> {
    let connection = open(path)?;
    connection
        .busy_timeout(timeout)
        .map_err(|error| open_error(path, &error))?;

    // `prepare` compiles exactly one statement and refuses a second one, so a
    // statement list never reaches the engine.
    let mut statement = connection.prepare(sql).map_err(|error| match error {
        rusqlite::Error::MultipleStatement => StepError::Environment(
            "`db.query` runs one read-only statement, and this `sql` holds more than one; send them as separate steps"
                .to_string(),
        ),
        other => refused_statement(&other),
    })?;

    let columns: Vec<String> = statement.column_names().map(str::to_string).collect();

    let watchdog = Watchdog::start(&connection, timeout);
    let mut rows = statement
        .query(params_from_iter(params.iter().map(bind)))
        .map_err(|error| execute_error(&error))?;

    let mut decoded = Vec::new();
    let mut truncated = false;
    loop {
        let row = match rows.next() {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => return Err(execute_error(&error)),
        };
        if decoded.len() as u64 == max_rows {
            // The row that does not fit is only proof that the result was
            // longer than `max_rows`, so it is not decoded.
            truncated = true;
            break;
        }
        let mut object = Map::with_capacity(columns.len());
        for (index, name) in columns.iter().enumerate() {
            let value = row.get_ref(index).map_err(|error| execute_error(&error))?;
            object.insert(name.clone(), cell(value));
        }
        decoded.push(object);
    }
    drop(rows);
    drop(watchdog);

    Ok(result(columns, decoded, truncated))
}

/// Opens the database file without the right to create or change it.
fn open(path: &Path) -> Result<Connection, StepError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| open_error(path, &error))
        .and_then(|connection| {
            // The second layer. `query_only` is a connection setting, not a
            // file change, and it survives a caller that opened the file
            // writable.
            connection
                .pragma_update(None, "query_only", true)
                .map_err(|error| open_error(path, &error))?;
            Ok(connection)
        })
}

fn open_error(path: &Path, error: &rusqlite::Error) -> StepError {
    StepError::Environment(format!(
        "could not open the SQLite database `{}`: {error}; check that the file exists and is readable 鈥?`db.query` never creates one",
        display(path)
    ))
}

/// Maps a failure of the statement itself.
///
/// The busy and interrupt codes are the two ways the timeout ends a query
/// early, so they are reported as a timeout instead of a database error.
fn execute_error(error: &rusqlite::Error) -> StepError {
    match error {
        rusqlite::Error::SqliteFailure(failure, message)
            if matches!(
                failure.code,
                ErrorCode::DatabaseBusy
                    | ErrorCode::DatabaseLocked
                    | ErrorCode::OperationInterrupted
            ) =>
        {
            let detail = message.as_deref().unwrap_or("the database was busy");
            StepError::Timeout(format!(
                "the query did not finish within the timeout and was stopped: {detail}; increase `timeout_ms`"
            ))
        }
        other => refused_statement(other),
    }
}

/// Turns one parameter into the value SQLite binds.
///
/// SQLite has no boolean type, so a boolean becomes `1` or `0`, the way a
/// `flag INTEGER` column stores it.
fn bind(param: &Param) -> SqlValue {
    match param {
        Param::Null(()) => SqlValue::Null,
        Param::Bool(flag) => SqlValue::Integer(i64::from(*flag)),
        Param::Integer(number) => SqlValue::Integer(*number),
        Param::Float(number) => SqlValue::Real(*number),
        Param::Text(text) => SqlValue::Text(text.clone()),
    }
}

/// Maps one SQLite value to the JSON the step returns.
fn cell(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => json!(number),
        ValueRef::Real(number) => json!(number),
        ValueRef::Text(bytes) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
        // SQLite has no text form for a blob, and a lossy decode would invent
        // one, so the bytes are shown as they are.
        ValueRef::Blob(bytes) => Value::String(hex(bytes)),
    }
}

/// Stops the connection if a statement runs longer than the step's timeout.
///
/// `rusqlite` has no progress callback that can bound a single statement, but
/// an interrupt handle may be used from another thread. The handle is only used
/// while the statement is running, so it cannot land on a later statement.
struct Watchdog {
    cancel: Option<Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl Watchdog {
    fn start(connection: &Connection, timeout: Duration) -> Self {
        let interrupts = connection.get_interrupt_handle();
        let (cancel, cancelled) = mpsc::channel();
        let handle = thread::spawn(move || {
            if cancelled.recv_timeout(timeout).is_err() {
                interrupts.interrupt();
            }
        });
        Self {
            cancel: Some(cancel),
            handle: Some(handle),
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A directory removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("da-db-unit-{}-{label}", std::process::id()));
            std::fs::create_dir_all(&path).expect("the temporary directory must be created");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(dir: &TempDir) -> PathBuf {
        let path = dir.join("app.db");
        let connection = Connection::open(&path).expect("the fixture must open");
        connection
            .execute_batch(
                "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, data BLOB);
                 INSERT INTO items VALUES (1, 'alice', X'00ff');",
            )
            .expect("the fixture must be written");
        path
    }

    fn params(values: &[Param]) -> Vec<Param> {
        values
            .iter()
            .map(|param| match param {
                Param::Null(()) => Param::Null(()),
                Param::Bool(flag) => Param::Bool(*flag),
                Param::Integer(number) => Param::Integer(*number),
                Param::Float(number) => Param::Float(*number),
                Param::Text(text) => Param::Text(text.clone()),
            })
            .collect()
    }

    #[test]
    fn reads_rows_and_maps_every_type() {
        let dir = TempDir::new("types");
        let database = fixture(&dir);

        let queried = query(
            &database,
            "SELECT id, name, data FROM items",
            &[],
            100,
            Duration::from_secs(5),
        )
        .expect("the query must run");

        assert_eq!(queried["columns"], json!(["id", "name", "data"]));
        assert_eq!(
            queried["rows"],
            json!([{"id": 1, "name": "alice", "data": "00ff"}])
        );
        assert_eq!(queried["row_count"], 1);
        assert_eq!(queried["truncated"], false);
    }

    #[test]
    fn binds_every_parameter_kind() {
        let dir = TempDir::new("bind");
        let database = fixture(&dir);
        let bound = params(&[
            Param::Integer(1),
            Param::Text("alice".to_string()),
            Param::Float(1.0),
            Param::Bool(true),
            Param::Null(()),
        ]);

        let queried = query(
            &database,
            "SELECT id FROM items WHERE id = ?1 AND name = ?2 AND ?3 = 1.0 AND ?4 = 1 AND ?5 IS NULL",
            &bound,
            100,
            Duration::from_secs(5),
        )
        .expect("the query must run");

        assert_eq!(queried["row_count"], 1, "got: {queried}");
    }

    #[test]
    fn truncates_after_max_rows_and_reports_it() {
        let dir = TempDir::new("truncate");
        let database = fixture(&dir);
        let connection = Connection::open(&database).expect("the fixture must open");
        connection
            .execute_batch("INSERT INTO items (id, name) VALUES (2, 'bob'), (3, 'carol');")
            .expect("the rows must be written");

        let queried = query(
            &database,
            "SELECT id FROM items ORDER BY id",
            &[],
            2,
            Duration::from_secs(5),
        )
        .expect("the query must run");

        assert_eq!(queried["row_count"], 2);
        assert_eq!(queried["truncated"], true);
    }

    #[test]
    fn refuses_a_second_statement() {
        let dir = TempDir::new("multi");
        let database = fixture(&dir);

        let error = query(
            &database,
            "SELECT 1; DELETE FROM items",
            &[],
            100,
            Duration::from_secs(5),
        )
        .expect_err("the statement list must be refused");

        let StepError::Environment(message) = error else {
            panic!("a refused statement must be an environment error");
        };
        assert!(message.contains("read-only"), "got: {message}");
        assert!(message.contains("more than one"), "got: {message}");
    }

    #[test]
    fn a_missing_file_is_an_environment_error_and_is_not_created() {
        let dir = TempDir::new("missing");
        let database = dir.join("missing.db");

        let error = query(&database, "SELECT 1", &[], 100, Duration::from_secs(5))
            .expect_err("a missing database must fail");

        let StepError::Environment(message) = error else {
            panic!("a missing database must be an environment error");
        };
        assert!(message.contains("missing.db"), "got: {message}");
        assert!(!database.exists(), "the file must not be created");
    }

    #[test]
    fn a_write_is_refused_and_the_rows_survive() {
        let dir = TempDir::new("write");
        let database = fixture(&dir);

        for sql in [
            "INSERT INTO items (id, name) VALUES (2, 'bob')",
            "UPDATE items SET name = 'changed'",
            "DELETE FROM items",
            "DROP TABLE items",
            "WITH gone AS (DELETE FROM items RETURNING *) SELECT * FROM gone",
        ] {
            let error = query(&database, sql, &[], 100, Duration::from_secs(5))
                .expect_err("a write must be refused");
            let StepError::Environment(message) = error else {
                panic!("`{sql}` must be an environment error");
            };
            assert!(message.contains("read-only"), "got: {message}");
        }

        let queried = query(
            &database,
            "SELECT id, name FROM items",
            &[],
            100,
            Duration::from_secs(5),
        )
        .expect("the rows must still be readable");
        assert_eq!(queried["rows"], json!([{"id": 1, "name": "alice"}]));
    }
}
