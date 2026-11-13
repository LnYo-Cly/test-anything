//! The `log.file` observer.
//!
//! It reports the text a log file gained while one step ran: [`Observer::start`]
//! records the current length and [`Observer::collect`] reads from there to the
//! end. Nothing is written, so watching a log never disturbs the program that
//! writes it.

use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use schemars::{JsonSchema, schema_for};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::core::observer::{Observer, ObserverDefinition, ObserverError};
use crate::core::paths;
use crate::core::registry::Registry;

/// Name of the observer this module registers.
pub const OBSERVER_NAME: &str = "log.file";

/// Most bytes one collection reads.
///
/// A log that grew by more than this is cut off and marked `limited`, so one
/// noisy step cannot fill a failure bundle or the terminal.
pub const MAX_BYTES: u64 = 1024 * 1024;

/// Registers the observers of this module.
pub fn register(registry: &mut Registry) -> Result<(), String> {
    let args_schema = serde_json::to_value(schema_for!(LogFileArgs))
        .map_err(|error| format!("could not build the schema of `{OBSERVER_NAME}`: {error}"))?;
    registry.register_observer(ObserverDefinition {
        name: OBSERVER_NAME,
        description: "Reports the lines a log file gained while the step ran.",
        args_schema,
        validate,
        create,
    })
}

/// Arguments of `log.file`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogFileArgs {
    /// File to read, as UTF-8. Invalid bytes become replacement characters.
    // The schema must enforce the same bound as `check`, so that a value the
    // schema accepts is not rejected later for this argument.
    #[schemars(extend("minLength" = 1))]
    path: String,
}

fn validate(args: &Value) -> Result<(), String> {
    check(&parse(args)?)
}

fn parse(args: &Value) -> Result<LogFileArgs, String> {
    serde_json::from_value(args.clone()).map_err(|error| format!("invalid arguments: {error}"))
}

fn check(args: &LogFileArgs) -> Result<(), String> {
    if args.path.is_empty() {
        return Err("invalid arguments: `path` must name the file to read".to_string());
    }
    Ok(())
}

fn create(args: &Value) -> Result<Box<dyn Observer>, ObserverError> {
    let args = parse(args).map_err(ObserverError::InvalidArguments)?;
    check(&args).map_err(ObserverError::InvalidArguments)?;
    Ok(Box::new(LogFile {
        path: PathBuf::from(args.path),
        position: 0,
    }))
}

/// One case's view of one log file.
struct LogFile {
    path: PathBuf,
    /// Length of the file when the current step started.
    position: u64,
}

impl Observer for LogFile {
    fn start(&mut self) -> Result<(), ObserverError> {
        // A file that does not exist yet is a file with no content: the program
        // under test may create it during this step.
        self.position = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == ErrorKind::NotFound => 0,
            Err(error) => return Err(environment(&self.path, &error)),
        };
        Ok(())
    }

    fn collect(&mut self) -> Result<Value, ObserverError> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            // A file that was never created, or was removed during the step,
            // has added nothing.
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(observation("", false, false));
            }
            Err(error) => return Err(environment(&self.path, &error)),
        };
        let length = file
            .metadata()
            .map_err(|error| environment(&self.path, &error))?
            .len();

        // A file that is shorter than it was when the step started was
        // truncated or rotated, so the content that matters starts at the
        // beginning.
        let truncated = length < self.position;
        let start = if truncated { 0 } else { self.position };

        file.seek(SeekFrom::Start(start))
            .map_err(|error| environment(&self.path, &error))?;
        // The increment is cut at the limit; the rest is what `limited` stands
        // for. The next step starts from the length the file has by then, so
        // nothing that arrived after this step is reported as if it had
        // belonged to this one.
        let limited = length - start > MAX_BYTES;
        let mut buffer = Vec::new();
        file.take(length - start)
            .take(MAX_BYTES)
            .read_to_end(&mut buffer)
            .map_err(|error| environment(&self.path, &error))?;
        self.position = length;
        let text = String::from_utf8_lossy(&buffer);

        Ok(observation(&text, truncated, limited))
    }
}

/// The JSON an observation of a log file reports.
fn observation(text: &str, truncated: bool, limited: bool) -> Value {
    json!({
        "lines": lines(text),
        "text": text,
        "truncated": truncated,
        "limited": limited,
    })
}

/// Splits the collected text into lines the way an editor would.
///
/// A final newline does not start another line, a carriage return before a line
/// break is dropped so a CRLF log reads the same as an LF one, and a last line
/// without a newline still counts as a line.
fn lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

fn environment(path: &Path, error: &std::io::Error) -> ObserverError {
    ObserverError::Environment(format!(
        "could not read `{}`: {error}",
        paths::display(path)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ta-driver-log-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the fixture directory must be created");
        dir
    }

    fn observer(path: &Path) -> Box<dyn Observer> {
        let args = json!({ "path": path.to_string_lossy() });
        create(&args).expect("the observer must be created")
    }

    #[test]
    fn reports_only_what_was_added_after_start() {
        let dir = fixture("increment");
        let log = dir.join("app.log");
        std::fs::write(&log, "old line\n").expect("the log must be written");
        let mut observer = observer(&log);
        observer.start().expect("start must succeed");
        std::fs::write(&log, "old line\nnew line\n").expect("the log must grow");

        let value = observer.collect().expect("collect must succeed");

        assert_eq!(value["lines"], json!(["new line"]));
        assert_eq!(value["text"], "new line\n");
        assert_eq!(value["truncated"], json!(false));
        assert_eq!(value["limited"], json!(false));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_empty_and_stays_missing() {
        let dir = fixture("missing");
        let log = dir.join("app.log");
        let mut observer = observer(&log);

        observer.start().expect("start must succeed");
        let value = observer.collect().expect("collect must succeed");

        assert_eq!(value["lines"], json!([]));
        assert_eq!(value["text"], "");
        assert!(!log.exists(), "a collector must not create the file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_shorter_than_before_is_read_from_the_start_and_marked() {
        let dir = fixture("truncated");
        let log = dir.join("app.log");
        std::fs::write(&log, "much longer original content\n").expect("the log must be written");
        let mut observer = observer(&log);
        observer.start().expect("start must succeed");
        std::fs::write(&log, "restarted\n").expect("the log must be replaced");

        let value = observer.collect().expect("collect must succeed");

        assert_eq!(value["lines"], json!(["restarted"]));
        assert_eq!(value["truncated"], json!(true));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_read_stops_at_the_limit_and_marks_the_result() {
        let dir = fixture("limited");
        let log = dir.join("app.log");
        let mut observer = observer(&log);
        observer.start().expect("start must succeed");
        let total = usize::try_from(MAX_BYTES).expect("the limit fits a usize") + 10;
        std::fs::write(&log, "y".repeat(total)).expect("the log must be written");

        let first = observer.collect().expect("collect must succeed");
        assert_eq!(first["limited"], json!(true));
        assert_eq!(
            first["text"].as_str().map(str::len),
            Some(usize::try_from(MAX_BYTES).expect("the limit fits a usize"))
        );

        // The next step starts from the length the file has by then, so the
        // cut-off remainder is not reported as part of it.
        observer.start().expect("start must succeed");
        let second = observer.collect().expect("collect must succeed");
        assert_eq!(second["limited"], json!(false));
        assert_eq!(second["text"], "");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_is_an_environment_error_that_names_it() {
        let dir = fixture("directory");
        let mut observer = observer(&dir);

        observer.start().expect("a directory has a length");
        let error = observer
            .collect()
            .expect_err("a directory cannot be read as a file");

        let ObserverError::Environment(message) = error else {
            panic!("expected an environment error, got: {error:?}");
        };
        assert!(
            message.contains(&paths::display(&dir)),
            "the message must name the path, got: {message}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_drop_a_carriage_return_and_keep_a_last_line_without_a_newline() {
        assert_eq!(lines(""), Vec::<String>::new());
        assert_eq!(lines("\n"), vec![""]);
        assert_eq!(lines("one"), vec!["one"]);
        assert_eq!(lines("one\n"), vec!["one"]);
        assert_eq!(lines("one\r\n"), vec!["one"]);
        assert_eq!(lines("one\r\ntwo"), vec!["one", "two"]);
        assert_eq!(lines("one\n\ntwo\n"), vec!["one", "", "two"]);
    }

    #[test]
    fn empty_arguments_are_refused() {
        let error = validate(&json!({ "path": "" })).expect_err("an empty path must be refused");
        assert!(error.contains("path"), "got: {error}");

        let error = validate(&json!({ "path": "app.log", "location": "x" }))
            .expect_err("an unknown argument must be refused");
        assert!(error.contains("location"), "got: {error}");
    }
}
