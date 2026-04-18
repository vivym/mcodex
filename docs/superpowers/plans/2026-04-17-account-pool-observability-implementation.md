# Account Pool Observability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the app-server-first pooled account observability slice: stable v2 read contracts for one known pool, local event/history storage, backend-neutral read seams, and local app-server handlers without adding CLI/TUI consumers.

**Architecture:** Keep the public observability contract in `codex-app-server-protocol`, put the backend-neutral read seam in `codex-account-pool`, implement local snapshots/events/diagnostics in new focused `codex-state` modules instead of growing the existing 4k-line account-pool runtime file, and expose the RPCs through a dedicated app-server module wired through the existing `accountLease`-style request path in `message_processor`/`codex_message_processor`. Persist only append-only event history; derive diagnostics at read time.

**Tech Stack:** Rust workspace crates (`codex-app-server-protocol`, `codex-app-server`, `codex-account-pool`, `codex-state`, `codex-core`), SQLite via `sqlx`, app-server v2 JSON-RPC schema fixtures, `pretty_assertions`, targeted crate tests, and `just write-app-server-schema` for generated protocol fixtures.

## Execution Status

Status as of 2026-04-18: implemented and verified. The checkbox tracker below was
not maintained during execution; use this section as the authoritative summary
of what landed.

Completed implementation slices:

- protocol contract: `06c24ae58 feat(protocol): add account pool observability rpc types`
- protocol fixture follow-up: `1090c306d fix(protocol): require nullable account pool response fields in schema`
- local state storage/reads: `30aa15fa3 feat(state): add pooled observability storage and reads`
- state follow-ups:
  - `fefbca9fc fix(state): align pooled diagnostics and selection events`
  - `bdabb5bab fix(state): emit cleared startup selection events`
  - `913e1089b fix(state): treat auth-failed leases as non-viable`
  - `a05fb60a4 fix(state): tighten pooled observability semantics`
  - `dd0346cc8 fix(state): preserve account list pagination`
  - `4d2b39067 fix(state): keep healthy single-lease pools healthy`
- runtime-only event emission: `099fa3b20 feat(core): emit pooled observability decision events`
- backend-neutral seam:
  - `c7fdb3f48 feat(account-pool): add observability reader seam`
  - `268112a79 fix(account-pool): decouple observability seam models`
- app-server exposure: `13a57ebe9 feat(app-server): expose account pool observability rpc`

Acceptance criteria in the paired design doc are met:

- app-server v2 exposes `accountPool/read`, `accountPool/accounts/list`,
  `accountPool/events/list`, and `accountPool/diagnostics/read`
- the local backend returns pool summary, account list, event history, and
  derived diagnostics through the backend-neutral observability seam
- event history is durably recorded in `state/migrations/0030_account_pool_events.sql`
- diagnostics remain derived at read time rather than persisted separately

Targeted verification completed during implementation:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol account_pool_observability -- --nocapture
cargo test -p codex-app-server-protocol schema_fixtures -- --nocapture
cargo test -p codex-state account_pool_observability -- --nocapture
cargo test -p codex-core observability_event -- --nocapture
cargo test -p codex-account-pool observability -- --nocapture
cargo test -p codex-app-server account_pool_ -- --nocapture
just fmt
just fix -p codex-app-server
just fix -p codex-state
git diff --check
```

Still intentionally deferred for follow-on work:

- remote backend support against the same observability contract
- CLI/TUI consumers of the new RPCs
- write-side control-plane operations such as pause/resume/drain
- richer local quota/pause/drain summary facts where no backend-authoritative
  local source exists yet; the contract is in place and local v1 returns `null`
  for those fields instead of inventing shadow state

---

## Scope

In scope:

- add app-server v2 request/response/enums for:
  - `accountPool/read`
  - `accountPool/accounts/list`
  - `accountPool/events/list`
  - `accountPool/diagnostics/read`
- keep the scope limited to one known `poolId`
- add local append-only pooled event storage and cursor-based event reads
- add local pool/account snapshot reads and derived diagnostics reads
- add a backend-neutral observability reader seam in `codex-account-pool`
- expose the new reads through app-server and document them in `app-server/README.md`

Out of scope:

- pool discovery / inventory RPCs
- pause / resume / drain / other write-side operator commands
- remote backend implementation
- CLI commands
- TUI views
- history backfill for runs before the new event table exists
- workspace-wide `cargo test` without explicit user approval

## Execution Rules

- Use `@superpowers:test-driven-development` before each implementation task.
- Use `@superpowers:verification-before-completion` before claiming a task is done.
- Run the targeted tests listed in each task before `just fmt` or `just fix -p ...`.
- Run `just fmt` from `codex-rs/` after each Rust code task.
- Run `just fix -p <crate>` for each touched crate after the task’s tests pass.
- Do not rerun tests after `just fmt` or `just fix -p ...`.
- If any task changes `codex-rs/app-server-protocol/src/protocol/v2.rs`, run:

```bash
cd codex-rs
just write-app-server-schema
cargo test -p codex-app-server-protocol
```

- Do not touch unrelated local changes in other files while executing this plan.

## Planned File Layout

- Modify `codex-rs/app-server-protocol/src/protocol/v2.rs` to add the four v2 request/response payloads, nested response structs, plus shared enums for backend kind, account operational state, event type, reason code, diagnostics status, and diagnostics severity.
- Modify `codex-rs/app-server-protocol/src/protocol/common.rs` to add the new `ClientRequest` method mappings.
- Create `codex-rs/app-server-protocol/tests/account_pool_observability.rs` for serialization and request-shape coverage; keep schema fixture verification in the existing generated-fixture tests.
- Create `codex-rs/state/migrations/0030_account_pool_events.sql` for append-only pooled event history and indexes.
- Create `codex-rs/state/src/model/account_pool_observability.rs` for focused observability model types instead of growing `codex-rs/state/src/model/account_pool.rs`.
- Create `codex-rs/state/src/runtime/account_pool_observability.rs` for local snapshot/event/diagnostics queries instead of extending the 4000+ line `codex-rs/state/src/runtime/account_pool.rs`.
- Modify `codex-rs/state/src/runtime/account_pool.rs` to append durable observability events from existing local state write paths such as lease lifecycle, health updates, and startup-selection updates.
- Modify `codex-rs/state/src/model/mod.rs`, `codex-rs/state/src/runtime.rs`, and `codex-rs/state/src/lib.rs` to export the new observability model/runtime APIs.
- Create `codex-rs/account-pool/src/observability.rs` for the backend-neutral read trait and shared filter/cursor types.
- Create `codex-rs/account-pool/src/backend/local/observability.rs` for the local implementation that adapts `codex-state` reads into the backend-neutral seam.
- Modify `codex-rs/account-pool/src/backend.rs`, `codex-rs/account-pool/src/backend/local/mod.rs`, and `codex-rs/account-pool/src/lib.rs` to expose the observability seam cleanly.
- Modify `codex-rs/core/src/state/service.rs` and `codex-rs/core/tests/suite/account_pool.rs` to emit runtime-only observability events for proactive switch outcomes that only the live manager knows.
- Create `codex-rs/app-server/src/account_pool_api.rs` for pooled observability handlers; keep it separate from `account_lease_api.rs` so the single-lease path stays focused. This module assembles the `policy` response from `Config`; `codex-state` owns persisted state facts, not configured policy.
- Modify `codex-rs/app-server/src/lib.rs`, `codex-rs/app-server/src/message_processor.rs`, and `codex-rs/app-server/src/codex_message_processor.rs` to register and dispatch the new RPCs through the existing `accountLease`-style request path.
- Create `codex-rs/app-server/tests/suite/v2/account_pool.rs` and modify `codex-rs/app-server/tests/suite/v2/mod.rs` for end-to-end RPC coverage.
- Modify `codex-rs/app-server/README.md` to document the new v2 methods and example payloads.

### Task 1: Define The App-Server v2 Observability Contract

**Files:**
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Create: `codex-rs/app-server-protocol/tests/account_pool_observability.rs`
- Generated: `codex-rs/app-server-protocol/schema/json/**`
- Generated: `codex-rs/app-server-protocol/schema/typescript/**`

- [ ] **Step 1: Write failing protocol tests**

Create `codex-rs/app-server-protocol/tests/account_pool_observability.rs` with focused request/response serialization tests:

```rust
use codex_app_server_protocol::AccountPoolBackendKind;
use codex_app_server_protocol::AccountPoolPolicyResponse;
use codex_app_server_protocol::AccountPoolReadParams;
use codex_app_server_protocol::AccountPoolReadResponse;
use codex_app_server_protocol::AccountPoolSummaryResponse;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn account_pool_read_params_serialize_pool_id_in_camel_case() {
    let params = AccountPoolReadParams {
        pool_id: "team-main".to_string(),
    };

    assert_eq!(serde_json::to_value(&params).unwrap(), json!({"poolId": "team-main"}));
}

#[test]
fn account_pool_backend_kind_serializes_as_closed_enum() {
    assert_eq!(serde_json::to_value(AccountPoolBackendKind::Local).unwrap(), json!("local"));
}

#[test]
fn account_pool_read_response_preserves_nullable_summary_fields() {
    let response = AccountPoolReadResponse {
        pool_id: "team-main".to_string(),
        backend: AccountPoolBackendKind::Local,
        summary: AccountPoolSummaryResponse {
            total_accounts: 2,
            active_leases: 1,
            available_accounts: Some(1),
            leased_accounts: Some(1),
            paused_accounts: None,
            draining_accounts: None,
            near_exhausted_accounts: None,
            exhausted_accounts: None,
            error_accounts: None,
        },
        policy: AccountPoolPolicyResponse {
            allocation_mode: "exclusive".to_string(),
            allow_context_reuse: true,
            proactive_switch_threshold_percent: Some(85),
            min_switch_interval_secs: Some(300),
        },
        refreshed_at: 1_710_000_000,
    };

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["summary"]["pausedAccounts"], serde_json::Value::Null);
}
```

- [ ] **Step 2: Run the new protocol tests to verify the API does not exist yet**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol account_pool_observability -- --nocapture
```

Expected: FAIL with missing types / request variants / enum definitions.

- [ ] **Step 3: Add the v2 payloads and enums**

In `codex-rs/app-server-protocol/src/protocol/v2.rs`, add:

- request params:
  - `AccountPoolReadParams`
  - `AccountPoolAccountsListParams`
  - `AccountPoolEventsListParams`
  - `AccountPoolDiagnosticsReadParams`
- responses:
  - `AccountPoolReadResponse`
  - `AccountPoolAccountsListResponse`
  - `AccountPoolEventsListResponse`
  - `AccountPoolDiagnosticsReadResponse`
- nested response structs:
  - `AccountPoolSummaryResponse`
  - `AccountPoolPolicyResponse`
  - `AccountPoolAccountResponse`
  - `AccountPoolLeaseResponse`
  - `AccountPoolQuotaResponse`
  - `AccountPoolSelectionResponse`
  - `AccountPoolEventResponse`
  - `AccountPoolDiagnosticsIssueResponse`
- shared enums:
  - `AccountPoolBackendKind`
  - `AccountOperationalState`
  - `AccountPoolEventType`
  - `AccountPoolReasonCode`
  - `AccountPoolDiagnosticsStatus`
  - `AccountPoolDiagnosticsSeverity`

Keep the wire contract aligned with the spec:

- `poolId` required on every request
- `accountPool/events/list` supports `accountId`, `types`, `cursor`, and `limit`, but not `since`
- nullable fields stay explicit; do not use `skip_serializing_if`
- `states` filter is `Option<Vec<AccountOperationalState>>`
- `accountKinds` filter is `Option<Vec<String>>`
- `types` filter is `Option<Vec<AccountPoolEventType>>`

Spell out the nested response fields in the protocol task so the implementation cannot drift from the spec:

- `AccountPoolSummaryResponse`
  - `total_accounts: u32`
  - `active_leases: u32`
  - `available_accounts: Option<u32>`
  - `leased_accounts: Option<u32>`
  - `paused_accounts: Option<u32>`
  - `draining_accounts: Option<u32>`
  - `near_exhausted_accounts: Option<u32>`
  - `exhausted_accounts: Option<u32>`
  - `error_accounts: Option<u32>`
- `AccountPoolPolicyResponse`
  - `allocation_mode: String`
  - `allow_context_reuse: bool`
  - `proactive_switch_threshold_percent: Option<u8>`
  - `min_switch_interval_secs: Option<u64>`
- `AccountPoolAccountResponse`
  - `account_id: String`
  - `backend_account_ref: Option<String>`
  - `account_kind: String`
  - `enabled: bool`
  - `health_state: Option<String>`
  - `operational_state: Option<AccountOperationalState>`
  - `allocatable: Option<bool>`
  - `status_reason_code: Option<AccountPoolReasonCode>`
  - `status_message: Option<String>`
  - `current_lease: Option<AccountPoolLeaseResponse>`
  - `quota: Option<AccountPoolQuotaResponse>`
  - `selection: Option<AccountPoolSelectionResponse>`
  - `updated_at: i64`
- `AccountPoolLeaseResponse`
  - `lease_id: String`
  - `lease_epoch: u64`
  - `holder_instance_id: String`
  - `acquired_at: i64`
  - `renewed_at: i64`
  - `expires_at: i64`
- `AccountPoolQuotaResponse`
  - `remaining_percent: Option<f64>`
  - `resets_at: Option<i64>`
  - `observed_at: i64`
- `AccountPoolSelectionResponse`
  - `eligible: bool`
  - `next_eligible_at: Option<i64>`
  - `preferred: bool`
  - `suppressed: bool`
- `AccountPoolEventResponse`
  - `event_id: String`
  - `occurred_at: i64`
  - `pool_id: String`
  - `account_id: Option<String>`
  - `lease_id: Option<String>`
  - `holder_instance_id: Option<String>`
  - `event_type: AccountPoolEventType`
  - `reason_code: Option<AccountPoolReasonCode>`
  - `message: String`
  - `details: Option<serde_json::Value>`
- `AccountPoolDiagnosticsReadResponse`
  - `pool_id: String`
  - `generated_at: i64`
  - `status: AccountPoolDiagnosticsStatus`
  - `issues: Vec<AccountPoolDiagnosticsIssueResponse>`
- `AccountPoolDiagnosticsIssueResponse`
  - `severity: AccountPoolDiagnosticsSeverity`
  - `reason_code: AccountPoolReasonCode`
  - `message: String`
  - `account_id: Option<String>`
  - `holder_instance_id: Option<String>`
  - `next_relevant_at: Option<i64>`

Also add the new `ClientRequest` variants in `codex-rs/app-server-protocol/src/protocol/common.rs` so the typed request layer knows:

- `accountPool/read`
- `accountPool/accounts/list`
- `accountPool/events/list`
- `accountPool/diagnostics/read`

- [ ] **Step 4: Regenerate schema fixtures and rerun protocol tests**

Run:

```bash
cd codex-rs
just write-app-server-schema
cargo test -p codex-app-server-protocol account_pool_observability -- --nocapture
cargo test -p codex-app-server-protocol schema_fixtures -- --nocapture
```

Expected: PASS. The generated JSON/TypeScript fixtures include the four new methods and types.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-app-server-protocol
git add app-server-protocol/src/protocol/v2.rs app-server-protocol/src/protocol/common.rs app-server-protocol/tests/account_pool_observability.rs app-server-protocol/schema
git commit -m "feat(protocol): add account pool observability rpc types"
```

### Task 2: Add Local Event Storage And State Read APIs

**Files:**
- Create: `codex-rs/state/migrations/0030_account_pool_events.sql`
- Create: `codex-rs/state/src/model/account_pool_observability.rs`
- Create: `codex-rs/state/src/runtime/account_pool_observability.rs`
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Modify: `codex-rs/state/src/model/mod.rs`
- Modify: `codex-rs/state/src/runtime.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Test: `codex-rs/state/src/runtime/account_pool_observability.rs`
- Test: `codex-rs/state/src/runtime/account_pool.rs`

- [ ] **Step 1: Write failing state tests in the new runtime module**

Create `codex-rs/state/src/runtime/account_pool_observability.rs` with focused tests:

```rust
#[tokio::test]
async fn list_account_pool_events_returns_descending_cursor_page() {
    let runtime = test_runtime().await;
    seed_account_pool_event(&runtime, "evt-1", 100, "leaseAcquired").await;
    seed_account_pool_event(&runtime, "evt-2", 200, "leaseReleased").await;

    let first = runtime
        .list_account_pool_events(
            "team-main",
            /*account_id*/ None,
            /*types*/ None,
            /*cursor*/ None,
            /*limit*/ Some(1),
        )
        .await
        .unwrap();

    assert_eq!(first.data.len(), 1);
    assert_eq!(first.data[0].event_id, "evt-2");
    assert!(first.next_cursor.is_some());
}

#[tokio::test]
async fn read_account_pool_snapshot_leaves_unknown_counts_null() {
    let runtime = test_runtime().await;
    seed_registered_account(&runtime, "acct-1", "team-main").await;

    let snapshot = runtime.read_account_pool_snapshot("team-main").await.unwrap();

    assert_eq!(snapshot.summary.total_accounts, 1);
    assert_eq!(snapshot.summary.paused_accounts, None);
    assert_eq!(snapshot.summary.draining_accounts, None);
}
```

- [ ] **Step 2: Run the new state tests to verify the APIs and migration are missing**

Run:

```bash
cd codex-rs
cargo test -p codex-state account_pool_observability -- --nocapture
```

Expected: FAIL with missing module / missing methods / missing table.

- [ ] **Step 3: Add the migration and focused observability model types**

Create `codex-rs/state/migrations/0030_account_pool_events.sql`:

```sql
CREATE TABLE account_pool_events (
    event_id TEXT PRIMARY KEY,
    occurred_at INTEGER NOT NULL,
    pool_id TEXT NOT NULL,
    account_id TEXT,
    lease_id TEXT,
    holder_instance_id TEXT,
    event_type TEXT NOT NULL,
    reason_code TEXT,
    message TEXT NOT NULL,
    details_json TEXT
);

CREATE INDEX account_pool_events_pool_occurred_idx
ON account_pool_events(pool_id, occurred_at DESC, event_id DESC);

CREATE INDEX account_pool_events_account_occurred_idx
ON account_pool_events(account_id, occurred_at DESC, event_id DESC);
```

Create `codex-rs/state/src/model/account_pool_observability.rs` with focused storage-facing structs such as:

- `AccountPoolSnapshotRecord`
- `AccountPoolSummaryRecord`
- `AccountPoolAccountRecord`
- `AccountPoolEventRecord`
- `AccountPoolAccountsListQuery`
- `AccountPoolEventsListQuery`
- `AccountPoolAccountsPage`
- `AccountPoolEventsPage`
- `AccountPoolDiagnosticsRecord`
- `AccountPoolIssueRecord`
- `AccountPoolEventsCursor`

Keep these types separate from `account_pool.rs` so the existing lease/startup models do not keep growing.

- [ ] **Step 4: Implement the local read/write APIs in the new runtime module**

Add focused `StateRuntime` methods in `codex-rs/state/src/runtime/account_pool_observability.rs`:

```rust
impl StateRuntime {
    pub async fn append_account_pool_event(
        &self,
        event: AccountPoolEventRecord,
    ) -> anyhow::Result<()>;

    pub async fn read_account_pool_snapshot(
        &self,
        pool_id: &str,
    ) -> anyhow::Result<AccountPoolSnapshotRecord>;

    pub async fn list_account_pool_accounts(
        &self,
        request: AccountPoolAccountsListQuery,
    ) -> anyhow::Result<AccountPoolAccountsPage>;

    pub async fn list_account_pool_events(
        &self,
        request: AccountPoolEventsListQuery,
    ) -> anyhow::Result<AccountPoolEventsPage>;

    pub async fn read_account_pool_diagnostics(
        &self,
        pool_id: &str,
    ) -> anyhow::Result<AccountPoolDiagnosticsRecord>;
}
```

Implementation rules:

- read current facts from existing registry / membership / health / lease / startup-selection tables
- keep unknown bucket counts as `None`
- use cursor-only pagination for events
- derive diagnostics from current facts plus, if useful, a small recent event window
- keep new query/diagnostics code in `codex-rs/state/src/runtime/account_pool_observability.rs`; only touch `codex-rs/state/src/runtime/account_pool.rs` for the narrow event-emission hooks listed below

Then extend the existing local state write paths in `codex-rs/state/src/runtime/account_pool.rs` to append durable events for facts that are already authoritatively persisted there:

- lease acquired / renewed / released
- lease acquire failed when the state layer can classify the failure
- account health event recorded
- durable startup-selection updates

Add companion tests in `codex-rs/state/src/runtime/account_pool.rs` that assert the matching rows appear in `account_pool_events`.

- [ ] **Step 5: Run the targeted state tests**

Run:

```bash
cd codex-rs
cargo test -p codex-state account_pool -- --nocapture
```

Expected: PASS. The migration exists, the cursor behavior is stable, and unknown facts remain nullable.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-state
git add state/migrations/0030_account_pool_events.sql state/src/model/account_pool_observability.rs state/src/model/mod.rs state/src/runtime/account_pool.rs state/src/runtime/account_pool_observability.rs state/src/runtime.rs state/src/lib.rs
git commit -m "feat(state): add pooled observability storage and reads"
```

### Task 3: Emit Runtime-Only Observability Events From The Pooled Manager

**Files:**
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/tests/suite/account_pool.rs`

- [ ] **Step 1: Write failing core tests for proactive-switch observability events**

Add focused tests to `codex-rs/core/tests/suite/account_pool.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proactive_switch_suppressed_records_minimum_switch_interval_event() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_response_sequence(
        &server,
        vec![sse_with_primary_usage_percent("resp-1", 92.0)],
    )
    .await;

    let mut builder = pooled_accounts_builder().with_config(|config| {
        config.accounts.as_mut().unwrap().min_switch_interval_secs = Some(5);
    });
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;
    seed_account_health_state(&test, PRIMARY_ACCOUNT_ID, AccountHealthState::Healthy).await?;

    let turn_error = submit_turn_and_wait(&test, "soft pressure turn").await?;
    assert!(turn_error.is_none());

    let state_db = test.codex.state_db().expect("state db");
    let events = state_db
        .list_account_pool_events(AccountPoolEventsListQuery {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            account_id: None,
            types: Some(vec![AccountPoolEventType::ProactiveSwitchSuppressed]),
            cursor: None,
            limit: Some(10),
        })
        .await
        .unwrap();

    assert_eq!(events.data[0].reason_code, Some(AccountPoolReasonCode::MinimumSwitchInterval));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proactive_switch_selected_records_rotation_event() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_response_sequence(
        &server,
        vec![
            sse_with_primary_usage_percent("resp-1", 92.0),
            sse_with_primary_usage_percent("resp-2", 18.0),
        ],
    )
    .await;

    let mut builder = pooled_accounts_builder().with_config(|config| {
        config.accounts.as_mut().unwrap().min_switch_interval_secs = Some(0);
    });
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "pressure turn").await?;
    assert!(first_turn_error.is_none());

    let second_turn_error = submit_turn_and_wait(&test, "replacement turn").await?;
    assert!(second_turn_error.is_none());

    let state_db = test.codex.state_db().expect("state db");
    let events = state_db
        .list_account_pool_events(AccountPoolEventsListQuery {
            pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            account_id: None,
            types: Some(vec![AccountPoolEventType::ProactiveSwitchSelected]),
            cursor: None,
            limit: Some(10),
        })
        .await
        .unwrap();

    assert_eq!(events.data[0].event_type, AccountPoolEventType::ProactiveSwitchSelected);
    Ok(())
}
```

- [ ] **Step 2: Run the core tests to verify the decision events are not emitted yet**

Run:

```bash
cd codex-rs
cargo test -p codex-core observability_event -- --nocapture
```

Expected: FAIL because the manager does not append proactive switch events yet.

- [ ] **Step 3: Append manager-only events from the pooled runtime**

Update `codex-rs/core/src/state/service.rs` so the live pooled manager appends events only for decisions that cannot be recovered from durable state writes alone:

- `proactiveSwitchSelected`
- `proactiveSwitchSuppressed`
- runtime-classified `leaseAcquireFailed` when selection logic, not SQLite itself, knows the reason

Keep the write points narrow and next to the existing switch/suppression decisions. Do not move snapshot assembly or diagnostics logic into `codex-core`.

- [ ] **Step 4: Run the targeted core tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core observability_event -- --nocapture
```

Expected: PASS. Runtime-only decisions now emit the durable event history the app-server reads depend on.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-core
git add core/src/state/service.rs core/tests/suite/account_pool.rs
git commit -m "feat(core): emit pooled observability decision events"
```

### Task 4: Add The Backend-Neutral Observability Reader And Local Adapter

**Files:**
- Create: `codex-rs/account-pool/src/observability.rs`
- Create: `codex-rs/account-pool/src/backend/local/observability.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`
- Modify: `codex-rs/account-pool/src/backend/local/mod.rs`
- Modify: `codex-rs/account-pool/src/lib.rs`
- Test: `codex-rs/account-pool/tests/observability.rs`

- [ ] **Step 1: Write failing account-pool seam tests**

Create `codex-rs/account-pool/tests/observability.rs`:

```rust
use codex_account_pool::AccountPoolObservabilityReader;
use codex_account_pool::LocalAccountPoolBackend;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn local_observability_reader_returns_nullable_state_without_inventing_shadow_facts() {
    let backend = local_backend_for_test().await;

    let page = backend
        .list_accounts(AccountPoolAccountsListRequest {
            pool_id: "team-main".to_string(),
            cursor: None,
            limit: Some(10),
            states: None,
            account_kinds: None,
        })
        .await
        .unwrap();

    assert_eq!(page.data[0].operational_state, None);
    assert_eq!(page.data[0].allocatable, None);
}
```

- [ ] **Step 2: Run the seam tests to verify the trait and adapter do not exist yet**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool observability -- --nocapture
```

Expected: FAIL with missing trait / request types / local adapter methods.

- [ ] **Step 3: Add the observability seam and local implementation**

Create `codex-rs/account-pool/src/observability.rs` with:

- `AccountPoolObservabilityReader` trait
- request/filter types:
  - `AccountPoolReadRequest`
  - `AccountPoolAccountsListRequest`
  - `AccountPoolEventsListRequest`
  - `AccountPoolDiagnosticsReadRequest`
- page/result types re-exported or wrapped from `codex-state`

The trait must cover all four app-server reads so app-server does not bypass the seam:

```rust
#[async_trait]
pub trait AccountPoolObservabilityReader: Send + Sync {
    async fn read_pool(
        &self,
        request: AccountPoolReadRequest,
    ) -> anyhow::Result<AccountPoolSnapshot>;

    async fn list_accounts(
        &self,
        request: AccountPoolAccountsListRequest,
    ) -> anyhow::Result<AccountPoolAccountsPage>;

    async fn list_events(
        &self,
        request: AccountPoolEventsListRequest,
    ) -> anyhow::Result<AccountPoolEventsPage>;

    async fn read_diagnostics(
        &self,
        request: AccountPoolDiagnosticsReadRequest,
    ) -> anyhow::Result<AccountPoolDiagnostics>;
}
```

Update `codex-rs/account-pool/src/backend.rs` to re-export the new read trait alongside execution/control-plane traits.

Create `codex-rs/account-pool/src/backend/local/observability.rs` that adapts `StateRuntime` / `LocalAccountPoolBackend` to the new trait. Keep this adapter thin:

- no new policy logic
- no new event derivation
- just pass request parameters to the focused `codex-state` reads and convert them into backend-neutral types

- [ ] **Step 4: Run the targeted account-pool tests**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool observability -- --nocapture
```

Expected: PASS. The account-pool seam now exposes local observability reads without app-server depending on SQLite details.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-account-pool
git add account-pool/src/observability.rs account-pool/src/backend.rs account-pool/src/backend/local/mod.rs account-pool/src/backend/local/observability.rs account-pool/src/lib.rs account-pool/tests/observability.rs
git commit -m "feat(account-pool): add observability reader seam"
```

### Task 5: Expose The RPCs Through App-Server And Document Them

**Files:**
- Create: `codex-rs/app-server/src/account_pool_api.rs`
- Modify: `codex-rs/app-server/src/lib.rs`
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Create: `codex-rs/app-server/tests/suite/v2/account_pool.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/mod.rs`
- Modify: `codex-rs/app-server/README.md`

- [ ] **Step 1: Write failing app-server tests for the four RPCs**

Create `codex-rs/app-server/tests/suite/v2/account_pool.rs` with focused RPC tests:

Add small local test helpers in the same file, copying the focused account seeding pattern from `account_lease.rs` instead of modifying shared test support in this task:

- `seed_default_pool_state(...)`
- `seed_two_accounts(...)`
- `write_pooled_auth(...)`
- `test_event(...)`

```rust
#[tokio::test]
async fn account_pool_read_returns_summary_for_known_pool() -> Result<()> {
    let codex_home = TempDir::new()?;
    seed_two_accounts(codex_home.path()).await?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;
    mcp.initialize().await?;

    let request_id = mcp
        .send_raw_request(
            "accountPool/read",
            AccountPoolReadParams {
                pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
            },
        )
        .await?;
    let response = mcp
        .read_stream_until_response_message(RequestId::Integer(request_id))
        .await?;

    let parsed: AccountPoolReadResponse = to_response(response)?;
    assert_eq!(parsed.pool_id, LEGACY_DEFAULT_POOL_ID);
    assert_eq!(parsed.summary.total_accounts, 2);
    assert_eq!(parsed.summary.paused_accounts, None);
    Ok(())
}

#[tokio::test]
async fn account_pool_events_list_paginates_with_cursor_only() -> Result<()> {
    let codex_home = TempDir::new()?;
    let runtime = seed_two_accounts(codex_home.path()).await?;
    runtime.append_account_pool_event(test_event("evt-1", 100)).await?;
    runtime.append_account_pool_event(test_event("evt-2", 200)).await?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    mcp.initialize().await?;
    let request_id = mcp
        .send_raw_request(
            "accountPool/events/list",
            AccountPoolEventsListParams {
                pool_id: LEGACY_DEFAULT_POOL_ID.to_string(),
                account_id: None,
                types: None,
                cursor: None,
                limit: Some(1),
            },
        )
        .await?;
    let response = mcp
        .read_stream_until_response_message(RequestId::Integer(request_id))
        .await?;

    let parsed: AccountPoolEventsListResponse = to_response(response)?;
    assert_eq!(parsed.data[0].event_id, "evt-2");
    assert!(parsed.next_cursor.is_some());
    Ok(())
}
```

Cover all four methods:

- `accountPool/read`
- `accountPool/accounts/list`
- `accountPool/events/list`
- `accountPool/diagnostics/read`

- [ ] **Step 2: Run the app-server tests to verify the request routing does not exist yet**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool_ -- --nocapture
```

Expected: FAIL with unsupported method / missing `ClientRequest` handling / missing handler functions.

- [ ] **Step 3: Add the dedicated pooled account API module and request routing**

Create `codex-rs/app-server/src/account_pool_api.rs` with focused helpers such as:

- `read_account_pool(...)`
- `list_account_pool_accounts(...)`
- `list_account_pool_events(...)`
- `read_account_pool_diagnostics(...)`

Implementation rules:

- resolve/open the state DB the same way `account_lease_api.rs` does
- build a local observability reader from the local backend seam
- assemble the `AccountPoolPolicyResponse` from `Config.accounts`; do not make `codex-state` own configured policy values
- keep the module read-only; no write-side control-plane behavior

Wire the new methods through the existing typed request path:

- add `mod account_pool_api;` in `lib.rs`
- dispatch the four `ClientRequest` variants in `message_processor.rs`
- mirror the same methods in `codex_message_processor.rs`, next to `account_lease_read` / `account_lease_resume`

Do not grow `account_lease_api.rs` into a general pool API module.

- [ ] **Step 4: Update the app-server README and rerun targeted tests**

Document each new method in `codex-rs/app-server/README.md` with example request/response payloads that match the final nullable-field rules.

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool_ -- --nocapture
```

Expected: PASS. The RPCs are routable, return the documented nullable fields, and diagnostics are derived rather than stored.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-app-server
git add app-server/src/account_pool_api.rs app-server/src/lib.rs app-server/src/message_processor.rs app-server/src/codex_message_processor.rs app-server/tests/suite/v2/account_pool.rs app-server/tests/suite/v2/mod.rs app-server/README.md
git commit -m "feat(app-server): expose account pool observability rpc"
```

### Task 6: Final Targeted Verification

**Files:**
- Verify only; no planned code changes

- [x] **Step 1: Re-run crate-level targeted verification in dependency order**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol account_pool_observability -- --nocapture
cargo test -p codex-app-server-protocol schema_fixtures -- --nocapture
cargo test -p codex-state account_pool_observability -- --nocapture
cargo test -p codex-core observability_event -- --nocapture
cargo test -p codex-account-pool observability -- --nocapture
cargo test -p codex-app-server account_pool_ -- --nocapture
```

Expected: PASS.

- [x] **Step 2: Verify generated schema is clean**

Run:

```bash
git diff --check
git status --short
```

Expected:

- no whitespace errors
- only the intended observability files are modified

- [x] **Step 3: Hand off for execution**

Record:

- new migration version `0030_account_pool_events.sql`
- generated schema fixture updates
- the fact that CLI/TUI remain out of scope for this plan

Recorded on April 18, 2026:

- reran the targeted post-merge verification commands, including `codex-app-server-protocol` schema fixtures, `codex-state account_pool_observability`, `codex-core` observability-event tests, `codex-account-pool observability`, and `codex-app-server account_pool_`
- reran `just write-app-server-schema`, `just write-config-schema`, `git diff --check`, and `just bazel-lock-check`; no generated-file drift or whitespace issues remained afterward
- CLI/TUI identity validation also passed in the broader branch validation pass, but those surfaces remained outside the scope of this observability implementation plan itself
