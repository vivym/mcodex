# Upstream Stable Sync Design

This document defines the process for syncing the `mcodex` fork to upstream
Codex stable releases, starting with upstream `rust-v0.122.0`.

The goal is to absorb upstream stable features and bug fixes without silently
regressing fork-specific product behavior.

## Summary

Use a two-stage stable checkpoint sync:

1. Merge upstream `rust-v0.121.0` into an internal checkpoint branch.
2. Restore the fork's core runtime contract at that checkpoint.
3. Merge upstream `rust-v0.122.0` from that checkpoint.
4. Restore the full release contract before merging back to `main`.

This is not a plan to support or release `mcodex` at `rust-v0.121.0`. The
`0.121` step exists only to reduce review and conflict complexity while moving
toward the real target, `rust-v0.122.0`.

## Goals

- Keep `mcodex` current with upstream stable releases.
- Maximize uptake of upstream features and bug fixes.
- Prevent silent regressions in `mcodex` identity, account-pool, lease-auth,
  and release/update/install behavior.
- Avoid resolving large conflicts with broad `ours` or `theirs` decisions.
- Keep fork-specific behavior expressed as explicit contracts rather than old
  implementation shapes.
- Produce a repeatable process for future upstream stable releases.

## Non-Goals

- Do not chase upstream `main` as the normal sync target.
- Do not release an intermediate `rust-v0.121.0`-based `mcodex`.
- Do not preserve obsolete fork implementation structure when upstream has
  introduced a better or required architecture.
- Do not split the sync by arbitrary modules such as "core first, TUI later"
  when upstream changes span those modules.
- Do not resolve generated schemas or lockfiles by hand as final content.

## Current Context

The fork shares history with `openai/codex`, so standard Git merge workflows
are viable. However, the current fork has significant changes across:

- product identity and home/env resolution
- account-pool state and policy
- pooled registration and lease-scoped auth
- app-server pooled APIs
- TUI pooled startup/status surfaces
- release, installer, and update distribution

A dry-run merge of upstream `rust-v0.122.0` shows broad conflicts in `core`,
`login`, `state`, `app-server`, `tui`, generated app-server schemas, Cargo
metadata, docs, and installer scripts. A dry-run merge of `rust-v0.121.0`
shows substantially fewer conflicts, so it is useful as an internal checkpoint.

## Recommended Workflow

### 1. Prepare upstream refs

Configure the upstream remote and fetch stable tags:

```bash
git remote add upstream https://github.com/openai/codex.git
git fetch upstream --tags
```

The normal target is the latest upstream stable tag, not `upstream/main`.
Prerelease tags may be inspected for context, but they should not become the
default fork sync baseline unless explicitly chosen.

### 2. Create the 0.121 checkpoint branch

Create an isolated worktree:

```bash
git worktree add .worktrees/sync-rust-v0.121.0-base \
  -b sync/rust-v0.121.0-base main
cd .worktrees/sync-rust-v0.121.0-base
git merge --no-ff rust-v0.121.0
```

Resolve conflicts using the conflict policy below. This checkpoint must compile
and satisfy the core runtime contract, but it is not a release candidate.

### 3. Create the 0.122 target branch

After the `0.121` checkpoint passes its gate, continue from it:

```bash
git switch -c sync/rust-v0.122.0
git merge --no-ff rust-v0.122.0
```

Resolve remaining conflicts and run the full final gate. Only this final branch
is eligible to merge back to `main`.

### 4. Merge back to main

Merge `sync/rust-v0.122.0` to `main` only after the final gate passes and all
allowed follow-up items are explicitly documented.

The `sync/rust-v0.121.0-base` branch is an internal checkpoint. It should not be
tagged or released as `mcodex`.

## Conflict Resolution Policy

Conflict resolution is contract-driven. The question is not "which side wins?"
but "how do we preserve upstream stable behavior and the fork's required
contracts?"

### Upstream-owned areas

Default to upstream for:

- general bug fixes
- upstream refactors and module splits
- shared protocol and tool infrastructure
- dependency and workspace maintenance
- upstream feature work not intentionally overridden by `mcodex`

If fork behavior touches these areas, reattach the fork seam to the upstream
shape rather than preserving old code.

### Fork-owned areas

Default to preserving the fork contract for:

- `mcodex` product identity
- `MCODEX_HOME` and `~/.mcodex`
- legacy upstream home probing only for migration
- account-pool state and policy
- pooled registration
- lease-scoped auth
- app-server pooled APIs
- OSS installer, update, and release behavior
- blocking CLI npm publication for native CLI artifacts

Preserving the contract does not mean preserving the exact old implementation.
If upstream changed the host architecture, the fork behavior must move to the
new architecture.

### Shared integration areas

Resolve shared integration files line by line. Do not use whole-file `ours` or
`theirs` for:

- `codex-rs/core/src/client.rs`
- `codex-rs/core/src/codex_thread.rs`
- `codex-rs/core/src/codex_delegate.rs`
- `codex-rs/login/src/auth/mod.rs`
- `codex-rs/state/src/lib.rs`
- `codex-rs/app-server/src/message_processor.rs`
- `codex-rs/app-server/src/codex_message_processor.rs`
- TUI app, status, onboarding, and update files
- release workflow and installer scripts

These files carry both upstream behavior and fork-specific seams, so the final
state must be reviewed semantically.

### Generated files

Generated outputs should not be hand-resolved as final content:

- app-server JSON schemas
- generated TypeScript protocol files
- config schema
- Cargo lockfile when dependency metadata changes
- snapshots when UI output intentionally changes

Resolve source files first, then regenerate the derived outputs with the
appropriate repo commands.

### Deleted by upstream, modified by fork

When upstream deletes a file that the fork modified, default to accepting the
upstream deletion and porting the fork semantics into the upstream replacement.

For this sync, `codex-rs/core/src/codex.rs` is a high-risk example. Upstream
removed that file, while the fork has runtime account-pool behavior in the old
shape. The correct resolution is to migrate the fork behavior into upstream's
new runtime/thread/delegate structure, not to resurrect the deleted file unless
there is a documented architectural reason.

## Core Runtime Contract

The following must not regress before either checkpoint is considered healthy.

### Product identity

- Normal runtime uses `MCODEX_HOME` and `~/.mcodex`.
- Normal runtime does not fall back to live `CODEX_HOME` or `~/.codex`.
- Legacy upstream home probing exists only for first-run migration.
- User-visible runtime identity remains `mcodex` where this fork has explicitly
  adopted it.

### Startup and login

- Fresh `mcodex` home startup works.
- Existing `mcodex` home startup works.
- First-run migration from upstream Codex does not block startup incorrectly.
- Migration copies only allowed config/auth data and does not import pooled
  state, runtime SQLite state, history, logs, or plugin caches.

### Account-pool and lease auth

- Account registration remains available for supported account types.
- Lease acquisition works for pooled runtime execution.
- Lease invalidation and unavailable-account paths fail closed.
- Pooled turns use lease-scoped auth rather than shared mutable legacy auth.
- Compact, review/subagent, realtime, and websocket request paths do not bypass
  lease-scoped auth when pooled execution is active.

### App-server pooled behavior

- App-server pooled APIs remain available where already shipped.
- App-server schema and Rust protocol definitions remain consistent.
- Pooled lease notifications and reads reflect the active runtime state.

### Release, install, and update

For the final `rust-v0.122.0` branch:

- installer scripts point to `downloads.mcodex.sota.wiki`
- update checks do not point back to upstream `openai/codex`
- GitHub Releases remain lightweight release records
- native CLI artifacts are distributed through OSS
- native CLI artifacts are not republished through npm

## Layered Gates

### 0.121 checkpoint gate

The internal `0.121` checkpoint must:

- compile
- pass targeted tests for changed core crates
- preserve the core runtime contract
- keep generated files in a coherent state where required for tests

It may defer:

- full release workflow validation
- installer smoke checks
- non-critical TUI polish
- non-critical observability command details
- documentation polish

Deferred work must be tracked explicitly if it still applies to the final
`0.122` target.

### 0.122 final gate

The final `0.122` branch must satisfy the `0.121` gate plus:

- release/update/install behavior remains fork-owned
- app-server generated schemas are regenerated and consistent
- UI snapshots are reviewed and accepted when intentionally changed
- non-core follow-up items are listed clearly
- no known core runtime contract regression remains

## Verification Plan

Run focused tests first, then broaden only as needed.

Recommended Rust checks:

```bash
cd codex-rs
cargo test -p codex-account-pool
cargo test -p codex-login
cargo test -p codex-state
cargo test -p codex-core
cargo test -p codex-app-server
cargo test -p codex-app-server-protocol
cargo test -p codex-tui
just fmt
```

Run scoped lint fixes for crates changed by the sync:

```bash
cd codex-rs
just fix -p <crate>
```

If config or app-server APIs change:

```bash
cd codex-rs
just write-config-schema
just write-app-server-schema
just write-app-server-schema --experimental
```

If UI output changes:

```bash
cd codex-rs
cargo test -p codex-tui
cargo insta pending-snapshots -p codex-tui
cargo insta accept -p codex-tui
```

Only accept snapshots after reviewing the pending output.

## Targeted Regression Checks

In addition to automated tests, manually or semi-automatically verify:

- fresh `mcodex` startup uses `~/.mcodex`
- `MCODEX_HOME` overrides the default home
- normal runtime does not use `CODEX_HOME`
- migration from upstream `~/.codex` copies only allowed data
- pooled turn execution uses lease-scoped auth
- compact/review/subagent/realtime/websocket paths do not bypass pooled auth
- unavailable pooled accounts fail closed
- app-server pooled read/notification behavior remains coherent
- installer scripts use OSS download URLs
- update prompts use the fork update source
- release workflow does not publish native CLI artifacts to npm

## Follow-Up Policy

Follow-up items are allowed only for non-core behavior. They must be written
down before merging the final sync branch.

Allowed follow-ups include:

- non-critical TUI wording or display polish
- observability command refinements
- documentation additions
- extra smoke-test automation

Not allowed as follow-ups:

- identity isolation regressions
- login/startup blockers
- account-pool lease-auth bypasses
- cross-account request contamination
- app-server schema/protocol inconsistency for shipped APIs
- installer or update paths pointing back to upstream Codex
- native CLI npm publishing regressions

## Future Stable Syncs

After `rust-v0.122.0`, future stable syncs should normally use one stable tag
at a time:

1. Create `sync/<upstream-tag>` from `main`.
2. Merge the upstream stable tag.
3. Apply the same conflict policy.
4. Run the final gate.
5. Merge to `main`.

Use intermediate checkpoints only when dry-run conflict analysis shows that a
direct stable-tag merge is too large to review safely.

Before starting a future sync, run a dry-run merge analysis and record:

- merge base
- conflict count
- changed path count
- high-risk files
- proposed checkpoint tags, if any

