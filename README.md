# test-anything

An agent-friendly, cross-layer testing tool written in Rust.

`ta` is a single binary that runs declarative test cases against HTTP APIs and,
later, command-line targets, and reports results in a form that both people and
coding agents can read. Status: early development. `ta run` executes YAML cases
with the built-in `http.request` step; `ta lint` and `ta schema` are not
implemented yet and exit with code 9.

## Install

Download a prebuilt binary for Linux, macOS or Windows from [GitHub Releases](https://github.com/LnYo-Cly/test-anything/releases), extract it, and put `ta` on your `PATH`. Each release includes a `SHA256SUMS` file.

## Build

Requires Rust 1.89 or newer (edition 2024).

```sh
cargo build
```

The binary is written to `target/debug/ta` (`target\debug\ta.exe` on Windows).
The `http` feature, which provides the `http.request` step, is enabled by
default. Build without any driver with `--no-default-features`.

## Test

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test
```

The integration tests in `tests/cli.rs` run the compiled binary against a local
HTTP server that each test starts itself. They never reach the network.

## Usage

```sh
ta run cases/login.yaml
ta run cases/
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
neither in `vars:` nor saved by an earlier step is a case error.

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

## Output

`--output text` (the default) writes one line per case to standard output, lists
each failed assertion with its path, expected value and actual value, and ends
with a summary line:

```text
FAIL cases/login.yaml [Login returns a token] 6 ms
  http.request: 1 of 2 assertions failed
    $.status equals 201 (actual 200)
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
  "errors": []
}
```

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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) ([简体中文](CONTRIBUTING.zh-CN.md)) for the commit and pull request format, project structure, code rules and dependency policy.

## License

MIT. See [LICENSE](LICENSE).
