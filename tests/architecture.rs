//! Guards the layering rule from `CONTRIBUTING.md`: the engine is
//! domain-neutral and never reaches into a driver.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|error| {
        panic!("failed to list {}: {error}", dir.display());
    });
    for entry in entries {
        let entry = entry.expect("failed to read a source entry");
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn the_engine_does_not_reference_a_driver_or_a_concrete_domain() {
    let core = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("core");
    let mut files = Vec::new();
    rust_sources(&core, &mut files);
    assert!(
        !files.is_empty(),
        "expected sources under {}",
        core.display()
    );

    // Words that would mean the engine knows about a concrete test target.
    let forbidden = [
        "drivers", "http", "ureq", "schemars", "reqwest", "hyper", "tcp",
    ];

    for file in files {
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", file.display()));
        let lowered = source.to_lowercase();
        for word in forbidden {
            assert!(
                !lowered.contains(word),
                "{} mentions `{word}`; the engine must stay domain-neutral",
                file.display()
            );
        }
    }
}
