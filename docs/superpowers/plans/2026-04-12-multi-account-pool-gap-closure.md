# Multi-Account Pool Spec Gap Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining multi-account pool v1 spec gaps around live app-server lease state, spec-complete CLI account management, targeted TUI lease status surfaces, and final verification.

**Architecture:** Treat the core session's pooled lease manager as the live runtime source of truth, and make app-server, CLI, and TUI consume projected snapshots instead of independently re-deriving live state from startup preview. Reuse the existing v2 `AccountLeaseReadResponse` fields where possible, keep `account/*` legacy semantics unchanged, and keep TUI changes limited to `/status` plus lightweight notifications rather than a full account-management UI.

**Tech Stack:** Rust workspace crates (`codex-state`, `codex-core`, `codex-cli`, `codex-app-server`, `codex-app-server-protocol`, `codex-tui`), SQLite via `sqlx`, app-server v2 JSON-RPC, clap, ratatui/insta snapshot tests.

---

## Scope

This plan closes the gaps found after Task 9:

- `accountLease/read` and `accountLease/updated` must reflect process-local live lease state, not only fresh-runtime startup preview.
- `codex accounts` must cover the v1 CLI scope from the spec, including structured status output, pool override selection, and account/pool mutator flows.
- TUI must cover the targeted pooled status/error/notification states from the spec.
- Final verification must include scoped crate tests, schema/docs updates, and workspace-level test approval.

Non-goals:

- Full TUI account-management UI.
- Remote backend implementation.
- Multi-client WebSocket pooled mode.
- App-server live manual lease switching.
- Broad protocol expansion beyond filling the already-added lease response fields, unless a failing test proves a field is missing.

## Planned File Layout

- Modify `codex-rs/state/src/model/account_pool.rs` for pool diagnostics and active-lease read models.
- Modify `codex-rs/state/src/runtime/account_pool.rs` for public read/list/update helpers over `account_registry`, `account_leases`, and `account_runtime_state`.
- Modify `codex-rs/state/src/lib.rs` to export the new public state models.
- Modify `codex-rs/core/src/state/service.rs` to expose live pooled lease snapshots from `AccountPoolManager`.
- Modify `codex-rs/core/src/codex.rs` to update snapshot metadata when automatic switching and remote-context reset occur.
- Modify `codex-rs/core/src/codex_thread.rs` to expose a narrow async `account_lease_snapshot` method to app-server.
- Modify `codex-rs/app-server/src/account_lease_api.rs` to map live snapshots into v2 API payloads, with startup-preview fallback only when there is no live loaded thread.
- Modify `codex-rs/app-server/src/codex_message_processor.rs` and `codex-rs/app-server/src/thread_state.rs` to read live snapshots and emit `accountLease/updated` only on meaningful snapshot changes.
- Prefer splitting `codex-rs/cli/src/accounts.rs` into `codex-rs/cli/src/accounts/mod.rs`, `diagnostics.rs`, `output.rs`, and `mutate.rs` before adding the full CLI surface.
- Modify `codex-rs/tui/src/app/app_server_adapter.rs`, `codex-rs/tui/src/chatwidget.rs`, and `codex-rs/tui/src/status/*` for targeted TUI status and notification behavior.
- Extend tests under `codex-rs/state`, `codex-rs/core/tests/suite/account_pool.rs`, `codex-rs/app-server/tests/suite/v2/account_lease.rs`, `codex-rs/cli/tests/accounts.rs`, and `codex-rs/tui/src/**/tests`.

### Task 1: Add Live Lease Snapshot Primitives in State and Core

**Files:**
- Modify: `codex-rs/state/src/model/account_pool.rs`
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/codex_thread.rs`
- Modify: `codex-rs/core/tests/suite/account_pool.rs`

- [ ] **Step 1: Write failing state diagnostics tests**

Add focused tests in `codex-rs/state/src/runtime/account_pool.rs`:

```rust
#[tokio::test]
async fn read_active_holder_lease_returns_current_unexpired_lease() {
    let runtime = StateRuntime::init(unique_temp_dir().await, "test-provider".to_string()).await.unwrap();
    import_account(&runtime, "acct-1", "team-main").await;
    let lease = runtime.acquire_account_lease("team-main", "holder-1", Duration::seconds(300)).await.unwrap();

    let read_back = runtime.read_active_holder_lease("holder-1").await.unwrap();

    assert_eq!(read_back, Some(lease));
}

#[tokio::test]
async fn read_pool_diagnostics_reports_per_account_eligibility_and_next_eligible_time() {
    // Seed one leased account and one rate-limited account.
    // Assert the diagnostic object contains stable per-account eligibility reasons
    // and a next_eligible_at timestamp when the only blocker is an active lease.
}
```

- [ ] **Step 2: Run state tests to verify failure**

Run: `cargo test -p codex-state account_pool -- --nocapture`  
Expected: FAIL because the new public read APIs do not exist.

- [ ] **Step 3: Add state read models and public APIs**

Add small model structs in `codex-rs/state/src/model/account_pool.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolDiagnostic {
    pub pool_id: String,
    pub accounts: Vec<AccountPoolAccountDiagnostic>,
    pub next_eligible_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPoolAccountDiagnostic {
    pub account_id: String,
    pub pool_id: String,
    pub healthy: bool,
    pub active_lease: Option<AccountLeaseRecord>,
    pub health_state: Option<AccountHealthState>,
    pub eligibility: AccountStartupEligibility,
    pub next_eligible_at: Option<DateTime<Utc>>,
}
```

Add `StateRuntime` methods:

```rust
pub async fn read_active_holder_lease(
    &self,
    holder_instance_id: &str,
) -> anyhow::Result<Option<AccountLeaseRecord>>;

pub async fn read_account_pool_diagnostic(
    &self,
    pool_id: &str,
    preferred_account_id: Option<&str>,
) -> anyhow::Result<AccountPoolDiagnostic>;
```

Keep these as reads only. Do not change the SQLite schema unless tests prove current schema cannot answer the queries.

- [ ] **Step 4: Write failing core runtime snapshot tests**

Extend `codex-rs/core/tests/suite/account_pool.rs`:

```rust
#[tokio::test]
async fn account_lease_snapshot_reports_active_lease_and_next_eligible_time() -> Result<()> {
    // Start a pooled session, run one turn, then assert the thread snapshot
    // contains account_id, pool_id, lease_id, lease_epoch, health_state,
    // and a stable active=true state.
    Ok(())
}

#[tokio::test]
async fn account_lease_snapshot_records_remote_reset_generation_when_account_changes() -> Result<()> {
    // Seed allow_context_reuse=false, trigger a future-turn account rotation,
    // run the next turn, and assert transport_reset_generation increments.
    Ok(())
}
```

- [ ] **Step 5: Run core tests to verify failure**

Run: `cargo test -p codex-core account_pool -- --nocapture`  
Expected: FAIL because core has no public lease snapshot.

- [ ] **Step 6: Add core snapshot type and manager method**

In `codex-rs/core/src/state/service.rs`, add a session-local snapshot type near `AccountPoolManager`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountLeaseRuntimeSnapshot {
    pub active: bool,
    pub suppressed: bool,
    pub account_id: Option<String>,
    pub pool_id: Option<String>,
    pub lease_id: Option<String>,
    pub lease_epoch: Option<i64>,
    pub health_state: Option<AccountHealthState>,
    pub switch_reason: Option<AccountLeaseRuntimeReason>,
    pub suppression_reason: Option<AccountLeaseRuntimeReason>,
    pub transport_reset_generation: Option<u64>,
    pub last_remote_context_reset_turn_id: Option<String>,
    pub next_eligible_at: Option<chrono::DateTime<Utc>>,
}
```

Use a Rust enum for `AccountLeaseRuntimeReason`, then map it to the existing API reason strings in app-server.

- [ ] **Step 7: Track remote reset and retry-suppressed state in core**

Update `prepare_turn` and the callsite in `codex-rs/core/src/codex.rs` so the manager records:

- account switches caused by automatic selection
- when current turn is non-replayable and only future turns rotate
- when `reset_remote_session_identity()` was called
- `transport_reset_generation += 1`
- `last_remote_context_reset_turn_id = Some(turn_id)`

Reuse existing state where possible. Do not add app-server protocol fields unless the existing `switch_reason` / `suppression_reason` fields cannot express the state.

- [ ] **Step 8: Expose the snapshot through `CodexThread`**

Add a narrow method in `codex-rs/core/src/codex_thread.rs`:

```rust
pub async fn account_lease_snapshot(&self) -> Option<AccountLeaseRuntimeSnapshot> {
    self.codex.account_lease_snapshot().await
}
```

Export the snapshot type from `codex-core` only as narrowly as app-server requires. Prefer `pub(crate)` inside core and a small public app-server-facing view if crate boundaries require it.

- [ ] **Step 9: Run targeted tests**

Run: `cargo test -p codex-state account_pool -- --nocapture`  
Run: `cargo test -p codex-core account_pool -- --nocapture`  
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add codex-rs/state/src/model/account_pool.rs codex-rs/state/src/runtime/account_pool.rs codex-rs/state/src/lib.rs codex-rs/core/src/state/service.rs codex-rs/core/src/codex.rs codex-rs/core/src/codex_thread.rs codex-rs/core/tests/suite/account_pool.rs
git commit -m "feat: expose live pooled account lease snapshots"
```

### Task 2: Wire App-Server Lease API to Live Snapshots and Auto Updates

**Files:**
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Modify: `codex-rs/app-server/src/thread_state.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_lease.rs`
- Modify if protocol shape changes: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify if protocol shape changes: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Modify if behavior docs change: `codex-rs/app-server/README.md`

- [ ] **Step 1: Write failing app-server live-state tests**

Add tests in `codex-rs/app-server/tests/suite/v2/account_lease.rs`:

```rust
#[tokio::test]
async fn account_lease_read_reports_live_active_lease_fields_after_turn_start() -> Result<()> {
    // Start a single stdio pooled thread, submit a turn, then call accountLease/read.
    // Assert active=true, account_id, pool_id, lease_id, and lease_epoch are Some.
    Ok(())
}

#[tokio::test]
async fn account_lease_updated_emits_when_automatic_switch_changes_live_snapshot() -> Result<()> {
    // Trigger a usage_limit_reached event that rotates only a future turn.
    // Assert accountLease/updated is emitted before/around the next visible lease state.
    Ok(())
}

#[tokio::test]
async fn account_lease_read_reports_remote_reset_and_retry_suppressed_reason() -> Result<()> {
    // Use allow_context_reuse=false and a non-replayable limit failure.
    // Assert transport_reset_generation, last_remote_context_reset_turn_id,
    // and switch_reason are populated from the live snapshot.
    Ok(())
}
```

- [ ] **Step 2: Run app-server test to verify failure**

Run: `cargo test -p codex-app-server account_lease -- --nocapture`  
Expected: FAIL because `accountLease/read` still uses startup preview and hardcodes live fields to `None`.

- [ ] **Step 3: Change app-server read path to prefer the live loaded thread**

In `codex_message_processor.rs`, after validating the v1 limits, find the single loaded thread if present. Pass it to `account_lease_api`:

```rust
let loaded_thread = self.single_loaded_thread_for_account_lease().await?;
let response = account_lease_api::read_account_lease(self.config.as_ref(), loaded_thread.as_ref()).await?;
```

Keep the startup-preview fallback in `account_lease_api` for process startup/no-thread cases.

- [ ] **Step 4: Map core snapshot to existing protocol fields**

In `account_lease_api.rs`, replace hardcoded `None` fields with live snapshot values:

- `lease_id`
- `lease_epoch`
- `transport_reset_generation`
- `last_remote_context_reset_turn_id`
- `next_eligible_at`
- `health_state`
- `switch_reason`
- `suppression_reason`

Use reason strings already understood by TUI when possible:

- `automaticAccountSelected`
- `preferredAccountSelected`
- `noEligibleAccount`
- `durablySuppressed`
- new string only if needed: `futureTurnOnlyAfterLimitFailure`

- [ ] **Step 5: Emit `accountLease/updated` from the thread listener**

Store the last emitted lease notification in `ThreadState`.

After each core event handled by `apply_bespoke_event_handling`, call a small helper:

```rust
maybe_emit_account_lease_updated(
    &conversation,
    &thread_state,
    &outgoing,
).await;
```

The helper reads the live snapshot, converts it to `AccountLeaseUpdatedNotification`, compares it to the previous notification, and emits only when different.

- [ ] **Step 6: Preserve stdio/websocket restrictions**

Keep existing rejection tests for:

- WebSocket pooled mode
- stdio runtime with more than one loaded thread
- `chatgptAuthTokens` runtime-local logout behavior

- [ ] **Step 7: Regenerate schema only if protocol fields changed**

If Task 2 adds or changes protocol fields:

Run: `just write-app-server-schema`

If it only fills existing fields and adds reason strings, no schema regeneration is needed.

- [ ] **Step 8: Run targeted tests**

Run: `cargo test -p codex-app-server account_lease -- --nocapture`  
Run if protocol changed: `cargo test -p codex-app-server-protocol account_lease -- --nocapture`  
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add codex-rs/app-server/src/account_lease_api.rs codex-rs/app-server/src/codex_message_processor.rs codex-rs/app-server/src/message_processor.rs codex-rs/app-server/src/thread_state.rs codex-rs/app-server/tests/suite/v2/account_lease.rs codex-rs/app-server/README.md codex-rs/app-server-protocol
git commit -m "feat: report live pooled lease updates from app server"
```

### Task 3: Complete CLI Status, Pool Override, and Structured Output

**Files:**
- Move: `codex-rs/cli/src/accounts.rs` to `codex-rs/cli/src/accounts/mod.rs`
- Create: `codex-rs/cli/src/accounts/diagnostics.rs`
- Create: `codex-rs/cli/src/accounts/output.rs`
- Modify: `codex-rs/cli/src/main.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`
- Modify as needed: `codex-rs/state/src/model/account_pool.rs`
- Modify as needed: `codex-rs/state/src/runtime/account_pool.rs`

- [ ] **Step 1: Write failing CLI diagnostics tests**

Add tests in `codex-rs/cli/tests/accounts.rs`:

```rust
#[tokio::test]
async fn accounts_status_json_reports_pool_diagnostics_and_per_account_reasons() -> Result<()> {
    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-main");
    assert!(json["accounts"].as_array().unwrap().iter().any(|row| row["accountId"] == "acct-1"));
    Ok(())
}

#[tokio::test]
async fn accounts_status_accepts_account_pool_override_without_persisting_it() -> Result<()> {
    let output = run_codex(&codex_home, &["accounts", "--account-pool", "team-other", "status"]).await?;
    assert!(output.stdout.contains("team-other"));
    assert_eq!(read_startup_selection(&codex_home).await?.default_pool_id.as_deref(), Some("team-main"));
    Ok(())
}
```

- [ ] **Step 2: Run CLI tests to verify failure**

Run: `cargo test -p codex-cli --test accounts`  
Expected: FAIL because `--json`, `--account-pool`, and per-account diagnostics do not exist.

- [ ] **Step 3: Split the CLI module**

Move `accounts.rs` into an `accounts/` directory:

- `mod.rs`: clap types, top-level dispatch, shared config loading
- `diagnostics.rs`: build status/current diagnostic objects
- `output.rs`: text and JSON formatting

Keep existing public imports stable for `main.rs`.

- [ ] **Step 4: Add `--account-pool` and `--json`**

Add `account_pool: Option<String>` to `AccountsCommand`, not to global config. It is a transient selector and must not mutate durable `default_pool_id`.

Add `--json` to `Current` and `Status` only unless tests prove other subcommands need it.

- [ ] **Step 5: Use state diagnostics for status output**

`accounts current` stays compact:

- effective pool
- durable suppression/override
- predicted account
- eligibility

`accounts status` becomes detailed:

- effective pool
- durable suppression/override
- predicted account
- health state
- switch reason
- next eligible time
- per-account eligibility reasons

- [ ] **Step 6: Run targeted tests**

Run: `cargo test -p codex-cli --test accounts`  
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add codex-rs/cli/src/accounts.rs codex-rs/cli/src/accounts codex-rs/cli/src/main.rs codex-rs/cli/tests/accounts.rs codex-rs/state/src/model/account_pool.rs codex-rs/state/src/runtime/account_pool.rs
git commit -m "feat: add structured pooled account status"
```

### Task 4: Add CLI Account and Pool Mutator Flows

**Files:**
- Create: `codex-rs/cli/src/accounts/mutate.rs`
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Modify: `codex-rs/cli/src/accounts/output.rs`
- Modify: `codex-rs/cli/src/login.rs`
- Modify: `codex-rs/state/src/model/account_pool.rs`
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`

- [ ] **Step 1: Write failing registry mutator tests**

Add tests:

```rust
#[tokio::test]
async fn accounts_disable_excludes_account_from_automatic_selection() -> Result<()> {
    let output = run_codex(&codex_home, &["accounts", "disable", "acct-1"]).await?;
    assert!(output.success);

    let status = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    let json: serde_json::Value = serde_json::from_str(&status.stdout)?;
    assert_eq!(json["accounts"][0]["enabled"], false);
    Ok(())
}

#[tokio::test]
async fn accounts_pool_assign_moves_account_only_with_explicit_pool() -> Result<()> {
    let output = run_codex(&codex_home, &["accounts", "pool", "assign", "acct-2", "team-other"]).await?;
    assert!(output.success);
    Ok(())
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p codex-cli --test accounts`  
Expected: FAIL because mutator subcommands do not exist.

- [ ] **Step 3: Add state registry mutation APIs**

Add public methods to `StateRuntime`:

```rust
pub async fn upsert_account_registry_entry(&self, entry: AccountRegistryEntryUpdate) -> anyhow::Result<()>;
pub async fn set_account_enabled(&self, account_id: &str, enabled: bool) -> anyhow::Result<()>;
pub async fn remove_account_registry_entry(&self, account_id: &str) -> anyhow::Result<()>;
pub async fn assign_account_pool(&self, account_id: &str, pool_id: &str) -> anyhow::Result<()>;
pub async fn list_account_pool_memberships(&self, pool_id: Option<&str>) -> anyhow::Result<Vec<AccountPoolMembership>>;
```

Use existing `healthy` if there is no separate enabled column. If a separate enabled concept is necessary, add a migration and run the appropriate state tests.

- [ ] **Step 4: Add CLI mutator subcommands**

Add clap variants:

```rust
Add(AddAccountCommand),
Enable(AccountIdCommand),
Disable(AccountIdCommand),
Remove(AccountIdCommand),
Pool(PoolCommand),
```

For `PoolCommand`:

```rust
enum PoolSubcommand {
    List,
    Assign { account_id: String, pool_id: String },
}
```

- [ ] **Step 5: Implement `add` conservatively**

Do not invent a fork-only credential container. For v1:

- `accounts add chatgpt` reuses existing ChatGPT login/import flow, then registers the resulting account id in `account_registry`.
- `accounts add chatgpt --device-auth` reuses the existing device auth path.
- `accounts add api-key` requires an explicit account id or label if the existing auth flow cannot derive one; document that full API-key quota awareness remains deferred.

If a clean credential import hook does not exist, stop after registry mutators and surface the credential-storage gap before changing `auth.json`.

- [ ] **Step 6: Run targeted tests**

Run: `cargo test -p codex-state account_pool -- --nocapture`  
Run: `cargo test -p codex-cli --test accounts`  
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add codex-rs/cli/src/accounts codex-rs/cli/src/login.rs codex-rs/cli/tests/accounts.rs codex-rs/state/src/model/account_pool.rs codex-rs/state/src/runtime/account_pool.rs codex-rs/state/src/lib.rs codex-rs/state/migrations
git commit -m "feat: add pooled account management commands"
```

### Task 5: Complete TUI Pooled Lease Status and Notifications

**Files:**
- Modify: `codex-rs/tui/src/app/app_server_adapter.rs`
- Modify: `codex-rs/tui/src/app_server_session.rs`
- Modify: `codex-rs/tui/src/chatwidget.rs`
- Modify: `codex-rs/tui/src/status/account.rs`
- Modify: `codex-rs/tui/src/status/card.rs`
- Modify: `codex-rs/tui/src/status/tests.rs`
- Modify: `codex-rs/tui/src/chatwidget/tests/status_command_tests.rs`
- Modify or add snapshots under: `codex-rs/tui/src/status/snapshots/`

- [ ] **Step 1: Write failing TUI snapshot tests**

Add tests:

```rust
#[tokio::test]
async fn status_snapshot_shows_no_available_account_error_state() {
    let display = StatusAccountLeaseDisplay {
        pool_id: Some("team-main".to_string()),
        account_id: None,
        status: "Waiting · Unavailable".to_string(),
        note: Some("No eligible account is available".to_string()),
        next_eligible_at: Some("03:24".to_string()),
        remote_reset: None,
    };
    // Assert snapshot.
}

#[tokio::test]
async fn status_snapshot_shows_retry_suppressed_after_non_replayable_limit_failure() {
    // Assert the note clearly says the current turn was not replayed
    // and future turns will use the next eligible account.
}
```

- [ ] **Step 2: Write failing chat notification tests**

Add `chatwidget` tests where `AccountLeaseUpdated` changes account id / reason:

```rust
#[test]
fn account_lease_updated_adds_automatic_switch_notice_when_account_changes() {
    // Seed previous lease display, call update with new account/reason,
    // assert a lightweight history cell is inserted.
}
```

- [ ] **Step 3: Run targeted tests to verify failure**

Run: `cargo test -p codex-tui status_snapshot_shows_no_available_account_error_state -- --exact`  
Run: `cargo test -p codex-tui account_lease_updated_adds_automatic_switch_notice_when_account_changes -- --exact`  
Expected: FAIL.

- [ ] **Step 4: Implement TUI display mapping**

Update `status_account_lease_display_from_response` so existing/new reason strings map to user-facing text:

- `noEligibleAccount`
- `durablySuppressed`
- `futureTurnOnlyAfterLimitFailure`
- `automaticAccountSelected`
- `preferredAccountSelected`

Keep `/status` as the source of truth. Do not add a full account UI.

- [ ] **Step 5: Add lightweight history notice on meaningful lease transitions**

Change `ChatWidget::update_account_lease_state` to diff old/new display:

- account id changed: show automatic switch notice with reason
- remote reset changed: show remote continuity reset notice, or rely on `/status` if duplicate
- no eligible account: show error-style notice
- retry suppressed: show current-turn-not-replayed notice

Use existing `PlainHistoryCell` / wrapped warning cells. Do not add cards inside cards.

- [ ] **Step 6: Review and accept snapshots**

Run: `cargo test -p codex-tui`  
Run: `cargo insta pending-snapshots -p codex-tui`  
Review the generated `*.snap.new` files.  
Run if intended: `cargo insta accept -p codex-tui`

- [ ] **Step 7: Run targeted TUI tests**

Run: `cargo test -p codex-tui`  
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add codex-rs/tui/src/app/app_server_adapter.rs codex-rs/tui/src/app_server_session.rs codex-rs/tui/src/chatwidget.rs codex-rs/tui/src/status codex-rs/tui/src/chatwidget/tests/status_command_tests.rs
git commit -m "feat: surface pooled lease transitions in tui"
```

### Task 6: Docs, Schema, Fixers, and Final Verification

**Files:**
- Modify as needed: `codex-rs/app-server/README.md`
- Modify if generated: `codex-rs/app-server-protocol/schema/**`
- Modify if generated: `codex-rs/core/config.schema.json`
- Modify if state migration added: `codex-rs/state/migrations/**`
- Modify if dependency changed: `codex-rs/Cargo.lock`, `codex-rs/MODULE.bazel.lock`

- [ ] **Step 1: Regenerate app-server schema if protocol changed**

Run: `just write-app-server-schema`  
Expected: generated schema files update only if Task 2 changed the API shape.

- [ ] **Step 2: Regenerate config schema if config types changed**

Run only if config types changed: `just write-config-schema`  
Expected: `codex-rs/core/config.schema.json` updates.

- [ ] **Step 3: Run targeted crate tests**

Run:

```bash
cargo test -p codex-state account_pool -- --nocapture
cargo test -p codex-core account_pool -- --nocapture
cargo test -p codex-app-server-protocol account_lease -- --nocapture
cargo test -p codex-app-server account_lease -- --nocapture
cargo test -p codex-cli --test accounts
cargo test -p codex-tui
```

Expected: PASS.

- [ ] **Step 4: Ask before full workspace tests**

Because this closes shared `state/core/protocol/app-server/cli/tui` work, ask the user before running full workspace tests:

Run after approval: `cargo test`  
Alternative if available and approved: `just test`

- [ ] **Step 5: Run scoped fixers**

Run after tests pass:

```bash
just fix -p codex-state
just fix -p codex-core
just fix -p codex-app-server-protocol
just fix -p codex-app-server
just fix -p codex-cli
just fix -p codex-tui
just fmt
```

Per repo convention, do not rerun tests after final `just fix` / `just fmt`.

- [ ] **Step 6: Commit final cleanup**

```bash
git add codex-rs docs/superpowers/plans/2026-04-12-multi-account-pool-gap-closure.md
git commit -m "chore: verify multi-account pool spec gap closure"
```

