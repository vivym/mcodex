# Multi-Account Pool And Automatic Failover Design

This document describes a merge-friendly design for adding multi-account management,
rate-limit monitoring, and automatic account switching to this fork of Codex.

The design is intentionally shaped so local multi-account support can ship first, while leaving a
clean path for a future remote account-pool service used by a team or company.

## Summary

The recommended design introduces a new account-pool strategy layer instead of rewriting the
existing single-account auth flow in place.

Key decisions:

- Add an account-pool strategy layer, tentatively named `codex-account-pool`, that owns
  account-pool policy, lease management, rate-limit state, and account selection.
- Keep `codex-login` focused on single-account auth material, refresh, and compatibility with the
  current auth model.
- Do not use shared mutable `auth.json` as the request-path source of truth for leased accounts.
  Leased request paths must use process-local auth materialization. Shared `auth.json` remains a
  legacy compatibility surface for single-account flows only.
- Use runtime-instance-scoped sticky leases: one Codex runtime instance keeps one active account
  until it is near its limit, exhausted, unauthorized, manually switched, or its lease expires.
- Default to `exclusive` allocation so multiple local Codex instances do not silently share the
  same account.
- Implement the local backend on top of atomic SQLite-backed lease state with fencing tokens.
- Implement automatic retry through an explicit turn replay guard, not an informal "no side
  effects yet" check.
- Support both local and future remote pool backends through a common backend interface.
- Ship the first version with local backend support, ChatGPT-first account support, and minimal
  TUI changes.

## Goals

- Support multiple accounts in a single Codex installation.
- Monitor rate limits and usage-limit signals per account.
- Automatically switch accounts before hard exhaustion when possible.
- Automatically fail over after `usage_limit_reached` only when the current turn is still
  replayable and transport commit state is known-safe.
- Avoid introducing a fork-only `auth.json` format that will make future upstream merges painful.
- Support multiple concurrent local Codex processes without accidentally assigning the same account
  by default.
- Preserve a clean path to a future remote account-pool service for company-managed accounts.
- Keep the first implementation small enough to land incrementally.

## Non-Goals

- Do not bypass upstream service limits or hide aggregate usage.
- Do not implement a complex TUI account-management UI in the first version.
- Do not ship a remote account-pool backend in the first version.
- Do not require cross-project or cross-thread sticky assignment in the first version.
- Do not depend on rewriting all existing auth, app-server, or TUI flows at once.

## Constraints

- This repo is a fork and should remain easy to rebase onto upstream Codex.
- Upstream already evolves auth, account, and rate-limit flows. The design should minimize edits
  to high-churn code in `codex-core`, app-server account handling, and large TUI files.
- The design must support both ChatGPT accounts and API keys in the long term.
- The design must allow same-pool context reuse across accounts only after explicit user choice.
- Automatic retry after switching accounts must not duplicate side-effecting tool work.
- Multiple local Codex processes may share one `CODEX_HOME`, so correctness must not depend on a
  single shared mutable "current account" file.

## Recommended Architecture

### 1. New strategy layer: `codex-account-pool`

Add a new strategy layer, tentatively named `codex-account-pool`, that owns:

- account registry metadata
- pool definitions and policy evaluation
- lease acquisition and release
- sticky active-account selection per runtime instance
- rate-limit state and cooldown tracking
- automatic failover decisions

This crate should not own low-level token refresh logic for a single account. That work remains in
`codex-login`.

This layer should not assume the existing `codex-account` crate is the neutral base for
multi-account orchestration. Today `codex-account` is already auth-bound and backend-bound.

Therefore:

- `codex-account` should remain the current auth-bound helper crate unless and until it is
  deliberately refactored
- `codex-account-pool` should depend directly on the auth and state layers it actually needs
- if shared account-domain primitives become necessary, they should be extracted into a new small
  auth-agnostic crate or module rather than by forcing `codex-account` into that role

### 2. Keep `codex-login` single-account focused

`codex-login` continues to handle:

- reading or persisting the auth material for one account
- refresh-token logic for managed ChatGPT auth
- compatibility with the current auth loading model
- constructing `CodexAuth` for the currently selected account

This keeps the existing auth code understandable and reduces conflicts with upstream changes.

### 3. Use process-local auth materialization for leased requests

Multi-account state must not be encoded by changing `auth.json` into a multi-account container,
and leased request paths must not depend on a shared mutable `auth.json` slot.

Instead:

- account-pool state is stored separately
- each process materializes its selected account into a process-local auth view for request
  execution, ideally by constructing `CodexAuth` directly from `credential_ref` or by using a
  per-instance ephemeral auth store
- shared `auth.json` is retained only as a legacy compatibility surface for single-account flows
  and explicit compatibility commands
- upstream code that assumes one current account can continue to operate inside a single process,
  but multi-process correctness must never depend on a single shared mutable auth file

This is the main merge-friendly choice in the design.

### 4. Separate backend from policy

The account-pool layer should depend on a backend trait rather than assuming local file storage.

Initial backend:

- `LocalPoolBackend`

Future backend:

- `RemotePoolBackend`

The backend abstraction is required from the start so a company-managed remote account-pool
service can be added later without redesigning the strategy layer.

### 5. Reuse the existing state runtime

The local backend should extend the existing `codex-state::StateRuntime` SQLite runtime instead of
introducing a parallel SQLite runtime under the same `sqlite_home`.

That means:

- account-pool tables and migrations live in the same state database runtime already initialized by
  app-server and core
- account-pool should reuse the existing DB initialization and migration path
- v1 should not create a second independent SQLite bootstrap path for account-pool state

## High-Level Flow

### Normal startup and steady state

1. Codex starts and creates a runtime-instance `instance_id`.
2. The account-pool layer loads pool policy and runtime state.
3. The runtime instance obtains one sticky active lease for its pool, including a fencing token such as
   `lease_epoch`.
4. The runtime instance materializes a process-local auth view for the selected account.
5. Requests keep using the same leased account.
6. Rate-limit snapshots and usage-limit signals update runtime state for that lease.

### Proactive switching

When the active account crosses a configured threshold, such as 85 or 90 percent:

- keep the current request running
- mark the account as near limit
- select a different account for the next turn
- place the old account into cooldown when appropriate

This avoids request-time thrashing and keeps switching behavior closer to "runtime-instance rotation" than
"per-request scheduling."

### Hard-failure switching

When the current account hits `usage_limit_reached`:

- mark it exhausted or cooling down
- if the turn is still in the replayable state and remote commit status is known-safe, switch
  accounts, rebuild turn transport state, and retry once
- if the turn is no longer replayable, do not replay the turn; only switch future turns

This preserves automation without risking duplicate shell, MCP, or patch operations.

### Rotation and local thread continuity

In v1, account rotation should not create a new local Codex thread by default.

Instead:

- local session history remains attached to the same local thread
- when rotation requires remote-context reset, the old remote conversation or session id is
  discarded
- cross-account rotation that resets remote context must mint a new remote session identity that is
  distinct from the stable local thread id
- the next turn is executed on the new account using fresh remote conversation state plus normal
  local-history reinjection
- if a transport cannot safely support this reset behavior, automatic cross-account rotation should
  be disabled for that transport in v1

## Sticky Lease Model

The allocation unit in v1 is a Codex runtime instance, not a single request.

For this spec, a "runtime instance" means:

- one CLI or TUI process, or
- one app-server process handling one pooled-selection context

Threads inside the same runtime instance share that one pooled lease context in v1. Supporting
multiple independent pooled leases inside one process is deferred.

Default behavior:

- one Codex runtime instance holds one sticky active lease
- that lease remains active until the account is no longer suitable
- switching is exceptional, not routine

Default switching triggers:

- proactive rate-limit threshold reached
- `usage_limit_reached`
- unrecoverable auth failure
- lease expiration or lease revocation
- manual user switch

Default safeguards:

- `exclusive` allocation mode
- minimum switch interval
- cooldown after exhausting an account
- no immediate switch-back to the just-replaced account

This model reduces account churn and is less likely to look abnormal from a risk or abuse
perspective than per-turn redistribution.

## Local And Remote Backend Model

### Backend responsibilities

The backend owns durable source-of-truth behavior for:

- account discovery and metadata retrieval
- credential lookup
- lease creation, heartbeat, and release
- runtime state persistence where needed

### Policy-layer responsibilities

The policy layer owns:

- choosing the active account for the runtime instance
- applying threshold rules
- cooldown logic
- deciding when a switch is allowed
- deciding when a failed turn is safe to retry

The backend remains authoritative for specific lease lifecycle decisions:

- `acquire_lease` grants or denies a concrete lease
- `renew_lease` confirms continued ownership or revocation
- `release_lease` finalizes release
- remote backends may override client-side cooldown or rotation hints
- remote credential renew or revalidate responses are authoritative for whether a leased credential
  remains usable

### Why this split matters

For local mode, the backend may store durable credentials in file or keyring.

For remote mode, the backend may instead:

- acquire an account lease from a company service
- receive short-lived access tokens instead of refresh tokens
- avoid persisting long-lived account credentials on the developer machine

The same policy layer should be able to operate in both cases.

Authority differs by backend:

- local backend is authoritative for local lease state and cooldown decisions
- remote backend is authoritative for lease ownership, revocation, cooldown, and lease renewal
  outcomes
- client-side policy may request proactive rotation, but remote policy may still deny or redirect
  that request
- in remote mode, client-side cooldown and eligibility caches are advisory only; acquire, renew,
  rotate, and revoke decisions are finalized by backend responses

## Data Model

The design separates static config, account metadata, secrets, and runtime state.

### 1. Static config in `config.toml`

`config.toml` should hold policy and backend configuration only.

Example shape:

```toml
[accounts]
backend = "local"
default_pool = "team-main"
proactive_switch_threshold_percent = 85
allocation_mode = "exclusive"
lease_ttl_secs = 900
heartbeat_interval_secs = 60
min_switch_interval_secs = 300

[accounts.pools.team-main]
allow_context_reuse = true
account_kinds = ["chatgpt"]
```

This area should define:

- backend type
- default pool
- thresholds
- allocation mode
- lease timing
- optional pool-level policy

Default-pool resolution in v1 should be explicit and ordered:

1. process-level override such as `--account-pool <pool>`
2. `accounts.default_pool` from config when present
3. synthesized migrated default pool id persisted in account-pool runtime state when config does
   not define a default
4. otherwise no effective default pool exists and startup must fail clearly

In v1, the combination of:

- manually assigning accounts into the same pool
- setting that pool's `allow_context_reuse = true`

is the explicit opt-in boundary for cross-account context reuse. There is no implicit
cross-account reuse outside that boundary.

When `allow_context_reuse = false`, any manual or automatic rotation to a different account must
rebuild transport state with fresh remote conversation or thread state. Remote conversation ids,
thread ids, and equivalent session handles must not be carried across accounts in that case.

Even when `allow_context_reuse = true`, context reuse is allowed only when all of the following are
true:

- source and target accounts are in the same pool
- both accounts have `explicit_context_reuse_consent = true`
- both accounts are compatible for context reuse under the same backend capability set
- both accounts resolve to the same effective workspace or equivalent backend boundary
- the transport or backend can attest that the current remote conversation or thread handle is
  portable across those account identities

If any of those checks fail, the system may still rotate accounts, but it must reset remote
conversation state instead of carrying remote ids across accounts.

### 2. Account registry metadata

The account registry should store metadata without assuming that credentials live in the same
record.

Suggested fields:

- `account_id`
- `account_kind`
- `pool_id`
- `label`
- `workspace_id`
- `email`
- `plan_type`
- `enabled`
- `priority`
- `source`
- `credential_ref`
- `explicit_context_reuse_consent`

Suggested account kinds:

- `chatgpt_managed`
- `chatgpt_external`
- `api_key`
- `remote_lease`

### 3. Credential storage

Credentials should remain backend-specific.

Local examples:

- managed ChatGPT refresh-token state
- API keys
- local external ChatGPT auth tokens when appropriate

Remote examples:

- short-lived lease credentials
- short-lived access tokens
- no durable refresh-token storage on the developer machine

### 4. Runtime state

Runtime state should include:

- latest rate-limit snapshot by account
- latest account health state
- latest authoritative health event sequence
- last selected time
- hard-limit timestamp
- next eligible timestamp
- current lease holder
- lease expiration and heartbeat
- selection suppression state for default startup
- durable preferred-account override for future runtime instances
- resolved effective default pool id for startup
- last switch reason

Suggested health states:

- `healthy`
- `near_limit`
- `exhausted`
- `cooling_down`
- `unavailable`

Durable startup-selection state is installation-scoped for a shared `CODEX_HOME` in v1.

That means:

- durable suppression and preferred-account overrides affect future runtime instances sharing the
  same local installation state
- they do not mutate already-running turns except where an API explicitly revokes the current
  process-local lease
- app-server and CLI management commands operate on that same installation-scoped startup state

### 5. Local backend storage layout

The local backend should not use ad hoc JSON files for lease state. It should use SQLite under the
existing `codex-state::StateRuntime` umbrella so multi-process lease acquisition can be atomic and
schema versioned.

Suggested durable local tables:

- `account_registry`
- `account_pool_membership`
- `account_runtime_state`
- `account_leases`
- `account_backend_metadata`

Credentials should remain out of SQLite where practical:

- API keys and long-lived secrets stay in file or keyring storage keyed by `credential_ref`
- SQLite stores references, lease state, and non-secret runtime metadata

The local backend must define schema versioning and migration behavior for upgrading from a legacy
single-account installation.

Migration rules for v1:

- migration triggers the first time an account-pool aware runtime opens state and finds no
  account-pool schema version recorded
- migration must not rewrite or delete legacy `auth.json`, keyring entries, or existing login
  state
- if legacy auth exists and no explicit pool config is present, migration synthesizes a durable
  default pool record such as `legacy-default` inside account-pool state
- that synthesized pool id also becomes the persisted effective default pool for startup
  resolution until the user explicitly configures another default
- the imported legacy account becomes the preferred default entry for future runtime instances until the
  user explicitly changes pool or account selection
- if migration fails, no partial account-pool state should become visible; legacy single-account
  behavior remains the fallback on the next startup
- migration must be idempotent across repeated startups

## Lease Model

Every active assignment should be represented by a lease.

Suggested lease fields:

- `lease_id`
- `account_id`
- `pool_id`
- `holder_instance_id`
- `lease_epoch`
- `leased_at`
- `expires_at`
- `last_heartbeat_at`
- `allocation_mode`

The first version should support:

- `exclusive`

The design should leave room for future:

- `shared`
- `max_concurrent_leases = N`

Default behavior must remain `exclusive`.

### Local acquisition and fencing

The local backend must implement exclusive leases with atomic compare-and-set semantics.

Required behavior:

- lease acquisition occurs inside a transaction
- a reclaimed or replaced lease increments `lease_epoch`
- heartbeat and release update only when `lease_id + lease_epoch` still match
- all mutating runtime-state reports for that lease update only when `lease_id + lease_epoch`
  still match
- a process that loses the lease must stop starting new work with its previously materialized auth
  immediately

Every turn should carry the current `lease_epoch` so stale holders can be detected before request
execution, again before any retry or auth refresh path, and again after any refresh or revalidation
round-trip before refreshed credentials are persisted or reused.

Late `report_rate_limits`, `report_usage_limit_reached`, and `report_unauthorized` calls from a
stale holder must be ignored for selection state purposes. At most they may be recorded as
best-effort diagnostics separate from authoritative runtime state.

Within one still-valid lease epoch, runtime-state writes must also be monotonic:

- every authoritative `report_*` update carries a lease-local event sequence or equivalent
  strictly monotonic ordering key
- newer events may advance health from healthier to less healthy states
- a later rate-limit snapshot or lower-severity event must not overwrite `cooling_down`,
  `exhausted`, or `unavailable` back to a healthier state within the same lease epoch
- recovery to a healthier state requires an explicit recovery event such as successful auth
  recovery, cooldown expiry, or a new lease epoch

For v1 local exclusivity, the hard guarantee is "no new work after fence failure." A long-running
turn that began while the holder was valid may finish if it cannot be cancelled cleanly, but it
must not start retries, refresh follow-up work, or additional turns after fence failure is
detected.

Heartbeat and TTL behavior must also be explicit:

- heartbeats run independently of individual turns
- a new turn must not start unless the active lease has a minimum remaining TTL safety margin
- if the safety margin is too small, the process must renew or reacquire before starting the turn
- once `expires_at` has passed, that lease must be treated as dead and must not be revived by a
  late renew or heartbeat
- local config validation must ensure `lease_ttl_secs` is strictly greater than
  `heartbeat_interval_secs`, and the required pre-turn safety margin must be strictly less than the
  lease TTL

## Core Interfaces

The account-pool layer should expose lease-oriented APIs rather than a simple "give me an account"
API.

Suggested interface shape:

- `ensure_active_lease(session_context) -> Lease`
- `materialize_turn_auth(lease) -> ProcessLocalAuthHandle`
- `report_rate_limits(lease, snapshot)`
- `report_usage_limit_reached(lease, error)`
- `report_unauthorized(lease)`
- `release_lease(lease)`
- `heartbeat_lease(lease)`

The active lease is runtime-instance-scoped and sticky. `ensure_active_lease` should typically return the
current active lease unless policy says a rotation is required.

`ProcessLocalAuthHandle` must not depend on shared mutable `auth.json`. It is the turn-local auth
materialization that request execution uses.

`ProcessLocalAuthHandle` must be an immutable snapshot bound to a specific lease epoch and turn.
Refresh or rotation must create a new handle and rebuild transport state; the auth used by an
in-flight turn must never mutate underneath that turn.

Turn execution must also carry the current `remote_context_generation` for its transport state.

## Integration Points

### `codex-core`

`codex-core` should gain concentrated integration points, but this is not just a thin wrapper over
the current single-auth access pattern.

- before beginning a model turn, ensure there is an active lease and materialize process-local auth
  for that turn
- update account-pool state when rate-limit snapshots arrive
- update account-pool state when usage-limit or unauthorized errors arrive
- attempt one automatic retry only when the turn is still replayable
- on account rotation, rebuild model-client or transport state from the new lease instead of
  mutating shared auth underneath an existing client session
- when rotating across accounts in a pool whose `allow_context_reuse = false`, rebuild transport
  state from a fresh remote conversation or thread context rather than carrying prior remote ids
- when rotating across accounts with remote-context reset, mint a new remote session identity
  instead of reusing the local thread id as the outbound remote session identifier
- replace direct active-auth reads on request-execution and account-affecting paths with a
  lease-scoped auth accessor or equivalent adapter
- revalidate `lease_id + lease_epoch` and `remote_context_generation` immediately before every new
  model round-trip, tool dispatch, or other effectful remote step inside a turn
- discard late callbacks or streamed events from abandoned transport generations after rotation,
  retry, or remote-context reset

The goal is to keep pool policy centralized even though auth access itself is currently
cross-cutting.

### `codex-login`

`codex-login` should provide the account-pool layer with the means to:

- materialize the selected local account into process-local auth
- persist or refresh a single account's auth state
- project an account into legacy compatibility storage only through explicit, lease-bound APIs

`codex-login` should be the only owner of compatibility-store writes. No other crate should write
shared auth store state directly.

### App-server

The first version should keep compatibility for existing `account/*` surfaces while adding one new
explicit pooled-status surface.

Existing `account/*` RPCs and notifications remain legacy compatibility operations and continue to
describe only the compatibility single-account auth view.

Any pool-state mutations triggered by legacy compatibility RPCs should route through narrow
account-pool compatibility helpers rather than open-coded logic in legacy app-server handlers.

Mutating legacy `account/*` RPCs remain compatibility-only single-account operations in v1:

- `account/login/*` operates on the installation's default legacy account entry only
- successful legacy login may upsert that default entry and attach it to the current effective
  default pool without changing any other stored accounts
- successful legacy login clears durable default-startup suppression
- `account/logout` clears the current compatibility auth view, revokes any currently active
  process-local pooled lease, and durably suppresses automatic pooled selection for default startup
- after legacy `account/logout`, a legacy client should observe signed-out behavior until an
  explicit new legacy login or explicit pool-aware selection clears that suppression state

These compatibility operations must not silently enumerate, mutate, or rotate through other pooled
accounts behind the caller's back.

To represent pooled status explicitly, v1 should add a new app-server v2 surface:

- `accountLease/read`
- `accountLease/resume`
- `accountLease/updated`

That surface should report the current process-local pooled lease state, including at minimum:

- whether pooled selection is active or durably suppressed
- current `account_id`, `pool_id`, `lease_id`, and `lease_epoch` when a lease exists
- the current switch reason or suppression reason when known
- a monotonic `remote_context_generation`
- `last_remote_context_reset_turn_id` when a reset has occurred in the current session

Existing `account/read` and `account/updated` should remain stable for legacy clients and should
not be overloaded with pooled lease semantics.

`accountLease/resume` clears durable default-startup suppression and removes any durable preferred
account override so the next fresh runtime instance returns to automatic selection from
the effective default pool resolution order.

For a running app-server process, `accountLease/resume` updates durable startup-selection state
immediately and emits `accountLease/updated`, but it does not interrupt an in-flight turn. If no
lease is active, the next turn in that same process may auto-acquire a lease under the resumed
policy.

For future remote pool support, the design should be compatible with existing app-server external
auth flows, especially:

- externally supplied `chatgptAuthTokens`
- refresh requests that already include `previousAccountId`

If pool-aware app-server APIs are needed later, they should be added as new `accounts/*` methods
rather than changing the meaning of existing legacy `account/*` methods.

### TUI

The first version should keep TUI changes intentionally small:

- show current account label or id
- show current pool
- show current health state
- show nearing-limit status
- show "automatically switched account" events with switch reason
- show when rotation reset remote conversation continuity
- show "no accounts available" errors
- show next eligible time when known

The first version should not add a large account-management screen.

## CLI Design

Multi-account management should use a new command namespace rather than extending the existing
single-account `login` and `logout` commands.

Recommended first-version commands:

- `codex accounts add chatgpt`
- `codex accounts add chatgpt --device-auth`
- `codex accounts add api-key`
- `codex accounts list`
- `codex accounts current`
- `codex accounts status`
- `codex accounts resume`
- `codex accounts switch <account>`
- `codex accounts enable <account>`
- `codex accounts disable <account>`
- `codex accounts remove <account>`
- `codex accounts pool list`
- `codex accounts pool assign <account> <pool>`

Compatibility expectations:

- `codex login` remains a legacy single-account convenience path that imports or replaces the
  installation's default active account entry
- `codex logout` remains a legacy "clear active credentials" path until a later deprecation
  window; it must not silently change semantics in v1
- docs and future features should guide users toward `codex accounts`

Legacy `codex login/logout` only operate on that default legacy account entry. They must not add,
remove, enable, disable, or auto-select unrelated pooled accounts.

Legacy `codex login` clears durable default-startup suppression.
Legacy `codex logout` also enables durable default-startup suppression for pooled auto-selection.
Generic future runtime instances should remain signed out until the user explicitly resumes pooled selection
through a pool-aware command.

Pool selection in v1 should be explicit:

- if default-startup suppression is inactive, use the effective default pool resolution order when
  no override is supplied
- support a process-level override such as `--account-pool <pool>` for interactive commands
- for management commands, the effective pool still resolves from the explicit `--account-pool`
  override first and then the default-pool resolution order above, even when startup suppression is
  active
- make `codex accounts current` and `codex accounts status` report durable selection state for a
  fresh runtime instance, not mutate or inspect an already-running runtime-instance lease
- make `codex accounts current` and `codex accounts status` show the effective pool, any durable
  preferred-account override, any durable suppression state, the predicted selected account for a
  fresh runtime instance, health state, switch reason, and next eligible time when known
- `codex accounts resume` clears durable default-startup suppression and removes any durable
  preferred-account override so automatic selection resumes from the effective default pool
  resolution order
- `codex accounts switch <account>` is a durable manual override for future runtime instances only
- `codex accounts switch <account>` clears durable default-startup suppression
- runtime switching of an already-running runtime instance requires an in-process or app-server
  pooled API and is not provided by the one-shot CLI in v1
- `codex accounts switch <account>` only switches within the current effective pool
- switching to an account in another pool must require an explicit pool override such as
  `--account-pool <pool>` instead of implicitly changing pool context

`codex accounts status` should also have a machine-readable form and include per-account
eligibility or ineligibility reasons for the current pool so pool behavior is debuggable without
parsing human-oriented text.

Migrated-install visibility in v1 should also be explicit:

- the synthesized migrated pool is exposed verbatim as `legacy-default` in v1
- `codex accounts list` and `codex accounts status` must mark migrated pools or accounts with a
  `source = migrated` style indicator
- immediately after migration, legacy compatibility views continue to reflect the imported legacy
  account while pooled status views report the synthesized `legacy-default` selection state

## Automatic Retry Rules

Automatic retry after switching accounts is only allowed when the current turn is still
replayable.

Side effects include at least:

- shell commands
- MCP tool calls with external effects
- patch application
- other tool operations that have already committed real work

Retry behavior:

- replayable turn: switch account and retry once
- non-replayable turn: switch only future turns, return a clear message for the current turn

This rule is mandatory to prevent duplicated work.

Replay safety must also account for remote commit state:

- automatic retry is allowed only when the local replay guard is `Replayable` and the transport can
  prove that remote commit state is safe to retry
- if the first attempt may already have created or advanced remote conversation state and commit
  status is unknown, the turn becomes non-replayable in v1
- when retry does proceed after rotation, it must use fresh transport state; if safe reuse of prior
  remote state cannot be proven, retry must use fresh remote conversation state

### Turn replay guard

Automatic retry must be implemented as an explicit turn-scoped state machine, not an informal
"nothing happened yet" check.

Suggested states:

- `Replayable`
- `VisibleOutputCommitted`
- `SideEffectStarted`
- `NotReplayable`

Required transitions:

- the guard flips out of `Replayable` before any effectful tool dispatch is launched
- the guard also flips out of `Replayable` before user-visible assistant output is committed
- `usage_limit_reached` may auto-retry only from `Replayable`
- once the guard leaves `Replayable`, that turn must never be replayed

This is intentionally conservative. A turn that already emitted visible assistant output but did
not yet launch tools is still treated as non-replayable in v1 to avoid duplicated or divergent
transcript output.

## Failure Handling

### No available accounts

If a pool has no eligible account:

- return a clear failure
- do not implicitly cross pools
- include next eligible reset time when known

### Threshold reached

If the current active account crosses the proactive threshold:

- finish the current turn
- mark the account for rotation
- switch before the next turn

### `usage_limit_reached`

- mark the account exhausted or cooling down
- retry once only if the turn is still replayable and remote commit state is known-safe
- otherwise switch only future turns

### Unauthorized or refresh failure

- for local managed auth, attempt the account's normal auth recovery once
- after any local auth refresh round-trip, re-check `lease_id + lease_epoch` before persisting or
  reusing refreshed credentials
- local refresh writes must also serialize per `credential_ref` or use credential-generation CAS so
  concurrent refreshes cannot clobber each other
- if local auth recovery fails permanently, mark the account unavailable and rotate
- for remote leases, first ask the backend to renew or revalidate the leased credential
- remote renew or revalidate responses must be correlated to the specific lease and request nonce
  so stale renewal results are discarded
- only backend-confirmed revocation, lease expiry, or unrecoverable auth failure should mark the
  account unavailable

### Local crash recovery

Leases should use TTL-based recovery:

- heartbeat while the process is alive
- release on clean shutdown
- reclaim after expiration if the process disappears

This only guarantees local exclusivity on the current machine. Cross-machine exclusivity is the
future remote backend's job.

### Remote backend outage

If a remote pool backend is unavailable:

- continue using the current active lease while it remains valid
- if a new lease is required and cannot be acquired, fail clearly
- do not silently fall back to unrelated local accounts

## First-Version Scope

The first implementation should include:

- new account-pool strategy layer independent from the existing auth-bound `codex-account` crate
- local backend
- runtime-instance-scoped sticky leases
- `exclusive` allocation
- ChatGPT-first full support
- proactive threshold switching
- `usage_limit_reached` failover with safe retry rules
- CLI account management under `codex accounts`
- small TUI status surfaces

The first implementation may include API key accounts in the data model and CLI, while deferring
full automatic quota-awareness for API keys until later.

In v1, automatic rotation should only be enabled for homogeneous pools of a single account kind.
Mixed ChatGPT and API-key pools may be represented in config and CLI metadata, but the automatic
selector should reject them until kind-specific quota semantics are implemented. That rejection
must be explicit in CLI, TUI, and logs so users do not mistake representable config for supported
automatic behavior.

If an unsupported mixed-kind pool is configured as the effective default startup pool in v1, that
must be treated as an explicit configuration error for automatic selection rather than silently
falling back or masquerading as a transient no-eligible-account case.

If a mixed-kind pool has a durable preferred-account override or explicit account selection for one
concrete account, startup may proceed in manual-only mode on that pinned account. In that mode:

- automatic failover and proactive rotation across kinds remain disabled
- if the pinned account becomes unsuitable, the session fails clearly instead of selecting another
  account from the mixed-kind pool

## Deferred Scope

The first version should not include:

- full TUI account-management UI
- production remote backend implementation
- advanced weighted scheduling
- shared lease mode
- cross-project sticky routing
- broad app-server protocol expansion unless it becomes necessary

## Testing Strategy

### Unit tests for `codex-account-pool`

Add focused tests for:

- sticky active-lease behavior
- proactive threshold rotation
- cooldown behavior
- minimum switch interval
- exclusive allocation across multiple local processes
- local lease expiration and reclaim
- lease fencing after reclaim
- SQLite transaction and lock race behavior
- no-eligible-account cases
- legacy single-account migration into account-pool state
- idempotent migration reruns
- migration recovery after partial failure
- restart behavior after migration with lease state rebuilt from persisted storage
- stale `report_*` updates rejected after fence loss
- monotonic state updates within one lease epoch
- minimum-TTL gate before starting a turn

### Integration tests for `codex-login`

Add coverage for:

- materializing one selected account into process-local auth
- ChatGPT and API key account materialization
- preserving single-account compatibility semantics
- legacy `login/logout` compatibility behavior during the migration window
- post-refresh fence checks before persisting refreshed credentials
- concurrent refresh serialization or CAS behavior per credential

### Integration tests for `codex-core`

Add coverage for:

- selecting the session's active lease before a turn
- reporting rate-limit snapshots into account-pool state
- rotating only future turns after threshold updates
- rebuilding request transport state after lease rotation
- retrying once after `usage_limit_reached` only from the replayable state
- refusing to replay a turn after visible output or side effects occurred
- stale lease fencing blocking request execution
- per-step lease and remote-context revalidation inside multi-step turns
- refusing new work after fence failure on a long-running local turn
- resetting remote conversation state on cross-account rotation when `allow_context_reuse = false`
- suppressing retry when remote commit status is unknown
- preserving the same local thread while resetting remote session state on rotation

### CLI tests

Add coverage for:

- `codex accounts list`
- `codex accounts current`
- `codex accounts status`
- `codex accounts resume`
- `codex accounts switch`
- add, remove, enable, and disable flows
- pool assignment flows
- pool override selection
- switch-reason and cooldown output
- cross-pool switch rejection without explicit override
- structured status output with per-account eligibility reasons
- migrated-install `accounts list/current/status` behavior after restart
- migrated `legacy-default` visibility and migrated-source markers
- durable signed-out suppression after legacy logout
- `codex accounts resume` clearing suppression without pinning a preferred account
- durable manual switch applying only to future runtime instances
- mixed-kind pool manual-only startup when a preferred account override is present

### App-server protocol and integration tests

Add coverage for:

- legacy `account/read` and `account/updated` preserving signed-out compatibility semantics
- legacy `account/login/*` clearing durable suppression
- `account/logout` revoking any active process-local pooled lease and enabling durable
  default-startup suppression
- `accountLease/read`, `accountLease/resume`, and `accountLease/updated` reflecting process-local
  lease state
- `accountLease/resume` affecting a live process without interrupting an in-flight turn
- pooled status surfaces reporting `remote_context_generation` and
  `last_remote_context_reset_turn_id` when reset occurs

### TUI tests

Add only targeted coverage and snapshot updates for:

- current account status display
- nearing-limit status
- automatic-switch notification with reason
- remote-context-reset notification or status indicator
- no-available-account error state
- retry-suppressed state after a non-replayable limit failure

## Rollout Plan

### Phase 1: Foundation

- add account-pool strategy layer independent from the existing auth-bound `codex-account` crate
- add local backend
- extend `codex-state::StateRuntime` with account-pool registry, lease, and runtime state tables
- add process-local auth materialization
- add `codex accounts` CLI
- add explicit pooled app-server lease-status surface
- add durable suppression and resume semantics
- support durable manual switch and current-account inspection for future runtime instances

### Phase 2: Automatic local rotation

- wire rate-limit reporting into account-pool state
- implement proactive threshold switching
- implement turn replay guard
- implement safe retry after `usage_limit_reached`
- add small TUI surfaces

### Phase 3: Remote backend

- add remote backend trait implementation
- acquire and release remote leases
- use external ChatGPT token flow for short-lived credentials
- preserve the same sticky-lease and safe-retry semantics

## Why This Design Is Recommended

This design keeps the fork's high-risk differences concentrated in a new strategy layer instead of
spreading them across upstream-owned auth and UI code.

It also avoids a dead-end local implementation:

- local mode works now
- remote pool support fits the same model later
- legacy `auth.json` compatibility is preserved without relying on a shared request-path auth slot
- multiple local Codex instances are handled by default through exclusive sticky leases

That makes it the best fit for a fork that still plans to keep rebasing onto upstream Codex.
