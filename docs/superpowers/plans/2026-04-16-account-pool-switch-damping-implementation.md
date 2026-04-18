# Account Pool Switch Damping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land runtime-local proactive switch damping for pooled accounts so threshold-only pressure stops causing immediate durable rate-limit rotation, while hard failures still fail over immediately and additive live diagnostics reach app-server and TUI.

**Architecture:** Keep the soft-pressure policy in a small shared helper under `codex-account-pool`, then reuse that helper from both the local backend manager and the live `codex-core` pooled runtime manager so they do not drift. Expose only additive live snapshot/protocol fields for damping state such as lease acquisition time and proactive-switch suppression timing; do not overload `next_eligible_at`, and do not persist damping state in SQLite as if it were installation-wide cooldown.

**Tech Stack:** Rust workspace crates (`codex-account-pool`, `codex-core`, `codex-app-server`, `codex-app-server-protocol`, `codex-tui`), existing pooled lease/state runtime, chrono timestamps, `pretty_assertions`, and `insta` snapshot coverage for TUI status rendering.

---

## Scope

In scope:

- wire the existing `accounts.min_switch_interval_secs` config value into both pooled runtime managers
- distinguish soft proactive pressure from hard failures in `report_rate_limits(...)` vs `report_usage_limit_reached(...)` / unauthorized paths
- keep soft proactive pressure runtime-local and non-durable
- preserve stronger no-immediate-switch-back behavior after proactive rotation
- extend live account-lease snapshots and app-server v2 account-lease responses with additive damping fields
- render the new live damping state in TUI status surfaces without reusing `next_eligible_at`

Out of scope:

- changing `codex accounts switch`
- adding `accounts doctor`
- redesigning `codex accounts status` to pretend it has process-local live runtime state
- remote backend implementation
- any new SQLite schema or durable cooldown persistence
- changes to `codex-rs/config/src/types.rs` or config schema generation unless the implementation uncovers a genuine gap; the config field already exists and validates in `codex-rs/core/src/config/mod.rs`

## Execution Rules

- Use `@superpowers:test-driven-development` before each implementation task.
- Use `@superpowers:verification-before-completion` before claiming a task is done.
- Run the targeted tests listed in each task before `just fmt` or `just fix -p ...`.
- Run `just fmt` from `codex-rs/` after each Rust code task.
- Run `just fix -p codex-account-pool`, `just fix -p codex-core`, `just fix -p codex-app-server-protocol`, `just fix -p codex-app-server`, or `just fix -p codex-tui` after the task that changes that crate passes its tests.
- Do not rerun tests after `just fmt` or `just fix -p ...`.
- If any task changes `codex-rs/app-server-protocol/src/protocol/v2.rs`, run `just write-app-server-schema` and `cargo test -p codex-app-server-protocol`.
- Ask the user before running workspace-wide `cargo test` because this slice changes `codex-core`.

## Planned File Layout

- Create `codex-rs/account-pool/src/proactive_switch.rs` for the runtime-local soft-pressure state machine and diagnostics snapshot shared by both manager implementations.
- Modify `codex-rs/account-pool/src/lib.rs` to export only the small proactive-switch types needed by `codex-core`.
- Modify `codex-rs/account-pool/src/types.rs` to add `min_switch_interval_secs` to `AccountPoolConfig` plus a duration/helper accessor. Do not touch `codex-rs/config/src/types.rs`.
- Modify `codex-rs/account-pool/src/manager.rs` to stop persisting `RateLimited` for threshold-only pressure, to schedule proactive rotation only after the damping window and a fresh observation, and to remember the just-replaced account for anti-flap.
- Modify `codex-rs/account-pool/tests/lease_lifecycle.rs` for focused coverage of soft-pressure suppression, stale-pressure expiry, hard-failure bypass, and proactive anti-flap.
- Modify `codex-rs/core/Cargo.toml` and `codex-rs/core/BUILD.bazel` to consume the shared proactive-switch helper from `codex-account-pool`.
- Modify `codex-rs/core/src/state/service.rs` to extend the live lease snapshot, reuse the shared helper in the pooled runtime manager, and keep hard-failure replay semantics unchanged.
- Modify `codex-rs/core/tests/suite/account_pool.rs` for live snapshot coverage of suppression, fresh revalidation after the window opens, hard-failure bypass, and no-immediate-switch-back.
- Modify `codex-rs/app-server-protocol/src/protocol/v2.rs` to add additive wire fields on `AccountLeaseReadResponse` / `AccountLeaseUpdatedNotification` for lease acquisition and proactive-switch live state.
- Modify `codex-rs/app-server/src/account_lease_api.rs` to map the new live snapshot fields without changing existing field meanings.
- Modify `codex-rs/app-server/README.md` if the public v2 account-lease response example needs to show the new additive fields.
- Modify `codex-rs/app-server/tests/suite/v2/account_lease.rs` to lock the additive JSON contract and rotated-notification behavior.
- Modify `codex-rs/tui/src/status/account.rs`, `codex-rs/tui/src/app_server_session.rs`, and `codex-rs/tui/src/status/card.rs` so TUI renders the new damping fields as a distinct “can switch at” / note path instead of as “next eligible”.
- Modify `codex-rs/tui/src/status/tests.rs` and `codex-rs/tui/src/chatwidget/tests/status_command_tests.rs` to accept the new status text and snapshot output.

### Task 1: Extract Shared Proactive-Switch Policy

**Files:**
- Create: `codex-rs/account-pool/src/proactive_switch.rs`
- Modify: `codex-rs/account-pool/src/lib.rs`
- Modify: `codex-rs/account-pool/src/types.rs`
- Test: `codex-rs/account-pool/src/proactive_switch.rs`

- [x] **Step 1: Write failing unit tests for the shared policy helper**

Add focused tests in `codex-rs/account-pool/src/proactive_switch.rs` that lock the desired state machine without involving SQLite or the full managers:

```rust
#[test]
fn soft_pressure_is_suppressed_before_min_switch_interval() {
    let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
    let observed_at = acquired_at + Duration::minutes(2);
    let mut state = ProactiveSwitchState::default();

    let outcome = state.observe_soft_pressure(ProactiveSwitchObservation {
        lease_acquired_at: acquired_at,
        observed_at,
        min_switch_interval: Duration::minutes(10),
    });

    assert_eq!(
        outcome,
        ProactiveSwitchOutcome::Suppressed {
            allowed_at: acquired_at + Duration::minutes(10),
        }
    );
    assert_eq!(
        state.snapshot(acquired_at + Duration::minutes(3)),
        ProactiveSwitchSnapshot {
            pending: true,
            suppressed: true,
            allowed_at: Some(acquired_at + Duration::minutes(10)),
        }
    );
}

#[test]
fn stale_soft_pressure_expires_when_window_opens_without_forcing_rotation() {
    let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
    let observed_at = acquired_at + Duration::minutes(2);
    let mut state = ProactiveSwitchState::default();
    state.observe_soft_pressure(ProactiveSwitchObservation {
        lease_acquired_at: acquired_at,
        observed_at,
        min_switch_interval: Duration::minutes(10),
    });

    let expired = state.revalidate_before_turn(acquired_at + Duration::minutes(11));

    assert_eq!(expired, ProactiveSwitchTurnDecision::KeepCurrentLease);
    assert_eq!(
        state.snapshot(acquired_at + Duration::minutes(11)),
        ProactiveSwitchSnapshot::default()
    );
}

#[test]
fn fresh_soft_pressure_after_window_requests_rotation() {
    let acquired_at = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
    let observed_at = acquired_at + Duration::minutes(12);
    let mut state = ProactiveSwitchState::default();

    let outcome = state.observe_soft_pressure(ProactiveSwitchObservation {
        lease_acquired_at: acquired_at,
        observed_at,
        min_switch_interval: Duration::minutes(10),
    });

    assert_eq!(outcome, ProactiveSwitchOutcome::RotateOnNextTurn);
}
```

- [x] **Step 2: Run the new unit tests and confirm they fail**

Run: `cargo test -p codex-account-pool proactive_switch -- --nocapture`

Expected: compile/test failure because the proactive-switch helper types and behavior do not exist yet.

- [x] **Step 3: Implement the shared helper and config accessor**

Create `codex-rs/account-pool/src/proactive_switch.rs` with a small, reusable API. Keep it data-only and runtime-local:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProactiveSwitchSnapshot {
    pub pending: bool,
    pub suppressed: bool,
    pub allowed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProactiveSwitchOutcome {
    NoAction,
    Suppressed { allowed_at: DateTime<Utc> },
    RotateOnNextTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProactiveSwitchTurnDecision {
    KeepCurrentLease,
    RotateAwayFromActive,
}
```

Rules to implement:

- use `lease_acquired_at + min_switch_interval` as the soft-pressure clock
- keep the remembered pressure live-only and clear it once the window opens without a fresh observation
- return `RotateOnNextTurn` only for a fresh soft-pressure observation at or after `allowed_at`
- expose a lightweight snapshot for runtime/app-server/TUI diagnostics

Add `min_switch_interval_secs` plus `min_switch_interval_duration()` to `AccountPoolConfig`, defaulting to `0`.

- [x] **Step 4: Re-run the shared-policy tests**

Run: `cargo test -p codex-account-pool proactive_switch -- --nocapture`

Expected: PASS for the new shared-policy tests.

- [x] **Step 5: Commit the shared helper**

```bash
git add codex-rs/account-pool/src/proactive_switch.rs codex-rs/account-pool/src/lib.rs codex-rs/account-pool/src/types.rs
git commit -m "feat: add pooled proactive switch policy helper"
```

### Task 2: Wire Soft/Hard Rotation Semantics Into `codex-account-pool`

**Files:**
- Modify: `codex-rs/account-pool/src/manager.rs`
- Test: `codex-rs/account-pool/tests/lease_lifecycle.rs`

- [x] **Step 1: Write failing manager tests for soft-pressure suppression and hard-failure bypass**

Add focused tests in `codex-rs/account-pool/tests/lease_lifecycle.rs`:

```rust
#[tokio::test]
async fn soft_pressure_before_min_interval_does_not_persist_rate_limited_health() {
    let harness = fixture_with_two_registered_accounts().await;
    let config = AccountPoolConfig {
        min_switch_interval_secs: 600,
        ..default_config()
    };
    let start = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
    let mut manager = harness.manager("holder-a", config).expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(start),
            pool_id: None,
        })
        .await
        .expect("acquire lease");

    manager
        .report_rate_limits(
            first.key(),
            RateLimitSnapshot::new(95.0, start + Duration::minutes(2)),
        )
        .await
        .expect("record soft pressure");

    let second = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(start + Duration::minutes(3)),
            pool_id: None,
        })
        .await
        .expect("keep sticky lease");

    assert_eq!(first.account_id(), second.account_id());
    assert_eq!(
        harness
            .runtime
            .read_account_health_event_sequence(first.account_id())
            .await
            .expect("read health sequence"),
        None
    );
}

#[tokio::test]
async fn stale_soft_pressure_does_not_force_delayed_rotation_after_window_opens() {
    let harness = fixture_with_two_registered_accounts().await;
    let config = AccountPoolConfig {
        min_switch_interval_secs: 600,
        ..default_config()
    };
    let start = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
    let mut manager = harness.manager("holder-a", config).expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(start),
            pool_id: None,
        })
        .await
        .expect("acquire lease");

    manager
        .report_rate_limits(
            first.key(),
            RateLimitSnapshot::new(95.0, start + Duration::minutes(2)),
        )
        .await
        .expect("record soft pressure");

    let same = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(start + Duration::minutes(11)),
            pool_id: None,
        })
        .await
        .expect("stale pressure must not rotate");

    assert_eq!(same.account_id(), first.account_id());
}

#[tokio::test]
async fn hard_usage_limit_bypasses_min_switch_interval() {
    let harness = fixture_with_two_registered_accounts().await;
    let config = AccountPoolConfig {
        min_switch_interval_secs: 600,
        ..default_config()
    };
    let start = Utc.with_ymd_and_hms(2026, 4, 16, 9, 0, 0).unwrap();
    let mut manager = harness.manager("holder-a", config).expect("create manager");
    let first = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(start),
            pool_id: None,
        })
        .await
        .expect("acquire lease");

    manager
        .report_usage_limit_reached(first.key(), UsageLimitEvent::new(start + Duration::minutes(2)))
        .await
        .expect("record hard limit");

    let rotated = manager
        .ensure_active_lease(SelectionRequest {
            now: Some(start + Duration::minutes(3)),
            pool_id: None,
        })
        .await
        .expect("hard failure should rotate immediately");

    assert_ne!(rotated.account_id(), first.account_id());
}
```

- [x] **Step 2: Run the focused manager tests and confirm they fail**

Run: `cargo test -p codex-account-pool --test lease_lifecycle -- --nocapture`

Expected: FAIL because `report_rate_limits(...)` still persists `RateLimited` and the manager has no damping state.

- [x] **Step 3: Implement the manager wiring**

Update `codex-rs/account-pool/src/manager.rs` so that:

- `report_rate_limits(...)` consults the shared proactive-switch helper instead of directly recording `RateLimited`
- threshold-only pressure never calls `backend.record_health_event(...)`
- `report_usage_limit_reached(...)` and `report_unauthorized(...)` keep the existing hard-health persistence path
- the proactive helper is cleared when the active lease is released or replaced
- the manager remembers the just-replaced account after a proactive rotation and excludes it from the immediate reselection path unless there is no other eligible account

Keep the public `HealthEventDisposition` surface unchanged unless tests prove the current enum is too coarse.

- [x] **Step 4: Re-run the focused manager tests**

Run: `cargo test -p codex-account-pool --test lease_lifecycle -- --nocapture`

Expected: PASS.

- [x] **Step 5: Commit the local-manager behavior change**

```bash
git add codex-rs/account-pool/src/manager.rs codex-rs/account-pool/tests/lease_lifecycle.rs
git commit -m "feat: damp proactive pool switching"
```

### Task 3: Reuse The Shared Policy In `codex-core` And Extend Live Snapshots

**Files:**
- Modify: `codex-rs/core/Cargo.toml`
- Modify: `codex-rs/core/BUILD.bazel`
- Modify: `codex-rs/core/src/state/service.rs`
- Test: `codex-rs/core/tests/suite/account_pool.rs`

- [x] **Step 1: Write failing core integration tests for live damping state**

Add focused tests in `codex-rs/core/tests/suite/account_pool.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_lease_snapshot_reports_proactive_switch_suppression_without_rate_limited_health() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    mount_sse_once(&server, sse(vec![
        ev_response_created("resp-1"),
        ev_assistant_message("m1", "soft pressure"),
        ev_completed_with_tokens("resp-1", 0, 0, 0),
    ])).await;

    let mut builder = pooled_accounts_builder();
    builder.accounts_config.min_switch_interval_secs = Some(600);
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let turn_error = submit_turn_and_wait(&test, "soft pressure turn").await?;
    assert!(turn_error.is_none());

    let snapshot = test.codex.account_lease_snapshot().await.expect("snapshot");
    assert_eq!(snapshot.health_state, Some(AccountHealthState::Healthy));
    assert_eq!(snapshot.proactive_switch_pending, Some(true));
    assert_eq!(snapshot.proactive_switch_suppressed, Some(true));
    assert!(snapshot.proactive_switch_allowed_at.is_some());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_soft_pressure_clears_after_window_without_forcing_rotation() -> Result<()> {
    // Seed a lease, report threshold pressure before min interval, advance past the
    // window without another observation, and assert the same account remains active
    // while proactive_switch_pending/suppressed clear.
    # unimplemented!()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proactive_rotation_does_not_immediately_switch_back_to_just_replaced_account() -> Result<()> {
    // Seed three accounts, trigger a proactive switch from A to B, then trigger
    // another proactive reselection and assert the runtime prefers C over A.
    # unimplemented!()
}
```

Also add an assertion in the existing hard-failure rotation tests that `usage_limit_reached` continues to rotate even when `min_switch_interval_secs` is configured.

- [x] **Step 2: Run the focused core tests and confirm they fail**

Run: `cargo test -p codex-core account_pool -- --nocapture`

Expected: FAIL because the live snapshot has no proactive-switch fields and `report_rate_limits(...)` still records `RateLimited`.

- [x] **Step 3: Implement the core runtime wiring**

Update `codex-rs/core/src/state/service.rs` so that:

- `AccountPoolManager` stores `min_switch_interval`, the shared `ProactiveSwitchState`, and a dedicated just-replaced-account id for proactive anti-flap
- `report_rate_limits(...)` uses the shared helper and only schedules `rotate_on_next_turn` for a fresh above-threshold observation after the damping window
- `report_usage_limit_reached(...)` and `report_unauthorized(...)` keep the hard-failure `record_health_event(...)` path
- stale soft pressure is cleared before turn preparation once the window has opened without a fresh observation
- `AccountLeaseRuntimeSnapshot` grows additive fields:
  - `lease_acquired_at`
  - `min_switch_interval_secs`
  - `proactive_switch_pending`
  - `proactive_switch_suppressed`
  - `proactive_switch_allowed_at`

Do not reuse `suppression_reason` or `next_eligible_at` for these live-only facts.

- [x] **Step 4: Re-run the focused core tests**

Run: `cargo test -p codex-core account_pool -- --nocapture`

Expected: PASS.

- [x] **Step 5: Commit the core runtime changes**

```bash
git add codex-rs/core/Cargo.toml codex-rs/core/BUILD.bazel codex-rs/core/src/state/service.rs codex-rs/core/tests/suite/account_pool.rs
git commit -m "feat: expose live pooled switch damping"
```

### Task 4: Extend App-Server Lease Responses Additively

**Files:**
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Test: `codex-rs/app-server/tests/suite/v2/account_lease.rs`

- [x] **Step 1: Write failing app-server tests for additive damping fields**

Add focused assertions in `codex-rs/app-server/tests/suite/v2/account_lease.rs`:

```rust
#[tokio::test]
async fn account_lease_read_and_update_report_live_proactive_switch_suppression_fields() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(vec![
        create_final_assistant_message_sse_response("soft pressure")?,
    ]).await;
    let codex_home = TempDir::new()?;
    create_pooled_config_toml_with_min_switch_interval(codex_home.path(), &server.uri(), 600)?;
    seed_default_pool_state(codex_home.path()).await?;

    let mut mcp = McpProcess::new_with_env(codex_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let thread = start_thread(&mut mcp).await?;
    let _turn = start_turn(&mut mcp, &thread.id, "soft pressure").await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.read_stream_until_notification_message("turn/completed")).await??;

    let response: AccountLeaseReadResponse = to_response(mcp.read_account_lease().await?)?;
    assert_eq!(response.proactive_switch_pending, Some(true));
    assert_eq!(response.proactive_switch_suppressed, Some(true));
    assert!(response.proactive_switch_allowed_at.is_some());
    assert!(response.lease_acquired_at.is_some());

    Ok(())
}
```

Also extend the rotated-notification test to assert the additive fields survive the `AccountLeaseUpdatedNotification` conversion.

- [x] **Step 2: Run the focused app-server tests and confirm they fail**

Run: `cargo test -p codex-app-server account_lease_read_and_update_report_live_proactive_switch_suppression_fields -- --nocapture`

Expected: FAIL because the wire types and API mapping do not yet expose the new fields.

- [x] **Step 3: Implement the additive protocol and mapping**

Update `codex-rs/app-server-protocol/src/protocol/v2.rs` and `codex-rs/app-server/src/account_lease_api.rs`:

- add `lease_acquired_at: Option<i64>`
- add `min_switch_interval_secs: Option<u64>`
- add `proactive_switch_pending: Option<bool>`
- add `proactive_switch_suppressed: Option<bool>`
- add `proactive_switch_allowed_at: Option<i64>`

Keep `AccountLeaseUpdatedNotification` as an additive `From<AccountLeaseReadResponse>` wrapper so no second mapping path drifts.

- [x] **Step 4: Regenerate schema fixtures and rerun protocol/app-server tests**

Run:

```bash
just write-app-server-schema
cargo test -p codex-app-server-protocol
cargo test -p codex-app-server account_lease_read_and_update_report_live_proactive_switch_suppression_fields -- --nocapture
```

Expected: PASS.

- [x] **Step 5: Commit the protocol update**

```bash
git add codex-rs/app-server-protocol/src/protocol/v2.rs codex-rs/app-server/src/account_lease_api.rs codex-rs/app-server/tests/suite/v2/account_lease.rs codex-rs/app-server/README.md
git commit -m "feat: add pooled damping lease diagnostics"
```

If `codex-rs/app-server/README.md` does not need changes after the wire contract is finalized, drop it from the commit.

### Task 5: Render Damping State In TUI Status Surfaces

**Files:**
- Modify: `codex-rs/tui/src/status/account.rs`
- Modify: `codex-rs/tui/src/app_server_session.rs`
- Modify: `codex-rs/tui/src/status/card.rs`
- Test: `codex-rs/tui/src/status/tests.rs`
- Test: `codex-rs/tui/src/chatwidget/tests/status_command_tests.rs`

- [x] **Step 1: Write failing TUI status tests**

Update `codex-rs/tui/src/status/tests.rs` and `codex-rs/tui/src/chatwidget/tests/status_command_tests.rs` so a live lease with soft-pressure suppression renders a dedicated damping line instead of reusing “Next eligible”:

```rust
fn test_damped_account_lease_display() -> Option<StatusAccountLeaseDisplay> {
    Some(StatusAccountLeaseDisplay {
        pool_id: Some("team-main".to_string()),
        account_id: Some("acct-1".to_string()),
        status: "Active · Healthy".to_string(),
        note: Some("Automatic switch held by minimum switch interval".to_string()),
        proactive_switch_allowed_at: Some("03:24".to_string()),
        next_eligible_at: None,
        remote_reset: None,
    })
}
```

Add snapshot assertions that the status card shows:

- `Lease note: Automatic switch held by minimum switch interval`
- `Can switch at: 03:24`

and does **not** show `Next eligible` for the same scenario.

- [x] **Step 2: Run the focused TUI tests and confirm they fail**

Run:

```bash
cargo test -p codex-tui status_snapshot_shows_damped_account_lease_without_next_eligible_time -- --nocapture
cargo test -p codex-tui status_command_renders_damped_account_lease_without_next_eligible_hint -- --nocapture
```

Expected: FAIL because `StatusAccountLeaseDisplay` and the account-lease response adapter do not yet have the new field.

- [x] **Step 3: Implement the TUI mapping and rendering**

Update:

- `codex-rs/tui/src/status/account.rs` to carry `proactive_switch_allowed_at: Option<String>`
- `codex-rs/tui/src/app_server_session.rs` to map the new response fields into:
  - a stable status string that remains based on active/suppressed/health
  - a lease note for minimum-interval suppression
  - a distinct `Can switch at` value formatted from `proactive_switch_allowed_at`
- `codex-rs/tui/src/status/card.rs` to print the new label only when present and to leave `Next eligible` reserved for real cooldown

- [x] **Step 4: Re-run TUI tests, inspect snapshots, and accept intentional updates**

Run:

```bash
cargo test -p codex-tui status_account_lease_display_from_response_formats_damped_proactive_switch -- --nocapture
cargo test -p codex-tui status_account_lease_display_from_response_hides_inert_damping_metadata -- --nocapture
cargo test -p codex-tui status_snapshot_shows_damped_account_lease_without_next_eligible_time -- --nocapture
cargo test -p codex-tui status_command_renders_damped_account_lease_without_next_eligible_hint -- --nocapture
cargo insta pending-snapshots --manifest-path tui/Cargo.toml
```

Review the generated `*.snap.new` files, then accept only the intended TUI changes:

```bash
cargo insta accept --snapshot 'tui/src/status/snapshots/codex_tui__status__tests__status_snapshot_shows_damped_account_lease_without_next_eligible_time.snap'
```

Expected: PASS with only the new damping-render snapshots accepted.

- [x] **Step 5: Commit the TUI status changes**

```bash
git add codex-rs/tui/src/status/account.rs codex-rs/tui/src/app_server_session.rs codex-rs/tui/src/status/card.rs codex-rs/tui/src/status/tests.rs codex-rs/tui/src/chatwidget/tests/status_command_tests.rs
git commit -m "feat: show pooled switch damping in tui status"
```

## Verification Checklist

- [x] `cargo test -p codex-account-pool proactive_switch -- --nocapture`
- [x] `cargo test -p codex-account-pool --test lease_lifecycle -- --nocapture`
- [x] `cargo test -p codex-core account_pool -- --nocapture`
- [x] `just write-app-server-schema`
- [x] `cargo test -p codex-app-server-protocol`
- [x] `cargo test -p codex-app-server account_lease_read_and_update_report_live_proactive_switch_suppression_fields -- --nocapture`
- [x] `cargo test -p codex-tui status_account_lease_display_from_response_formats_damped_proactive_switch -- --nocapture`
- [x] `cargo test -p codex-tui status_account_lease_display_from_response_hides_inert_damping_metadata -- --nocapture`
- [x] `cargo test -p codex-tui status_snapshot_shows_damped_account_lease_without_next_eligible_time -- --nocapture`
- [x] `cargo test -p codex-tui status_command_renders_damped_account_lease_without_next_eligible_hint -- --nocapture`
- [x] `cargo insta pending-snapshots --manifest-path tui/Cargo.toml`

## Notes For The Implementer

- Do not add a new durable SQLite table or column for damping state in this slice.
- Do not reinterpret existing `next_eligible_at` or `coolingDown` as minimum-switch suppression.
- Do not teach `codex accounts status` to infer process-local damping from shared state; that belongs to a later `accounts doctor` / live-inspection slice.
- If a helper is only used once, inline it instead of adding another wrapper function.
