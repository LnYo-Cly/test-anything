//! The `http.request` step.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::time::Duration;

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::core::registry::{Registry, StepDefinition, StepError};
use crate::core::session::Sessions;

/// Name of the step this module registers.
pub const STEP_NAME: &str = "http.request";

const DEFAULT_METHOD: &str = "GET";
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Registers the steps of this driver.
pub fn register(registry: &mut Registry) -> Result<(), String> {
    let args_schema = serde_json::to_value(schema_for!(HttpRequestArgs))
        .map_err(|error| format!("could not build the schema of `{STEP_NAME}`: {error}"))?;
    registry.register(StepDefinition {
        name: STEP_NAME,
        description: "Sends an HTTP request and returns `status`, `headers` and `body`.",
        args_schema,
        validate,
        run,
    })
}

/// Arguments of `http.request`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HttpRequestArgs {
    /// HTTP method. Defaults to `GET`.
    #[serde(default = "default_method")]
    method: String,
    /// Absolute URL, including the `http://` or `https://` scheme.
    url: String,
    /// Request headers.
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// Query parameters appended to the URL.
    #[serde(default)]
    query: BTreeMap<String, String>,
    /// JSON request body. Mutually exclusive with `body`.
    #[serde(default)]
    json: Option<Value>,
    /// Raw request body. Mutually exclusive with `json`.
    #[serde(default)]
    body: Option<String>,
    /// Timeout for the whole request, in milliseconds. Defaults to 30000.
    // The schema must enforce the same bound as `check`, so that a value the
    // schema accepts is not rejected later for this argument.
    #[schemars(extend("minimum" = 1))]
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

fn default_method() -> String {
    DEFAULT_METHOD.to_string()
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn validate(args: &Value) -> Result<(), String> {
    check(&parse(args)?)
}

fn parse(args: &Value) -> Result<HttpRequestArgs, String> {
    serde_json::from_value(args.clone()).map_err(|error| format!("invalid arguments: {error}"))
}

fn check(args: &HttpRequestArgs) -> Result<(), String> {
    if args.method.is_empty() || !args.method.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return Err(format!(
            "invalid arguments: `method` must be an HTTP method such as `GET`, found `{}`",
            args.method
        ));
    }
    if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
        return Err(format!(
            "invalid arguments: `url` must start with `http://` or `https://`, found `{}`",
            args.url
        ));
    }
    if args.json.is_some() && args.body.is_some() {
        return Err(
            "invalid arguments: `json` and `body` cannot both be set; remove one".to_string(),
        );
    }
    if args.timeout_ms == 0 {
        return Err("invalid arguments: `timeout_ms` must be at least 1".to_string());
    }
    Ok(())
}

fn run(args: &Value, _sessions: &mut Sessions) -> Result<Value, StepError> {
    let args = parse(args).map_err(StepError::InvalidArguments)?;
    check(&args).map_err(StepError::InvalidArguments)?;
    execute(&args)
}

fn execute(args: &HttpRequestArgs) -> Result<Value, StepError> {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_millis(args.timeout_ms)))
        .build();
    let agent: ureq::Agent = config.into();

    let url = with_query(&args.url, &args.query);
    let mut builder = ureq::http::Request::builder()
        .method(args.method.as_str())
        .uri(url);
    for (name, value) in &args.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }

    let payload = payload(args)?;
    if let Some(content_type) = payload.content_type
        && !has_header(&args.headers, "content-type")
    {
        builder = builder.header("content-type", content_type);
    }

    let response = match payload.body {
        Some(body) => agent.run(builder.body(body).map_err(invalid_request)?),
        None => agent.run(builder.body(()).map_err(invalid_request)?),
    };
    let mut response = response.map_err(classify)?;

    let status = response.status().as_u16();
    let headers = Value::Object(response_headers(response.headers()));
    let text = response.body_mut().read_to_string().map_err(classify)?;
    // A body that is not JSON stays a string so that plain text can be tested.
    let body = serde_json::from_str(&text).unwrap_or(Value::String(text));

    Ok(json!({
        "status": status,
        "headers": headers,
        "body": body,
    }))
}

struct Payload {
    body: Option<Vec<u8>>,
    content_type: Option<&'static str>,
}

fn payload(args: &HttpRequestArgs) -> Result<Payload, StepError> {
    if let Some(value) = &args.json {
        let body = serde_json::to_vec(value).map_err(|error| {
            StepError::Execution(format!("could not serialize the `json` argument: {error}"))
        })?;
        return Ok(Payload {
            body: Some(body),
            content_type: Some("application/json"),
        });
    }
    Ok(match &args.body {
        Some(body) => Payload {
            body: Some(body.clone().into_bytes()),
            content_type: None,
        },
        None => Payload {
            body: None,
            content_type: None,
        },
    })
}

fn invalid_request(error: ureq::http::Error) -> StepError {
    StepError::InvalidArguments(format!(
        "invalid arguments: the request could not be built: {error}"
    ))
}

fn has_header(headers: &BTreeMap<String, String>, wanted: &str) -> bool {
    headers.keys().any(|name| name.eq_ignore_ascii_case(wanted))
}

fn with_query(url: &str, query: &BTreeMap<String, String>) -> String {
    if query.is_empty() {
        return url.to_string();
    }
    let mut pairs = String::new();
    for (name, value) in query {
        if !pairs.is_empty() {
            pairs.push('&');
        }
        pairs.push_str(&encode(name));
        pairs.push('=');
        pairs.push_str(&encode(value));
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}{pairs}")
}

/// Percent-encodes one query component, keeping the unreserved characters.
fn encode(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(char::from(byte));
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn response_headers(headers: &ureq::http::HeaderMap) -> Map<String, Value> {
    let mut map = Map::new();
    for (name, value) in headers {
        let key = name.as_str().to_ascii_lowercase();
        let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
        match map.get_mut(&key) {
            Some(Value::String(existing)) => {
                existing.push_str(", ");
                existing.push_str(&text);
            }
            _ => {
                map.insert(key, Value::String(text));
            }
        }
    }
    map
}

fn classify(error: ureq::Error) -> StepError {
    match error {
        ureq::Error::Timeout(_) => StepError::Timeout(format!("the request timed out: {error}")),
        ureq::Error::Io(io) if io.kind() == ErrorKind::TimedOut => {
            StepError::Timeout(format!("the request timed out: {io}"))
        }
        ureq::Error::HostNotFound => {
            StepError::Environment("could not resolve the host name; check `url`".to_string())
        }
        ureq::Error::ConnectionFailed => {
            StepError::Environment(format!("the connection failed: {error}"))
        }
        ureq::Error::Io(io) => StepError::Environment(format!("the connection failed: {io}")),
        ureq::Error::BadUri(uri) => {
            StepError::InvalidArguments(format!("invalid arguments: `{uri}` is not a valid URL"))
        }
        ureq::Error::Http(inner) => {
            StepError::InvalidArguments(format!("invalid arguments: {inner}"))
        }
        ureq::Error::StatusCode(code) => {
            StepError::Environment(format!("the request returned status {code}"))
        }
        other => StepError::Environment(format!("the request failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_get_and_the_documented_timeout() {
        let args = parse(&json!({"url": "http://127.0.0.1:1/"})).expect("the arguments must parse");

        assert_eq!(args.method, "GET");
        assert_eq!(args.timeout_ms, 30_000);
    }

    #[test]
    fn rejects_an_unknown_argument() {
        let error = validate(&json!({"url": "http://x/", "metod": "GET"}))
            .expect_err("the arguments must be refused");

        assert!(error.contains("metod"), "got: {error}");
    }

    #[test]
    fn rejects_json_and_body_together() {
        let error = validate(&json!({
            "url": "http://x/",
            "json": {"a": 1},
            "body": "text",
        }))
        .expect_err("the arguments must be refused");

        assert!(error.contains("cannot both be set"), "got: {error}");
    }

    #[test]
    fn rejects_a_url_without_a_scheme() {
        let error = validate(&json!({"url": "127.0.0.1:8080/health"}))
            .expect_err("the arguments must be refused");

        assert!(error.contains("http://"), "got: {error}");
    }

    #[test]
    fn rejects_a_zero_timeout() {
        let error = validate(&json!({"url": "http://x/", "timeout_ms": 0}))
            .expect_err("the arguments must be refused");

        assert!(error.contains("timeout_ms"), "got: {error}");
    }

    #[test]
    fn appends_query_parameters_and_percent_encodes_them() {
        let query = BTreeMap::from([
            ("page".to_string(), "2".to_string()),
            ("q".to_string(), "a b&c".to_string()),
        ]);

        assert_eq!(
            with_query("http://x/search", &query),
            "http://x/search?page=2&q=a%20b%26c"
        );
        assert_eq!(
            with_query("http://x/search?all=1", &query),
            "http://x/search?all=1&page=2&q=a%20b%26c"
        );
        assert_eq!(
            with_query("http://x/search", &BTreeMap::new()),
            "http://x/search"
        );
    }
}
