# Pooled Account Registration And Backend-Owned Auth Design

This document narrows the multi-account pool v1 design around the currently missing
`accounts add` path, backend-owned credential handles, and the contract needed to keep
local development aligned with a future remote account-pool service.

It is an addendum to
`docs/superpowers/specs/2026-04-10-multi-account-pool-design.md`, not a replacement.

## Summary

The recommended direction is to treat pooled account registration as a backend-owned control-plane
operation instead of extending the legacy single-account compatibility store.

Key decisions:

- Keep `auth.json` and the current `AuthManager` storage path as a legacy compatibility surface
  only.
- Model pooled accounts around backend-owned opaque handles, not local-storage-specific
  `credential_ref` semantics.
- Make lease acquisition return directly usable turn auth material so runtime execution depends on
  `LeaseGrant -> LeasedTurnAuth`, not on long-lived credential lookups.
- Separate account catalog state from pool membership state.
- Separate control-plane account management from execution-plane lease handling.
- In phase 1, implement only the local backend, but shape it so a future remote backend can slot in
  without redesigning `codex-core`, app-server, CLI, or TUI.

## Goals

- Unblock a real `accounts add` implementation without mutating the shared legacy auth slot.
- Keep the phase 1 local backend compatible with a future remote service that owns real
  credentials and returns temporary auth material at lease time.
- Avoid baking local keyring or file layout assumptions into the runtime lease path.
- Support registered-but-unassigned accounts for future enterprise import flows.
- Keep merge risk low by localizing changes to new interfaces and storage tables.

## Non-Goals

- Do not implement the remote backend in this phase.
- Do not redesign legacy `codex login/logout/status`.
- Do not introduce a full pooled account management UI.
- Do not require multi-pool membership for one account in v1.
- Do not require `accounts add api-key` in the same implementation slice as the first ChatGPT
  registration path.

## Constraints

- The user expects the long-term primary model to be a remote account pool whose control plane
  owns real credentials.
- The future remote backend should be able to hand out temporary auth material directly on
  `acquire_lease`.
- Phase 1 must remain locally usable and testable without introducing a fork-only shared auth
  container.
- Existing automatic failover behavior and runtime lease semantics must remain intact while the
  registration path is refactored.

## Recommended Architecture

### 1. Treat pooled registration as backend-owned

`accounts add` should not mean "write another credential into shared auth storage."

Instead, pooled registration should mean:

1. authenticate or ingest a credential source,
2. hand that source to the selected backend,
3. let the backend return a stable account registration result,
4. persist only control-plane metadata locally.

This keeps the registration contract compatible with both:

- a local backend that stores credentials in a backend-private namespace, and
- a future remote backend that stores credentials centrally and only returns temporary turn auth.

### 2. Make `backend_account_handle` opaque and backend-owned

Every registered pooled account should carry a backend-owned opaque handle.

The handle must not encode local file paths or keyring assumptions into callers. It is a stable
control-plane identifier used only by the backend to locate the credential source that belongs to
that account.

The runtime should never interpret this handle directly.

### 3. Split control plane from execution plane

The design should expose two distinct backend-neutral surfaces.

#### Execution plane

Used by runtime turn execution:

- `acquire_lease(pool_id, instance_id) -> LeaseGrant`
- `renew_lease(lease_key) -> LeaseRenewal`
- `release_lease(lease_key)`
- `report_health(account_id, event)`
- `read_runtime_snapshot(...)`

This surface is responsible for leases, health, and live runtime auth only.

#### Control plane

Used by CLI and app-server management paths:

- `register_account(RegisterAccountRequest) -> RegisteredAccount`
- `import_legacy_account(...)`
- `list_accounts(...)`
- `assign_pool(account_id, pool_id)`
- `set_enabled(account_id, enabled)`
- `remove_account(account_id)`
- `list_pools()`
- `read_pool_status(...)`

This surface is responsible for registration, membership, and operator-visible metadata only.

### 4. Return temporary auth material directly from lease acquisition

`acquire_lease` should return a `LeaseGrant` that already contains the turn-scoped auth material
needed to build `LeasedTurnAuth`.

That contract should be stable across backends:

- local backend: load from backend-private local storage and materialize `LeasedTurnAuth`
- remote backend: obtain temporary auth from the remote pool service and materialize
  `LeasedTurnAuth`

This prevents `codex-core` from depending on storage-specific credential lookup rules.

### 5. Keep local pooled credentials in a backend-private namespace

Phase 1 should reuse existing auth storage implementations, but not the shared compatibility slot.

For each local pooled account, derive a backend-private auth namespace such as:

`CODEX_HOME/.pooled-auth/backends/<backend_id>/accounts/<backend_account_handle>/`

That namespace may continue using the existing file/keyring/auto storage implementations, because
those are already keyed by `codex_home`.

This yields two desirable properties:

- local pooled credentials do not overwrite the legacy shared slot
- the phase 1 backend can reuse existing auth storage code without teaching the runtime about
  local storage details

### 6. Keep legacy auth import explicit

Fresh registration and legacy import should not be conflated.

Recommended CLI shape:

- `accounts add chatgpt`
- `accounts add chatgpt --device-auth`
- `accounts add api-key`
- `accounts import-legacy`

`accounts add` should mean fresh registration against the backend.
`accounts import-legacy` should mean "take the current legacy compatibility account and register
that existing account into pooled management."

This keeps user intent clear and avoids accidentally reusing the single legacy slot as though it
were a second pooled account.

## Data Model

### Registered account catalog

The local registry should represent "known pooled account" separately from pool assignment.

Recommended `account_registry` fields:

- `account_id`
- `backend_id`
- `backend_account_handle`
- `account_kind`
- `provider_fingerprint`
- `display_name`
- `source`
- `enabled`
- `healthy`
- `created_at`
- `updated_at`

Notes:

- `account_id` is a local stable opaque identifier.
- `provider_fingerprint` is for dedupe and idempotency, not user-facing display.
- `backend_account_handle` is internal control-plane data and should not be part of normal text
  output.

### Pool membership

Pool assignment should live in a dedicated table such as `account_pool_membership`:

- `account_id`
- `pool_id`
- `position`
- `assigned_at`
- `updated_at`

This allows:

- registered but currently unassigned accounts,
- later assignment into a pool,
- clean migration toward future enterprise-managed account catalogs.

## Core Types

Recommended control-plane and execution-plane boundary types:

```rust
pub struct RegisterAccountRequest {
    pub backend_id: String,
    pub display_name: Option<String>,
    pub credential_input: CredentialInput,
    pub idempotency_key: Option<String>,
}

pub enum CredentialInput {
    ChatgptManaged { auth: AuthDotJson },
    ApiKey { api_key: SecretString, label: Option<String> },
}

pub struct RegisteredAccount {
    pub account_id: String,
    pub backend_id: String,
    pub backend_account_handle: String,
    pub account_kind: AccountKind,
    pub provider_fingerprint: String,
    pub display_name: Option<String>,
    pub source: AccountSource,
    pub enabled: bool,
}

pub struct LeaseGrant {
    pub lease_key: LeaseKey,
    pub account_id: String,
    pub pool_id: String,
    pub leased_auth: LeasedTurnAuth,
    pub expires_at: DateTime<Utc>,
    pub next_eligible_at: Option<DateTime<Utc>>,
}
```

The exact type names may change during implementation, but the boundary responsibilities should
remain the same.

## CLI Semantics

### `accounts add`

`accounts add` should perform fresh backend registration.

Recommended behavior:

- if a pool is specified, register the account and assign it to that pool
- if no pool is specified but `default_pool` exists, assign it there
- if no pool target exists, register successfully but leave the account unassigned
- when registration sees an already-known account, treat the operation as idempotent rather than
  creating a duplicate record

### `accounts import-legacy`

`accounts import-legacy` should:

- inspect the current legacy compatibility auth
- derive its provider fingerprint
- reuse an existing pooled record if present
- otherwise register a new pooled record sourced from the legacy account
- optionally assign the account into a target pool

It must not silently relabel existing provenance for already-registered accounts.

## Idempotency, Dedupe, and Failure Handling

### Uniqueness

The control plane should enforce:

- `(backend_id, provider_fingerprint)` unique
- `(backend_id, backend_account_handle)` unique

### `accounts add` idempotency

Recommended behavior:

- same account, no pool target: return existing account successfully
- same account, same pool target: succeed as a no-op
- same account, currently unassigned, new pool target: assign and succeed
- same account, different existing pool target: fail with guidance to use `accounts pool assign`

### Registration rollback

`accounts add --pool ...` should behave as one atomic operator intent.

If backend registration succeeds but local registry persistence fails, the caller should invoke a
backend compensation path such as `delete_registered_account(backend_account_handle)` before
returning failure.

If compensation also fails, surface a partial-failure error that includes enough information for
manual cleanup.

## Migration Strategy

Recommended sequence:

1. Add backend-neutral control-plane and execution-plane contracts without changing user-visible
   behavior.
2. Extend schema for `backend_id`, `backend_account_handle`, `provider_fingerprint`, and
   `account_pool_membership`.
3. Change execution to consume `LeaseGrant { leased_auth }` directly.
4. Enable `accounts add chatgpt`, `accounts add chatgpt --device-auth`, and
   `accounts import-legacy`.
5. Add `accounts add api-key` as a follow-up slice if needed.

This keeps the highest-risk auth materialization change isolated from the CLI surface change.

## Testing And Validation

The implementation should be validated in three layers.

### 1. Control-plane tests

Verify:

- registration dedupe
- idempotent re-add
- explicit legacy import behavior
- membership assignment semantics
- compensation delete on persistence failure
- registered-but-unassigned accounts do not participate in automatic selection

### 2. Execution-plane tests

Verify:

- lease acquisition returns usable `LeasedTurnAuth`
- local pooled auth loads from backend-private namespaces
- automatic future-turn rotation still works after the new lease grant path
- exclusive lease behavior still prevents multiple local instances from sharing one account

### 3. Product-surface tests

Verify:

- `accounts add chatgpt`
- `accounts import-legacy`
- `accounts current/status --json`
- `accounts switch/resume/disable/remove/pool assign`
- TUI `/status` pooled lease display
- visible history notice when automatic switching occurs

## Recommendation

Implement phase 1 with:

- backend-neutral control-plane and execution-plane contracts,
- a local backend that reuses existing auth storage implementations via backend-private namespaces,
- `LeaseGrant` carrying directly usable turn auth,
- explicit `accounts import-legacy`,
- and ChatGPT registration as the first fully working add path.

This is the smallest design that keeps the current fork merge-friendly while preserving a clean
upgrade path to a remote account-pool service that owns real credentials and issues temporary auth
material at lease time.
