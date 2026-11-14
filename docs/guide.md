# do-anything guide

The complete reference for `da`: installation options, building from source, the case file format, the built-in steps, failure bundles, output and exit codes. For a quick introduction, see the [README](../README.md).

## Contents

- [Installation](#installation)
- [Build](#build)
- [Test](#test)
  - [Self-test suite](#self-test-suite)
- [Usage](#usage)
  - [Parallel runs](#parallel-runs)
  - [The service under test](#the-service-under-test)
  - [Observers](#observers)
    - [Observers and parallel runs](#observers-and-parallel-runs)
  - [Sessions](#sessions)
  - [Classification and filtering](#classification-and-filtering)
  - [Linting](#linting)
  - [Secrets](#secrets)
  - [Schema](#schema)
- [Case files](#case-files)
  - [Variables](#variables)
  - [Assertions](#assertions)
  - [`http.request`](#httprequest)
  - [`cli.run`](#clirun)
  - [`db.query`](#dbquery)
  - [`log.file`](#logfile)
- [Failure bundles](#failure-bundles)
- [Output](#output)
- [Exit codes](#exit-codes)

## Installation

Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/LnYo-Cly/do-anything/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/LnYo-Cly/do-anything/main/install.ps1 | iex
```

The scripts pick the build for your system and CPU (x86_64 or Arm64), download the latest release, check it against `SHA256SUMS`, and install `da` without administrator rights: to `~/.local/bin` on Linux and macOS, and to `%LOCALAPPDATA%\Programs\da` on Windows, which is added to your user `PATH`. On Linux and macOS the script prints the line to add when the directory is not on `PATH` yet.

If `raw.githubusercontent.com` is slow or blocked on your network, fetch the same scripts from jsDelivr:

```sh
curl -fsSL https://cdn.jsdelivr.net/gh/LnYo-Cly/do-anything@main/install.sh | sh
```

```powershell
irm https://cdn.jsdelivr.net/gh/LnYo-Cly/do-anything@main/install.ps1 | iex
```

Options, set as environment variables:

| Variable | Meaning |
| --- | --- |
| `DA_VERSION` | Release to install, for example `v0.1.0`. Default: the latest. |
| `DA_INSTALL_DIR` | Directory that receives `da`. |
| `DA_DOWNLOAD_URL` | Base URL of a mirror or proxy that serves the release files as `<base>/<tag>/<file>`. The archive and `SHA256SUMS` both come from it, so use one you trust. Finding the latest release still contacts `github.com` unless `DA_VERSION` is set. |
| `DA_NO_MODIFY_PATH` | Windows only: leave the user `PATH` unchanged. |

You can also download an archive from [GitHub Releases](https://github.com/LnYo-Cly/do-anything/releases) yourself, extract it, and put `da` on your `PATH`. Each release includes a `SHA256SUMS` file.

## Build

Requires Rust 1.89 or newer (edition 2024).

```sh
cargo build
```

The binary is written to `target/debug/da` (`target\debug\da.exe` on Windows).
The `http` feature provides the `http.request` step, the `cli` feature provides
the `cli.run` step and the `log` feature provides the `log.file` observer; all
three are enabled by default. Build a subset with
`--no-default-features --features cli`, or nothing at all with
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

`tests/self/*.yaml` are cases that test `da` with `da`: every step is a
`cli.run` that starts the binary under test. `tests/self_suite.rs` runs them with
the compiled binary, so the existing `cargo test` covers them on Linux, macOS and
Windows without a separate CI job:

```sh
cargo test --test self_suite
da run tests/self --var da=target/debug/da    # the same suite, by hand
```

`--var da=<path>` tells every case which binary to start. The cases that the
suite calls are fixtures in `tests/self/fixtures/`; they use the `.case`
extension so that the recursive directory scan of `tests/self` does not pick
them up as tests. Run one explicitly, `da run tests/self/fixtures/passing.case`,
to see what it does. The fixtures use `da` itself as the program under test, so
the suite needs neither the network nor a system command such as `echo` or
`sleep`. One case runs a case that is meant to fail, so a manual run writes a
failure bundle under `.da/failures`, which git ignores; add
`--var bundles=<directory>` to put it somewhere else.

`tests/self/observe-log.yaml` exercises the `log.file` observer. It needs a
program that appends a line to a file without a shell, and there is exactly one
such program every checkout of this repository has: `git`. The case calls
`git config --file <file> --add da.line "order created"`, which appends to the
file it names on Linux, macOS and Windows. The file it writes is ignored by
`tests/self/.gitignore`. A machine without `git` on `PATH` fails that one case
with an environment error (exit 4); the rest of the suite does not use it.

## Usage

```sh
da run cases/login.yaml
da run cases/
da run cases/ --var token=abc --var user=alice
da run cases/ --vars-file envs/local.yaml --vars-file envs/ci.yaml
da run cases/ --layer api --tag smoke
da run cases/ --tag smoke --skip-tag slow
da run cases/ --jobs 4
da run cases/ --bundle-dir /tmp/da-failures
da run cases/ --no-bundle
da --output json run cases/
da lint cases/
da lint cases/ --var token=abc
da lint cases/ --vars-file envs/ci.yaml
da --output json lint cases/
da schema
da --output json schema > da-case.schema.json
```

`da run` accepts files and directories. A directory is searched recursively for
`.yaml` and `.yml` files, which are run in path order. Every case file, every
step and every observation is validated before the first case runs: if anything
is invalid, no case runs and the exit code is 3. A step that fails an assertion
stops its own case; the remaining steps of that case are reported as skipped and
the other cases still run.

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
da run cases/ --vars-file envs/staging.yaml
```

Use it to run one set of cases against a local server, a staging deployment and
CI without editing the case files:

```sh
da run cases/ --vars-file envs/local.yaml
da run cases/ --vars-file envs/staging.yaml
da run cases/ --vars-file envs/ci.yaml --var base=http://127.0.0.1:8080
```

`da lint` accepts `--vars-file` and `--var` as well, and validates the cases
with the same values a run would use.

`--bundle-dir <directory>` changes where failure bundles are written (see
[Failure bundles](#failure-bundles)); `--no-bundle` turns them off.

### Parallel runs

`--jobs N` runs up to N cases at the same time, with N at least 1. The default
is 1, which runs them one after another. Cases are independent by design —
variables and `save` belong to one case, so no case sees another one's values —
and a set that waits on a service benefits directly: cases that each wait a
second finish in about a second when they run at the same time, instead of
adding up.

```sh
da run cases/ --jobs 4
```

The result does not depend on the job count. Cases are reported in the order
they were discovered, in text output and in JSON, and the exit code follows the
same rules; only the durations change. Every failing case still gets its own
failure bundle, because the name of a bundle directory carries the case's
position in the run. A value below 1 and a value that is not a whole number are
usage errors (exit 2), and `--jobs` belongs to `da run`; `da lint` does not have
it.

Do not raise `--jobs` when the cases are not independent. Two cases that touch
the same state — one test account, one row, one file under a fixed path — can
pass alone and fail together, and the failure looks like a race in the service.
Run that group with `--jobs 1` and a filter of its own, such as `--tag slow`,
and give the rest of the cases a run of their own.

A listener receives the events of one case in order — `CaseStarted`, the
events of its steps, then `CaseFinished` — but the events of different cases
interleave, so a listener must group them by the case index every event carries
instead of assuming the order of the run. Nothing in `da` subscribes to the bus
today; this is the contract for the plugins that will.

### The service under test

A case talks to a service that is already running: `da` sends the requests and
starts the programs a step names, but it never starts, restarts or stops the
service itself. Running a case against a server therefore takes four steps of
your own shell:

1. Start the service, in its own terminal or as a background process.
2. Wait until it accepts requests.
3. Run the cases.
4. Stop the service when the run is over. `da` leaves it running.

Pass the address with `--var` instead of writing a port into the case file, so
the same cases run against a local server and a deployed environment:

```sh
da run cases/ --var base=http://127.0.0.1:8080
```

A run that starts before the service is listening reports an environment error
(exit 4) whose message names the connection failure, and no assertion is
evaluated. That is how a wrong target is told apart from a wrong expectation: an
unreachable service is never a failed assertion.

### Observers

An interface that answers `201` has not necessarily done what it promised. An
**observer** reads another layer while a step runs — a log file today, a network
capture or a database later — so a case can assert on the evidence the step left
behind.

`observe:` is optional. It is a mapping: the key is the name you choose for the
observation, and the value is exactly one registered observer and its arguments,
written the same way as a step:

```yaml
name: Order is logged
observe:
  app_log:
    log.file:
      path: ${log_dir}/app.log
steps:
  - http.request:
      method: POST
      url: ${base}/orders
    expect:
      - path: $.status
        equals: 201
      - path: $.observed.app_log.text
        contains: "order created"
```

`text` is the new log text as one string, so `contains` asks whether it holds
that substring. `lines` is a list, and `contains` on a list asks whether one
element is exactly the expected value — not whether an element contains it. A
line such as `request method=POST ... order created` is therefore not a match for
`contains: "order created"` on `lines`. Use `text` for a substring, and a path
into one element, such as `$.observed.app_log.lines[0]`, with `equals` when the
exact line matters.

The collected value is merged into **every step result** of the case as
`observed.<name>`, before the assertions are evaluated. An ordinary `expect` path
is enough, no new assertion syntax is needed, and a failure bundle's `report.json`
carries the same values, redacted like everything else.

| Rule | Meaning |
| --- | --- |
| Name | Becomes a key of `observed`, so it uses lowercase letters, digits, `.`, `_` and `-`. A repeated name is a duplicate mapping key and the YAML parser refuses it with its line and column. |
| One observer | An entry names exactly one observer and its arguments. An unknown observer or arguments that do not satisfy its schema is a case error (exit 3), reported by `da lint` and `da run` with the line and column of the node. |
| Variables | Arguments accept `${name}` and `${env:NAME}` and are resolved once, when the case starts. A value a step `save`s is not available to an observation. |
| Isolation | Each case creates its own observation, but the file it reads is shared state: a case that observes the same log file as another case sees its lines too. See [Observers and parallel runs](#observers-and-parallel-runs). |
| Steps | The observer starts before each step and is collected after it, so it reports what that one step changed. |
| Read-only | Observers only read. The built-in ones open files for reading and write nothing. |
| Observer errors | A file that cannot be read is an environment error (exit 4); the message names the observation and the observer. |
| No result | A step that fails or times out produces no result, so its observation is not collected: there is nothing to merge it into. |

#### Observers and parallel runs

An observation is only as isolated as the file it reads. When several cases
observe one log file and run at the same time with `--jobs`, the lines another
case adds during a step are part of this case's `observed.<name>.text` and
`lines` too. The observed value then depends on how many cases run at once, and
an assertion such as `contains: "order created"` can pass on another case's line.
That is the "one file under a fixed path" shared state of
[Parallel runs](#parallel-runs), reached through an observer.

Give each case its own log file, and let the program under test write to the file
the case names. A case takes the directory from the environment and chooses the
file name itself:

```yaml
name: Customer order is logged
vars:
  log_name: customer
observe:
  app_log:
    log.file:
      path: ${log_dir}/${log_name}.log
steps:
  - http.request:
      method: POST
      url: ${base}/orders
      json: { sku: SKU-CUSTOMER }
    expect:
      - path: $.observed.app_log.text
        contains: "SKU-CUSTOMER"
```

A second case is the same file with `log_name: status` and its own SKU, and the
service writes `${log_dir}/customer.log` and `${log_dir}/status.log` — one worker
or log target per case, chosen so that a request cannot reach the other file.
Then `da run cases/ --jobs 2` observes only its own case's lines, and the result
is the same as with `--jobs 1`. `base` and `log_dir` come from a `--vars-file`, as
in [Usage](#usage).

If the program can only write one file, do not raise `--jobs` for those cases:
run them with `--jobs 1` and a `--tag` of their own, as
[Parallel runs](#parallel-runs) says. A shared log does not fail by itself — it
makes the observed value depend on the other cases, which is worse, because the
assertion can still pass.

Every built-in step returns a JSON object. If a plugin's step returns another
value and the case declares observations, the result is wrapped so that nothing
is lost: the original value moves to `value` and the observations stay under
`observed`, as in `{"value": 5, "observed": {"app_log": {"lines": []}}}`.

**Writing an observer.** The engine offers the same registration path to
built-in and external extensions. An observer declares a name, a description, a
JSON Schema for its arguments and a `validate` function that enforces the same
contract; `da schema` publishes both under `$defs` and inside `observe`. Its
factory creates one instance per case, and the instance implements two calls:
`start` before a step and `collect` after it, which returns the JSON value the
step result shows under `observed.<name>`. An environment failure from either
call is an environment error (exit 4); arguments that do not satisfy `validate`
are a case error (exit 3). Both name the observation and the observer kind. The
contract lives in `src/core/observer.rs`; `src/drivers/log.rs` is a complete
example, and `tests/architecture.rs` keeps the engine free of any concrete
observer.

### Sessions

A step that talks to a browser, a device or a connection needs the same
resource in several steps: the page opened by step one is the page step two
clicks, and the network observer has to read that page. A **session** is that
resource. It belongs to one case, is created the first time a step asks for it,
and is closed when the case ends.

The engine knows nothing about the resource. A plugin declares its own type,
implements `Session`, and stores it under a kind it chooses:

```rust
use do_anything::core::session::{Session, Sessions};
use do_anything::core::registry::StepError;

struct Page {
    // A connection, a child process, a device handle, ...
}

impl Session for Page {
    fn close(&mut self) -> Result<(), String> {
        // Release the page. Return a message if that fails.
        Ok(())
    }
}

/// Opens the resource, or explains why it could not.
fn launch_page() -> Result<Page, String> {
    // ...
}

fn open(
    _args: &serde_json::Value,
    sessions: &mut Sessions,
) -> Result<serde_json::Value, StepError> {
    let page = sessions
        .get_or_create("browser.page", launch_page)
        .map_err(StepError::Execution)?;
    // Use `page` for this step; later steps ask for the same kind.
    Ok(serde_json::json!({ "opened": true }))
}
```

`get_or_create` takes the concrete type as a generic parameter and the kind as
a `&'static str`. The first call runs the factory; every later call of the same
case returns the value already stored. A factory that returns an error leaves
no slot behind, so a later step may try again. Asking for one kind with two
different types is an error, not a panic: one kind holds one concrete type for
the whole case.

A kind follows the step name rule: lowercase letters, digits, `.`, `_` and
`-`. It is the plugin's own namespace and does not appear in a case file, so
two plugins can choose their own kinds without coordinating.

An observer receives the same `Sessions` value in `start` and `collect`:

```rust
use do_anything::core::observer::{Observer, ObserverError};
use do_anything::core::session::Sessions;
use serde_json::Value;

impl Observer for NetworkCapture {
    fn start(&mut self, _sessions: &mut Sessions) -> Result<(), ObserverError> {
        Ok(())
    }

    fn collect(&mut self, sessions: &mut Sessions) -> Result<Value, ObserverError> {
        let page = sessions
            .get::<Page>("browser.page")
            .map_err(ObserverError::Environment)?
            .ok_or_else(|| ObserverError::Environment("no page was opened".to_string()))?;
        // Read the requests the page made during the step.
        Ok(serde_json::json!({ "requests": [] }))
    }
}
```

`start` runs before the step, so on the first step the session may not exist
yet; `collect` runs after it, so that is where a resource the step opened is
available. `get` returns `Ok(None)` for a kind the case never opened and an
error for a kind held by another type.

| Rule | Meaning |
| --- | --- |
| One set per case | The runner creates a new `Sessions` for every case. Cases that run at the same time with `--jobs` cannot see each other's sessions, so a plugin must not keep the resource in a global static. |
| Created on first use | The factory runs when a step or an observation first asks for the kind. A case that never asks creates nothing. |
| Closed at the end | The runner closes every open session in reverse creation order, whatever the case did: passed, failed an assertion, errored, timed out, or stopped before the remaining steps. Each session is closed exactly once. |
| Close failure | The message names the session kind. A case that would have passed becomes an environment error (exit 4). A case that already failed or errored keeps its own status and problem, and the close error is attached to the report. |
| Where it appears | Text output adds a `close:` line under the case. JSON adds `close_errors` to the case object, and a failure bundle's `report.json` carries the same field. Close messages are redacted like every other string. |
| Fallback | `Sessions` also closes what is still open when it is dropped, for a panic or an early return. The normal path does not rely on it, and a fallback close cannot be reported because the report is already gone. |

The built-in `http.request`, `cli.run`, `db.query` and `log.file` use no
session, so their behavior is unchanged: each step still opens and closes what
it needs within the step. The contract exists for the drivers and observers
that need a resource across steps.

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

`da run` selects cases with `--project`, `--suite`, `--layer`, `--tag` and
excludes them with `--skip-tag`; each option may be repeated:

```sh
da run cases/ --layer api
da run cases/ --project shop --project billing
da run cases/ --layer api --layer cli --tag smoke
da run cases/ --tag smoke --skip-tag slow
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

When the filters select no case, `da run` exits 2 and the message names the
options that were used and the `project`, `suite`, `layer` and tag values the
discovered cases actually have, so a typo is visible without running anything.
A run without filters is not affected by this rule.

Filtered-out cases do not appear in the result; the summary counts them as
`filtered` (text) or `summary.filtered` (JSON).

### Linting

`da lint` takes the same files and directories as `da run`, and the same
`--var name=value` and `--vars-file` options. It discovers, loads and validates
every case with the same preflight that `da run` uses, but it does not execute a
step, write a failure bundle or emit run events. A case that lint accepts is a
case that `da run` accepts before it starts; problems that only appear while a
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
4 |       command: da
5 |     expect:
6 |       - path: $.exit_code
  |         ^ step 1: assertion 1: unknown field(s) equal; use one of equals, not_equals, contains, exists
7 |         equal: 0
  |
cases/logout.yaml: line 2 column 1: unknown field `naem`, expected one of name, vars, project, suite, layer, tags, observe, steps
 --> <input>:2:1
  |
1 | name: Logout
2 | naem: typo
  | ^ unknown field `naem`, expected one of name, vars, project, suite, layer, tags, observe, steps
3 | steps:
4 |   - cli.run:
  |
Checked 2 files: 2 errors.
```

Problems that need the registry — an unregistered step name, step arguments that
do not match the step's schema, an undefined variable, or an observation that
names an unknown observer — name the node the user has to edit and carry its line
and column, because the parser cannot know them. An unregistered step points at
its name, an argument error at the step's argument mapping, and an undefined
variable at the value that holds the reference:

```text
cases/login.yaml: line 12 column 33: step 2: undefined variable `${acces_token}`; define it under `vars:` or save it in an earlier step
  --> <input>:12:33
   |
10 |   - http.request:
11 |       url: ${base}/me
12 |       headers: { Authorization: "Bearer ${acces_token}" }
   |                                 ^ step 2: undefined variable `${acces_token}`; define it under `vars:` or save it in an earlier step
Checked 1 file: 1 error.
```

```text
cases/login.yaml: line 4 column 5: observation `app_log`: `log.fil` is not a registered observer; registered observers: `log.file`
 --> <input>:4:5
  |
2 | observe:
3 |   app_log:
4 |     log.fil:
  |     ^ observation `app_log`: `log.fil` is not a registered observer; registered observers: `log.file`
5 |       path: app.log
  |
```

`--output json` writes exactly one object:

```json
{
  "ok": false,
  "files": 2,
  "errors": [
    {
      "file": "cases/login.yaml",
      "message": "line 12 column 33: step 2: undefined variable `${acces_token}`; define it under `vars:` or save it in an earlier step"
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

A located message also carries the source snippet, with its line breaks escaped
as `\n` inside the JSON string; the example shows the first line only.

`cases` lists every case that parsed, with its classification, so an agent can
discover the available values without running anything. `project`, `suite` and
`layer` are `null` when the case does not declare them.

The exit code is 0 when every case is valid and 3 when anything is invalid,
including a path that cannot be read and a directory with no case files. Use
`da lint tests/self --var da=target/debug/da` to check the self suite without
running it.

### Secrets

A value that `da` read through `${env:NAME}` is treated as a secret. The rule
follows the source, not a declaration: nothing has to be marked, so a value
cannot be forgotten. Values from `vars:`, `--vars-file` and `--var` are not
secrets and stay readable, because they are already visible in a file or in the
command line.

Before anything is written, every occurrence of every secret is replaced with
`***`. That covers:

- text output on standard output and standard error, including the actual value
  of a failed assertion and the message of a step error;
- `--output json`, as one object with the same replacement inside strings,
  object keys, numbers, booleans and `null`;
- the `report.json` and `meta.json` of every failure bundle;
- every error message, including one produced before a case runs, such as a
  rejected step argument.

A string or an object key keeps its own text and is redacted in place. A number,
a boolean or `null` has no text of its own to replace, so its JSON text is
redacted and the result becomes a string: with the secret `1`, the number `10`
becomes `"***0"`, and with the secret `true`, the boolean `true` becomes
`"***"`. A token that contains no secret keeps its type.

The replacement happens in the two places where `da` writes results — the
renderer and the bundle writer — not in each driver, so a new driver or reporter
cannot bypass it. A failure bundle keeps the copy of the case file as written:
it holds `${env:NAME}` references and no values.

```console
$ export DA_TOKEN=s3cr3t-token
$ da run cases/
FAIL cases/login.yaml [Login returns a token] 6 ms
  http.request: 1 of 2 assertions failed
    $.body.token equals "t-123" (actual "***")
  bundle: .da/failures/1763000000123-4242/0-login-returns-a-token
Summary: 1 case (0 passed, 1 failed, 0 errors, 0 filtered), 2 steps (1 passed, 1 failed, 0 errors, 0 skipped), 7 ms
```

Redaction has limits that matter when you choose a value:

- **Short secrets hide unrelated text.** A secret of `1` replaces every `1` in
  the output, including digits of a status code or a duration. It also rewrites
  numbers in JSON: a timestamp such as `created_at` in `meta.json` becomes a
  string with the digits replaced. Use a value that is long enough to be
  distinctive.
- **Only values `da` read are secrets.** A program a case starts inherits the
  whole environment. If it prints a variable that no `${env:NAME}` reference
  read, `da` never saw that value and cannot redact it.
- **Encoded secrets are not recognized.** If a program base64-encodes,
  URL-encodes or otherwise transforms the value, the result no longer matches
  and appears as it is. The same holds for a value a program splits or
  reassembles.
- **A JSON scalar changes type when it contains a secret.** A number, a boolean
  or `null` that is redacted becomes a string, so the type of that one value
  changes in the report.
- **Only the value is secret.** The name of the variable and the fact that it is
  set are not.

### Schema

`da schema` prints a [JSON Schema](https://json-schema.org/) (draft 2020-12) for
one case file. The schema follows the parser: only `name`, `project`, `suite`,
`layer`, `tags`, `vars`, `observe` and `steps` at the top level; one registered
step name per step, plus optional `expect` and `save`; one registered observer
and its arguments per entry of `observe`; the four comparisons with their value
types; and each step's and observer's argument schema under `$defs`, together
with its description. The classification fields are optional strings that cannot
be empty, `tags` is an array of unique non-empty strings, and an observation name
matches `^[a-z0-9._-]+$`. The registry builds it, so a build with fewer features
describes fewer steps and observers. `--output text` (the default) pretty-prints
the schema and `--output json` prints it compactly; both are the same document,
and the command exits 0.

Save the output as a file next to the cases and point an editor at it. For
[yaml-language-server](https://github.com/redhat-developer/yaml-language-server),
add a modeline to each case file:

```yaml
# yaml-language-server: $schema=./da-case.schema.json
name: Login returns a token
steps:
  - http.request:
      url: http://127.0.0.1:8080/health
```

Or point the server's `yaml.schemas` setting at the file once:

```json
{
  "yaml.schemas": {
    "./da-case.schema.json": "cases/**/*.yaml"
  }
}
```

The schema describes the shape of a case, not every check: undefined variables
and the syntax of JSON paths still need `da lint`. A bound that a step declares
for an argument, such as a timeout of at least 1, is in the schema as well: it
appears as `minimum` in the step's `$defs` entry, and `da lint` enforces the
same bound. A build with `--no-default-features` still prints a valid schema,
but no step is registered, so the step item accepts nothing.

## Case files

A case is a YAML file with a name, optional variables, optional observations and
a list of steps. Each step is a mapping with exactly one step name, optionally
followed by `expect` and `save`:

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

Unknown fields in a case file, unregistered step names, unregistered observers,
step or observer arguments that do not match their schema, and undefined
variables are all case errors. A case may also declare `project`, `suite`,
`layer` and `tags` (see
[Classification and filtering](#classification-and-filtering)) and `observe`
(see [Observers](#observers)).

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
  base: "${env:DA_BASE_URL}"
steps:
  - http.request:
      url: "${base}/health"
      headers: { Authorization: "Bearer ${env:DA_TOKEN}" }
    expect:
      - path: $.status
        equals: 200
```

- The name must start with a letter or `_` and continue with letters, digits or
  `_`. An empty name or any other character is a case error (exit 3).
- A name that is not set in the environment is a case error (exit 3) whose
  message names it and says to set it before running. `da lint` reports the same
  error, so a run that would fail this way never starts.
- A name set to an empty string counts as set and expands to nothing.
- There is no default-value syntax such as `${env:NAME:-fallback}`; a missing
  value is always an error, so a typo cannot silently use a fallback.
- Only `${env:NAME}` is expanded inside a variable's value. A `${name}`
  reference there stays as written, even where the variable is later used in a
  step argument: substitution is a single pass and does not rescan the text it
  inserts.

A value read from the environment is a **secret**, and `da` replaces it with
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
| `command` | Program name or path. Required. A relative path is resolved against the working directory of `da`. |
| `args` | List of arguments, one per entry. Nothing is quoted, expanded or split. |
| `cwd` | Working directory of the program. Defaults to the working directory of `da`. |
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
and `cwd` are resolved against the working directory of the `da` process, not
against the case file.

A program that cannot be started is an environment error (exit 4) whose message
names it and says what to check. When `timeout_ms` expires the program is killed
and the step is a timeout (exit 5). Both pipes are read on their own threads, so
a program that writes more than a pipe buffer cannot deadlock the run; after the
program ends `da` waits at most one second for the pipes to close, so a process
the program left behind cannot hang it.

### `db.query`

Reads the database to check what an earlier step wrote. The step runs exactly
one read-only statement against SQLite or PostgreSQL and returns what the
database answered.

| Argument | Meaning |
| --- | --- |
| `url` | Connection string. Required. `sqlite:<path>` names a SQLite file, `postgres://` and `postgresql://` are a PostgreSQL URL. |
| `sql` | One SQL statement. Required. |
| `params` | Values bound to the placeholders, in order. Each one is a string, a number, a boolean or null. |
| `max_rows` | Largest number of rows to return. Defaults to `1000`; the step reads one row more to know whether the result was longer. |
| `timeout_ms` | Timeout for connecting and for the whole query. Defaults to `30000`. |

```yaml
name: The order was written
steps:
  - http.request:
      method: POST
      url: ${base}/orders
      json: { id: 42, total: 19.9 }
    save:
      order_id: $.body.id
  - db.query:
      url: ${env:DATABASE_URL}
      sql: SELECT status, total FROM orders WHERE id = $1
      params: ["${order_id}"]
    expect:
      - path: $.rows[0].status
        equals: paid
      - path: $.row_count
        equals: 1
```

The result is an object with `columns`, `rows`, `row_count` and `truncated`:

| Field | Meaning |
| --- | --- |
| `columns` | Column names, in the order the database returned them. |
| `rows` | One object per row, keyed by column name. |
| `row_count` | Number of entries in `rows`. |
| `truncated` | `true` when the statement matched more rows than `max_rows`, in which case only `max_rows` are returned. |

A SQLite `url` is a path: a relative one is resolved against the working
directory of `da`, not against the case file. `sqlite:` also accepts an absolute
path, for example `sqlite:C:/data/app.db` or `sqlite:/var/lib/app.db`. Any other
prefix is an invalid argument (exit 3).

#### Parameters

SQLite takes `?` for an anonymous placeholder and `?1`, `?2` for numbered ones;
PostgreSQL takes `$1`, `$2`. A boolean bound to SQLite becomes `1` or `0`,
because SQLite has no boolean type of its own. A parameter list that does not
match the placeholders is refused by the database.

On PostgreSQL each value is sent at the width its placeholder declares, so
`WHERE id = $1` works against an `integer` column with `params: [42]`. A number
that does not fit the placeholder — `70000` for a `smallint`, a fraction for an
`integer` — is an invalid argument (exit 3) whose message names the parameter
and the type.

#### Types

| Database type | JSON |
| --- | --- |
| integer | number |
| real, double precision | number |
| text, varchar, char, enum | string |
| boolean (PostgreSQL) | `true` / `false` |
| `NULL` | `null` |
| blob, bytea | lower-case hexadecimal string, for example `"00ff"` |
| `numeric`, `decimal`, `uuid`, `date`, `time`, `timestamp`, `timestamptz`, `interval`, arrays, ranges, and any other PostgreSQL type | string, exactly as PostgreSQL prints it under `::text` |
| `json`, `jsonb` | the JSON value itself |

A PostgreSQL column whose type is not in the first rows of that table is
requested as `::text` in a wrapper query, so it still arrives as a string
instead of failing the step. The column names, the order and the `params` are
unchanged by that wrapper.

#### Read-only

`db.query` cannot write, and the guarantee does not rest on looking at the SQL:

- SQLite opens the file read-only, so the connection has no write permission,
  and it also sets `PRAGMA query_only`. A database file that does not exist is
  an environment error (exit 4) — the step never creates one.
- PostgreSQL sets `SESSION CHARACTERISTICS AS TRANSACTION READ ONLY` and runs
  the statement in an explicit `READ ONLY` transaction that is always rolled
  back. A statement that tries to write is refused by the server.
- Only one statement is accepted. SQLite refuses a second statement after it
  prepares the first one, and PostgreSQL prepares through the extended protocol,
  which cannot carry two commands.

`INSERT`, `UPDATE`, `DELETE`, `CREATE`, `DROP`, a write hidden in a common table
expression such as `WITH gone AS (DELETE FROM orders RETURNING *) SELECT * FROM
gone`, and a statement list such as `SELECT 1; DELETE FROM orders` all fail the
step with an environment error (exit 4) whose message says the step is read-only.
The database is left exactly as it was. SQLite does not accept a data-modifying
common table expression at all, so there the statement is a syntax error before
either read-only layer sees it; PostgreSQL accepts the syntax, and the read-only
transaction is what refuses it.

One side effect is outside the database file itself: SQLite refuses
`VACUUM INTO 'copy.db'` because the connection is read-only, but it opens the
target file before it stops, so an empty `copy.db` can be left behind. No data
is copied. The step's "never created" promise is about the database it opens.

#### Connection strings and secrets

Never write a password into a case file. Read the whole connection string from
the environment instead:

```yaml
- db.query:
    url: ${env:DATABASE_URL}
    sql: SELECT 1
```

A value that comes from `${env:NAME}` is a secret: it is replaced with `***` in
the terminal, in `--output json` and in every file of a failure bundle, so a
connection string that appears in an error message cannot leak the password. See
[Secrets](#secrets).

A refused connection and a statement the server rejects are both environment
errors (exit 4); exceeding `timeout_ms` is a timeout (exit 5). The message names
the SQLite file it could not open, but never repeats a PostgreSQL URL, which
usually carries a password.

### `log.file`

Reports the text a log file gained while the step ran.

| Argument | Meaning |
| --- | --- |
| `path` | File to read. Required. Read as UTF-8; invalid bytes become replacement characters. |

The result is an object with `lines`, `text`, `truncated` and `limited`:

| Field | Meaning |
| --- | --- |
| `lines` | The new text split into lines. A trailing `\r` is dropped, so a CRLF log reads like an LF one; a final newline does not start an empty line; a last line without a newline still counts as a line. `contains` on this list requires one element to equal the expected value; it is not a substring match, so use `text` for a substring. |
| `text` | The new text exactly as read. |
| `truncated` | `true` when the file is shorter than it was when the step started — it was truncated or rotated — and the read therefore restarted at the beginning. |
| `limited` | `true` when the step added more than 1 MiB: only the first 1 MiB is reported, so one noisy step cannot fill a failure bundle or a terminal. |

```yaml
name: Worker logs the order
observe:
  worker_log:
    log.file:
      path: ${log_dir}/worker.log
steps:
  - cli.run:
      command: ./worker
      args: ["--once"]
    expect:
      - path: $.exit_code
        equals: 0
      - path: $.observed.worker_log.text
        contains: "order created"
      - path: $.observed.worker_log.truncated
        equals: false
```

- The read starts at the length the file had when the step started, so lines
  that were already there never appear.
- A file that does not exist is empty, not an error: a program that creates it
  during the step is exactly what this observer is for. The observer never
  creates or changes the file.
- A path that is not a regular file — a directory, for example — and a file
  that cannot be read are environment errors (exit 4) that name the path and
  the reason. The check happens before the read, so the reason is the same on
  every platform.
- Rotation is detected by length. A file that is replaced between two steps by
  one of the same length or longer cannot be told apart from an append, and a
  file that is rewritten between two steps is read from its recorded length.

## Failure bundles

Terminal output is truncated and gone after the run, so every case that does not
pass also leaves a directory that an agent can read afterwards:

```text
.da/failures/
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
| `da_version` | Version of `da` that wrote the bundle. |
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

The default directory is `.da/failures` relative to the working directory of
`da`. `--bundle-dir <directory>` replaces it and `--no-bundle` turns bundles off.
A run where every case passes creates nothing. When a bundle cannot be written,
the exit code of the run does not change: the problem is reported on standard
error and in the `bundle_errors` list of `--output json`.

> A bundle contains the arguments and results of every step. Values that came
> from `${env:NAME}` are replaced with `***`, but a credential that was written
> into a case file, a `--vars-file` or a `--var` value is not: treat a bundle as
> sensitive and keep its directory out of version control; `.da/` is in this
> repository's `.gitignore`.

## Output

`--output` is a global option, so it may stand before or after the subcommand:
`da --output json run cases/` and `da run cases/ --output json` are the same
command. The same holds for `lint` and `schema`.

`--output text` (the default) writes one line per case to standard output, lists
each failed assertion with its path, expected value and actual value, and ends
with a summary line:

```text
FAIL cases/login.yaml [Login returns a token] 6 ms
  http.request: 1 of 2 assertions failed
    $.status equals 201 (actual 200)
  bundle: .da/failures/1763000000123-4242/0-login-returns-a-token
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
      "bundle": ".da/failures/1763000000123-4242/0-login-returns-a-token",
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

Every path that `da` prints, in text output, in JSON and in a failure bundle,
uses `/` as the separator on every platform, so a path from the output can be
copied back unchanged.

A value that `da` read through `${env:NAME}` appears as `***` in both formats
and in a bundle; see [Secrets](#secrets) for what that covers and where it
stops.

### Capturing JSON on Windows

`da` writes UTF-8 to standard output, and the shell can re-encode it on the way
to a file. Windows PowerShell 5.1, the `powershell.exe` that ships with Windows,
writes the result of `>` as UTF-16LE, so the file does not start with the `{` of
a JSON object and most tools refuse it:

```powershell
da --output json run cases/ > out.json   # out.json starts with FF FE
```

Capture the bytes `da` wrote with `cmd` instead, which keeps UTF-8 without a
byte order mark and keeps the exit code of the run:

```powershell
cmd /c "da --output json run cases/ > out.json"
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
| 3 | Invalid case file, invalid step arguments, unknown step, unknown observer, invalid observer arguments, undefined variable or an unset `${env:NAME}` |
| 4 | Environment error (for example an unreachable target, or a log file that cannot be read) |
| 5 | Timeout |
| 9 | Reserved for a feature that is not implemented; no current command returns it |

When several kinds of problem occur in one run, the most fundamental one wins:
3, then 4, then 5, then 1.
