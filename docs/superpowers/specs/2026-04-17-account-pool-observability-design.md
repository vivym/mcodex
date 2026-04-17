# Account Pool Observability Design

This document defines a complete read-only observability surface for pooled
accounts, exposed through app-server v2 and implemented first for the local
backend.

It is intentionally scoped to pool/account snapshots, event history,
diagnostics, and backend-neutral read seams. It does not add write-side
operator commands, it does not add a remote backend implementation, and it
does not require immediate CLI or TUI consumption in the same slice.

## Summary

The recommended direction is:

- freeze a complete app-server v2 read contract now instead of starting with a
  narrower local-only snapshot API
- implement four read-only RPCs:
  - `accountPool/read`
  - `accountPool/accounts/list`
  - `accountPool/events/list`
  - `accountPool/diagnostics/read`
- keep app-server dependent on a backend-neutral observability reader rather
  than local SQLite details
- implement local pooled observability by reading current state from existing
  state tables and persisting only append-only event history
- treat diagnostics as a derived view, not a separately persisted truth source
- keep stable top-level identifiers local (`pool_id`, `account_id`) so future
  remote support can plug in without rewriting the client-facing contract
- defer CLI and TUI consumers until the contract and local implementation are
  stable

This provides a complete operator-visible read surface in one contract shape
without taking the merge risk of also building all consumers and control-plane
commands in the same change.

## Goals

- Give operators a complete read-only pooled-account observability surface.
- Keep the first operator contract stable enough for later CLI, TUI, and remote
  reuse.
- Make it possible to answer both:
  - "what is the pool doing now?"
  - "why did it get into this state?"
- Keep future remote support merge-friendly by introducing narrow backend
  seams instead of scattering pool-specific reads through `codex-core`.
- Avoid stale duplicated state by persisting event history but deriving
  diagnostics from current data.

## Non-Goals

- Do not add pause, resume, drain, or other write-side operator commands in
  this slice.
- Do not add a remote backend implementation.
- Do not redesign pooled allocation or lease policy.
- Do not add a full TUI pool-management page.
- Do not add a large CLI operator experience in the same slice.
- Do not backfill historical observability data from earlier runs.

## Constraints

- Upstream mergeability matters, so the design should localize new behavior to
  `codex-account-pool`, `codex-state`, `codex-app-server-protocol`, and
  `codex-app-server` as much as possible.
- Existing pooled surfaces already expose a single current lease view through
  app-server and TUI status.
- Future remote support is expected, so the contract must not assume local
  SQLite is the only long-term source of truth.
- Local product homes may persist non-secret control-plane identifiers, but
  must remain compatible with the future remote contract that keeps remote
  secret material remote-owned.
- The observability contract must support empty data sets cleanly; "no events"
  and "no active lease" are valid states, not protocol failures.

## Problem Statement

Current pooled observability is fragmented and too narrow:

- the TUI and app-server can explain part of the current active lease
- there is no pool-level summary contract
- there is no account list contract that shows per-account operational state
- there is no event history contract for recent lease, switch, suppression, or
  failure activity
- there is no derived diagnostics contract that summarizes why a pool is
  healthy, degraded, or blocked

That leaves several problems:

- operators cannot inspect the whole pool through one stable read interface
- future CLI and TUI work would be tempted to build local-only read paths
- future remote support would have to retrofit a contract after local clients
  had already grown around implementation-specific behavior
- the codebase lacks a clean place for pooled observability that is separate
  from lease execution and write-side control-plane operations

The missing piece is not another ad hoc status string. It is a full, stable
read contract plus a local implementation that is careful about which facts are
persisted and which are derived.

## Approaches Considered

### Approach A: Add only a minimal pool snapshot API now

Under this approach, the first slice would expose only current summary data and
defer accounts, events, and diagnostics until later.

Pros:

- smallest first implementation
- lowest immediate schema surface area

Cons:

- almost guarantees later app-server contract expansion
- encourages early consumers to depend on an incomplete shape
- weakens the path to remote because the richer contract would still need to be
  designed later

This approach is rejected.

### Approach B: Freeze the full read contract now, implement local backend reads first

Under this approach:

- the app-server v2 contract includes pool summary, account listing, event
  history, and diagnostics from the start
- the first implementation supports only the local backend
- CLI and TUI consumers are deferred to a later slice
- diagnostics are derived at read time
- only event history gets new durable storage

Pros:

- gives the project one stable observability contract
- keeps the future remote path open
- keeps the first implementation focused on the layers that should own the
  contract
- avoids unnecessary UI churn while the contract is still settling

Cons:

- larger first protocol surface than a snapshot-only slice
- requires reason taxonomy and event model decisions up front

This is the recommended approach.

### Approach C: Build the full operator experience now, including app-server, CLI, and TUI

Under this approach, the project would ship the complete read contract and all
initial consumers in one slice.

Pros:

- gives users the most immediately visible feature set

Cons:

- highest merge risk
- broadest touch surface across protocol, state, app-server, CLI, and TUI
- harder to revise if the contract needs correction after first use

This approach is rejected.

## Recommended Design

### 1. Freeze four app-server v2 read RPCs

The first stable observability contract should expose exactly four new
read-only RPCs:

- `accountPool/read`
- `accountPool/accounts/list`
- `accountPool/events/list`
- `accountPool/diagnostics/read`

These methods should live only in app-server v2. New observability surface area
must not be added to app-server v1.

The contract should be complete enough that later CLI, TUI, and remote-backed
consumers can build on it without reshaping the wire format.

### 2. Keep app-server dependent on a backend-neutral observability seam

`codex-account-pool` should add a new backend-neutral read trait for pooled
observability, separate from execution and control-plane writes.

The intent is:

- app-server depends on a stable reader abstraction
- local pooled state implements that abstraction
- future remote support adds another implementation instead of teaching
  app-server about local SQLite details

This seam should own pooled read concepts such as:

- pool summary reads
- account list reads
- event list reads
- diagnostics reads

It should not absorb lease execution logic or write-side operator commands.

### 3. Keep top-level identifiers local and stable

The observability contract should use these stable top-level identifiers:

- `poolId`
- `accountId`
- `leaseId`
- `holderInstanceId`

Future remote support may include an additive backend correlation field such as
`backendAccountRef`, but that field must remain optional correlated metadata,
not the primary wire identity everywhere.

This keeps local startup selection and future remote correlation aligned with
the existing local identifier model.

### 4. Persist only append-only event history

The local implementation should add one new durable history table:

- `account_pool_events`

The table should be append-only and store stable event metadata, not derived
summary state.

Recommended columns:

- `event_id`
- `occurred_at`
- `pool_id`
- `account_id` nullable
- `lease_id` nullable
- `holder_instance_id` nullable
- `event_type`
- `reason_code` nullable
- `message`
- `details_json` nullable

Recommended indexes:

- `(pool_id, occurred_at DESC, event_id DESC)`
- `(account_id, occurred_at DESC, event_id DESC)`
- optional `(event_type, occurred_at DESC)`

The design intentionally does not introduce a second persisted snapshot table.
Current-state reads should come from the already authoritative local state
tables wherever possible.

### 5. Derive current snapshots from authoritative local state

Pool and account snapshots should be assembled from existing authoritative local
state rather than from duplicated observability caches.

For example, the local reader may compose:

- registered account metadata
- runtime state / health state
- active lease state
- startup selection facts
- configured pool policy

This keeps the read model close to the real source of truth and reduces the
risk of snapshot drift.

When a fact has no stable local source yet, the contract should return `null`
instead of inventing a fragile shadow state layer.

### 6. Make diagnostics a derived view

`accountPool/diagnostics/read` should be computed at read time from current
state plus, where useful, a small recent event window.

Diagnostics should summarize whether the pool is:

- `healthy`
- `degraded`
- `blocked`

And should return issue records that point to current blockers or degradation
signals, such as:

- no eligible account
- all accounts paused
- preferred account suppressed
- proactive switch blocked by minimum switch interval
- recent acquire failure with no replacement

Diagnostics must not become a separately persisted source of truth. They are a
derived operator aid.

### 7. Limit `codex-core` changes to event emission at true decision points

Most event writes should happen in the same local state paths that already own
lease and health persistence.

Only events that exist purely at runtime decision level should be emitted from
`codex-core` or the account-pool manager, such as:

- proactive switch selected
- proactive switch suppressed
- acquire failed with a high-level runtime reason

This prevents pooled observability logic from expanding across unrelated core
execution paths.

## Wire Contract

### 1. `accountPool/read`

Purpose: return the current pool summary and effective policy.

Request:

- `poolId`

Response:

- `poolId`
- `backend`
- `summary`
- `policy`
- `refreshedAt`

`summary` should include at least:

- `totalAccounts`
- `availableAccounts`
- `leasedAccounts`
- `pausedAccounts`
- `drainingAccounts`
- `nearExhaustedAccounts`
- `exhaustedAccounts`
- `errorAccounts`
- `activeLeases`

`policy` should include at least:

- `allocationMode`
- `allowContextReuse`
- `proactiveSwitchThresholdPercent`
- `minSwitchIntervalSecs`

### 2. `accountPool/accounts/list`

Purpose: list current account-level state within one pool.

Request:

- `poolId`
- `cursor` optional
- `limit` optional
- `states` optional
- `accountKinds` optional
- `query` optional

Response:

- `data`
- `nextCursor`

Each account item should include at least:

- `accountId`
- `backendAccountRef` nullable
- `accountKind`
- `operationalState`
- `allocatable`
- `statusReasonCode` nullable
- `statusMessage` nullable
- `currentLease` nullable
- `quota` nullable
- `selection` nullable
- `updatedAt`

`currentLease` should include:

- `leaseId`
- `leaseEpoch`
- `holderInstanceId`
- `acquiredAt`
- `renewedAt`
- `expiresAt`

`quota` should include:

- `remainingPercent`
- `resetsAt`
- `observedAt`

`selection` should include:

- `eligible`
- `nextEligibleAt`
- `preferred`
- `suppressed`

### 3. `accountPool/events/list`

Purpose: list recent pool/account events and explain why they happened.

Request:

- `poolId`
- `accountId` optional
- `types` optional
- `since` optional
- `cursor` optional
- `limit` optional

Response:

- `data`
- `nextCursor`

Each event item should include at least:

- `eventId`
- `occurredAt`
- `poolId`
- `accountId` nullable
- `leaseId` nullable
- `holderInstanceId` nullable
- `eventType`
- `reasonCode` nullable
- `message`
- `details` nullable object

### 4. `accountPool/diagnostics/read`

Purpose: summarize current pool health and operator-relevant issues.

Request:

- `poolId`

Response:

- `poolId`
- `generatedAt`
- `status`
- `issues`

Each diagnostic issue should include at least:

- `severity`
- `reasonCode`
- `message`
- `accountId` nullable
- `holderInstanceId` nullable
- `nextRelevantAt` nullable

## Shared Enums and Semantics

### Account operational state

The first stable account state enum should include:

- `available`
- `leased`
- `paused`
- `draining`
- `coolingDown`
- `nearExhausted`
- `exhausted`
- `error`

This enum answers "what is the account's current operational state?" It should
not be overloaded as an event taxonomy or as a durable failure reason.

### Event type

The first stable event type enum should include:

- `leaseAcquired`
- `leaseRenewed`
- `leaseReleased`
- `leaseAcquireFailed`
- `proactiveSwitchSelected`
- `proactiveSwitchSuppressed`
- `quotaObserved`
- `quotaNearExhausted`
- `quotaExhausted`
- `accountPaused`
- `accountResumed`
- `accountDrainingStarted`
- `accountDrainingCleared`
- `authFailed`
- `cooldownStarted`
- `cooldownCleared`

This list is intentionally broad enough to cover local v1 observability and the
future remote contract without exposing backend-private transport details.

### Reason code

The first stable reason-code enum should include:

- `manualPause`
- `manualDrain`
- `quotaNearExhausted`
- `quotaExhausted`
- `authFailure`
- `cooldownActive`
- `minimumSwitchInterval`
- `preferredAccountSuppressed`
- `noEligibleAccount`
- `leaseHeldByAnotherInstance`
- `unknown`

Reason-code rules:

- `operationalState` answers "what is true now?"
- `reasonCode` answers "why is this state true?" or "why did this event happen?"
- `message` is for humans
- `details` carries structured additive context and may be backend-specific

Clients should key logic off enums, not free-form message strings.

## Errors and Pagination

### Error model

The read methods should keep a simple, stable error model:

- missing `poolId` target: `NOT_FOUND`
- invalid cursor or invalid params: `INVALID_PARAMS`
- empty list results: success with empty `data`
- no issues: success with empty `issues`

The contract must not treat missing events or missing current leases as errors.

### Pagination

Both list methods should use cursor pagination from the first version.

Request fields:

- `cursor: Option<String>`
- `limit: Option<u32>`

Response fields:

- `data: Vec<_>`
- `nextCursor: Option<String>`

Recommended sort order:

- `accountPool/accounts/list`: stable account order, for example `account_id ASC`
- `accountPool/events/list`: `occurred_at DESC, event_id DESC`

The cursor should encode an ordering anchor rather than exposing SQL offset.

## Local Implementation Notes

### 1. `codex-app-server-protocol`

Add the four v2 request/response types plus shared enums.

All new payloads should follow v2 rules:

- `*Params` for request payloads
- `*Response` for responses
- camelCase on the wire
- `#[ts(export_to = "v2/")]`
- no `skip_serializing_if` on v2 payload fields except existing request
  compatibility exceptions

### 2. `codex-account-pool`

Add a backend-neutral observability read trait, separate from execution and
control-plane write traits.

This trait should define the minimum read surface app-server needs without
forcing app-server to know about local state schema.

### 3. `codex-state`

Own:

- the `account_pool_events` migration
- local snapshot queries
- local event queries
- local diagnostics helpers or reader-side inputs

It should not grow a second long-lived snapshot truth store.

### 4. `codex-app-server`

Own:

- RPC registration
- request validation
- adaptation from the observability reader into protocol responses

This slice should not add CLI or TUI reads yet.

## Testing Strategy

At minimum, the first implementation should include:

### 1. Protocol tests

- schema fixture coverage for new v2 payloads
- enum coverage for the shared observability enums

### 2. State tests

- event write/read tests
- snapshot query tests
- pagination and cursor edge-case tests
- account state classification tests

### 3. App-server tests

- `accountPool/read`
- `accountPool/accounts/list`
- `accountPool/events/list`
- `accountPool/diagnostics/read`

### 4. Scenario-oriented contract tests

At least these local scenarios should be covered:

- healthy pool with one active lease
- no eligible account
- proactive switch suppressed by minimum switch interval
- near-exhausted account with replacement still available
- paused or draining account present in the pool

These scenarios should be written to remain useful when a future remote backend
implements the same top-level contract.

## Rollout and Follow-On Work

This design intentionally sequences the work as:

1. freeze protocol contract
2. implement local state/event reads
3. add backend-neutral reader seam
4. expose app-server handlers
5. later add CLI and TUI consumers

Follow-on work may include:

- CLI operator commands that consume the new read APIs
- TUI pool-management/status views
- write-side control-plane operations such as pause/resume/drain
- remote backend support against the same observability contract

## Acceptance Criteria

This design is complete when:

- app-server v2 exposes the four pooled observability RPCs
- the local backend can return pool summary, account list, event history, and
  diagnostics without requiring CLI or TUI-specific code
- event history is durably recorded through an append-only local table
- diagnostics are derived rather than separately persisted
- the top-level contract remains compatible with future remote support through a
  backend-neutral reader seam
