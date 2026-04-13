# Pooled Account Registration Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a merge-friendly phase 1 pooled-account registration stack with explicit `accounts import-legacy`, working `accounts add chatgpt` and `accounts add chatgpt --device-auth`, backend-private credential storage, lease-scoped auth sessions, and aligned logout/remove semantics.

**Architecture:** Keep `codex-state` as the owner of catalog, membership, and crash-recovery journal data, while `codex-account-pool` owns the backend-neutral control-plane and execution-plane contracts. `codex-login` owns provider token acquisition plus lease-scoped auth sessions, and `codex-core` consumes immutable `LeasedTurnAuth` snapshots for request execution while temporary bridges keep non-request consumers working until they are migrated.

**Tech Stack:** Rust workspace crates (`codex-state`, `codex-account-pool`, `codex-login`, `codex-core`, `codex-cli`, `codex-app-server`), SQLite via `sqlx`, clap, existing browser and device-code ChatGPT login flows, app-server v2 JSON-RPC, `pretty_assertions`, and targeted crate tests.

---

## Scope

This plan implements the addendum in:

- `docs/superpowers/specs/2026-04-13-pooled-account-registration-design.md`
- `docs/superpowers/specs/2026-04-10-multi-account-pool-design.md`

In scope:

- backend-owned pooled registration metadata
- `account_pool_membership` and `pending_account_registration`
- explicit `accounts import-legacy`
- local backend control plane and execution plane split
- lease-scoped auth sessions with stable identity across refresh
- request-path isolation from shared `AuthManager`
- working `accounts add chatgpt` and `accounts add chatgpt --device-auth`
- backend-private pooled credential cleanup on `accounts remove`
- legacy logout/app-server logout alignment with the revised spec

Out of scope for this plan:

- remote backend implementation
- multi-pool membership for one account
- `accounts add api-key`
- new pooled account management UI
- workspace-wide `cargo test` without explicit user approval

## Execution Rules

- Use `@superpowers:test-driven-development` before each implementation task.
- Use `@superpowers:verification-before-completion` before claiming a task is done.
- Run only the targeted tests listed in each task unless the user explicitly approves a full `cargo test`.
- Run `just fmt` after each Rust code task and `just fix -p <crate>` for the crates touched by that task.
- Do not rerun tests after `just fmt` or `just fix -p ...`; test before formatting/fixing, then commit.

## Planned File Layout

- Create `codex-rs/state/migrations/0029_pooled_account_registration.sql` for the new catalog fields, membership table, and pending-registration journal.
- Create `codex-rs/state/src/runtime/account_pool_control.rs` so registration, membership, and crash-recovery code does not make `codex-rs/state/src/runtime/account_pool.rs` even larger.
- Modify `codex-rs/state/src/model/account_pool.rs` to add typed registration, membership, and pending-registration models that hide raw SQL details from callers.
- Split `codex-rs/account-pool/src/backend/local.rs` into `codex-rs/account-pool/src/backend/local/mod.rs`, `control.rs`, and `execution.rs` so control-plane code and lease code do not grow into one mixed backend file.
- Create `codex-rs/login/src/pooled_registration.rs` for non-persisting ChatGPT token acquisition helpers used by pooled registration, separate from legacy `codex login`.
- Create `codex-rs/login/src/auth/lease_scoped_session.rs` for the backend-neutral `LeaseScopedAuthSession` trait, the local lease-backed session implementation, and any temporary bridge helpers.
- Create `codex-rs/core/src/lease_auth.rs` for the session-local lease-auth holder so `codex-rs/core/src/client.rs` and `codex-rs/core/src/state/service.rs` do not absorb all of the new pooled auth logic directly.
- Create `codex-rs/cli/src/accounts/registration.rs` so `codex-rs/cli/src/accounts/mod.rs` stays focused on clap wiring and top-level dispatch.
- Keep app-server changes narrowly scoped to `codex-rs/app-server/src/account_lease_api.rs` and `codex-rs/app-server/src/codex_message_processor.rs`; do not add new protocol surface unless a failing test proves it is required.

### Task 1: Add Registration Schema, Membership State, and Crash-Recovery Journal

**Files:**
- Create: `codex-rs/state/migrations/0029_pooled_account_registration.sql`
- Create: `codex-rs/state/src/runtime/account_pool_control.rs`
- Modify: `codex-rs/state/src/model/account_pool.rs`
- Modify: `codex-rs/state/src/runtime.rs`
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Test: `codex-rs/state/src/runtime/account_pool.rs`

- [ ] **Step 1: Write the failing state tests**

Add focused tests near the existing account-pool runtime tests:

```rust
#[tokio::test]
async fn membership_backfill_reads_new_membership_table_and_keeps_compat_columns_synced() {
    let runtime = StateRuntime::init(unique_temp_dir().await, "test-provider".to_string()).await.unwrap();
    seed_registry_row(&runtime, "acct-1", "team-main", 0).await;

    let membership = runtime.read_account_pool_membership("acct-1").await.unwrap().unwrap();

    assert_eq!(membership.account_id, "acct-1");
    assert_eq!(membership.pool_id, "team-main");
}

#[tokio::test]
async fn pending_registration_round_trip_is_keyed_by_idempotency_key() {
    let runtime = StateRuntime::init(unique_temp_dir().await, "test-provider".to_string()).await.unwrap();
    runtime.create_pending_account_registration(NewPendingAccountRegistration {
        idempotency_key: "idem-1".to_string(),
        backend_id: "local".to_string(),
        provider_kind: "chatgpt".to_string(),
        target_pool_id: Some("team-main".to_string()),
    }).await.unwrap();

    let pending = runtime.read_pending_account_registration("idem-1").await.unwrap().unwrap();

    assert_eq!(pending.idempotency_key, "idem-1");
    assert_eq!(pending.target_pool_id.as_deref(), Some("team-main"));
}
```

- [ ] **Step 2: Run the state tests to verify failure**

Run: `cargo test -p codex-state account_pool -- --nocapture`  
Expected: FAIL because the new tables and runtime methods do not exist.

- [ ] **Step 3: Add the migration and compatibility-column rules**

In `0029_pooled_account_registration.sql`, add:

```sql
ALTER TABLE account_registry ADD COLUMN backend_id TEXT NOT NULL DEFAULT 'local';
ALTER TABLE account_registry ADD COLUMN backend_account_handle TEXT;
ALTER TABLE account_registry ADD COLUMN provider_fingerprint TEXT;
ALTER TABLE account_registry ADD COLUMN display_name TEXT;

CREATE TABLE account_pool_membership (
    account_id TEXT PRIMARY KEY,
    pool_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    assigned_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES account_registry(account_id) ON DELETE CASCADE
);

CREATE TABLE pending_account_registration (
    idempotency_key TEXT PRIMARY KEY,
    backend_id TEXT NOT NULL,
    provider_kind TEXT NOT NULL,
    target_pool_id TEXT,
    backend_account_handle TEXT,
    account_id TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE account_compat_migration_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    legacy_import_completed INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX account_registry_backend_fingerprint_idx
ON account_registry(backend_id, provider_fingerprint);

CREATE UNIQUE INDEX account_registry_backend_handle_idx
ON account_registry(backend_id, backend_account_handle);
```

Also backfill `account_pool_membership` from existing `account_registry.pool_id` / `position`, and leave `account_registry.pool_id`, `position`, and `account_runtime_state.pool_id` synchronized for the transition slice.
Initialize `account_compat_migration_state.singleton = 1` with `legacy_import_completed = 0` for
upgraded installs until the explicit import/migration path marks it complete.

- [ ] **Step 4: Add typed state models and CRUD helpers**

In `codex-rs/state/src/model/account_pool.rs`, add types along these lines:

```rust
pub struct RegisteredAccountRecord {
    pub account_id: String,
    pub backend_id: String,
    pub backend_account_handle: String,
    pub account_kind: String,
    pub provider_fingerprint: String,
    pub display_name: Option<String>,
    pub source: Option<AccountSource>,
    pub enabled: bool,
    pub healthy: bool,
}

pub struct PendingAccountRegistration {
    pub idempotency_key: String,
    pub backend_id: String,
    pub provider_kind: String,
    pub target_pool_id: Option<String>,
    pub backend_account_handle: Option<String>,
    pub account_id: Option<String>,
}
```

In `codex-rs/state/src/runtime/account_pool_control.rs`, add methods such as:

```rust
impl StateRuntime {
    pub async fn upsert_registered_account(&self, entry: RegisteredAccountUpsert) -> anyhow::Result<()>;
    pub async fn assign_account_pool_membership(&self, account_id: &str, pool_id: &str) -> anyhow::Result<()>;
    pub async fn create_pending_account_registration(&self, entry: NewPendingAccountRegistration) -> anyhow::Result<()>;
    pub async fn list_pending_account_registrations(&self) -> anyhow::Result<Vec<PendingAccountRegistration>>;
    pub async fn read_pending_account_registration(&self, idempotency_key: &str) -> anyhow::Result<Option<PendingAccountRegistration>>;
    pub async fn finalize_pending_account_registration(&self, idempotency_key: &str, backend_account_handle: &str, account_id: &str) -> anyhow::Result<()>;
    pub async fn clear_pending_account_registration(&self, idempotency_key: &str) -> anyhow::Result<()>;
    pub async fn read_account_compat_migration_state(&self) -> anyhow::Result<AccountCompatMigrationState>;
    pub async fn write_account_compat_migration_state(&self, completed: bool) -> anyhow::Result<()>;
}
```

Keep `read_account_pool_membership`, `assign_account_pool`, `record_account_health_event`, and diagnostics working through the new source of truth instead of continuing to treat `account_registry.pool_id` as authoritative.

- [ ] **Step 5: Run the targeted state tests**

Run: `cargo test -p codex-state account_pool -- --nocapture`  
Expected: PASS.

- [ ] **Step 6: Format, lint, and commit**

Run: `just fmt`  
Run: `just fix -p codex-state`

```bash
git add codex-rs/state/migrations/0029_pooled_account_registration.sql codex-rs/state/src/model/account_pool.rs codex-rs/state/src/runtime.rs codex-rs/state/src/runtime/account_pool.rs codex-rs/state/src/runtime/account_pool_control.rs codex-rs/state/src/lib.rs
git commit -m "feat(state): add pooled registration persistence"
```

### Task 2: Split Control Plane from Execution Plane and Retire Implicit Bootstrap

**Files:**
- Modify: `codex-rs/account-pool/src/backend.rs`
- Modify: `codex-rs/account-pool/src/bootstrap.rs`
- Modify: `codex-rs/account-pool/src/lib.rs`
- Modify: `codex-rs/account-pool/src/manager.rs`
- Move: `codex-rs/account-pool/src/backend/local.rs` to `codex-rs/account-pool/src/backend/local/mod.rs`
- Create: `codex-rs/account-pool/src/backend/local/control.rs`
- Create: `codex-rs/account-pool/src/backend/local/execution.rs`
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Create: `codex-rs/cli/src/accounts/registration.rs`
- Test: `codex-rs/account-pool/tests/lease_lifecycle.rs`
- Test: `codex-rs/cli/tests/accounts.rs`

- [ ] **Step 1: Write the failing “explicit import only” tests**

Add tests that lock the new semantics:

```rust
#[tokio::test]
async fn ensure_active_lease_does_not_bootstrap_legacy_auth_when_startup_state_is_empty() {
    let mut manager = test_manager_with_legacy_auth("acct-legacy").await;

    let err = manager.ensure_active_lease(SelectionRequest::default()).await.unwrap_err();

    assert!(err.to_string().contains("no eligible account"));
}

#[tokio::test]
async fn accounts_list_no_longer_bootstraps_legacy_auth_into_pooled_state() -> Result<()> {
    let codex_home = prepared_legacy_auth_only_home().await?;

    let output = run_codex(&codex_home, &["accounts", "list"]).await?;

    assert!(output.success);
    assert!(output.stdout.trim().is_empty());
    assert!(!state_db_path(codex_home.path()).exists());
    Ok(())
}

#[tokio::test]
async fn accounts_import_legacy_registers_and_assigns_legacy_account_explicitly() -> Result<()> {
    let codex_home = prepared_legacy_auth_only_home().await?;

    let output = run_codex(&codex_home, &["accounts", "--account-pool", "team-main", "import-legacy", "--pool", "team-main"]).await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert_eq!(read_pool_membership(&codex_home, "acct-1").await?.unwrap().pool_id, "team-main");
    assert!(read_account_compat_migration_state(&codex_home).await?.legacy_import_completed);
    Ok(())
}
```

- [ ] **Step 2: Run the account-pool and CLI tests to verify failure**

Run: `cargo test -p codex-account-pool lease_lifecycle -- --nocapture`  
Run: `cargo test -p codex-cli --test accounts`  
Expected: FAIL because the manager and CLI still auto-import legacy auth.

- [ ] **Step 3: Split the backend-neutral traits**

In `codex-rs/account-pool/src/backend.rs`, replace the current mixed trait with two surfaces:

```rust
#[async_trait]
pub trait AccountPoolExecutionBackend: Send + Sync {
    async fn acquire_lease(&self, pool_id: &str, holder_instance_id: &str) -> Result<LeaseGrant, AccountLeaseError>;
    async fn renew_lease(&self, lease: &LeaseKey, now: DateTime<Utc>) -> anyhow::Result<LeaseRenewal>;
    async fn release_lease(&self, lease: &LeaseKey, now: DateTime<Utc>) -> anyhow::Result<bool>;
    async fn record_health_event(&self, event: AccountHealthEvent) -> anyhow::Result<()>;
    async fn read_startup_selection(&self) -> anyhow::Result<AccountStartupSelectionState>;
}

#[async_trait]
pub trait AccountPoolControlPlane: Send + Sync {
    async fn register_account(&self, request: RegisterAccountRequest) -> anyhow::Result<RegisteredAccount>;
    async fn delete_registered_account(&self, backend_account_handle: &str) -> anyhow::Result<()>;
}
```

Do not keep `import_legacy_default_account(...)` on the backend-neutral trait.

- [ ] **Step 4: Remove implicit bootstrap from runtime and generic CLI startup**

In `codex-rs/account-pool/src/manager.rs`, delete `bootstrap_from_legacy_auth()` from normal lease acquisition.  
In `codex-rs/cli/src/accounts/mod.rs`, remove `bootstrap_from_legacy_auth_if_needed()` from the generic `accounts` path.

Keep a local-only helper in `codex-rs/cli/src/accounts/registration.rs`:

```rust
pub(crate) async fn import_legacy_account(runtime: &StateRuntime, config: &Config) -> anyhow::Result<ImportedLegacyAccount> {
    let auth_manager = AuthManager::shared_from_config(config, /*enable_codex_api_key_env*/ true);
    let legacy_auth = LegacyAuthView::new(&auth_manager).current().await;
    // derive provider fingerprint + account id, then call state/control-plane helpers
}
```

If legacy auth exists on a pre-upgrade install, initialize a recorded compatibility marker in
`account_compat_migration_state`, but do not perform ordinary startup-time import anymore.

- [ ] **Step 5: Implement the explicit `accounts import-legacy` command**

Add a clap variant:

```rust
pub struct ImportLegacyCommand {
    #[arg(long = "pool", value_name = "POOL_ID")]
    pub pool: Option<String>,
}

pub enum AccountsSubcommand {
    Add(AddAccountCommand),
    ImportLegacy(ImportLegacyCommand),
    // existing commands...
}
```

`accounts import-legacy` should:

- read the current legacy auth snapshot
- derive account identity and provider fingerprint
- use `--pool` first, then the top-level `--account-pool` override if present, then fall back to
  `legacy-default`
- create and reconcile a `pending_account_registration` row keyed by a stable `idempotency_key`
  before any new pooled record is written
- reuse an existing pooled record if one already exists
- otherwise write a new registered account + default membership
- preserve existing provenance instead of relabeling it
- mark `account_compat_migration_state.legacy_import_completed = true`

Use the same crash-recovery semantics as `accounts add`: clear the pending row on pre-backend
failure, finalize it on success, and let reconciliation clean up any partially completed attempt on
the next run.

- [ ] **Step 6: Run the targeted tests**

Run: `cargo test -p codex-account-pool lease_lifecycle -- --nocapture`  
Run: `cargo test -p codex-cli --test accounts`  
Expected: PASS.

- [ ] **Step 7: Format, lint, and commit**

Run: `just fmt`  
Run: `just fix -p codex-account-pool`  
Run: `just fix -p codex-cli`

```bash
git add codex-rs/account-pool/src/backend.rs codex-rs/account-pool/src/bootstrap.rs codex-rs/account-pool/src/lib.rs codex-rs/account-pool/src/manager.rs codex-rs/account-pool/src/backend/local codex-rs/cli/src/accounts/mod.rs codex-rs/cli/src/accounts/registration.rs codex-rs/account-pool/tests/lease_lifecycle.rs codex-rs/cli/tests/accounts.rs
git commit -m "feat(accounts): make legacy import explicit"
```

### Task 3: Add Pooled Registration Helpers and Lease-Scoped Auth Sessions in `codex-login`

**Files:**
- Create: `codex-rs/login/src/pooled_registration.rs`
- Create: `codex-rs/login/src/auth/lease_scoped_session.rs`
- Modify: `codex-rs/login/src/auth/leased_auth.rs`
- Modify: `codex-rs/login/src/auth/mod.rs`
- Modify: `codex-rs/login/src/lib.rs`
- Modify: `codex-rs/login/src/server.rs`
- Modify: `codex-rs/login/src/device_code_auth.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`
- Modify: `codex-rs/account-pool/src/types.rs`
- Modify: `codex-rs/account-pool/src/backend/local/control.rs`
- Modify: `codex-rs/account-pool/src/backend/local/execution.rs`
- Test: `codex-rs/login/tests/suite/auth_seams.rs`
- Create: `codex-rs/login/tests/suite/pooled_registration.rs`
- Test: `codex-rs/account-pool/tests/lease_lifecycle.rs`

- [ ] **Step 1: Write the failing login and local-backend tests**

Add tests that protect the new seam:

```rust
#[tokio::test]
async fn pooled_browser_registration_returns_tokens_without_writing_shared_auth() -> Result<()> {
    let codex_home = tempdir()?;
    let tokens = run_pooled_browser_registration(test_server_options(codex_home.path())).await?;

    assert!(codex_home.path().join("auth.json").exists() == false);
    assert_eq!(tokens.account_id.as_deref(), Some("acct-1"));
    Ok(())
}

#[tokio::test]
async fn local_lease_scoped_session_refresh_preserves_stable_account_identity() -> Result<()> {
    let session = seed_local_lease_session("acct-1").await?;

    let before = session.binding().account_id.clone();
    let _ = session.refresh_leased_turn_auth()?;
    let after = session.binding().account_id.clone();

    assert_eq!(before, after);
    Ok(())
}
```

- [ ] **Step 2: Run the login and account-pool tests to verify failure**

Run: `cargo test -p codex-login auth_seams -- --nocapture`  
Run: `cargo test -p codex-login pooled_registration -- --nocapture`  
Run: `cargo test -p codex-account-pool lease_lifecycle -- --nocapture`  
Expected: FAIL because pooled registration helpers and `LeaseScopedAuthSession` do not exist.

- [ ] **Step 3: Extract non-persisting ChatGPT token acquisition helpers**

In `codex-rs/login/src/pooled_registration.rs`, add provider-level helpers that stop before writing shared auth:

```rust
pub struct ChatgptManagedRegistrationTokens {
    pub id_token: String,
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub account_id: String,
}

pub async fn run_pooled_browser_registration(opts: ServerOptions) -> io::Result<ChatgptManagedRegistrationTokens>;
pub async fn run_pooled_device_code_registration(opts: ServerOptions) -> io::Result<ChatgptManagedRegistrationTokens>;
```

Refactor `server.rs` and `device_code_auth.rs` so the legacy `codex login` path still persists shared auth, but pooled registration can reuse the OAuth exchange without touching `CODEX_HOME/auth.json`.

- [ ] **Step 4: Add the lease-scoped auth session seam**

In `codex-rs/login/src/auth/lease_scoped_session.rs`, add:

```rust
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
```

Add a local implementation backed by:

`CODEX_HOME/.pooled-auth/backends/local/accounts/<backend_account_handle>/`

Refresh must fail closed if the persisted auth rebinds to a different account identity.

- [ ] **Step 5: Return `LeaseGrant { auth_session }` from the local execution backend**

In `codex-rs/account-pool/src/types.rs`, replace the `LeasedAccount`-only return shape with:

```rust
pub struct LeaseGrant {
    pub lease_key: LeaseKey,
    pub account_id: String,
    pub pool_id: String,
    pub auth_session: Arc<dyn LeaseScopedAuthSession>,
    pub expires_at: DateTime<Utc>,
    pub next_eligible_at: Option<DateTime<Utc>>,
}
```

The local control plane should persist pooled auth in the backend-private namespace, return a `backend_account_handle`, and the local execution backend should materialize a lease-scoped session from that namespace on `acquire_lease`.

- [ ] **Step 6: Run the targeted tests**

Run: `cargo test -p codex-login auth_seams -- --nocapture`  
Run: `cargo test -p codex-login pooled_registration -- --nocapture`  
Run: `cargo test -p codex-account-pool lease_lifecycle -- --nocapture`  
Expected: PASS.

- [ ] **Step 7: Format, lint, and commit**

Run: `just fmt`  
Run: `just fix -p codex-login`  
Run: `just fix -p codex-account-pool`

```bash
git add codex-rs/login/src/pooled_registration.rs codex-rs/login/src/auth/lease_scoped_session.rs codex-rs/login/src/auth/leased_auth.rs codex-rs/login/src/auth/mod.rs codex-rs/login/src/lib.rs codex-rs/login/src/server.rs codex-rs/login/src/device_code_auth.rs codex-rs/login/tests/suite/auth_seams.rs codex-rs/login/tests/suite/pooled_registration.rs codex-rs/account-pool/src/backend.rs codex-rs/account-pool/src/types.rs codex-rs/account-pool/src/backend/local
git commit -m "feat(login): add pooled registration and lease auth sessions"
```

### Task 4: Move `codex-core` Request Execution onto Lease-Scoped Auth

**Files:**
- Create: `codex-rs/core/src/lease_auth.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/src/client.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/codex_thread.rs`
- Modify: `codex-rs/core/src/plugins/manager.rs`
- Modify: `codex-rs/cloud-requirements/src/lib.rs`
- Modify: `codex-rs/analytics/src/client.rs`
- Test: `codex-rs/core/tests/suite/account_pool.rs`
- Test: `codex-rs/login/tests/suite/auth_seams.rs`

- [ ] **Step 1: Write the failing pooled-auth integration tests**

Extend `codex-rs/core/tests/suite/account_pool.rs` with tests like:

```rust
#[tokio::test]
async fn unauthorized_retry_uses_leased_auth_session_not_shared_auth_manager() -> Result<()> {
    // Seed shared legacy auth for acct-shared and pooled lease auth for acct-lease.
    // Force an unauthorized retry and assert the second request still carries acct-lease auth.
    Ok(())
}

#[tokio::test]
async fn lease_rotation_invalidates_old_non_request_auth_bridge() -> Result<()> {
    // Run one turn on acct-1, rotate to acct-2, then assert plugin/cloud/analytics auth reads bind to acct-2.
    Ok(())
}
```

- [ ] **Step 2: Run the core tests to verify failure**

Run: `cargo test -p codex-core account_pool -- --nocapture`  
Expected: FAIL because request retries still use `AuthManager::unauthorized_recovery()` and session services still cache only the shared manager.

- [ ] **Step 3: Add a session-local lease-auth holder**

In `codex-rs/core/src/lease_auth.rs`, add a small holder such as:

```rust
pub(crate) struct SessionLeaseAuth {
    current: RwLock<Option<Arc<dyn LeaseScopedAuthSession>>>,
}

impl SessionLeaseAuth {
    pub(crate) fn install(&self, session: Arc<dyn LeaseScopedAuthSession>) { /* ... */ }
    pub(crate) fn leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> { /* ... */ }
}
```

Store it in `SessionServices` and wire it from pooled lease acquisition.

- [ ] **Step 4: Change request-path retries to stay on leased auth snapshots**

In `codex-rs/core/src/client.rs`, remove pooled request-path dependence on the shared `AuthManager`:

```rust
let leased_auth = self.client.state.lease_auth.leased_turn_auth()?;
let mut auth_recovery = leased_auth_session.map(LeaseScopedAuthSession::refresh_leased_turn_auth);
```

The shared `AuthManager` remains for legacy auth paths only. Request retries must never reload shared storage mid-turn.

- [ ] **Step 5: Add a temporary bridge for non-request consumers**

For code that still calls `auth_manager.auth().await` outside request execution (`plugins`, `cloud-requirements`, `analytics`), install a narrow adapter backed by the current lease-scoped session.  
The bridge must:

- fail closed when the old lease is released
- require a new bridge instance on lease rotation
- never silently mutate an old session to point at a different account

- [ ] **Step 6: Run the targeted core tests**

Run: `cargo test -p codex-core account_pool -- --nocapture`  
Run: `cargo test -p codex-login auth_seams -- --nocapture`  
Expected: PASS.

- [ ] **Step 7: Format, lint, and commit**

Run: `just fmt`  
Run: `just fix -p codex-core`

```bash
git add codex-rs/core/src/lease_auth.rs codex-rs/core/src/state/service.rs codex-rs/core/src/client.rs codex-rs/core/src/codex.rs codex-rs/core/src/codex_thread.rs codex-rs/core/src/plugins/manager.rs codex-rs/cloud-requirements/src/lib.rs codex-rs/analytics/src/client.rs codex-rs/core/tests/suite/account_pool.rs codex-rs/login/tests/suite/auth_seams.rs
git commit -m "feat(core): run pooled turns through lease auth sessions"
```

### Task 5: Implement `accounts add chatgpt` and `accounts add chatgpt --device-auth`

**Files:**
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Create: `codex-rs/cli/src/accounts/registration.rs`
- Modify: `codex-rs/cli/src/accounts/output.rs`
- Test: `codex-rs/cli/src/accounts/registration.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`
- Modify: `codex-rs/account-pool/src/backend/local/control.rs`
- Modify: `codex-rs/state/src/runtime/account_pool_control.rs`
- Modify: `codex-rs/state/src/model/account_pool.rs`
- Modify: `codex-rs/login/src/lib.rs`
- Test: `codex-rs/cli/tests/accounts.rs`

- [ ] **Step 1: Replace the current hard-fail tests with real add-flow tests**

Use two layers of tests:

1. unit tests in `codex-rs/cli/src/accounts/registration.rs` with a fake registration runner for
   the browser path
2. integration tests in `codex-rs/cli/tests/accounts.rs` for the device-auth command path and
   durable state changes

Add tests along these lines:

```rust
#[tokio::test]
async fn add_chatgpt_browser_path_registers_account_without_persisting_pool_override() -> Result<()> {
    let runner = FakeRegistrationRunner::browser_success("acct-2");
    let runtime = test_runtime().await?;

    let result = run_add_chatgpt_with_runner(&runner, &runtime, test_config(), Some("team-other"), false).await?;

    assert_eq!(result.account_id, "acct-2");
    assert_eq!(runtime.read_account_startup_selection().await?.default_pool_id.as_deref(), Some("team-main"));
    Ok(())
}

#[tokio::test]
async fn accounts_add_chatgpt_is_idempotent_for_same_provider_identity() -> Result<()> {
    let codex_home = prepared_home().await?;
    let first = run_codex(&codex_home, &["accounts", "add", "chatgpt", "--device-auth"]).await?;
    let second = run_codex(&codex_home, &["accounts", "add", "chatgpt", "--device-auth"]).await?;

    assert!(first.success, "stderr: {}", first.stderr);
    assert!(second.success, "stderr: {}", second.stderr);
    assert_eq!(list_registered_accounts(&codex_home).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn accounts_add_chatgpt_refuses_to_reuse_same_identity_in_different_pool() -> Result<()> {
    let codex_home = prepared_home().await?;
    let first = run_codex(&codex_home, &["accounts", "--account-pool", "team-main", "add", "chatgpt", "--device-auth"]).await?;
    let second = run_codex(&codex_home, &["accounts", "--account-pool", "team-other", "add", "chatgpt", "--device-auth"]).await?;

    assert!(first.success, "stderr: {}", first.stderr);
    assert!(!second.success, "stdout: {}", second.stdout);
    assert!(second.stderr.contains("accounts pool assign"));
    Ok(())
}

#[tokio::test]
async fn accounts_add_chatgpt_device_auth_registers_account_in_target_pool() -> Result<()> {
    let codex_home = prepared_home().await?;
    let output = run_codex(&codex_home, &["accounts", "--account-pool", "team-other", "add", "chatgpt", "--device-auth"]).await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert_eq!(read_pool_membership(&codex_home, "acct-2").await?.unwrap().pool_id, "team-other");
    Ok(())
}
```

The real browser/device OAuth exchange is already covered in `codex-rs/login/tests/suite/login_server_e2e.rs`
and `codex-rs/login/tests/suite/device_code_login.rs`; CLI tests should focus on command orchestration,
idempotency, pool assignment, and state persistence instead of re-testing interactive UX.

- [ ] **Step 2: Run the CLI test to verify failure**

Run: `cargo test -p codex-cli --test accounts`  
Expected: FAIL because `accounts add` still bails out before config loading or registration.

- [ ] **Step 3: Implement the crash-safe registration workflow**

In `codex-rs/cli/src/accounts/registration.rs`, build a workflow like:

```rust
#[async_trait]
trait ChatgptRegistrationRunner {
    async fn run_browser(&self, config: &Config) -> anyhow::Result<ChatgptManagedRegistrationTokens>;
    async fn run_device_auth(&self, config: &Config) -> anyhow::Result<ChatgptManagedRegistrationTokens>;
}

pub(crate) async fn run_add_chatgpt(
    runtime: &StateRuntime,
    config: &Config,
    pool_override: Option<&str>,
    device_auth: bool,
) -> anyhow::Result<()> {
    reconcile_pending_pooled_registrations(runtime, &local_control_plane).await?;
    let idempotency_key = generate_idempotency_key();
    runtime.create_pending_account_registration(/* ... */).await?;
    let tokens = if device_auth {
        runner.run_device_auth(config).await?
    } else {
        runner.run_browser(config).await?
    };
    let registered = local_control_plane.register_account(RegisterAccountRequest { /* ... */ }).await?;
    runtime.finalize_pending_account_registration(&idempotency_key, &registered.backend_account_handle, &registered.account_id).await?;
}
```

`reconcile_pending_pooled_registrations(...)` should:

- scan `list_pending_account_registrations()`
- finalize rows that already have both `backend_account_handle` and `account_id`
- delete orphaned backend registrations when a handle exists but local commit never finished
- clear rows that never reached backend registration
- run before a fresh add/import attempt so operator retries do not create duplicates

If token acquisition fails before backend registration completes, clear the pending row immediately.
If backend registration succeeds but local persistence fails, attempt backend deletion, then either
clear the row on success or leave it for the next reconciliation pass with the backend handle
recorded.

On local failure after backend registration, call `delete_registered_account(backend_account_handle)` before returning the error.

- [ ] **Step 4: Assign into the correct pool without mutating durable defaults**

Use this precedence only for the add flow:

1. `--account-pool` override
2. `accounts.default_pool`
3. leave unassigned

Do not persist the override into `account_startup_selection.default_pool_id`.
If the same `(backend_id, provider_fingerprint)` is already registered in a different pool, fail
with a message directing the operator to `accounts pool assign` instead of silently moving it.

- [ ] **Step 5: Remove the legacy `credential_ref` gap text and surface real add results**

Delete `ACCOUNTS_ADD_CREDENTIAL_STORAGE_GAP`.  
Print concise output such as:

```text
registered account acct-2 in pool team-main
```

Keep `backend_account_handle` out of normal user-facing output.

- [ ] **Step 6: Run the targeted tests**

Run: `cargo test -p codex-cli --test accounts`  
Run: `cargo test -p codex-state account_pool -- --nocapture`  
Run: `cargo test -p codex-login pooled_registration -- --nocapture`  
Expected: PASS.

- [ ] **Step 7: Format, lint, and commit**

Run: `just fmt`  
Run: `just fix -p codex-cli`  
Run: `just fix -p codex-state`  
Run: `just fix -p codex-login`

```bash
git add codex-rs/cli/src/accounts/mod.rs codex-rs/cli/src/accounts/registration.rs codex-rs/cli/src/accounts/output.rs codex-rs/cli/tests/accounts.rs codex-rs/account-pool/src/backend/local/control.rs codex-rs/state/src/runtime/account_pool_control.rs codex-rs/state/src/model/account_pool.rs codex-rs/login/src/lib.rs
git commit -m "feat(cli): add pooled ChatGPT account registration"
```

### Task 6: Implement Removal Cleanup and Align Legacy Logout/App-Server Semantics

**Files:**
- Modify: `codex-rs/account-pool/src/backend/local/control.rs`
- Modify: `codex-rs/cli/src/accounts/mutate.rs`
- Modify: `codex-rs/cli/src/login.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_lease.rs`
- Modify: `codex-rs/app-server/README.md`

- [ ] **Step 1: Write the failing cleanup and logout tests**

Add tests such as:

```rust
#[tokio::test]
async fn accounts_remove_deletes_backend_private_namespace() -> Result<()> {
    let codex_home = prepared_home_with_registered_pooled_account().await?;

    let output = run_codex(&codex_home, &["accounts", "remove", "acct-1"]).await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(!pooled_account_namespace(codex_home.path(), "acct-1").exists());
    Ok(())
}

#[tokio::test]
async fn logout_only_persists_suppression_for_managed_or_persisted_legacy_auth_modes() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["logout"]).await?;

    assert!(output.success);
    assert_eq!(read_startup_selection(&codex_home).await?.suppressed, true);
    Ok(())
}
```

In `codex-rs/app-server/tests/suite/v2/account_lease.rs`, add:

```rust
#[tokio::test]
async fn account_logout_revokes_live_pooled_lease_before_emitting_updated_notification() -> Result<()> {
    // Start a pooled thread, submit a turn, then call account/logout.
    // Assert accountLease/updated reports active=false and suppressed according to auth mode.
    Ok(())
}
```

- [ ] **Step 2: Run the targeted tests to verify failure**

Run: `cargo test -p codex-cli --test accounts`  
Run: `cargo test -p codex-app-server account_lease -- --nocapture`  
Expected: FAIL because remove only deletes registry state and app-server logout only writes suppression.

- [ ] **Step 3: Route `accounts remove` through backend cleanup**

In the local control plane, implement:

```rust
async fn delete_registered_account(&self, backend_account_handle: &str) -> anyhow::Result<()> {
    delete_backend_private_namespace(namespace_for_handle(backend_account_handle))?;
    runtime.remove_registered_account(/* account_id */).await?;
}
```

If namespace deletion fails before the state mutation, fail without deleting local metadata.  
If the failure happens after partial local mutation, surface the backend handle in the error.

- [ ] **Step 4: Align CLI and app-server logout behavior with the new spec**

For CLI:

- inspect the legacy auth mode before suppression
- only persist durable suppression for managed or persisted legacy auth modes
- leave runtime-local `chatgptAuthTokens` non-durable

For app-server:

- release the active process-local pooled lease before writing suppression
- emit `accountLease/updated`
- keep runtime-local `chatgptAuthTokens` non-durable

- [ ] **Step 5: Update the app-server README only for behavior that actually changed**

Document:

- `account/logout` revokes the active pooled lease
- durable suppression applies only to managed/persisted legacy auth modes
- runtime-local `chatgptAuthTokens` remain non-durable

- [ ] **Step 6: Run the targeted tests**

Run: `cargo test -p codex-cli --test accounts`  
Run: `cargo test -p codex-app-server account_lease -- --nocapture`  
Expected: PASS.

- [ ] **Step 7: Format, lint, and commit**

Run: `just fmt`  
Run: `just fix -p codex-cli`  
Run: `just fix -p codex-app-server`

```bash
git add codex-rs/account-pool/src/backend/local/control.rs codex-rs/cli/src/accounts/mutate.rs codex-rs/cli/src/login.rs codex-rs/cli/tests/accounts.rs codex-rs/app-server/src/account_lease_api.rs codex-rs/app-server/src/codex_message_processor.rs codex-rs/app-server/tests/suite/v2/account_lease.rs codex-rs/app-server/README.md
git commit -m "fix(accounts): clean up pooled credentials and align logout"
```

### Task 7: Final Verification, Generated Artifacts, and Workspace Gate

**Files:**
- Modify if needed: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify if needed: `codex-rs/app-server/README.md`
- Modify if needed: `codex-rs/core/config.schema.json`

- [ ] **Step 1: Run the full targeted crate matrix**

Run:

```bash
cargo test -p codex-state account_pool -- --nocapture
cargo test -p codex-account-pool lease_lifecycle -- --nocapture
cargo test -p codex-login auth_seams -- --nocapture
cargo test -p codex-login pooled_registration -- --nocapture
cargo test -p codex-core account_pool -- --nocapture
cargo test -p codex-cli --test accounts
cargo test -p codex-app-server account_lease -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Regenerate artifacts only if the implementation changed those surfaces**

If any v2 app-server protocol shape changed:

Run: `just write-app-server-schema`  
Run: `cargo test -p codex-app-server-protocol`

If any config TOML types changed:

Run: `just write-config-schema`

Do not regenerate schemas if tests prove the implementation stayed within existing wire/config shapes.

- [ ] **Step 3: Run final formatting and scoped lints**

Run: `just fmt`

Then run `just fix -p <crate>` for each crate touched in the last uncommitted slice. Do not rerun tests afterward.

- [ ] **Step 4: Ask for approval before the full workspace test**

Prompt the user before:

```bash
cargo test
```

Reason: this repo requires user confirmation before the complete workspace suite, and the earlier targeted matrix should already prove the feature slice.

- [ ] **Step 5: Commit any generated docs/schema drift if Step 2 changed files**

```bash
git add codex-rs/app-server/README.md codex-rs/app-server-protocol codex-rs/core/config.schema.json
git commit -m "docs: finalize pooled registration artifacts"
```

Skip this commit if Step 2 did not modify tracked files.
