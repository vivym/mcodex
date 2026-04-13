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
- Make lease acquisition return a refresh-capable leased auth session from which runtime turn auth
  snapshots are derived, so execution no longer depends on long-lived credential lookups tied to
  local storage layout.
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
- Current `codex-core` session services still have non-request consumers that depend on
  refresh-capable auth manager behavior, so the lease contract cannot degrade to a one-shot bearer
  token snapshot in phase 1.

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

This addendum supersedes earlier shorthand references to `credential_ref` in the broader design.
Those references should now be read as one of:

- a backend-owned opaque account handle used only by the control plane, or
- a lease-scoped auth session returned by the execution plane.

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
- `list_accounts(...)`
- `assign_pool(account_id, pool_id)`
- `set_enabled(account_id, enabled)`
- `remove_account(account_id)`
- `list_pools()`
- `read_pool_status(...)`

This surface is responsible for registration, membership, and operator-visible metadata only.

Legacy compatibility migration is intentionally outside the backend-neutral control plane.
`accounts import-legacy` should be implemented by a local-only migration/helper path that inspects
legacy auth, derives provider identity, and then calls ordinary control-plane registration and pool
assignment primitives.

### 4. Return a refresh-capable leased auth session from lease acquisition

`acquire_lease` should return a `LeaseGrant` that contains a lease-scoped auth session, not just a
single bearer token snapshot.

That session is responsible for:

- yielding a `LeasedTurnAuth` snapshot for request execution,
- supporting managed refresh or backend-mediated refresh for the lease lifetime,
- exposing stable lease binding metadata so consumers can verify which account/lease they are using,
- and failing closed once the lease is released or rotated away.

Execution-plane rules must be explicit:

- request-path code consumes only immutable `LeasedTurnAuth` snapshots
- request retries must not consult a shared `AuthManager` or mutable auth storage mid-turn
- lease-scoped refresh may rotate tokens only for the same stable `account_id` and
  `backend_account_handle`
- any identity change terminates the current lease and requires a fresh `LeaseGrant`

Phase 1 may temporarily ship a legacy-only bridge that adapts a lease-scoped session to
`AuthManager`-shaped consumers that have not migrated yet, but that adapter is not part of the
backend-neutral execution contract and must not be used by request-path retry logic.

That contract should be stable across backends:

- local backend: construct a lease-private auth session backed by the backend-private namespace and
  derive `LeasedTurnAuth` snapshots from it
- remote backend: obtain temporary auth and refresh capability from the remote pool service and
  derive `LeasedTurnAuth` snapshots from that session

`LeasedTurnAuth` remains the request-path snapshot, but it is no longer the entire lease contract.
Long-lived non-request consumers must hold a lease-scoped session that becomes invalid when the
lease is released or rotated. Rebinding is done by installing a new `LeaseGrant`, never by silently
mutating an old session to point at a different account.

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

#### Legacy bootstrap transition

Steady-state pooled behavior should not retain today's implicit legacy auto-import.

Once this design lands:

- `accounts import-legacy` becomes the only steady-state operator-visible import path
- runtime turn preparation and generic CLI startup should stop auto-importing the legacy account
- if a compatibility migration is still needed for pre-existing installs, it must run as a
  one-time migration step with recorded completion, not as ordinary pooled runtime behavior

This resolves the conflict between explicit account registration and today's startup-time
`import_legacy_default_account` fallback.

### 7. Phase 1 CLI delivery semantics

The first shippable `accounts add` slice should be intentionally narrow.

Phase 1 command behavior:

- `accounts add chatgpt` is fully supported.
- `accounts add chatgpt --device-auth` is fully supported.
- `accounts add api-key` remains explicitly unsupported in this slice and must fail with a clear
  message instead of pretending registration succeeded.

Pool targeting rules for the ChatGPT add paths:

1. use `codex accounts --account-pool <pool> add ...` when provided,
2. otherwise use the current effective pool from pooled account diagnostics,
3. if neither exists, fail and instruct the operator to configure a pool or pass
   `--account-pool`.

Phase 1 must not silently create an implicit default pool for fresh registrations.

Implementation rules:

- fresh registration writes only backend-private pooled auth owned by the selected backend
- fresh registration must not mutate shared legacy compatibility auth storage
- the CLI should orchestrate registration, while backend-specific credential persistence remains in
  the backend control plane
- idempotency and rollback should continue to be handled through the pending-registration journal

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

- `account_id` is a local stable control-plane identifier.
- `provider_fingerprint` is for dedupe and idempotency, not user-facing display.
- `backend_account_handle` is internal control-plane data and should not be part of normal text
  output.
- provider-specific routing identifiers such as ChatGPT workspace/account ids continue to live in
  the leased auth session and provider metadata rather than replacing the local control-plane id.

### Pool membership

Pool assignment should live in a dedicated table such as `account_pool_membership`:

- `account_id`
- `pool_id`
- `position`
- `assigned_at`
- `updated_at`

Required invariants:

- `account_id` is unique in `account_pool_membership` so one account belongs to at most one pool
- `FOREIGN KEY(account_id) REFERENCES account_registry(account_id) ON DELETE CASCADE`
- if assignment history is needed, keep it in a separate audit table rather than allowing multiple
  live membership rows

This allows:

- registered but currently unassigned accounts,
- later assignment into a pool,
- clean migration toward future enterprise-managed account catalogs.

### Transition from `account_registry.pool_id`

The new membership table needs an explicit cutover plan because the current runtime still reads
`account_registry.pool_id`, `position`, and `account_runtime_state.pool_id` directly.

Recommended transition:

1. add `account_pool_membership` and backfill it from the existing `account_registry.pool_id` and
   `position` columns
2. treat `account_pool_membership` as the new source of truth
3. keep `account_registry.pool_id` and `position` as synchronized compatibility columns for one
   transition slice while readers and writers migrate
4. keep `account_runtime_state.pool_id` synchronized from the resolved membership/pool binding
   until health, diagnostics, and snapshot readers stop depending on it
5. remove compatibility reads and then drop the legacy columns in a later cleanup change

The implementation should not leave both representations as independent writable sources.

## Core Types

Recommended control-plane and execution-plane boundary types:

```rust
pub struct RegisterAccountRequest {
    pub backend_id: String,
    pub display_name: Option<String>,
    pub credential_input: CredentialInput,
    pub idempotency_key: String,
}

pub enum CredentialInput {
    ChatgptManaged {
        oauth_tokens: ChatgptManagedRegistrationTokens,
    },
    ApiKey { api_key: SecretString, label: Option<String> },
}

pub struct ChatgptManagedRegistrationTokens {
    pub id_token: String,
    pub access_token: SecretString,
    pub refresh_token: SecretString,
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

pub struct LeaseAuthBinding {
    pub account_id: String,
    pub backend_account_handle: String,
    pub lease_epoch: u64,
}

pub trait LeaseScopedAuthSession: Send + Sync {
    fn leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth>;
    fn refresh_leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth>;
    fn binding(&self) -> &LeaseAuthBinding;
    fn ensure_current(&self) -> anyhow::Result<()>;
}

pub struct LeaseGrant {
    pub lease_key: LeaseKey,
    pub account_id: String,
    pub pool_id: String,
    pub auth_session: Arc<dyn LeaseScopedAuthSession>,
    pub expires_at: DateTime<Utc>,
    pub next_eligible_at: Option<DateTime<Utc>>,
}
```

The exact type names may change during implementation, but the boundary responsibilities should
remain the same.

`ChatgptManagedRegistrationTokens` intentionally mirrors provider-level OAuth exchange output rather
than the local `auth.json` persistence schema. The backend may transform it into any backend-private
storage representation it needs.

`LeaseScopedAuthSession` is intentionally backend-neutral. Any temporary `AuthManager` bridge must
wrap this session outside the common contract rather than extending the trait with storage-shaped
methods.

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

If a one-time compatibility migration remains in the product, it must behave equivalently to
`accounts import-legacy` and record completion so that routine startup no longer performs implicit
imports.

This command is a local compatibility helper, not a backend-neutral control-plane verb.

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

### Crash-safe pending registration

In-process compensation is not enough on its own. Before contacting the backend, the caller should
persist a pending-registration record keyed by `idempotency_key` that captures:

- requested backend
- provider kind
- target pool assignment intent
- registration start time

On success, the record is finalized and removed or marked complete with the resulting
`backend_account_handle` and `account_id`.

On retry or process restart, the control plane reconciles any pending record before starting a new
registration attempt. This prevents orphaned backend registrations after crashes.

Operator-visible `accounts add ...` and `accounts import-legacy` flows should always generate and
persist a stable `idempotency_key` before contacting the backend. The shared control-plane request
shape therefore treats `idempotency_key` as required.

### Removal and logout semantics

`accounts remove` must route through the control plane and clean up both:

- local control-plane state such as registry and membership rows
- backend-owned credential material such as a backend-private namespace or remote registration

The command must not silently delete only the registry while leaving backend-owned credential state
behind.

Recommended behavior:

- attempt backend credential deletion or registration revocation as part of the remove operation
- if backend cleanup fails before control-plane deletion, fail without deleting local metadata
- if backend cleanup fails after partial local mutation, surface an explicit partial-failure error
  that includes the backend handle and required follow-up action

`logout` remains a legacy compatibility command:

- it revokes any active process-local pooled lease
- it clears legacy compatibility auth
- it enables durable default-startup suppression only for managed or persisted legacy auth modes
- it remains runtime-local and non-durable for ephemeral `chatgptAuthTokens`-style auth
- it must not delete pooled registrations or backend-private pooled credential namespaces

## Migration Strategy

Recommended sequence:

1. Add backend-neutral control-plane and execution-plane contracts without changing user-visible
   behavior, and retire steady-state legacy auto-import from runtime and generic CLI startup.
2. Extend schema for `backend_id`, `backend_account_handle`, `provider_fingerprint`,
   `account_pool_membership`, and a crash-recovery `pending_account_registration` journal.
3. Backfill `account_pool_membership` from existing `account_registry.pool_id` data and keep the
   legacy columns synchronized as compatibility fields while readers and writers migrate.
4. Change execution to consume `LeaseGrant { auth_session }`, with request-path code using only
   `LeasedTurnAuth` snapshots and non-request consumers holding invalidation-aware lease-scoped
   sessions.
5. Add a legacy-only bridge for remaining `AuthManager`-shaped consumers that cannot move in the
   same slice, and remove request-path reliance on `AuthManager` retry/reload behavior.
6. Enable `accounts add chatgpt`, `accounts add chatgpt --device-auth`, and
   `accounts import-legacy`.
7. Add `accounts add api-key` as a follow-up slice if needed.

This keeps the highest-risk auth materialization change isolated from the CLI surface change.

## Testing And Validation

The implementation should be validated in three layers.

### 1. Control-plane tests

Verify:

- registration dedupe
- idempotent re-add
- explicit legacy import behavior
- no steady-state implicit legacy import during routine startup
- one-account-one-membership constraints
- membership backfill and synchronized compatibility-column behavior during cutover
- membership assignment semantics
- compensation delete on persistence failure
- crash recovery for pending registrations keyed by `idempotency_key`
- `accounts remove` deletes backend-owned credential state or fails explicitly
- `logout` revokes the active pooled lease, applies durable suppression only for legacy modes, and
  does not delete pooled backend-owned credential state
- registered-but-unassigned accounts do not participate in automatic selection

### 2. Execution-plane tests

Verify:

- lease acquisition returns a leased auth session that yields usable `LeasedTurnAuth`
- local pooled auth loads from backend-private namespaces
- managed refresh continues to work for lease-private auth sessions
- request retries stay on lease-scoped snapshots and do not reload from the shared legacy manager
- lease-scoped sessions fail closed on release/rotation and require rebinding via a new `LeaseGrant`
- refresh preserves stable account identity for the full lease lifetime
- phase 1 `AuthManager`-shaped bridge consumers can read from the leased auth session without
  falling back to the shared legacy manager
- automatic future-turn rotation still works after the new lease grant path
- exclusive lease behavior still prevents multiple local instances from sharing one account

### 3. Product-surface tests

Verify:

- `accounts add chatgpt`
- `accounts import-legacy`
- `accounts current/status --json`
- routine startup does not auto-import legacy auth after the migration step has completed
- `accounts switch/resume/disable/remove/pool assign`
- TUI `/status` pooled lease display
- visible history notice when automatic switching occurs

## Recommendation

Implement phase 1 with:

- backend-neutral control-plane and execution-plane contracts,
- a local backend that reuses existing auth storage implementations via backend-private namespaces,
- `LeaseGrant` carrying a refresh-capable leased auth session,
- explicit `accounts import-legacy`,
- and ChatGPT registration as the first fully working add path.

This is the smallest design that keeps the current fork merge-friendly while preserving a clean
upgrade path to a remote account-pool service that owns real credentials and issues temporary auth
material at lease time.
