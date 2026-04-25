# Single-Pool Startup Fallback Phase 2 App-Server And Remote Integration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose the Phase 1 startup-resolution model through app-server v2 and remote startup probing on top of the merged runtime lease authority baseline, without introducing a second lease control plane.

**Architecture:** Add an authoritative nested `AccountStartupSnapshot` to app-server read/notification surfaces, keep the existing flattened startup-resolution fields as compatibility projection, then wire local default-pool mutations through app-server using the same helper as CLI. Runtime lease ownership already lives behind `RuntimeLeaseHost`; this plan consumes that baseline and must not add an alternate acquisition, rotation, or failover path.

**Tech Stack:** Rust, Tokio, app-server v2 JSON-RPC protocol, `ts-rs`, schemars, `codex-app-server`, `codex-app-server-protocol`, `codex-account-pool`, `app_test_support`, @superpowers:test-driven-development.

---

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
- This plan must not add a second runtime lease owner, per-session fallback
  manager, or independent turn-time acquisition path.
- If a Phase 2 test exposes missing runtime-lease startup coverage, add the
  smallest regression against the existing `RuntimeLeaseHost` path before
  continuing protocol work; do not resurrect pre-host manager behavior.

## File Structure

- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
  - Add `AccountStartupSnapshot`, availability enum, issue type/source enums, candidate-pool type, and default-pool mutation request/response types.
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
- Modify: `codex-rs/app-server/src/message_processor.rs`
  - Route new client requests through `CodexMessageProcessor`.
- Modify: `codex-rs/app-server/src/thread_state.rs`
  - Ensure dedupe sees full startup snapshots in `AccountLeaseUpdatedNotification`.
- Modify: `codex-rs/app-server/tests/suite/v2/account_lease.rs`
  - Add protocol behavior tests for startup snapshots, blockers, live lease projection, and default mutations.
- Modify: `codex-rs/app-server-test-client/src/lib.rs`
  - Add helpers for `accountPool/default/set|clear` if the suite uses typed helpers.
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

- [ ] **Step 1: Confirm Phase 1 symbols exist**

Run:

```bash
rg -n "AccountStartupAvailability|AccountStartupResolutionIssue|SingleVisiblePool|set_local_default_pool|clear_local_default_pool" codex-rs/state/src codex-rs/account-pool/src
```

Expected: all symbols exist. If not, stop and complete Phase 1 first.

- [ ] **Step 2: Confirm runtime lease authority is the current baseline**

Run:

```bash
git status --short codex-rs/core
rg -n "RuntimeLeaseHost|RuntimeLeaseAuthority|LeaseAdmissionGuard" codex-rs/core/src/runtime_lease codex-rs/core/src/state/service.rs
```

Expected: `RuntimeLeaseHost`, `RuntimeLeaseAuthority`, and request admission are present. `git status` may show only the current merge or intentional follow-up edits; no Phase 2 step should add an alternate runtime lease authority.

- [ ] **Step 3: Confirm app-server protocol baseline**

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

- [ ] **Step 1: Write failing serialization tests**

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

- [ ] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol account_startup_snapshot account_pool_default -- --nocapture
```

Expected: FAIL because types/methods do not exist.

- [ ] **Step 3: Add v2 exported types**

In `v2.rs`, add:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum AccountStartupAvailability {
    Available,
    Suppressed,
    MultiplePoolsRequireDefault,
    InvalidExplicitDefault,
    Unavailable,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountStartupCandidatePool {
    pub pool_id: String,
    pub display_name: Option<String>,
    pub status: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountStartupResolutionIssue {
    pub r#type: String,
    pub source: String,
    pub pool_id: Option<String>,
    #[ts(type = "number | null")]
    pub candidate_pool_count: Option<u32>,
    pub candidate_pools: Option<Vec<AccountStartupCandidatePool>>,
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct AccountStartupSnapshot {
    pub effective_pool_id: Option<String>,
    pub effective_pool_resolution_source: String,
    pub configured_default_pool_id: Option<String>,
    pub persisted_default_pool_id: Option<String>,
    pub startup_availability: AccountStartupAvailability,
    pub startup_resolution_issue: Option<AccountStartupResolutionIssue>,
    pub selection_eligibility: String,
}
```

Do not use `skip_serializing_if` on response/notification fields.

- [ ] **Step 4: Add snapshot to read and notification types**

Add:

```rust
pub startup: AccountStartupSnapshot,
```

to both:

- `AccountLeaseReadResponse`
- `AccountLeaseUpdatedNotification`

Update `impl From<AccountLeaseReadResponse> for AccountLeaseUpdatedNotification`.

- [ ] **Step 5: Add default mutation method types**

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

- [ ] **Step 6: Run protocol tests**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add codex-rs/app-server-protocol/src/protocol/v2.rs codex-rs/app-server-protocol/src/protocol/common.rs
git commit -m "feat(app-server): add account startup snapshot protocol"
```

## Task 2: Project Shared Startup Status Into App-Server Responses

**Files:**
- Add: `codex-rs/app-server/src/account_startup_snapshot.rs`
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/src/thread_state.rs`
- Test: `codex-rs/app-server/tests/suite/v2/account_lease.rs`

- [ ] **Step 1: Add failing read-response tests**

Add tests:

```rust
#[tokio::test]
async fn account_lease_read_includes_startup_snapshot_for_single_pool_fallback() -> Result<()> {
    let mcp = start_app_server_with_single_pool_without_defaults().await?;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;

    assert_eq!(response.pool_id.as_deref(), Some("team-main"));
    assert_eq!(response.startup.effective_pool_id.as_deref(), Some("team-main"));
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
    assert_eq!(issue.r#type, "multiplePoolsRequireDefault");
    assert_eq!(issue.source, "none");
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
    assert_eq!(issue.r#type, "configDefaultPoolUnavailable");
    assert_eq!(issue.source, "configDefault");
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
    let mcp = start_app_server_with_live_lease_and_two_pools().await?;

    mcp.account_pool_default_set("team-other").await?;
    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;

    assert_eq!(response.pool_id.as_deref(), Some("team-main"));
    assert_eq!(response.startup.effective_pool_id.as_deref(), Some("team-other"));
    Ok(())
}
```

- [ ] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_lease_read_includes_startup_snapshot -- --nocapture
```

Expected: FAIL because response has no startup snapshot.

- [ ] **Step 3: Implement conversion module**

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

- [ ] **Step 4: Include snapshot in empty/startup/live responses**

In `account_lease_api.rs`:

- `empty_account_lease_response()` returns a snapshot with `startupAvailability = Unavailable`, source `none`, eligibility `missingPool`
- `account_lease_response_from_startup_status()` uses no-live-lease legacy projection from the spec
- `account_lease_response_from_runtime_snapshot()` keeps top-level live lease fields from `live_snapshot`, but sets nested `startup` from current startup status when available

- [ ] **Step 5: Update notification dedupe expectations**

Because `AccountLeaseUpdatedNotification` includes `startup`, existing whole-object dedupe in `thread_state.rs` should remain valid. Add a focused test if there is existing thread-state unit coverage; otherwise rely on app-server suite notification tests.

- [ ] **Step 6: Run app-server account lease tests**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_lease -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add codex-rs/app-server/src/account_startup_snapshot.rs codex-rs/app-server/src/account_lease_api.rs codex-rs/app-server/src/thread_state.rs codex-rs/app-server/tests/suite/v2/account_lease.rs
git commit -m "feat(app-server): return startup snapshots with account lease state"
```

## Task 3: Add App-Server Default-Pool Mutation RPCs

**Files:**
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Modify: `codex-rs/app-server-test-client/src/lib.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_lease.rs`

- [ ] **Step 1: Add failing mutation tests**

Add tests:

```rust
#[tokio::test]
async fn account_pool_default_set_reuses_cli_mutation_matrix() -> Result<()> {
    let mcp = start_app_server_with_two_pools_no_config_suppressed_preferred().await?;

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

- [ ] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool_default -- --nocapture
```

Expected: FAIL because the RPCs do not exist.

- [ ] **Step 3: Implement API functions**

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
- emit `Some(notification)` only when observable startup/live state changes
- return `Ok(None)` for successful no-op mutations
- never retarget or interrupt an already active lease
- when `live_snapshot` is present, build the notification from the live top-level lease fields plus the updated nested startup snapshot

- [ ] **Step 4: Dispatch JSON-RPC methods**

In `codex_message_processor.rs` and `message_processor.rs`, route:

- `accountPool/default/set`
- `accountPool/default/clear`

Responses are empty response objects. Notifications use `accountLease/updated` only for state-changing calls.

- [ ] **Step 5: Add test-client helpers**

Add helpers:

```rust
pub async fn account_pool_default_set(&mut self, pool_id: &str) -> Result<JSONRPCResponse>;
pub async fn account_pool_default_clear(&mut self) -> Result<JSONRPCResponse>;
```

- [ ] **Step 6: Run app-server mutation tests**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool_default -- --nocapture
cargo test -p codex-app-server account_lease_resume -- --nocapture
```

Expected: PASS, and `accountLease/resume` remains behaviorally identical to CLI `accounts resume`.

- [ ] **Step 7: Commit**

```bash
git add codex-rs/app-server/src/account_lease_api.rs codex-rs/app-server/src/codex_message_processor.rs codex-rs/app-server/src/message_processor.rs codex-rs/app-server-test-client/src/lib.rs codex-rs/app-server/tests/suite/v2/account_lease.rs
git commit -m "feat(app-server): add default pool mutation rpc"
```

## Task 4: Consume Remote Startup Snapshot In TUI Probe

**Files:**
- Modify: `codex-rs/tui/src/startup_access.rs`
- Test: `codex-rs/tui/src/startup_access.rs`

- [ ] **Step 1: Add failing remote probe tests**

Add tests:

```rust
#[test]
fn remote_startup_probe_uses_snapshot_multi_pool_blocker() {
    let issue = AccountStartupResolutionIssue {
        r#type: "multiplePoolsRequireDefault".to_string(),
        source: "none".to_string(),
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
```

Add equivalent remote probe tests for invalid persisted default, invalid config
default, and invalid override. Each test must assert the `StartupNoticeData`
issue source and candidate pool ids, not just the high-level probe variant.

- [ ] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-tui remote_startup_probe_uses_snapshot_multi_pool_blocker -- --nocapture
```

Expected: FAIL because remote probe still uses legacy `pool_id`/`suppressed`.

- [ ] **Step 3: Map remote snapshot availability**

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

- [ ] **Step 4: Run TUI tests**

Run:

```bash
cd codex-rs
cargo test -p codex-tui startup_access -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add codex-rs/tui/src/startup_access.rs
git commit -m "feat(tui): use remote startup snapshots"
```

## Runtime Lease Baseline Validation

Runtime lease authority has already landed in `main`; Phase 2 must consume that
path instead of adding another core lease owner. Before adding app-server protocol
surface area, verify the existing runtime lease path still covers or can be
extended with focused regressions for:

- a state-only home with one visible pool and no configured/persisted default
  acquires a pooled lease for a real turn
- multi-pool-without-default blocks acquisition with a structured
  `multiplePoolsRequireDefault` issue
- invalid explicit defaults do not fall back to another visible pool
- suppressed single-pool fallback does not acquire until resumed
- request admission still flows through the runtime lease host; no per-session
  fallback manager is introduced

If any of those facts are missing, add targeted coverage against the existing
`RuntimeLeaseHost` implementation before proceeding. Do not move the app-server
startup snapshot work into `codex-rs/core` unless the missing coverage proves a
real host integration bug.

## Task 5: Docs, Schema Generation, And Final Verification

**Files:**
- Modify: `codex-rs/app-server/README.md`
- Regenerate schema fixtures touched by `just write-app-server-schema`
- All files touched above.

- [ ] **Step 1: Update app-server docs**

Document:

- `AccountStartupSnapshot` on `accountLease/read`
- `startup` on `accountLease/updated`
- `accountPool/default/set`
- `accountPool/default/clear`
- no-op mutation notification behavior
- live top-level lease fields remaining separate from nested `startup`

- [ ] **Step 2: Regenerate app-server schema**

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

- [ ] **Step 3: Run verification**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol
cargo test -p codex-app-server account_lease
cargo test -p codex-tui startup_access
```

Expected: PASS.

- [ ] **Step 4: Format and fix**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-app-server-protocol
just fix -p codex-app-server
just fix -p codex-tui
```

Expected: PASS. Do not rerun tests after `fmt`/`fix` unless code was manually changed afterward.

- [ ] **Step 5: Final commit if needed**

```bash
git status --short
git add codex-rs/app-server/README.md codex-rs/app-server-protocol codex-rs/app-server codex-rs/tui
git commit -m "docs(app-server): document startup snapshot account lease api"
```

Only create this commit if docs/schema updates were not already included in earlier task commits.
