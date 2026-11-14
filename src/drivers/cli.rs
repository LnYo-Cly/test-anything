//! The `cli.run` step.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use wait_timeout::ChildExt;

use crate::core::registry::{Registry, StepDefinition, StepError};
use crate::core::session::Sessions;

/// Name of the step this module registers.
pub const STEP_NAME: &str = "cli.run";

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const READ_CHUNK: usize = 8 * 1024;
/// How long the output pipes may stay open after the program ended.
///
/// A process that outlived the child, for example a daemon it started, keeps
/// the write end of the pipe open. Waiting for it without a bound would hang
/// `da`, so the output read so far is used instead.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Registers the steps of this driver.
pub fn register(registry: &mut Registry) -> Result<(), String> {
    let args_schema = serde_json::to_value(schema_for!(CliRunArgs))
        .map_err(|error| format!("could not build the schema of `{STEP_NAME}`: {error}"))?;
    registry.register(StepDefinition {
        name: STEP_NAME,
        description: "Runs a program without a shell and returns `exit_code`, `stdout`, `stderr` and `json`.",
        args_schema,
        validate,
        run,
    })
}

/// Arguments of `cli.run`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CliRunArgs {
    /// Program name or path. A relative path is resolved against the working directory of `da`.
    command: String,
    /// Arguments passed to the program, one per entry. No shell is involved.
    #[serde(default)]
    args: Vec<String>,
    /// Working directory of the program. Defaults to the working directory of `da`.
    #[serde(default)]
    cwd: Option<PathBuf>,
    /// Environment variables added to the inherited environment, replacing names already set.
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Text written to the program's standard input. Without it the input is empty.
    #[serde(default)]
    stdin: Option<String>,
    /// Timeout for the whole run, in milliseconds. Defaults to 30000.
    // The schema must enforce the same bound as `check`, so that a value the
    // schema accepts is not rejected later for this argument.
    #[schemars(extend("minimum" = 1))]
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn validate(args: &Value) -> Result<(), String> {
    check(&parse(args)?)
}

fn parse(args: &Value) -> Result<CliRunArgs, String> {
    serde_json::from_value(args.clone()).map_err(|error| format!("invalid arguments: {error}"))
}

fn check(args: &CliRunArgs) -> Result<(), String> {
    if args.command.is_empty() {
        return Err("invalid arguments: `command` must name a program".to_string());
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

fn execute(args: &CliRunArgs) -> Result<Value, StepError> {
    let mut command = Command::new(&args.command);
    command
        .args(&args.args)
        .envs(&args.env)
        .stdin(if args.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = &args.cwd {
        command.current_dir(dir);
    }

    let mut child = command
        .spawn()
        .map_err(|error| spawn_error(&args.command, &error))?;

    let stdout = child.stdout.take().map(StreamReader::spawn);
    let stderr = child.stderr.take().map(StreamReader::spawn);
    if let (Some(input), Some(mut pipe)) = (&args.stdin, child.stdin.take()) {
        // The write must not block this thread: a program that never reads its
        // input would otherwise keep `da` waiting before it even starts the
        // timeout. The thread ends when the pipe closes.
        let input = input.clone().into_bytes();
        thread::spawn(move || {
            let _ = pipe.write_all(&input);
        });
    }

    let waited = match child.wait_timeout(Duration::from_millis(args.timeout_ms)) {
        Ok(waited) => waited,
        Err(error) => {
            // The wait itself failed, so the program is still running; it must
            // not outlive the step.
            let _ = child.kill();
            let _ = child.wait();
            return Err(StepError::Environment(format!(
                "could not wait for `{}`: {error}",
                args.command
            )));
        }
    };
    let status = match waited {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            // Drain what the program wrote before it was killed: the message
            // is more useful with it, and the pipes must not stay full.
            let _ = stdout.map(StreamReader::finish);
            let _ = stderr.map(StreamReader::finish);
            return Err(StepError::Timeout(format!(
                "`{}` did not finish within {} ms and was killed; increase `timeout_ms` or check that the program does not wait for input",
                args.command, args.timeout_ms
            )));
        }
    };

    let stdout = stdout.map(StreamReader::finish).unwrap_or_default();
    let stderr = stderr.map(StreamReader::finish).unwrap_or_default();
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();

    let mut result = Map::new();
    result.insert(
        "exit_code".to_string(),
        match status.code() {
            Some(code) => json!(code),
            // A program killed by a signal has no exit code.
            None => Value::Null,
        },
    );
    // `json` is only present when standard output is JSON, so that a case can
    // assert on `$.json` with `exists` instead of comparing parsed text.
    if let Ok(parsed) = serde_json::from_str::<Value>(stdout.trim()) {
        result.insert("json".to_string(), parsed);
    }
    result.insert("stdout".to_string(), Value::String(stdout));
    result.insert("stderr".to_string(), Value::String(stderr));
    Ok(Value::Object(result))
}

fn spawn_error(command: &str, error: &std::io::Error) -> StepError {
    let hint = match error.kind() {
        ErrorKind::NotFound => {
            "check that the program exists and is on `PATH`, or pass an absolute path"
        }
        ErrorKind::PermissionDenied => "check that the program is executable",
        _ => "check `command`, `cwd` and `env`",
    };
    StepError::Environment(format!("could not start `{command}`: {error}; {hint}"))
}

/// Reads one pipe on its own thread, so a program that fills a pipe cannot
/// block on it while `da` is still waiting for the program to exit.
struct StreamReader {
    buffer: Arc<Mutex<Vec<u8>>>,
    done: Receiver<()>,
    handle: Option<JoinHandle<()>>,
}

impl StreamReader {
    fn spawn<R: Read + Send + 'static>(mut pipe: R) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&buffer);
        let (sender, done) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut chunk = [0_u8; READ_CHUNK];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        if let Ok(mut buffer) = shared.lock() {
                            buffer.extend_from_slice(&chunk[..read]);
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = sender.send(());
        });
        Self {
            buffer,
            done,
            handle: Some(handle),
        }
    }

    /// Waits for the pipe to close, then returns everything that was read.
    fn finish(mut self) -> Vec<u8> {
        let closed = self.done.recv_timeout(DRAIN_TIMEOUT).is_ok();
        if closed && let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // A reader that is still blocked stays detached with its pipe. It owns
        // nothing `da` needs, and the process ends without it.
        match self.buffer.lock() {
            Ok(buffer) => buffer.clone(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_no_arguments_no_input_and_the_documented_timeout() {
        let args = parse(&json!({"command": "da"})).expect("the arguments must parse");

        assert!(args.args.is_empty());
        assert!(args.cwd.is_none());
        assert!(args.env.is_empty());
        assert!(args.stdin.is_none());
        assert_eq!(args.timeout_ms, 30_000);
    }

    #[test]
    fn rejects_an_unknown_argument() {
        let error = validate(&json!({"command": "da", "argv": ["--version"]}))
            .expect_err("the arguments must be refused");

        assert!(error.contains("argv"), "got: {error}");
    }

    #[test]
    fn rejects_an_empty_command() {
        let error = validate(&json!({"command": ""})).expect_err("the arguments must be refused");

        assert!(error.contains("command"), "got: {error}");
    }

    #[test]
    fn rejects_a_zero_timeout() {
        let error = validate(&json!({"command": "da", "timeout_ms": 0}))
            .expect_err("the arguments must be refused");

        assert!(error.contains("timeout_ms"), "got: {error}");
    }

    #[test]
    fn a_program_that_does_not_exist_is_an_environment_error_naming_it() {
        let error = run(
            &json!({"command": "da-no-such-program-for-a-test"}),
            &mut Sessions::new(),
        )
        .expect_err("the step must fail");

        let StepError::Environment(message) = error else {
            panic!("expected an environment error, got: {error:?}");
        };
        assert!(
            message.contains("da-no-such-program-for-a-test"),
            "got: {message}"
        );
    }
}
