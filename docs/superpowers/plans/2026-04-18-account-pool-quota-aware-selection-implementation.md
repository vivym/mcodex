# Account Pool Quota-Aware Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land quota-aware pooled-account selection so startup, proactive rotation, and hard failover all choose accounts from durable per-account quota knowledge instead of legacy coarse `RateLimited` health.

**Architecture:** Add a first-class `account_quota_state` persistence layer in `codex-state`, then route both startup selection and runtime rotation through a shared selector in `codex-account-pool` that consumes lease facts, auth facts, and family-scoped quota facts. Keep wire/API compatibility additive by projecting the legacy singular `quota` field from the `codex` family while introducing typed multi-family `quotas` surfaces for app-server, CLI, and TUI.

**Tech Stack:** Rust workspace crates (`codex-state`, `codex-account-pool`, `codex-core`, `codex-app-server-protocol`, `codex-app-server`, `codex-cli`, `codex-tui`), SQLite via `sqlx`, app-server v2 schema generation, `pretty_assertions`, and `insta` snapshot coverage for TUI.

---

## Scope

In scope:

- add durable `account_quota_state` storage keyed by `account_id + limit_id`
- add shared quota-domain and selection-policy types in `codex-account-pool`
- define explicit selector intents for `Startup`, `SoftRotation`, `HardFailover`, and `ProbeRecovery`
- stop treating coarse `RateLimited` / `healthy` state as selector-facing quota truth
- persist and read quota state by `selection_family + codex fallback`
- add coordinated `next_probe_after` reservation and verification-lease reprobe wiring
- route runtime live rate-limit observations and hard usage-limit failures into the new quota store
- extend observability and app-server account-pool responses with typed `quotas`
- keep the singular compatibility `quota` field projected from `codex`
- update CLI/TUI status consumers to explain the richer quota knowledge and selection behavior

Out of scope:

- background quota probe workers
- custom weighting/config knobs for ranking formulas
- manual `accounts switch` redesign
- remote backend support beyond trait seams needed for the local backend
- workspace-wide `cargo test` without explicit user approval

## Execution Rules

- Use `@superpowers:test-driven-development` before each implementation task.
- Use `@superpowers:verification-before-completion` before claiming a task is done.
- Run the targeted tests listed in each task before `just fmt` or `just fix -p ...`.
- Run `just fmt` from `codex-rs/` after each Rust code task.
- Run `just fix -p codex-state`, `just fix -p codex-account-pool`, `just fix -p codex-core`, `just fix -p codex-app-server-protocol`, `just fix -p codex-app-server`, `just fix -p codex-cli`, or `just fix -p codex-tui` after the task that changes that crate passes its tests.
- Do not rerun tests after `just fmt` or `just fix -p ...`.
- If any task changes `codex-rs/app-server-protocol/src/protocol/v2.rs`, run:

```bash
cd codex-rs
just write-app-server-schema
cargo test -p codex-app-server-protocol
```

- Ask the user before running workspace-wide `cargo test` because this slice changes `codex-core`, `codex-state`, and shared protocol types.

## Planned File Layout

- Create `codex-rs/state/migrations/0031_account_pool_quota_state.sql` for durable per-account, per-family quota rows and supporting indexes.
- Create `codex-rs/state/src/model/account_pool_quota.rs` for focused quota-state records, enums, and conversion helpers instead of growing `codex-rs/state/src/model/account_pool.rs`.
- Create `codex-rs/state/src/runtime/account_pool_quota.rs` for quota-state reads, writes, probe reservation CAS updates, and compatibility projections.
- Modify `codex-rs/state/src/model/mod.rs`, `codex-rs/state/src/runtime.rs`, and `codex-rs/state/src/lib.rs` to export the new quota-state modules cleanly.
- Modify `codex-rs/state/src/runtime/account_pool.rs` so startup/lease acquisition fetches candidate facts from quota-aware state queries and stop using coarse `RateLimited` as a veto.
- Modify `codex-rs/state/src/model/account_pool.rs` only for compatibility projections that still need coarse health/status enums; quota truth lives in the new module.
- Create `codex-rs/state/tests/account_pool_quota.rs` for migration-backed persistence and probe-throttle integration coverage.
- Create `codex-rs/account-pool/src/quota.rs` for selector-facing quota-domain structs such as family views, exhausted-window state, probe results, and plan/reason enums.
- Create `codex-rs/account-pool/src/quota_selection.rs` for the shared four-stage selection engine and per-intent admissibility rules.
- Modify `codex-rs/account-pool/src/policy.rs` to reduce startup selection to a thin wrapper over the shared engine instead of keeping independent health-based logic.
- Modify `codex-rs/account-pool/src/types.rs` to thread `selection_family` and any explicit selector context through `SelectionRequest` instead of inventing ad hoc call-site parameters.
- Modify `codex-rs/account-pool/src/backend.rs` plus `codex-rs/account-pool/src/backend/local/execution.rs` and `codex-rs/account-pool/src/backend/local/mod.rs` to add quota-aware candidate loading, probe reservation, and lease-scoped quota refresh capability.
- Modify `codex-rs/account-pool/src/manager.rs` so soft-rotation reprobe keeps the active lease held, uses a dedicated verification lease, always releases that verification lease, and reruns the original intent before any real reselection.
- Modify `codex-rs/account-pool/src/lib.rs` to export the new selector types without leaking more API than needed.
- Create `codex-rs/account-pool/tests/quota_selection.rs` for pure policy coverage and update `codex-rs/account-pool/tests/lease_lifecycle.rs` for runtime manager integration coverage.
- Modify `codex-rs/core/src/state/service.rs` to resolve `selection_family`, write live quota observations, trigger hard-failure quota writes, and adapt selector outcomes into runtime rotation/failover flows.
- Modify `codex-rs/core/tests/suite/account_pool.rs` for startup selection, hard failover, reprobe, and early-reset behavior.
- Modify `codex-rs/state/src/model/account_pool_observability.rs`, `codex-rs/state/src/runtime/account_pool_observability.rs`, `codex-rs/account-pool/src/observability.rs`, and `codex-rs/account-pool/src/observability/conversions.rs` to surface typed `quotas` while keeping additive compatibility fields.
- Modify `codex-rs/app-server-protocol/src/protocol/v2.rs`, `codex-rs/app-server/src/account_pool_api.rs`, and `codex-rs/app-server/src/account_pool_api/conversions.rs` to publish the richer account-pool quota contract.
- Modify `codex-rs/app-server/tests/suite/v2/account_pool.rs` and `codex-rs/app-server/README.md` to lock and document the additive API behavior.
- Modify `codex-rs/cli/src/accounts/output.rs` and `codex-rs/cli/tests/accounts.rs` to print the new multi-family quota facts and selection explanations deterministically.
- Modify `codex-rs/tui/src/status/account.rs`, `codex-rs/tui/src/status/rate_limits.rs`, `codex-rs/tui/src/status/card.rs`, `codex-rs/tui/src/status/tests.rs`, and affected snapshots so runtime status reflects the richer quota model.

### Task 1: Add Durable Quota-State Storage In `codex-state`

**Files:**
- Create: `codex-rs/state/migrations/0031_account_pool_quota_state.sql`
- Create: `codex-rs/state/src/model/account_pool_quota.rs`
- Create: `codex-rs/state/src/runtime/account_pool_quota.rs`
- Create: `codex-rs/state/tests/account_pool_quota.rs`
- Modify: `codex-rs/state/src/model/mod.rs`
- Modify: `codex-rs/state/src/runtime.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Modify: `codex-rs/state/src/model/account_pool.rs`

- [ ] **Step 1: Write failing persistence tests**

Create `codex-rs/state/tests/account_pool_quota.rs` with focused coverage for family-scoped quota rows, independent `predicted_blocked_until` / `next_probe_after`, and probe reservation CAS:

```rust
#[tokio::test]
async fn quota_rows_are_scoped_by_account_and_limit_family() {
    let harness = AccountPoolQuotaHarness::new().await;
    harness
        .write_quota_observation(quota_row("acct-a", "codex").with_primary_used(82.0))
        .await
        .unwrap();
    harness
        .write_quota_observation(quota_row("acct-a", "chatgpt").with_primary_used(37.0))
        .await
        .unwrap();

    let codex = harness.read_quota_state("acct-a", "codex").await.unwrap().unwrap();
    let chatgpt = harness.read_quota_state("acct-a", "chatgpt").await.unwrap().unwrap();

    assert_eq!(codex.limit_id, "codex");
    assert_eq!(chatgpt.limit_id, "chatgpt");
    assert_ne!(codex.primary_used_percent, chatgpt.primary_used_percent);
}

#[tokio::test]
async fn probe_reservation_only_succeeds_when_next_probe_after_has_elapsed() {
    let harness = AccountPoolQuotaHarness::new().await;
    let now = Utc::now();
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_next_probe_after(now - Duration::seconds(1)),
        )
        .await
        .unwrap();

    assert_eq!(
        harness
            .reserve_probe_slot("acct-a", "codex", now, now + Duration::seconds(30))
            .await
            .unwrap(),
        ProbeReservationOutcome::Reserved
    );
}

#[tokio::test]
async fn selection_family_row_wins_before_codex_fallback_and_probe_results_update_backoff() {
    let harness = AccountPoolQuotaHarness::new().await;
    let now = Utc::now();
    harness
        .write_quota_observation(quota_row("acct-a", "codex").with_primary_used(12.0))
        .await
        .unwrap();
    harness
        .write_quota_observation(
            quota_row("acct-a", "chatgpt")
                .with_primary_used(88.0)
                .with_exhausted_windows(QuotaExhaustedWindows::Unknown)
                .with_probe_backoff_level(0),
        )
        .await
        .unwrap();

    let selected = harness
        .read_selection_quota_facts("acct-a", "chatgpt")
        .await
        .unwrap();
    assert_eq!(selected.limit_id, "chatgpt");

    harness
        .record_probe_result(
            "acct-a",
            "chatgpt",
            ProbeWrite::ambiguous(now + Duration::minutes(10)),
        )
        .await
        .unwrap();

    let refreshed = harness.read_quota_state("acct-a", "chatgpt").await.unwrap().unwrap();
    assert_eq!(refreshed.last_probe_result, Some(QuotaProbeResult::Ambiguous));
    assert_eq!(refreshed.probe_backoff_level, 1);
}

#[tokio::test]
async fn legacy_rate_limited_rows_do_not_synthesize_quota_blocking_truth_after_upgrade() {
    let harness = AccountPoolQuotaHarness::new().await;
    harness.seed_legacy_rate_limited_health("acct-a").await.unwrap();

    let quota = harness
        .read_selection_quota_facts("acct-a", "codex")
        .await
        .unwrap();

    assert!(quota.is_none());
}

#[tokio::test]
async fn fresh_non_exhausted_observation_immediately_clears_existing_blocked_row() {
    let harness = AccountPoolQuotaHarness::new().await;
    let now = Utc::now();
    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(now + Duration::hours(1)),
        )
        .await
        .unwrap();

    harness
        .write_quota_observation(
            quota_row("acct-a", "codex")
                .with_exhausted_windows(QuotaExhaustedWindows::None)
                .with_observed_at(now + Duration::minutes(5)),
        )
        .await
        .unwrap();

    let refreshed = harness.read_quota_state("acct-a", "codex").await.unwrap().unwrap();
    assert_eq!(refreshed.exhausted_windows, QuotaExhaustedWindows::None);
    assert_eq!(refreshed.predicted_blocked_until, None);
}
```

- [ ] **Step 2: Run the new state tests to verify the quota store does not exist yet**

Run:

```bash
cd codex-rs
cargo test -p codex-state account_pool_quota -- --nocapture
```

Expected: FAIL with missing migration, missing quota-state types, and missing runtime APIs.

- [ ] **Step 3: Implement the migration, model types, and runtime helpers**

Implement:

- `0031_account_pool_quota_state.sql` with a durable `account_quota_state` table keyed by `(account_id, limit_id)`
- explicit enums/records for:
  - `QuotaExhaustedWindows`
  - `QuotaProbeResult`
  - `AccountQuotaStateRecord`
- runtime APIs for:
  - upserting quota observations
  - reading `selection_family` first and only falling back to `codex` when the family row is absent
  - compare-and-set probe reservation on `next_probe_after`
  - clearing exhaustion on successful reprobe
  - recording `success`, `still_blocked`, and `ambiguous` probe outcomes while updating `probe_backoff_level` and `last_probe_result`
  - ignoring or normalizing preexisting persisted `RateLimited` rows so selector paths never continue to treat them as quota truth after upgrade
  - treating `predicted_blocked_until` as a recovery prediction only: it must not auto-clear `exhausted_windows`, and a fresh non-exhausted live observation must immediately overwrite stale blocked state
  - projecting compatibility coarse cooldown/rate-limited reads from quota rows

Keep coarse `AccountHealthState::Unauthorized` support intact, but do not let quota writes depend on legacy `RateLimited`.

- [ ] **Step 4: Re-run the state tests and the existing state account-pool suite**

Run:

```bash
cd codex-rs
cargo test -p codex-state account_pool_quota -- --nocapture
cargo test -p codex-state account_pool -- --nocapture
```

Expected: PASS for the new quota-state tests and no regressions in the existing account-pool slice.

- [ ] **Step 5: Format, lint, and commit the state slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-state
git add state/migrations/0031_account_pool_quota_state.sql state/src/model/account_pool_quota.rs state/src/runtime/account_pool_quota.rs state/src/model/mod.rs state/src/runtime.rs state/src/lib.rs state/src/model/account_pool.rs state/tests/account_pool_quota.rs
git commit -m "feat(state): add account pool quota state"
```

### Task 2: Build The Shared Quota-Aware Selector In `codex-account-pool`

**Files:**
- Create: `codex-rs/account-pool/src/quota.rs`
- Create: `codex-rs/account-pool/src/quota_selection.rs`
- Create: `codex-rs/account-pool/tests/quota_selection.rs`
- Modify: `codex-rs/account-pool/src/policy.rs`
- Modify: `codex-rs/account-pool/src/types.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`
- Modify: `codex-rs/account-pool/src/lib.rs`
- Modify: `codex-rs/account-pool/tests/policy.rs`

- [ ] **Step 1: Write failing policy tests for veto, ranking, and reprobe intent rules**

Create `codex-rs/account-pool/tests/quota_selection.rs` with pure selector tests:

```rust
#[test]
fn secondary_exhausted_account_is_vetoed_before_primary_ranking() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup))
        .with_candidate(candidate("acct-hot").with_secondary_exhausted())
        .with_candidate(candidate("acct-cool").with_primary_used(41.0))
        .run();

    assert_eq!(plan.terminal_action, SelectionAction::Select("acct-cool".into()));
    assert_rejected_reason(&plan, "acct-hot", SelectionRejectReason::PredictedBlocked);
}

#[test]
fn soft_rotation_stays_on_current_when_no_other_admissible_candidate_exists() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::SoftRotation)
            .with_current_account("acct-a")
            .with_just_replaced_account("acct-b"),
    )
    .with_candidate(candidate("acct-a").with_primary_used(91.0))
    .with_candidate(candidate("acct-b").with_primary_used(20.0))
    .run();

    assert_eq!(plan.terminal_action, SelectionAction::StayOnCurrent);
}

#[test]
fn stale_primary_blocked_account_becomes_probe_candidate_after_ordinary_candidates_exhaust() {
    let plan = build_selection_plan(selection_request(SelectionIntent::HardFailover))
        .with_candidate(candidate("acct-a").with_primary_block(now_minus_minutes(20)))
        .run();

    assert_eq!(plan.terminal_action, SelectionAction::Probe("acct-a".into()));
}

#[test]
fn missing_or_partial_quota_data_stays_not_blocked_but_ranks_below_complete_rows() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup))
        .with_candidate(candidate("acct-low-confidence").with_missing_secondary_window())
        .with_candidate(candidate("acct-complete").with_primary_used(48.0).with_secondary_used(22.0))
        .run();

    assert_eq!(plan.eligible_candidates[0].account_id, "acct-complete");
    assert_eq!(plan.eligible_candidates[1].account_id, "acct-low-confidence");
}

#[test]
fn selection_family_row_is_used_before_codex_fallback() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::HardFailover).with_selection_family("chatgpt"),
    )
    .with_candidate(
        candidate("acct-a")
            .with_family_quota("codex", healthy_primary(10.0))
            .with_family_quota("chatgpt", exhausted_primary()),
    )
    .with_candidate(candidate("acct-b").with_family_quota("chatgpt", healthy_primary(44.0)))
    .run();

    assert_eq!(plan.terminal_action, SelectionAction::Select("acct-b".into()));
}

#[test]
fn hard_failover_may_reuse_just_replaced_account_and_reports_decision_reason() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::HardFailover)
            .with_current_account("acct-a")
            .with_just_replaced_account("acct-b"),
    )
    .with_candidate(candidate("acct-a").with_primary_used(99.0))
    .with_candidate(candidate("acct-b").with_primary_used(18.0))
    .run();

    assert_eq!(plan.terminal_action, SelectionAction::Select("acct-b".into()));
    assert_eq!(plan.decision_reason, SelectionDecisionReason::HardFailoverOverride);
}

#[test]
fn probe_recovery_only_rechecks_reserved_target_and_can_return_no_candidate() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::ProbeRecovery).with_reserved_probe_target("acct-probe"),
    )
    .with_candidate(candidate("acct-probe").with_primary_block(now_minus_minutes(30)))
    .run();

    assert_eq!(plan.probe_candidate.as_deref(), Some("acct-probe"));
    assert!(plan.eligible_candidates.is_empty());
    assert_eq!(plan.terminal_action, SelectionAction::Probe("acct-probe".into()));
}

#[test]
fn primary_threshold_beats_lower_position_candidate_even_before_later_tie_breakers() {
    let plan = build_selection_plan(
        selection_request(SelectionIntent::Startup).with_proactive_threshold_percent(85),
    )
    .with_candidate(candidate("acct-threshold-hot").with_primary_used(87.0).with_secondary_used(5.0))
    .with_candidate(candidate("acct-threshold-safe").with_primary_used(52.0).with_secondary_used(40.0))
    .run();

    assert_eq!(plan.eligible_candidates[0].account_id, "acct-threshold-safe");
}

#[test]
fn ranking_uses_primary_then_secondary_then_reset_then_stable_tie_breakers() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup))
        .with_candidate(candidate("acct-b").with_primary_used(40.0).with_secondary_used(30.0))
        .with_candidate(
            candidate("acct-a")
                .with_primary_used(40.0)
                .with_secondary_used(30.0)
                .with_primary_reset(now_plus_minutes(5)),
        )
        .run();

    assert_eq!(plan.eligible_candidates[0].account_id, "acct-a");
}

#[test]
fn reprobe_prefers_stale_primary_block_over_fresher_secondary_block() {
    let plan = build_selection_plan(selection_request(SelectionIntent::HardFailover))
        .with_candidate(candidate("acct-secondary").with_secondary_block(now_minus_minutes(5)))
        .with_candidate(candidate("acct-primary").with_primary_block(now_minus_minutes(40)))
        .run();

    assert_eq!(plan.terminal_action, SelectionAction::Probe("acct-primary".into()));
}

#[test]
fn exhausted_row_stays_blocked_after_predicted_blocked_until_until_cleared_by_new_fact() {
    let plan = build_selection_plan(selection_request(SelectionIntent::Startup).with_now(now_plus_hours(2)))
        .with_candidate(
            candidate("acct-a")
                .with_exhausted_windows(QuotaExhaustedWindows::Secondary)
                .with_predicted_blocked_until(now_plus_minutes(5)),
        )
        .run();

    assert_eq!(plan.terminal_action, SelectionAction::Probe("acct-a".into()));
    assert_rejected_reason(&plan, "acct-a", SelectionRejectReason::PredictedBlocked);
}
```

- [ ] **Step 2: Run the new selector tests to verify the shared engine does not exist yet**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool quota_selection -- --nocapture
```

Expected: FAIL with missing quota-domain structs, selector types, and terminal actions.

- [ ] **Step 3: Implement the quota-domain types and shared selection engine**

Implement:

- `quota.rs` with explicit selector-facing data types:
  - `SelectionIntent`
  - `SelectionAction`
  - `SelectionPlan`
  - `SelectionRejectReason`
  - `QuotaFamilyView`
  - `QuotaBlockClass`
  - `ProbeOutcome`
- `quota_selection.rs` with the four-stage pipeline from the spec:
  - hard filtering
  - soft-block classification
  - primary-first ordinary ranking
  - single-candidate reprobe nomination
- missing/partial quota rows remain `NotBlocked` with lower confidence and rank below otherwise comparable complete rows
- `predicted_blocked_until` never auto-unblocks an exhausted row by itself; exhausted rows stay blocked until successful reprobe or fresh live observation clears them
- `selection_family` facts win whenever present; `codex` fallback is used only when the requested family row is absent
- all required selector outputs remain first-class:
  - `eligible_candidates`
  - `probe_candidate`
  - `rejected_candidates`
  - `decision_reason`
  - terminal `Select`, `Probe`, `StayOnCurrent`, and `NoCandidate`
- `HardFailover` explicitly readmits `just_replaced_account_id`, while `ProbeRecovery` evaluates only the reserved probe target and reruns the original intent after probe execution
- `policy.rs` as a thin startup wrapper over the new engine
- `types.rs` updates so `SelectionRequest` explicitly carries `selection_family` and other selector context needed by startup/runtime callers
- backend seam additions needed for callers to supply `selection_family`, fallback family facts, and quota-refresh probe results

Keep `just_replaced_account_id` inside the selector input rather than duplicating that filtering in callers.

- [ ] **Step 4: Re-run selector tests and existing startup-policy coverage**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool quota_selection -- --nocapture
cargo test -p codex-account-pool policy -- --nocapture
```

Expected: PASS for the new shared selector tests and the legacy startup wrapper coverage.

- [ ] **Step 5: Format, lint, and commit the selector slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-account-pool
git add account-pool/src/quota.rs account-pool/src/quota_selection.rs account-pool/src/policy.rs account-pool/src/types.rs account-pool/src/backend.rs account-pool/src/lib.rs account-pool/tests/quota_selection.rs account-pool/tests/policy.rs
git commit -m "feat(account-pool): add quota-aware selector"
```

### Task 3: Wire Quota-Aware Candidate Fetching And Reprobe Into The Local Backend

**Files:**
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Modify: `codex-rs/account-pool/src/manager.rs`
- Modify: `codex-rs/account-pool/src/backend/local/execution.rs`
- Modify: `codex-rs/account-pool/src/backend/local/mod.rs`
- Modify: `codex-rs/account-pool/tests/lease_lifecycle.rs`

- [ ] **Step 1: Write failing backend/runtime tests for selection-family fetches and probe reservation flow**

Extend `codex-rs/account-pool/tests/lease_lifecycle.rs` with integration coverage:

```rust
#[tokio::test]
async fn acquire_lease_prefers_primary_safe_account_over_lower_position_blocked_account() {
    let harness = quota_fixture_with_three_accounts().await;
    harness.write_quota("acct-a", "codex", exhausted_secondary()).await;
    harness.write_quota("acct-b", "codex", healthy_primary(44.0)).await;

    let lease = harness
        .backend()
        .acquire_runtime_selected_lease(selection_request("pool-main"))
        .await
        .unwrap();

    assert_eq!(lease.account_id, "acct-b");
}

#[tokio::test]
async fn runtime_selection_uses_requested_family_before_consulting_codex_fallback() {
    let harness = quota_fixture_with_three_accounts().await;
    harness.write_quota("acct-a", "codex", healthy_primary(5.0)).await;
    harness.write_quota("acct-a", "chatgpt", exhausted_primary()).await;
    harness.write_quota("acct-b", "chatgpt", healthy_primary(42.0)).await;

    let lease = harness
        .backend()
        .acquire_runtime_selected_lease(selection_request("pool-main").with_selection_family("chatgpt"))
        .await
        .unwrap();

    assert_eq!(lease.account_id, "acct-b");
}

#[tokio::test]
async fn probe_reservation_is_left_in_place_when_verification_lease_loses_a_race() {
    let harness = quota_fixture_with_probe_candidate().await;

    let result = harness.force_probe_lease_contention().await;

    assert_eq!(result, ProbeExecutionOutcome::SelectionRestartRequired);
    assert!(harness.read_quota("acct-probe", "codex").await.unwrap().next_probe_after > Utc::now());
}

#[tokio::test]
async fn soft_rotation_probe_keeps_active_lease_and_releases_verification_lease_before_reselection() {
    let harness = quota_fixture_with_probe_candidate().await;

    let outcome = harness.trigger_soft_rotation_probe_success().await.unwrap();

    assert_eq!(outcome.original_lease_account_id, "acct-current");
    assert!(outcome.verification_lease_released);
    assert_eq!(outcome.final_selected_account_id, "acct-recovered");
}
```

- [ ] **Step 2: Run the backend integration tests to verify the local backend still uses position-only selection**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool lease_lifecycle -- --nocapture
```

Expected: FAIL because the local backend still acquires by legacy eligibility order and has no reprobe flow.

- [ ] **Step 3: Implement quota-aware candidate loading and verification-lease execution**

Implement:

- quota-aware candidate enumeration in `state/src/runtime/account_pool.rs`
- selection-family loading that prefers the requested family row and only falls back to `codex` when that row is absent
- `manager.rs` orchestration that keeps the current active lease held during `SoftRotation` reprobe, acquires a dedicated verification lease, releases that verification lease on every path, and reruns the original intent instead of promoting the probe lease
- atomic `next_probe_after` reservation handoff from state to the local backend
- verification-lease acquisition using a derived probe holder identity
- lease-scoped quota refresh plumbing in `backend/local/execution.rs`
- immediate selection restart when a normal ranked candidate or a probe lease loses a lease race

Do not let startup or lease acquisition paths keep consulting coarse `healthy` / `RateLimited` as quota vetoes.

- [ ] **Step 4: Re-run local backend tests**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool lease_lifecycle -- --nocapture
cargo test -p codex-state account_pool -- --nocapture
```

Expected: PASS for quota-aware lease lifecycle tests and no regressions in state-backed lease operations.

- [ ] **Step 5: Format, lint, and commit the backend slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-account-pool
just fix -p codex-state
git add state/src/runtime/account_pool.rs account-pool/src/manager.rs account-pool/src/backend/local/execution.rs account-pool/src/backend/local/mod.rs account-pool/tests/lease_lifecycle.rs
git commit -m "feat(account-pool): wire local quota-aware lease selection"
```

### Task 4: Integrate Live Quota Observation And Runtime Failover In `codex-core`

**Files:**
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/tests/suite/account_pool.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`

- [ ] **Step 1: Write failing runtime tests for family resolution, hard-failure writes, and early-reset reprobe**

Extend `codex-rs/core/tests/suite/account_pool.rs`:

```rust
#[tokio::test]
async fn hard_failover_uses_active_limit_family_before_falling_back_to_codex() {
    let harness = runtime_fixture_with_limit_family("chatgpt").await;
    harness.seed_quota("acct-a", "chatgpt", exhausted_primary()).await;
    harness.seed_quota("acct-b", "chatgpt", healthy_primary(12.0)).await;

    let next = harness.trigger_usage_limit_reached().await.unwrap();

    assert_eq!(next.account_id, "acct-b");
}

#[tokio::test]
async fn successful_probe_clears_stale_secondary_block_before_retrying_original_intent() {
    let harness = runtime_fixture_with_early_reset_probe().await;

    let outcome = harness.trigger_soft_rotation_without_ordinary_candidates().await.unwrap();

    assert_eq!(outcome.selection_reason, "probeRecoveredThenReselected");
    assert_eq!(outcome.account_id, "acct-recovered");
}
```

- [ ] **Step 2: Run the runtime suite to verify `codex-core` still depends on legacy quota handling**

Run:

```bash
cd codex-rs
cargo test -p codex-core account_pool -- --nocapture
```

Expected: FAIL because live runtime paths still persist/read coarse rate-limited state and do not execute the new reprobe contract.

- [ ] **Step 3: Implement runtime family resolution and selector integration**

Implement in `core/src/state/service.rs`:

- resolve `selection_family` per attempt:
  - `Startup` -> `codex`
  - `SoftRotation` / `HardFailover` -> active family when known, otherwise `codex`
- write live rate-limit observations into `account_quota_state`
- map ambiguous `usage_limit_reached` without a known active family into the `codex` row with `exhausted_windows = unknown`
- route proactive switch and hard failover through the shared selector
- keep switch damping as a separate runtime-local concern
- rerun the original non-probe intent after a successful probe instead of directly promoting the verification lease
- append structured account-pool events for live quota observation, exhausted-window transitions, probe reservation outcomes, and probe recovery details so observability readers have the required `details_json`
- ensure fresh non-exhausted live observations immediately clear previously blocked quota rows instead of waiting for `predicted_blocked_until` to expire

- [ ] **Step 4: Re-run core runtime tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core account_pool -- --nocapture
```

Expected: PASS for family-aware failover, reprobe, and early-reset recovery coverage.

- [ ] **Step 5: Format, lint, and commit the core slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-core
git add core/src/state/service.rs core/tests/suite/account_pool.rs account-pool/src/backend.rs
git commit -m "feat(core): use quota-aware pooled selection"
```

### Task 5: Expand Observability And App-Server Protocol Surfaces

**Files:**
- Modify: `codex-rs/state/src/model/account_pool_observability.rs`
- Modify: `codex-rs/state/src/runtime/account_pool_observability.rs`
- Modify: `codex-rs/account-pool/src/observability.rs`
- Modify: `codex-rs/account-pool/src/observability/conversions.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server-protocol/tests/account_pool_observability.rs`
- Modify: `codex-rs/app-server/src/account_pool_api.rs`
- Modify: `codex-rs/app-server/src/account_pool_api/conversions.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_pool.rs`
- Modify: `codex-rs/app-server/README.md`
- Generated: `codex-rs/app-server-protocol/schema/json/**`
- Generated: `codex-rs/app-server-protocol/schema/typescript/**`

- [ ] **Step 1: Write failing protocol and app-server tests for additive `quotas` fields**

Extend `codex-rs/app-server/tests/suite/v2/account_pool.rs` and `codex-rs/app-server-protocol/tests/account_pool_observability.rs`:

```rust
#[test]
fn account_pool_account_response_serializes_sorted_quota_families() {
    let response = AccountPoolAccountResponse {
        account_id: "acct-a".into(),
        quota: Some(AccountPoolQuotaResponse {
            remaining_percent: Some(18.0),
            resets_at: Some(1_710_000_000),
            observed_at: 1_709_999_000,
        }),
        quotas: vec![
            quota_family("chatgpt", 72.0),
            quota_family("codex", 82.0),
        ],
        ..account_row_fixture()
    };

    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["quotas"][0]["limitId"], "chatgpt");
    assert!(value["quotas"][0]["primary"].is_object());
    assert!(value["quotas"][0]["secondary"].is_object());
    assert!(value["quotas"][0].get("exhaustedWindows").is_some());
    assert!(value["quotas"][0].get("predictedBlockedUntil").is_some());
    assert!(value["quotas"][0].get("nextProbeAfter").is_some());
    assert!(value["quotas"][0].get("observedAt").is_some());
    assert_eq!(value["quota"]["remainingPercent"], 18.0);
}

#[tokio::test]
async fn account_pool_read_returns_additive_quota_and_quotas_fields() {
    let response = call_account_pool_read().await;
    assert!(response["data"][0]["quotas"].is_array());
    assert!(response["data"][0].get("quota").is_some());
}
```

- [ ] **Step 2: Run protocol and app-server tests to verify the wire contract is still singular**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol account_pool_observability -- --nocapture
cargo test -p codex-app-server account_pool -- --nocapture
```

Expected: FAIL with missing typed `quotas` fields and missing conversion logic.

- [ ] **Step 3: Implement typed quota families while keeping additive compatibility**

Implement:

- quota-family read models in `state` / `account-pool` observability layers that carry the full typed family payload shape expected by the spec
- typed `AccountPoolQuotaFamilyResponse` payloads in `protocol/v2.rs`
- additive `quotas` on account responses while keeping the singular `quota`
- deterministic `limit_id` ascending ordering
- full typed family payloads for:
  - `primary`
  - `secondary`
  - `exhausted_windows`
  - `predicted_blocked_until`
  - `next_probe_after`
  - `observed_at`
- singular `quota` projected only from the `codex` row, or `null` if absent
- richer `details_json` payloads for probe/exhausted-window explanation without adding new top-level event enums in this slice
- event-write plumbing in the runtime/state paths that already append account-pool events, so quota/probe details are durably persisted rather than only projected at read time
- README examples that show both `quota` and `quotas`

- [ ] **Step 4: Regenerate schema fixtures and re-run protocol/app-server tests**

Run:

```bash
cd codex-rs
just write-app-server-schema
cargo test -p codex-state account_pool_observability -- --nocapture
cargo test -p codex-account-pool observability -- --nocapture
cargo test -p codex-app-server-protocol account_pool_observability -- --nocapture
cargo test -p codex-app-server-protocol schema_fixtures -- --nocapture
cargo test -p codex-app-server account_pool -- --nocapture
```

Expected: PASS for crate-local observability coverage, schema fixtures, protocol serialization, and app-server integration coverage.

- [ ] **Step 5: Format, lint, and commit the observability/API slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-app-server-protocol
just fix -p codex-app-server
just fix -p codex-state
just fix -p codex-account-pool
git add state/src/model/account_pool_observability.rs state/src/runtime/account_pool_observability.rs account-pool/src/observability.rs account-pool/src/observability/conversions.rs app-server-protocol/src/protocol/v2.rs app-server/tests/suite/v2/account_pool.rs app-server/src/account_pool_api.rs app-server/src/account_pool_api/conversions.rs app-server/README.md app-server-protocol/schema/json app-server-protocol/schema/typescript app-server-protocol/tests/account_pool_observability.rs
git commit -m "feat(app-server): expose quota-aware pool observability"
```

### Task 6: Update CLI And TUI Consumers

**Files:**
- Modify: `codex-rs/cli/src/accounts/output.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`
- Modify: `codex-rs/tui/src/status/account.rs`
- Modify: `codex-rs/tui/src/status/rate_limits.rs`
- Modify: `codex-rs/tui/src/status/card.rs`
- Modify: `codex-rs/tui/src/status/tests.rs`
- Modify: affected `codex-rs/tui/src/status/snapshots/*.snap`

- [ ] **Step 1: Write failing CLI/TUI tests for multi-family quota rendering and selection explanations**

Add focused assertions in `codex-rs/cli/tests/accounts.rs` and `codex-rs/tui/src/status/tests.rs`:

```rust
#[test]
fn accounts_status_prefers_quotas_output_over_singular_quota_when_present() {
    let rendered = render_account_row(account_fixture_with_quotas());
    assert!(rendered.contains("codex"));
    assert!(rendered.contains("chatgpt"));
    assert!(rendered.contains("secondary exhausted"));
}

#[test]
fn status_snapshot_explains_probe_throttle_without_reusing_next_eligible_copy() {
    let status = status_fixture_with_probe_blocked_account();
    let rendered = render_status_snapshot(status);
    assert!(rendered.contains("next probe"));
    assert!(!rendered.contains("next eligible"));
}
```

- [ ] **Step 2: Run CLI/TUI tests to verify consumers still assume the old quota model**

Run:

```bash
cd codex-rs
cargo test -p codex-cli accounts -- --nocapture
cargo test -p codex-tui status -- --nocapture
```

Expected: FAIL with missing `quotas` handling and outdated copy/snapshots.

- [ ] **Step 3: Implement the additive UI updates**

Implement:

- CLI output that renders `quotas` deterministically by family
- selection explanations that distinguish:
  - blocked by secondary window
  - blocked by probe throttle
  - fallback to `codex` family
- TUI status rendering that uses the richer quota model without reusing misleading `next eligible` copy for probe throttling
- snapshot updates for the intentional copy and layout changes

- [ ] **Step 4: Re-run CLI/TUI tests and snapshot checks**

Run:

```bash
cd codex-rs
cargo test -p codex-cli accounts -- --nocapture
cargo test -p codex-tui status -- --nocapture
cargo insta accept -p codex-tui
cargo insta pending-snapshots --manifest-path tui/Cargo.toml
```

Expected: PASS for CLI/TUI tests and `No pending snapshots` after accepting intentional snapshot updates.

- [ ] **Step 5: Format, lint, and commit the CLI/TUI slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
just fix -p codex-tui
git add cli/src/accounts/output.rs cli/tests/accounts.rs tui/src/status/account.rs tui/src/status/rate_limits.rs tui/src/status/card.rs tui/src/status/tests.rs tui/src/status/snapshots
git commit -m "feat(ui): render quota-aware account selection state"
```

### Task 7: Final Verification And Handoff

**Files:**
- Verify: `codex-rs/state/tests/account_pool_quota.rs`
- Verify: `codex-rs/account-pool/tests/quota_selection.rs`
- Verify: `codex-rs/account-pool/tests/lease_lifecycle.rs`
- Verify: `codex-rs/core/tests/suite/account_pool.rs`
- Verify: `codex-rs/app-server-protocol/tests/account_pool_observability.rs`
- Verify: `codex-rs/app-server/tests/suite/v2/account_pool.rs`
- Verify: `codex-rs/cli/tests/accounts.rs`
- Verify: `codex-rs/tui/src/status/tests.rs`
- Verify: `codex-rs/app-server/README.md`

- [ ] **Step 1: Run the full targeted verification matrix**

Run:

```bash
cd codex-rs
cargo test -p codex-state account_pool_quota -- --nocapture
cargo test -p codex-state account_pool -- --nocapture
cargo test -p codex-account-pool quota_selection -- --nocapture
cargo test -p codex-account-pool lease_lifecycle -- --nocapture
cargo test -p codex-state account_pool_observability -- --nocapture
cargo test -p codex-account-pool observability -- --nocapture
cargo test -p codex-core account_pool -- --nocapture
cargo test -p codex-app-server-protocol account_pool_observability -- --nocapture
cargo test -p codex-app-server-protocol schema_fixtures -- --nocapture
cargo test -p codex-app-server account_pool -- --nocapture
cargo test -p codex-cli accounts -- --nocapture
cargo test -p codex-tui status -- --nocapture
```

Expected: PASS across the full quota-aware selection slice.

- [ ] **Step 2: Run generated-artifact and hygiene checks**

Run:

```bash
cd codex-rs
just write-app-server-schema
git diff --check
cargo insta pending-snapshots --manifest-path tui/Cargo.toml
```

Expected: no generated-schema drift that was not intentionally committed, `git diff --check` clean, and `No pending snapshots`.

- [ ] **Step 3: Update plan/spec bookkeeping if execution uncovered necessary drift**

If implementation changed scope materially, update:

- `docs/superpowers/specs/2026-04-18-account-pool-quota-aware-selection-design.md`
- this plan document

Only record real deltas; do not rewrite approved design text for cosmetic reasons.

- [ ] **Step 4: Ask before running workspace-wide tests**

If all targeted checks pass and broader confidence is still needed, ask the user before running:

```bash
cd codex-rs
cargo test
```

or

```bash
cd codex-rs
just test
```

- [ ] **Step 5: Commit the final verification/bookkeeping follow-up**

If Step 3 changed any docs or bookkeeping files, run:

```bash
git add docs/superpowers/specs/2026-04-18-account-pool-quota-aware-selection-design.md docs/superpowers/plans/2026-04-18-account-pool-quota-aware-selection-implementation.md codex-rs/app-server/README.md
git commit -m "chore: record quota-aware selection verification"
```

Otherwise, skip this step instead of forcing an empty commit.
