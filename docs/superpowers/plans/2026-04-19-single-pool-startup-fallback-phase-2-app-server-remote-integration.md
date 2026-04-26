# Single-Pool Startup Fallback Phase 2 App-Server And Remote Integration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose the Phase 1 startup-resolution model through app-server v2 and remote startup probing on top of the merged runtime lease authority baseline, without introducing a second lease control plane.

**Architecture:** Add an authoritative nested `AccountStartupSnapshot` to app-server read/notification surfaces, keep the existing flattened startup-resolution fields as compatibility projection, then wire local startup-intent mutations through app-server using the same helper as CLI. Runtime lease ownership already lives behind `RuntimeLeaseHost`; this plan consumes that baseline and must not add an alternate acquisition, rotation, or failover path. App-server startup read/mutation RPCs are local control-plane operations and may run over WebSocket, while runtime lease admission and pooled thread execution remain guarded by the existing stdio-only `RuntimeLeaseHost` path.

**Tech Stack:** Rust, Tokio, app-server v2 JSON-RPC protocol, `ts-rs`, schemars, `codex-app-server`, `codex-app-server-protocol`, `codex-account-pool`, `app_test_support`, @superpowers:test-driven-development.

---

## Status

Completed on 2026-04-26.

Final verification:

- `just write-app-server-schema`
- `cargo test -p codex-app-server-protocol`
- `cargo test -p codex-app-server account_lease`
- `cargo test -p codex-tui startup_access`
- `cargo test -p codex-app-server`
- `LK_CUSTOM_WEBRTC=/Users/viv/.cache/mcodex-webrtc/mac-arm64-release cargo test -p codex-tui`
- `just fmt`
- `just fix -p codex-app-server-protocol`
- `just fix -p codex-app-server`
- `LK_CUSTOM_WEBRTC=/Users/viv/.cache/mcodex-webrtc/mac-arm64-release just fix -p codex-tui`
- `git diff --check`

Review-fix verification for pooled blocked startup surfaces:

- `cargo test -p codex-account-pool startup_status_multiple_pool_blocker_keeps_pooled_surface_applicable -- --nocapture`
- `cargo test -p codex-app-server account_logout_suppresses_clean_multi_pool_startup_blocker -- --nocapture`
- `cargo test -p codex-account-pool`
- `cargo test -p codex-app-server account_lease -- --nocapture`
- `cargo test -p codex-core build_account_pool_manager -- --nocapture`
- `cargo test -p codex-core build_root_runtime_lease_host -- --nocapture`
- `cargo test -p codex-core runtime_lease_host -- --nocapture`
- `cargo test -p codex-core startup_selected_pool_without_context_reuse_mints_new_remote_session_id -- --nocapture`
- `cargo test -p codex-core suppressed_startup_selection_blocks_fresh_runtime_pool_acquisition -- --nocapture`
- `cargo test -p codex-core preferred_startup_selection_is_used_for_fresh_runtime -- --nocapture`
- `just fmt`
- `just fix -p codex-account-pool`
- `just fix -p codex-app-server`

## Dependencies And Main Baseline

Prerequisites:

- Phase 1 plan is merged or available in the current branch:
  `docs/superpowers/plans/2026-04-19-single-pool-startup-fallback-phase-1-implementation.md`
- Runtime lease authority is merged into `main`, including `RuntimeLeaseHost`,
  request admission, app-server pooled scope guards, and live lease snapshot
  projection.
- App-server currently has a preliminary flattened startup projection on
  `AccountLeaseReadResponse` (`effectivePoolResolutionSource`,
  `configuredDefaultPoolId`, `persistedDefaultPoolId`) but no authoritative
  nested `AccountStartupSnapshot` and no startup parity on
  `AccountLeaseUpdatedNotification`.
- The shared startup model exposes:
  - `startup_availability`
  - `startup_resolution_issue`
  - `candidate_pools`
  - `selection_eligibility`
  - `singleVisiblePool`
  - shared default-pool mutation helper

Runtime lease boundary:

- This plan may read existing live lease snapshots exposed through app-server.
- `accountLease/read`, `accountLease/resume`, and
  `accountPool/default/set|clear` are startup-intent APIs. They must not reserve
  a top-level pooled runtime or require stdio-only runtime admission.
- Thread, turn, review, compact, and other execution paths that can acquire or
  use pooled runtime leases must continue through the existing
  `pooled_runtime_scope_required_for_config` / `RuntimeLeaseHost` admission path.
- This plan must not add a second runtime lease owner, per-session fallback
  manager, independent turn-time acquisition path, or WebSocket lease executor.
- If a Phase 2 test exposes missing runtime-lease startup coverage, add the
  smallest regression against the existing `RuntimeLeaseHost` path before
  continuing protocol work; do not resurrect pre-host manager behavior.

## File Structure

- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
  - Add `AccountStartupSnapshot`, availability enum, issue type/source enums, candidate-pool type, and default-pool mutation request/response types.
  - Mark response/notification `Option` fields that are required-nullable with `#[schemars(required, schema_with = "nullable_field_schema::<T>")]`.
  - Add `startup: AccountStartupSnapshot` to `AccountLeaseReadResponse` and `AccountLeaseUpdatedNotification`.
- Modify: `codex-rs/app-server-protocol/src/protocol/common.rs`
  - Add `accountPool/default/set` and `accountPool/default/clear` methods.
  - Update serialization tests.
- Add: `codex-rs/app-server/src/account_startup_snapshot.rs`
  - Convert Phase 1 shared startup state into app-server v2 wire types.
  - Keep projection logic out of the already dense `account_lease_api.rs`.
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
  - Include startup snapshot in reads/notifications.
  - Add default set/clear mutation functions.
  - Preserve live top-level lease fields when an active lease exists.
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
  - Dispatch new JSON-RPC methods and emit notifications when state changes.
  - Split startup-intent API transport validation from runtime lease admission so WebSocket can read/mutate startup intent without acquiring pooled runtime leases.
- Modify: `codex-rs/app-server/src/message_processor.rs`
  - Route new client requests through `CodexMessageProcessor`.
- Modify: `codex-rs/app-server/src/thread_state.rs`
  - Ensure dedupe sees full startup snapshots in `AccountLeaseUpdatedNotification`.
- Modify: `codex-rs/app-server/tests/common/mcp_process.rs`
  - Add suite helpers for `accountPool/default/set|clear`.
- Modify: `codex-rs/app-server/tests/suite/v2/account_lease.rs`
  - Add protocol behavior tests for startup snapshots, blockers, WebSocket startup-intent APIs, live lease projection, and default mutations.
- Optional: `codex-rs/app-server-test-client/src/lib.rs`
  - Add helpers for `accountPool/default/set|clear` only if a live/manual client workflow needs them.
- Modify: `codex-rs/tui/src/startup_access.rs`
  - Use remote `response.startup.startupAvailability` instead of legacy `poolId`/`suppressed` heuristics.
- Modify: `codex-rs/app-server/README.md`
  - Document new startup snapshot fields and default-pool mutation RPCs.
- Regenerate: app-server schema fixtures via `just write-app-server-schema`.

## Task 0: Preflight And Integration Gate

**Files:**
- Read: `docs/superpowers/specs/2026-04-19-single-pool-startup-fallback-and-default-pool-selection-design.md`
- Read: `docs/superpowers/specs/2026-04-18-runtime-lease-authority-for-subagents-design.md`
- Read: `docs/superpowers/plans/2026-04-19-single-pool-startup-fallback-phase-1-implementation.md`

- [x] **Step 1: Confirm Phase 1 symbols exist**

Run:

```bash
rg -n "AccountStartupAvailability|AccountStartupResolutionIssue|SingleVisiblePool|set_local_default_pool|clear_local_default_pool" codex-rs/state/src codex-rs/account-pool/src
```

Expected: all symbols exist. If not, stop and complete Phase 1 first.

- [x] **Step 2: Confirm runtime lease authority is the current baseline**

Run:

```bash
git status --short codex-rs/core
rg -n "RuntimeLeaseHost|RuntimeLeaseAuthority|LeaseAdmissionGuard" codex-rs/core/src/runtime_lease codex-rs/core/src/state/service.rs
```

Expected: `RuntimeLeaseHost`, `RuntimeLeaseAuthority`, and request admission are present. `git status` may show only the current merge or intentional follow-up edits; no Phase 2 step should add an alternate runtime lease authority.

- [x] **Step 2a: Confirm runtime lease startup coverage before app-server protocol work**

Run the existing baseline checks:

```bash
cd codex-rs
cargo test -p codex-core runtime_lease_host -- --nocapture
cargo test -p codex-core startup_selected_pool_without_context_reuse_mints_new_remote_session_id -- --nocapture
cargo test -p codex-core suppressed_startup_selection_blocks_fresh_runtime_pool_acquisition -- --nocapture
cargo test -p codex-core preferred_startup_selection_is_used_for_fresh_runtime -- --nocapture
```

Expected: PASS. If any command exposes missing coverage rather than a product
bug, add focused regressions against the existing `RuntimeLeaseHost` path before
continuing protocol work. The missing-coverage tests should be named after the
startup invariant they protect, for example:

- `single_visible_pool_startup_selection_acquires_fresh_runtime`
- `multi_pool_without_default_blocks_runtime_startup_selection`
- `invalid_explicit_default_blocks_runtime_startup_selection`

Do not move app-server startup snapshot work into `codex-rs/core` unless the
missing coverage proves a real host integration bug.

- [x] **Step 3: Confirm app-server protocol baseline**

Run:

```bash
rg -n "AccountLeaseReadResponse|AccountLeaseUpdatedNotification|AccountLeaseRead|AccountLeaseResume" codex-rs/app-server-protocol/src/protocol
```

Expected: current protocol has `accountLease/read`, `accountLease/resume`, and notification. It may already have flattened startup-resolution compatibility fields, but it must not yet have the nested `AccountStartupSnapshot` contract required by this plan.

## Task 1: Add App-Server V2 Startup Snapshot Types

**Files:**
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Test: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Test: `codex-rs/app-server-protocol/src/protocol/v2.rs`

- [x] **Step 1: Write failing serialization tests**

Add or update protocol tests so this value serializes with required nullable fields:

```rust
let snapshot = v2::AccountStartupSnapshot {
    effective_pool_id: Some("team-main".to_string()),
    effective_pool_resolution_source: "singleVisiblePool".to_string(),
    configured_default_pool_id: None,
    persisted_default_pool_id: None,
    startup_availability: v2::AccountStartupAvailability::Available,
    startup_resolution_issue: None,
    selection_eligibility: "automaticAccountSelected".to_string(),
};

let value = serde_json::to_value(snapshot)?;
assert_eq!(value["effectivePoolId"], "team-main");
assert_eq!(value["configuredDefaultPoolId"], serde_json::Value::Null);
assert_eq!(value["startupResolutionIssue"], serde_json::Value::Null);
```

Add method serialization tests for:

- `accountPool/default/set`
- `accountPool/default/clear`

- [x] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol account_startup_snapshot -- --nocapture
cargo test -p codex-app-server-protocol account_pool_default -- --nocapture
```

Expected: FAIL because types/methods do not exist.

- [x] **Step 3: Add v2 exported types**

In `v2.rs`, add:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AccountStartupAvailability {
    Available,
    Suppressed,
    MultiplePoolsRequireDefault,
    InvalidExplicitDefault,
    Unavailable,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AccountStartupResolutionIssueType {
    MultiplePoolsRequireDefault,
    OverridePoolUnavailable,
    ConfigDefaultPoolUnavailable,
    PersistedDefaultPoolUnavailable,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum AccountStartupResolutionIssueSource {
    Override,
    ConfigDefault,
    PersistedSelection,
    None,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountStartupCandidatePool {
    pub pool_id: String,
    #[schemars(required, schema_with = "nullable_field_schema::<String>")]
    pub display_name: Option<String>,
    #[schemars(required, schema_with = "nullable_field_schema::<String>")]
    pub status: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountStartupResolutionIssue {
    #[serde(rename = "type")]
    #[ts(rename = "type")]
    pub r#type: AccountStartupResolutionIssueType,
    pub source: AccountStartupResolutionIssueSource,
    #[schemars(required, schema_with = "nullable_field_schema::<String>")]
    pub pool_id: Option<String>,
    #[schemars(required, schema_with = "nullable_field_schema::<u32>")]
    pub candidate_pool_count: Option<u32>,
    #[schemars(
        required,
        schema_with = "nullable_field_schema::<Vec<AccountStartupCandidatePool>>"
    )]
    pub candidate_pools: Option<Vec<AccountStartupCandidatePool>>,
    #[schemars(required, schema_with = "nullable_field_schema::<String>")]
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountStartupSnapshot {
    #[schemars(required, schema_with = "nullable_field_schema::<String>")]
    pub effective_pool_id: Option<String>,
    pub effective_pool_resolution_source: String,
    #[schemars(required, schema_with = "nullable_field_schema::<String>")]
    pub configured_default_pool_id: Option<String>,
    #[schemars(required, schema_with = "nullable_field_schema::<String>")]
    pub persisted_default_pool_id: Option<String>,
    pub startup_availability: AccountStartupAvailability,
    #[schemars(
        required,
        schema_with = "nullable_field_schema::<AccountStartupResolutionIssue>"
    )]
    pub startup_resolution_issue: Option<AccountStartupResolutionIssue>,
    pub selection_eligibility: String,
}
```

Do not use `skip_serializing_if` on response/notification fields.

- [x] **Step 4: Add snapshot to read and notification types**

Add:

```rust
pub startup: AccountStartupSnapshot,
```

to both:

- `AccountLeaseReadResponse`
- `AccountLeaseUpdatedNotification`

Update `impl From<AccountLeaseReadResponse> for AccountLeaseUpdatedNotification`.

- [x] **Step 5: Add default mutation method types**

Add:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountPoolDefaultSetParams {
    pub pool_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountPoolDefaultSetResponse {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountPoolDefaultClearResponse {}
```

In `common.rs`, add methods:

- `AccountPoolDefaultSet => "accountPool/default/set"`
- `AccountPoolDefaultClear => "accountPool/default/clear"`

Use undefined params for clear if the request enum supports no-param methods.

- [x] **Step 6: Run protocol tests**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol
```

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add codex-rs/app-server-protocol/src/protocol/v2.rs codex-rs/app-server-protocol/src/protocol/common.rs
git commit -m "feat(app-server): add account startup snapshot protocol"
```

## Task 2: Project Shared Startup Status Into App-Server Responses

**Files:**
- Add: `codex-rs/app-server/src/account_startup_snapshot.rs`
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server/src/thread_state.rs`
- Test: `codex-rs/app-server/tests/suite/v2/account_lease.rs`

- [x] **Step 1: Add failing read-response tests**

Add tests:

```rust
#[tokio::test]
async fn account_lease_read_includes_startup_snapshot_for_single_pool_fallback() -> Result<()> {
    let mcp = start_app_server_with_single_pool_without_defaults().await?;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;

    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(response.startup.effective_pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(response.startup.effective_pool_resolution_source, "singleVisiblePool");
    assert_eq!(response.startup.startup_availability, AccountStartupAvailability::Available);
    assert_eq!(response.startup.selection_eligibility, "automaticAccountSelected");
    Ok(())
}

#[tokio::test]
async fn account_lease_read_preserves_candidate_pools_for_multi_pool_blocker() -> Result<()> {
    let mcp = start_app_server_with_two_pools_without_defaults().await?;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;

    assert_eq!(
        response.startup.startup_availability,
        AccountStartupAvailability::MultiplePoolsRequireDefault
    );
    let issue = response
        .startup
        .startup_resolution_issue
        .as_ref()
        .expect("startup issue");
    assert_eq!(
        issue.r#type,
        AccountStartupResolutionIssueType::MultiplePoolsRequireDefault
    );
    assert_eq!(issue.source, AccountStartupResolutionIssueSource::None);
    assert_eq!(issue.candidate_pool_count, Some(2));
    let pool_ids = issue
        .candidate_pools
        .as_ref()
        .expect("candidate pools")
        .iter()
        .map(|pool| pool.pool_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(pool_ids, vec!["team-main", "team-other"]);
    Ok(())
}

#[tokio::test]
async fn account_lease_read_preserves_candidate_pools_for_invalid_config_default() -> Result<()> {
    let mcp = start_app_server_with_invalid_config_default_and_one_visible_pool().await?;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;

    assert_eq!(
        response.startup.startup_availability,
        AccountStartupAvailability::InvalidExplicitDefault
    );
    let issue = response
        .startup
        .startup_resolution_issue
        .as_ref()
        .expect("startup issue");
    assert_eq!(
        issue.r#type,
        AccountStartupResolutionIssueType::ConfigDefaultPoolUnavailable
    );
    assert_eq!(
        issue.source,
        AccountStartupResolutionIssueSource::ConfigDefault
    );
    let pool_ids = issue
        .candidate_pools
        .as_ref()
        .expect("candidate pools")
        .iter()
        .map(|pool| pool.pool_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(pool_ids, vec!["team-main"]);
    Ok(())
}

#[tokio::test]
async fn account_lease_read_keeps_live_top_level_fields_separate_from_startup_snapshot() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = create_mock_responses_server_sequence_unchecked(vec![
        create_final_assistant_message_sse_response("Done")?,
    ]).await;
    create_pooled_config_toml(codex_home.path(), &server.uri())?;
    let runtime = seed_two_accounts(codex_home.path()).await?;
    runtime
        .assign_account_pool(SECONDARY_ACCOUNT_ID, "team-other")
        .await?;
    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    start_thread(&mut mcp).await?;

    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("team-other".to_string()),
            preferred_account_id: None,
            suppressed: false,
        })
        .await?;
    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;

    assert_eq!(response.pool_id.as_deref(), Some("legacy-default"));
    assert_eq!(response.startup.effective_pool_id.as_deref(), Some("team-other"));
    Ok(())
}
```

Replace the existing WebSocket pooled-runtime rejection test with read-only
startup-intent coverage:

```rust
#[tokio::test]
async fn policy_only_config_allows_websocket_account_lease_read_startup_snapshot() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_policy_only_pooled_config_toml(codex_home.path(), &server.uri())?;
    seed_default_pool_state(codex_home.path()).await?;

    let (mut process, bind_addr) = spawn_websocket_server(codex_home.path()).await?;
    let mut ws = connect_websocket(bind_addr).await?;
    send_initialize_request(&mut ws, /*id*/ 1, "ws_account_lease_read").await?;
    let _init = read_response_for_id(&mut ws, /*id*/ 1).await?;

    send_request(&mut ws, "accountLease/read", /*id*/ 2, /*params*/ None).await?;
    let response: AccountLeaseReadResponse = to_response(read_response_for_id(&mut ws, /*id*/ 2).await?)?;

    assert_eq!(response.startup.startup_availability, AccountStartupAvailability::Available);
    assert_eq!(response.startup.effective_pool_id.as_deref(), Some("legacy-default"));

    process.kill().await.context("failed to stop websocket app-server process")?;
    Ok(())
}
```

Add a companion WebSocket resume test:

- seed durable startup suppression
- call `accountLease/resume` over WebSocket
- assert the response succeeds, emits `accountLease/updated`, and the emitted
  nested `startup.startupAvailability` is no longer `suppressed`
- assert no thread or runtime lease is created as part of the resume call

Add or keep a separate WebSocket runtime-admission test proving pooled execution
still rejects over WebSocket, for example `thread/start` with pooled accounts
still returns the existing unsupported-transport error. This guards the
read/mutation split: startup-intent APIs are transport-safe, runtime lease
execution is not.

- [x] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_lease_read_includes_startup_snapshot -- --nocapture
```

Expected: FAIL because response has no startup snapshot.

- [x] **Step 3: Implement conversion module**

Create `account_startup_snapshot.rs` with conversion helpers:

```rust
pub(crate) fn snapshot_from_startup_status(
    startup: &codex_state::AccountStartupStatus,
) -> codex_app_server_protocol::AccountStartupSnapshot;
```

Rules:

- convert enum names to camelCase strings/variants at protocol boundary
- `candidate_pool_count` is `u32::try_from(...)`
- preserve the full `startup_resolution_issue`, including source,
  candidate-pool count, candidate-pool list, and deterministic `poolId`
  ordering
- `multiplePoolsRequireDefault`, invalid defaults, and unavailable map `selectionEligibility = "missingPool"`
- `suppressed` preserves underlying selection eligibility

- [x] **Step 4: Include snapshot in empty/startup/live responses**

In `account_lease_api.rs`:

- split startup reads into two helpers:
  - one full startup-status reader used by `accountLease/read`, resume/default
    mutations, and notifications; it must return blocker states even when
    no effective pool is available yet
  - one runtime-admission helper used by `pooled_mode_is_enabled`
- keep `SharedStartupStatus.pooled_applicable` aligned with the design spec:
  every startup availability except `Unavailable` represents a pooled startup
  surface, including blocked states such as `multiplePoolsRequireDefault` and
  `invalidExplicitDefault`
- `empty_account_lease_response()` returns a snapshot with `startupAvailability = Unavailable`, source `none`, eligibility `missingPool`
- `account_lease_response_from_startup_status()` uses no-live-lease legacy projection from the spec
- `account_lease_response_from_runtime_snapshot()` keeps top-level live lease fields from `live_snapshot`, but sets nested `startup` from current startup status when available

- [x] **Step 4a: Allow startup-intent APIs over WebSocket without runtime admission**

In `codex_message_processor.rs`:

- remove `pooled_runtime_scope_required_for_config(...)` from
  `account_lease_read` and `account_lease_resume`
- add a narrow startup-intent check if needed, but do not reject WebSocket
  solely because pooled mode is configured
- keep `pooled_runtime_scope_required_for_config(...)` on runtime execution
  methods such as `thread/start`, `turn/start`, `thread/resume`, review, and
  compact paths

Expected behavior:

- WebSocket `accountLease/read` can return `AccountStartupSnapshot`
- WebSocket `accountLease/resume` can clear durable startup suppression and
  emit a startup notification without creating a pooled runtime lease
- WebSocket runtime execution in pooled mode still returns the existing
  unsupported-transport error

- [x] **Step 5: Update notification dedupe expectations**

Because `AccountLeaseUpdatedNotification` includes `startup`, existing whole-object dedupe in `thread_state.rs` should remain valid. Add a focused test if there is existing thread-state unit coverage; otherwise rely on app-server suite notification tests.

Also update `account_lease_updated_notification_from_runtime_snapshot` so every
runtime snapshot notification reads the full startup snapshot. The top-level
lease fields still come from `live_snapshot`; only the nested `startup` object
comes from current startup/default state. Do not keep the current behavior where
startup context is read only for stale startup-suppressed live snapshots.

- [x] **Step 6: Run app-server account lease tests**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_lease -- --nocapture
```

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add codex-rs/app-server/src/account_startup_snapshot.rs codex-rs/app-server/src/account_lease_api.rs codex-rs/app-server/src/codex_message_processor.rs codex-rs/app-server/src/thread_state.rs codex-rs/app-server/tests/suite/v2/account_lease.rs
git commit -m "feat(app-server): return startup snapshots with account lease state"
```

## Task 3: Add App-Server Default-Pool Mutation RPCs

**Files:**
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Modify: `codex-rs/app-server/tests/common/mcp_process.rs`
- Optional: `codex-rs/app-server-test-client/src/lib.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_lease.rs`

- [x] **Step 1: Add failing mutation tests**

Add tests:

```rust
#[tokio::test]
async fn account_pool_default_set_reuses_cli_mutation_matrix() -> Result<()> {
    let mut mcp = start_app_server_with_two_pools_no_config_suppressed_preferred().await?;

    let _: JSONRPCResponse = mcp.account_pool_default_set("team-main").await?;
    let updated: AccountLeaseUpdatedNotification =
        read_notification(&mut mcp, "accountLease/updated").await?;

    assert_eq!(updated.startup.persisted_default_pool_id.as_deref(), Some("team-main"));
    assert_eq!(updated.startup.startup_availability, AccountStartupAvailability::Suppressed);

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.startup.selection_eligibility, "automaticAccountSelected");
    Ok(())
}

#[tokio::test]
async fn account_pool_default_clear_noop_does_not_emit_notification() -> Result<()> {
    let mut mcp = start_app_server_with_single_pool_without_defaults().await?;

    let _: JSONRPCResponse = mcp.account_pool_default_clear().await?;

    assert_no_account_lease_updated_notification(&mut mcp).await?;
    Ok(())
}

#[tokio::test]
async fn account_pool_default_set_notification_preserves_live_top_level_lease() -> Result<()> {
    let mut mcp = start_app_server_with_live_lease_and_two_pools().await?;

    let _: JSONRPCResponse = mcp.account_pool_default_set("team-other").await?;
    let updated: AccountLeaseUpdatedNotification =
        read_notification(&mut mcp, "accountLease/updated").await?;

    assert_eq!(updated.pool_id.as_deref(), Some("team-main"));
    assert_eq!(updated.account_id.as_deref(), Some("acct-main"));
    assert_eq!(updated.startup.effective_pool_id.as_deref(), Some("team-other"));
    Ok(())
}
```

Add the remaining mutation matrix tests before implementation:

- `account_pool_default_set_rejects_unknown_pool_with_invalid_params`
  - seed one visible pool
  - call `accountPool/default/set` with a missing pool id
  - assert JSON-RPC invalid params/request error, no state mutation, and no
    `accountLease/updated`
- `account_pool_default_set_same_state_pool_clears_preferred_when_state_controls_default`
  - seed persisted default + preferred account
  - set the same persisted default pool
  - assert persisted default remains, preferred account clears, and startup
    snapshot still points at that pool
- `account_pool_default_set_preserves_preferred_when_config_controls_default`
  - configure `accounts.default_pool`
  - seed preferred account in startup state
  - set a local default
  - assert preferred account is preserved because config controls effective
    selection
- `account_lease_resume_uses_same_startup_snapshot_projection_as_default_mutations`
  - seed durable suppression plus multi-pool/invalid-default state
  - call `accountLease/resume`
  - assert the notification/read pair contains the same `startup` shape as
    `accountPool/default/set|clear`
- `websocket_account_pool_default_set_and_clear_mutate_startup_intent_without_runtime_admission`
  - use WebSocket transport
  - call default set/clear
  - assert response/notification succeeds
  - separately assert WebSocket pooled thread execution still rejects

- [x] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool_default -- --nocapture
```

Expected: FAIL because the RPCs do not exist.

- [x] **Step 3: Implement API functions**

Add to `account_lease_api.rs`:

```rust
pub(crate) async fn set_account_pool_default(
    config: &Config,
    live_snapshot: Option<AccountLeaseRuntimeSnapshot>,
    pool_id: String,
) -> Result<Option<AccountLeaseUpdatedNotification>, JSONRPCErrorError>;

pub(crate) async fn clear_account_pool_default(
    config: &Config,
    live_snapshot: Option<AccountLeaseRuntimeSnapshot>,
) -> Result<Option<AccountLeaseUpdatedNotification>, JSONRPCErrorError>;
```

Rules:

- call Phase 1 shared helper
- pass `configured_default_pool_id(config).map(ToOwned::to_owned)` into the
  helper request so config-controlled default behavior matches CLI
- emit `Some(notification)` only when observable startup/live state changes
- return `Ok(None)` for successful no-op mutations
- never retarget or interrupt an already active lease
- when `live_snapshot` is present, build the notification from the live top-level lease fields plus the updated nested startup snapshot
- map visible-pool validation failures to a client error such as
  `INVALID_PARAMS_ERROR_CODE`; do not surface an invalid pool id as an internal
  server error

- [x] **Step 4: Dispatch JSON-RPC methods**

In `codex_message_processor.rs` and `message_processor.rs`, route:

- `accountPool/default/set`
- `accountPool/default/clear`

Responses are empty response objects. Notifications use `accountLease/updated`
only for state-changing calls.

Use the same startup-intent transport boundary as `accountLease/read` and
`accountLease/resume`: WebSocket is allowed because these methods mutate only
local startup/default intent and never reserve a pooled runtime lease.

- [x] **Step 5: Add test-client helpers**

Add helpers to `codex-rs/app-server/tests/common/mcp_process.rs`:

```rust
pub async fn account_pool_default_set(&mut self, pool_id: &str) -> Result<JSONRPCResponse>;
pub async fn account_pool_default_clear(&mut self) -> Result<JSONRPCResponse>;
```

Only add equivalent helpers to `codex-rs/app-server-test-client/src/lib.rs` if a
manual/live client workflow needs them. Do not add that crate to the required
write set solely for app-server suite tests.

- [x] **Step 6: Run app-server mutation tests**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool_default -- --nocapture
cargo test -p codex-app-server account_lease_resume -- --nocapture
```

Expected: PASS, and `accountLease/resume` remains behaviorally identical to CLI `accounts resume`.

- [x] **Step 7: Commit**

```bash
git add codex-rs/app-server/src/account_lease_api.rs codex-rs/app-server/src/codex_message_processor.rs codex-rs/app-server/src/message_processor.rs codex-rs/app-server/tests/common/mcp_process.rs codex-rs/app-server/tests/suite/v2/account_lease.rs
git commit -m "feat(app-server): add default pool mutation rpc"
```

## Task 4: Consume Remote Startup Snapshot In TUI Probe

**Files:**
- Modify: `codex-rs/tui/src/startup_access.rs`
- Test: `codex-rs/tui/src/startup_access.rs`

- [x] **Step 1: Add failing remote probe tests**

Add tests:

```rust
#[test]
fn remote_startup_probe_uses_snapshot_multi_pool_blocker() {
    let issue = AccountStartupResolutionIssue {
        r#type: AccountStartupResolutionIssueType::MultiplePoolsRequireDefault,
        source: AccountStartupResolutionIssueSource::None,
        pool_id: None,
        candidate_pool_count: Some(2),
        candidate_pools: Some(vec![
            AccountStartupCandidatePool {
                pool_id: "team-main".to_string(),
                display_name: None,
                status: None,
            },
            AccountStartupCandidatePool {
                pool_id: "team-other".to_string(),
                display_name: None,
                status: None,
            },
        ]),
        message: None,
    };
    let response = AccountLeaseReadResponse {
        startup: AccountStartupSnapshot {
            effective_pool_id: None,
            effective_pool_resolution_source: "none".to_string(),
            configured_default_pool_id: None,
            persisted_default_pool_id: None,
            startup_availability: AccountStartupAvailability::MultiplePoolsRequireDefault,
            startup_resolution_issue: Some(issue),
            selection_eligibility: "missingPool".to_string(),
        },
        ..empty_account_lease_response_for_test()
    };

    assert_eq!(
        remote_startup_probe_from_response(response),
        StartupProbe::PooledDefaultSelectionRequired {
            remote: true,
            notice: StartupNoticeData {
                issue_kind: StartupNoticeIssueKind::MultiplePoolsRequireDefault,
                issue_source: StartupNoticeIssueSource::None,
                candidate_pool_ids: vec![
                    "team-main".to_string(),
                    "team-other".to_string(),
                ],
            },
        }
    );
}

fn empty_account_lease_response_for_test() -> AccountLeaseReadResponse {
    AccountLeaseReadResponse {
        active: false,
        suppressed: false,
        account_id: None,
        pool_id: None,
        lease_id: None,
        lease_epoch: None,
        lease_acquired_at: None,
        health_state: None,
        switch_reason: None,
        suppression_reason: None,
        transport_reset_generation: None,
        last_remote_context_reset_turn_id: None,
        min_switch_interval_secs: None,
        proactive_switch_pending: None,
        proactive_switch_suppressed: None,
        proactive_switch_allowed_at: None,
        next_eligible_at: None,
        effective_pool_resolution_source: None,
        configured_default_pool_id: None,
        persisted_default_pool_id: None,
        startup: AccountStartupSnapshot {
            effective_pool_id: None,
            effective_pool_resolution_source: "none".to_string(),
            configured_default_pool_id: None,
            persisted_default_pool_id: None,
            startup_availability: AccountStartupAvailability::Unavailable,
            startup_resolution_issue: None,
            selection_eligibility: "missingPool".to_string(),
        },
    }
}
```

Add equivalent remote probe tests for invalid persisted default, invalid config
default, and invalid override. Each test must assert the `StartupNoticeData`
issue source and candidate pool ids, not just the high-level probe variant.
Add the `empty_account_lease_response_for_test()` helper first so the intended
red test fails on legacy remote probing behavior rather than on a missing test
builder.

- [x] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-tui remote_startup_probe_uses_snapshot_multi_pool_blocker -- --nocapture
```

Expected: FAIL because remote probe still uses legacy `pool_id`/`suppressed`.

- [x] **Step 3: Map remote snapshot availability**

Use `response.startup.startup_availability`:

- `Available` -> `PooledAvailable { remote: true }`
- `Suppressed` -> `PooledSuppressed { remote: true }`
- `MultiplePoolsRequireDefault` ->
  `PooledDefaultSelectionRequired { remote: true, notice }`
- `InvalidExplicitDefault` -> `PooledInvalidDefault { remote: true, notice }`
- `Unavailable` -> `Unavailable`

Build `notice` from `response.startup.startup_resolution_issue`:

- map issue type to `StartupNoticeIssueKind`
- map issue source to `StartupNoticeIssueSource`
- copy `candidatePools[*].poolId` in wire order
- when issue is unexpectedly absent, use an empty candidate list but still keep
  the correct availability-derived notice kind

Do not implement an inline remote pool picker in this phase.

- [x] **Step 4: Run TUI tests**

Run:

```bash
cd codex-rs
cargo test -p codex-tui startup_access -- --nocapture
```

Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add codex-rs/tui/src/startup_access.rs
git commit -m "feat(tui): use remote startup snapshots"
```

## Task 5: Docs, Schema Generation, And Final Verification

**Files:**
- Modify: `codex-rs/app-server/README.md`
- Regenerate schema fixtures touched by `just write-app-server-schema`
- All files touched above.

- [x] **Step 1: Update app-server docs**

Document:

- `AccountStartupSnapshot` on `accountLease/read`
- `startup` on `accountLease/updated`
- `accountPool/default/set`
- `accountPool/default/clear`
- `accountLease/read`, `accountLease/resume`, and
  `accountPool/default/set|clear` as read/write startup-intent APIs that may run
  over WebSocket without acquiring pooled runtime leases
- runtime execution APIs continuing to reject pooled WebSocket execution through
  the existing unsupported-transport path
- no-op mutation notification behavior
- live top-level lease fields remaining separate from nested `startup`

- [x] **Step 2: Regenerate app-server schema**

Run:

```bash
cd codex-rs
just write-app-server-schema
```

If experimental fixtures are affected:

```bash
just write-app-server-schema --experimental
```

Expected: schema files update cleanly.

- [x] **Step 3: Run verification**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol
cargo test -p codex-app-server account_lease
cargo test -p codex-tui startup_access
cargo test -p codex-app-server
cargo test -p codex-tui
```

Expected: PASS.

If `codex-rs/app-server-test-client/src/lib.rs` was touched despite being
optional, also run:

```bash
cd codex-rs
cargo test -p codex-app-server-test-client --no-run
```

If the local environment needs the vendored WebRTC archive, prefix the affected
commands with:

```bash
LK_CUSTOM_WEBRTC=/Users/viv/.cache/mcodex-webrtc/mac-arm64-release
```

- [x] **Step 4: Format and fix**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-app-server-protocol
just fix -p codex-app-server
just fix -p codex-tui
```

Expected: PASS. Do not rerun tests after `fmt`/`fix` unless code was manually changed afterward.

- [x] **Step 5: Final commit if needed**

```bash
git status --short
git add codex-rs/app-server/README.md codex-rs/app-server-protocol codex-rs/app-server codex-rs/tui
git commit -m "docs(app-server): document startup snapshot account lease api"
```

Only create this commit if docs/schema updates were not already included in earlier task commits.
