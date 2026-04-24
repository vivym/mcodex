# Provenance Architecture

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

This document defines the mcodex-side kernel for tracing how code and execution
state evolve during Codex activity.

The first release should not be framed as a diff feature, a `git blame`
feature, or an extension of `thread/read`. It should be framed as a local trace
kernel with scoped hash-chained ledgers:

- `GlobalExecution`: what Codex executed, when, under which thread/turn/tool/process
- `WorkspaceCustody`: what workspace state, file state, hunk lineage, and
  revision alias facts were observed
- `AccessAudit`: which raw/blob reads were disclosed locally

The primary job of mcodex is to produce durable, queryable, exportable local
trace facts. Forgeloop remains the online product for organization-wide review,
incident analysis, entitlement, retention, and postmortem workflows.

## Summary

Build a Git-first local trace kernel with these properties:

1. Record execution lifecycle facts and code provenance facts as separate but
   explicitly linked journals.
2. Enforce a mandatory mutator boundary for every file-writing execution path,
   instead of relying on turn-scoped net diffs.
3. Use anchored code coordinates for queries:
   - live workspace queries: `workspace stream/head selector + path + line/range`
   - recorded-state queries: `workspace_state_id + path + line/range`
   - historical queries: `git commit/tree/alias selector + path + line/range`
4. Treat turn-level and workspace-level summaries as derived read models, not
   the primary truth source.
5. Capture synchronous chain-of-custody facts first, then run heavier hunk and
   range indexing asynchronously when needed.
6. Make selector status, freshness, certainty, coverage, and indexing orthogonal
   result dimensions instead of collapsing them into one status.
7. Keep v1 Git-first for line-level provenance. Non-Git workspaces may still
   record execution and file-level facts, but they are not first-class range
   provenance targets.
8. Expose a stable export contract that later maps cleanly into Forgeloop's
   execution and evidence plane.

Recommended implementation:

- add `codex-provenance` for models and lineage/projector logic
- add `codex-provenance-store` for a dedicated provenance journal and query
  index, backed by a separate local SQLite database
- add a `MutationSupervisor` integration layer in `codex-core`
- add provenance-specific app-server v2 APIs instead of extending `thread/read`

## Why This Is Not a Diff Feature

The target question is not "what changed in this turn?" It is:

- which request initiated the logic
- which assistant turn and tool activity produced it
- which later turns reshaped it
- whether the attribution is exact, stale, ambiguous, partial, or unavailable
- how the current or historical code range maps back to those observed facts

A net turn diff is a useful presentation artifact, but it is not strong enough
to preserve chain-of-custody across shell writes, long-lived processes, bulk
rewrites, branch switches, or external drift.

## Goals

- Answer: "How did the logic at this anchored code coordinate get here?"
- Preserve chain-of-custody for Codex-driven file mutations without pretending
  certainty where the recorder cannot prove it.
- Record enough execution context for future systems to reconstruct work traces
  without reverse-engineering rollouts.
- Support exact historical lookup for Git-backed workspaces when a selected
  revision can be mapped to recorded workspace state.
- Provide export-safe local facts for future Forgeloop ingestion.
- Keep Codex language-agnostic; symbol, issue, incident, and actor semantics
  remain outside the kernel.

## Non-Goals

- Do not make `git blame` the primary provenance mechanism.
- Do not require symbol parsing inside Codex v1.
- Do not build the online postmortem platform inside mcodex.
- Do not backfill arbitrary historical repository history or historical rollouts.
- Do not make incident, issue, PR, employee, or performance objects first-class
  provenance keys inside mcodex.
- Do not promise line-level provenance for non-Git workspaces in v1.
- Do not implement SaaS upload, tenant authorization, retention, or raw-content
  approval inside mcodex.

## Relationship to Forgeloop

Forgeloop is the future online consumer of this kernel's facts.

### mcodex owns

- local capture of execution and code provenance facts
- local workspace and stream identity
- workspace state capture and revision alias mapping
- hunk lineage and range projection
- local provenance query APIs
- local blob manifests and export cursors

### Forgeloop owns

- SaaS tenant and repo enrollment
- actor identity and organization mapping
- ExecutionPackage, RunSession, Review, Release, Incident, and related business
  objects
- cross-user repo aggregation
- policy, raw-content entitlement, retention, legal hold, and cloud blob
  lifecycle
- online review, postmortem, and learning workflows

## Design Principles

1. **Separate execution facts from code facts.** Code provenance without
   execution provenance leaves future systems guessing how work happened.
2. **Capture first, project second.** Chain-of-custody facts must be durable
   before expensive line-level indexing finishes.
3. **Git-first for exact range provenance.** Git is the historical coordinate
   system for v1.
4. **Derived summaries are not the truth source.** Turn summaries, file
   summaries, and collapsed range summaries are projections.
5. **Never guess across ambiguous identity or drift boundaries.**
6. **Stable export contract beats convenient local-only shapes.**

## Current Reality

Useful existing building blocks:

- turn-scoped diff tracking in `core/src/turn_diff_tracker.rs`
- tool event emission in `core/src/tools/events.rs`
- shell, `apply_patch`, `js_repl`, and unified exec handlers in `codex-core`
- ghost snapshot and git helper machinery in `codex-git-utils`
- existing app-server v2 infrastructure

Important current limitations:

1. `TurnDiffTracker` is turn-scoped and effectively patch-seeded, not a shared
   write barrier for all file mutations.
2. `shell`, `js_repl`, and unified exec do not currently flow through one common
   mutation observation path.
3. ghost snapshots are undo-oriented Git snapshots, not provenance baselines.
4. current local SQLite state is rollout/thread metadata oriented, not a
   replayable provenance journal.
5. current app-server read surfaces are thread-oriented, not provenance-oriented.

These limitations mean the optimal solution is a new trace subsystem, not a
small extension of the current diff and thread read models.

## V1 Scope

### Supported in v1

- Git-backed workspaces
- exact and bulk file mutations caused by Codex tools
- long-lived process sessions when mutations remain attributable to a process
  activity or child mutation interval
- line-level provenance queries for Git-backed workspaces
- local export of execution and code provenance facts

### Limited in v1

- non-Git workspaces may record execution and file-level mutation facts
- non-Git workspaces are not guaranteed line-level or historical range
  provenance
- manifest-only states are not queryable as exact line-range provenance targets

### Excluded from v1

- symbol-aware queries
- cross-repo semantic equivalence
- SaaS upload and online access control

## Architecture

The trace kernel is defined by six contracts. These contracts are more important
than the crate split because they define which data is authoritative and which
data is derived.

1. capture and mutation observation contract
2. ledger contract
3. state and continuity contract
4. projection contract
5. query contract
6. blob and export contract

