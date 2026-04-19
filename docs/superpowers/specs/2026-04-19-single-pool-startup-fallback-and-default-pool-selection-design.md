# Single-Pool Startup Fallback And Default-Pool Selection Design

This document defines how pooled startup should behave when account pools are
already visible to the account-pool backend but no explicit default pool has
been configured.

It is an additive follow-up to:

- `docs/superpowers/specs/2026-04-16-account-pool-selection-state-policy-design.md`
- `docs/superpowers/specs/2026-04-14-pooled-only-startup-notice-design.md`
- `docs/superpowers/specs/2026-04-16-remote-account-pool-contract-v0-design.md`

## Summary

Today pooled startup remains awkward in two common cases:

- a home has exactly one visible pool, but neither `config.toml` nor the
  persisted startup-selection state names it as the default
- a home has multiple visible pools, but no explicit default has been
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
- expose the new resolution, startup availability, and structured issue states
  consistently in CLI, TUI, core, and remote-facing protocol output

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
  core runtime, and remote protocol output.
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

### 1. Resolver outputs

Startup resolution should produce separate outputs instead of encoding every
state into `effective_pool_id`:

- `effective_pool_id`: the pool that startup may use for pooled account
  selection, or `None` when no usable pool has been resolved
- `effective_pool_resolution_source`: where that pool came from
- `startup_availability`: whether pooled startup can proceed, is paused, or is
  blocked by a selection problem
- `startup_resolution_issue`: an optional structured reason for blocked or
  degraded resolution

The availability values should be:

- `available`: a valid effective pool is resolved and pooled startup is not
  suppressed
- `suppressed`: a valid effective pool is resolved, but durable startup
  suppression is active
- `multiplePoolsRequireDefault`: pooled inventory exists, but multiple pools
  are visible and no explicit default was selected
- `invalidExplicitDefault`: an explicit default source names a pool that is not
  visible in the startup pool inventory
- `noEligibleAccount`: a valid effective pool exists, but no account in that
  pool can be selected for a fresh lease
- `unavailable`: no pooled startup surface is visible

TUI and app-server startup decisions should use `startup_availability`, not a
derived boolean such as "has an effective pool". This is what prevents the
multi-pool/no-default case from falling back to the generic login screen.
Any compatibility helper that still exposes `pooled_applicable` should derive
it from availability: all values except `unavailable` represent a pooled
startup surface, even when that surface is blocked and cannot acquire a lease.

### 2. Effective-pool precedence

The effective-pool precedence should become:

1. process-local override such as `--account-pool`
2. `config.toml` `accounts.default_pool`
3. persisted local default pool in startup-selection state
4. single backend-visible pool fallback
5. no effective pool

This precedence is shared across local CLI, local TUI startup, and remote
protocol surfaces.

The design keeps explicit config higher priority than the state-backed default.
That preserves the current operator-owned semantics of `config.toml` while
still giving users a productized local preference mechanism.

### 3. Backend-neutral pool inventory

The resolver must not hard-code local SQLite membership as the abstract source
of pool existence. It should consume a backend-neutral startup pool inventory.

Implementation should introduce a small backend-facing shape, for example
`StartupPoolInventory`, containing `StartupPoolCandidate` rows with:

- `pool_id`
- optional display label
- optional status suitable for user-facing selection guidance

The exact type names can follow the crate's local conventions, but the
abstraction should make the resolver depend on candidate pools rather than on
local membership tables.

For local backends, the inventory is derived from registered account
membership:

- a pool is visible if at least one registered account belongs to it
- disabled or unhealthy accounts still make the pool visible
- config-only policy entries under `accounts.pools` do not make a pool visible
  by themselves
- account eligibility is evaluated after the pool is resolved

For remote backends, the inventory comes from the remote readable control
plane or startup catalog:

- a remote pool can be visible even when individual account membership is not
  stored locally
- if a remote backend cannot expose a pool inventory yet, it must not invent a
  local-only single-pool fallback; it should report `unavailable` or the
  server-provided startup state until the catalog exists

Pool-count decisions use visible pools, not eligible accounts. A disabled-only
or temporarily exhausted pool can still be a valid pool, but it should surface
`noEligibleAccount` instead of being treated as an invalid default.

### 4. Single visible pool fallback

If all of the following are true:

- no process-local override is present
- `accounts.default_pool` is absent
- persisted `default_pool_id` is absent
- exactly one pool is visible in the backend startup inventory

then startup resolution should treat that pool as the effective pool for the
current read.

This fallback is intentionally read-only:

- it does not write `config.toml`
- it does not write startup-selection state
- it does not clear or modify preferred account state

The fallback should surface a distinct backend-neutral resolution source,
`singleVisiblePool`, instead of pretending that the pool came from config or
persisted state.

If durable suppression is active, single-pool fallback still resolves the pool,
but `startup_availability` becomes `suppressed`. The fallback source is still
visible in status output so users can understand what would resume.

### 5. Explicit default validation

If an explicit default source exists but names a pool that is not visible in
the backend startup inventory, startup must not silently fall back to the sole
visible pool.

That applies to:

- `accounts.default_pool`
- persisted `default_pool_id`
- process-local override

In those cases the explicit source remains authoritative, `effective_pool_id`
should be `None`, and `startup_availability` should be
`invalidExplicitDefault`. The structured issue should identify the source and
the requested pool id.

This preserves debuggability and avoids surprising users by silently changing
their declared default.

Validity is only about whether the pool is visible in startup inventory:

- a config-only pool with no registered local membership is not a visible local
  pool
- an empty or disabled-only visible pool is valid but may produce
  `noEligibleAccount`
- remote catalog pools are visible even if local membership rows do not exist

### 6. Suppression overlay

Suppression is an availability overlay, not a pool-selection source.

Rules:

- if a valid effective pool is resolved and `suppressed = true`, availability
  is `suppressed`
- if no valid effective pool is resolved because multiple pools require a
  default, availability remains `multiplePoolsRequireDefault`; choosing a
  default must happen before `accounts resume` can make startup usable
- if an explicit default is invalid, availability remains
  `invalidExplicitDefault`; fixing the explicit source takes priority over
  resuming
- `accounts pool default set` does not clear suppression; `accounts resume`
  remains the command that resumes pooled startup

This precedence means a user with multiple visible pools, no default, and
`suppressed = true` first sees the default-selection blocker. After selecting a
default, startup can then show the existing paused notice until the user runs
`accounts resume`.

### 7. Decision table

| Override | Config default | Persisted default | Visible pools | Suppressed | Result |
| --- | --- | --- | --- | --- | --- |
| valid | any | any | any | false | `effective_pool_id = override`, source `override`, availability `available` or `noEligibleAccount` |
| valid | any | any | any | true | `effective_pool_id = override`, source `override`, availability `suppressed` |
| invalid | any | any | any | any | no effective pool, source `override`, availability `invalidExplicitDefault`, issue `overridePoolUnavailable` |
| none | valid | any | any | false | `effective_pool_id = config`, source `configDefault`, availability `available` or `noEligibleAccount` |
| none | valid | any | any | true | `effective_pool_id = config`, source `configDefault`, availability `suppressed` |
| none | invalid | any | any | any | no effective pool, source `configDefault`, availability `invalidExplicitDefault`, issue `configDefaultPoolUnavailable` |
| none | none | valid | any | false | `effective_pool_id = persisted`, source `persistedSelection`, availability `available` or `noEligibleAccount` |
| none | none | valid | any | true | `effective_pool_id = persisted`, source `persistedSelection`, availability `suppressed` |
| none | none | invalid | any | any | no effective pool, source `persistedSelection`, availability `invalidExplicitDefault`, issue `persistedDefaultPoolUnavailable` |
| none | none | none | 1 | false | `effective_pool_id = only pool`, source `singleVisiblePool`, availability `available` or `noEligibleAccount` |
| none | none | none | 1 | true | `effective_pool_id = only pool`, source `singleVisiblePool`, availability `suppressed` |
| none | none | none | 2+ | any | no effective pool, source `none`, availability `multiplePoolsRequireDefault`, issue `multiplePoolsRequireDefault` |
| none | none | none | 0 | any | no effective pool, source `none`, availability `unavailable` |

### 8. Explicit CLI for durable local default selection

Add a dedicated CLI surface:

- `mcodex accounts pool default set <POOL_ID>`
- `mcodex accounts pool default clear`

These commands operate on the existing startup-selection state in SQLite.
They do not write `config.toml`.

These mutation commands should reject top-level `--account-pool`. A
process-local override is useful for read and execution commands, but allowing
it to coexist with persistent default mutation would make post-write messaging
ambiguous.

#### `default set`

`mcodex accounts pool default set <POOL_ID>` should:

- validate that `<POOL_ID>` is visible in the backend startup inventory
- write `default_pool_id = <POOL_ID>`
- preserve `suppressed`
- clear `preferred_account_id` only when the state-backed default source is
  active after the write

The preferred-account reset is a pool-scoped preference reset. It prevents a
preferred account from the old effective pool from blocking automatic selection
in the new effective pool. If `config.toml` still controls the effective pool,
the command must not clear the currently active preferred account as a hidden
side effect.

If `config.toml` also defines `accounts.default_pool`, the command should still
persist the local default but report that the effective pool remains controlled
by config until that config value is removed or changed.

If startup is suppressed, the command should report that the local default was
set but pooled startup remains paused until `mcodex accounts resume` is run.

The command has two same-pool outcomes:

- if the persisted default is already `<POOL_ID>` and no preferred-account
  reset is required, print that no state change was needed
- if the persisted default is already `<POOL_ID>` but the state-backed default
  source is active and `preferred_account_id` is present, clear the preferred
  account and print that the preferred startup selection was reset

#### `default clear`

`mcodex accounts pool default clear` should:

- clear persisted `default_pool_id`
- preserve `suppressed`
- clear `preferred_account_id` only when the state-backed default source was
  active before the clear

This keeps "default pool selection" separate from "resume pooled startup".
Users should continue to use the existing `accounts resume` command when they
intend to clear durable suppression.

The command is idempotent. Clearing an already absent persisted default
succeeds and prints that no state change was needed.

#### CLI output

The first slice can keep these commands text-only. Structured inspection should
continue to happen through `accounts status --json`.

Required text messages:

- successful set: print the persisted default pool id
- successful clear: print that the persisted default was cleared
- config-controlled set: print that config still controls the effective pool
- suppressed set or clear: print that pooled startup remains paused and name
  `mcodex accounts resume`
- missing pool: fail with a clear error naming the requested pool
- rejected `--account-pool`: fail with a clear error saying persistent default
  mutation cannot be combined with a process-local override

#### Preferred-account reset matrix

For the default mutation commands, "state-backed default source is active"
means there is no configured default in `config.toml` and no process-local
override participating in the command. Validity does not matter for this
specific reset rule: an invalid persisted default still counts as the active
state-backed source when no higher-priority source exists.

| Command state | Preferred-account behavior |
| --- | --- |
| `default set` with no configured default | clear `preferred_account_id` |
| `default set` with configured default present, valid or invalid | preserve `preferred_account_id` |
| `default clear` when persisted default exists and no configured default exists | clear `preferred_account_id` |
| `default clear` when configured default exists, valid or invalid | preserve `preferred_account_id` |
| `default clear` when no persisted default exists | preserve `preferred_account_id` |
| any `default set|clear` with `--account-pool` | reject before mutation |

This rule avoids hidden mutations under config-controlled startup while still
clearing stale pool-scoped preferences when the state-backed default is the
selection source being changed.

#### Registration interaction

The single-pool fallback remains read-only even when registration commands run.
Registration must not persist `default_pool_id` merely because startup resolved
through `singleVisiblePool`.

Startup validation is not the registration target validator. Mutation commands
that create or repair pool membership may name a pool that is not yet visible
in startup inventory.

Existing first-default bootstrap behavior should be narrowed to explicit
registration into an otherwise empty pool inventory:

- `accounts add --account-pool <POOL_ID>` may target a non-visible pool and
  makes that pool visible only after successful registration writes membership
- `import-legacy --pool <POOL_ID>` follows the same creation-target rule
- if registration supports implicit targeting from `accounts.default_pool`, it
  may also use that configured pool as a creation target even when it is not
  yet visible; this is a mutation-time convenience, not evidence that startup
  should treat the pool as valid before membership exists
- `accounts add --account-pool <POOL_ID>` may persist `default_pool_id` only
  after successful registration, and only when no configured default exists,
  no persisted default exists, and the visible pool inventory before the
  command is empty
- `import-legacy --pool <POOL_ID>` follows the same post-success persistence
  rule
- registering into a second or later visible pool does not auto-persist or
  auto-switch the default; the user should run
  `mcodex accounts pool default set <POOL_ID>` when they want a durable
  default

This preserves the first-account UX while preventing single-pool fallback from
silently becoming durable state or switching to a newly added second pool.

### 9. Keep startup-selection concerns separate

Existing command responsibilities remain distinct:

- `accounts pool default set|clear` manages the durable default pool
- `accounts switch <ACCOUNT_ID>` manages preferred account selection inside the
  current effective pool
- `accounts resume` clears durable suppression

The design explicitly avoids one command that mixes default-pool choice,
preferred-account choice, and suppression state.

`accounts switch <ACCOUNT_ID>` under `singleVisiblePool` should remain a
preferred-account operation. It should not persist `default_pool_id`. If a
second pool is later added, users still need
`mcodex accounts pool default set <POOL_ID>` to make the pool choice durable.

### 10. Multi-pool without default is a distinct startup condition

When multiple pools are visible but no effective default can be resolved,
interactive startup should not fall back to the generic login screen.

Instead it should enter a dedicated pooled notice that explains:

- pooled access exists
- multiple pools are visible
- no default pool is configured
- the user should run
  `mcodex accounts pool default set <POOL_ID>` to make pooled startup durable

This notice should remain separate from the existing login screen so that
"needs shared login" and "needs pooled default selection" are not conflated.

The notice may still offer a way to continue into shared-login onboarding, but
its primary explanation is about pool selection rather than authentication.

### 11. TUI notice behavior

The multi-pool/default-required notice should be a blocking pooled-startup
notice. It must not offer "continue with pooled access" because there is no
effective pool.

Recommended behavior:

- `Enter`: open the existing shared-login onboarding flow
- `L`: open the existing shared-login onboarding flow
- `N`: not available; this notice must not be hidden because it represents an
  unresolved startup requirement
- `Esc` or the existing terminal quit path: exit so the user can run the CLI
  default-selection command from the shell

The notice should print the exact CLI command shape and enough pool context for
the user to pick the right pool. That context must come from the backend
startup inventory and, for remote clients, from protocol fields described
below. It does not need to implement an interactive pool picker in this slice.

Invalid explicit defaults should use the same blocking notice shell with
source-specific copy:

- config default invalid: tell the user to fix or remove
  `accounts.default_pool`
- persisted default invalid: tell the user to run
  `mcodex accounts pool default set <POOL_ID>` or
  `mcodex accounts pool default clear`
- override invalid: tell the user to correct the process-local override

Suppressed startup with a valid effective pool continues to use the existing
paused notice.

### 12. Reuse the existing onboarding notice shell

The new multi-pool-without-default screen should be implemented as another
pooled-access notice kind within the current onboarding shell rather than as a
new onboarding subsystem.

This keeps the UX structure consistent with the existing:

- `PooledOnlyNotice`
- `PooledAccessPausedNotice`

and limits merge risk in `tui/src/onboarding`.

### 13. Structured observability for startup resolution

The startup-resolution model should become more explicit for both text and JSON
surfaces.

#### Resolution source

Add `singleVisiblePool` to the effective-pool resolution source enum and
propagate it through:

- CLI output
- app-server protocol output
- core runtime diagnostics
- remote TUI startup probe handling

#### Availability and issue shape

Add typed internal enums for availability and issue state. Convert them to
camelCase strings only at CLI and protocol boundaries.

Issue types:

- `multiplePoolsRequireDefault`
- `overridePoolUnavailable`
- `configDefaultPoolUnavailable`
- `persistedDefaultPoolUnavailable`

Issue fields:

- `type`: one of the issue type strings above
- `source`: `override`, `configDefault`, `persistedSelection`, or `none`
- `poolId`: the requested pool id when one exists
- `candidatePoolCount`: visible pool count when relevant
- `candidatePools`: candidate pools when relevant to user selection
- `message`: optional user-facing diagnostic text

The field should be additive and should not repurpose existing JSON fields.
For `multiplePoolsRequireDefault`, `candidatePools` should contain the complete
visible candidate list for the current read, sorted by `poolId` ascending, and
`candidatePoolCount` should equal the full list length. The first slice should
not introduce partial pagination or list truncation for this field.

#### App-server v2 fields

Add the following fields to `AccountLeaseReadResponse`:

- `startupAvailability: AccountStartupAvailability | null`
- `startupResolutionIssue: AccountStartupResolutionIssue | null`

Add the same fields to `AccountLeaseUpdatedNotification` so remote clients can
update visible startup state without issuing an immediate read after every
notification.

Define `AccountStartupAvailability` as a closed v2 enum/string union with
these camelCase values:

- `available`
- `suppressed`
- `multiplePoolsRequireDefault`
- `invalidExplicitDefault`
- `noEligibleAccount`
- `unavailable`

Define `AccountStartupResolutionIssue` as a v2 exported type with
`#[serde(rename_all = "camelCase")]` and `#[ts(export_to = "v2/")]`. The
fields above should not use `skip_serializing_if`; they should follow v2
response/notification conventions.

Define `AccountStartupCandidatePool` as a v2 exported type with:

- `poolId: string`
- `displayName: string | null`
- `status: string | null`

For `multiplePoolsRequireDefault`, `startupResolutionIssue.candidatePools`
must include enough candidate pool rows for the TUI notice to show concrete
pool ids instead of a placeholder. For invalid explicit defaults,
`candidatePools` may be present to show alternatives but is not required.

#### `accounts status`

`accounts status` should explain the startup state in human-readable form. In
particular it should distinguish:

- config default selected
- persisted default selected
- single visible pool fallback selected
- multiple visible pools require default selection
- configured default is invalid
- persisted default is invalid
- process override is invalid
- startup is suppressed despite a resolved effective pool
- resolved pool has no eligible account

The existing `poolObservability` addition remains useful, but it is not a
replacement for startup-resolution diagnostics because there may be no
effective pool to observe yet.

### 14. Remote compatibility

Remote backends should consume and return the same startup-resolution concepts
instead of re-deriving pool-selection semantics in the TUI.

That means:

- the shared startup model owns resolution source, availability, and issue
  shape
- app-server protocol surfaces should expose the additive fields needed by
  remote clients
- the TUI should treat remote startup as a consumer of those fields, not the
  place where multi-pool/no-default policy is invented
- single-pool fallback depends on backend-visible pool inventory, not local
  SQLite membership

This keeps the local and remote models aligned and avoids later rework when a
remote pool authority becomes primary.

## Testing Strategy

### State and account-pool tests

Add focused tests for:

- no explicit default plus exactly one visible pool resolves
  `singleVisiblePool`
- no explicit default plus multiple visible pools yields no effective pool and
  `multiplePoolsRequireDefault`
- invalid config default does not fall back to the sole visible pool
- invalid persisted default does not fall back to the sole visible pool
- visible disabled-only or exhausted pools are valid pools but surface
  `noEligibleAccount`
- suppression overlays a valid effective pool as `suppressed`
- suppression does not hide `multiplePoolsRequireDefault` or
  `invalidExplicitDefault`

### CLI tests

Add focused tests for:

- `accounts pool default set`
- `accounts pool default clear`
- rejection of `--account-pool` with persistent default mutation
- config-controlled set preserving active preferred-account state
- suppressed set/clear preserving suppression and printing resume guidance
- preferred-account reset matrix cases for config-controlled, persisted-valid,
  persisted-invalid, and no-op clear states
- registration bootstrap only persisting the first explicit pool when the
  pre-command visible inventory is empty
- registration bootstrap allowing a non-visible creation target for explicit
  `--account-pool` and implicit configured-default repair paths
- registration into a second visible pool not auto-persisting or auto-switching
  the default
- same-pool `default set` with an existing preferred account clearing the
  preferred startup selection instead of reporting a pure no-op
- `accounts status` text output for each new resolution state
- `accounts status --json` additive fields for new resolution and warning
- observability commands when there is one visible pool, multiple visible
  pools, an invalid explicit target, and explicit `--pool` target selection

### Core runtime tests

Add focused tests proving actual turn-time behavior, not only status output:

- a state-only home with one visible pool and no configured/persisted default
  acquires a pooled lease
- multi-pool-without-default does not acquire a lease and exposes the
  structured blocker
- invalid explicit defaults do not fall back to another visible pool
- suppressed single-pool fallback does not acquire a lease until resumed

### TUI tests

Add snapshot and behavior coverage for:

- the new multi-pool default-selection notice
- invalid explicit default notice copy
- startup prompt resolution for multi-pool-without-default
- single-pool fallback continuing through the pooled startup path rather than
  the login path
- suppressed single-pool fallback using the paused notice
- no hide action for default-selection blockers

### App-server tests

Add protocol and server coverage for:

- new resolution source serialization
- `startupAvailability`
- `startupResolutionIssue`
- `AccountStartupCandidatePool` serialization for
  `multiplePoolsRequireDefault`
- `candidatePools` completeness and deterministic `poolId` ordering
- remote startup responses that represent single-pool fallback and
  multi-pool-without-default distinctly
- notification conversion preserving availability and issue fields
- schema regeneration with `just write-app-server-schema`, plus
  `just write-app-server-schema --experimental` if experimental fixtures are
  affected

## Migration And Compatibility

This slice should remain additive.

- No config migration is required.
- No startup action should silently rewrite user config.
- The single-pool fallback is a read-time semantic enhancement, not a schema
  migration.
- The explicit default-pool commands write the existing startup-selection
  state shape and preserve suppression.

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
- the minimal core runtime touchpoint that prepares fresh pooled leases
- CLI pool-default commands and status output
- app-server protocol projection of additive startup metadata
- one additional onboarding notice in the TUI

That keeps the behavior coherent while limiting churn in unrelated areas of
the codebase.
