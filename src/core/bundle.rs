//! Failure bundles: the evidence a failed run leaves on disk.
//!
//! One run writes one directory per failed case. Each case directory is staged
//! under a temporary name in the same parent and renamed into place, and
//! `meta.json` is the last file written. A directory that contains `meta.json`
//! is therefore complete; a reader must ignore every other directory.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::core::output;
use crate::core::paths;
use crate::core::report::CaseReport;
use crate::core::secrets::Secrets;

/// Version of the bundle layout this build writes.
pub const SCHEMA_VERSION: u64 = 1;
/// Directory that holds failure bundles unless `--bundle-dir` is given.
pub const DEFAULT_DIRECTORY: &str = ".ta/failures";
/// Longest part of a directory name taken from a case name.
const MAX_NAME: usize = 40;

/// Writes the failure bundles of one run.
///
/// The run directory is created with the first bundle, so a run where every
/// case passes leaves the file system untouched.
#[derive(Debug)]
pub struct FailureBundles {
    root: PathBuf,
    run: Option<PathBuf>,
    sequence: u64,
}

impl FailureBundles {
    /// Creates a writer that puts its run directories under `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            run: None,
            sequence: 0,
        }
    }

    /// Writes the bundle of one failed case and returns its final directory.
    ///
    /// `position` is the index of the case in the run, which keeps the
    /// directories in run order and makes their names unique. `secrets` is
    /// applied to every file this function writes; the copy of the case file is
    /// left as written, because it holds `${env:NAME}` references and no values.
    pub fn write(
        &mut self,
        position: usize,
        case: &CaseReport,
        secrets: &Secrets,
    ) -> Result<PathBuf, String> {
        // Checked before anything is created, so a case file that vanished
        // leaves no empty directory behind.
        std::fs::metadata(&case.file).map_err(|error| {
            failure(
                case,
                format!("could not read `{}`: {error}", paths::display(&case.file)),
            )
        })?;

        let run = self
            .run_directory()
            .map_err(|detail| failure(case, detail))?;
        let final_dir = run.join(format!("{position}-{}", safe_name(&case.name)));
        let staging = run.join(format!(".tmp-{}-{}", std::process::id(), self.sequence));
        self.sequence += 1;

        // An earlier attempt that failed may have left its staging directory.
        let _ = std::fs::remove_dir_all(&staging);
        if let Err(error) = std::fs::create_dir(&staging) {
            return Err(failure(
                case,
                format!("could not create `{}`: {error}", paths::display(&staging)),
            ));
        }

        if let Err(detail) = stage(&staging, case, &final_dir, secrets) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(failure(case, detail));
        }
        if let Err(error) = std::fs::rename(&staging, &final_dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(failure(
                case,
                format!(
                    "could not move `{}` to `{}`: {error}",
                    paths::display(&staging),
                    paths::display(&final_dir)
                ),
            ));
        }
        // The metadata comes last: it is what tells a reader that the rest of
        // the directory is complete.
        if let Err(detail) = write_meta(&final_dir, case, secrets) {
            let _ = std::fs::remove_dir_all(&final_dir);
            return Err(failure(case, detail));
        }
        Ok(final_dir)
    }

    fn run_directory(&mut self) -> Result<PathBuf, String> {
        if let Some(dir) = &self.run {
            return Ok(dir.clone());
        }
        let dir = create_run_directory(&self.root)?;
        self.run = Some(dir.clone());
        Ok(dir)
    }
}

fn stage(
    staging: &Path,
    case: &CaseReport,
    final_dir: &Path,
    secrets: &Secrets,
) -> Result<(), String> {
    let case_copy = staging.join("case.yaml");
    std::fs::copy(&case.file, &case_copy).map_err(|error| {
        format!(
            "could not copy `{}` to `{}`: {error}",
            paths::display(&case.file),
            paths::display(&case_copy)
        )
    })?;

    // The report is the same object `--output json` shows for this case, so a
    // reader of the bundle and a reader of the output see one shape. Both are
    // redacted the same way, because a bundle is read after the terminal is
    // gone.
    let mut reported = case.clone();
    reported.bundle = Some(final_dir.to_path_buf());
    write_json(
        &staging.join("report.json"),
        &secrets.redact_json(&output::case_json(&reported)),
    )
}

fn write_meta(dir: &Path, case: &CaseReport, secrets: &Secrets) -> Result<(), String> {
    write_json(
        &dir.join("meta.json"),
        &secrets.redact_json(&json!({
            "schema_version": SCHEMA_VERSION,
            "ta_version": env!("CARGO_PKG_VERSION"),
            "case_file": paths::display(&case.file),
            "case_name": case.name,
            "project": case.project,
            "suite": case.suite,
            "layer": case.layer,
            "tags": case.tags,
            "status": case.status.as_str(),
            "created_at": now_millis(),
        })),
    )
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("could not render `{}`: {error}", paths::display(path)))?;
    std::fs::write(path, text + "\n")
        .map_err(|error| format!("could not write `{}`: {error}", paths::display(path)))
}

/// Creates `<root>/<millis>-<pid>` and returns it.
///
/// A name that is already taken gets a counter suffix, so a second run in the
/// same millisecond never writes into the directories of the first one.
fn create_run_directory(root: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root)
        .map_err(|error| format!("could not create `{}`: {error}", paths::display(root)))?;
    let base = format!("{}-{}", now_millis(), std::process::id());
    for attempt in 1..=100 {
        let candidate = if attempt == 1 {
            root.join(&base)
        } else {
            root.join(format!("{base}-{attempt}"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "could not create `{}`: {error}",
                    paths::display(&candidate)
                ));
            }
        }
    }
    Err(format!(
        "could not find a free run directory name under `{}`",
        paths::display(root)
    ))
}

/// Milliseconds since the Unix epoch, or 0 when the clock is set before it.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Turns a case name into a directory name that every platform accepts.
///
/// Only letters, digits and `_` are kept; every other character becomes a
/// single `-`. The case position prefixes the name, so a case called `con` or
/// `nul` cannot collide with a Windows device name.
fn safe_name(name: &str) -> String {
    let mut output = String::new();
    let mut used = 0;
    let mut after_separator = false;
    for character in name.chars() {
        if used >= MAX_NAME {
            break;
        }
        if character.is_alphanumeric() || character == '_' {
            output.push(character);
            used += 1;
            after_separator = false;
        } else if !after_separator && !output.is_empty() {
            output.push('-');
            used += 1;
            after_separator = true;
        }
    }
    let trimmed = output.trim_matches('-');
    if trimmed.is_empty() {
        "case".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The message a caller reports without changing the run's exit code.
fn failure(case: &CaseReport, detail: String) -> String {
    format!(
        "could not write the failure bundle for `{}`: {detail}; the run result is unchanged, and `--no-bundle` turns failure bundles off",
        case.name
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::core::report::{CaseStatus, StepReport, StepStatus};

    fn failing_case(file: PathBuf) -> CaseReport {
        CaseReport {
            file,
            name: "Wrong exit code".to_string(),
            project: Some("shop".to_string()),
            suite: Some("smoke".to_string()),
            layer: Some("cli".to_string()),
            tags: vec!["smoke".to_string(), "fast".to_string()],
            status: CaseStatus::Failed,
            duration: Duration::from_millis(3),
            steps: vec![StepReport {
                name: "cli.run".to_string(),
                status: StepStatus::Failed,
                duration: Duration::from_millis(2),
                result: Some(json!({"exit_code": 0})),
                assertions: Vec::new(),
                error: None,
            }],
            error: None,
            close_errors: Vec::new(),
            bundle: None,
        }
    }

    fn fixture(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ta-core-bundle-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the fixture directory must be created");
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("the directory must be readable")
            .map(|entry| {
                entry
                    .expect("the entry must be readable")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_the_case_the_report_and_the_metadata() {
        let dir = fixture("complete");
        let source = dir.join("case.yaml");
        std::fs::write(&source, "name: Wrong exit code\nsteps: []\n")
            .expect("the case file must be written");
        let images = dir.join("failures");
        let mut bundles = FailureBundles::new(images.clone());

        let bundle = bundles
            .write(0, &failing_case(source.clone()), &Secrets::new())
            .expect("the bundle must be written");

        assert!(bundle.starts_with(&images));
        assert_eq!(
            names(&bundle),
            vec!["case.yaml", "meta.json", "report.json"]
        );
        assert_eq!(
            std::fs::read(bundle.join("case.yaml")).expect("case.yaml must be readable"),
            std::fs::read(&source).expect("the case file must be readable")
        );

        let meta: Value = serde_json::from_str(
            &std::fs::read_to_string(bundle.join("meta.json")).expect("meta.json must be readable"),
        )
        .expect("meta.json must be JSON");
        assert_eq!(meta["schema_version"], json!(SCHEMA_VERSION));
        assert_eq!(meta["ta_version"], json!(env!("CARGO_PKG_VERSION")));
        assert_eq!(meta["case_name"], json!("Wrong exit code"));
        assert_eq!(meta["project"], json!("shop"));
        assert_eq!(meta["suite"], json!("smoke"));
        assert_eq!(meta["layer"], json!("cli"));
        assert_eq!(meta["tags"], json!(["smoke", "fast"]));
        assert_eq!(meta["status"], json!("failed"));
        assert!(
            meta["created_at"].as_u64().is_some_and(|millis| millis > 0),
            "got: {meta}"
        );

        let report: Value = serde_json::from_str(
            &std::fs::read_to_string(bundle.join("report.json"))
                .expect("report.json must be readable"),
        )
        .expect("report.json must be JSON");
        assert_eq!(report["status"], json!("failed"));
        assert_eq!(
            report["bundle"],
            json!(paths::display(&bundle)),
            "the bundled report must point at its own directory"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_secret_is_redacted_in_the_report_and_the_metadata_but_not_in_the_case_copy() {
        let dir = fixture("redacted");
        let source = dir.join("case.yaml");
        let case_text = "name: Wrong exit code\nsteps:\n  - cli.run:\n      args: [\"${env:TA_BUNDLE_TOKEN}\"]\n";
        std::fs::write(&source, case_text).expect("the case file must be written");
        let mut bundles = FailureBundles::new(dir.join("failures"));
        let mut case = failing_case(source);
        case.name = "Wrong exit code s3cr3t-value".to_string();
        case.steps[0].result = Some(json!({"token": "s3cr3t-value"}));
        let mut secrets = Secrets::new();
        secrets.record("s3cr3t-value");

        let bundle = bundles
            .write(0, &case, &secrets)
            .expect("the bundle must be written");

        for name in ["report.json", "meta.json"] {
            let text = std::fs::read_to_string(bundle.join(name))
                .unwrap_or_else(|error| panic!("{name} must be readable: {error}"));
            assert!(!text.contains("s3cr3t-value"), "{name} leaked: {text}");
            assert!(text.contains("***"), "{name} must show the marker: {text}");
        }
        assert_eq!(
            std::fs::read_to_string(bundle.join("case.yaml")).expect("case.yaml must be readable"),
            case_text,
            "the case copy holds references, not values, and stays as written"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_run_directory_holds_only_finished_bundles() {
        let dir = fixture("no-staging");
        let source = dir.join("case.yaml");
        std::fs::write(&source, "name: One\nsteps: []\n").expect("the case file must be written");
        let mut bundles = FailureBundles::new(dir.join("failures"));

        let first = bundles
            .write(0, &failing_case(source.clone()), &Secrets::new())
            .expect("the first bundle must be written");
        let second = bundles
            .write(1, &failing_case(source), &Secrets::new())
            .expect("the second bundle must be written");

        let run = first.parent().expect("a bundle has a run directory");
        assert_eq!(second.parent(), Some(run));
        assert_eq!(names(run), vec!["0-Wrong-exit-code", "1-Wrong-exit-code"]);
        assert!(
            names(&first).contains(&"meta.json".to_string()),
            "the last file must be the metadata"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_run_in_the_same_millisecond_gets_its_own_directory() {
        let dir = fixture("unique-run");
        let root = dir.join("failures");
        let first = create_run_directory(&root).expect("the first run directory must be created");
        let second = create_run_directory(&root).expect("the second run directory must be created");

        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        assert_eq!(names(&root).len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_case_file_that_cannot_be_read_leaves_no_directory_behind() {
        let dir = fixture("missing-source");
        let images = dir.join("failures");
        let mut bundles = FailureBundles::new(images.clone());

        let error = bundles
            .write(0, &failing_case(dir.join("missing.yaml")), &Secrets::new())
            .expect_err("the bundle must fail");

        assert!(error.contains("Wrong exit code"), "got: {error}");
        assert!(error.contains("--no-bundle"), "got: {error}");
        assert!(!images.exists(), "no run directory may be created");
        assert!(!dir.join("failures").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_root_that_is_a_file_is_reported_as_a_bundle_failure() {
        let dir = fixture("blocked-root");
        let source = dir.join("case.yaml");
        std::fs::write(&source, "name: Blocked\nsteps: []\n")
            .expect("the case file must be written");
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, "not a directory").expect("the blocking file must be written");
        let mut bundles = FailureBundles::new(blocked);

        let error = bundles
            .write(0, &failing_case(source), &Secrets::new())
            .expect_err("the bundle must fail");

        assert!(error.contains("could not create"), "got: {error}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_reduced_to_safe_characters() {
        assert_eq!(safe_name("Wrong exit code"), "Wrong-exit-code");
        assert_eq!(safe_name("a/b:c*d?"), "a-b-c-d");
        assert_eq!(safe_name("  spaced  "), "spaced");
        assert_eq!(safe_name("***"), "case");
        assert_eq!(safe_name(""), "case");
        assert_eq!(safe_name("登录失败"), "登录失败");
        assert!(safe_name(&"x".repeat(200)).chars().count() <= MAX_NAME);
    }
}
