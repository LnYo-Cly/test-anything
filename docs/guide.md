# test-anything guide

The complete reference for `ta`: installation options, building from source, the case file format, the built-in steps, failure bundles, output and exit codes. For a quick introduction, see the [README](../README.md).

## Contents

- [Installation](#installation)
- [Build](#build)
- [Test](#test)
  - [Self-test suite](#self-test-suite)
- [Usage](#usage)
  - [The service under test](#the-service-under-test)
  - [Classification and filtering](#classification-and-filtering)
  - [Linting](#linting)
  - [Secrets](#secrets)
  - [Schema](#schema)
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
ta run cases/ --vars-file envs/local.yaml --vars-file envs/ci.yaml
ta run cases/ --layer api --tag smoke
ta run cases/ --tag smoke --skip-tag slow
ta run cases/ --bundle-dir /tmp/ta-failures
ta run cases/ --no-bundle
ta --output json run cases/
ta lint cases/
ta lint cases/ --var token=abc
ta lint cases/ --vars-file envs/ci.yaml
ta --output json lint cases/
ta schema
ta --output json schema > ta-case.schema.json
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

`--vars-file <file>` reads variables from a YAML mapping and may be repeated.
The keys are variable names; the values are strings, numbers or booleans, the
same way `vars:` reads them, so `port: 8080` becomes the text `8080`. A file is
applied in the order it appears on the command line, and a later source
overrides an earlier one: a case's `vars:`, then each `--vars-file`, then
`--var`. A file that does not exist, is not a mapping, or holds a value that is
not a string, number or boolean is a usage error (exit 2) that names the file
and, where the parser knows it, the line and column:

```yaml
# envs/staging.yaml
base: https://staging.example.test
user: alice
retries: 3
```

```sh
ta run cases/ --vars-file envs/staging.yaml
```

Use it to run one set of cases against a local server, a staging deployment and
CI without editing the case files:

```sh
ta run cases/ --vars-file envs/local.yaml
ta run cases/ --vars-file envs/staging.yaml
ta run cases/ --vars-file envs/ci.yaml --var base=http://127.0.0.1:8080
```

`ta lint` accepts `--vars-file` and `--var` as well, and validates the cases
with the same values a run would use.

`--bundle-dir <directory>` changes where failure bundles are written (see
[Failure bundles](#failure-bundles)); `--no-bundle` turns them off.

### The service under test

A case talks to a service that is already running: `ta` sends the requests and
starts the programs a step names, but it never starts, restarts or stops the
service itself. Running a case against a server therefore takes four steps of
your own shell:

1. Start the service, in its own terminal or as a background process.
2. Wait until it accepts requests.
3. Run the cases.
4. Stop the service when the run is over. `ta` leaves it running.

Pass the address with `--var` instead of writing a port into the case file, so
the same cases run against a local server and a deployed environment:

```sh
ta run cases/ --var base=http://127.0.0.1:8080
```

A run that starts before the service is listening reports an environment error
(exit 4) whose message names the connection failure, and no assertion is
evaluated. That is how a wrong target is told apart from a wrong expectation: an
unreachable service is never a failed assertion.

### Classification and filtering

A case may describe where it belongs, so one repository can hold interface,
command-line, UI and future database cases side by side:

```yaml
name: Login returns a token
project: shop
suite: smoke
layer: api
tags: [smoke, fast]
steps:
  - http.request:
      url: http://127.0.0.1:8080/health
    expect:
      - path: $.status
        equals: 200
```

| Field | Type | Meaning |
| --- | --- | --- |
| `project` | string | Project the case belongs to. |
| `suite` | string | Suite the case belongs to, for example a smoke or regression suite. |
| `layer` | string | Layer the case exercises. Free-form: this project suggests `unit`, `api`, `cli`, `ui`, `integration`, `e2e` and `db`, and a game, chip or systems project uses its own values. |
| `tags` | list of strings | Cross-cutting labels. Duplicates are a case error. |

All four fields are optional and are never interpolated: write the value you
mean, and a `${name}` inside one stays literal. An empty value is a case error
(exit 3), as is a repeated tag.

`ta run` selects cases with `--project`, `--suite`, `--layer`, `--tag` and
excludes them with `--skip-tag`; each option may be repeated:

```sh
ta run cases/ --layer api
ta run cases/ --project shop --project billing
ta run cases/ --layer api --layer cli --tag smoke
ta run cases/ --tag smoke --skip-tag slow
```

- Values of one option are alternatives: `--layer api --layer cli` runs both
  layers.
- Different options must all match: `--layer api --tag smoke` runs cases that
  are in the api layer **and** carry the smoke tag.
- `--skip-tag` wins over every other option: a case with any skipped tag does
  not run.
- Matching is exact and case-sensitive.
- A case that does not declare a field is not selected by a filter on that
  field: with `--layer api`, a case without `layer` does not run.

Every discovered file is still parsed and validated, whether or not the filters
select it. A filter never lets an invalid case file pass: if a filtered-out file
is broken, the run exits 3 and no case runs.

When the filters select no case, `ta run` exits 2 and the message names the
options that were used and the `project`, `suite`, `layer` and tag values the
discovered cases actually have, so a typo is visible without running anything.
A run without filters is not affected by this rule.

Filtered-out cases do not appear in the result; the summary counts them as
`filtered` (text) or `summary.filtered` (JSON).

### Linting

`ta lint` takes the same files and directories as `ta run`, and the same
`--var name=value` and `--vars-file` options. It discovers, loads and validates
every case with the same preflight that `ta run` uses, but it does not execute a
step, write a failure bundle or emit run events. A case that lint accepts is a
case that `ta run` accepts before it starts; problems that only appear while a
step runs, such as an unreachable target, are outside lint's scope.

Unlike a run, lint reports every problem in every file instead of stopping at
the first one. Text output writes `file: message` for each problem and ends with
a summary. A YAML grammar error and every structural problem of a case file — a
step, an assertion, a `save` entry or a variable — name the line and column of
the node they belong to and keep the parser's source snippet, including its line
breaks and caret line, so the reported location can be read directly:

```text
cases/login.yaml: line 6 column 9: step 1: assertion 1: unknown field(s) equal; use one of equals, not_equals, contains, exists
 --> <input>:6:9
  |
4 |       command: ta
5 |     expect:
6 |       - path: $.exit_code
  |         ^ step 1: assertion 1: unknown field(s) equal; use one of equals, not_equals, contains, exists
7 |         equal: 0
  |
cases/logout.yaml: line 2 column 1: unknown field `naem`, expected one of name, vars, project, suite, layer, tags, steps
 --> <input>:2:1
  |
1 | name: Logout
2 | naem: typo
  | ^ unknown field `naem`, expected one of name, vars, project, suite, layer, tags, steps
3 | steps:
4 |   - cli.run:
  |
Checked 2 files: 2 errors.
```

Problems that need the registry, such as an undefined variable or a step
argument that does not match the step's schema, name the step instead, because
the parser cannot know them:

```text
cases/login.yaml: step 2: undefined variable `${token}`; define it under `vars:` or save it in an earlier step
```

`--output json` writes exactly one object:

```json
{
  "ok": false,
  "files": 2,
  "errors": [
    {
      "file": "cases/login.yaml",
      "message": "step 2: undefined variable `${token}`; define it under `vars:` or save it in an earlier step"
    }
  ],
  "cases": [
    {
      "file": "cases/smoke.yaml",
      "name": "Login returns a token",
      "project": "shop",
      "suite": "smoke",
      "layer": "api",
      "tags": ["smoke", "fast"]
    }
  ]
}
```

`cases` lists every case that parsed, with its classification, so an agent can
discover the available values without running anything. `project`, `suite` and
`layer` are `null` when the case does not declare them.

The exit code is 0 when every case is valid and 3 when anything is invalid,
including a path that cannot be read and a directory with no case files. Use
`ta lint tests/self --var ta=target/debug/ta` to check the self suite without
running it.

### Secrets

A value that `ta` read through `${env:NAME}` is treated as a secret. The rule
follows the source, not a declaration: nothing has to be marked, so a value
cannot be forgotten. Values from `vars:`, `--vars-file` and `--var` are not
secrets and stay readable, because they are already visible in a file or in the
command line.

Before anything is written, every occurrence of every secret is replaced with
`***`. That covers:

- text output on standard output and standard error, including the actual value
  of a failed assertion and the message of a step error;
- `--output json`, as one object with the same replacement inside strings,
  object keys and numbers;
- the `report.json` and `meta.json` of every failure bundle;
- every error message, including one produced before a case runs, such as a
  rejected step argument.

The replacement happens in the two places where `ta` writes results — the
renderer and the bundle writer — not in each driver, so a new driver or reporter
cannot bypass it. A failure bundle keeps the copy of the case file as written:
it holds `${env:NAME}` references and no values.

```console
$ export TA_TOKEN=s3cr3t-token
$ ta run cases/
FAIL cases/login.yaml [Login returns a token] 6 ms
  http.request: 1 of 2 assertions failed
    $.body.token equals "t-123" (actual "***")
  bundle: .ta/failures/1763000000123-4242/0-login-returns-a-token
Summary: 1 case (0 passed, 1 failed, 0 errors, 0 filtered), 2 steps (1 passed, 1 failed, 0 errors, 0 skipped), 7 ms
```

Redaction has limits that matter when you choose a value:

- **Short secrets hide unrelated text.** A secret of `1` replaces every `1` in
  the output, including digits of a status code or a duration. Use a value that
  is long enough to be distinctive.
- **Encoded secrets are not recognized.** If a program base64-encodes,
  URL-encodes or otherwise transforms the value, the result no longer matches
  and appears as it is. The same holds for a value a program splits or
  reassembles.
- **A number is redacted as a string.** When a secret looks like a number and a
  service echoes it back as a JSON number, the JSON output shows the string
  `"***"` there, so the type of that one value changes in the report.
- **Only the value is secret.** The name of the variable and the fact that it is
  set are not.

### Schema

`ta schema` prints a [JSON Schema](https://json-schema.org/) (draft 2020-12) for
one case file. The schema follows the parser: only `name`, `project`, `suite`,
`layer`, `tags`, `vars` and `steps` at the top level; one registered step name
per step, plus optional `expect` and `save`; the four comparisons with their
value types; and each step's argument schema under `$defs`, together with the
step's description. The classification fields are optional strings that cannot
be empty, and `tags` is an array of unique non-empty strings. The registry
builds it, so a build with fewer drivers describes fewer steps. `--output text`
(the default) pretty-prints the schema and `--output json` prints it compactly;
both are the same document, and the command exits 0.

Save the output as a file next to the cases and point an editor at it. For
[yaml-language-server](https://github.com/redhat-developer/yaml-language-server),
add a modeline to each case file:

```yaml
# yaml-language-server: $schema=./ta-case.schema.json
name: Login returns a token
steps:
  - http.request:
      url: http://127.0.0.1:8080/health
```

Or point the server's `yaml.schemas` setting at the file once:

```json
{
  "yaml.schemas": {
    "./ta-case.schema.json": "cases/**/*.yaml"
  }
}
```

The schema describes the shape of a case, not every check: undefined variables
and the syntax of JSON paths still need `ta lint`. A bound that a step declares
for an argument, such as a timeout of at least 1, is in the schema as well: it
appears as `minimum` in the step's `$defs` entry, and `ta lint` enforces the
same bound. A build with `--no-default-features` still prints a valid schema,
but no step is registered, so the step item accepts nothing.

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
not match the step's schema and undefined variables are all case errors. A case
may also declare `project`, `suite`, `layer` and `tags`; see
[Classification and filtering](#classification-and-filtering).

### Variables

`vars:` defines variables for the whole case. `save:` copies a value out of a
step result into a variable. A later step argument can use any defined variable
as `${name}`. Substitution is textual and only happens inside strings, so a
variable may not change the type of a value. Referencing a variable that is
neither in `vars:` nor saved by an earlier step is a case error. `--var
name=value` on the command line defines a variable for every case and overrides
the same name in `vars:`; `--vars-file` supplies a whole set of them (see
[Usage](#usage)).

`${env:NAME}` reads the process environment instead of a variable. It works in
a step argument, in a `vars:` value and in a `--vars-file` value, so a
connection string or a token can stay out of the case file:

```yaml
vars:
  base: "${env:TA_BASE_URL}"
steps:
  - http.request:
      url: "${base}/health"
      headers: { Authorization: "Bearer ${env:TA_TOKEN}" }
    expect:
      - path: $.status
        equals: 200
```

- The name must start with a letter or `_` and continue with letters, digits or
  `_`. An empty name or any other character is a case error (exit 3).
- A name that is not set in the environment is a case error (exit 3) whose
  message names it and says to set it before running. `ta lint` reports the same
  error, so a run that would fail this way never starts.
- A name set to an empty string counts as set and expands to nothing.
- There is no default-value syntax such as `${env:NAME:-fallback}`; a missing
  value is always an error, so a typo cannot silently use a fallback.
- Only `${env:NAME}` is expanded inside a variable's value. A `${name}`
  reference there stays as written, because only a step argument can resolve it.

A value read from the environment is a **secret**, and `ta` replaces it with
`***` in everything it writes. See [Secrets](#secrets).

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
| `case_file` | Case file, with `/` as the separator. |
| `case_name` | Name of the case. |
| `project` | Project the case declares, or `null`. |
| `suite` | Suite the case declares, or `null`. |
| `layer` | Layer the case declares, or `null`. |
| `tags` | Tags the case declares, or `[]`. |
| `status` | `failed` or `error`. |
| `created_at` | Unix milliseconds. |

`schema_version` stays `1`: the classification fields are an addition to the
layout, and a reader must ignore fields it does not know.

A case directory is staged under `.tmp-…` in the same parent, renamed into
place, and only then does `meta.json` appear. **A directory without `meta.json`
is incomplete and must be ignored**, including one left by an interrupted run.

The default directory is `.ta/failures` relative to the working directory of
`ta`. `--bundle-dir <directory>` replaces it and `--no-bundle` turns bundles off.
A run where every case passes creates nothing. When a bundle cannot be written,
the exit code of the run does not change: the problem is reported on standard
error and in the `bundle_errors` list of `--output json`.

> A bundle contains the arguments and results of every step. Values that came
> from `${env:NAME}` are replaced with `***`, but a credential that was written
> into a case file, a `--vars-file` or a `--var` value is not: treat a bundle as
> sensitive and keep its directory out of version control; `.ta/` is in this
> repository's `.gitignore`.

## Output

`--output` is a global option, so it may stand before or after the subcommand:
`ta --output json run cases/` and `ta run cases/ --output json` are the same
command. The same holds for `lint` and `schema`.

`--output text` (the default) writes one line per case to standard output, lists
each failed assertion with its path, expected value and actual value, and ends
with a summary line:

```text
FAIL cases/login.yaml [Login returns a token] 6 ms
  http.request: 1 of 2 assertions failed
    $.status equals 201 (actual 200)
  bundle: .ta/failures/1763000000123-4242/0-login-returns-a-token
Summary: 1 case (0 passed, 1 failed, 0 errors, 0 filtered), 2 steps (1 passed, 1 failed, 0 errors, 0 skipped), 7 ms
```

`--output json` writes exactly one JSON object to standard output:

```json
{
  "ok": false,
  "summary": {
    "cases": 1,
    "filtered": 0,
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
      "project": "shop",
      "suite": "smoke",
      "layer": "api",
      "tags": ["smoke", "fast"],
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
written; they never change the exit code. Every case also carries its
`project`, `suite`, `layer` (`null` when not declared) and `tags` (`[]` when
none), and `summary.filtered` counts the cases the filters excluded.

Every path that `ta` prints, in text output, in JSON and in a failure bundle,
uses `/` as the separator on every platform, so a path from the output can be
copied back unchanged.

A value that `ta` read through `${env:NAME}` appears as `***` in both formats
and in a bundle; see [Secrets](#secrets) for what that covers and where it
stops.

### Capturing JSON on Windows

`ta` writes UTF-8 to standard output, and the shell can re-encode it on the way
to a file. Windows PowerShell 5.1, the `powershell.exe` that ships with Windows,
writes the result of `>` as UTF-16LE, so the file does not start with the `{` of
a JSON object and most tools refuse it:

```powershell
ta --output json run cases/ > out.json   # out.json starts with FF FE
```

Capture the bytes `ta` wrote with `cmd` instead, which keeps UTF-8 without a
byte order mark and keeps the exit code of the run:

```powershell
cmd /c "ta --output json run cases/ > out.json"
```

`Set-Content -Encoding utf8` is no better in that version: it adds a byte order
mark (`EF BB BF`) at the start of the file. To check a captured file, look at
its first bytes; `7B`, the `{` of the object, is what a usable JSON file starts
with:

```powershell
Format-Hex -Path out.json | Select-Object -First 1
```

When no case can run, `cases` is empty and `errors` lists one entry per invalid
file, with `file`, `message` and `kind`. When the filters select no case,
`cases` and `errors` are empty, `ok` is `false`, `filter_error` holds the message
that names the filters and the available values, and the process exits 2.
Diagnostics never go to standard output.

## Exit codes

| Code | Meaning |
| ---: | --- |
| 0 | Every case ran and passed |
| 1 | At least one case failed an assertion or could not finish a step |
| 2 | Command-line usage error, including filters that select no case |
| 3 | Invalid case file, invalid step arguments, unknown step, undefined variable or an unset `${env:NAME}` |
| 4 | Environment error (for example an unreachable target) |
| 5 | Timeout |
| 9 | Reserved for a feature that is not implemented; no current command returns it |

When several kinds of problem occur in one run, the most fundamental one wins:
3, then 4, then 5, then 1.
