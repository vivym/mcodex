# Mcodex Smoke Test Matrix Design

This document defines a smoke-test matrix for the recent mcodex account-pool,
product-identity, startup, quota, runtime-lease, observability, and installer
work.

It is intentionally a validation design, not an implementation plan. The goal
is to make post-merge and pre-release validation systematic without turning
smoke tests into a duplicate of the full Rust workspace test suite.

It is an additive follow-up to:

- `docs/superpowers/specs/2026-04-10-multi-account-pool-design.md`
- `docs/superpowers/specs/2026-04-16-mcodex-product-identity-design.md`
- `docs/superpowers/specs/2026-04-16-remote-account-pool-contract-v0-design.md`
- `docs/superpowers/specs/2026-04-17-account-pool-observability-design.md`
- `docs/superpowers/specs/2026-04-18-cli-account-pool-observability-design.md`
- `docs/superpowers/specs/2026-04-18-account-pool-quota-aware-selection-design.md`
- `docs/superpowers/specs/2026-04-18-runtime-lease-authority-for-subagents-design.md`
- `docs/superpowers/specs/2026-04-19-single-pool-startup-fallback-and-default-pool-selection-design.md`
- `docs/superpowers/specs/2026-04-20-mcodex-cli-without-npm-design.md`

## Summary

The recommended direction is:

- keep full `cargo test` as the exhaustive correctness suite
- add a separate smoke-test matrix that validates real product entrypoints and
  cross-spec integration paths
- split smoke coverage into:
  - P0 manual smoke checks that can be run immediately on a local machine
  - P1 automated smoke checks that can later become `just smoke-mcodex-*`
    commands
- organize the matrix by entrypoint, state scenario, and authority source
  rather than by spec file
- make each smoke row explicit about the binary under test, fixture source, and
  observable pass/fail marker
- reserve remote-backend smoke coverage from the beginning through a fake
  backend contract, without pretending a production remote pool is already
  available

This gives mcodex a practical release gate for the flows that normal unit and
integration tests do not fully exercise: wrapper behavior, home isolation,
startup UX, pool default persistence, account-pool observability, runtime lease
inheritance, quota-aware selection, and installer wiring.

## Goals

- Verify the user-visible behavior of recently added account-pool features
  through real CLI, TUI, app-server, runtime, and installer entrypoints.
- Catch cross-spec regressions that may not be obvious from isolated crate
  tests.
- Keep the first smoke pass cheap enough to run during local development and
  before internal use.
- Provide a path to gradual automation without requiring a large harness before
  the first useful smoke run.
- Preserve compatibility with a future remote account-pool backend.
- Keep the validation workflow friendly to upstream merges by testing through
  narrow public entrypoints instead of relying on fork-specific internals where
  avoidable.

## Non-Goals

- Do not replace full `cargo test`.
- Do not attempt to exhaustively validate every table row or selection-policy
  branch.
- Do not burn real account quota to test quota exhaustion.
- Do not require a production remote account-pool service.
- Do not make every smoke check mandatory for every commit.
- Do not add a broad end-to-end harness that becomes harder to maintain than
  the product feature itself.

## Problem Statement

Recent mcodex work changed several layers at once:

- runtime home identity moved to `MCODEX_HOME` and `~/.mcodex`
- startup selection gained single-pool fallback and a state-backed default pool
- multi-pool-without-default now has its own pooled access condition
- CLI and app-server observability expose richer startup, lease, and quota facts
- runtime lease authority became the boundary for parent and subagent access
- quota-aware selection can change which account should be used
- installer and wrapper work is moving mcodex away from the upstream npm launch
  path

Each piece has unit or integration coverage, but users experience these pieces
as one startup and runtime path. A full workspace test run can still pass while
a real local launch does the wrong thing, for example:

- reads `CODEX_HOME` or upstream `~/.codex` instead of `MCODEX_HOME`
- falls back to the ChatGPT login page despite visible pooled access
- requires `-c 'accounts.default_pool="..."'` even when a single local pool is
  available
- hides the reason a multi-pool home is paused
- lets app-server, CLI, and TUI disagree about the effective pool
- lets subagents bypass or duplicate runtime lease ownership
- keeps choosing an account that quota state has already blocked

The missing validation layer is a smoke matrix that proves these entrypoints
still compose after merges and before local or internal distribution.

## Approaches Considered

### Approach A: Smoke each spec document independently

Under this approach every recent spec would own a separate smoke checklist.

Pros:

- Easy to trace checklist items back to their design documents
- Simple to delegate by document

Cons:

- Repeats the same entrypoints many times
- Does not naturally catch cross-spec integration bugs
- Encourages documentation-shaped tests rather than product-shaped tests
- Becomes stale when several specs converge on the same CLI or TUI surface

This approach is rejected.

### Approach B: Smoke only user-facing entrypoints

Under this approach the smoke suite would be organized only around CLI, TUI,
app-server, runtime, and installer entrypoints.

Pros:

- Closest to the real user experience
- Keeps the checklist understandable
- Avoids retesting internal implementation details

Cons:

- Can miss authority-source bugs where two entrypoints read different state
- Can under-test quota and lease state combinations
- Does not explicitly reserve space for future remote-pool compatibility

This approach is useful but incomplete.

### Approach C: Matrix by entrypoint, state scenario, and authority source

Under this approach each smoke row names:

- the product entrypoint being exercised
- the account-pool state scenario
- the authority source expected to decide the result
- whether the row is P0 manual, P1 automated, or future remote coverage

Pros:

- Tests product behavior instead of spec files
- Still makes state and authority-source coverage explicit
- Naturally exposes CLI/TUI/app-server disagreements
- Leaves a stable slot for remote backend smoke coverage
- Can start as a manual runbook and evolve into automation

This approach is recommended.

## Recommended Design

### 1. Use three matrix dimensions

The smoke matrix should be built from three dimensions.

Entrypoints:

| Entrypoint | Purpose |
| --- | --- |
| `mcodex` wrapper or release binary | Validate product identity, argument forwarding, exit codes, and home resolution |
| CLI `accounts` commands | Validate startup, default-pool, pool detail, diagnostics, and events output |
| TUI startup | Validate whether users land in usable pooled access, paused pooled access, or login |
| Core runtime turn | Validate that a pooled account can run a normal turn |
| Subagent runtime | Validate runtime lease authority inheritance |
| App-server v2 | Validate startup snapshots, default mutations, and account-pool observability RPCs |
| Installer | Validate install root, wrapper replacement, metadata, and local execution |

State scenarios:

| Scenario | Risk Covered |
| --- | --- |
| Empty home | Product should not invent pooled access |
| Single pool, no default | Startup should use the single-pool fallback |
| Multiple pools, no default | Startup should pause with a pooled-access explanation |
| Valid persisted default | Startup should use the state-backed default across restarts |
| Valid config default | Config should outrank persisted state |
| Invalid persisted default | Startup should explain the repair path |
| Invalid config default | Startup should not silently fall back to another pool |
| Startup suppressed | Startup should pause pooled access without losing facts |
| Account busy | A second instance should not steal another active lease |
| Quota exhausted | Selection should skip the blocked account |
| Quota near exhausted | Selection and switching should respect damping and safety windows |
| Parent plus subagent | Child sessions should not allocate outside runtime lease authority |

Authority sources:

| Source | Smoke Strategy |
| --- | --- |
| Local SQLite | Covered by P0 and P1 |
| `config.toml` | Covered by P0 and P1 |
| Runtime lease authority | Covered by P1 and a small P0 runtime check |
| App-server startup snapshot | Covered by P1 |
| Fake remote backend | Covered by future P1 contract smoke |
| Real account | Covered only by minimal P0 launch checks, not quota exhaustion |

### 2. Make the tested binary explicit

Every smoke run must record which executable is under test. The smoke matrix
should not assume that `mcodex` on `PATH` is the build that was just produced.

Each run should record:

- `MCODEX_BIN`: absolute path to the executable used for product-entrypoint
  checks
- `which mcodex`, when testing the installed wrapper
- `mcodex --version` output
- the git commit SHA for local builds
- whether the run is exercising:
  - direct debug binary, for quick development checks
  - direct release binary, for local daily-driver validation
  - installed wrapper, for installer and distribution validation

The P0 runbook may use a direct local binary for early validation, but installer
and wrapper rows must use the installed wrapper. If a row is meant to validate
the product launch path, prefer `target/release/mcodex` or the installed
wrapper over `target/debug/codex`.

The direct binary and installed wrapper should be separate matrix rows because
they fail in different ways:

- the direct binary validates compiled product behavior
- the wrapper validates PATH resolution, argument forwarding, exit-code
  forwarding, install-root selection, and packaged runtime assumptions

### 3. Define fixture and seed strategy

Each smoke row must say how state is created. "Seed one local pool/account" is
not specific enough to be executable.

Fixture classes:

| Fixture Class | Use For | Allowed In P0 | Allowed In P1 |
| --- | --- | --- | --- |
| Empty isolated home | Identity and no-account startup | yes | yes |
| Real account home | Minimal launch and status checks only | yes, explicitly | no by default |
| Local state fixture | Pools, defaults, leases, quota, events, diagnostics | yes, via documented helper | yes |
| Mock runtime fixture | Runtime turns, quota pressure, subagent lease assertions | no by default | yes |
| Fake app-server client fixture | App-server v2 RPC checks | no by default | yes |
| Fake remote backend | Remote contract shape and authority behavior | no | deferred P1 |
| Installer fixture | Local install root and wrapper checks | yes | yes |

The first implementation slice should provide one canonical local state fixture
helper that can create:

- one pool with one fake ChatGPT account
- two pools with one fake account each
- a persisted default pool
- a conflicting config default
- an invalid persisted default
- an invalid config default
- a suppressed startup state
- a busy lease row owned by another holder
- quota rows for exhausted and near-exhausted cases
- representative diagnostics and events rows

The fixture should only write under the isolated `MCODEX_HOME`. It should not
write to a developer's real `~/.mcodex` or `~/.codex`.

Until that helper exists, P0 rows that require seeded pool state are runbook
items, not guaranteed one-command checks. The runbook must point to the exact
fixture command or manual setup used for that run.

### 4. Define a P0 manual smoke runbook

P0 should be runnable immediately on a developer machine using an isolated
`MCODEX_HOME`. It should prefer a local release binary or installed wrapper
over direct internal helper binaries when the goal is product validation.

Initial P0 rows:

| ID | Scenario | Action | Expected Result |
| --- | --- | --- | --- |
| P0-01 | Isolated home | Run `MCODEX_HOME=<tmp> "$MCODEX_BIN" accounts status --json` | The command uses the isolated home and does not read upstream `~/.codex` |
| P0-02 | Empty home startup | Run `MCODEX_HOME=<tmp> mcodex` | Startup reaches the normal unauthenticated or no-account surface, not a pooled state |
| P0-03 | Single pool, no default | Seed one local pool/account and launch TUI | Startup uses pooled access without `-c accounts.default_pool=...` |
| P0-04 | Multiple pools, no default | Seed two visible pools and launch TUI | Startup shows the dedicated pooled access paused/default-required surface |
| P0-05 | Set default pool | Run `mcodex accounts pool default set <pool>` | The persisted default is written and `accounts status` reports its source |
| P0-06 | Clear default pool | Run `mcodex accounts pool default clear` | Multi-pool startup returns to default-required state |
| P0-07 | Observability CLI | Run status, pool show, diagnostics, and events commands | Output explains startup, lease, and quota state without exposing credentials |
| P0-08 | Config default precedence | Set config default and a different persisted default | Effective pool comes from config, with clear source reporting |
| P0-09 | Home override conflict | Run with both `MCODEX_HOME=<a>` and `CODEX_HOME=<b>` | Runtime state comes from `<a>` and does not read `<b>` |
| P0-10 | Subagent lease observation | Start a pooled parent session and spawn a subagent, then inspect events or lease rows | The child stays under the same runtime authority; no second random account allocation is observed |
| P0-11 | Local installer wrapper | Install into an isolated root and run `mcodex --version` plus a CLI command | Wrapper forwards arguments, preserves exit code, and uses mcodex home identity |

P0 should not attempt to exhaust a real account quota. Quota and switch behavior
should be validated through seeded state or fake backends.

### 5. Define P1 automated smoke groups

P1 should turn stable P0 expectations into repeatable commands over time.

Recommended command groups:

| Command | Coverage |
| --- | --- |
| `just smoke-mcodex-local` | Isolated home, local SQLite startup selection, default set/clear, status JSON |
| `just smoke-mcodex-cli` | CLI grammar and output smoke for status, default, show, diagnostics, and events |
| `just smoke-mcodex-app-server` | App-server account-pool read, accounts list, diagnostics, events, and default mutation RPCs |
| `just smoke-mcodex-runtime` | Pooled runtime turn, lease renewal, release, and parent/subagent lease inheritance |
| `just smoke-mcodex-quota` | Seeded quota exhausted, near-exhausted, reprobe, and damping scenarios |
| `just smoke-mcodex-installer` | Local install root, wrapper replacement, metadata, and PATH forwarding |
| `just smoke-mcodex-all` | Release-gate aggregate for internal distribution or main-branch merge validation |

The first automated slice should prioritize `smoke-mcodex-local`,
`smoke-mcodex-cli`, and `smoke-mcodex-app-server`. These give the best return
without requiring real TUI automation or production account access.

For app-server smoke, the first P1 implementation should use the existing
app-server test client or in-process harness. It should not require a real UI
client, remote browser session, or production remote server.

For runtime and subagent smoke, the first P1 implementation should use core test
support and mock model responses. Real account P0 checks may validate that the
interactive path launches, but runtime lease inheritance and quota behavior
should be asserted through fake or seeded data.

### 6. Keep P0 and P1 data setup isolated

Smoke checks must not mutate a developer's real account data by default.

Rules:

- Always set `MCODEX_HOME` to a temporary or explicitly named smoke directory.
- Do not rely on `CODEX_HOME` as an active runtime override.
- If a test process spawns child mcodex processes, pass `MCODEX_HOME` through
  deliberately and clear inherited `CODEX_HOME` when the product path should be
  isolated.
- Include at least one explicit conflict row where `MCODEX_HOME` and
  `CODEX_HOME` are both set to different directories.
- Use seeded local state for pool membership, quota rows, and lease rows.
- Use fake tokens or fake credentials for local startup and selection smoke
  unless the row explicitly says it is a real-account check.
- Real-account P0 checks should validate launch and observability only; they
  must not intentionally drive usage limits.

### 7. Treat TUI smoke as a layered problem

TUI startup is important, but it should not block the first automated smoke
slice.

P0 should manually verify the TUI startup surfaces:

- empty home
- single pool fallback
- multi-pool default required
- pooled access paused

The manual TUI row must define observable markers before execution. At minimum,
capture:

- whether the ChatGPT login screen appears
- whether the pooled access paused/default-required title appears
- whether `accounts status --json` for the same `MCODEX_HOME` reports
  `singleVisiblePool`, `multiplePoolsRequireDefault`,
  `invalidExplicitDefault`, or another expected startup code
- a screenshot or terminal capture for any failure

P1 should first validate the same states through shared startup-resolution
objects, CLI output, and app-server startup snapshots. Headless TUI smoke can
come later and should be limited to a few snapshot-like startup surfaces rather
than full interactive sessions.

### 8. Reserve remote backend coverage without depending on production remote

The matrix should include remote rows from the beginning, but those rows should
use a fake backend until a production remote pool exists.

Remote-compatible smoke should verify:

- backend-neutral inventory shape
- startup snapshot shape
- default set/clear behavior when supported
- read-only observability shape
- error behavior when remote authority reports paused, drained, quota-blocked,
  or unavailable

Local SQLite must not become the source of truth for remote-only facts. If a
remote row cannot provide authoritative quota, pause, or drain information, the
expected result should expose that absence rather than invent synthetic facts.

### 9. Keep smoke output operator-oriented

Every smoke row should record:

- command or action
- binary path and version output
- isolated home path
- setup method
- fixture class
- expected source of truth
- expected user-facing result
- exact output marker or state marker used to decide pass/fail
- whether credentials are real, fake, or absent
- whether the row is manual, automated, or deferred

This makes failures actionable. For example, "TUI showed login" is not enough;
the smoke output should also show whether startup resolution saw zero pools,
one pool, multiple pools without default, or an invalid configured default.

## Initial Matrix

| ID | Entry Point | State Scenario | Authority Source | Level | Expected Result |
| --- | --- | --- | --- | --- | --- |
| M-01 | CLI status | Empty home | `MCODEX_HOME` + local SQLite | P0/P1 | Reports no pooled startup without reading upstream home |
| M-02 | CLI status | Conflicting `MCODEX_HOME` and `CODEX_HOME` | Product home resolver | P0/P1 | Uses `MCODEX_HOME`; does not read `CODEX_HOME` |
| M-03 | Direct binary identity | Local build | Release/debug binary | P0/P1 | Version and product identity match the intended build |
| M-04 | Installed wrapper identity | Local install root | Wrapper script | P0/P1 | Wrapper forwards args and exit codes to the intended binary |
| M-05 | TUI startup | Empty home | `MCODEX_HOME` | P0 | Shows normal no-account/login path |
| M-06 | CLI status | Single pool, no default | Local SQLite fixture | P0/P1 | Reports single-pool fallback source |
| M-07 | TUI startup | Single pool, no default | Local SQLite fixture | P0 | Does not show ChatGPT login; status marker is single-pool fallback |
| M-08 | CLI status | Multiple pools, no default | Local SQLite fixture | P0/P1 | Reports `multiplePoolsRequireDefault` |
| M-09 | TUI startup | Multiple pools, no default | Startup snapshot | P0 | Shows pooled access paused/default-required surface |
| M-10 | CLI default set | Multiple pools | Local SQLite fixture | P0/P1 | Persists default pool and reports source |
| M-11 | CLI default clear | Multiple pools | Local SQLite fixture | P0/P1 | Clears persisted default and returns to default-required state |
| M-12 | CLI status | Config default outranks persisted default | Config + local SQLite fixture | P0/P1 | Effective pool source is config |
| M-13 | CLI status | Invalid config default | Config + local SQLite fixture | P0/P1 | Reports invalid explicit default without fallback |
| M-14 | CLI status | Invalid persisted default | Local SQLite fixture | P0/P1 | Reports invalid persisted default with set/clear repair path |
| M-15 | App-server | Startup snapshot | Local backend fixture | P1 | Snapshot matches CLI startup facts |
| M-16 | App-server | Default set/clear | Local backend fixture | P1 | Mutations update startup snapshot and notify clients |
| M-17 | CLI observability | Pool with lease and quota rows | Local SQLite fixture | P0/P1 | Status/show/diagnostics/events expose facts without tokens |
| M-18 | Runtime turn | Pooled account | Runtime lease authority fixture | P1 | Turn uses leased account and releases/renews correctly |
| M-19 | Subagent | Parent has active pooled lease | Runtime lease authority fixture | P0/P1 | Child inherits authority; observed events or lease rows show no independent random allocation |
| M-20 | Runtime selection | Account busy in another holder | Local SQLite lease fixture | P1 | Selection skips account held by another instance |
| M-21 | Runtime selection | Quota exhausted | Seeded quota fixture | P1 | Selection skips blocked account |
| M-22 | Runtime selection | Quota near exhausted | Seeded quota fixture | P1 | Damping prevents per-turn account churn |
| M-23 | Installer | Local install root | Install scripts + wrapper | P0/P1 | Wrapper forwards args, exit code, and mcodex home identity |
| M-24 | Fake remote | Remote inventory and startup snapshot | Fake backend | Deferred P1 | Remote-shaped facts flow through same startup and observability surfaces |

## Execution Policy

Suggested local use:

1. Run P0 smoke after large merges into `main` or before using a fresh local
   build as the daily driver.
2. Run `cargo test` for code correctness separately.
3. Promote a P0 row to P1 only after it has a stable setup path and does not
   require real account quota or fragile human interaction.
4. Run `just smoke-mcodex-all` only for internal release candidates or major
   branch consolidation.

Suggested automation order:

1. `smoke-mcodex-local`
2. `smoke-mcodex-cli`
3. `smoke-mcodex-app-server`
4. `smoke-mcodex-runtime`
5. `smoke-mcodex-quota`
6. `smoke-mcodex-installer`
7. fake remote contract smoke

## Testing Strategy

Because this document defines validation structure, not product code, the
implementation tests belong to the future smoke harness.

The smoke harness should use:

- shell-level assertions for wrapper, binary, and installer checks
- seeded `MCODEX_HOME` directories for startup and CLI checks
- local SQLite seed helpers for account, pool, lease, quota, diagnostics, and
  event rows
- app-server test clients for v2 account-pool RPC checks
- core test support for pooled runtime and subagent authority checks
- fake remote backend implementations for remote-contract rows

The harness should avoid:

- mutating a real `~/.mcodex` unless explicitly requested
- relying on upstream `~/.codex`
- treating `CODEX_HOME` as a fallback for active runtime state
- using real account quota exhaustion as a test mechanism
- running full workspace tests as part of smoke commands

## Acceptance Criteria

- A developer can use the P0 matrix to manually validate a local mcodex build
  without touching their official Codex home.
- Every smoke row identifies the binary under test, fixture class, authority
  source, and pass/fail marker.
- The matrix can answer why startup showed login, pooled access, or pooled
  access paused.
- The matrix validates default-pool persistence and config precedence.
- The matrix validates `MCODEX_HOME` precedence when `CODEX_HOME` is also set.
- The matrix validates that CLI, TUI, and app-server agree on startup facts.
- The matrix validates that subagents stay under runtime lease authority.
- The matrix validates quota-blocked selection without consuming real quota.
- The matrix validates wrapper and installer identity for local use.
- The matrix has explicit deferred rows for remote backend smoke without
  coupling the local implementation to fake remote-only behavior.

## Risks And Mitigations

### Risk: smoke tests become another full test suite

Mitigation: keep smoke rows focused on entrypoints and cross-spec behavior.
Detailed branch coverage remains in unit and integration tests.

### Risk: manual P0 checks drift

Mitigation: store the P0 runbook in the same matrix and promote stable rows to
P1 automation as soon as setup becomes deterministic.

### Risk: smoke mutates real account data

Mitigation: require explicit `MCODEX_HOME` isolation and fake seeded data for
default smoke paths.

### Risk: smoke validates the wrong binary

Mitigation: require each run to record `MCODEX_BIN`, `which mcodex`, version
output, and commit SHA where applicable. Keep direct-binary and wrapper smoke
as separate rows.

### Risk: remote smoke is blocked by lack of production remote

Mitigation: use fake backend contract smoke and clearly mark production remote
rows as deferred.

### Risk: TUI smoke becomes flaky

Mitigation: validate most startup facts through shared resolution, CLI, and
app-server snapshots first. Add only a small headless TUI surface later.

## Decision

Adopt Approach C: a two-layer smoke strategy organized by entrypoint, state
scenario, and authority source.

The immediate next step is to write an implementation plan for the first smoke
slice: a P0 manual runbook plus the smallest useful automated local/CLI smoke
commands. Remote backend, installer release packaging, and headless TUI smoke
should remain explicit follow-up rows unless the first slice needs them to
explain a current failure.
