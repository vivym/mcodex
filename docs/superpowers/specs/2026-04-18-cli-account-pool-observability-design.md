# CLI Account Pool Observability Design

This document defines the first CLI consumer slice for the pooled-account
observability contract that already exists behind the local backend and
app-server v2.

It is intentionally scoped to a read-only operator-facing CLI surface. It does
not add new pooled state, it does not add app-server protocol changes, it does
not implement a remote backend, and it does not add write-side operator
commands such as pause, resume, or drain.

## Summary

The recommended direction is:

- add a small read-only operator CLI surface under `codex accounts`
- keep `codex accounts status` as the top-level overview command
- add focused drill-down commands:
  - `codex accounts pool show`
  - `codex accounts diagnostics`
  - `codex accounts events`
- make CLI consume the backend-neutral observability seam in
  `codex-account-pool`, rather than:
  - querying `StateRuntime` directly
  - calling app-server RPCs through a transport loop
- preserve the existing `codex accounts pool list|assign` command family and
  add `show` under that subtree rather than introducing a conflicting top-level
  `pool` read command
- keep the first text output concise and operator-oriented while preserving the
  complete response shape in `--json`

This gives the fork an immediately useful local operator/debug surface without
expanding the pooled runtime contract or increasing merge risk in state/core.

## Goals

- Improve day-to-day operator UX for pooled accounts.
- Make it easy to answer:
  - "what pool is active right now?"
  - "why is the pool degraded or blocked?"
  - "what happened recently?"
  - "which accounts are currently in the pool and what state are they in?"
- Reuse the existing pooled observability contract instead of growing another
  CLI-only read path.
- Keep the design compatible with a future remote backend by making CLI depend
  on the same backend-neutral seam that future consumers will use.
- Keep the command surface small enough that users can discover it from
  `codex accounts --help` without learning a separate tool.

## Non-Goals

- Do not change pooled allocation, lease, or proactive switch semantics.
- Do not add remote backend implementation or remote transport logic in this
  slice.
- Do not add new app-server RPCs or change the current observability wire
  contract.
- Do not add write-side control-plane commands such as pause, resume, drain, or
  pool-level mutation.
- Do not redesign `codex accounts current`; it may remain startup-selection
  focused in this slice.
- Do not add a TUI consumer in the same change.
- Do not invent synthetic quota, pause, or drain facts when the local backend
  still has no authoritative source for them.

## Constraints

- Upstream mergeability matters, so the implementation should stay in the CLI
  crate plus existing backend-neutral seams instead of changing pooled state or
  runtime ownership again.
- The current CLI already has a `codex accounts pool` subtree for control-plane
  operations (`list` and `assign`), so any new read view must fit without
  breaking that grammar.
- The existing observability contract intentionally returns `null` for fields
  that the local backend cannot fill authoritatively yet. The CLI must preserve
  that realism rule instead of papering over it with guessed values.
- The first operator slice should prioritize high-signal debugging and
  operational visibility rather than a large command matrix.

## Problem Statement

The pooled observability slice is now implemented below the CLI, but the local
operator experience still stops at startup-selection diagnostics and coarse
account management:

- `codex accounts status` explains startup selection and eligibility, but it
  does not show pooled observability summary, diagnostics status, or event
  history
- there is no CLI command that shows the current pool summary through the new
  observability seam
- there is no CLI command that lists the current diagnostics issues for a pool
- there is no CLI command that lists recent append-only pool events

That creates two practical problems:

1. users can operate the local pool, but they still have to infer why it is
   degraded or blocked
2. future CLI work would be tempted to re-query local state tables directly,
   which would duplicate semantics and make remote support harder later

The missing piece is not another ad hoc status string. It is a CLI surface that
consumes the already-shipped observability contract with stable command
boundaries and operator-oriented output.

## Approaches Considered

### Approach A: Query `StateRuntime` directly from CLI

Under this approach, CLI would read pooled observability facts straight from the
state layer and format them locally.

Pros:

- fastest implementation path
- avoids introducing new CLI wiring over existing seams

Cons:

- duplicates observability semantics outside the backend-neutral layer
- encourages more local-only branching when remote support arrives
- makes CLI a second owner of cursor/filter mapping and nullability behavior

This approach is rejected.

### Approach B: Consume the backend-neutral observability seam from CLI

Under this approach:

- CLI resolves the target pool
- CLI constructs a local `codex-account-pool` backend
- CLI reads summary/accounts/events/diagnostics through the observability seam
- CLI formats text and JSON output on top of those seam types

Pros:

- keeps one shared read contract below the CLI
- future remote support can plug in behind the same seam
- avoids pulling app-server transport into the local CLI path

Cons:

- requires a small new CLI read adapter layer
- still needs CLI-specific output view models and formatting

This is the recommended approach.

### Approach C: Call app-server `accountPool/*` RPCs from CLI

Under this approach, CLI would boot an app-server-style transport path and
consume the pooled observability RPCs directly.

Pros:

- strongest reuse of the app-server wire contract

Cons:

- adds unnecessary transport/process complexity to the local CLI path
- makes local command startup and testing heavier
- increases coupling between CLI and app-server runtime behavior

This approach is rejected.

## Recommended Design

### 1. Add one overview command and three drill-down commands

The first CLI observability slice should expose:

- `codex accounts status`
- `codex accounts pool show`
- `codex accounts diagnostics`
- `codex accounts events`

Command roles:

- `status`: concise overview and best first command
- `pool show`: operator view of current pool summary and current account rows
- `diagnostics`: focused explanation of current degraded/blocked state
- `events`: recent append-only history for time-ordered debugging

`codex accounts pool list|assign` remain unchanged. `show` is additive under the
existing subtree, avoiding a grammar conflict with the current `pool`
management commands.

### 2. Keep `status` concise and additive

`codex accounts status` already owns startup-selection and effective-pool
explanation. This slice should preserve that role and append a pooled
observability summary rather than replacing the existing diagnostic path.

The first text output should include:

- effective pool id and source
- preferred/predicted account ids when present
- suppression state
- startup eligibility summary
- pooled diagnostics status: `healthy`, `degraded`, or `blocked`
- compact summary counts:
  - total accounts
  - active leases
  - available accounts
  - leased accounts
- one high-signal issue summary when diagnostics are not healthy

`status` should not become a full dump of accounts, issues, and events. It
should point users to `diagnostics`, `events`, and `pool show` for detail.

### 3. Add `pool show` as the operational detail view

`codex accounts pool show` should present the current summary and account rows
for one pool.

Pool selection rules:

- if `--pool <POOL_ID>` is present, use it
- otherwise, resolve the current effective pool from the existing startup
  diagnostic path
- if neither exists, fail with an explicit message telling the user to pass
  `--pool <POOL_ID>`

First-pass parameters:

- `--pool <POOL_ID>`
- `--limit <N>`
- `--cursor <CURSOR>`
- `--json`

Text output should show:

- pool id
- backend kind
- refreshed timestamp
- summary counts
- account rows with these columns:
  - `accountId`
  - `kind`
  - `enabled`
  - `health`
  - `state`
  - `lease`
  - `eligible`
  - `preferred`

Less critical fields such as `backendAccountRef`, `statusReasonCode`,
`statusMessage`, and the full `selection` object may remain primarily JSON-only
in the first text implementation.

### 4. Add `diagnostics` as the current-state explanation view

`codex accounts diagnostics` should show why a pool is currently healthy,
degraded, or blocked.

First-pass parameters:

- `--pool <POOL_ID>`
- `--json`

Text output should include:

- top-level diagnostics status
- generated timestamp
- one row per issue with:
  - severity
  - reason code
  - message
  - account id when present
  - holder instance id when present
  - next relevant timestamp when present

This command is intentionally about current issues, not historical order. Users
who want chronology should move to `events`.

### 5. Add `events` as the chronological debugging view

`codex accounts events` should expose recent append-only pooled history with the
existing cursor model.

First-pass parameters:

- `--pool <POOL_ID>`
- `--account <ACCOUNT_ID>`
- `--type <EVENT_TYPE>` (repeatable)
- `--limit <N>`
- `--cursor <CURSOR>`
- `--json`

Text output should include:

- occurred timestamp
- event type
- account id when present
- reason code when present
- message

When another page exists, text output should print the next cursor explicitly so
the operator can request the next page without a separate pagination protocol.

`events --follow` and live streaming behavior are intentionally deferred.

### 6. Keep CLI dependent on the backend-neutral observability seam

The CLI should load config and construct a backend reader through
`codex-account-pool`, then consume:

- `read_pool`
- `list_accounts`
- `read_diagnostics`
- `list_events`

The CLI should not:

- duplicate SQL queries
- read observability tables directly
- call app-server methods through a transport client just to read local state

This keeps the local CLI aligned with the same seam a future remote backend can
implement.

### 7. Isolate read helpers and output formatting

The first implementation should avoid growing the existing accounts modules into
one large mixed control-plane/read-formatting file.

Recommended CLI additions under `codex-rs/cli/src/accounts/`:

- `observability.rs`
  - target pool resolution
  - local backend reader construction
  - command-level read helpers
- `observability_types.rs`
  - optional CLI-facing view models that decouple formatting from backend types
- `observability_output.rs`
  - text/json formatting for `status`, `pool show`, `diagnostics`, and `events`

If the implementation can stay clear without `observability_types.rs`, it may
be omitted, but the formatter should still remain separate from command parsing
and reader construction.

### 8. Preserve the contract realism rule in CLI output

The CLI must not turn nullable local observability fields into invented facts.

For local v1 specifically:

- `quota` may still be `null`
- paused/draining/near-exhausted/exhausted bucket counts may still be `null`
- some per-account operational details may remain unknown

Text mode may render these as `unknown` or omit them, but JSON output must
preserve the real nullable values and the CLI must not pretend the local
backend knows more than it does.

### 9. Sequence the implementation from pure drill-down to mixed overview

The recommended implementation order is:

1. `codex accounts pool show`
2. `codex accounts diagnostics`
3. `codex accounts events`
4. pooled observability summary integration into `codex accounts status`

This order keeps the first tasks focused on new read-only commands with clear
boundaries. `status` is intentionally last because it mixes existing startup
diagnostics with the new observability summary.

## Error Handling

The first slice should keep a small, explicit error model:

- no effective pool resolved and no `--pool` passed:
  - fail with: `no effective pool is configured; pass --pool <POOL_ID>`
- target pool not found:
  - propagate a clear pool-not-found error
- invalid cursor:
  - fail with an explicit cursor error
- empty event page:
  - return success with no rows
- diagnostics with no issues:
  - render `healthy` and no issue rows

Text rendering should prefer omission or `unknown` for nullable fields over
guessed operator conclusions.

## Testing Strategy

The first CLI slice should use three layers of tests.

### 1. Command parsing and output tests

Cover:

- `accounts pool show`
- `accounts diagnostics`
- `accounts events`
- additive `accounts status` pooled summary output
- coexistence with existing `accounts pool list|assign`

### 2. Read-path integration tests with seeded local state

Use temporary homes and seeded pooled state to verify:

- healthy pool summary
- degraded diagnostics with at least one issue
- blocked diagnostics
- event pagination through cursors
- `--pool` overriding the current effective pool
- explicit pool requirement when no effective pool can be resolved

### 3. Formatter-focused unit tests

Cover:

- nullable fields remain nullable in JSON
- text output does not invent synthetic facts
- event and diagnostics rows remain stable and readable
- next cursor rendering in `events` text output

## Acceptance Criteria

This design is complete when:

- `codex accounts status` shows additive pooled observability summary for the
  effective pool when one is available
- `codex accounts pool show` returns current summary plus account rows for one
  pool
- `codex accounts diagnostics` explains current pool issues through the
  observability seam
- `codex accounts events` exposes recent append-only history with cursor
  pagination
- CLI consumes the backend-neutral observability seam instead of direct local
  state queries
- existing `codex accounts pool list|assign` behavior remains intact

## Follow-On Work

- TUI consumption of the same observability seam
- richer doctor-style or summary-first operator surfaces
- remote backend implementation behind the same seam
- write-side control-plane commands such as pause, resume, and drain
- richer local quota/pause/drain facts once the backend has authoritative
  sources for them
