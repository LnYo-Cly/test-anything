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
use postgres::fallible_iterator::FallibleIterator;
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
        refused_statement(&detail(&error))
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
    // Rows are streamed, so a large table stops at `max_rows + 1` instead of
    // being loaded whole.
    let mut rows = transaction
        .query_raw(&prepared, bound.iter().map(|value| value.as_ref()))
        .map_err(|error| execute_error(&error))?;

    let mut decoded = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next().map_err(|error| execute_error(&error))? {
        if decoded.len() as u64 == max_rows {
            truncated = true;
            break;
        }
        let mut object = Map::with_capacity(columns.len());
        for (index, name) in columns.iter().enumerate() {
            object.insert(name.clone(), cell(&row, index, &types[index], name)?);
        }
        decoded.push(object);
    }

    drop(rows);

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
            "invalid arguments: `url` is not a PostgreSQL connection string: {}",
            detail(&error)
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
            detail(error)
        ));
    }
    StepError::Environment(format!(
        "could not connect to the PostgreSQL database: {}; check `url`, that the server is running and that the network reaches it",
        detail(error)
    ))
}

/// The server's own words for a failure.
///
/// [`postgres::Error`] prints the literal string `db error` and keeps the
/// reason in its source, so a bare `{error}` would make a syntax error, a
/// missing table and a read-only refusal look the same. A server error carries
/// severity, SQLSTATE and message; anything else, a network failure for example,
/// is spelled out through its source chain.
fn detail(error: &postgres::Error) -> String {
    if let Some(failure) = error.as_db_error() {
        // The severities are localized; the SQLSTATE and the message are the
        // part a user can act on.
        return format!(
            "{} (SQLSTATE {}): {}",
            failure.severity(),
            failure.code().code(),
            failure.message()
        );
    }
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(current) = source {
        let message = current.to_string();
        if !text.contains(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        source = current.source();
    }
    text
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
        bound.push(bind(index, param, types.get(index))?);
    }
    Ok(bound)
}

/// Chooses the Rust type the placeholder asks for.
///
/// `postgres-types` implements `ToSql` for `i64` on `INT8` only and for `f64`
/// on `FLOAT8` only, so a value serialized as `i64` is refused by an `integer`
/// placeholder before the server sees it. The placeholder's own type, which the
/// prepared statement reports, decides the width here.
fn bind(
    index: usize,
    param: &Param,
    ty: Option<&Type>,
) -> Result<Box<dyn ToSql + Sync>, StepError> {
    let ty = ty.unwrap_or(&Type::TEXT);
    Ok(match param {
        Param::Null(()) => Box::new(Null),
        Param::Bool(flag) => Box::new(*flag),
        Param::Integer(number) => match *ty {
            Type::INT2 => Box::new(fitting::<i16>(index, *number, ty)?),
            Type::INT4 => Box::new(fitting::<i32>(index, *number, ty)?),
            Type::OID => Box::new(fitting::<u32>(index, *number, ty)?),
            Type::FLOAT4 => Box::new(*number as f32),
            Type::FLOAT8 => Box::new(*number as f64),
            _ => Box::new(*number),
        },
        Param::Float(number) => match *ty {
            Type::FLOAT4 => Box::new(*number as f32),
            Type::FLOAT8 => Box::new(*number),
            Type::INT2 => Box::new(whole::<i16>(index, *number, ty)?),
            Type::INT4 => Box::new(whole::<i32>(index, *number, ty)?),
            Type::OID => Box::new(whole::<u32>(index, *number, ty)?),
            _ => Box::new(*number),
        },
        Param::Text(text) => Box::new(text.clone()),
    })
}

/// Narrows a whole number to the placeholder's width, or explains why it cannot.
fn fitting<T>(index: usize, value: i64, ty: &Type) -> Result<T, StepError>
where
    T: TryFrom<i64>,
{
    T::try_from(value).map_err(|_| does_not_fit(index, &value.to_string(), ty))
}

/// Narrows a fractional number that happens to hold a whole value.
fn whole<T>(index: usize, value: f64, ty: &Type) -> Result<T, StepError>
where
    T: TryFrom<i64>,
{
    if value.fract() != 0.0 || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(does_not_fit(index, &value.to_string(), ty));
    }
    fitting::<T>(index, value as i64, ty)
}

fn does_not_fit(index: usize, value: &str, ty: &Type) -> StepError {
    StepError::InvalidArguments(format!(
        "invalid arguments: `params[{index}]` is {value}, which does not fit the `{}` ({ty}) placeholder; change the value or cast the placeholder, for example `$1::bigint`",
        sql_name(ty)
    ))
}

/// The SQL name of a type, which is what a user writes in a cast.
fn sql_name(ty: &Type) -> String {
    let name = match *ty {
        Type::INT2 => "smallint",
        Type::INT4 => "integer",
        Type::INT8 => "bigint",
        Type::OID => "oid",
        Type::FLOAT4 => "real",
        Type::FLOAT8 => "double precision",
        Type::BOOL => "boolean",
        Type::TEXT => "text",
        Type::VARCHAR => "varchar",
        Type::NUMERIC => "numeric",
        _ => return ty.name().to_string(),
    };
    name.to_string()
}

/// Whether a column can be decoded without help from the server.
///
/// Everything else 閳?`numeric`, `uuid`, the date and time types, enums, arrays,
/// ranges and the rest 閳?is asked for as text instead. `postgres` has no Rust
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
        .map_err(|error| refused_statement(&detail(&error)))
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
    // The statement goes on a line of its own: an end-of-line comment at its
    // end would otherwise swallow the closing parenthesis, and the query would
    // fail on the wrapper instead of running.
    format!(
        "SELECT {} FROM (\n{}\n) AS ta_db_query",
        names.join(", "),
        without_terminator(sql)
    )
}

/// Quotes an identifier, so a column whose name holds a quote still works.
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Removes the terminator and the comments that follow it.
///
/// A `;` is valid at the top level but not inside the subquery the wrapping
/// adds, and it is not always the last character: `SELECT 1; -- note` ends with
/// a comment. The terminator is found by scanning, so a `;` inside a string
/// literal, a quoted identifier, a dollar-quoted body or a comment is left
/// alone.
fn without_terminator(sql: &str) -> &str {
    match terminator(sql) {
        Some(end) => sql[..end].trim(),
        None => sql.trim(),
    }
}

/// The byte index of the `;` that ends the statement, if there is one.
fn terminator(sql: &str) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' => index = after_quoted(bytes, index, b'\''),
            b'"' => index = after_quoted(bytes, index, b'"'),
            b'-' if bytes.get(index + 1) == Some(&b'-') => index = after_line_comment(bytes, index),
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = after_block_comment(bytes, index)
            }
            b'$' => index = after_dollar_quoted(bytes, index),
            b';' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

/// Skips a quoted run, where a doubled quote is an escaped one.
fn after_quoted(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == quote {
            if bytes.get(index + 1) == Some(&quote) {
                index += 2;
                continue;
            }
            return index + 1;
        }
        index += 1;
    }
    bytes.len()
}

fn after_line_comment(bytes: &[u8], start: usize) -> usize {
    match bytes[start..].iter().position(|byte| *byte == b'\n') {
        Some(offset) => start + offset + 1,
        None => bytes.len(),
    }
}

/// Skips a block comment; the server nests them, so this does too.
fn after_block_comment(bytes: &[u8], start: usize) -> usize {
    let mut depth = 0_usize;
    let mut index = start;
    while index < bytes.len() {
        if bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'*') {
            depth += 1;
            index += 2;
            continue;
        }
        if bytes.get(index) == Some(&b'*') && bytes.get(index + 1) == Some(&b'/') {
            depth -= 1;
            index += 2;
            if depth == 0 {
                return index;
            }
            continue;
        }
        index += 1;
    }
    bytes.len()
}

/// Skips a `$tag$ 閳?$tag$` body, or one byte when this is not a dollar quote.
fn after_dollar_quoted(bytes: &[u8], start: usize) -> usize {
    let mut tag_end = start + 1;
    while tag_end < bytes.len()
        && (bytes[tag_end].is_ascii_alphanumeric() || bytes[tag_end] == b'_')
    {
        tag_end += 1;
    }
    if bytes.get(tag_end) != Some(&b'$') {
        return start + 1;
    }
    let tag = &bytes[start..=tag_end];
    let mut index = tag_end + 1;
    while index + tag.len() <= bytes.len() {
        if &bytes[index..index + tag.len()] == tag {
            return index + tag.len();
        }
        index += 1;
    }
    bytes.len()
}

/// Maps one cell to the JSON the step returns.
fn cell(row: &Row, index: usize, ty: &Type, name: &str) -> Result<Value, StepError> {
    // `postgres-types` decodes `i64` from `INT8` only and `f64` from `FLOAT8`
    // only; each narrower width needs its own Rust type or the decode fails
    // before any value is read.
    let decoded = match *ty {
        Type::BOOL => row
            .try_get::<_, Option<bool>>(index)
            .map(|value| value.map_or(Value::Null, Value::Bool)),
        Type::INT2 => row
            .try_get::<_, Option<i16>>(index)
            .map(|value| value.map_or(Value::Null, |number| json!(number))),
        Type::INT4 => row
            .try_get::<_, Option<i32>>(index)
            .map(|value| value.map_or(Value::Null, |number| json!(number))),
        Type::INT8 => row
            .try_get::<_, Option<i64>>(index)
            .map(|value| value.map_or(Value::Null, |number| json!(number))),
        Type::OID => row
            .try_get::<_, Option<u32>>(index)
            .map(|value| value.map_or(Value::Null, |number| json!(number))),
        Type::FLOAT4 => row
            .try_get::<_, Option<f32>>(index)
            .map(|value| value.map_or(Value::Null, |number| json!(number))),
        Type::FLOAT8 => row
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
            "could not decode column `{name}` of type `{ty}`: {}",
            detail(&error)
        ))
    })
}

/// Maps a failure of the statement or of its transaction.
fn execute_error(error: &postgres::Error) -> StepError {
    match error.code() {
        Some(code) if *code == SqlState::QUERY_CANCELED => StepError::Timeout(format!(
            "the query did not finish within the timeout and was cancelled by the server; increase `timeout_ms`: {}",
            detail(error)
        )),
        Some(code) if *code == SqlState::READ_ONLY_SQL_TRANSACTION => {
            StepError::Environment(format!(
                "`db.query` runs one read-only statement and the database refused this one because it writes: {}",
                detail(error)
            ))
        }
        _ => refused_statement(&detail(error)),
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
        "could not start the read-only query: {}; check that `url` names a reachable server",
        detail(error)
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
        // names so a case can still assert on `rows[0].total`. The statement
        // sits on its own line so an end-of-line comment cannot comment out the
        // closing parenthesis.
        let sql = "SELECT total, name FROM orders;";
        let wrapped = projection(sql, &["total".to_string(), "name".to_string()], &[0]);

        assert_eq!(
            wrapped,
            "SELECT \"total\"::text AS \"total\", \"name\" FROM (\nSELECT total, name FROM orders\n) AS ta_db_query"
        );
    }

    #[test]
    fn a_terminator_followed_by_a_comment_is_removed_with_the_comment() {
        // A `;` is legal at the top level and illegal inside the subquery, and
        // it is not always the last character.
        assert_eq!(without_terminator("SELECT 1; -- note"), "SELECT 1");
        assert_eq!(without_terminator("SELECT 1 -- note"), "SELECT 1 -- note");
        assert_eq!(without_terminator("SELECT 1;"), "SELECT 1");
        assert_eq!(without_terminator("SELECT 1;  \n"), "SELECT 1");
        assert_eq!(without_terminator("  SELECT 1  "), "SELECT 1");
        assert_eq!(
            without_terminator("SELECT 1 /* done */"),
            "SELECT 1 /* done */"
        );
    }

    #[test]
    fn a_semicolon_inside_a_literal_is_not_a_terminator() {
        for sql in [
            "SELECT 'a;b'",
            "SELECT \"odd;name\" FROM t",
            "SELECT 1 -- a; b",
            "SELECT 1 /* a; b */",
            "SELECT $$a;b$$",
            "SELECT $tag$a;b$tag$",
        ] {
            assert_eq!(
                without_terminator(sql),
                sql,
                "`{sql}` holds no statement terminator"
            );
        }
        assert_eq!(
            without_terminator("SELECT 'a;b'; -- done"),
            "SELECT 'a;b'",
            "the terminator after the literal is still found"
        );
    }

    #[test]
    fn a_block_comment_is_skipped_even_when_it_nests() {
        assert_eq!(
            without_terminator("SELECT 1 /* outer /* inner; */ still */;"),
            "SELECT 1 /* outer /* inner; */ still */"
        );
    }
}
