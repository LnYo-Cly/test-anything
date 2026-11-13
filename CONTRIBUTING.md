# Contributing to test-anything

[简体中文](CONTRIBUTING.zh-CN.md)

These rules apply to every change, whether it comes from a person or a coding agent.

## Workflow

1. Branch from `main`. One PR does one thing.
2. Run the checks locally before opening the PR:

   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```

3. Open the PR with the template. CI must pass on Linux, Windows and macOS.
4. PRs are squash-merged. The PR title becomes the commit message.

## Commits and PR titles

Use [Conventional Commits](https://www.conventionalcommits.org/), in English:

```
<type>(<scope>): <summary>
```

- `type`: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `build`, `ci`, `chore`.
- `scope` is optional and names the module, for example `core`, `http`, `cli`, `bundle`.
- `summary` is imperative, at most 72 characters, with no trailing period.
- A breaking change adds `!` after the type or scope and a `BREAKING CHANGE:` footer.

Example: `feat(http): add JSON path assertions`

## Pull request description

Every PR uses the same sections, in this order. Each section is written in English first, then Chinese. Write `None / 无` in a section that has nothing to report; do not delete it.

| Section | Content |
| --- | --- |
| `## Summary / 摘要` | What changed and why, in one to three sentences per language. |
| `## Changes / 变更` | A list of the behavior changes. |
| `## Verification / 验证` | The commands that were run and their real results, plus what was not verified. |
| `## Dependencies / 依赖` | Every dependency added, removed or upgraded, with the comparison required below. |
| `## Risks and follow-ups / 风险与后续` | Known gaps, risks, and work left for later. |

Rules for the description:

- Report real command results. Do not write "passed" for something that was not run.
- State what was not verified.
- No local paths, user names, machine names or secrets.

The template is in [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md).

## Project structure

| Path | Purpose |
| --- | --- |
| `src/main.rs` | Entry point only: parse arguments, call the library, map the result to an exit code. |
| `src/exit.rs` | The only place exit codes are defined. |
| `src/core/` | Domain-neutral engine: case model, registry, event bus, runner, failure bundle, output. |
| `src/drivers/` | One module per driver (`http`, `cli`, ...), each behind its own cargo feature. |
| `tests/` | Integration tests that run the compiled binary. |
| `tests/self/` | Cases that `ta` runs against itself. |

`src/core/`, `src/drivers/` and `tests/self/` are created by the changes that first need them.

Structure rules:

- `core` never imports from `drivers`, and its types do not mention a specific domain such as HTTP or the web.
- Drivers reach the engine only through the registry. Built-in drivers use the same registration API as external ones; there is no private path.
- A step takes JSON arguments, returns a JSON result, and declares a JSON Schema for its arguments.
- Exit codes come only from `src/exit.rs`. JSON output is produced in one place.

## Code rules

- Format with rustfmt defaults. Clippy runs with `-D warnings`.
- No `unsafe` code; the `unsafe_code` lint is set to `forbid`.
- An `#[allow(...)]` attribute needs a comment that explains why.
- No `unwrap`, `expect` or `panic!` on a path that can run in production. Return an error that says what failed.
- Every user-facing error maps to one exit code and tells the user what to do next.
- Standard output carries results only. With `--output json` it carries exactly one JSON object. Diagnostics go to standard error.
- Public items have doc comments. Comments explain why, not what.
- No speculative abstraction: no trait with a single implementation unless it is a registry extension point, and no configuration for a value that never changes.
- Never log a secret. Redact secrets in output, reports and failure bundles.
- Code, comments, documentation and commit messages are in English.

Cross-platform rules:

- Build paths with `std::path`; never hard-code a separator.
- Read and write text as UTF-8, regardless of the console code page.
- Do not assume a particular shell when starting a process.
- Files in the repository use LF line endings.

## Tests

- New behavior ships with tests. A bug fix starts with a test that fails before the fix.
- Test the failure path as well as the success path.
- Tests do not call external network services or an AI model. Use a local server started by the test.
- Do not weaken or delete an assertion to make a check pass.

## Dependencies

Reuse before you build. Before writing new functionality, search crates.io, lib.rs and GitHub for a library that already does it.

Evaluate each candidate, and compare at least two when more than one exists:

| Criterion | What to check |
| --- | --- |
| Maintenance | A release within about the last 12 months; issues receive answers. |
| Adoption | Download counts and well-known dependents. |
| License | MIT, Apache-2.0, BSD, ISC, Zlib or Unicode. No copyleft license. |
| Weight | Number of transitive dependencies, compile time, effect on binary size. |
| Safety | No open RustSec advisory. |
| Platforms | Works on Linux, Windows and macOS. |
| Quality | Has tests and documentation; performance fits our use. |

Use the library when it meets these criteria. Write our own implementation only when one of these holds:

- No candidate meets the criteria.
- The best candidate has a confirmed bug or performance problem that affects us. Link the issue or include the measurement.
- The need is small, roughly under 50 lines, and a dependency would cost more than it saves.

When adding a dependency, enable only the features that are used, and record the following in the PR's Dependencies section: the crate, what it is for, the alternatives that were considered, and why this one was chosen. When choosing to build instead, record which libraries were rejected and why.

CI runs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) to check licenses, security advisories and dependency sources. Its configuration is in `deny.toml`.
