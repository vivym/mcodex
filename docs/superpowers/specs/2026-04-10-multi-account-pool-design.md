# Multi-Account Pool And Automatic Failover Design

This document describes a merge-friendly design for adding multi-account management,
rate-limit monitoring, and automatic account switching to this fork of Codex.

The design is intentionally shaped so local multi-account support can ship first, while leaving a
clean path for a future remote account-pool service used by a team or company.

## Summary

The recommended design introduces a new account-pool strategy layer instead of rewriting the
existing single-account auth flow in place.

Key decisions:

- Add a new `codex-account-pool` crate that owns account-pool policy, lease management,
  rate-limit state, and account selection.
- Keep `codex-login` focused on single-account auth material, refresh, and compatibility with the
  current auth model.
- Treat existing `auth.json` or keyring state as a compatibility projection for the current active
  account, not as the source of truth for all accounts.
- Use session-scoped sticky leases: one Codex process keeps one active account until it is near
  its limit, exhausted, unauthorized, manually switched, or its lease expires.
- Default to `exclusive` allocation so multiple local Codex instances do not silently share the
  same account.
- Support both local and future remote pool backends through a common backend interface.
- Ship the first version with local backend support, ChatGPT-first account support, and minimal
  TUI changes.

## Goals

- Support multiple accounts in a single Codex installation.
- Monitor rate limits and usage-limit signals per account.
- Automatically switch accounts before hard exhaustion when possible.
- Automatically fail over after `usage_limit_reached` when the current turn has not produced
  side effects.
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
- The design must allow same-pool context reuse across accounts by explicit user choice.
- Automatic retry after switching accounts must not duplicate side-effecting tool work.

## Recommended Architecture

### 1. New strategy crate: `codex-account-pool`

Add a new crate, tentatively named `codex-account-pool`, that owns:

- account registry metadata
- pool definitions and policy evaluation
- lease acquisition and release
- sticky active-account selection per Codex process
- rate-limit state and cooldown tracking
- automatic failover decisions

This crate should not own low-level token refresh logic for a single account. That work remains in
`codex-login`.

### 2. Keep `codex-login` single-account focused

`codex-login` continues to handle:

- reading or persisting the auth material for one account
- refresh-token logic for managed ChatGPT auth
- compatibility with the current auth loading model
- constructing `CodexAuth` for the currently selected account

This keeps the existing auth code understandable and reduces conflicts with upstream changes.

### 3. Treat `auth.json` as a compatibility projection

Multi-account state must not be encoded by changing `auth.json` into a multi-account container.

Instead:

- account-pool state is stored separately
- the current active account may be projected into the existing auth store so existing request
  paths continue to work
- upstream code that assumes one current account can continue to operate with minimal changes

This is the main merge-friendly choice in the design.

### 4. Separate backend from policy

The account-pool layer should depend on a backend trait rather than assuming local file storage.

Initial backend:

- `LocalPoolBackend`

Future backend:

- `RemotePoolBackend`

The backend abstraction is required from the start so a company-managed remote account-pool
service can be added later without redesigning the strategy layer.

## High-Level Flow

### Normal startup and steady state

1. Codex starts and creates a process/session `instance_id`.
2. The account-pool layer loads pool policy and runtime state.
3. The process obtains one sticky active lease for its pool.
4. Requests keep using the same leased account.
5. Rate-limit snapshots and usage-limit signals update runtime state for that lease.

### Proactive switching

When the active account crosses a configured threshold, such as 85 or 90 percent:

- keep the current request running
- mark the account as near limit
- select a different account for the next turn
- place the old account into cooldown when appropriate

This avoids request-time thrashing and keeps switching behavior closer to "session rotation" than
"per-request scheduling."

### Hard-failure switching

When the current account hits `usage_limit_reached`:

- mark it exhausted or cooling down
- if the turn has not yet produced any side effects, switch accounts and retry once
- if the turn has already produced side effects, do not replay the turn; only switch future turns

This preserves automation without risking duplicate shell, MCP, or patch operations.

## Sticky Lease Model

The allocation unit is a Codex process or session, not a single request.

Default behavior:

- one Codex process holds one sticky active lease
- that lease remains active until the account is no longer suitable
- switching is exceptional, not routine

Default switching triggers:

- proactive rate-limit threshold reached
- `usage_limit_reached`
- repeated unauthorized or refresh failure
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

- choosing the active account for the process
- applying threshold rules
- cooldown logic
- deciding when a switch is allowed
- deciding when a failed turn is safe to retry

### Why this split matters

For local mode, the backend may store durable credentials in file or keyring.

For remote mode, the backend may instead:

- acquire an account lease from a company service
- receive short-lived access tokens instead of refresh tokens
- avoid persisting long-lived account credentials on the developer machine

The same policy layer should be able to operate in both cases.

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
account_kinds = ["chatgpt", "api_key"]
```

This area should define:

- backend type
- default pool
- thresholds
- allocation mode
- lease timing
- optional pool-level policy

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
- last selected time
- hard-limit timestamp
- next eligible timestamp
- current lease holder
- lease expiration and heartbeat

Suggested health states:

- `healthy`
- `near_limit`
- `exhausted`
- `cooling_down`
- `unavailable`

## Lease Model

Every active assignment should be represented by a lease.

Suggested lease fields:

- `lease_id`
- `account_id`
- `pool_id`
- `holder_instance_id`
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

## Core Interfaces

The account-pool layer should expose lease-oriented APIs rather than a simple "give me an account"
API.

Suggested interface shape:

- `ensure_active_lease(session_context) -> Lease`
- `report_rate_limits(lease, snapshot)`
- `report_usage_limit_reached(lease, error)`
- `report_unauthorized(lease)`
- `release_lease(lease)`
- `heartbeat_lease(lease)`

The active lease is session-scoped and sticky. `ensure_active_lease` should typically return the
current active lease unless policy says a rotation is required.

## Integration Points

### `codex-core`

`codex-core` should only gain thin integration points:

- before beginning a model turn, ensure there is an active lease
- update account-pool state when rate-limit snapshots arrive
- update account-pool state when usage-limit or unauthorized errors arrive
- attempt one automatic retry only when the turn has not yet produced side effects

This keeps the new behavior mostly out of the existing large request pipeline.

### `codex-login`

`codex-login` should provide the account-pool layer with the means to:

- materialize the selected local account as current auth
- persist or refresh a single account's auth state
- project the active local account into the current auth store

### App-server

The first version should avoid large app-server protocol changes.

Minimal app-server work is acceptable where needed for:

- surfacing current account or lease status
- reusing existing rate-limit notifications
- reusing external ChatGPT token support

For future remote pool support, the design should be compatible with existing app-server external
auth flows, especially:

- externally supplied `chatgptAuthTokens`
- refresh requests that already include `previousAccountId`

### TUI

The first version should keep TUI changes intentionally small:

- show current account label or id
- show nearing-limit status
- show "automatically switched account" events
- show "no accounts available" errors

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
- `codex accounts switch <account>`
- `codex accounts enable <account>`
- `codex accounts disable <account>`
- `codex accounts remove <account>`
- `codex accounts pool list`
- `codex accounts pool assign <account> <pool>`

Compatibility expectations:

- `codex login` remains a single-account convenience path
- `codex logout` only logs out the current active account
- docs and future features should guide users toward `codex accounts`

## Automatic Retry Rules

Automatic retry after switching accounts is only allowed when the current turn has not produced
side effects.

Side effects include at least:

- shell commands
- MCP tool calls with external effects
- patch application
- other tool operations that have already committed real work

Retry behavior:

- no side effects yet: switch account and retry once
- side effects already occurred: switch only future turns, return a clear message for the current
  turn

This rule is mandatory to prevent duplicated work.

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
- retry once only if the turn has not produced side effects
- otherwise switch only future turns

### Unauthorized or refresh failure

- first attempt the selected account's normal auth recovery
- if recovery fails, mark the account unavailable
- rotate to another eligible account in the same pool

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

- new `codex-account-pool` crate
- local backend
- session-scoped sticky leases
- `exclusive` allocation
- ChatGPT-first full support
- proactive threshold switching
- `usage_limit_reached` failover with safe retry rules
- CLI account management under `codex accounts`
- small TUI status surfaces

The first implementation may include API key accounts in the data model and CLI, while deferring
full automatic quota-awareness for API keys until later.

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
- no-eligible-account cases

### Integration tests for `codex-login`

Add coverage for:

- materializing one selected account as current auth
- ChatGPT and API key account materialization
- preserving single-account compatibility semantics

### Integration tests for `codex-core`

Add coverage for:

- selecting the session's active lease before a turn
- reporting rate-limit snapshots into account-pool state
- rotating only future turns after threshold updates
- retrying once after `usage_limit_reached` when no side effects occurred
- refusing to replay a turn after side effects occurred

### CLI tests

Add coverage for:

- `codex accounts list`
- `codex accounts current`
- `codex accounts switch`
- add, remove, enable, and disable flows
- pool assignment flows

### TUI tests

Add only targeted coverage and snapshot updates for:

- current account status display
- nearing-limit status
- automatic-switch notification
- no-available-account error state

## Rollout Plan

### Phase 1: Foundation

- add `codex-account-pool`
- add local backend
- add registry, lease, and runtime state model
- add `codex accounts` CLI
- support manual switch and current-account inspection

### Phase 2: Automatic local rotation

- wire rate-limit reporting into account-pool state
- implement proactive threshold switching
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
- `auth.json` compatibility is preserved
- multiple local Codex instances are handled by default through exclusive sticky leases

That makes it the best fit for a fork that still plans to keep rebasing onto upstream Codex.
