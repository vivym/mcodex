# Single-Pool Startup Fallback Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make local single-pool homes start through pooled access without manual `-c accounts.default_pool=...`, and add a first-class CLI for persisting a local default pool.

**Architecture:** Extend the shared startup resolver in `codex-state`/`codex-account-pool` so CLI, local TUI startup, and future runtime lease code consume one startup status model. Keep this phase away from app-server v2 protocol shape changes and runtime-lease refactors so it can run in parallel with `runtime-lease-authority-for-subagents`.

**Tech Stack:** Rust, Tokio, SQLx SQLite state, `codex-state`, `codex-account-pool`, `codex-cli`, `codex-tui`, `clap`, `serde_json`, `insta`, `pretty_assertions`, @superpowers:test-driven-development.

---

## Coordination Boundaries

This phase is safe to develop in parallel with `docs/superpowers/specs/2026-04-18-runtime-lease-authority-for-subagents-design.md` if the worker stays inside these boundaries:

- Allowed: startup resolver, local startup-selection state, CLI commands/output, local TUI startup notice.
- Avoid: app-server v2 protocol fields, `AccountLeaseReadResponse`, `AccountLeaseUpdatedNotification`, and core runtime lease-acquisition refactors.
- Integration point: Phase 2 consumes the resolver and command helpers created here from app-server and runtime lease code.

Do not modify the existing untracked runtime lease plan file unless the user explicitly asks.

## File Structure

- Modify: `codex-rs/state/src/model/account_pool.rs`
  - Add startup availability, issue, candidate-pool, and resolution-source variants.
  - Keep legacy `AccountStartupEligibility::Suppressed` compiling for compatibility, but stop producing it from new resolver paths.
- Modify: `codex-rs/state/src/model/mod.rs`
  - Re-export the new startup model types from the private model module.
- Modify: `codex-rs/state/src/lib.rs`
  - Re-export the new startup model types for cross-crate consumers.
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
  - Add local visible-pool inventory/facts reads used by the backend-neutral resolver.
- Modify: `codex-rs/account-pool/src/backend.rs`
  - Add backend-facing startup inventory/facts methods.
- Add: `codex-rs/account-pool/src/startup_resolution.rs`
  - Backend-neutral resolver that consumes startup inventory, persisted selection, explicit sources, and selection facts.
- Modify: `codex-rs/account-pool/src/startup_status.rs`
  - Call the backend-neutral resolver and make pooled applicability derive from `startup_availability`.
- Add: `codex-rs/account-pool/src/startup_default.rs`
  - Shared local default-pool mutation helper used by CLI now and app-server in Phase 2.
- Modify: `codex-rs/account-pool/src/lib.rs`
  - Export the new helper types narrowly.
- Modify: `codex-rs/account-pool/src/manager.rs`
  - Use shared startup status in `resolve_pool_id` so single-pool fallback can acquire leases in current code before runtime-lease refactor lands.
- Modify: `codex-rs/cli/src/accounts/mod.rs`
  - Add `accounts pool default set|clear` parsing and dispatch.
- Add: `codex-rs/cli/src/accounts/default_pool.rs`
  - CLI-specific presentation around the shared default-pool mutation helper.
- Modify: `codex-rs/cli/src/accounts/registration.rs`
  - Fix plain `import-legacy` implicit target order.
- Modify: `codex-rs/cli/src/accounts/diagnostics.rs`
  - Read the new startup status and candidate-pool facts.
- Modify: `codex-rs/cli/src/accounts/output.rs`
  - Add top-level `startup` JSON object and text diagnostics for fallback/blocker states.
- Modify: `codex-rs/cli/tests/accounts.rs`
  - Add CLI command, status JSON, and import-legacy regression coverage.
- Modify: `codex-rs/tui/src/startup_access.rs`
  - Add local startup probe states for single-pool fallback and multi-pool/default blockers.
- Modify: `codex-rs/tui/src/onboarding/pooled_access_notice.rs`
  - Add a read-only default-selection notice kind.
- Modify: `codex-rs/tui/src/onboarding/onboarding_screen.rs`
  - Route the new notice kind through the existing onboarding shell.
- Update snapshots under: `codex-rs/tui/src/onboarding/snapshots/`

## Task 0: Preflight And Scope Guard

**Files:**
- Read: `docs/superpowers/specs/2026-04-19-single-pool-startup-fallback-and-default-pool-selection-design.md`
- Read: `docs/superpowers/specs/2026-04-18-runtime-lease-authority-for-subagents-design.md`

- [x] **Step 1: Confirm branch and unrelated files**

Run:

```bash
git status --short
git branch --show-current
```

Expected: current branch is the intended feature branch. Unrelated files, especially `docs/superpowers/plans/2026-04-19-runtime-lease-authority-for-subagents-implementation.md`, are not staged.

- [x] **Step 2: Locate current startup and CLI seams**

Run:

```bash
rg -n "AccountStartupStatus|AccountStartupEligibility|EffectivePoolResolutionSource|preview_account_startup_selection|read_shared_startup_status" codex-rs/state/src codex-rs/account-pool/src
rg -n "PoolSubcommand|ImportLegacy|Status|Resume|Switch" codex-rs/cli/src/accounts
rg -n "StartupProbe|PooledAccessNoticeKind|StartupPromptDecision" codex-rs/tui/src
```

Expected: current implementation still has `EffectivePoolResolutionSource::{Override, ConfigDefault, PersistedSelection, None}` and no `singleVisiblePool`.

## Task 1: Add Startup Resolution Model Types

**Files:**
- Modify: `codex-rs/state/src/model/account_pool.rs`
- Modify: `codex-rs/state/src/model/mod.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Test: `codex-rs/state/src/runtime/account_pool.rs`

- [x] **Step 1: Write failing model/resolution tests**

Add tests near the existing account-pool runtime tests:

```rust
#[tokio::test]
async fn startup_status_uses_single_visible_pool_when_no_default_exists() {
    let runtime = test_runtime().await;
    seed_registered_account(&runtime, "acct-1", "pool-main").await;

    let status = runtime
        .read_account_startup_status(None)
        .await
        .expect("read startup status");

    assert_eq!(
        status,
        AccountStartupStatus {
            preview: AccountStartupSelectionPreview {
                effective_pool_id: Some("pool-main".to_string()),
                preferred_account_id: None,
                suppressed: false,
                predicted_account_id: Some("acct-1".to_string()),
                eligibility: AccountStartupEligibility::AutomaticAccountSelected,
            },
            configured_default_pool_id: None,
            persisted_default_pool_id: None,
            effective_pool_resolution_source: EffectivePoolResolutionSource::SingleVisiblePool,
            startup_availability: AccountStartupAvailability::Available,
            startup_resolution_issue: None,
            candidate_pools: vec![AccountStartupCandidatePool {
                pool_id: "pool-main".to_string(),
                display_name: None,
                status: None,
            }],
        }
    );
}

#[tokio::test]
async fn startup_status_requires_default_when_multiple_pools_are_visible() {
    let runtime = test_runtime().await;
    seed_registered_account(&runtime, "acct-1", "pool-main").await;
    seed_registered_account(&runtime, "acct-2", "pool-other").await;

    let status = runtime
        .read_account_startup_status(None)
        .await
        .expect("read startup status");

    assert_eq!(status.preview.effective_pool_id, None);
    assert_eq!(
        status.startup_availability,
        AccountStartupAvailability::MultiplePoolsRequireDefault
    );
    assert_eq!(
        status.startup_resolution_issue.as_ref().map(|issue| issue.kind),
        Some(AccountStartupResolutionIssueKind::MultiplePoolsRequireDefault)
    );
    assert_eq!(
        status.preview.eligibility,
        AccountStartupEligibility::MissingPool
    );
}
```

- [x] **Step 2: Run focused state tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-state startup_status_uses_single_visible_pool_when_no_default_exists -- --nocapture
```

Expected: FAIL to compile because the new model fields and enum variants do not exist.

- [x] **Step 3: Add the new model fields and enums**

In `codex-rs/state/src/model/account_pool.rs`, extend the model:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectivePoolResolutionSource {
    Override,
    ConfigDefault,
    PersistedSelection,
    SingleVisiblePool,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStartupAvailability {
    Available,
    Suppressed,
    MultiplePoolsRequireDefault,
    InvalidExplicitDefault,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStartupResolutionIssueKind {
    MultiplePoolsRequireDefault,
    OverridePoolUnavailable,
    ConfigDefaultPoolUnavailable,
    PersistedDefaultPoolUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStartupResolutionIssueSource {
    Override,
    ConfigDefault,
    PersistedSelection,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStartupCandidatePool {
    pub pool_id: String,
    pub display_name: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStartupResolutionIssue {
    pub kind: AccountStartupResolutionIssueKind,
    pub source: AccountStartupResolutionIssueSource,
    pub pool_id: Option<String>,
    pub candidate_pool_count: Option<usize>,
    pub candidate_pools: Option<Vec<AccountStartupCandidatePool>>,
    pub message: Option<String>,
}
```

Add these fields to `AccountStartupStatus`:

```rust
pub startup_availability: AccountStartupAvailability,
pub startup_resolution_issue: Option<AccountStartupResolutionIssue>,
pub candidate_pools: Vec<AccountStartupCandidatePool>,
```

Also re-export all newly added public model types from:

- `codex-rs/state/src/model/mod.rs`
- `codex-rs/state/src/lib.rs`

Cross-crate consumers such as `codex-account-pool`, `codex-cli`, and Phase 2
app-server code must not need to reach into the private `model` module.

- [x] **Step 4: Update all current status constructors**

Run:

```bash
cd codex-rs
cargo test -p codex-state --no-run
```

Expected: FAIL at every `AccountStartupStatus` construction site. Update those constructors with conservative defaults:

```rust
startup_availability: AccountStartupAvailability::Unavailable,
startup_resolution_issue: None,
candidate_pools: Vec::new(),
```

- [x] **Step 5: Re-run model tests**

Run:

```bash
cd codex-rs
cargo test -p codex-state startup_status_ -- --nocapture
```

Expected: tests compile; new behavior still fails until resolver logic lands.

## Task 2: Implement Backend-Neutral Inventory And Shared Resolver Semantics

**Files:**
- Modify: `codex-rs/state/src/runtime/account_pool.rs`
- Add: `codex-rs/account-pool/src/startup_resolution.rs`
- Modify: `codex-rs/account-pool/src/startup_status.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`
- Modify: `codex-rs/account-pool/src/manager.rs`
- Modify: `codex-rs/cli/src/accounts/registration.rs`
- Test: `codex-rs/state/src/runtime/account_pool.rs`
- Test: `codex-rs/account-pool/src/startup_resolution.rs`
- Test: `codex-rs/account-pool/src/startup_status.rs`

- [x] **Step 1: Add failing resolver tests for invalid explicit defaults and suppression overlay**

Add tests:

```rust
#[tokio::test]
async fn invalid_config_default_does_not_fall_back_to_single_visible_pool() {
    let runtime = test_runtime().await;
    seed_registered_account(&runtime, "acct-1", "pool-main").await;

    let status = runtime
        .read_account_startup_status(Some("missing-pool"))
        .await
        .expect("read startup status");

    assert_eq!(status.preview.effective_pool_id, None);
    assert_eq!(
        status.startup_availability,
        AccountStartupAvailability::InvalidExplicitDefault
    );
    assert_eq!(
        status.startup_resolution_issue.as_ref().map(|issue| issue.kind),
        Some(AccountStartupResolutionIssueKind::ConfigDefaultPoolUnavailable)
    );
}

#[tokio::test]
async fn suppression_overlays_single_visible_pool_without_replacing_eligibility() {
    let runtime = test_runtime().await;
    seed_registered_account(&runtime, "acct-1", "pool-main").await;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: None,
            preferred_account_id: None,
            suppressed: true,
        })
        .await
        .expect("write selection");

    let status = runtime
        .read_account_startup_status(None)
        .await
        .expect("read startup status");

    assert_eq!(status.preview.effective_pool_id.as_deref(), Some("pool-main"));
    assert_eq!(status.startup_availability, AccountStartupAvailability::Suppressed);
    assert_eq!(
        status.preview.eligibility,
        AccountStartupEligibility::AutomaticAccountSelected
    );
}
```

Add an account-pool shared-status test for invalid process override:

```rust
#[tokio::test]
async fn invalid_override_reports_override_issue_not_config_issue() {
    let backend = FakeStartupBackend::with_visible_pools(["pool-main"]);

    let status = read_shared_startup_status(
        &backend,
        Some("pool-main"),
        Some("missing-override"),
    )
    .await
    .expect("read shared startup status");

    assert_eq!(
        status.startup.effective_pool_resolution_source,
        EffectivePoolResolutionSource::Override
    );
    assert_eq!(
        status
            .startup
            .startup_resolution_issue
            .as_ref()
            .map(|issue| issue.kind),
        Some(AccountStartupResolutionIssueKind::OverridePoolUnavailable)
    );
}
```

- [x] **Step 2: Run tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-state invalid_config_default_does_not_fall_back_to_single_visible_pool suppression_overlays_single_visible_pool_without_replacing_eligibility -- --nocapture
```

Expected: FAIL because current resolver treats missing defaults as normal effective pool ids, synthetic suppression, or config-default failures.

- [x] **Step 3: Add backend-neutral resolver input types**

Create `codex-rs/account-pool/src/startup_resolution.rs` and define resolver-facing shapes:

```rust
pub struct StartupResolutionInput {
    pub override_pool_id: Option<String>,
    pub configured_default_pool_id: Option<String>,
    pub persisted_default_pool_id: Option<String>,
    pub persisted_preferred_account_id: Option<String>,
    pub suppressed: bool,
    pub inventory: StartupPoolInventory,
}

pub struct StartupPoolInventory {
    pub candidates: Vec<StartupPoolCandidate>,
}

pub struct StartupPoolCandidate {
    pub pool_id: String,
    pub display_name: Option<String>,
    pub status: Option<String>,
}

pub struct StartupSelectionFacts {
    pub preferred_account_outcome: Option<StartupPreferredAccountOutcome>,
    pub predicted_account_id: Option<String>,
    pub any_eligible_account: bool,
}
```

Add a resolver function that gets account-level facts only after a pool is resolved:

```rust
pub async fn resolve_startup_status<F, Fut>(
    input: StartupResolutionInput,
    selection_facts_for_pool: F,
) -> anyhow::Result<AccountStartupStatus>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = anyhow::Result<StartupSelectionFacts>>;
```

The resolver must own default precedence, single-pool fallback, explicit-source
validation, suppression overlay, issue generation, and candidate-pool ordering.
It must not query SQLite directly.

- [x] **Step 4: Add local inventory and facts providers**

In `StateRuntime`, add local helpers that return the backend-neutral inventory
and facts. The local inventory returns distinct pool ids from
`account_pool_membership` rows joined to registered accounts. Disabled/unhealthy
accounts still count as visible.

Required behavior:

- sort by `pool_id` ascending for deterministic `candidate_pools`
- ignore config-only `accounts.pools` entries
- include pools even when all accounts are disabled/unhealthy

- [x] **Step 5: Refactor preview to evaluate account eligibility independent of suppression**

Keep the account-level selection logic that produces:

- `PreferredAccountSelected`
- `AutomaticAccountSelected`
- `PreferredAccountMissing`
- `PreferredAccountInOtherPool`
- `PreferredAccountDisabled`
- `PreferredAccountUnhealthy`
- `PreferredAccountBusy`
- `NoEligibleAccount`
- `MissingPool`

Do not produce `AccountStartupEligibility::Suppressed` from the new resolver path. Preserve the enum variant only for old conversion call sites until Phase 2 cleans projections.

- [x] **Step 6: Implement precedence and availability**

Implement this order:

1. explicit override when routed through shared status
2. configured default
3. persisted default
4. single visible pool
5. none

Explicit-source validation must produce the right issue kind:

- override missing -> `OverridePoolUnavailable`
- config default missing -> `ConfigDefaultPoolUnavailable`
- persisted default missing -> `PersistedDefaultPoolUnavailable`

An invalid higher-priority source must not fall back to a lower-priority single visible pool.

- [x] **Step 7: Update `read_shared_startup_status` pooled applicability and override mapping**

In `codex-rs/account-pool/src/startup_status.rs`, pass both
`configured_default_pool_id` and `explicit_override_pool_id` into the shared
resolver instead of flattening them with `.or(...)`.

Keep `pooled_applicable` conservative for the legacy pooled-mode gate:

```rust
pooled_applicable: startup.preview.effective_pool_id.is_some(),
```

Implementation note: a review pass found that deriving this from
`startup_availability != Unavailable` would make multi-pool/default-blocker states
look pooled-applicable to existing core and app-server gates. New CLI/TUI
surfaces should use `startup_availability` directly; the legacy boolean remains
true only when an effective pool exists.

Review follow-up: registration is a mutation path, so it must not require a
configured or persisted default pool to already be visible in registered
membership. `accounts add chatgpt` resolves its write target as explicit
override, configured default, persisted default, then resolved startup pool.
Only an explicit override with no existing durable default is persisted as the
first durable default; single-visible fallback remains read-only.

When `explicit_override_pool_id` is present:

- preserve `configured_default_pool_id`
- set source `Override`
- map invalid override to `OverridePoolUnavailable`
- leave config-default issue kinds only for invalid config defaults

- [x] **Step 8: Update `AccountPoolManager::resolve_pool_id`**

Replace direct `read_startup_selection().default_pool_id` fallback with shared startup status:

```rust
let status = self
    .backend
    .read_account_startup_status(self.config.default_pool_id.as_deref())
    .await?;
status
    .preview
    .effective_pool_id
    .context("account pool has no default pool configured")
```

Keep this small. The runtime-lease plan may later move this into `RuntimeLeaseAuthority`; this change makes current code consume Phase 1 semantics until then.

- [x] **Step 9: Run focused tests**

Run:

```bash
cd codex-rs
cargo test -p codex-state startup_status_ -- --nocapture
cargo test -p codex-account-pool startup_status -- --nocapture
```

Expected: PASS.

- [x] **Step 10: Commit**

```bash
git add codex-rs/state/src/model/account_pool.rs codex-rs/state/src/model/mod.rs codex-rs/state/src/lib.rs codex-rs/state/src/runtime/account_pool.rs codex-rs/account-pool/src/backend.rs codex-rs/account-pool/src/startup_resolution.rs codex-rs/account-pool/src/startup_status.rs codex-rs/account-pool/src/manager.rs
git commit -m "feat(accounts): resolve single visible startup pool"
```

## Task 3: Add Shared Default-Pool Mutation Helper

**Files:**
- Add: `codex-rs/account-pool/src/startup_default.rs`
- Modify: `codex-rs/account-pool/src/lib.rs`
- Test: `codex-rs/account-pool/src/startup_default.rs`

- [x] **Step 1: Write failing helper tests**

Create unit tests for the mutation matrix:

```rust
#[tokio::test]
async fn set_default_clears_preferred_when_state_backed_source_is_active() {
    let runtime = seeded_runtime_with_pools(["pool-main", "pool-other"]).await;
    runtime
        .write_account_startup_selection(AccountStartupSelectionUpdate {
            default_pool_id: Some("pool-main".to_string()),
            preferred_account_id: Some("acct-main".to_string()),
            suppressed: true,
        })
        .await
        .expect("write selection");

    let outcome = set_local_default_pool(
        &runtime,
        LocalDefaultPoolSetRequest {
            pool_id: "pool-other".to_string(),
            configured_default_pool_id: None,
        },
    )
    .await
    .expect("set default");

    assert_eq!(outcome.state_changed, true);
    assert_eq!(
        runtime.read_account_startup_selection().await.unwrap(),
        AccountStartupSelectionState {
            default_pool_id: Some("pool-other".to_string()),
            preferred_account_id: None,
            suppressed: true,
        }
    );
}
```

- [x] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool startup_default -- --nocapture
```

Expected: FAIL because `startup_default` does not exist.

- [x] **Step 3: Implement helper types**

Add:

```rust
pub struct LocalDefaultPoolSetRequest {
    pub pool_id: String,
    pub configured_default_pool_id: Option<String>,
}

pub struct LocalDefaultPoolClearRequest {
    pub configured_default_pool_id: Option<String>,
}

pub struct LocalDefaultPoolMutationOutcome {
    pub state_changed: bool,
    pub persisted_default_pool_id: Option<String>,
    pub effective_pool_still_config_controlled: bool,
    pub suppressed: bool,
    pub preferred_account_cleared: bool,
}
```

Use self-documenting constructors if the final API would otherwise require unclear booleans at call sites.

- [x] **Step 4: Implement `set_local_default_pool`**

Required rules:

- validate requested `pool_id` is visible in state inventory
- reject missing pool with a clear error
- write `default_pool_id = pool_id`
- preserve `suppressed`
- clear `preferred_account_id` only when no configured default exists
- same-pool set with no preferred reset is no-op
- same-pool set with preferred reset is state-changing

- [x] **Step 5: Implement `clear_local_default_pool`**

Required rules:

- clear persisted `default_pool_id`
- preserve `suppressed`
- clear `preferred_account_id` only when a persisted default existed and no configured default exists
- clearing absent persisted default succeeds as no-op

- [x] **Step 6: Run helper tests**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool startup_default -- --nocapture
```

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add codex-rs/account-pool/src/startup_default.rs codex-rs/account-pool/src/lib.rs
git commit -m "feat(accounts): add default pool mutation helper"
```

## Task 4: Add CLI `accounts pool default set|clear`

**Files:**
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Add: `codex-rs/cli/src/accounts/default_pool.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`

- [x] **Step 1: Write failing parse and behavior tests**

Add tests:

```rust
#[test]
fn accounts_pool_default_commands_parse() {
    let set = AccountsCommand::try_parse_from(["codex", "pool", "default", "set", "team-main"])
        .expect("default set parses");
    assert!(format!("{set:?}").contains("Default"));

    let clear = AccountsCommand::try_parse_from(["codex", "pool", "default", "clear"])
        .expect("default clear parses");
    assert!(format!("{clear:?}").contains("Clear"));
}

#[tokio::test]
async fn accounts_pool_default_set_persists_local_default_without_resuming() -> Result<()> {
    let codex_home = prepared_home_with_two_pools_and_no_config().await?;
    write_startup_selection(
        &codex_home,
        AccountStartupSelectionUpdate {
            default_pool_id: None,
            preferred_account_id: Some("acct-other".to_string()),
            suppressed: true,
        },
    )
    .await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "pool", "default", "set", "team-main"],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("default pool set: team-main"));
    assert!(output.stdout.contains("pooled startup remains paused"));
    assert_eq!(
        read_startup_selection(&codex_home).await?,
        AccountStartupSelectionState {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: true,
        }
    );
    Ok(())
}
```

- [x] **Step 2: Run focused CLI tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts accounts_pool_default -- --nocapture
```

Expected: FAIL because the subcommand does not exist.

- [x] **Step 3: Add clap command shape**

In `PoolSubcommand`:

```rust
Default(PoolDefaultCommand),
```

Add:

```rust
#[derive(Debug, Args)]
pub struct PoolDefaultCommand {
    #[command(subcommand)]
    pub subcommand: PoolDefaultSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum PoolDefaultSubcommand {
    Set(PoolDefaultSetCommand),
    Clear,
}

#[derive(Debug, Args)]
pub struct PoolDefaultSetCommand {
    #[arg(value_name = "POOL_ID")]
    pub pool_id: String,
}
```

- [x] **Step 4: Implement CLI presentation module**

Create `default_pool.rs` with functions:

```rust
pub(crate) async fn set_default_pool(
    runtime: &StateRuntime,
    config: &Config,
    account_pool_override_id: Option<&str>,
    pool_id: &str,
) -> anyhow::Result<()>;

pub(crate) async fn clear_default_pool(
    runtime: &StateRuntime,
    config: &Config,
    account_pool_override_id: Option<&str>,
) -> anyhow::Result<()>;
```

Rules:

- reject `account_pool_override_id.is_some()` before mutation
- call shared helper from `codex-account-pool`
- print config-controlled message when applicable
- print resume guidance if `suppressed` remains true
- no JSON output in this phase

- [x] **Step 5: Dispatch from `run_accounts_impl`**

Add a `PoolSubcommand::Default` match arm in the existing runtime-initialized branch.

- [x] **Step 6: Run CLI tests**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts accounts_pool_default -- --nocapture
```

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add codex-rs/cli/src/accounts/mod.rs codex-rs/cli/src/accounts/default_pool.rs codex-rs/cli/tests/accounts.rs
git commit -m "feat(cli): add default pool commands"
```

## Task 5: Fix Plain `import-legacy` Target Selection

**Files:**
- Modify: `codex-rs/cli/src/accounts/registration.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`
- Test support: existing helpers in `accounts.rs`

- [x] **Step 1: Add failing import target-order tests**

Add tests for:

- configured default used even when not yet visible
- persisted default used when no config default exists
- resolved single visible pool used when neither explicit default exists
- empty inventory falls back to `legacy-default`
- multiple visible pools with no default fails and asks for `--pool`

Example:

```rust
#[tokio::test]
async fn import_legacy_without_pool_requires_pool_when_multiple_visible_pools_have_no_default() -> Result<()> {
    let codex_home = prepared_home_with_two_pools_and_no_config().await?;

    let output = run_codex(&codex_home, &["accounts", "import-legacy"]).await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("pass --pool <POOL_ID>"));
    assert!(output.stderr.contains("multiple account pools are registered"));
    Ok(())
}
```

- [x] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts import_legacy_without_pool -- --nocapture
```

Expected: FAIL because current code still uses legacy/default behavior.

- [x] **Step 3: Implement target order**

In `registration.rs`, make plain `import-legacy` target resolution:

1. command `--pool`
2. top-level `--account-pool`
3. `accounts.default_pool`
4. persisted `default_pool_id`
5. current resolved effective pool if present
6. `legacy-default` only when visible inventory is empty
7. otherwise error

Keep explicit `--pool` and `--account-pool` behavior unchanged except for clearer diagnostics.

- [x] **Step 4: Preserve bootstrap persistence rule**

Only persist `default_pool_id` automatically when pre-command visible inventory was empty and no configured/persisted default existed.

- [x] **Step 5: Run import tests**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts import_legacy -- --nocapture
```

Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add codex-rs/cli/src/accounts/registration.rs codex-rs/cli/tests/accounts.rs
git commit -m "fix(cli): resolve legacy import pool target explicitly"
```

## Task 6: Surface Startup State In CLI Status

**Files:**
- Modify: `codex-rs/cli/src/accounts/diagnostics.rs`
- Modify: `codex-rs/cli/src/accounts/output.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`

- [x] **Step 1: Add failing status JSON tests**

Add test:

```rust
#[tokio::test]
async fn accounts_status_json_includes_startup_object_for_single_pool_fallback() -> Result<()> {
    let codex_home = prepared_single_pool_home_without_defaults().await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;

    assert!(output.success, "stderr: {}", output.stderr);
    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-main");
    assert_eq!(json["startup"]["effectivePoolId"], "team-main");
    assert_eq!(json["startup"]["effectivePoolResolutionSource"], "singleVisiblePool");
    assert_eq!(json["startup"]["startupAvailability"], "available");
    assert!(json["startup"]["startupResolutionIssue"].is_null());
    assert_eq!(json["startup"]["selectionEligibility"], "automaticAccountSelected");
    Ok(())
}
```

- [x] **Step 2: Run and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts accounts_status_json_includes_startup_object -- --nocapture
```

Expected: FAIL because the `startup` object is absent.

- [x] **Step 3: Add wire conversion helpers**

In `output.rs`, add private helpers for:

- `startup_availability_code`
- `startup_issue_json`
- `candidate_pool_json`
- `startup_status_json`

Keep existing top-level fields unchanged.

- [x] **Step 4: Add text diagnostics**

Text output should explicitly show:

- `single visible pool fallback`
- `multiple visible pools require a default`
- invalid config/persisted/override default
- suppressed with resolved pool
- no eligible account

- [x] **Step 5: Run CLI status tests**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts accounts_status -- --nocapture
cargo test -p codex-cli --test accounts_observability
```

Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add codex-rs/cli/src/accounts/diagnostics.rs codex-rs/cli/src/accounts/output.rs codex-rs/cli/tests/accounts.rs codex-rs/cli/tests/accounts_observability.rs
git commit -m "feat(cli): explain startup pool resolution"
```

## Task 7: Add Local TUI Default-Selection Notice

**Files:**
- Modify: `codex-rs/tui/src/startup_access.rs`
- Modify: `codex-rs/tui/src/onboarding/pooled_access_notice.rs`
- Modify: `codex-rs/tui/src/onboarding/onboarding_screen.rs`
- Update snapshots: `codex-rs/tui/src/onboarding/snapshots/*.snap`

- [ ] **Step 1: Add failing startup decision tests**

In `startup_access.rs` tests:

```rust
#[test]
fn startup_decision_uses_pool_default_notice_for_multi_pool_blocker() {
    let notice = StartupNoticeData {
        issue_kind: StartupNoticeIssueKind::MultiplePoolsRequireDefault,
        issue_source: StartupNoticeIssueSource::None,
        candidate_pool_ids: vec!["team-main".to_string(), "team-other".to_string()],
    };
    let decision = decide_startup_access(
        LoginStatus::NotAuthenticated,
        true,
        false,
        StartupProbe::PooledDefaultSelectionRequired {
            remote: false,
            notice: notice.clone(),
        },
    );

    assert_eq!(
        decision,
        StartupPromptDecision::PooledDefaultSelectionNotice(notice)
    );
}
```

- [ ] **Step 2: Add failing widget snapshot test**

In `pooled_access_notice.rs` tests:

```rust
#[test]
fn pooled_default_selection_notice_renders_snapshot() {
    let widget = PooledAccessNoticeWidget::default_pool_required(
        vec!["team-main".to_string(), "team-other".to_string()],
        /*animations_enabled*/ false,
    );
    assert_snapshot!("pooled_default_selection_notice", render_to_string(&widget));
}
```

- [ ] **Step 3: Run TUI tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-tui startup_decision_uses_pool_default_notice_for_multi_pool_blocker pooled_default_selection_notice_renders_snapshot -- --nocapture
```

Expected: FAIL because the new probe/notice does not exist.

- [ ] **Step 4: Implement new probe and decision**

Add:

```rust
StartupProbe::PooledDefaultSelectionRequired {
    remote: bool,
    notice: StartupNoticeData,
}
StartupProbe::PooledInvalidDefault {
    remote: bool,
    notice: StartupNoticeData,
}
StartupPromptDecision::PooledDefaultSelectionNotice(StartupNoticeData)
```

For local probing, use `startup_availability`:

- `available` -> `PooledAvailable`
- `suppressed` -> `PooledSuppressed`
- `multiplePoolsRequireDefault` or `invalidExplicitDefault` -> default-selection notice
- `unavailable` -> `Unavailable`

Do not let the notice-hidden preference suppress blocker notices.

- [ ] **Step 5: Implement notice kind**

Add `PooledAccessNoticeKind::DefaultPoolRequired` with:

- title: `Choose a default account pool`
- candidate pool ids when available
- source-specific body:
  - multiple pools with no default: name `mcodex accounts pool default set <POOL_ID>`
  - persisted default invalid: name `mcodex accounts pool default set <POOL_ID>` or `mcodex accounts pool default clear`
  - config default invalid: tell the user to fix or remove `accounts.default_pool`
  - override invalid: tell the user to correct the process-local override
- key behavior: `Enter` and `l` open the existing shared-login handoff, and there is no hide action

Keep this read-only; do not implement a picker.

- [ ] **Step 6: Update onboarding routing**

Route `StartupPromptDecision::PooledDefaultSelectionNotice(data)` through the
existing onboarding screen and pass `candidate_pool_ids` plus issue source into
the widget. Ensure the login handoff path remains available.

- [ ] **Step 7: Generate and review snapshots**

Run:

```bash
cd codex-rs
cargo test -p codex-tui pooled_default_selection_notice -- --nocapture
cargo insta pending-snapshots -p codex-tui
```

Review the new `.snap.new` file, then accept only if correct:

```bash
cargo insta accept -p codex-tui
```

- [ ] **Step 8: Run TUI package tests**

Run:

```bash
cd codex-rs
cargo test -p codex-tui
```

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add codex-rs/tui/src/startup_access.rs codex-rs/tui/src/onboarding/pooled_access_notice.rs codex-rs/tui/src/onboarding/onboarding_screen.rs codex-rs/tui/src/onboarding/snapshots
git commit -m "feat(tui): show default pool selection notice"
```

## Task 8: Final Verification

**Files:**
- All files touched in this plan.

- [ ] **Step 1: Run scoped tests**

Run:

```bash
cd codex-rs
cargo test -p codex-state startup_status_
cargo test -p codex-account-pool
cargo test -p codex-cli --test accounts
cargo test -p codex-cli --test accounts_observability
cargo test -p codex-tui
```

Expected: PASS.

- [ ] **Step 2: Format and fix**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-state
just fix -p codex-account-pool
just fix -p codex-cli
just fix -p codex-tui
```

Expected: PASS. Do not rerun tests after `fmt`/`fix` unless a command changed behavior manually.

- [ ] **Step 3: Manual local smoke**

Use a temp home with one registered pool and no defaults:

```bash
MCODEX_HOME=/tmp/mcodex-single-pool-smoke target/debug/codex accounts status --json
MCODEX_HOME=/tmp/mcodex-single-pool-smoke target/debug/codex accounts pool default set main-pool
MCODEX_HOME=/tmp/mcodex-single-pool-smoke target/debug/codex accounts status
```

Expected:

- single-pool home reports `singleVisiblePool`
- `default set` persists `persistedDefaultPoolId`
- no ChatGPT login prompt path is selected for local single-pool startup

- [ ] **Step 4: Final commit if needed**

```bash
git status --short
git add <only files from this plan>
git commit -m "test(accounts): cover single pool startup fallback"
```

Only create this commit if verification required additional test or doc edits.
