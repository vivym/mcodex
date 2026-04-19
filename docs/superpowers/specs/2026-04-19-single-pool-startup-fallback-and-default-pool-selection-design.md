# Single-Pool Startup Fallback And Default-Pool Selection Design

This document defines how pooled startup should behave when account pools are
already registered locally but no explicit default pool has been configured.

It is an additive follow-up to:

- `docs/superpowers/specs/2026-04-16-account-pool-selection-state-policy-design.md`
- `docs/superpowers/specs/2026-04-14-pooled-only-startup-notice-design.md`
- `docs/superpowers/specs/2026-04-16-remote-account-pool-contract-v0-design.md`

## Summary

Today pooled startup remains awkward in two common cases:

- a home has exactly one registered pool, but neither `config.toml` nor the
  persisted startup-selection state names it as the default
- a home has multiple registered pools, but no explicit default has been
  chosen yet

In both cases the product already knows meaningful pool state, but startup
resolution reduces that state to "missing pool" and interactive startup can
fall back to the ChatGPT login flow even though pooled access exists.

The recommended direction is:

- add a shared startup-resolution fallback for the case where exactly one pool
  is registered and no explicit default exists
- keep config-owned `accounts.default_pool` higher priority than any
  state-backed default
- add an explicit CLI for managing the state-backed default pool instead of
  requiring users to edit `config.toml`
- treat multi-pool-without-default as its own startup condition, with a
  dedicated pooled notice rather than the generic login screen
- expose the new resolution and warning states consistently in CLI, TUI, and
  remote-facing protocol output

This keeps merge risk low because it extends the existing startup-selection
model instead of introducing a fork-only control plane or silently mutating
configuration files.

## Goals

- Make single-pool local homes work without requiring
  `-c 'accounts.default_pool=\"...\"'`.
- Give users a first-class command for choosing a durable local default pool.
- Preserve the ownership boundary where config expresses operator intent and
  SQLite expresses local runtime state.
- Make "why pooled startup did or did not activate" explainable in CLI, TUI,
  and remote protocol output.
- Keep the design compatible with a future remote pool backend.
- Minimize churn against upstream startup and onboarding structure.

## Non-Goals

- Do not auto-generate or silently rewrite `config.toml`.
- Do not change the priority of `accounts.default_pool` relative to explicit
  local state.
- Do not redesign the broader account lease lifecycle or pool allocation
  policy.
- Do not add a TUI pool picker in this slice.
- Do not invent a remote-only startup rule that differs from local behavior.

## Problem Statement

The current startup-selection model recognizes three durable pool sources:

1. process-local override such as `--account-pool`
2. config-owned `accounts.default_pool`
3. persisted startup-selection state in SQLite

When none of those are present, startup reports `MissingPool` even if the
registry already contains registered accounts and pool membership. This causes
two UX failures:

- if exactly one pool is registered, users still have to inject a default by
  hand even though the only reasonable pooled target is already obvious
- if multiple pools are registered, the startup path falls back to the login
  screen instead of telling the user that pooled access exists but an explicit
  default is now required

The product therefore has real pool state but no productized way to either use
the obvious single pool or persist a user-selected default once a second pool
arrives.

## Approaches Considered

### Approach A: Patch the TUI only

Under this approach the TUI would special-case pool counts and decide locally
whether to continue, prompt for a pool, or show login.

Pros:

- Smallest apparent code change
- No CLI surface change

Cons:

- CLI and TUI semantics diverge immediately
- Remote support would require a second implementation
- `accounts status` would still say `effective pool: none` in cases where the
  TUI pretends otherwise

This approach is rejected.

### Approach B: Auto-persist or auto-write config when startup finds one pool

Under this approach single-pool detection would immediately write
`config.toml` or persisted startup-selection state.

Pros:

- Startup appears to "fix itself" after one launch

Cons:

- Read-path logic gains hidden write side effects
- Silent config mutation is hostile to operator-owned config
- Debugging resolution source becomes harder
- Remote behavior would still need a different story

This approach is rejected.

### Approach C: Shared startup fallback plus explicit state-backed default

Under this approach:

- shared startup resolution gains a read-only single-pool fallback
- users can explicitly set or clear a durable local default through CLI
- multi-pool-without-default becomes a first-class startup condition surfaced
  through CLI, TUI, and remote responses

Pros:

- Solves the actual UX issue instead of hiding it
- Preserves current ownership boundaries
- Keeps the behavior explainable
- Extends naturally to remote backends
- Limits merge risk by reusing existing state and onboarding structure

Cons:

- Requires coordinated updates across state, CLI, TUI, and app-server

This is the recommended approach.

## Recommended Design

### 1. Effective-pool precedence

The effective-pool precedence should become:

1. process-local override such as `--account-pool`
2. `config.toml` `accounts.default_pool`
3. persisted local default pool in startup-selection state
4. single registered pool fallback
5. no effective pool

This precedence is shared across local CLI, local TUI startup, and remote
protocol surfaces.

The design keeps explicit config higher priority than the state-backed default.
That preserves the current operator-owned semantics of `config.toml` while
still giving users a productized local preference mechanism.

### 2. Single registered pool fallback

If all of the following are true:

- no process-local override is present
- `accounts.default_pool` is absent
- persisted `default_pool_id` is absent
- exactly one pool is registered in state

then startup resolution should treat that pool as the effective pool for the
current read.

This fallback is intentionally read-only:

- it does not write `config.toml`
- it does not write startup-selection state
- it does not clear or modify preferred account state

The fallback should surface a distinct resolution source,
`singleRegisteredPool`, instead of pretending that the pool came from config
or persisted state.

### 3. Invalid explicit defaults do not auto-fallback

If an explicit default source exists but points to a missing or otherwise
invalid pool, startup must not silently fall back to the sole registered pool.

That applies to:

- `accounts.default_pool`
- persisted `default_pool_id`
- process-local override

In those cases the explicit source remains authoritative and startup should
surface a structured warning or blocker that explains which source was invalid.

This preserves debuggability and avoids surprising users by silently changing
their declared default.

### 4. Explicit CLI for durable local default selection

Add a dedicated CLI surface:

- `mcodex accounts pool default set <POOL_ID>`
- `mcodex accounts pool default clear`

These commands operate on the existing startup-selection state in SQLite.
They do not write `config.toml`.

#### `default set`

`mcodex accounts pool default set <POOL_ID>` should:

- validate that `<POOL_ID>` is a registered pool
- write `default_pool_id = <POOL_ID>`
- clear `preferred_account_id`
- set `suppressed = false`

If `config.toml` also defines `accounts.default_pool`, the command should still
persist the local default but report that the effective pool remains controlled
by config until that config value is removed or changed.

#### `default clear`

`mcodex accounts pool default clear` should:

- clear persisted `default_pool_id`
- clear `preferred_account_id`
- preserve `suppressed`

This keeps "default pool selection" separate from "resume pooled startup".
Users should continue to use the existing `accounts resume` command when they
intend to clear durable suppression.

### 5. Keep startup-selection concerns separate

Existing command responsibilities remain distinct:

- `accounts pool default set|clear` manages the durable default pool
- `accounts switch <ACCOUNT_ID>` manages preferred account selection inside the
  current effective pool
- `accounts resume` clears durable suppression

The design explicitly avoids one command that mixes default-pool choice,
preferred-account choice, and suppression state.

### 6. Multi-pool without default is a distinct startup condition

When multiple pools are registered but no effective default can be resolved,
interactive startup should not fall back to the generic login screen.

Instead it should enter a dedicated pooled notice that explains:

- pooled access exists
- multiple pools are registered
- no default pool is configured
- the user should run
  `mcodex accounts pool default set <POOL_ID>` to make pooled startup durable

This notice should remain separate from the existing login screen so that
"needs shared login" and "needs pooled default selection" are not conflated.

The notice may still offer a way to continue into shared-login onboarding, but
its primary explanation is about pool selection rather than authentication.

### 7. Reuse the existing onboarding notice shell

The new multi-pool-without-default screen should be implemented as another
pooled-access notice kind within the current onboarding shell rather than as a
new onboarding subsystem.

This keeps the UX structure consistent with the existing:

- `PooledOnlyNotice`
- `PooledAccessPausedNotice`

and limits merge risk in `tui/src/onboarding`.

### 8. Structured observability for startup resolution

The startup-resolution model should become more explicit for both text and JSON
surfaces.

#### Resolution source

Add `singleRegisteredPool` to the effective-pool resolution source enum and
propagate it through:

- CLI output
- app-server protocol output
- remote TUI startup probe handling

#### Warning or blocker shape

Add an additive structured warning or blocker field so clients can distinguish
at least these cases:

- multiple registered pools require explicit default selection
- configured default pool is missing or invalid
- persisted default pool is missing or invalid
- process override points to a missing or invalid pool

This field should be additive and should not repurpose existing JSON fields.

#### `accounts status`

`accounts status` should explain the startup state in human-readable form. In
particular it should distinguish:

- config default selected
- persisted default selected
- single registered pool fallback selected
- multiple registered pools require default selection
- configured default is invalid
- persisted default is invalid

The existing `poolObservability` addition remains useful, but it is not a
replacement for startup-resolution diagnostics because there may be no
effective pool to observe yet.

### 9. Remote compatibility

Remote backends should consume and return the same startup-resolution concepts
instead of re-deriving pool-selection semantics in the TUI.

That means:

- the shared startup model owns resolution source and warning/blocker shape
- app-server protocol surfaces should expose the additive fields needed by
  remote clients
- the TUI should treat remote startup as a consumer of those fields, not the
  place where multi-pool/no-default policy is invented

This keeps the local and remote models aligned and avoids later rework when a
remote pool authority becomes primary.

## Testing Strategy

### State and account-pool tests

Add focused tests for:

- no explicit default plus exactly one registered pool resolves
  `singleRegisteredPool`
- no explicit default plus multiple registered pools yields no effective pool
  and the new structured warning
- invalid config default does not fall back to the sole registered pool
- invalid persisted default does not fall back to the sole registered pool

### CLI tests

Add focused tests for:

- `accounts pool default set`
- `accounts pool default clear`
- `accounts status` text output for each new resolution state
- `accounts status --json` additive fields for new resolution and warning

### TUI tests

Add snapshot and behavior coverage for:

- the new multi-pool default-selection notice
- startup prompt resolution for multi-pool-without-default
- single-pool fallback continuing through the pooled startup path rather than
  the login path

### App-server tests

Add protocol and server coverage for:

- new resolution source serialization
- additive warning or blocker fields
- remote startup responses that represent single-pool fallback and
  multi-pool-without-default distinctly

## Migration And Compatibility

This slice should remain additive.

- No config migration is required.
- No startup action should silently rewrite user config.
- The single-pool fallback is a read-time semantic enhancement, not a schema
  migration.
- The explicit default-pool commands write the existing startup-selection
  state shape.

Older homes therefore gain better startup behavior without needing a data
migration, while users who later add a second pool gain a first-class command
for making their preferred default durable.

## Merge-Risk Notes

To stay friendly to upstream merges, this slice should avoid:

- changing ownership of pool policy config
- adding fork-only storage systems
- burying startup policy in TUI-only heuristics
- mutating `config.toml` as part of startup reads

The intended code footprint is concentrated in:

- startup-resolution logic in state/account-pool
- CLI pool-default commands and status output
- app-server protocol projection of additive startup metadata
- one additional onboarding notice in the TUI

That keeps the behavior coherent while limiting churn in unrelated areas of
the codebase.
