//! Runs the self-test suite: `ta` testing `ta`.
//!
//! `tests/self/*.yaml` are ordinary case files whose steps start the built
//! binary. This test is the only place they run from, so `cargo test` covers
//! them on every platform without a separate CI job.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use support::TempDir;

#[test]
fn the_self_suite_passes() {
    let ta = env!("CARGO_BIN_EXE_ta");
    let work = TempDir::new("self-suite");
    let bundles = work.path().join("failures");

    let output = Command::new(ta)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "run",
            "tests/self",
            "--var",
            &format!("ta={ta}"),
            "--var",
            &format!("bundles={}", bundles.display()),
        ])
        .output()
        .expect("failed to run the ta binary");

    assert!(
        output.status.success(),
        "the self suite must pass, got {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // A case step cannot list a directory, so the bundle contract is checked
    // here, where the file system is available.
    let metas = files_named(&bundles, "meta.json");
    assert_eq!(
        metas.len(),
        1,
        "the failing fixture must leave exactly one complete bundle, got: {metas:?}"
    );
    let meta: Value = serde_json::from_str(
        &fs::read_to_string(&metas[0]).expect("meta.json must be readable as text"),
    )
    .expect("meta.json must be JSON");
    assert_eq!(meta["schema_version"], 1);
    assert_eq!(meta["ta_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(meta["case_name"], "Failing fixture");
    assert_eq!(meta["status"], "failed");
    assert!(
        meta["case_file"]
            .as_str()
            .is_some_and(|file| file.ends_with("failing.case")),
        "got: {meta}"
    );
    assert!(
        meta["created_at"].as_u64().is_some_and(|millis| millis > 0),
        "got: {meta}"
    );
}

/// Every file called `name` anywhere below `dir`.
fn files_named(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_named(&path, name));
        } else if path.file_name().is_some_and(|file| file == name) {
            found.push(path);
        }
    }
    found
}
