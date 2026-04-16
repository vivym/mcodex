# Account Pool Selection Ownership And Policy Boundary Design

This document defines how pooled account startup selection should be made
state-owned while keeping pool policy configuration in `config.toml`.

It is intentionally scoped to ownership boundaries, startup resolution, and
operator-visible CLI behavior. It does not redesign the broader lease model,
remote backend shape, or the existing account registry schema beyond what is
needed to make pool selection coherent.

## Summary

The recommended direction is:

- treat registered accounts, pool membership, startup selection, preferred
  account, and suppression as state-owned data
- keep backend choice and pool policy in `config.toml`
- keep the existing upstream-friendly default-pool precedence where explicit
  config remains higher priority than persisted startup-selection fallback
- extend the existing state-layer startup preview into a richer startup-status
  API consumed by CLI, core, app-server, and TUI
- make fresh registration into an explicit pool establish persisted startup
  selection only when no durable default exists in either config or state
- make pooled-mode enablement depend on pooled intent discovered from config or
  state rather than raw `config.accounts` presence
- preserve existing JSON field meanings and add new resolution-source fields
  instead of repurposing shipped output contracts
- stop reporting config-only pool counts as though they were actual registered
  runtime pools

This resolves the current mismatch where:

- `accounts pool list` reports pools from state
- `accounts status` reports configured pools from config
- interactive startup still behaves as though no pool exists unless
  `config.toml` contains `[accounts]`

The design stays close to upstream shape by preserving:

- `AccountsConfigToml`
- config-based policy fields such as lease timing and context-reuse flags
- process-level pool override support
- persisted startup-selection state in the existing state DB

The main semantic change is not default-pool precedence; it is pooled-mode
ownership and activation. Persisted startup selection remains the durable
fallback for installs that do not declare an explicit config default, while
config remains the higher-priority operator intent when present.

## Goals

- Make pooled account startup selection understandable and consistent.
- Ensure that registering accounts into a pool can make future startup work
  without requiring manual `config.toml` edits.
- Keep account/pool facts in the state DB instead of splitting them across
  unrelated sources.
- Preserve mergeability with upstream by minimizing churn to config types and
  reusing existing state models where possible.
- Keep policy-level settings declarative and reviewable in `config.toml`.
- Unify startup selection and availability reporting across CLI, core runtime,
  app-server, and TUI.

## Non-Goals

- Do not remove `AccountsConfigToml`.
- Do not move lease timing or backend policy into SQLite in this slice.
- Do not redesign pool membership or account registry ownership.
- Do not introduce a fork-only backend abstraction that diverges from upstream
  control-plane and execution-plane boundaries.
- Do not change the persistent state DB location or migration ownership.
- Do not broaden this slice into a full remote account-pool redesign.

## Constraints

- Upstream design already separates static config from runtime state and local
  persistence.
- `mcodex` product-identity work explicitly copies config and auth during
  migration, but does not copy pooled SQLite state from upstream `codex`.
- Mergeability with upstream matters, so retaining current types and minimizing
  cross-crate API churn is preferable to deleting config support entirely.
- Startup selection still has to support:
  - process-local override such as `--account-pool`
  - durable preferred-account selection
  - durable suppression
  - compatibility bootstrap from config or migration state

## Problem Statement

The current implementation has three separate notions of "what pool exists":

1. state-owned pool membership derived from registered accounts
2. config-owned `accounts.default_pool` and `accounts.pools`
3. persisted startup-selection state in SQLite

Those notions are not consistently prioritized.

Current consequences:

- `accounts pool list` derives pools from registered state, so it shows pools
  that actually contain accounts.
- `accounts status` reports `configured pools` by counting
  `config.accounts.pools`, even if the runtime has registered accounts in state.
- `accounts add --account-pool X` registers the account in pool `X` but does
  not establish a durable startup default for future runs.
- core pooled mode is gated on the presence of `config.accounts`, so a home
  with valid pooled state but no config can still fall back to legacy login/API
  key onboarding.

The result is a misleading operator model: pool state is real enough to hold
accounts, but not real enough to drive startup.

## Approaches Considered

### Approach A: Keep current ownership and patch the CLI wording only

Under this approach:

- `configured pools` would be renamed to clarify it comes from config
- docs would tell users to keep editing `config.toml`
- pooled startup would still fundamentally depend on config presence

Pros:

- Lowest code churn
- Minimal risk to upstream behavior

Cons:

- Does not actually solve the product problem
- Leaves startup behavior split across config and state
- Still requires manual config edits after registration

This approach is rejected.

### Approach B: Eliminate accounts config and use the state DB as the only truth

Under this approach:

- `AccountsConfigToml` for pooled accounts would be removed or drastically
  reduced
- default pool, pool policy, and lease timing would move into SQLite
- startup and runtime would be DB-only

Pros:

- One apparent source of truth
- Startup behavior would line up with registered account state

Cons:

- Blurs operator policy and runtime state
- Makes reviewable declarative policy harder
- Fights the existing upstream architecture and docs
- Increases migration and schema churn
- Raises merge friction significantly

This approach is rejected.

### Approach C: State-owned selection, config-owned policy

Under this approach:

- state owns startup selection and registered runtime facts
- config owns backend and policy
- config `default_pool` remains the higher-priority operator default
- persisted startup selection remains the durable fallback when config does not
  declare a default

Pros:

- Fixes the actual UX and ownership problem
- Preserves most upstream types and configuration surfaces
- Keeps policy declarative
- Requires less migration churn than DB-only

Cons:

- Needs coordinated updates across CLI, core, app-server, and TUI
- Requires careful handling for state-only homes and missing policy config

This is the recommended approach.

## Recommended Design

### 1. Separate facts from policy

#### State-owned facts

The following remain or become authoritative in SQLite:

- registered accounts
- pool membership
- persisted startup default pool
- preferred account
- suppression flag
- lease and health state
- runtime eligibility state

These are installation-local facts that evolve as the user operates the tool.

#### Config-owned policy

The following remain in `config.toml`:

- `accounts.backend`
- `accounts.proactive_switch_threshold_percent`
- `accounts.lease_ttl_secs`
- `accounts.heartbeat_interval_secs`
- `accounts.min_switch_interval_secs`
- `accounts.allocation_mode`
- `accounts.pools.<id>.allow_context_reuse`
- `accounts.pools.<id>.account_kinds`

These are operator intent and policy, not runtime facts.

### 2. Narrow the meaning of `accounts.default_pool`

`accounts.default_pool` should remain in the config type for compatibility, but
its role should be clarified:

- It remains an explicit operator-configured default and therefore keeps higher
  priority than persisted startup-selection fallback.
- It does not stop startup selection from being persisted in state.
- Persisted startup selection is still used when config does not define a
  default.

This preserves config compatibility while still allowing state to carry durable
selection for config-less installs and fresh local product homes.

### 3. Use one effective-pool resolution rule everywhere via the state layer

The effective-pool resolution order should become:

1. process-level override such as `--account-pool <pool>`
2. `accounts.default_pool` from config when present
3. persisted startup-selection default pool in state
4. otherwise no effective pool

This rule should be shared by:

- `codex accounts current`
- `codex accounts status`
- `AccountPoolManager` startup lease preparation
- app-server account lease diagnostics
- TUI startup access probing

There should not be separate CLI-only or app-server-only interpretations of
default pool precedence. This preserves the current upstream-friendly precedence
while making the rule shared and explicit.

### 4. Make fresh registration establish durable startup selection when needed

When `accounts add chatgpt` is invoked with an explicit pool target and there is
no durable default in either config or persisted startup-selection state:

- register the account into the requested pool
- persist that pool as the startup default pool
- clear suppression for the fresh selection
- leave preferred account unset unless the workflow already has a good reason to
  choose a specific preferred account

This keeps registration idempotent while ensuring that a fresh home can become
startup-eligible without a manual config edit.

This behavior should be limited to the "no durable default exists yet" case so
that explicit later operator choices in either config or state are not silently
overwritten.

### 5. Stop using config presence as the pooled-mode gate

Core runtime should not require `config.accounts` to exist before pooled mode is
available.

Instead:

- pooled mode should initialize only when a pooled-intent probe succeeds
- config policy, when present, should overlay onto runtime behavior only after
  pooled applicability has been established
- when policy config is absent, runtime should use explicit built-in defaults

This removes the current false dependency where valid pooled state exists but
the runtime still falls back to legacy onboarding because `[accounts]` is
missing.

The pooled-intent probe should be satisfied by at least one of:

- a process-level override explicitly naming a pool
- config declaring `accounts.default_pool`
- pooled account membership already existing in state

The following must not count as pooled intent by themselves:

- `config.accounts` containing only policy fields
- migrated policy-only pooled config copied into a fresh `mcodex` home
- preferred-account or suppression markers without matching pool membership

Implementation should preserve `account_pool_manager: Option<_>` as the
top-level pooled-mode gate so existing turn and compact call sites do not begin
failing closed in non-pooled sessions.

### 6. Defer policy-default changes in this slice

This slice should not change runtime policy defaults beyond what is required to
remove the false dependency on config presence.

Recommended behavior:

- preserve current runtime defaults when policy config is absent
- treat state-only pooled homes as implying the `local` account-pool backend
- do not tighten `allow_context_reuse` semantics in the same slice as startup
  selection ownership

Reasoning:

- the source-of-truth problem can be solved without bundling a separate runtime
  behavior change
- state-only pooled homes are backed by local SQLite and local pooled-auth
  storage, so treating them as `local` avoids undefined backend semantics
- preserving current defaults minimizes regression risk and merge friction
- context-reuse policy tightening can be evaluated later as its own focused
  design slice with dedicated compatibility review

### 7. Make diagnostics describe both policy and runtime state accurately

`accounts status` should distinguish:

- persisted default pool
- configured default pool
- effective pool
- effective pool resolution source
- effective account source
- registered pool count
- configured policy pool count

It should not collapse these into one misleading `configured pools` line.

Recommended text fields:

- `persisted default pool: <id|none>`
- `configured default pool: <id|none>`
- `effective pool: <id|none>`
- `effective pool resolution: override|persistedSelection|configDefault|none`
- `effective account source: <accountSource|none>`
- `registered pools: <n>`
- `configured policy pools: <n>`

The JSON form should expose the same distinctions.

For compatibility:

- preserve existing `configuredPoolCount` with its current config-policy meaning
- preserve existing `effectivePoolSource` with its current account-source
  meaning
- add new fields such as `registeredPoolCount`,
  `configuredPolicyPoolCount`, `persistedDefaultPoolId`, and
  `configuredDefaultPoolId`
- add a new field such as `effectivePoolResolutionSource` rather than silently
  changing the old `effectivePoolSource` contract

### 8. Keep merge friction low by preserving existing public shapes where possible

To stay merge-friendly:

- keep `AccountsConfigToml` rather than replacing it
- avoid changing the state schema unless a small additive field is genuinely
  needed
- reuse existing startup-selection state fields instead of introducing a second
  persistent default-pool store
- extend the existing state-layer startup preview/status APIs rather than
  creating a second parallel resolver contract
- prefer additive diagnostics fields over breaking JSON removals in the first
  slice
- preserve `account_pool_manager: Option<_>` semantics at the session-service
  boundary where practical

## Architecture Changes

### State/account-pool startup status API

Extend the existing state-layer startup preview so `codex-state` remains the
source of persisted startup-selection facts and membership-backed availability.
Add the corresponding startup-status and resolved-policy adapter in
`codex-account-pool`, then have CLI, core, app-server, and TUI consume that
shared result instead of each reimplementing the probe.

It should take:

- optional process override
- optional configured default pool id

and returns:

- startup preview
- effective pool resolution source
- configured default pool id
- persisted default pool id
- startup availability classification
- optional pool diagnostic when an effective pool resolves

This should build on `StateRuntime::preview_account_startup_selection` rather
than creating a second resolver with overlapping semantics. The state layer
continues to own persisted facts; the account-pool layer owns resolved policy
defaults and pooled-applicability decisions that depend on those facts. The
`codex-account-pool` layer should remain a thin adapter over `codex-state`
rather than growing a second independent startup-selection or membership
resolver.

### Policy resolution

Introduce a resolved policy view in `codex-account-pool` with built-in defaults
so runtime can operate even when `config.accounts` is absent.

This view should be derived from `AccountsConfigToml` when present, but it must
also be constructible from defaults alone. It should package config/defaulted
policy only; it should not duplicate persisted-fact reads or precedence logic
that already belongs to `codex-state`.

### Runtime initialization

Session bootstrap should stop deciding pooled applicability directly inside the
core-local manager wiring. Instead, core should ask the shared account-pool
startup-status path whether pooled mode applies, then treat the existing
`build_account_pool_manager` logic as a thin adapter over that result.

Runtime initialization should:

- run the pooled-intent probe through the shared account-pool startup-status
  path
- require state DB plus pooled intent before constructing the manager
- reject policy-only migrated config as insufficient pooled applicability
- derive a resolved policy view from config or defaults inside
  `codex-account-pool` only after pooled mode has been deemed applicable

This preserves the existing optional-manager gate while removing the false
requirement that config must be present for pooled mode to exist.

## CLI Behavior

### `accounts add`

- `--account-pool` continues to target the registration pool explicitly
- if neither config nor state already declares a durable default, the requested
  pool becomes the persisted startup default
- if a durable config or state default already exists, it remains unchanged

### `accounts current`

- show effective pool under the shared resolution rule
- preserve `effectivePoolSource` as account-source output
- add `effectivePoolResolutionSource` in JSON for selection provenance

### `accounts status`

- stop presenting config-only pool count as total configured runtime pools
- show persisted/configured/effective pool distinctions
- preserve existing JSON field meanings while adding new resolution fields

### `accounts pool list`

- continue listing registered pools from state
- optionally gain `--json` in a later slice, but that is not required here

### `accounts resume` and `accounts switch`

- continue to mutate persisted startup-selection state
- continue to leave process-level config overrides transient only

## App-Server Behavior

App-server lease diagnostics should consume the same state-layer startup status
API as CLI and core. This prevents different surfaces from disagreeing about
whether a pool is selected or startup access is available.

## TUI Behavior

TUI startup-access probing must also consume the same state-layer startup status
API. This slice is not complete unless the local startup prompt logic stops
falling back to login/API-key prompts for homes that have pooled membership and
resolved pooled startup access.

## Migration And Compatibility

### Existing homes with config and state

- config `default_pool` continues to take precedence over persisted
  startup-selection fallback
- persisted startup selection remains visible in diagnostics and continues to
  serve config-less startups

### Existing homes with config but no persisted selection

- config `default_pool` continues to work as the effective configured default
- first explicit stateful pool choice may establish persisted default selection
  only when config does not already declare a durable default

### Existing homes with state but no config

- pooled startup should become available
- runtime uses built-in policy defaults
- runtime treats the account-pool backend as `local`

### Product-identity migration

- config and auth can still be copied
- SQLite pooled state still remains product-local and is not imported from
  upstream `codex`
- config migration should remain a transform, not a blind copy
- when upstream pooled SQLite state is not imported, config migration should
  preserve accounts policy fields, including `accounts.allocation_mode`, but
  intentionally drop `accounts.default_pool` so the new product home can
  establish its own persisted startup selection from local registration
- imported policy-only pooled config must not by itself cause the fresh product
  home to enter pooled mode before local pooled membership exists or an explicit
  local default pool is chosen

This design works with the `mcodex` migration principle because policy remains
copyable in config while selection and runtime state remain installation-local.

## Testing Strategy

At minimum, add or update tests for:

- CLI status on a home with registered pools but no `config.toml`
- CLI status distinguishing persisted default vs configured default
- CLI JSON preserving `effectivePoolSource` semantics while adding
  `effectivePoolResolutionSource`
- `accounts add --account-pool X` persisting startup default only when no
  durable config or state default exists
- `accounts add --account-pool X` not overwriting an existing persisted default
- core startup using pooled mode with state-only selection and default policy
- core and TUI startup staying out of pooled mode for migrated policy-only
  config with no local pooled membership
- app-server diagnostics using configured default before persisted fallback
- TUI startup access on a home with pooled membership but no login/auth baseline
- migration transform dropping `accounts.default_pool` while preserving
  `accounts.allocation_mode` and the rest of accounts policy
- migration prompt detection when the legacy upstream home is surfaced via
  `CODEX_HOME`
- context-reuse behavior when policy config is absent

## Risks

### Risk: pooled-intent probing could enable pooled mode in sessions that only
contain stale or partial state

Mitigation:

- require explicit pooled signals rather than "state DB exists" alone
- use real registered pool membership as the state-side pooled-intent signal
  instead of treating preferred-account or suppression markers as sufficient
- add regression tests for non-pooled sessions that still should not build an
  account-pool manager

### Risk: registration may unexpectedly persist a startup default

Mitigation:

- limit persistence to the case where neither config nor state already declares
  a durable default
- add regression tests covering config-default homes and existing-persisted
  default homes

### Risk: copied upstream config could still block fresh local startup selection

Mitigation:

- make config migration explicitly drop `accounts.default_pool` when pooled state
  is not imported
- treat policy-only migrated accounts config as insufficient pooled intent until
  local pooled membership or an explicit local default exists
- add migration tests covering fresh `mcodex` homes seeded from upstream config

### Risk: field-level JSON compatibility regressions

Mitigation:

- preserve existing `effectivePoolSource` and `configuredPoolCount` semantics
- add only additive JSON fields for new diagnostics

### Risk: partial adoption leaves CLI, core, and app-server inconsistent

Mitigation:

- do not land this slice without the state-layer startup status API being used
  by CLI, core, app-server, and TUI

## Recommended Implementation Shape

1. extend the state-layer startup preview into a richer startup-status API with
   resolution-source and availability outputs, then expose it through a shared
   `codex-account-pool` startup-status adapter
2. update CLI diagnostics/output to use the richer state-layer status while
   preserving existing JSON field meanings
3. make `accounts add` establish persisted default selection only when no
   durable config or state default exists
4. define state-only pooled homes as `local` backend and add pooled-intent
   probing based on explicit override, configured default pool, or registered
   membership
5. update `codex-account-pool` to own the thin resolved-policy/applicability
   adapter over `codex-state`, without introducing a second startup resolver
6. update core, app-server, and TUI startup access to use the shared
   account-pool startup status API
7. update `mcodex` config migration transform to strip `accounts.default_pool`
   while preserving accounts policy, including `allocation_mode`
8. add regression tests across CLI, core, app-server, TUI, and migration

## Acceptance Criteria

- A fresh `mcodex` home with pooled accounts registered into `main-pool` starts
  in pooled mode without requiring manual `config.toml` edits.
- A fresh `mcodex` home created by config/auth migration from upstream can
  register a local pooled account and establish startup selection without manual
  config edits.
- `accounts status` on that home reports a real effective pool and distinguishes
  runtime pools from configured policy pools.
- CLI JSON preserves existing field meanings while adding explicit resolution
  provenance.
- CLI, core, app-server, and TUI agree on startup selection and availability.
- `config.toml` still carries policy fields and remains merge-compatible with
  upstream shape.
- No DB-only rewrite is required.
