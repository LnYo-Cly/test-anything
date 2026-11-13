<div align="center">

# test-anything

**One binary to test anything — built for coding agents and the people who work with them.**

Write a test as YAML, run it with `ta`, get a clear verdict, a precise exit code and machine-readable evidence.

[![CI](https://github.com/LnYo-Cly/test-anything/actions/workflows/ci.yml/badge.svg)](https://github.com/LnYo-Cly/test-anything/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/LnYo-Cly/test-anything?sort=semver)](https://github.com/LnYo-Cly/test-anything/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.89+](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey.svg)

**English** · [简体中文](README.zh-CN.md)

[Install](#install) · [Quick start](#quick-start) · [Guide](docs/guide.md) · [Roadmap](#roadmap) · [Contributing](CONTRIBUTING.md)

</div>

---

## Why `ta`?

- **One small static binary.** No runtime, no package manager, no browser download. Linux, macOS and Windows on x86_64 and Arm64.
- **Made for agents.** Every run can be one JSON object, every outcome has its own exit code, and every failure leaves a bundle on disk that an agent can read after the terminal output is gone.
- **Declarative cases.** Steps, assertions and variables in plain YAML that people can review and agents can write. Invalid cases are rejected before anything runs, with line and column.
- **Cross-layer by design.** Actions start where a user or client starts them (an API, a command, later a UI); results are asserted with JSONPath on any step's output.
- **Pluggable.** Built-in steps register through the same registry that external drivers will use. The core knows nothing about HTTP or processes.
- **Tests itself.** The project's own self-test suite is written in `ta` and runs on every platform in CI.

## Install

**Linux and macOS**

```sh
curl -fsSL https://raw.githubusercontent.com/LnYo-Cly/test-anything/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/LnYo-Cly/test-anything/main/install.ps1 | iex
```

The scripts pick the right build for your system and CPU, verify it against `SHA256SUMS`, and need no administrator rights. Mirror, version pinning and other options are in the [installation guide](docs/guide.md#installation); archives are also on the [Releases](https://github.com/LnYo-Cly/test-anything/releases) page.

## Quick start

Create `cases/smoke.yaml`:

```yaml
name: GitHub API returns the repository
steps:
  - http.request:
      url: https://api.github.com/repos/LnYo-Cly/test-anything
      headers: { User-Agent: ta-demo }
    expect:
      - path: $.status
        equals: 200
      - path: $.body.license.spdx_id
        equals: MIT
```

and `cases/git.yaml`, which checks a command-line program:

```yaml
name: git is installed
steps:
  - cli.run:
      command: git
      args: ["--version"]
    expect:
      - path: $.exit_code
        equals: 0
      - path: $.stdout
        contains: "git version 3"
```

Run them:

```console
$ ta run cases/
FAIL cases/git.yaml [git is installed] 70 ms
  cli.run: 1 of 2 assertions failed
    $.stdout contains "git version 3" (actual "git version 2.45.1\n")
  bundle: .ta/failures/1794581562319-43704/0-git-is-installed
PASS cases/smoke.yaml [GitHub API returns the repository] 1857 ms
Summary: 2 cases (1 passed, 1 failed, 0 errors), 2 steps (1 passed, 1 failed, 0 errors, 0 skipped), 1936 ms
$ echo $?
1
```

The failed case says which assertion failed and what the real value was, and its evidence is in `.ta/failures/…`. Add `--output json` for a single JSON object instead of text.

## What you can test today

| Step | Tests | Result you assert on |
| --- | --- | --- |
| `http.request` | HTTP and HTTPS APIs | `status`, `headers`, `body` (parsed when JSON) |
| `cli.run` | Command-line programs, run without a shell | `exit_code`, `stdout`, `stderr`, `json` |

Assertions: `equals`, `not_equals`, `contains`, `exists` on any [JSONPath](https://www.rfc-editor.org/rfc/rfc9535.html). Variables: `vars`, `save` and `--var name=value`, referenced as `${name}`.

## For coding agents

`ta` is meant to be driven by an agent without a human reading the terminal:

| Need | What `ta` gives |
| --- | --- |
| A verdict | Exit code `0` passed, `1` failed, `2` usage, `3` invalid case, `4` environment, `5` timeout |
| Details | `ta --output json run …`: one JSON object with every step result and assertion |
| Evidence after the run | A failure bundle per failed case: `case.yaml`, `report.json`, `meta.json` (written last) |
| Inputs from outside | `--var name=value` for URLs, tokens and paths |

Everything is in the [guide](docs/guide.md): [case format](docs/guide.md#case-files), [output](docs/guide.md#output), [failure bundles](docs/guide.md#failure-bundles), [exit codes](docs/guide.md#exit-codes).

## Roadmap

`ta` grows one layer at a time. Released features are checked.

- [x] YAML cases, assertions, variables, JSON output, precise exit codes
- [x] `http.request` and `cli.run` steps, failure bundles, self-test suite
- [x] One-line installers; Linux, macOS and Windows on x86_64 and Arm64
- [ ] `ta lint` and `ta schema` — in progress
- [ ] Case classification and filters: project, suite, layer, tags
- [ ] Read-only observers for network, database and logs
- [ ] Web UI testing
- [ ] Agent integration: MCP server and skill files
- [ ] Android, desktop apps, iOS, HarmonyOS
- [ ] External drivers as plugins, for domains beyond software

## Documentation

- [Guide](docs/guide.md) — installation options, case format, steps, failure bundles, output, exit codes
- [Release notes](https://github.com/LnYo-Cly/test-anything/releases)
- [Contributing](CONTRIBUTING.md) — workflow, code rules, dependency policy

## Contributing

Issues and pull requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) ([简体中文](CONTRIBUTING.zh-CN.md)) first: it covers the commit and pull request format, project structure and how new dependencies are chosen. To build and test locally:

```sh
cargo build
cargo test
```

## License

[MIT](LICENSE)
