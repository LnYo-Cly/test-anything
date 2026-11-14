// Each integration test target compiles this module separately and uses a
// different subset of its helpers.
#![allow(dead_code)]

//! Helpers shared by the integration tests: a local HTTP server and temporary
//! case files. Nothing here reaches the network beyond loopback.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A request the test server received.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

impl Request {
    /// The query string of the request target, or an empty string.
    pub fn query(&self) -> &str {
        self.target
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or("")
    }

    /// The path part of the request target.
    pub fn path(&self) -> &str {
        self.target
            .split_once('?')
            .map(|(path, _)| path)
            .unwrap_or(&self.target)
    }
}

/// How long the `/sleep` route waits before it answers.
pub const SLEEP_ROUTE_DELAY: Duration = Duration::from_millis(1000);

/// A minimal HTTP server bound to a loopback port, started by the test.
pub struct TestServer {
    port: u16,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Starts a server on a random loopback port.
    pub fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind the test server");
        let port = listener
            .local_addr()
            .expect("failed to read the test server address")
            .port();
        listener
            .set_nonblocking(true)
            .expect("failed to make the test server non-blocking");

        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let server_requests = Arc::clone(&requests);
        let server_stop = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            while !server_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    // One thread per connection: a test that starts several
                    // requests at once must be answered in parallel, the way a
                    // real server answers, or a slowly answering route would
                    // serialize the clients that the test runs concurrently.
                    // The threads are not joined; they only hold the shared
                    // request log, which outlives them.
                    Ok((stream, _)) => {
                        let requests = Arc::clone(&server_requests);
                        thread::spawn(move || handle_connection(stream, &requests));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            port,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    /// The loopback port the server listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// An absolute URL for `path` on this server.
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    /// Every request the server received so far.
    pub fn requests(&self) -> Vec<Request> {
        self.requests
            .lock()
            .expect("test server requests mutex is poisoned")
            .clone()
    }

    /// The number of requests the server received so far.
    pub fn request_count(&self) -> usize {
        self.requests
            .lock()
            .expect("test server requests mutex is poisoned")
            .len()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(stream: TcpStream, requests: &Mutex<Vec<Request>>) {
    // Accepting from a non-blocking listener yields a non-blocking socket on
    // some platforms, so the connection mode is set back to blocking here.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    requests
        .lock()
        .expect("test server requests mutex is poisoned")
        .push(Request {
            method: method.clone(),
            target: target.clone(),
            headers: headers.clone(),
            body: body.clone(),
        });

    let (path, query) = target
        .split_once('?')
        .map_or((target.as_str(), ""), |(path, query)| (path, query));
    let response = route(&method, path, query, &headers, &body);

    let mut stream = reader.into_inner();
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn route(
    method: &str,
    path: &str,
    query: &str,
    headers: &BTreeMap<String, String>,
    body: &str,
) -> String {
    match (method, path) {
        (_, "/health") => json_response("200 OK", r#"{"status":"ok"}"#),
        ("POST", "/login") => json_response("200 OK", r#"{"token":"t-123","user":"alice"}"#),
        (_, "/me") => {
            let authorized = headers
                .get("authorization")
                .is_some_and(|value| value == "Bearer t-123");
            if authorized {
                json_response("200 OK", r#"{"name":"alice"}"#)
            } else {
                json_response("401 Unauthorized", r#"{"error":"unauthorized"}"#)
            }
        }
        (_, "/slow") => {
            thread::sleep(Duration::from_millis(400));
            json_response("200 OK", r#"{"slow":true}"#)
        }
        // A wait long enough that four cases overlap for a measurable time when
        // they run at once, and dominate the run when they do not.
        (_, "/sleep") => {
            thread::sleep(SLEEP_ROUTE_DELAY);
            json_response("200 OK", r#"{"slept":true}"#)
        }
        (_, "/status/500") => json_response("500 Internal Server Error", r#"{"error":"boom"}"#),
        (_, "/text") => text_response("200 OK", "hello world"),
        // A fixed body with one number, one boolean and one null, so a test can
        // assert on how each JSON scalar is redacted without depending on a
        // duration or a timestamp.
        (_, "/scalars") => json_response("200 OK", r#"{"count":10,"flag":true,"nothing":null}"#),
        // Writes a file while the request is answered, so a case can produce
        // log lines during its own step: an observer sees only what changed
        // between its start and its collect call.
        ("POST", "/write-log") => match write_log(body) {
            Ok(()) => json_response("200 OK", r#"{"written":true}"#),
            Err(error) => json_response(
                "500 Internal Server Error",
                &serde_json::json!({"error": error}).to_string(),
            ),
        },
        (_, "/echo") => {
            let echo = serde_json::json!({
                "method": method,
                "path": path,
                "query": query,
                "headers": headers,
                "body": body,
            });
            json_response("200 OK", &echo.to_string())
        }
        _ => json_response("404 Not Found", r#"{"error":"not found"}"#),
    }
}

/// Appends to or rewrites the file named in a `/write-log` request body.
///
/// The body is a JSON object with `path` and `text`, plus `append` to add to
/// the file instead of replacing it. A missing file is created.
fn write_log(body: &str) -> Result<(), String> {
    let request: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("the body must be a JSON object: {error}"))?;
    let path = request["path"]
        .as_str()
        .ok_or("`path` must be a string naming the file to write")?;
    let text = request["text"]
        .as_str()
        .ok_or("`text` must be a string holding the content")?;
    let append = request["append"].as_bool().unwrap_or(false);

    let mut options = fs::OpenOptions::new();
    options.create(true).write(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("could not open {path}: {error}"))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("could not write {path}: {error}"))?;
    file.flush()
        .map_err(|error| format!("could not flush {path}: {error}"))
}

fn json_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn text_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A temporary directory removed when the value is dropped.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates an empty temporary directory with a unique name.
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "da-integration-{}-{label}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("failed to create the temporary directory");
        Self { path }
    }

    /// The directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `contents` to `name` inside the directory, creating parents.
    pub fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create a case directory");
        }
        fs::write(&path, contents).expect("failed to write a case file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Returns a loopback port that nothing is listening on.
pub fn closed_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind a probe socket");
    let port = listener
        .local_addr()
        .expect("failed to read the probe address")
        .port();
    drop(listener);
    port
}
