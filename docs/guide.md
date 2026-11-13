# test-anything guide

The complete reference for `ta`: installation options, building from source, the case file format, the built-in steps, failure bundles, output and exit codes. For a quick introduction, see the [README](../README.md).

## Contents

- [Installation](#installation)
- [Build](#build)
- [Test](#test)
  - [Self-test suite](#self-test-suite)
- [Usage](#usage)
- [Case files](#case-files)
  - [Variables](#variables)
  - [Assertions](#assertions)
  - [`http.request`](#httprequest)
  - [`cli.run`](#clirun)
- [Failure bundles](#failure-bundles)
- [Output](#output)
- [Exit codes](#exit-codes)

## Installation

Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/LnYo-Cly/test-anything/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/LnYo-Cly/test-anything/main/install.ps1 | iex
```

The scripts pick the build for your system and CPU (x86_64 or Arm64), download the latest release, check it against `SHA256SUMS`, and install `ta` without administrator rights: to `~/.local/bin` on Linux and macOS, and to `%LOCALAPPDATA%\Programs\ta` on Windows, which is added to your user `PATH`. On Linux and macOS the script prints the line to add when the directory is not on `PATH` yet.

If `raw.githubusercontent.com` is slow or blocked on your network, fetch the same scripts from jsDelivr:

```sh
curl -fsSL https://cdn.jsdelivr.net/gh/LnYo-Cly/test-anything@main/install.sh | sh
```

```powershell
irm https://cdn.jsdelivr.net/gh/LnYo-Cly/test-anything@main/install.ps1 | iex
```

Options, set as environment variables:

| Variable | Meaning |
| --- | --- |
| `TA_VERSION` | Release to install, for example `v0.2.0`. Default: the latest. |
| `TA_INSTALL_DIR` | Directory that receives `ta`. |
| `TA_DOWNLOAD_URL` | Base URL of a mirror or proxy that serves the release files as `<base>/<tag>/<file>`. The archive and `SHA256SUMS` both come from it, so use one you trust. Finding the latest release still contacts `github.com` unless `TA_VERSION` is set. |
| `TA_NO_MODIFY_PATH` | Windows only: leave the user `PATH` unchanged. |

You can also download an archive from [GitHub Releases](https://github.com/LnYo-Cly/test-anything/releases) yourself, extract it, and put `ta` on your `PATH`. Each release includes a `SHA256SUMS` file.

## Build

Requires Rust 1.89 or newer (edition 2024).

```sh
cargo build
```

The binary is written to `target/debug/ta` (`target\debug\ta.exe` on Windows).
The `http` feature provides the `http.request` step and the `cli` feature
provides the `cli.run` step; both are enabled by default. Build a subset with
`--no-default-features --features cli`, or no driver at all with
`--no-default-features`.

## Test

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test
```

The integration tests in `tests/cli.rs` run the compiled binary against a local
HTTP server that each test starts itself. They never reach the network.

### Self-test suite

`tests/self/*.yaml` are cases that test `ta` with `ta`: every step is a
`cli.run` that starts the binary under test. `tests/self_suite.rs` runs them with
the compiled binary, so the existing `cargo test` covers them on Linux, macOS and
Windows without a separate CI job:

```sh
cargo test --test self_suite
ta run tests/self --var ta=target/debug/ta    # the same suite, by hand
```

`--var ta=<path>` tells every case which binary to start. The cases that the
suite calls are fixtures in `tests/self/fixtures/`; they use the `.case`
extension so that the recursive directory scan of `tests/self` does not pick
them up as tests. Run one explicitly, `ta run tests/self/fixtures/passing.case`,
to see what it does. The fixtures use `ta` itself as the program under test, so
the suite needs neither the network nor a system command such as `echo` or
`sleep`. One case runs a case that is meant to fail, so a manual run writes a
failure bundle under `.ta/failures`, which git ignores; add
`--var bundles=<directory>` to put it somewhere else.

## Usage

```sh
ta run cases/login.yaml
ta run cases/
ta run cases/ --var token=abc --var user=alice
ta run cases/ --bundle-dir /tmp/ta-failures
ta run cases/ --no-bundle
ta --output json run cases/
ta lint cases/        # not implemented yet
ta schema             # not implemented yet
```

`ta run` accepts files and directories. A directory is searched recursively for
`.yaml` and `.yml` files, which are run in path order. Every case file and every
step is validated before the first case runs: if anything is invalid, no case
runs and the exit code is 3. A step that fails an assertion stops its own case;
the remaining steps of that case are reported as skipped and the other cases
still run.

`--var name=value` defines a variable for every case of the run and may be
repeated. It overrides a variable of the same name in the case's `vars:`. A
`--var` without `=` or with an empty name is a usage error (exit 2).

`--bundle-dir <directory>` changes where failure bundles are written (see
[Failure bundles](#failure-bundles)); `--no-bundle` turns them off.

## Case files

A case is a YAML file with a name, optional variables and a list of steps. Each
step is a mapping with exactly one step name, optionally followed by `expect`
and `save`:

```yaml
name: Login returns a token
vars:
  port: "8080"
steps:
  - http.request:
      method: POST
      url: http://127.0.0.1:${port}/login
      json: { user: alice, password: secret }
    expect:
      - path: $.status
        equals: 200
      - path: $.body.token
        exists: true
    save:
      token: $.body.token
  - http.request:
      url: http://127.0.0.1:${port}/me
      headers: { Authorization: "Bearer ${token}" }
    expect:
      - path: $.body.name
        equals: alice
```

Unknown fields in a case file, unregistered step names, step arguments that do
not match the step's schema and undefined variables are all case errors.

### Variables

`vars:` defines variables for the whole case. `save:` copies a value out of a
step result into a variable. A later step argument can use any defined variable
as `${name}`. Substitution is textual and only happens inside strings, so a
variable may not change the type of a value. Referencing a variable that is
neither in `vars:` nor saved by an earlier step is a case error. `--var
name=value` on the command line defines a variable for every case and overrides
the same name in `vars:`.

### Assertions

`expect:` is a list of assertions evaluated against the step's JSON result. Each
entry has a `path` — a [JSONPath](https://www.rfc-editor.org/rfc/rfc9535.html)
expression such as `$.body.items[0].id` — and exactly one comparison:

| Comparison | Meaning |
| --- | --- |
| `equals` | The selected value equals the expected JSON value. |
| `not_equals` | The selected value differs from the expected JSON value. |
| `contains` | A string contains a substring, or an array contains a value. |
| `exists` | `true` requires a value, `false` requires none. |

Except for `exists`, the path must select exactly one value; a path that matches
nothing fails the assertion and is reported as such instead of being compared
with `null`. Member names that are not plain identifiers need bracket notation,
for example `$.body.headers['content-type']`.

### `http.request`

| Argument | Meaning |
| --- | --- |
| `method` | HTTP method. Defaults to `GET`. |
| `url` | Absolute URL including the scheme. Required. |
| `headers` | Request headers. |
| `query` | Query parameters appended to the URL, percent-encoded. |
| `json` | JSON request body; `content-type: application/json` is added unless a header sets it. |
| `body` | Raw request body. Mutually exclusive with `json`. |
| `timeout_ms` | Timeout for the whole request. Defaults to `30000`. |

The result is an object with `status` (a number), `headers` (lower-cased names)
and `body`. A body that is valid JSON becomes a JSON value; anything else stays a
string. A non-2xx status is not an error: it is part of the result and is judged
by assertions. A refused connection or an unresolvable host is an environment
error; exceeding `timeout_ms` is a timeout.

### `cli.run`

Runs a program without a shell and returns how it ended and what it wrote.

| Argument | Meaning |
| --- | --- |
| `command` | Program name or path. Required. A relative path is resolved against the working directory of `ta`. |
| `args` | List of arguments, one per entry. Nothing is quoted, expanded or split. |
| `cwd` | Working directory of the program. Defaults to the working directory of `ta`. |
| `env` | Variables added to the inherited environment, replacing names that are already set. |
| `stdin` | Text written to the program's standard input. Without it the input is empty. |
| `timeout_ms` | Timeout for the whole run. Defaults to `30000`. |

The result is an object with `exit_code`, `stdout` and `stderr`, plus `json` when
standard output is JSON:

```yaml
steps:
  - cli.run:
      command: git
      args: ["rev-parse", "--short", "HEAD"]
    expect:
      - path: $.exit_code
        equals: 0
      - path: $.stderr
        equals: ""
      - path: $.json
        exists: false
```

A non-zero exit code is not an error: it is part of the result and is judged by
assertions. `exit_code` is `null` when the program was killed by a signal.
Standard output and standard error are decoded as UTF-8, lossily. `json` is only
present when standard output without surrounding whitespace parses as JSON, so a
case can test it with `exists`.

The arguments are passed to `std::process::Command` directly, so shell syntax
such as quotes, `|` or `&&` does not apply. Run a shell explicitly when a case
needs one, for example `command: sh, args: ["-c", "..."]`. A relative `command`
and `cwd` are resolved against the working directory of the `ta` process, not
against the case file.

A program that cannot be started is an environment error (exit 4) whose message
names it and says what to check. When `timeout_ms` expires the program is killed
and the step is a timeout (exit 5). Both pipes are read on their own threads, so
a program that writes more than a pipe buffer cannot deadlock the run; after the
program ends `ta` waits at most one second for the pipes to close, so a process
the program left behind cannot hang it.

## Failure bundles

Terminal output is truncated and gone after the run, so every case that does not
pass also leaves a directory that an agent can read afterwards:

```text
.ta/failures/
└── 1763000000123-4242/                run id: Unix milliseconds and process id
    ├── 0-wrong-exit-code/
    │   ├── case.yaml                  copy of the case file
    │   ├── report.json                the case as `--output json` shows it
    │   └── meta.json                  written last
    └── 1-login-returns-a-token/
        └── (the same three files)
```

`meta.json` holds at least these fields:

| Field | Meaning |
| --- | --- |
| `schema_version` | Version of the layout, currently `1`. |
| `ta_version` | Version of `ta` that wrote the bundle. |
| `case_file` | Case file as given on the command line. |
| `case_name` | Name of the case. |
| `status` | `failed` or `error`. |
| `created_at` | Unix milliseconds. |

A case directory is staged under `.tmp-…` in the same parent, renamed into
place, and only then does `meta.json` appear. **A directory without `meta.json`
is incomplete and must be ignored**, including one left by an interrupted run.

The default directory is `.ta/failures` relative to the working directory of
`ta`. `--bundle-dir <directory>` replaces it and `--no-bundle` turns bundles off.
A run where every case passes creates nothing. When a bundle cannot be written,
the exit code of the run does not change: the problem is reported on standard
error and in the `bundle_errors` list of `--output json`.

> A bundle contains the arguments and results of every step, which may include
> credentials. Keep the bundle directory out of version control; `.ta/` is in
> this repository's `.gitignore`.

## Output

`--output text` (the default) writes one line per case to standard output, lists
each failed assertion with its path, expected value and actual value, and ends
with a summary line:

```text
FAIL cases/login.yaml [Login returns a token] 6 ms
  http.request: 1 of 2 assertions failed
    $.status equals 201 (actual 200)
  bundle: .ta/failures/1763000000123-4242/0-login-returns-a-token
Summary: 1 case (0 passed, 1 failed, 0 errors), 2 steps (1 passed, 1 failed, 0 errors, 0 skipped), 7 ms
```

`--output json` writes exactly one JSON object to standard output:

```json
{
  "ok": false,
  "summary": {
    "cases": 1,
    "cases_passed": 0,
    "cases_failed": 1,
    "cases_errored": 0,
    "steps": 2,
    "steps_passed": 1,
    "steps_failed": 1,
    "steps_errored": 0,
    "steps_skipped": 0,
    "duration_ms": 7
  },
  "cases": [
    {
      "file": "cases/login.yaml",
      "name": "Login returns a token",
      "status": "failed",
      "duration_ms": 6,
      "bundle": ".ta/failures/1763000000123-4242/0-login-returns-a-token",
      "error": { "kind": "assertion", "message": "1 of 2 assertions failed" },
      "steps": [
        {
          "name": "http.request",
          "status": "failed",
          "duration_ms": 6,
          "result": { "status": 200, "headers": { "content-type": "application/json" }, "body": { "token": "t-123" } },
          "error": { "kind": "assertion", "message": "1 of 2 assertions failed" },
          "assertions": [
            { "path": "$.status", "comparison": "equals", "expected": 201, "actual": 200, "passed": false, "message": null },
            { "path": "$.body.token", "comparison": "exists", "expected": true, "actual": "t-123", "passed": true, "message": null }
          ]
        }
      ]
    }
  ],
  "errors": [],
  "bundle_errors": []
}
```

`bundle` is the directory of the case's failure bundle, or `null` when the case
passed or bundles are off. `bundle_errors` lists the bundles that could not be
written; they never change the exit code.

When no case can run, `cases` is empty and `errors` lists one entry per invalid
file, with `file`, `message` and `kind`. Diagnostics never go to standard output.

## Exit codes

| Code | Meaning |
| ---: | --- |
| 0 | Every case ran and passed |
| 1 | At least one case failed an assertion or could not finish a step |
| 2 | Command-line usage error |
| 3 | Invalid case file, invalid step arguments, unknown step or undefined variable |
| 4 | Environment error (for example an unreachable target) |
| 5 | Timeout |
| 9 | Feature not implemented |

When several kinds of problem occur in one run, the most fundamental one wins:
3, then 4, then 5, then 1.
