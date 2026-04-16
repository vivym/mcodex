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
- narrow `accounts.default_pool` from an always-live startup override to a
  bootstrap compatibility fallback used only when persisted startup selection
  has not been established yet
- make CLI, core, and app-server use one shared effective-pool resolution rule
- make fresh registration into an explicit pool establish persisted startup
  selection when no persisted default exists yet
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

The main semantic change is ownership: persisted startup selection becomes the
authoritative default for future runtimes, while config remains policy and
bootstrap input rather than the primary long-lived source of startup truth.

## Goals

- Make pooled account startup selection understandable and consistent.
- Ensure that registering accounts into a pool can make future startup work
  without requiring manual `config.toml` edits.
- Keep account/pool facts in the state DB instead of splitting them across
  unrelated sources.
- Preserve mergeability with upstream by minimizing churn to config types and
  reusing existing state models where possible.
- Keep policy-level settings declarative and reviewable in `config.toml`.
- Unify effective-pool resolution across CLI, core runtime, and app-server.

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
- config `default_pool` becomes bootstrap compatibility input, not the primary
  persisted startup owner

Pros:

- Fixes the actual UX and ownership problem
- Preserves most upstream types and configuration surfaces
- Keeps policy declarative
- Requires less migration churn than DB-only

Cons:

- Requires one semantic change to effective-pool precedence
- Needs coordinated updates across CLI, core, and app-server
- Requires careful handling for missing per-pool policy config

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
- `accounts.pools.<id>.allow_context_reuse`
- `accounts.pools.<id>.account_kinds`

These are operator intent and policy, not runtime facts.

### 2. Narrow the meaning of `accounts.default_pool`

`accounts.default_pool` should remain in the config type for compatibility, but
its semantics should narrow:

- It is no longer the always-authoritative default for future runtimes.
- It acts as bootstrap input when persisted startup selection has not yet been
  established.
- Once startup selection has a persisted default pool in state, that persisted
  value becomes the authoritative default unless a process-level override is
  supplied.

This preserves config compatibility while moving durable selection ownership to
state.

### 3. Use one effective-pool resolution rule everywhere

The effective-pool resolution order should become:

1. process-level override such as `--account-pool <pool>`
2. persisted startup-selection default pool in state
3. `accounts.default_pool` from config as bootstrap compatibility fallback
4. otherwise no effective pool

This rule should be shared by:

- `codex accounts current`
- `codex accounts status`
- `AccountPoolManager` startup lease preparation
- app-server account lease diagnostics

There should not be separate CLI-only or app-server-only interpretations of
default pool precedence.

### 4. Make fresh registration establish durable startup selection when needed

When `accounts add chatgpt` is invoked with an explicit pool target and there is
no persisted default pool in startup-selection state yet:

- register the account into the requested pool
- persist that pool as the startup default pool
- clear suppression for the fresh selection
- leave preferred account unset unless the workflow already has a good reason to
  choose a specific preferred account

This keeps registration idempotent while ensuring that a fresh home can become
startup-eligible without a manual config edit.

This behavior should be limited to the "no persisted default exists yet" case so
that explicit later operator choices are not silently overwritten.

### 5. Stop using config presence as the pooled-mode gate

Core runtime should not require `config.accounts` to exist before pooled mode is
available.

Instead:

- pooled mode should initialize when state DB exists and effective-pool
  resolution succeeds under the shared resolution rule
- config policy, when present, should overlay onto runtime behavior
- when policy config is absent, runtime should use explicit built-in defaults

This removes the current false dependency where valid pooled state exists but
the runtime still falls back to legacy onboarding because `[accounts]` is
missing.

### 6. Clarify policy defaults when config is absent

This slice must explicitly choose behavior for missing per-pool policy config.

Recommended default:

- `allow_context_reuse = false` when there is no explicit per-pool policy entry

Reasoning:

- it matches the documented "explicit opt-in" boundary for cross-account context
  reuse
- it is safer than implicitly enabling reuse on state-only installs
- it avoids surprising behavior after fresh registration in an otherwise
  unconfigured home

Lease timing and switch thresholds may continue to use existing built-in
defaults when config is absent.

This is the one part of the slice with the highest behavior-change risk relative
to current code, but it is still the correct ownership boundary.

### 7. Make diagnostics describe both policy and runtime state accurately

`accounts status` should distinguish:

- persisted default pool
- configured bootstrap default pool
- effective pool
- effective pool source
- registered pool count
- configured policy pool count

It should not collapse these into one misleading `configured pools` line.

Recommended text fields:

- `persisted default pool: <id|none>`
- `configured bootstrap pool: <id|none>`
- `effective pool: <id|none>`
- `effective pool source: override|persistedSelection|configBootstrap|none`
- `registered pools: <n>`
- `configured policy pools: <n>`

The JSON form should expose the same distinctions.

### 8. Keep merge friction low by preserving existing public shapes where possible

To stay merge-friendly:

- keep `AccountsConfigToml` rather than replacing it
- avoid changing the state schema unless a small additive field is genuinely
  needed
- reuse existing startup-selection state fields instead of introducing a second
  persistent default-pool store
- centralize effective-pool resolution in a shared helper rather than scattering
  fork-specific conditionals
- prefer additive diagnostics fields over breaking JSON removals in the first
  slice

## Architecture Changes

### Shared effective-pool resolver

Introduce a small shared resolver used by CLI, core, and app-server that takes:

- optional process override
- persisted startup-selection state
- optional accounts config

and returns:

- effective pool id
- source of that decision
- configured bootstrap default pool
- persisted default pool

This is the key refactor that removes duplicated precedence logic.

### Policy resolution

Introduce a resolved policy view with built-in defaults so core runtime can
operate even when `config.accounts` is absent.

This view should be derived from `AccountsConfigToml` when present, but it must
also be constructible from defaults alone.

### Runtime initialization

`build_account_pool_manager` should stop returning `None` merely because
`config.accounts` is absent. It should instead:

- require state DB
- derive a resolved policy view from config or defaults
- let startup lease preparation decide whether pooled mode is actually
  startable based on effective-pool resolution

## CLI Behavior

### `accounts add`

- `--account-pool` continues to target the registration pool explicitly
- if persisted startup default is absent, the requested pool becomes the
  persisted startup default
- if persisted startup default already exists, it remains unchanged

### `accounts current`

- show effective pool under the shared resolution rule
- show effective pool source in JSON

### `accounts status`

- stop presenting config-only pool count as total configured runtime pools
- show persisted/configured/effective pool distinctions

### `accounts pool list`

- continue listing registered pools from state
- optionally gain `--json` in a later slice, but that is not required here

### `accounts resume` and `accounts switch`

- continue to mutate persisted startup-selection state
- continue to leave process-level config overrides transient only

## App-Server Behavior

App-server lease diagnostics should consume the same effective-pool resolver as
CLI and core. This prevents the desktop/app-server surface from disagreeing
with the terminal CLI about whether a pool is available.

## Migration And Compatibility

### Existing homes with config and state

- persisted startup selection should begin taking precedence over config
  `default_pool`
- config `default_pool` remains visible as bootstrap compatibility metadata

### Existing homes with config but no persisted selection

- config `default_pool` continues to work as bootstrap fallback
- first explicit stateful pool choice may establish persisted default selection

### Existing homes with state but no config

- pooled startup should become available
- runtime uses built-in policy defaults

### Product-identity migration

No change is needed to the existing `mcodex` migration principle:

- config and auth can still be copied
- SQLite pooled state still remains product-local and is not imported from
  upstream `codex`

This design works with that rule because policy remains copyable in config while
selection and runtime state remain installation-local.

## Testing Strategy

At minimum, add or update tests for:

- CLI status on a home with registered pools but no `config.toml`
- CLI status distinguishing persisted default vs configured bootstrap default
- `accounts add --account-pool X` persisting startup default when none exists
- `accounts add --account-pool X` not overwriting an existing persisted default
- core startup using pooled mode with state-only selection and default policy
- app-server diagnostics using persisted selection before config bootstrap
- context-reuse behavior when policy config is absent

## Risks

### Risk: behavior change for existing installs that rely on config overriding state

Mitigation:

- document the precedence change clearly
- preserve config field shape
- expose both persisted and configured defaults in diagnostics

### Risk: context-reuse default tightening changes transport behavior

Mitigation:

- scope the change to missing-policy cases only
- add explicit regression tests around account rotation

### Risk: partial adoption leaves CLI, core, and app-server inconsistent

Mitigation:

- do not land this slice without a shared resolver used by all three surfaces

## Recommended Implementation Shape

1. extract shared effective-pool resolution helper
2. update CLI diagnostics/output to use richer resolved data
3. make `accounts add` establish persisted default selection when absent
4. refactor core pooled-mode initialization to use resolved policy defaults
   instead of `config.accounts` presence as the gate
5. update app-server diagnostics to use the shared resolver
6. add regression tests across CLI, core, and app-server

## Acceptance Criteria

- A fresh `mcodex` home with pooled accounts registered into `main-pool` starts
  in pooled mode without requiring manual `config.toml` edits.
- `accounts status` on that home reports a real effective pool and distinguishes
  runtime pools from configured policy pools.
- CLI, core, and app-server agree on effective-pool resolution.
- `config.toml` still carries policy fields and remains merge-compatible with
  upstream shape.
- No DB-only rewrite is required.
