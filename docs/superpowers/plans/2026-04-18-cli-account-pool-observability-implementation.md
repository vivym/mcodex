# CLI Account Pool Observability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land a read-only pooled-account observability CLI surface that adds `accounts pool show`, `accounts diagnostics`, `accounts events`, and additive `accounts status` observability without changing pooled runtime contracts.

**Architecture:** Keep the current startup-selection/status path in `codex-rs/cli/src/accounts/diagnostics.rs` and `output.rs`, then add a separate CLI observability adapter that resolves the target pool and reads `LocalAccountPoolBackend` through `AccountPoolObservabilityReader`. Keep drill-down commands strict, keep `status` additive and partial on observability read failure, and isolate new formatting into focused modules so `mod.rs`, `output.rs`, and `cli/tests/accounts.rs` do not keep growing.

**Tech Stack:** Rust CLI (`clap`, `serde_json`, `chrono`), `codex-account-pool` backend-neutral observability seam, existing `StateRuntime`-seeded CLI integration tests with `assert_cmd` and temp `MCODEX_HOME`, plus `just fmt` / `just fix -p codex-cli`.

---

## Scope

In scope:

- add `codex accounts pool show`
- add `codex accounts diagnostics`
- add `codex accounts events`
- extend `codex accounts status` with additive pooled observability summary and `poolObservability` JSON
- resolve drill-down target pools from command `--pool`, top-level `--account-pool`, or effective startup pool
- preserve backend order, cursor semantics, nullable JSON, and RFC 3339 UTC timestamps from the approved spec
- keep `codex accounts pool list|assign` behavior unchanged
- document the new local operator commands in repo docs

Out of scope:

- state schema or pooled runtime behavior changes
- app-server protocol changes
- remote backend implementation
- TUI observability consumption
- write-side commands such as pause, resume, or drain
- workspace-wide `cargo test` without explicit user approval

## Execution Rules

- Use `@superpowers:test-driven-development` before each implementation task.
- Use `@superpowers:verification-before-completion` before claiming a task is done.
- Keep changes in `codex-rs/cli` plus repo docs; do not widen `codex-account-pool` or app-server contracts for this slice.
- Run the targeted test named in the task before writing implementation code.
- After the task-level targeted tests pass, run `cargo test -p codex-cli` before `just fmt` / `just fix -p codex-cli`.
- Run `just fmt` from `codex-rs/` after each Rust task.
- Run `just fix -p codex-cli` from `codex-rs/` after the task’s tests pass.
- Do not rerun tests after `just fmt` or `just fix -p codex-cli`.

## Planned File Layout

- Create `codex-rs/cli/src/accounts/observability.rs` for target-pool resolution, backend reader construction, strict vs partial read helpers, and command-level DTO assembly.
- Create `codex-rs/cli/src/accounts/observability_types.rs` for CLI-facing observability view structs and timestamp normalization helpers, so formatters do not operate directly on seam types.
- Create `codex-rs/cli/src/accounts/observability_output.rs` for text/json formatting of `pool show`, `diagnostics`, `events`, and the additive `status` pooled section, with local formatter-focused unit tests kept next to the rendering helpers.
- Modify `codex-rs/cli/src/accounts/mod.rs` to add new clap subcommands/args and dispatch into the observability helpers while preserving `pool list|assign`.
- Modify `codex-rs/cli/src/accounts/diagnostics.rs` only enough to expose the startup-status read needed by the new resolver and to carry additive status observability data.
- Modify `codex-rs/cli/src/accounts/output.rs` only enough to delegate pooled summary rendering/JSON assembly for `accounts status`; keep legacy top-level fields stable.
- Create `codex-rs/cli/tests/accounts_observability.rs` for new integration coverage rather than extending the existing 1200+ line `accounts.rs`.
- Modify `docs/getting-started.md` to advertise the new local operator commands with `mcodex` examples.

## Implementation Notes

- Reuse `read_accounts_startup_status(...)` for all default-pool fallback logic so the CLI stays aligned with the shared startup-status adapter added in `mcodex-pooled-startup-identity`.
- For strict drill-down commands, resolve `--pool` and top-level `--account-pool` first and only read startup status if both are absent. Do not make explicit drill-down requests depend on startup diagnostics when the pool target is already known.
- Do not replace the existing `AccountsStatusDiagnostic.pool` payload. The legacy top-level `healthState`, `switchReason`, `accounts`, and related JSON remain, and pooled observability is additive under `poolObservability`.
- Keep `status` partial on observability read failures by storing warning text in the diagnostic object instead of bailing the command.
- Keep `pool show`, `diagnostics`, and `events` strict. If their read path cannot resolve a pool or the backend read fails, return an error.
- Preserve seam ordering and cursor behavior exactly. The CLI must not sort accounts, issues, or events client-side.
- Use RFC 3339 UTC strings for every new CLI JSON timestamp field.

### Task 1: Add command grammar and target-pool resolution

**Files:**
- Create: `codex-rs/cli/src/accounts/observability.rs`
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Modify: `codex-rs/cli/src/accounts/diagnostics.rs`
- Test: `codex-rs/cli/tests/accounts_observability.rs`

- [ ] **Step 1: Write failing command and resolver tests**

Add parser-focused coverage in `codex-rs/cli/tests/accounts_observability.rs` so Task 1 only proves grammar and conflict handling, not the later runtime read path:

```rust
#[test]
fn accounts_pool_show_and_existing_pool_subcommands_parse() {
    use clap::Parser;
    use codex_cli::AccountsCommand;

    let list = AccountsCommand::try_parse_from(["codex", "pool", "list"]);
    assert!(list.is_ok());

    let show = AccountsCommand::try_parse_from(["codex", "pool", "show", "--json"]);
    assert!(show.is_ok());
}

#[tokio::test]
async fn accounts_diagnostics_rejects_conflicting_pool_flags() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &[
            "accounts",
            "--account-pool",
            "team-main",
            "diagnostics",
            "--pool",
            "team-other",
        ],
    )
    .await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("conflicts with --account-pool"));
    Ok(())
}
```

Add focused unit tests in the new `observability.rs` for the resolver:

```rust
#[tokio::test]
async fn resolve_target_pool_rejects_conflicting_command_and_override_pool_ids() {
    let err = resolve_target_pool(
        Some("team-command"),
        Some("team-override"),
        Some("team-effective"),
    )
    .expect_err("expected conflict");

    assert!(err.to_string().contains("conflicts with --account-pool"));
}
```

- [ ] **Step 2: Run the focused tests and confirm they fail**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability accounts_pool_show_and_existing_pool_subcommands_parse -- --nocapture
```

Expected: FAIL because the new `pool show` parser branch and explicit conflict handling do not exist yet.

- [ ] **Step 3: Implement clap subcommands and the shared resolver**

Add the new command structs and subcommands in `codex-rs/cli/src/accounts/mod.rs`:

```rust
#[derive(Debug, Args)]
pub struct PoolShowCommand {
    #[arg(long = "pool", value_name = "POOL_ID")]
    pub pool: Option<String>,
    #[arg(long = "limit")]
    pub limit: Option<u32>,
    #[arg(long = "cursor")]
    pub cursor: Option<String>,
    #[arg(long = "json", default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, clap::Subcommand)]
pub enum PoolSubcommand {
    List,
    Assign(PoolAssignCommand),
    Show(PoolShowCommand),
}

#[derive(Debug, clap::Subcommand)]
pub enum AccountsSubcommand {
    // existing variants...
    Diagnostics(AccountsDiagnosticsCommand),
    Events(AccountsEventsCommand),
}
```

In `codex-rs/cli/src/accounts/observability.rs`, add a small resolver API that all new commands use:

```rust
pub(crate) enum TargetPoolSource {
    CommandArg,
    TopLevelOverride,
    EffectivePool,
}

pub(crate) struct ResolvedTargetPool {
    pub pool_id: String,
    pub source: TargetPoolSource,
}

pub(crate) fn resolve_target_pool(
    command_pool: Option<&str>,
    top_level_override: Option<&str>,
    effective_pool_id: Option<&str>,
) -> anyhow::Result<ResolvedTargetPool> {
    if let (Some(command_pool), Some(top_level_override)) = (command_pool, top_level_override)
        && command_pool != top_level_override
    {
        anyhow::bail!(
            "--pool `{command_pool}` conflicts with --account-pool `{top_level_override}`"
        );
    }
    // choose command pool, else override, else effective pool
}
```

Expose the existing startup-status read from `diagnostics.rs` so the observability module can resolve the effective pool without duplicating config/state probing.

- [ ] **Step 4: Run the CLI crate tests for the grammar slice**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability
cargo test -p codex-cli
```

Expected: PASS for the parser/resolver slice, while the later `pool show` / `events` assertions can stay ignored or absent until their tasks land.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
git add cli/src/accounts/mod.rs cli/src/accounts/diagnostics.rs cli/src/accounts/observability.rs cli/tests/accounts_observability.rs
git commit -m "feat(cli): add pooled observability command grammar"
```

### Task 2: Implement `accounts pool show`

**Files:**
- Create: `codex-rs/cli/src/accounts/observability_types.rs`
- Create: `codex-rs/cli/src/accounts/observability_output.rs`
- Modify: `codex-rs/cli/src/accounts/observability.rs`
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Test: `codex-rs/cli/tests/accounts_observability.rs`
- Test: `codex-rs/cli/src/accounts/observability_output.rs`

- [ ] **Step 1: Write failing `pool show` tests**

Add integration tests that prove both default effective-pool fallback and stable JSON/text output:

```rust
#[tokio::test]
async fn accounts_pool_show_json_uses_effective_pool_and_preserves_nullable_fields() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "pool", "show", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["poolId"], "team-main");
    assert_eq!(json["summary"]["totalAccounts"], 2);
    assert!(json["refreshedAt"].is_string() || json["refreshedAt"].is_null());
    assert_eq!(json["data"].as_array().expect("data").len(), 2);
    assert!(json["data"][0]["quota"].is_null());
    assert!(json["nextCursor"].is_null());
    Ok(())
}

#[tokio::test]
async fn accounts_pool_show_command_pool_overrides_current_effective_pool() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "pool", "show", "--pool", "team-other", "--json"],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["poolId"], "team-other");
    Ok(())
}

#[tokio::test]
async fn accounts_pool_show_text_reports_accounts_none_for_empty_pool() -> Result<()> {
    let codex_home = prepared_empty_pool_home().await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "pool", "show", "--pool", "team-empty"],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("accounts: none"));
    Ok(())
}
```

- [ ] **Step 2: Run the focused `pool show` tests and confirm failure**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability accounts_pool_show_json_uses_effective_pool_and_preserves_nullable_fields -- --nocapture
```

Expected: FAIL because `pool show` does not exist and no output mapping is implemented.

- [ ] **Step 3: Implement the strict read path and output mapping**

Add CLI-specific view structs in `codex-rs/cli/src/accounts/observability_types.rs`:

```rust
pub(crate) struct PoolShowView {
    pub pool_id: String,
    pub refreshed_at: Option<String>,
    pub summary: PoolSummaryView,
    pub data: Vec<PoolAccountView>,
    pub next_cursor: Option<String>,
}

pub(crate) struct PoolAccountView {
    pub account_id: String,
    pub backend_account_ref: Option<String>,
    pub account_kind: String,
    pub enabled: bool,
    pub health_state: Option<String>,
    pub operational_state: Option<String>,
    pub allocatable: Option<bool>,
    pub status_reason_code: Option<String>,
    pub status_message: Option<String>,
    pub current_lease: Option<PoolLeaseView>,
    pub quota: Option<PoolQuotaView>,
    pub selection: Option<PoolSelectionView>,
    pub updated_at: Option<String>,
}
```

Implement `read_pool_show(...)` in `codex-rs/cli/src/accounts/observability.rs`:

```rust
pub(crate) async fn read_pool_show(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    top_level_override: Option<&str>,
    command: &PoolShowCommand,
) -> anyhow::Result<PoolShowView> {
    let target = resolve_strict_target_pool(
        runtime,
        config,
        top_level_override,
        command.pool.as_deref(),
    )
    .await?;
    let reader = local_observability_reader(runtime, config);
    let snapshot = reader.read_pool(AccountPoolReadRequest { pool_id: target.pool_id.clone() }).await?;
    let page = reader
        .list_accounts(AccountPoolAccountsListRequest {
            pool_id: target.pool_id,
            cursor: command.cursor.clone(),
            limit: command.limit,
            ..Default::default()
        })
        .await?;
    map_pool_show(snapshot, page)
}
```

Add formatter unit tests in `codex-rs/cli/src/accounts/observability_output.rs` for `leaseId@holderInstanceId`, `accounts: none`, `next cursor: ...`, and JSON null-preservation. Then format text/json there without inventing missing timestamp values.

- [ ] **Step 4: Run `pool show` tests and the full CLI crate tests**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability accounts_pool_show_json_uses_effective_pool_and_preserves_nullable_fields -- --nocapture
cargo test -p codex-cli --test accounts_observability accounts_pool_show_command_pool_overrides_current_effective_pool -- --nocapture
cargo test -p codex-cli --test accounts_observability accounts_pool_show_text_reports_accounts_none_for_empty_pool -- --nocapture
cargo test -p codex-cli
```

Expected: PASS. `pool show` succeeds, keeps nullable JSON fields, and leaves `pool list|assign` untouched.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
git add cli/src/accounts/mod.rs cli/src/accounts/observability.rs cli/src/accounts/observability_types.rs cli/src/accounts/observability_output.rs cli/tests/accounts_observability.rs
git commit -m "feat(cli): add account pool show"
```

### Task 3: Implement `accounts diagnostics`

**Files:**
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Modify: `codex-rs/cli/src/accounts/observability.rs`
- Modify: `codex-rs/cli/src/accounts/observability_types.rs`
- Modify: `codex-rs/cli/src/accounts/observability_output.rs`
- Test: `codex-rs/cli/tests/accounts_observability.rs`
- Test: `codex-rs/cli/src/accounts/observability_output.rs`

- [ ] **Step 1: Write failing diagnostics tests**

Add coverage for healthy empty issues and blocked/degraded output:

```rust
#[tokio::test]
async fn accounts_diagnostics_text_reports_issues_none_for_healthy_pool() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "diagnostics"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("status: healthy"));
    assert!(output.stdout.contains("issues: none"));
    Ok(())
}

#[tokio::test]
async fn accounts_diagnostics_json_reports_blocked_issue_details() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_busy_and_unhealthy_pool_state(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "diagnostics", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["poolId"], "team-main");
    assert!(json["generatedAt"].as_str().is_some());
    assert_eq!(json["issues"].as_array().expect("issues").len(), 2);
    Ok(())
}

#[tokio::test]
async fn accounts_diagnostics_requires_pool_when_no_effective_pool_resolves() -> Result<()> {
    let codex_home = prepared_no_pool_home().await?;

    let output = run_codex(&codex_home, &["accounts", "diagnostics"]).await?;
    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("pass --pool <POOL_ID>"));
    Ok(())
}
```

- [ ] **Step 2: Run the focused diagnostics tests and confirm failure**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability accounts_diagnostics_text_reports_issues_none_for_healthy_pool -- --nocapture
```

Expected: FAIL because `accounts diagnostics` does not exist yet.

- [ ] **Step 3: Implement diagnostics read and formatting**

Add a view model and mapper:

```rust
pub(crate) struct DiagnosticsView {
    pub pool_id: String,
    pub generated_at: Option<String>,
    pub status: String,
    pub issues: Vec<DiagnosticsIssueView>,
}

pub(crate) struct DiagnosticsIssueView {
    pub severity: String,
    pub reason_code: String,
    pub message: String,
    pub account_id: Option<String>,
    pub holder_instance_id: Option<String>,
    pub next_relevant_at: Option<String>,
}
```

Implement a strict helper:

```rust
pub(crate) async fn read_pool_diagnostics(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    top_level_override: Option<&str>,
    command_pool: Option<&str>,
) -> anyhow::Result<DiagnosticsView> {
    let target = resolve_strict_target_pool(runtime, config, top_level_override, command_pool).await?;
    let diagnostics = local_observability_reader(runtime, config)
        .read_diagnostics(AccountPoolDiagnosticsReadRequest { pool_id: target.pool_id })
        .await?;
    map_diagnostics(diagnostics)
}
```

Add formatter unit tests for `issues: none` and stable issue-row text, then render text with backend order preserved.

- [ ] **Step 4: Run diagnostics tests and the full CLI crate tests**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability accounts_diagnostics_text_reports_issues_none_for_healthy_pool -- --nocapture
cargo test -p codex-cli --test accounts_observability accounts_diagnostics_json_reports_blocked_issue_details -- --nocapture
cargo test -p codex-cli --test accounts_observability accounts_diagnostics_requires_pool_when_no_effective_pool_resolves -- --nocapture
cargo test -p codex-cli
```

Expected: PASS. Diagnostics remains strict, uses seam ordering, and preserves nullable fields in JSON.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
git add cli/src/accounts/mod.rs cli/src/accounts/observability.rs cli/src/accounts/observability_types.rs cli/src/accounts/observability_output.rs cli/tests/accounts_observability.rs
git commit -m "feat(cli): add account diagnostics view"
```

### Task 4: Implement `accounts events`

**Files:**
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Modify: `codex-rs/cli/src/accounts/observability.rs`
- Modify: `codex-rs/cli/src/accounts/observability_types.rs`
- Modify: `codex-rs/cli/src/accounts/observability_output.rs`
- Test: `codex-rs/cli/tests/accounts_observability.rs`
- Test: `codex-rs/cli/src/accounts/observability_output.rs`

- [ ] **Step 1: Write failing events tests**

Cover cursor pagination, repeatable `--type`, and empty-state text:

```rust
#[tokio::test]
async fn accounts_events_json_preserves_cursor_and_details_payload() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_pool_events_with_array_details(&codex_home).await?;

    let output = run_codex(
        &codex_home,
        &[
            "accounts",
            "events",
            "--type",
            "leaseAcquired",
            "--type",
            "quotaObserved",
            "--limit",
            "1",
            "--json",
        ],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["data"].as_array().expect("data").len(), 1);
    assert!(json["nextCursor"].as_str().is_some());
    assert_eq!(json["data"][0]["details"], serde_json::json!(["soft-limit", 42]));
    Ok(())
}

#[tokio::test]
async fn accounts_events_text_reports_events_none_when_filter_matches_nothing() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "events", "--account", "missing-account"],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("events: none"));
    Ok(())
}

#[tokio::test]
async fn accounts_events_rejects_invalid_cursor() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &["accounts", "events", "--cursor", "not-a-valid-cursor"],
    )
    .await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("invalid"));
    Ok(())
}
```

- [ ] **Step 2: Run the focused events tests and confirm failure**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability accounts_events_json_preserves_cursor_and_details_payload -- --nocapture
```

Expected: FAIL because `accounts events` and the event-type argument mapping do not exist yet.

- [ ] **Step 3: Implement events args, mapper, and formatter**

Add a repeatable event-type argument that maps directly onto seam filters:

```rust
#[derive(Clone, Debug, clap::ValueEnum)]
#[value(rename_all = "camelCase")]
pub enum AccountPoolEventTypeArg {
    LeaseAcquired,
    LeaseRenewed,
    LeaseReleased,
    LeaseAcquireFailed,
    ProactiveSwitchSelected,
    ProactiveSwitchSuppressed,
    QuotaObserved,
    QuotaNearExhausted,
    QuotaExhausted,
    AccountPaused,
    AccountResumed,
    AccountDrainingStarted,
    AccountDrainingCleared,
    AuthFailed,
    CooldownStarted,
    CooldownCleared,
}
```

Build the strict read helper in `observability.rs`:

```rust
pub(crate) async fn read_pool_events(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    top_level_override: Option<&str>,
    command: &AccountsEventsCommand,
) -> anyhow::Result<EventsPageView> {
    let target = resolve_strict_target_pool(runtime, config, top_level_override, command.pool.as_deref()).await?;
    let page = local_observability_reader(runtime, config)
        .list_events(AccountPoolEventsListRequest {
            pool_id: target.pool_id,
            account_id: command.account.clone(),
            types: map_event_types(command.types.as_slice()),
            cursor: command.cursor.clone(),
            limit: command.limit,
        })
        .await?;
    map_events_page(page)
}
```

Add formatter unit tests for `events: none` and `next cursor: ...`, then render text rows newest-first without re-sorting and keep JSON `details` as raw `serde_json::Value`.

- [ ] **Step 4: Run events tests and the full CLI crate tests**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability accounts_events_json_preserves_cursor_and_details_payload -- --nocapture
cargo test -p codex-cli --test accounts_observability accounts_events_text_reports_events_none_when_filter_matches_nothing -- --nocapture
cargo test -p codex-cli --test accounts_observability accounts_events_rejects_invalid_cursor -- --nocapture
cargo test -p codex-cli
```

Expected: PASS. Events pagination, OR filtering, raw details JSON, and text empty states all match the spec.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
git add cli/src/accounts/mod.rs cli/src/accounts/observability.rs cli/src/accounts/observability_types.rs cli/src/accounts/observability_output.rs cli/tests/accounts_observability.rs
git commit -m "feat(cli): add account events view"
```

### Task 5: Additive `accounts status` pooled observability and docs

**Files:**
- Modify: `codex-rs/cli/src/accounts/diagnostics.rs`
- Modify: `codex-rs/cli/src/accounts/output.rs`
- Modify: `codex-rs/cli/src/accounts/observability.rs`
- Modify: `codex-rs/cli/src/accounts/observability_types.rs`
- Modify: `codex-rs/cli/src/accounts/observability_output.rs`
- Modify: `docs/getting-started.md`
- Test: `codex-rs/cli/tests/accounts_observability.rs`
- Test: `codex-rs/cli/src/accounts/observability_output.rs`

- [ ] **Step 1: Write failing `accounts status` observability tests**

Add coverage for the additive JSON contract, the full partial-failure matrix, and the required status text behavior:

```rust
#[tokio::test]
async fn accounts_status_json_adds_pool_observability_on_success() -> Result<()> {
    let codex_home = prepared_home().await?;
    seed_pool_events(&codex_home).await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "team-main");
    assert_eq!(json["poolObservability"]["poolId"], "team-main");
    assert_eq!(json["poolObservability"]["summary"]["totalAccounts"], 2);
    assert_eq!(json["poolObservability"]["warning"], serde_json::Value::Null);
    Ok(())
}

#[tokio::test]
async fn accounts_status_json_keeps_startup_fields_when_observability_read_fails() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(
        &codex_home,
        &[
            "accounts",
            "--account-pool",
            "missing-pool",
            "status",
            "--json",
        ],
    )
    .await?;

    assert!(output.success, "stderr: {}", output.stderr);
    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(json["effectivePoolId"], "missing-pool");
    assert!(json["poolObservability"]["summary"].is_null());
    assert!(json["poolObservability"]["diagnostics"].is_null());
    assert!(json["poolObservability"]["warning"].as_str().is_some());
    Ok(())
}

#[tokio::test]
async fn accounts_status_json_sets_pool_observability_null_when_no_effective_pool_resolves() -> Result<()> {
    let codex_home = prepared_no_pool_home().await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert!(json["effectivePoolId"].is_null());
    assert!(json["poolObservability"].is_null());
    Ok(())
}

#[tokio::test]
async fn accounts_status_json_keeps_summary_when_diagnostics_read_fails() -> Result<()> {
    let codex_home = prepared_home_with_broken_diagnostics_read().await?;

    let output = run_codex(&codex_home, &["accounts", "status", "--json"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);

    let json: serde_json::Value = serde_json::from_str(&output.stdout)?;
    assert!(json["poolObservability"]["summary"].is_object());
    assert!(json["poolObservability"]["diagnostics"].is_null());
    assert!(json["poolObservability"]["warning"].as_str().is_some());
    Ok(())
}

#[tokio::test]
async fn accounts_status_text_shows_counts_issue_summary_and_warning() -> Result<()> {
    let codex_home = prepared_home_with_broken_summary_read().await?;

    let output = run_codex(&codex_home, &["accounts", "status"]).await?;
    assert!(output.success, "stderr: {}", output.stderr);
    assert!(output.stdout.contains("pooled diagnostics status: degraded"));
    assert!(output.stdout.contains("issue:"));
    assert!(output.stdout.contains("warning:"));
    Ok(())
}
```

- [ ] **Step 2: Run the focused status tests and confirm failure**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability accounts_status_json_adds_pool_observability_on_success -- --nocapture
```

Expected: FAIL because `accounts status` does not emit the additive pooled observability object or mixed-failure text behavior yet.

- [ ] **Step 3: Implement additive status reads, output, and docs**

Extend `AccountsStatusDiagnostic` in `codex-rs/cli/src/accounts/diagnostics.rs`:

```rust
pub(crate) struct AccountsStatusDiagnostic {
    pub account_pool_override_id: Option<String>,
    pub configured_pool_count: usize,
    pub registered_pool_count: usize,
    pub startup: SharedStartupStatus,
    pub pool: Option<AccountPoolDiagnostic>,
    pub pool_observability: Option<StatusPoolObservabilityView>,
}
```

Add a helper that applies the spec’s partial matrix:

```rust
async fn read_status_pool_observability(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    effective_pool_id: Option<&str>,
) -> Option<StatusPoolObservabilityView> {
    let Some(pool_id) = effective_pool_id else {
        return None;
    };
    let reader = local_observability_reader(runtime, config);
    let summary = reader
        .read_pool(AccountPoolReadRequest { pool_id: pool_id.to_string() })
        .await
        .map(map_summary_view)
        .map_err(|err| err.to_string());
    let diagnostics = reader
        .read_diagnostics(AccountPoolDiagnosticsReadRequest { pool_id: pool_id.to_string() })
        .await
        .map(map_diagnostics_view)
        .map_err(|err| err.to_string());
    Some(StatusPoolObservabilityView::from_results(pool_id, summary, diagnostics))
}
```

In `codex-rs/cli/src/accounts/output.rs`, keep the legacy JSON fields and append:

```rust
"poolObservability": diagnostic.pool_observability.as_ref().map(status_pool_observability_json)
```

Delegate text rendering of the pooled summary to `observability_output.rs` so `output.rs` only remains the owner of the existing startup/status output. Add formatter unit tests there for the two mixed failure cases, the healthy summary line set, and JSON null-preservation for nullable fields.

Update `docs/getting-started.md` with a short `mcodex` operator snippet:

```md
## Inspect pooled accounts locally

Use these commands after joining or configuring a pool:

- `mcodex accounts status`
- `mcodex accounts pool show`
- `mcodex accounts diagnostics`
- `mcodex accounts events --limit 20`
```

- [ ] **Step 4: Run the observability file tests and the full CLI crate tests**

Run:

```bash
cd codex-rs
cargo test -p codex-cli --test accounts_observability
cargo test -p codex-cli
```

Expected: PASS. `status` keeps its legacy top-level JSON, adds `poolObservability`, and degrades partially instead of failing when observability reads fail.

- [ ] **Step 5: Format, lint, and commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-cli
git add cli/src/accounts/diagnostics.rs cli/src/accounts/output.rs cli/src/accounts/observability.rs cli/src/accounts/observability_types.rs cli/src/accounts/observability_output.rs cli/tests/accounts_observability.rs ../docs/getting-started.md
git commit -m "feat(cli): surface pooled account observability"
```

## Final Verification Checklist

- [ ] `codex accounts pool list` still works unchanged
- [ ] `codex accounts pool assign <ACCOUNT_ID> <POOL_ID>` still works unchanged
- [ ] `codex accounts pool show` succeeds against an effective pool and with explicit `--pool`
- [ ] `codex accounts diagnostics` is strict on missing pool / read failure
- [ ] `codex accounts events` is strict on missing pool / read failure and preserves cursor semantics
- [ ] `codex accounts status --json` keeps legacy top-level keys and adds `poolObservability`
- [ ] strict drill-down commands do not consult startup status when `--pool` or top-level `--account-pool` already provides the target
- [ ] new CLI JSON timestamps are RFC 3339 UTC strings
- [ ] text output prints `accounts: none`, `issues: none`, `events: none`, and `next cursor: ...` in the corresponding places
- [ ] formatter unit tests cover lease rendering, text empty states, cursor lines, and status mixed-failure summaries
- [ ] formatter unit tests cover JSON null-preservation for nullable observability fields
- [ ] `cd codex-rs && cargo test -p codex-cli`
- [ ] `cd codex-rs && just fmt`
- [ ] `cd codex-rs && just fix -p codex-cli`
