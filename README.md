# test-anything

An agent-friendly, cross-layer testing tool written in Rust.

`ta` is a single binary that runs declarative test cases against HTTP APIs and
command-line targets, and reports results in a form that both people and
coding agents can read. Status: early development. The command-line skeleton
exists, but no test execution is implemented yet.

## Build

Requires Rust 1.85 or newer (edition 2024).

```sh
cargo build
```

The binary is written to `target/debug/ta` (`target\debug\ta.exe` on Windows).

## Test

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The integration tests in `tests/cli.rs` run the compiled binary.

## Usage

```sh
ta --help
ta run some.yaml
ta lint some.yaml
ta schema
ta --output json run some.yaml
```

Every subcommand currently reports "not implemented" on standard error and
exits with code 9.

`--output text` (the default) keeps standard output empty and prints
human-readable messages on standard error. `--output json` writes exactly one
JSON object to standard output:

```json
{"ok":false,"error":{"code":"not_implemented","message":"ta run is not implemented yet"}}
```

## Exit codes

| Code | Meaning |
| ---: | --- |
| 0 | Success |
| 1 | Test case failed (an assertion did not pass) |
| 2 | Command-line usage error |
| 3 | Invalid test case file (validation error) |
| 4 | Environment error (for example an unreachable target) |
| 5 | Timeout |
| 9 | Feature not implemented |

Codes 0, 2, and 9 are the only ones currently produced; the rest are reserved
for the features that follow.
