//! How a path is shown to people and machines.
//!
//! Paths are built and used as [`std::path::Path`], which keeps whatever
//! separators the caller typed. Windows accepts both, so joining a discovered
//! file to a `/`-separated directory yields `cases\login.yaml` next to the
//! `cases/` the user wrote. Every user-facing and JSON path goes through
//! [`display`], so the text is copyable and the JSON is the same on every
//! platform. File system calls keep using the `Path` itself.

use std::path::Path;

/// Returns `path` with `/` as the only separator.
///
/// Forward slashes are accepted by every platform `ta` runs on, and they need
/// no escaping in JSON, in a shell argument or in a Markdown file.
pub fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_every_backslash_with_a_forward_slash() {
        let path = Path::new(r"G:\tmp\cases\login.yaml");

        assert_eq!(display(path), "G:/tmp/cases/login.yaml");
    }

    #[test]
    fn keeps_a_path_that_already_uses_forward_slashes() {
        assert_eq!(display(Path::new("cases/login.yaml")), "cases/login.yaml");
        assert_eq!(display(Path::new("")), "");
    }
}
