# Multi-Account Pool V1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a merge-friendly local multi-account pool with proactive future-turn failover, compatibility-preserving auth seams, and a narrow v1 app-server/CLI/TUI surface.

**Architecture:** Introduce a new `codex-account-pool` strategy crate that owns lease selection and policy while `codex-state` owns all pooled SQLite schema and persistence, `codex-login` remains the only owner of legacy auth storage, and `codex-core` consumes lease-bound `LeasedTurnAuth` plus a distinct outbound `RemoteSessionId`. V1 keeps pooled app-server support limited to `stdio` with one loaded/running thread, preserves `chatgptAuthTokens` as runtime-local external auth, and disables current-turn replay on current Responses transports.

**Tech Stack:** Rust workspace crates (`codex-account-pool`, `codex-state`, `codex-login`, `codex-core`, `codex-cli`, `codex-app-server`, `codex-app-server-protocol`, `codex-tui`, `codex-config`), SQLite via `sqlx`, clap, ratatui snapshot tests, existing app-server v2 JSON-RPC.

**Completion Notes:** Completed on `multi-account-pool-v1`. This plan delivered the local pool crate, pooled state/config/auth seams, lease-aware future-turn routing, and the first app-server/CLI/TUI pooled account surface. Later gap-closure and `accounts add` follow-up work is tracked in the subsequent dated plans.

---

## Planned File Layout

- Create `codex-rs/account-pool/Cargo.toml`, `codex-rs/account-pool/BUILD.bazel`, and focused source files under `codex-rs/account-pool/src/` for policy, backend traits, and lease types.
- Extend `codex-rs/state/migrations/` and add `codex-rs/state/src/model/account_pool.rs` plus `codex-rs/state/src/runtime/account_pool.rs`; keep SQLite ownership in `codex-state`.
- Add pooled config TOML types in `codex-rs/config/src/types.rs` and `codex-rs/config/src/config_toml.rs`; regenerate [config.schema.json](/Users/viv/projs/mcodex/codex-rs/core/config.schema.json).
- Add auth seam code in `codex-rs/login/src/auth/` and a remote transport identity seam in `codex-rs/core/src/client.rs` and related transport files.
- Keep `codex-rs/app-server/src/codex_message_processor.rs` small by extracting pooled account lease behavior into a new module instead of growing the existing file further.
- Add a new CLI module `codex-rs/cli/src/accounts.rs` rather than bloating [main.rs](/Users/viv/projs/mcodex/codex-rs/cli/src/main.rs) and [login.rs](/Users/viv/projs/mcodex/codex-rs/cli/src/login.rs).
- Keep TUI changes in `codex-rs/tui/src/status/` and `codex-rs/tui/src/chatwidget/status_surfaces.rs`; avoid adding new logic to [app.rs](/Users/viv/projs/mcodex/codex-rs/tui/src/app.rs) unless unavoidable.

### Task 1: Scaffold `codex-account-pool`

**Files:**
- Create: `codex-rs/account-pool/Cargo.toml`
- Create: `codex-rs/account-pool/BUILD.bazel`
- Create: `codex-rs/account-pool/src/lib.rs`
- Create: `codex-rs/account-pool/src/types.rs`
- Create: `codex-rs/account-pool/src/backend.rs`
- Create: `codex-rs/account-pool/src/policy.rs`
- Create: `codex-rs/account-pool/tests/policy.rs`
- Modify: `codex-rs/Cargo.toml`

- [x] **Step 1: Write the failing crate test**

```rust
#[test]
fn default_selection_prefers_healthy_account_and_rejects_mixed_kind_auto_rotation() {
    let pool = TestPool::homogeneous_chatgpt(/*healthy_accounts*/ 2);
    let selection = select_startup_account(&pool, SelectionRequest::default()).unwrap();
    assert_eq!(selection.account_id, "acct-1");

    let mixed = TestPool::mixed_kind_manual_only();
    assert!(select_startup_account(&mixed, SelectionRequest::default()).is_err());
}
```

- [x] **Step 2: Run the new crate test to verify the workspace is missing the crate**

Run: `cargo test -p codex-account-pool policy`  
Expected: FAIL with “package ID specification `codex-account-pool` did not match any packages”.

- [x] **Step 3: Add the workspace entry and crate skeleton**

```toml
# codex-rs/account-pool/Cargo.toml
[package]
name = "codex-account-pool"

[dependencies]
anyhow = { workspace = true }
codex-login = { workspace = true }
codex-state = { workspace = true }
```

```rust
// codex-rs/account-pool/src/lib.rs
pub mod backend;
pub mod policy;
pub mod types;

pub use policy::select_startup_account;
pub use types::{SelectionRequest, SelectionResult};
```

- [x] **Step 4: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 5: Run the new crate test to verify the scaffold passes**

Run: `cargo test -p codex-account-pool policy`  
Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add codex-rs/Cargo.toml codex-rs/account-pool
git commit -m "feat: scaffold account pool crate"
```

### Task 2: Add pooled state schema and typed persistence APIs in `codex-state`

**Files:**
- Create: `codex-rs/state/migrations/0025_account_pool.sql`
- Create: `codex-rs/state/src/model/account_pool.rs`
- Create: `codex-rs/state/src/runtime/account_pool.rs`
- Modify: `codex-rs/state/src/model/mod.rs`
- Modify: `codex-rs/state/src/runtime.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Test: `codex-rs/state/src/runtime/account_pool.rs`

- [x] **Step 1: Write failing storage tests next to the new runtime module**

```rust
#[tokio::test]
async fn acquire_exclusive_lease_rejects_second_holder() {
    let runtime = test_runtime().await;
    seed_account(runtime.as_ref(), "acct-1").await;

    let first = runtime.acquire_account_lease("pool-main", "inst-a").await.unwrap();
    let second = runtime.acquire_account_lease("pool-main", "inst-b").await;

    assert_eq!(second.unwrap_err(), AccountLeaseError::NoEligibleAccount);
    assert_eq!(first.account_id, "acct-1");
}

#[tokio::test]
async fn migrated_install_creates_legacy_default_selection_state() {
    let runtime = test_runtime_with_legacy_auth("acct-legacy").await;
    let selection = runtime.read_account_startup_selection().await.unwrap();

    assert_eq!(selection.default_pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(selection.preferred_account_id.as_deref(), Some("acct-legacy"));
    assert_eq!(selection.suppressed, false);
}
```

- [x] **Step 2: Run the state test to verify the APIs do not exist yet**

Run: `cargo test -p codex-state account_pool`  
Expected: FAIL with missing migration table and/or missing `acquire_account_lease`.

- [x] **Step 3: Add the migration and typed runtime APIs**

```sql
-- 0025_account_pool.sql
CREATE TABLE account_registry (...);
CREATE TABLE account_runtime_state (...);
CREATE TABLE account_startup_selection (... suppressed INTEGER NOT NULL ...);
CREATE TABLE account_leases (... lease_epoch INTEGER NOT NULL ...);
CREATE UNIQUE INDEX account_leases_active_account_idx ON account_leases(account_id) WHERE released_at IS NULL;
```

```rust
// codex-rs/state/src/runtime/account_pool.rs
impl StateRuntime {
    pub async fn acquire_account_lease(&self, pool_id: &str, holder_instance_id: &str) -> anyhow::Result<AccountLeaseRecord>;
    pub async fn renew_account_lease(&self, lease: &LeaseKey, now: DateTime<Utc>) -> anyhow::Result<LeaseRenewal>;
    pub async fn record_account_health_event(&self, event: AccountHealthEvent) -> anyhow::Result<()>;
    pub async fn import_legacy_default_account(&self, legacy_account: LegacyAccountImport) -> anyhow::Result<()>;
    pub async fn read_account_startup_selection(&self) -> anyhow::Result<AccountStartupSelectionState>;
    pub async fn write_account_startup_selection(&self, update: AccountStartupSelectionUpdate) -> anyhow::Result<()>;
}
```

- [x] **Step 4: Export the new model/runtime types without leaking raw SQL to consumers**

```rust
// codex-rs/state/src/lib.rs
pub use model::AccountLeaseRecord;
pub use model::AccountPoolHealthState;
```

- [x] **Step 5: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 6: Run the state test suite for the new module**

Run: `cargo test -p codex-state account_pool`  
Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add codex-rs/state
git commit -m "feat: add account pool state runtime"
```

### Task 3: Add pooled config types and config schema support

**Files:**
- Modify: `codex-rs/config/src/types.rs`
- Modify: `codex-rs/config/src/config_toml.rs`
- Modify: `codex-rs/config/src/types_tests.rs`
- Modify: `codex-rs/core/src/config/config_tests.rs`
- Modify: `codex-rs/core/config.schema.json`

- [x] **Step 1: Write failing config parsing tests**

```rust
#[test]
fn parses_accounts_pool_config() {
    let cfg: ConfigToml = toml::from_str(r#"
[accounts]
backend = "local"
default_pool = "team-main"
proactive_switch_threshold_percent = 85
allocation_mode = "exclusive"

[accounts.pools.team-main]
allow_context_reuse = false
account_kinds = ["chatgpt"]
"#).unwrap();

    assert_eq!(cfg.accounts.unwrap().default_pool.as_deref(), Some("team-main"));
}
```

- [x] **Step 2: Run the config tests to verify the field is rejected**

Run: `cargo test -p codex-config parses_accounts_pool_config -- --exact`  
Expected: FAIL with unknown field / missing type errors.

- [x] **Step 3: Add the TOML types and resolved config plumbing**

```rust
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema, Default)]
#[schemars(deny_unknown_fields)]
pub struct AccountsConfigToml {
    pub backend: Option<AccountPoolBackendToml>,
    pub default_pool: Option<String>,
    pub proactive_switch_threshold_percent: Option<u8>,
    pub lease_ttl_secs: Option<u64>,
    pub heartbeat_interval_secs: Option<u64>,
    pub min_switch_interval_secs: Option<u64>,
    pub allocation_mode: Option<AccountAllocationModeToml>,
    pub pools: Option<HashMap<String, AccountPoolDefinitionToml>>,
}
```

- [x] **Step 4: Add config validation for lease timing invariants**

```rust
if let Some(accounts) = &config.accounts
    && let (Some(lease_ttl_secs), Some(heartbeat_interval_secs)) =
        (accounts.lease_ttl_secs, accounts.heartbeat_interval_secs)
{
    let safety_margin_secs = heartbeat_interval_secs.saturating_mul(2);

    if lease_ttl_secs <= heartbeat_interval_secs {
        anyhow::bail!(
            "accounts.lease_ttl_secs must be greater than accounts.heartbeat_interval_secs"
        );
    }

    if safety_margin_secs >= lease_ttl_secs {
        anyhow::bail!(
            "derived account lease safety margin must be less than accounts.lease_ttl_secs"
        );
    }
}

if let Some(accounts) = &config.accounts
    && let (Some(lease_ttl_secs), Some(min_switch_interval_secs)) =
        (accounts.lease_ttl_secs, accounts.min_switch_interval_secs)
    && min_switch_interval_secs >= lease_ttl_secs
{
    anyhow::bail!(
        "accounts.min_switch_interval_secs must be less than accounts.lease_ttl_secs"
    );
}
```

- [x] **Step 5: Regenerate the config schema**

Run: `just write-config-schema`

- [x] **Step 6: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 7: Run targeted config tests**

Run: `cargo test -p codex-config parses_accounts_pool_config -- --exact`  
Run: `cargo test -p codex-core config_tests -- --skip windows`  
Expected: PASS, including the new accounts config coverage.

- [x] **Step 8: Commit**

```bash
git add codex-rs/config/src/types.rs codex-rs/config/src/config_toml.rs codex-rs/config/src/types_tests.rs codex-rs/core/src/config/config_tests.rs codex-rs/core/config.schema.json
git commit -m "feat: add account pool config types"
```

### Task 4: Add `LegacyAuthView`, `LeasedTurnAuth`, and `RemoteSessionId` seams

**Files:**
- Create: `codex-rs/login/src/auth/legacy_auth_view.rs`
- Create: `codex-rs/login/src/auth/leased_auth.rs`
- Modify: `codex-rs/login/src/auth/mod.rs`
- Modify: `codex-rs/login/src/lib.rs`
- Modify: `codex-rs/login/tests/suite/mod.rs`
- Modify: `codex-rs/core/src/client.rs`
- Modify: `codex-rs/codex-api/src/requests/headers.rs`
- Modify: `codex-rs/core/src/realtime_conversation.rs`
- Test: `codex-rs/core/tests/suite/window_headers.rs`
- Test: `codex-rs/core/tests/suite/client_websockets.rs`

- [x] **Step 1: Write failing auth and transport seam tests**

```rust
#[tokio::test]
async fn legacy_auth_view_reads_auth_manager_snapshot() {
    let manager = seeded_auth_manager().await;
    let legacy = LegacyAuthView::new(&manager);
    assert_eq!(legacy.current().await.unwrap().unwrap().account_id(), Some("acct-legacy"));
}

#[test]
fn leased_turn_auth_does_not_read_shared_auth_manager() {
    let leased = LeasedTurnAuth::chatgpt("acct-1", "token-1");
    assert_eq!(leased.account_id(), Some("acct-1"));
}

#[tokio::test]
async fn remote_session_reset_changes_session_id_without_changing_thread_id() {
    let mut session = model_client.new_session();
    let before = session.remote_session_id().to_string();
    session.reset_remote_session_identity();
    assert_ne!(before, session.remote_session_id().to_string());
}
```

- [x] **Step 2: Run targeted tests to verify the seams are absent**

Run: `cargo test -p codex-login legacy_auth_view_reads_auth_manager_snapshot -- --exact`  
Run: `cargo test -p codex-login leased_turn_auth_does_not_read_shared_auth_manager -- --exact`  
Run: `cargo test -p codex-core remote_session_reset_changes_session_id_without_changing_thread_id -- --exact`  
Expected: FAIL.

- [x] **Step 3: Add the seam types and `ModelClient` transport identity plumbing**

```rust
pub struct LegacyAuthView<'a> {
    manager: &'a AuthManager,
}

pub struct LeasedTurnAuth {
    auth: CodexAuth,
    lease_epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RemoteSessionId(String);
```

```rust
pub(crate) fn reset_remote_session_identity(&mut self) {
    self.client.advance_window_generation();
    self.client.state.remote_session_id = RemoteSessionId::new();
    self.reset_websocket_session();
    self.clear_previous_response_id();
}
```

- [x] **Step 4: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 5: Run the targeted login/core tests**

Run: `cargo test -p codex-login legacy_auth_view_reads_auth_manager_snapshot -- --exact`  
Run: `cargo test -p codex-login leased_turn_auth_does_not_read_shared_auth_manager -- --exact`  
Run: `cargo test -p codex-core window_headers`  
Run: `cargo test -p codex-core client_websockets`  
Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add codex-rs/login codex-rs/core/src/client.rs codex-rs/core/src/realtime_conversation.rs codex-rs/codex-api/src/requests/headers.rs codex-rs/core/tests/suite/window_headers.rs codex-rs/core/tests/suite/client_websockets.rs
git commit -m "feat: add leased auth and remote session seams"
```

### Task 5: Implement the local pool backend, lease lifecycle, and selection policy

**Files:**
- Modify: `codex-rs/account-pool/src/lib.rs`
- Modify: `codex-rs/account-pool/src/types.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`
- Create: `codex-rs/account-pool/src/backend/local.rs`
- Create: `codex-rs/account-pool/src/bootstrap.rs`
- Create: `codex-rs/account-pool/src/lease_lifecycle.rs`
- Create: `codex-rs/account-pool/src/manager.rs`
- Create: `codex-rs/account-pool/src/selection.rs`
- Modify: `codex-rs/account-pool/tests/policy.rs`
- Create: `codex-rs/account-pool/tests/lease_lifecycle.rs`

- [x] **Step 1: Write failing policy/lease tests**

```rust
#[tokio::test]
async fn ensure_active_lease_reuses_sticky_account_until_threshold() {
    let mut pool = fixture_manager().await;
    let first = pool.ensure_active_lease(Default::default()).await.unwrap();
    pool.report_rate_limits(first.key(), snapshot(/*used_percent*/ 70.0)).await.unwrap();
    let second = pool.ensure_active_lease(Default::default()).await.unwrap();
    assert_eq!(first.account_id, second.account_id);
}

#[tokio::test]
async fn stale_holder_health_event_is_ignored_after_epoch_bump() {
    let mut pool = fixture_manager().await;
    let lease = pool.ensure_active_lease(Default::default()).await.unwrap();
    pool.force_epoch_bump_for_test(lease.account_id()).await.unwrap();

    let result = pool
        .report_usage_limit_reached(lease.key(), usage_limit_event())
        .await;

    assert_eq!(result.unwrap(), HealthEventDisposition::IgnoredAsStale);
}

#[tokio::test]
async fn bootstrap_imports_legacy_default_only_once() {
    let mut pool = fixture_manager_with_legacy_auth("acct-legacy").await;
    pool.bootstrap_from_legacy_auth().await.unwrap();
    pool.bootstrap_from_legacy_auth().await.unwrap();

    let selection = pool.read_startup_selection_for_test().await.unwrap();
    assert_eq!(selection.preferred_account_id.as_deref(), Some("acct-legacy"));
}

#[test]
fn context_reuse_requires_consent_and_portable_transport() {
    let decision = evaluate_context_reuse(ContextReuseRequest {
        allow_context_reuse: true,
        explicit_context_reuse_consent: false,
        same_workspace: true,
        same_backend_family: true,
        transport_portable: true,
    });

    assert_eq!(decision, ContextReuseDecision::ResetRemoteContext);
}
```

- [x] **Step 2: Run the new crate tests to confirm the manager is missing**

Run: `cargo test -p codex-account-pool ensure_active_lease_reuses_sticky_account_until_threshold -- --exact`  
Expected: FAIL.

- [x] **Step 3: Implement the minimal local backend, bootstrap, and manager**

```rust
pub struct AccountPoolManager<B: AccountPoolBackend, L: LegacyAuthBootstrap> {
    backend: B,
    legacy_bootstrap: L,
    config: Arc<AccountPoolConfig>,
    active_lease: Option<LeasedAccount>,
}

impl<B: AccountPoolBackend, L: LegacyAuthBootstrap> AccountPoolManager<B, L> {
    pub async fn bootstrap_from_legacy_auth(&mut self) -> anyhow::Result<()>;
    pub async fn ensure_active_lease(&mut self, request: SelectionRequest) -> anyhow::Result<LeasedAccount>;
    pub async fn renew_active_lease_if_needed(&mut self, now: DateTime<Utc>) -> anyhow::Result<LeasedAccount>;
    pub async fn heartbeat_active_lease(&mut self, now: DateTime<Utc>) -> anyhow::Result<()>;
    pub async fn release_active_lease(&mut self) -> anyhow::Result<()>;
    pub async fn report_usage_limit_reached(&mut self, lease: LeaseKey, event: UsageLimitEvent) -> anyhow::Result<HealthEventDisposition>;
    pub async fn report_unauthorized(&mut self, lease: LeaseKey) -> anyhow::Result<HealthEventDisposition>;
}
```

```rust
pub async fn bootstrap_from_legacy_auth(&mut self) -> anyhow::Result<()> {
    if self.backend.has_startup_selection_state().await? {
        return Ok(());
    }

    if let Some(auth) = self.legacy_bootstrap.current_legacy_auth().await? {
        self.backend.import_legacy_default_account(auth).await?;
    }

    Ok(())
}
```

- [x] **Step 4: Add monotonic event ordering and pre-turn safety-margin checks**

```rust
if active_lease.remaining_ttl(now) <= self.config.derived_pre_turn_safety_margin() {
    self.renew_active_lease_if_needed(now).await?;
}

if !self
    .backend
    .accept_health_event(lease_key, event.sequence_number(), event.severity())
    .await?
{
    return Ok(HealthEventDisposition::IgnoredAsStale);
}
```

```rust
match evaluate_context_reuse(ContextReuseRequest {
    allow_context_reuse: pool.allow_context_reuse(),
    explicit_context_reuse_consent: pool.explicit_context_reuse_consent(),
    same_workspace: active.workspace_id() == next.workspace_id(),
    same_backend_family: active.backend_family() == next.backend_family(),
    transport_portable: transport.supports_cross_account_context_portability(),
}) {
    ContextReuseDecision::ReuseRemoteContext => {}
    ContextReuseDecision::ResetRemoteContext => session.reset_remote_session_identity(),
}
```

- [x] **Step 5: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 6: Run the full new-crate test suite**

Run: `cargo test -p codex-account-pool`  
Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add codex-rs/account-pool
git commit -m "feat: implement local account pool manager"
```

### Task 6: Wire `codex-core` to leased auth and future-turn failover

**Files:**
- Create: `codex-rs/core/tests/suite/account_pool.rs`
- Modify: `codex-rs/core/tests/suite/mod.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/client.rs`
- Modify: `codex-rs/core/src/lib.rs`

- [x] **Step 1: Write failing core integration tests for future-turn rotation**

```rust
#[tokio::test]
async fn usage_limit_reached_rotates_only_future_turns_on_responses_transport() {
    let harness = pooled_harness().await;
    harness.inject_usage_limit_reached().await;
    assert_eq!(harness.last_turn_replayed(), false);
    assert_eq!(harness.next_turn_account_id().await, "acct-2");
}

#[tokio::test]
async fn nearing_limit_snapshot_rotates_the_next_turn_before_exhaustion() {
    let harness = pooled_harness().await;
    harness.inject_rate_limits_snapshot(/*used_percent*/ 91.0).await;
    assert_eq!(harness.next_turn_account_id().await, "acct-2");
}

#[tokio::test]
async fn rotation_without_context_reuse_mints_new_remote_session_id() {
    let harness = pooled_harness_without_context_reuse().await;
    let before = harness.remote_session_id().await;
    harness.inject_usage_limit_reached().await;
    let after = harness.next_turn_remote_session_id().await;
    assert_ne!(before, after);
}

#[tokio::test]
async fn unauthorized_failure_marks_account_unavailable_for_next_turn() {
    let harness = pooled_harness().await;
    harness.inject_unauthorized_after_failed_refresh().await;
    assert_eq!(harness.next_turn_account_id().await, "acct-2");
}

#[tokio::test]
async fn fence_loss_blocks_followup_remote_work() {
    let harness = pooled_harness().await;
    harness.force_lease_epoch_bump().await;
    assert_eq!(harness.continue_turn_after_epoch_loss().await, TurnDisposition::AbortedAsStale);
}
```

- [x] **Step 2: Run the targeted core test**

Run: `cargo test -p codex-core usage_limit_reached_rotates_only_future_turns_on_responses_transport -- --exact`  
Expected: FAIL.

- [x] **Step 3: Add the lease-aware turn startup and future-turn failover wiring**

```rust
let lease = self.account_pool.ensure_active_lease(session_context).await?;
let auth = self.account_pool.materialize_turn_auth(&lease).await?;
let mut session = self.client.new_session_with_leased_auth(auth, lease.remote_session_id().clone());
```

```rust
self.account_pool
    .revalidate_active_lease(lease.key(), session.transport_generation())
    .await?;

if let Some(snapshot) = response.rate_limits() {
    self.account_pool.report_rate_limits(lease.key(), snapshot.into()).await?;
}

match err {
    CodexErr::UsageLimitReached(limit) => {
        self.account_pool.report_usage_limit_reached(lease.key(), limit.into()).await?;
        return Err(TurnFailure::RotateFutureTurnsOnly);
    }
    CodexErr::Unauthorized(_) => {
        self.account_pool.report_unauthorized(lease.key()).await?;
        return Err(TurnFailure::RotateFutureTurnsOnly);
    }
    _ => {}
}
```

```rust
if rotation.required()
    && matches!(context_reuse_decision, ContextReuseDecision::ResetRemoteContext)
{
    session.reset_remote_session_identity();
}
```

```rust
self.account_pool
    .revalidate_active_lease(lease.key(), session.transport_generation())
    .await?;
let refreshed_auth = self.account_pool.refresh_turn_auth(&lease).await?;
```

- [x] **Step 4: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 5: Run targeted core tests**

Run: `cargo test -p codex-core account_pool`  
Run: `cargo test -p codex-core quota_exceeded`  
Run: `cargo test -p codex-core nearing_limit_snapshot_rotates_the_next_turn_before_exhaustion -- --exact`  
Run: `cargo test -p codex-core rotation_without_context_reuse_mints_new_remote_session_id -- --exact`  
Run: `cargo test -p codex-core unauthorized_failure_marks_account_unavailable_for_next_turn -- --exact`  
Run: `cargo test -p codex-core fence_loss_blocks_followup_remote_work -- --exact`  
Run: `cargo test -p codex-core stream_error_allows_next_turn -- --exact`  
Run: `cargo test -p codex-core window_headers`  
Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add codex-rs/core/src/codex.rs codex-rs/core/src/client.rs codex-rs/core/src/lib.rs codex-rs/core/tests/suite/account_pool.rs codex-rs/core/tests/suite/mod.rs
git commit -m "feat: wire core to leased account pool"
```

### Task 7: Add `codex accounts` CLI and keep legacy login compatibility

**Files:**
- Create: `codex-rs/cli/src/accounts.rs`
- Modify: `codex-rs/cli/src/lib.rs`
- Modify: `codex-rs/cli/src/main.rs`
- Modify: `codex-rs/cli/src/login.rs`
- Create: `codex-rs/cli/tests/accounts.rs`

- [x] **Step 1: Write failing CLI parsing and behavior tests**

```rust
#[test]
fn login_status_reads_legacy_auth_view_only() {
    let output = run_codex(["login", "status"]);
    assert!(output.stderr.contains("Logged in using ChatGPT"));
}

#[test]
fn accounts_current_reports_predicted_pool_selection() {
    let output = run_codex(["accounts", "current"]);
    assert!(output.stdout.contains("effective pool"));
}

#[test]
fn accounts_status_reports_suppression_and_eligibility() {
    let output = run_codex(["accounts", "status"]);
    assert!(output.stdout.contains("eligibility"));
}

#[test]
fn accounts_resume_clears_durable_suppression() {
    let output = run_codex(["accounts", "resume"]);
    assert!(output.stdout.contains("automatic selection resumed"));
}

#[test]
fn accounts_switch_sets_preferred_account_override() {
    let output = run_codex(["accounts", "switch", "acct-2"]);
    assert!(output.stdout.contains("preferred account"));
}

#[test]
fn logout_enables_durable_startup_suppression_for_future_runtimes() {
    let output = run_codex(["logout"]);
    assert!(output.stderr.contains("automatic pooled selection suppressed"));
}
```

- [x] **Step 2: Run the new CLI test file**

Run: `cargo test -p codex-cli --test accounts`  
Expected: FAIL with unknown subcommand and/or assertion failures.

- [x] **Step 3: Add the new command module and wire the split**

```rust
#[derive(Debug, clap::Subcommand)]
pub enum AccountsSubcommand {
    Add(AddAccountCommand),
    List,
    Current,
    Status,
    Resume,
    Switch(SwitchAccountCommand),
}
```

- [x] **Step 4: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 5: Run targeted CLI tests**

Run: `cargo test -p codex-cli --test accounts`  
Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add codex-rs/cli/src/accounts.rs codex-rs/cli/src/lib.rs codex-rs/cli/src/main.rs codex-rs/cli/src/login.rs codex-rs/cli/tests/accounts.rs
git commit -m "feat: add accounts cli namespace"
```

### Task 8: Add pooled app-server protocol and implementation

**Files:**
- Create: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server/README.md`
- Modify: `codex-rs/app-server/tests/suite/v2/account.rs`
- Create: `codex-rs/app-server/tests/suite/v2/account_lease.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/mod.rs`

- [x] **Step 1: Write failing protocol and app-server tests**

```rust
#[tokio::test]
async fn account_lease_read_reports_process_local_pool_state() -> Result<()> {
    let mut mcp = pooled_stdio_server().await?;
    let response = mcp.read_account_lease().await?;
    assert_eq!(response.suppressed, false);
    assert_eq!(response.health_state.as_deref(), Some("healthy"));
    Ok(())
}

#[tokio::test]
async fn account_lease_updated_emits_on_resume() -> Result<()> {
    let mut mcp = pooled_stdio_server().await?;
    mcp.account_lease_resume().await?;
    assert_eq!(mcp.next_notification_method().await?, "accountLease/updated");
    Ok(())
}

#[tokio::test]
async fn pooled_mode_rejects_multi_thread_stdio_runtime() -> Result<()> {
    let mut mcp = stdio_server_with_two_loaded_threads().await?;
    let err = mcp.read_account_lease().await.unwrap_err();
    assert!(err.to_string().contains("pooled mode requires one loaded thread"));
    Ok(())
}

#[tokio::test]
async fn pooled_mode_rejects_websocket_runtime() -> Result<()> {
    let mut mcp = pooled_websocket_server().await?;
    let err = mcp.read_account_lease().await.unwrap_err();
    assert!(err.to_string().contains("pooled lease mode is only supported for stdio"));
    Ok(())
}

#[tokio::test]
async fn account_logout_with_runtime_local_chatgpt_tokens_is_not_durable() -> Result<()> {
    let mut mcp = server_with_runtime_chatgpt_auth_tokens().await?;
    mcp.account_logout().await?;
    let lease = mcp.read_account_lease().await?;
    assert_eq!(lease.suppressed, false);
    Ok(())
}
```

- [x] **Step 2: Run protocol and app-server tests**

Run: `cargo test -p codex-app-server-protocol account_lease`  
Run: `cargo test -p codex-app-server account_lease`  
Expected: FAIL.

- [x] **Step 3: Add the v2 payloads and extract pooled handling into a new module**

```rust
pub struct AccountLeaseReadResponse {
    pub active: bool,
    pub suppressed: bool,
    pub account_id: Option<String>,
    pub pool_id: Option<String>,
    pub lease_id: Option<String>,
    pub lease_epoch: Option<u64>,
    pub health_state: Option<String>,
    pub switch_reason: Option<String>,
    pub suppression_reason: Option<String>,
    pub transport_reset_generation: Option<u64>,
    pub last_remote_context_reset_turn_id: Option<String>,
    pub next_eligible_at: Option<i64>,
}

pub struct AccountLeaseUpdatedNotification {
    pub account_id: Option<String>,
    pub pool_id: Option<String>,
    pub suppressed: bool,
}
```

```rust
// account_lease_api.rs
pub(crate) async fn handle_account_lease_read(...) { ... }
pub(crate) async fn handle_account_lease_resume(...) { ... }
pub(crate) fn publish_account_lease_updated(...) { ... }

// codex_message_processor.rs
pub(crate) async fn handle_account_logout(...) -> Result<()> {
    if runtime.uses_runtime_chatgpt_auth_tokens() {
        runtime.clear_runtime_external_auth().await?;
        runtime.release_runtime_lease().await?;
        return Ok(());
    }

    runtime.clear_legacy_auth_view().await?;
    runtime.suppress_default_startup_selection().await?;
    runtime.release_runtime_lease().await?;
    Ok(())
}
```

- [x] **Step 4: Register the new v2 methods and enforce v1 pooled-mode limits**

```rust
// protocol/common.rs
inspect_params("accountLease/read", ...);
inspect_params("accountLease/resume", ...);

// message_processor.rs
if transport.is_websocket() || runtime.loaded_thread_count() > 1 {
    return Err(ServerRequestError::invalid_request(
        "pooled lease mode is only supported for stdio with one loaded thread",
    ));
}
```

- [x] **Step 5: Regenerate the app-server schema**

Run: `just write-app-server-schema`

- [x] **Step 6: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 7: Run targeted protocol and app-server tests**

Run: `cargo test -p codex-app-server-protocol account_lease`  
Run: `cargo test -p codex-app-server account_lease`  
Run: `cargo test -p codex-app-server account_logout_with_runtime_local_chatgpt_tokens_is_not_durable -- --exact`  
Run: `cargo test -p codex-app-server rate_limits`  
Expected: PASS.

- [x] **Step 8: Commit**

```bash
git add codex-rs/app-server-protocol/src/protocol/common.rs codex-rs/app-server-protocol/src/protocol/v2.rs codex-rs/app-server/src/account_lease_api.rs codex-rs/app-server/src/message_processor.rs codex-rs/app-server/src/codex_message_processor.rs codex-rs/app-server/README.md codex-rs/app-server/tests/suite/v2/account.rs codex-rs/app-server/tests/suite/v2/account_lease.rs codex-rs/app-server/tests/suite/v2/mod.rs
git commit -m "feat: add pooled account lease app-server api"
```

### Task 9: Add minimal TUI pooled status surfaces and finish verification

**Files:**
- Modify: `codex-rs/tui/src/status/account.rs`
- Modify: `codex-rs/tui/src/status/card.rs`
- Modify: `codex-rs/tui/src/status/tests.rs`
- Modify: `codex-rs/tui/src/chatwidget/status_surfaces.rs`
- Modify: `codex-rs/tui/src/chatwidget/tests/status_command_tests.rs`
- Modify: `codex-rs/tui/src/chatwidget/tests/mod.rs`
- Update snapshots under: `codex-rs/tui/src/status/snapshots/`

- [x] **Step 1: Write failing status-surface tests**

```rust
#[test]
fn status_snapshot_shows_active_pool_and_next_eligible_time() {
    let output = render_status_card(pooled_status_fixture());
    insta::assert_snapshot!(output);
}

#[test]
fn status_snapshot_shows_auto_switch_and_remote_reset_messages() {
    let output = render_status_card(pooled_switch_fixture());
    insta::assert_snapshot!(output);
}
```

- [x] **Step 2: Run the targeted TUI tests to generate failing snapshots**

Run: `cargo test -p codex-tui status_snapshot_shows_active_pool_and_next_eligible_time -- --exact`  
Expected: FAIL with a new or mismatched snapshot.

- [x] **Step 3: Add the minimal status rendering**

```rust
pub(crate) enum StatusAccountDisplay {
    ChatGpt { email: String, plan_type: PlanType, pool_id: Option<String> },
    ApiKey { label: Option<String>, pool_id: Option<String> },
}
```

- [x] **Step 4: Review and accept the intended snapshots**

Run: `cargo insta pending-snapshots -p codex-tui`  
Run: `cargo insta accept -p codex-tui`

- [x] **Step 5: Run `just fmt` in `codex-rs`**

Run: `just fmt`

- [x] **Step 6: Run targeted crate tests**

Run: `cargo test -p codex-tui`  
Expected: PASS.

- [x] **Step 7: Ask before the full workspace test suite, then run it if approved**

Run: `cargo test`  
Expected: PASS.  
Note: this touches `codex-core`, `codex-state`, and `codex-app-server-protocol`, so ask the user before running the complete suite.

- [x] **Step 8: Run scoped fixers without re-running tests afterward**

Run: `just fix -p codex-account-pool`  
Run: `just fix -p codex-state`  
Run: `just fix -p codex-config`  
Run: `just fix -p codex-login`  
Run: `just fix -p codex-core`  
Run: `just fix -p codex-cli`  
Run: `just fix -p codex-app-server-protocol`  
Run: `just fix -p codex-app-server`  
Run: `just fix -p codex-tui`

Expected: PASS / no remaining clippy fixes.

- [x] **Step 9: Commit**

```bash
git add codex-rs/tui/src/status/account.rs codex-rs/tui/src/status/card.rs codex-rs/tui/src/status/tests.rs codex-rs/tui/src/chatwidget/status_surfaces.rs codex-rs/tui/src/chatwidget/tests/status_command_tests.rs codex-rs/tui/src/chatwidget/tests/mod.rs codex-rs/tui/src/status/snapshots
git commit -m "feat: surface pooled account status in tui"
```
