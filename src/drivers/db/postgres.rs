//! The PostgreSQL half of `db.query`.
//!
//! Two independent layers keep a query from writing, and both are server-side
//! settings rather than client-side filtering. The session is switched to
//! read-only, and the statement runs in an explicit `READ ONLY` transaction
//! that is always rolled back. A statement list never reaches either layer: the
//! extended protocol prepares exactly one statement, so a second one is a
//! syntax error at parse time.

use std::time::Duration;

use postgres::error::SqlState;
use postgres::types::private::BytesMut;
use postgres::types::{IsNull, ToSql, Type};
use postgres::{Client, Row, Statement, Transaction};
use serde_json::{Map, Value, json};
use tokio_postgres_rustls::MakeRustlsConnect;

use super::{Param, hex, refused_statement, result};
use crate::core::registry::StepError;

/// Runs `sql` against the PostgreSQL server `url` names.
pub(super) fn query(
    url: &str,
    sql: &str,
    params: &[Param],
    max_rows: u64,
    timeout: Duration,
) -> Result<Value, StepError> {
    let mut client = connect(url, timeout)?;
    // The first layer: everything this session does is read-only, whether or
    // not the statement below is wrapped in a transaction.
    client
        .batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
        .map_err(|error| plan_error(&error))?;

    // The second layer: an explicit read-only transaction that is rolled back.
    let mut transaction = client
        .build_transaction()
        .read_only(true)
        .start()
        .map_err(|error| plan_error(&error))?;
    // The server ends a statement that runs too long, so the timeout holds even
    // when the statement never returns to the client.
    if let Err(error) = transaction.batch_execute(&format!(
        "SET LOCAL statement_timeout = {}",
        timeout.as_millis()
    )) {
        return Err(plan_error(&error));
    }

    let prepared = transaction.prepare(sql).map_err(|error| {
        // The extended protocol refuses a statement list here, before anything
        // runs; every other parse error lands here too.
        refused_statement(&error)
    })?;
    let prepared = cast_unsupported_columns(&mut transaction, sql, prepared)?;

    let columns: Vec<String> = prepared
        .columns()
        .iter()
        .map(|column| column.name().to_string())
        .collect();
    let types: Vec<Type> = prepared
        .columns()
        .iter()
        .map(|column| column.type_().clone())
        .collect();
    let bound = bind_all(&prepared, params)?;
    let references: Vec<&(dyn ToSql + Sync)> = bound.iter().map(|value| value.as_ref()).collect();

    let rows = transaction
        .query(&prepared, &references)
        .map_err(|error| execute_error(&error))?;

    let mut decoded = Vec::new();
    let mut truncated = false;
    for row in &rows {
        if decoded.len() as u64 == max_rows {
            truncated = true;
            break;
        }
        let mut object = Map::with_capacity(columns.len());
        for (index, name) in columns.iter().enumerate() {
            object.insert(name.clone(), cell(row, index, &types[index], name)?);
        }
        decoded.push(object);
    }

    // Nothing this step ran may survive it. A rollback that fails leaves the
    // connection unusable, and the client closes it when it is dropped, so the
    // failure needs no message of its own.
    let _ = transaction.rollback();

    Ok(result(columns, decoded, truncated))
}

/// Opens one connection, with TLS unless the URL disables it.
fn connect(url: &str, timeout: Duration) -> Result<Client, StepError> {
    let mut config: postgres::Config = url.parse().map_err(|error| {
        StepError::InvalidArguments(format!(
            "invalid arguments: `url` is not a PostgreSQL connection string: {error}"
        ))
    })?;
    config.connect_timeout(timeout);

    // `tokio-postgres-rustls` builds its configuration from the process-wide
    // crypto provider, and `ureq` already relies on the same one. Installing it
    // again is harmless: the second attempt reports the first provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = MakeRustlsConnect::with_webpki_roots();

    config.connect(tls).map_err(|error| connect_error(&error))
}

fn connect_error(error: &postgres::Error) -> StepError {
    if let Some(io) = io_error(error)
        && matches!(
            io.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        )
    {
        return StepError::Timeout(format!(
            "the connection to the PostgreSQL server timed out; increase `timeout_ms` or check `url`: {}",
            error
        ));
    }
    StepError::Environment(format!(
        "could not connect to the PostgreSQL database: {error}; check `url`, that the server is running and that the network reaches it"
    ))
}

/// The innermost [`std::io::Error`] of an error chain, if it holds one.
fn io_error(error: &postgres::Error) -> Option<&std::io::Error> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>() {
            return Some(io);
        }
        source = current.source();
    }
    None
}

/// A parameter that is SQL NULL whatever type the placeholder has.
///
/// `Option<T>` refuses a type `T` does not accept, and a NULL parameter has no
/// value that could choose a type, so this type accepts every one.
#[derive(Debug)]
struct Null;

impl ToSql for Null {
    fn to_sql(
        &self,
        _ty: &Type,
        _out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        Ok(IsNull::Yes)
    }

    // `postgres-types` checks `accepts` before `to_sql`, and a NULL has no
    // value whose type could be checked, so both sides accept everything.
    fn to_sql_checked(
        &self,
        _ty: &Type,
        _out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        Ok(IsNull::Yes)
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }
}

fn bind_all(
    statement: &Statement,
    params: &[Param],
) -> Result<Vec<Box<dyn ToSql + Sync>>, StepError> {
    let types = statement.params();
    let mut bound: Vec<Box<dyn ToSql + Sync>> = Vec::with_capacity(params.len());
    for (index, param) in params.iter().enumerate() {
        bound.push(bind(param, types.get(index)));
    }
    Ok(bound)
}

fn bind(param: &Param, ty: Option<&Type>) -> Box<dyn ToSql + Sync> {
    match param {
        Param::Null(()) => Box::new(Null),
        Param::Bool(flag) => Box::new(*flag),
        // A whole number sent to a floating-point placeholder has to become a
        // float, or the server refuses the parameter's type.
        Param::Integer(number) => match ty {
            Some(ty) if matches!(*ty, Type::FLOAT4 | Type::FLOAT8) => Box::new(*number as f64),
            _ => Box::new(*number),
        },
        Param::Float(number) => Box::new(*number),
        Param::Text(text) => Box::new(text.clone()),
    }
}

/// Whether a column can be decoded without help from the server.
///
/// Everything else — `numeric`, `uuid`, the date and time types, enums, arrays,
/// ranges and the rest — is asked for as text instead. `postgres` has no Rust
/// type for them without an extra feature and a matching Rust type, and a
/// mapping table that only covers what this crate happens to implement would
/// fail on the column a case actually needs.
fn decoded_directly(ty: &Type) -> bool {
    matches!(
        *ty,
        Type::BOOL
            | Type::INT2
            | Type::INT4
            | Type::INT8
            | Type::OID
            | Type::FLOAT4
            | Type::FLOAT8
            | Type::TEXT
            | Type::VARCHAR
            | Type::BPCHAR
            | Type::NAME
            | Type::UNKNOWN
            | Type::JSON
            | Type::JSONB
            | Type::BYTEA
    )
}

/// Prepares a query that returns the unsupported columns as text.
///
/// The statement is wrapped rather than rewritten, so the query the case wrote
/// stays the query the server runs; only the projection changes. Preparing the
/// wrapper also keeps the one-statement rule: the case's SQL is a subquery, and
/// a subquery cannot hold a second statement.
fn cast_unsupported_columns(
    transaction: &mut Transaction<'_>,
    sql: &str,
    prepared: Statement,
) -> Result<Statement, StepError> {
    let unsupported: Vec<usize> = prepared
        .columns()
        .iter()
        .enumerate()
        .filter(|(_, column)| !decoded_directly(column.type_()))
        .map(|(index, _)| index)
        .collect();
    if unsupported.is_empty() {
        return Ok(prepared);
    }

    let wrapped = projection(
        sql,
        &prepared
            .columns()
            .iter()
            .map(|column| column.name().to_string())
            .collect::<Vec<_>>(),
        &unsupported,
    );
    transaction
        .prepare(&wrapped)
        .map_err(|error| refused_statement(&error))
}

fn projection(sql: &str, columns: &[String], unsupported: &[usize]) -> String {
    let names: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let name = quote(column);
            if unsupported.contains(&index) {
                format!("{name}::text AS {name}")
            } else {
                name
            }
        })
        .collect();
    format!(
        "SELECT {} FROM ({}) AS ta_db_query",
        names.join(", "),
        without_terminator(sql)
    )
}

/// Quotes an identifier, so a column whose name holds a quote still works.
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Removes the `;` that may end the statement and the space around it.
///
/// A terminator is valid at the top level but not inside the subquery the
/// wrapping adds.
fn without_terminator(sql: &str) -> &str {
    let sql = sql.trim();
    match sql.strip_suffix(';') {
        Some(rest) => rest.trim_end(),
        None => sql,
    }
}

/// Maps one cell to the JSON the step returns.
fn cell(row: &Row, index: usize, ty: &Type, name: &str) -> Result<Value, StepError> {
    let decoded = match *ty {
        Type::BOOL => row
            .try_get::<_, Option<bool>>(index)
            .map(|value| value.map_or(Value::Null, Value::Bool)),
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => row
            .try_get::<_, Option<i64>>(index)
            .map(|value| value.map_or(Value::Null, |number| json!(number))),
        Type::FLOAT4 | Type::FLOAT8 => row
            .try_get::<_, Option<f64>>(index)
            .map(|value| value.map_or(Value::Null, |number| json!(number))),
        Type::JSON | Type::JSONB => row
            .try_get::<_, Option<Value>>(index)
            .map(|value| value.unwrap_or(Value::Null)),
        // A byte string has no text form; it is shown as it is.
        Type::BYTEA => row
            .try_get::<_, Option<Vec<u8>>>(index)
            .map(|value| value.map_or(Value::Null, |bytes| Value::String(hex(&bytes)))),
        _ => row
            .try_get::<_, Option<String>>(index)
            .map(|value| value.map_or(Value::Null, Value::String)),
    };

    decoded.map_err(|error| {
        StepError::Environment(format!(
            "could not decode column `{name}` of type `{ty}`: {error}"
        ))
    })
}

/// Maps a failure of the statement or of its transaction.
fn execute_error(error: &postgres::Error) -> StepError {
    match error.code() {
        Some(code) if *code == SqlState::QUERY_CANCELED => StepError::Timeout(format!(
            "the query did not finish within the timeout and was cancelled by the server; increase `timeout_ms`: {error}"
        )),
        Some(code) if *code == SqlState::READ_ONLY_SQL_TRANSACTION => {
            StepError::Environment(format!(
                "`db.query` runs one read-only statement and the database refused this one because it writes: {error}"
            ))
        }
        _ => refused_statement(error),
    }
}

/// Maps a failure that happens before a statement runs.
fn plan_error(error: &postgres::Error) -> StepError {
    if error
        .code()
        .is_some_and(|code| *code == SqlState::QUERY_CANCELED)
    {
        return StepError::Timeout(format!(
            "the database did not answer within the timeout; increase `timeout_ms`: {error}"
        ));
    }
    StepError::Environment(format!(
        "could not start the read-only query: {error}; check that `url` names a reachable server"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_statement_terminator_is_removed_before_wrapping() {
        assert_eq!(without_terminator("SELECT 1"), "SELECT 1");
        assert_eq!(without_terminator("SELECT 1;"), "SELECT 1");
        assert_eq!(without_terminator("SELECT 1;  \n"), "SELECT 1");
        assert_eq!(without_terminator("  SELECT 1  "), "SELECT 1");
    }

    #[test]
    fn identifiers_are_quoted_so_a_quote_in_a_name_stays_one_name() {
        assert_eq!(quote("id"), "\"id\"");
        assert_eq!(quote("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn a_null_parameter_is_accepted_by_every_type() {
        for ty in [Type::TEXT, Type::INT4, Type::JSONB, Type::NUMERIC] {
            assert!(Null::accepts(&ty), "`{ty}` must accept a NULL parameter");
        }
    }

    #[test]
    fn the_native_types_are_decoded_without_casting() {
        for ty in [
            Type::BOOL,
            Type::INT4,
            Type::INT8,
            Type::FLOAT8,
            Type::TEXT,
            Type::VARCHAR,
            Type::JSONB,
            Type::BYTEA,
        ] {
            assert!(decoded_directly(&ty), "`{ty}` must be decoded directly");
        }
        for ty in [
            Type::NUMERIC,
            Type::UUID,
            Type::DATE,
            Type::TIMESTAMP,
            Type::TIMESTAMPTZ,
            Type::INTERVAL,
        ] {
            assert!(!decoded_directly(&ty), "`{ty}` must be requested as text");
        }
    }

    #[test]
    fn a_projection_casts_only_the_columns_that_need_it() {
        // The column list is what `prepare` reports, and the wrapper keeps the
        // names so a case can still assert on `rows[0].total`.
        let sql = "SELECT total, name FROM orders;";
        let wrapped = projection(sql, &["total".to_string(), "name".to_string()], &[0]);

        assert_eq!(
            wrapped,
            "SELECT \"total\"::text AS \"total\", \"name\" FROM (SELECT total, name FROM orders) AS ta_db_query"
        );
    }
}
