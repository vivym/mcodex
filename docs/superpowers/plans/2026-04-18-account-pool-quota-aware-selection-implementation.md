# Account Pool Quota-Aware Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the remaining quota-aware observability and UI work on top of
the already-landed quota-state, shared-selector, local-backend, and runtime
authority integration. Startup, proactive rotation, and hard failover already
choose accounts from durable per-account quota knowledge instead of legacy
coarse `RateLimited` health.

**Architecture:** The merged baseline already has a first-class
`account_quota_state` persistence layer in `codex-state`, shared selector logic
in `codex-account-pool`, local backend quota-aware lease acquisition, and
runtime integration through `RuntimeLeaseAuthority`. The remaining architecture
work is additive observability: project page-scoped, family-sorted `quotas`
beside the legacy singular `quota` field, then update CLI/TUI consumers to use
the richer account-row facts.

**Tech Stack:** Rust workspace crates (`codex-state`, `codex-account-pool`, `codex-core`, `codex-app-server-protocol`, `codex-app-server`, `codex-cli`, `codex-tui`), SQLite via `sqlx`, app-server v2 schema generation, `pretty_assertions`, and `insta` snapshot coverage for TUI.

---

## Scope

Already landed baseline:

- add durable `account_quota_state` storage keyed by `account_id + limit_id`
- add shared quota-domain and selection-policy types in `codex-account-pool`
- define explicit selector intents for `Startup`, `SoftRotation`, `HardFailover`, and `ProbeRecovery`
- stop treating coarse `RateLimited` / `healthy` state as selector-facing quota truth
- persist and read quota state by `selection_family + codex fallback`
- add coordinated `next_probe_after` reservation and verification-lease reprobe wiring
- route runtime live rate-limit observations and hard usage-limit failures into the new quota store through `RuntimeLeaseAuthority`

Remaining in scope:

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

## Current Execution Baseline

After syncing this branch with `main`:

- Tasks 1-3 are already implemented in the current codebase and are retained
  below only as historical red/green implementation records.
- Task 4 is superseded by the merged runtime lease authority plan.
- The next executable implementation task is Task 5.
- Do not re-run the Task 1-4 implementation steps unless a regression is found;
  use Task 7's verification matrix to validate those baseline slices.

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
- Runtime quota integration in `codex-rs/core/src/state/service.rs`,
  `codex-rs/core/src/runtime_lease/**`, and
  `codex-rs/core/tests/suite/account_pool.rs` is now provided by the merged
  runtime authority plan. Do not reimplement the original Task 4 per-session
  integration path.
- Modify `codex-rs/state/src/model/account_pool_observability.rs`, `codex-rs/state/src/model/mod.rs`, `codex-rs/state/src/lib.rs`, `codex-rs/state/src/runtime/account_pool_observability.rs`, `codex-rs/account-pool/src/observability.rs`, `codex-rs/account-pool/src/observability/conversions.rs`, `codex-rs/account-pool/src/backend.rs`, `codex-rs/account-pool/src/lib.rs`, and `codex-rs/account-pool/tests/observability.rs` to surface typed `quotas` while keeping additive compatibility fields.
- Modify `codex-rs/app-server-protocol/src/protocol/v2.rs`, `codex-rs/app-server/src/account_pool_api.rs`, and `codex-rs/app-server/src/account_pool_api/conversions.rs` to publish the richer account-pool quota contract on `accountPool/accounts/list`.
- Modify `codex-rs/app-server/tests/suite/v2/account_pool.rs` and `codex-rs/app-server/README.md` to lock and document the additive API behavior.
- Modify existing CLI observability files
  `codex-rs/cli/src/accounts/observability.rs`,
  `codex-rs/cli/src/accounts/observability_types.rs`,
  `codex-rs/cli/src/accounts/observability_output.rs`,
  `codex-rs/cli/src/accounts/output.rs`, and
  `codex-rs/cli/tests/accounts_observability.rs` to print the new multi-family
  quota facts and selection explanations deterministically.
- Modify `codex-rs/tui/src/app_server_session.rs`, `codex-rs/tui/src/status/account.rs`, `codex-rs/tui/src/status/rate_limits.rs`, `codex-rs/tui/src/status/card.rs`, `codex-rs/tui/src/status/tests.rs`, and affected snapshots so runtime status reflects the richer quota model.

### Task 1: Add Durable Quota-State Storage In `codex-state`

**Status:** Completed in the current `main` baseline. Do not execute this task
again; keep the steps below as historical implementation trace.

**Files:**
- Create: `codex-rs/state/migrations/0031_account_pool_quota_state.sql`
- Create: `codex-rs/state/src/model/account_pool_quota.rs`
- Create: `codex-rs/state/src/runtime/account_pool_quota.rs`
- Create: `codex-rs/state/tests/account_pool_quota.rs`
- Modify: `codex-rs/state/src/model/mod.rs`
- Modify: `codex-rs/state/src/runtime.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Modify: `codex-rs/state/src/model/account_pool.rs`

- [x] **Step 1: Write failing persistence tests**

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

- [x] **Step 2: Run the new state tests to verify the quota store does not exist yet**

Run:

```bash
cd codex-rs
cargo test -p codex-state account_pool_quota -- --nocapture
```

Expected: FAIL with missing migration, missing quota-state types, and missing runtime APIs.

- [x] **Step 3: Implement the migration, model types, and runtime helpers**

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

- [x] **Step 4: Re-run the state tests and the existing state account-pool suite**

Run:

```bash
cd codex-rs
cargo test -p codex-state account_pool_quota -- --nocapture
cargo test -p codex-state account_pool -- --nocapture
```

Expected: PASS for the new quota-state tests and no regressions in the existing account-pool slice.

- [x] **Step 5: Format, lint, and commit the state slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-state
git add state/migrations/0031_account_pool_quota_state.sql state/src/model/account_pool_quota.rs state/src/runtime/account_pool_quota.rs state/src/model/mod.rs state/src/runtime.rs state/src/lib.rs state/src/model/account_pool.rs state/tests/account_pool_quota.rs
git commit -m "feat(state): add account pool quota state"
```

### Task 2: Build The Shared Quota-Aware Selector In `codex-account-pool`

**Status:** Completed in the current `main` baseline. Do not execute this task
again; keep the steps below as historical implementation trace.

**Files:**
- Create: `codex-rs/account-pool/src/quota.rs`
- Create: `codex-rs/account-pool/src/quota_selection.rs`
- Create: `codex-rs/account-pool/tests/quota_selection.rs`
- Modify: `codex-rs/account-pool/src/policy.rs`
- Modify: `codex-rs/account-pool/src/types.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`
- Modify: `codex-rs/account-pool/src/lib.rs`
- Modify: `codex-rs/account-pool/tests/policy.rs`

- [x] **Step 1: Write failing policy tests for veto, ranking, and reprobe intent rules**

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

- [x] **Step 2: Run the new selector tests to verify the shared engine does not exist yet**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool quota_selection -- --nocapture
```

Expected: FAIL with missing quota-domain structs, selector types, and terminal actions.

- [x] **Step 3: Implement the quota-domain types and shared selection engine**

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

- [x] **Step 4: Re-run selector tests and existing startup-policy coverage**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool quota_selection -- --nocapture
cargo test -p codex-account-pool policy -- --nocapture
```

Expected: PASS for the new shared selector tests and the legacy startup wrapper coverage.

- [x] **Step 5: Format, lint, and commit the selector slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-account-pool
git add account-pool/src/quota.rs account-pool/src/quota_selection.rs account-pool/src/policy.rs account-pool/src/types.rs account-pool/src/backend.rs account-pool/src/lib.rs account-pool/tests/quota_selection.rs account-pool/tests/policy.rs
git commit -m "feat(account-pool): add quota-aware selector"
```

### Task 3: Wire Quota-Aware Candidate Fetching And Reprobe Into The Local Backend

**Status:** Completed in the current `main` baseline. Do not execute this task
again; keep the steps below as historical implementation trace.

**Files:**
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Modify: `codex-rs/account-pool/src/manager.rs`
- Modify: `codex-rs/account-pool/src/backend/local/execution.rs`
- Modify: `codex-rs/account-pool/src/backend/local/mod.rs`
- Modify: `codex-rs/account-pool/tests/lease_lifecycle.rs`

- [x] **Step 1: Write failing backend/runtime tests for selection-family fetches and probe reservation flow**

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

- [x] **Step 2: Run the backend integration tests to verify the local backend still uses position-only selection**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool lease_lifecycle -- --nocapture
```

Expected: FAIL because the local backend still acquires by legacy eligibility order and has no reprobe flow.

- [x] **Step 3: Implement quota-aware candidate loading and verification-lease execution**

Implement:

- quota-aware candidate enumeration in `state/src/runtime/account_pool.rs`
- selection-family loading that prefers the requested family row and only falls back to `codex` when that row is absent
- `manager.rs` orchestration that keeps the current active lease held during `SoftRotation` reprobe, acquires a dedicated verification lease, releases that verification lease on every path, and reruns the original intent instead of promoting the probe lease
- atomic `next_probe_after` reservation handoff from state to the local backend
- verification-lease acquisition using a derived probe holder identity
- lease-scoped quota refresh plumbing in `backend/local/execution.rs`
- immediate selection restart when a normal ranked candidate or a probe lease loses a lease race

Do not let startup or lease acquisition paths keep consulting coarse `healthy` / `RateLimited` as quota vetoes.

- [x] **Step 4: Re-run local backend tests**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool lease_lifecycle -- --nocapture
cargo test -p codex-state account_pool -- --nocapture
```

Expected: PASS for quota-aware lease lifecycle tests and no regressions in state-backed lease operations.

- [x] **Step 5: Format, lint, and commit the backend slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-account-pool
just fix -p codex-state
git add state/src/runtime/account_pool.rs account-pool/src/manager.rs account-pool/src/backend/local/execution.rs account-pool/src/backend/local/mod.rs account-pool/tests/lease_lifecycle.rs
git commit -m "feat(account-pool): wire local quota-aware lease selection"
```

### Task 4: Superseded By RuntimeLeaseAuthority Integration

**Status:** Superseded and completed by
`docs/superpowers/plans/2026-04-19-runtime-lease-authority-for-subagents-implementation.md`
after that branch merged to `main`.

Do not execute the original per-session `codex-core` implementation steps from
this plan. The runtime authority branch intentionally replaced this task so
quota-aware selection is integrated through `RuntimeLeaseAuthority` and
`RuntimeLeaseHost`, not through a parallel session-local failover path.

The replacement implementation already establishes these invariants:

- pooled runtimes share one `RuntimeLeaseHost` across the top-level thread and
  child subagents
- every pooled provider request is admitted with a `LeaseSnapshot`
- live rate-limit reports write quota state using the admitted snapshot's
  `selection_family`
- `usage_limit_reached` reports close the current generation, let admitted work
  drain, and rotate only through the runtime authority
- app-server `accountLease/read` and `accountLease/updated` report live runtime
  host state after per-session pooled managers are removed

Remaining work in this quota-aware plan should treat these as baseline behavior.
Future changes must preserve the runtime authority boundary and should add
regression coverage there rather than reintroducing the original Task 4
session-local integration.

- [x] **Step 1: Replace the original `codex-core` runtime integration task**

The original tests:

- `hard_failover_uses_active_limit_family_before_falling_back_to_codex`
- `successful_probe_clears_stale_secondary_block_before_retrying_original_intent`

are covered unevenly in the merged baseline: hard failover has direct
runtime-authority coverage, while successful probe/reselection is covered in
the account-pool manager/backend tests. The remaining verification matrix in
Task 7 should run both sets as regressions. If implementation changes probe
handoff behavior, add an explicit host-backed runtime-authority probe regression
before final handoff.

- [x] **Step 2: Continue with observability and UI work on top of main**

After syncing with `main`, the next executable quota-aware task is Task 5.
Task 5 should add typed multi-family quota observability fields to the existing
state/account-pool/app-server protocol surfaces. Task 6 should update existing
CLI observability and TUI consumers to render those fields.

### Task 5: Expand Observability And App-Server Protocol Surfaces For Multi-Family Quota Facts

**Current baseline after syncing with `main`:**

- runtime quota writes already flow through `RuntimeLeaseAuthority`
- app-server account-pool responses still expose only the legacy singular
  `quota` field on each account row
- local CLI observability commands already exist, but they consume the same
  singular quota projection
- this task must be additive and must preserve the existing singular `quota`
  field for compatibility

**Files:**
- Modify: `codex-rs/state/src/model/account_pool_observability.rs`
- Modify: `codex-rs/state/src/model/mod.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Modify: `codex-rs/state/src/runtime/account_pool_observability.rs`
- Modify: `codex-rs/account-pool/src/observability.rs`
- Modify: `codex-rs/account-pool/src/observability/conversions.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`
- Modify: `codex-rs/account-pool/src/lib.rs`
- Modify: `codex-rs/account-pool/tests/observability.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server-protocol/tests/account_pool_observability.rs`
- Modify: `codex-rs/app-server/src/account_pool_api.rs`
- Modify: `codex-rs/app-server/src/account_pool_api/conversions.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_pool.rs`
- Modify: `codex-rs/app-server/README.md`
- Generated: `codex-rs/app-server-protocol/schema/json/**`
- Generated: `codex-rs/app-server-protocol/schema/typescript/**`

- [ ] **Step 1: Write failing protocol and app-server tests for additive `quotas` fields**

Extend `codex-rs/account-pool/tests/observability.rs`,
`codex-rs/app-server/tests/suite/v2/account_pool.rs`, and
`codex-rs/app-server-protocol/tests/account_pool_observability.rs`.

Account-pool crate coverage should assert that
`LocalAccountPoolBackend::list_accounts` preserves the same `quotas` vector and
singular `quota` compatibility projection returned by the state runtime after
quota rows are attached.

Protocol serialization coverage should validate the typed quota-family payload
shape without pretending that serde itself sorts the vector:

```rust
#[test]
fn account_pool_account_response_serializes_quota_families() {
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
```

Also extend schema coverage so `AccountPoolAccountResponse.quotas` is a
required, non-null array field and the new accounts-list request filter is
optional/nullable on the TypeScript side.

State/app-server integration coverage must prove account-row reads expose
sorted multi-family quota rows through `accountPool/accounts/list`, not
`accountPool/read`:

```rust
#[tokio::test]
async fn account_pool_accounts_list_returns_additive_quota_and_quotas_fields() {
    let response = call_account_pool_accounts_list_with_quota_rows_inserted_as(
        ["codex", "chatgpt"],
    )
    .await;

    assert!(response["data"][0]["quotas"].is_array());
    assert_eq!(response["data"][0]["quotas"][0]["limitId"], "chatgpt");
    assert_eq!(response["data"][0]["quotas"][1]["limitId"], "codex");
    assert!(response["data"][0].get("quota").is_some());
}
```

Add point-lookup coverage for the new `accountId` filter:

```rust
#[tokio::test]
async fn account_pool_accounts_list_account_id_filter_returns_single_row_without_cursor() {
    let response = call_account_pool_accounts_list_filtered_by_account_id("acct-b").await;

    assert_eq!(response["data"].as_array().unwrap().len(), 1);
    assert_eq!(response["data"][0]["accountId"], "acct-b");
    assert!(response["nextCursor"].is_null());
}
```

- [ ] **Step 2: Run protocol and app-server tests to verify the wire contract is still singular**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol --test account_pool_observability -- --nocapture
cargo test -p codex-app-server account_pool -- --nocapture
```

Expected: FAIL with missing typed `quotas` fields and missing conversion logic.

- [ ] **Step 3: Implement typed quota families while keeping additive compatibility**

Implement:

- quota-family read models in `state` / `account-pool` observability layers that carry the full typed family payload shape expected by the spec
- typed `AccountPoolQuotaFamilyResponse` payloads in `protocol/v2.rs`
- additive `quotas: Vec<AccountPoolQuotaFamilyResponse>` on account responses
  while keeping the singular `quota`; `quotas` is always serialized as a
  required non-null array and is empty when no quota rows exist
- optional `account_id` filtering on `AccountPoolAccountsListParams`, annotated
  with `#[ts(optional = nullable)]` and exposed on the wire as `accountId`, so
  active-account consumers such as the TUI can hydrate exactly the currently
  leased account row without scanning a full pool page
- implement `account_id` as a point lookup scoped by `pool_id`: apply it before
  cursor/limit pagination, return at most one account row, and return
  `next_cursor = None` for that point-lookup response
- thread that filter through the protocol params, app-server handler/conversions, account-pool observability query structs, and state SQL query before adding TUI hydration that depends on it
- deterministic `limit_id` ascending ordering
- full typed family payloads for:
  - `primary`
  - `secondary`
  - `exhausted_windows`
  - `predicted_blocked_until`
  - `next_probe_after`
  - `observed_at`
- singular `quota` projected only from the `codex` row, or `null` if absent
- legacy `quota` collapse from a `codex` row:
  - choose the populated quota window with the lowest remaining percent
  - `remaining_percent = 100.0 - used_percent`, clamped to `0..=100`
  - `resets_at` comes from the chosen window's reset timestamp
  - if no window has `used_percent`, keep `remaining_percent = null` and use the most specific available reset timestamp only when it belongs to an exhausted window
  - keep `observed_at` from the `codex` row
- preserve and, where needed, enrich existing `details_json` payloads for
  probe/exhausted-window explanation without adding new top-level event enums in
  this slice
- do not reimplement runtime event-write plumbing from Task 4; only update
  observability readers/conversions so persisted quota/probe details are
  surfaced durably instead of being projected ad hoc
- implement the state read without multiplying account rows:
  - apply pool/account filters before pagination
  - page the filtered account rows first
  - collect visible account ids from that page
  - load all `account_quota_state` rows for those ids ordered by `account_id, limit_id`
  - attach sorted `quotas` to each account row in memory
  - derive singular `quota` from the attached `codex` row
- README examples that show both `quota` and `quotas`

- [ ] **Step 4: Regenerate schema fixtures and re-run protocol/app-server tests**

Run:

```bash
cd codex-rs
just write-app-server-schema
cargo test -p codex-state account_pool_observability -- --nocapture
cargo test -p codex-account-pool observability -- --nocapture
cargo test -p codex-app-server-protocol --test account_pool_observability -- --nocapture
cargo test -p codex-app-server-protocol --test schema_fixtures -- --nocapture
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
git add state/src/model/account_pool_observability.rs state/src/model/mod.rs state/src/lib.rs state/src/runtime/account_pool_observability.rs account-pool/src/observability.rs account-pool/src/observability/conversions.rs account-pool/src/backend.rs account-pool/src/lib.rs account-pool/tests/observability.rs app-server-protocol/src/protocol/v2.rs app-server/tests/suite/v2/account_pool.rs app-server/src/account_pool_api.rs app-server/src/account_pool_api/conversions.rs app-server/README.md app-server-protocol/schema/json app-server-protocol/schema/typescript app-server-protocol/tests/account_pool_observability.rs
git commit -m "feat(app-server): expose quota-aware pool observability"
```

### Task 6: Update Existing CLI And TUI Consumers

**Files:**
- Modify: `codex-rs/cli/src/accounts/observability.rs`
- Modify: `codex-rs/cli/src/accounts/observability_types.rs`
- Modify: `codex-rs/cli/src/accounts/observability_output.rs`
- Modify: `codex-rs/cli/src/accounts/output.rs`
- Modify: `codex-rs/cli/tests/accounts_observability.rs`
- Modify: `codex-rs/tui/src/app_server_session.rs`
- Modify: `codex-rs/tui/src/status/account.rs`
- Modify: `codex-rs/tui/src/status/rate_limits.rs`
- Modify: `codex-rs/tui/src/status/card.rs`
- Modify: `codex-rs/tui/src/status/tests.rs`
- Modify: affected `codex-rs/tui/src/status/snapshots/*.snap`

- [ ] **Step 1: Write failing CLI/TUI tests for multi-family quota rendering and selection explanations**

The CLI observability command surface already exists on `main`; do not add a
second command layer. Add focused assertions in
`codex-rs/cli/tests/accounts_observability.rs` and
`codex-rs/tui/src/status/tests.rs`:

```rust
#[tokio::test]
async fn accounts_pool_show_renders_sorted_quota_families() -> Result<()> {
    let output = run_codex(
        &home_with_quota_rows_inserted_as(["codex", "chatgpt"]).await?,
        &["accounts", "pool", "show", "--pool", "team-main"],
    )
    .await?;

    assert!(output.stdout.contains("chatgpt"));
    assert!(output.stdout.contains("codex"));
    assert!(output.stdout.contains("secondary exhausted"));
    Ok(())
}

#[tokio::test]
async fn status_snapshot_explains_probe_throttle_without_reusing_next_eligible_copy() {
    let account_lease = test_probe_throttled_account_lease_display();
    let composite = new_status_output_with_account_lease(
        test_chatgpt_account_display().as_ref(),
        Some(&account_lease),
        /*rate_limits*/ None,
        /*refreshing_rate_limits*/ false,
    );

    assert_snapshot!(render_status_lines(composite));
}
```

The exact fixture/helper names may differ; use the existing
`run_codex`-based CLI integration helpers and TUI snapshot helpers instead of
introducing isolated unit-only helpers that bypass production mapping.

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

- existing CLI observability mapping in `observability.rs` so
  `AccountPoolAccount.quotas` reaches the CLI view model
- existing CLI observability view models and output that render `quotas`
  deterministically by family
- selection explanations that distinguish:
  - blocked by secondary window
  - blocked by probe throttle
  - fallback to `codex` family, only if Task 5 adds an explicit source or
    explanation field; otherwise render raw `quotas` and do not invent a
    fallback explanation
- TUI production plumbing in `app_server_session.rs`:
  - read the current lease through `accountLease/read`
  - when the response has both `pool_id` and `account_id`, hydrate the current
    account row through `accountPool/accounts/list` using the Task 5
    `account_id` filter
  - derive display-only quota/probe metadata from the hydrated `quotas` row
  - keep status rendering in `tui/src/status/*` focused on formatting the
    already-mapped display model
- TUI status rendering that uses the richer quota model without reusing
  misleading `Next eligible` copy for probe throttling
- snapshot updates for the intentional copy and layout changes

- [ ] **Step 4: Re-run CLI/TUI tests and snapshot checks**

Run:

```bash
cd codex-rs
cargo test -p codex-cli accounts -- --nocapture
cargo test -p codex-tui status -- --nocapture
cargo insta accept -p codex-tui
cargo insta pending-snapshots -p codex-tui
```

Expected: PASS for CLI/TUI tests and `No pending snapshots` after accepting intentional snapshot updates.

- [ ] **Step 5: Format, lint, and commit the CLI/TUI slice**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
just fix -p codex-tui
git add cli/src/accounts/observability.rs cli/src/accounts/observability_types.rs cli/src/accounts/observability_output.rs cli/src/accounts/output.rs cli/tests/accounts_observability.rs tui/src/app_server_session.rs tui/src/status/account.rs tui/src/status/rate_limits.rs tui/src/status/card.rs tui/src/status/tests.rs tui/src/status/snapshots
git commit -m "feat(ui): render quota-aware account selection state"
```

### Task 7: Final Verification And Handoff

**Files:**
- Verify: `codex-rs/state/tests/account_pool_quota.rs`
- Verify: `codex-rs/account-pool/tests/quota_selection.rs`
- Verify: `codex-rs/account-pool/tests/lease_lifecycle.rs`
- Verify: `codex-rs/account-pool/tests/observability.rs`
- Verify: `codex-rs/core/tests/suite/account_pool.rs`
- Verify: runtime-authority probe handoff coverage if Task 5/6 changes any probe or lease handoff behavior
- Verify: `codex-rs/app-server-protocol/tests/account_pool_observability.rs`
- Verify: `codex-rs/app-server/tests/suite/v2/account_pool.rs`
- Verify: `codex-rs/cli/tests/accounts_observability.rs`
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
cargo test -p codex-app-server-protocol --test account_pool_observability -- --nocapture
cargo test -p codex-app-server-protocol --test schema_fixtures -- --nocapture
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
git diff --exit-code -- app-server-protocol/schema/json app-server-protocol/schema/typescript
cargo insta pending-snapshots -p codex-tui
```

Expected: no generated-schema drift that was not intentionally committed,
`git diff --check` clean, generated schema fixtures clean, and
`No pending snapshots`.

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
