# Harness Agent Entry

This document defines repository operating rules and the system ontology. In this repository, Harness serves as a cognitive ledger engine helping you and human collaborators maintain alignment and track evolutionary progress. Real-world code and tests are the objective reality; Harness records the causal evolution behind them.

## Context Loading

- Load context on demand; avoid preloading the entire codebase or knowledge tree.
- Run `ha settings read` for the repository governance baseline upon startup.
- When assigned a task, read only `task_plan.md` and the code/context files explicitly referenced, keeping context concise and focused.

## Worktree Discipline

- Maintain workspace isolation: complex feature development or refactoring should happen in isolated branches or worktrees to avoid polluting the main working tree.
- Stage and commit only task-owned paths, preserving unrelated changes.
- Follow declared base branch, merge, cleanup, and publication instructions.

## Kernel Workflow

- **Single Source of Truth vs Read Projections (SSoT)**: The resident daemon's single-writer event stream and SQLite Outbox maintain authoritative state; markdown files under `harness/` are read projections. Always use `ha` semantic commands instead of manually editing machine-read metadata.
- **The Triad Causal Loop in Software Engineering**:
  - **Fact**: Ground truth observations from the objective world. In software engineering, "unmeasured modification is blind guessing": failing test outputs, benchmark numbers, dependency versions, and verified inputs/outputs are Facts. Load-bearing claims must anchor to Facts; record one when you observe something worth preserving or a decision needs the evidence — never as a completion quota.
  - **Decision**: Irreversible technical choices and architectural trade-offs (why choose Option A over Option B, why introduce a dependency, system boundaries). Load-bearing claims must be grounded in Facts (`evidenced-by`), rejecting arbitrary assumptions.
  - **Task**: The minimal verifiable delivery slice derived from a Decision. Follow verification-first/red-green cycles; when the delivery surfaces a load-bearing observation, record it as a Fact to close the causal loop.
  - **Causal Spiral**: Objective observation forms a **Fact** → Fact grounds a **Decision** → Decision derives a **Task** → Task produces a new **Fact** to verify hypotheses and establish a new baseline.
- Casual mentions in prose do not replace canonical Facts, Decisions, or Relations.

## Relation Rules

- Use canonical IDs to establish causal links:
  - `derives`: Decision directly spawns a Task;
  - `evidenced-by`: Fact supports a Decision Claim;
  - `relates`: Connection between tasks or artifacts identified later;
  - `refines`: Evolution or amendment from Decision to Decision.

## Write Coordination

- Lifecycle transitions, lease acquisitions, and relations must be written via `ha` commands.
- Long-term repository documentation synchronizes via doc-sync, relative to `harness/`.
- Ephemeral state under `.harness/` is local-only and must not be committed.

## Harness CLI (software/coding)

- **General Software Engineering Principles**:
  1. **Verification-First / TDD**: When fixing bugs, first write or identify a reproducing test (observe red), then implement until green; when adding features, write targeted tests and never ship unverified code.
  2. **Minimal Blast Radius**: Choose the simplest implementation satisfying current requirements; avoid speculative abstractions and unneeded configurations; fewer lines of code with equal correctness is always preferred, and deletions count equally to additions.
  3. **Zero Unintended Regressions**: Restrict modifications to paths directly required by the task, preserving existing project functionality and existing tests.
- **The Software Delivery Golden Path**:
  1. **Create Task**: `ha task create --title "..."` to define scope, goals, and acceptance criteria;
  2. **Acquire Lease**: `ha task start <task-id>` to acquire an exclusive Lease, protecting execution and avoiding concurrent collisions;
  3. **Implement & Ground Truth**: Implement code and tests in an isolated environment; use `ha fact record --statement "..." --source "..." --task <task-id>` to capture key test evidence;
  4. **Closeout Synthesis**: `closeout.md` is materialized by task create as a fixed skeleton (Summary, What Changed, Verification, Residual Risk, Same Mechanism Elsewhere); rewrite each section in place. Submit reads the prose and never compares headings or rejects retained prompt sentences. A public PR delivery must also author `## 摘要` and `## 验证`; `ha task submit` and the PR body renderer call the same judgment, and a missing section is refused at submit with the closeout path. What Changed lists the behavior changes and feeds the PR body directly; in the last section, state the underlying mechanism, where you searched for it, and what you found. **Critical: execution records assimilate directly from closeout.md**; never update artifacts while leaving closeout stale;
     `ha task submit <task-id>` accepts the current task-package documents and artifacts before freezing the delivery. Summary does not select or pin files.
  5. **Submit & Independent Review**: `ha task submit <task-id>` submits deliverables; an independent reviewer verifies code and test evidence against ground truth (self-review is rejected with `actor_unauthorized`); settle after completion gates pass via `ha task complete <task-id>`. Low risk tier is the exception: its effective tier closes the review and consent gates by design, and the submit and complete receipts print `review waived: effective risk tier low (declared by <actor>)`. A repository risk floor may raise a declared low tier, and the notice only appears while the effective tier stays low;
  6. **Dynamic Inspection**: The CLI is fully self-describing; always run `ha <command> --help` or `ha capabilities` to verify current grammar, and never memorize static command sequences.

## Repository Scaffolds

Repository Harness layout and responsibilities:
- Settings (read with `ha settings read`): Repository governance configuration baseline.
- `harness/tasks/`: Task packages collection. Each package manages its own `task_plan.md`, `closeout.md`, and artifacts.
- `harness/decisions/`: Architectural and design decision ledger (recording context, choices, rejected alternatives, and evidence).
- `harness/facts/`: Objective evidence ledger (immutable records of ground-truth measurements).
- `harness/context/`: Long-term project knowledge (`architecture/`, `development/`, `integrations/`, `research/`).
- `harness/governance/standards/`: Team and project engineering standards.
- Read each directory's own README; do not duplicate rules here.

## Architecture-aware Changes

- Before large-scale refactoring or exploration, check architectural guidance under `harness/context/architecture/`.
- Respect declared layering boundaries and dependency directions; do not violate architectural contracts or fabricate unnecessary layers.

## Governance Routing

Follow project governance standards during delivery and collaboration:
- Repository change and branch management: follow `harness/governance/standards/repository-governance.md` and repository conventions.
- Technical decisions: follow `harness/governance/standards/decision-writing.md`, documenting Why and Why-Not.
- Team-specific standards: consult documents under `harness/governance/standards/` or the project root README.

## Script Discovery

- Use `ha script list` and `ha script inspect <id>` to inspect declared automation scripts.
- Run scripts only when inspection explicitly reports execution as available.

## Repository Specifics

Repository-specific rules may be added here after explicit diagnosis; the deterministic base and vertical overlay above remain unchanged.

### test-anything rules

- Follow [CONTRIBUTING.md](CONTRIBUTING.md): commit and pull request format, project structure, code rules, tests and dependency policy. It is the authority for repository conventions.
- Before adding a dependency, search crates.io, lib.rs and GitHub and record the comparison as CONTRIBUTING.md requires. If you cannot search, stop and report which library you need and why.
- For a task-bound delivery, write the pull request description to `artifacts/pr-body.md` in the task package, using the bilingual sections from `.github/PULL_REQUEST_TEMPLATE.md`. The description is published as written, so it must not contain local paths, user names, machine names or secrets.
- On this project, build and test on Windows with the GNU toolchain when the MSVC linker is unavailable; CI covers MSVC, Linux and macOS.
