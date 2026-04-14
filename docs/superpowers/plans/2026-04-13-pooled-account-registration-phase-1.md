# Pooled Account Registration Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the remaining phase 1 `accounts add` work so `codex accounts add chatgpt` and `codex accounts add chatgpt --device-auth` register pooled accounts without mutating shared legacy auth, while `accounts add api-key` fails explicitly as unsupported.

**Architecture:** Reuse the existing pooled ChatGPT registration flows in `codex-login` to obtain provider tokens, but keep the CLI responsible for target-pool resolution, deterministic idempotency keys, and pending-registration journal updates. Keep the local backend responsible for converting provider identity into a filesystem-safe `backend_account_handle` before staging backend-private auth, so the CLI does not learn local storage rules and the path to a future remote backend stays clean.

**Tech Stack:** Rust workspace crates (`codex-cli`, `codex-account-pool`, `codex-login`, `codex-state`), SQLite via `sqlx`, clap, existing pooled browser/device auth flows, `pretty_assertions`, and targeted crate tests.

**Completion Notes:** Completed on `multi-account-pool-v1`. `codex accounts add chatgpt` and `codex accounts add chatgpt --device-auth` now register pooled accounts through backend-private auth without touching shared legacy auth. `accounts add api-key` and remote backends remain explicitly out of scope for this phase.

---

## Scope

This plan is intentionally narrower than the earlier broad phase-1 plan at the same path.
Most pooled lease/runtime/app-server/TUI work is already landed on this branch. The remaining
implementation slice is the operator-facing `accounts add` path.

In scope:

- `accounts add chatgpt`
- `accounts add chatgpt --device-auth`
- `accounts add` as a browser-ChatGPT alias so the current optional CLI grammar remains usable
- deterministic target-pool resolution for fresh add
- deterministic pending-registration journaling after provider identity is known
- local-backend generation of safe `backend_account_handle` values
- explicit phase-1 error for `accounts add api-key`

Out of scope:

- remote backend implementation
- `accounts add api-key`
- redesign of legacy `codex login/logout/status`
- protocol changes outside the current CLI/local-backend registration path
- workspace-wide `cargo test` without explicit user approval

## Execution Rules

- Use `@superpowers:test-driven-development` before each implementation task.
- Use `@superpowers:verification-before-completion` before claiming a task is done.
- Run only the targeted tests listed in each task unless the user explicitly approves more.
- Run `just fmt` after each Rust code task and `just fix -p <crate>` for each touched crate.
- Do not rerun tests after `just fmt` or `just fix -p ...`; test first, then format/fix, then commit.

## Planned File Layout

- Modify `codex-rs/cli/src/accounts/mod.rs` to remove the hard-coded credential-gap bailout and dispatch the add flows through `registration.rs`.
- Modify `codex-rs/cli/src/accounts/registration.rs` to own pool resolution, ChatGPT browser/device registration orchestration, deterministic idempotency keys, explicit API-key rejection, and the user-facing result type for add/import helpers.
- Modify `codex-rs/cli/tests/accounts.rs` only for process-level command behavior that does not require interactive OAuth: no-pool fast-fail, bare `accounts add` alias behavior, and explicit API-key unsupported messaging.
- Modify `codex-rs/account-pool/src/backend/local/control.rs` to normalize raw provider identity into a filesystem-safe local `backend_account_handle` before staging auth or writing the registry row.
- Modify `codex-rs/account-pool/src/backend/local/mod.rs` to hold the small local-backend handle-normalization helper close to the path helpers that consume it.
- Modify `codex-rs/account-pool/tests/lease_lifecycle.rs` for backend-private auth path safety and re-registration coverage.
- Reuse `codex-rs/login/src/pooled_registration.rs` as-is; do not add new CLI flags or hidden OAuth test knobs in this slice.

### Task 1: Add CLI Registration Orchestration and Unit Coverage

**Files:**
- Modify: `codex-rs/cli/src/accounts/registration.rs`
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Test: `codex-rs/cli/src/accounts/registration.rs`

- [x] **Step 1: Write the failing registration unit tests**

Add focused tests inside `codex-rs/cli/src/accounts/registration.rs` so the command orchestration can
be tested without spawning the binary or opening a browser:

```rust
#[tokio::test]
async fn add_chatgpt_registration_uses_override_pool_and_keeps_startup_defaults() -> Result<()> {
    let harness = RegistrationHarness::with_configured_pool("team-main").await?;
    let runner = FakeChatgptRegistrationRunner::browser_success("acct-new");

    let result = add_chatgpt_account_with_runner(
        &harness.runtime,
        &harness.config,
        Some("team-other"),
        /*device_auth*/ false,
        &runner,
    )
    .await?;

    assert_eq!(result.account_id, "acct-new");
    assert_eq!(result.pool_id, "team-other");
    assert_eq!(
        harness.runtime.read_account_startup_selection().await?,
        AccountStartupSelectionState {
            default_pool_id: Some("team-main".to_string()),
            preferred_account_id: None,
            suppressed: false,
        }
    );
    Ok(())
}

#[tokio::test]
async fn add_chatgpt_registration_fails_without_resolved_pool_before_persisting_state() -> Result<()> {
    let harness = RegistrationHarness::without_configured_pool().await?;
    let runner = FakeChatgptRegistrationRunner::browser_success("acct-new");

    let err = add_chatgpt_account_with_runner(
        &harness.runtime,
        &harness.config,
        None,
        /*device_auth*/ false,
        &runner,
    )
    .await
    .expect_err("missing pool should fail before registration");

    assert!(err.to_string().contains("configure a pool"));
    assert!(harness.runtime.list_pending_account_registrations().await?.is_empty());
    assert_eq!(runner.browser_calls(), 0);
    Ok(())
}

#[tokio::test]
async fn add_chatgpt_registration_is_idempotent_for_same_identity_in_same_pool() -> Result<()> {
    let harness = RegistrationHarness::with_configured_pool("team-main").await?;
    let runner = FakeChatgptRegistrationRunner::device_success("provider-acct-new");

    let first = add_chatgpt_account_with_runner(
        &harness.runtime,
        &harness.config,
        None,
        /*device_auth*/ true,
        &runner,
    )
    .await?;
    let second = add_chatgpt_account_with_runner(
        &harness.runtime,
        &harness.config,
        None,
        /*device_auth*/ true,
        &runner,
    )
    .await?;

    assert_eq!(first.account_id, second.account_id);
    assert_eq!(first.provider_account_id, "provider-acct-new");
    assert_eq!(harness.membership(first.account_id.as_str()).await?.unwrap().pool_id, "team-main");
    Ok(())
}

#[tokio::test]
async fn add_chatgpt_registration_rejects_existing_identity_in_other_pool() -> Result<()> {
    let harness = RegistrationHarness::with_registered_account(
        "acct-local-1",
        "provider-acct-new",
        "team-main",
    )
    .await?;
    let runner = FakeChatgptRegistrationRunner::browser_success("provider-acct-new");

    let err = add_chatgpt_account_with_runner(
        &harness.runtime,
        &harness.config,
        Some("team-other"),
        /*device_auth*/ false,
        &runner,
    )
    .await
    .expect_err("cross-pool reuse should be rejected");

    assert!(err.to_string().contains("accounts pool assign"));
    Ok(())
}

#[tokio::test]
async fn add_chatgpt_registration_assigns_existing_unassigned_identity_to_resolved_pool() -> Result<()> {
    let harness = RegistrationHarness::with_registered_unassigned_account(
        "acct-local-1",
        "provider-acct-new",
    )
    .await?;
    let runner = FakeChatgptRegistrationRunner::browser_success("provider-acct-new");

    let result = add_chatgpt_account_with_runner(
        &harness.runtime,
        &harness.config,
        Some("team-main"),
        /*device_auth*/ false,
        &runner,
    )
    .await?;

    assert_eq!(result.account_id, "acct-local-1");
    assert_eq!(result.provider_account_id, "provider-acct-new");
    assert_eq!(harness.membership("acct-local-1").await?.unwrap().pool_id, "team-main");
    Ok(())
}

#[tokio::test]
async fn add_chatgpt_registration_reconciles_pending_row_before_retry() -> Result<()> {
    let harness = RegistrationHarness::with_configured_pool("team-main").await?;
    harness
        .runtime
        .create_pending_account_registration(NewPendingAccountRegistration {
            idempotency_key: "chatgpt-add:local:provider-acct-new:team-main".to_string(),
            backend_id: "local".to_string(),
            provider_kind: "chatgpt".to_string(),
            target_pool_id: Some("team-main".to_string()),
            backend_account_handle: Some("chatgpt-70726f76696465722d616363742d6e6577".to_string()),
            account_id: Some("acct-local-1".to_string()),
        })
        .await?;

    reconcile_pending_add_registration(
        &harness.runtime,
        "chatgpt-add:local:provider-acct-new:team-main",
        &FakeControlPlane::default(),
    )
    .await?;

    let pending = harness
        .runtime
        .read_pending_account_registration("chatgpt-add:local:provider-acct-new:team-main")
        .await?;
    assert!(pending.is_none() || pending.unwrap().completed_at.is_some());
    Ok(())
}

#[tokio::test]
async fn add_chatgpt_registration_compensates_backend_record_when_finalize_fails() -> Result<()> {
    let harness = RegistrationHarness::with_configured_pool("team-main").await?;
    let runner = FakeChatgptRegistrationRunner::browser_success("provider-acct-new");
    let control_plane = FakeControlPlane::register_success(
        "acct-local-1",
        "provider-acct-new",
        "chatgpt-70726f76696465722d616363742d6e6577",
    );
    let finalizer = FailingPendingFinalizer::once("finalize failed");

    let err = add_chatgpt_account_with_dependencies(
        &harness.runtime,
        &harness.config,
        None,
        /*device_auth*/ false,
        &runner,
        &control_plane,
        &finalizer,
    )
    .await
    .expect_err("finalize failure should trigger compensation");

    assert!(err.to_string().contains("finalize failed"));
    assert_eq!(
        control_plane.deleted_account_ids(),
        vec!["acct-local-1".to_string()]
    );
    Ok(())
}

#[tokio::test]
async fn add_api_key_reports_phase_one_unsupported() {
    let err = api_key_add_is_unsupported().expect_err("api-key should stay unsupported");
    assert!(err.to_string().contains("phase 1"));
    assert!(err.to_string().contains("chatgpt"));
}
```

- [x] **Step 2: Run the CLI crate tests to verify failure**

Run: `cargo test -p codex-cli add_chatgpt_registration_ -- --nocapture`  
Expected: FAIL because the orchestration helper, fake runner seam, and API-key rejection helper do not exist.

- [x] **Step 3: Implement the minimal orchestration helpers**

In `codex-rs/cli/src/accounts/registration.rs`, add:

```rust
pub(crate) struct RegisteredAddAccount {
    pub account_id: String,
    pub provider_account_id: String,
    pub pool_id: String,
}

/// Test seam for the two interactive ChatGPT registration flows.
trait ChatgptRegistrationRunner {
    async fn run_browser(
        &self,
        config: &Config,
    ) -> std::io::Result<ChatgptManagedRegistrationTokens>;

    async fn run_device_auth(
        &self,
        config: &Config,
    ) -> std::io::Result<ChatgptManagedRegistrationTokens>;
}

pub(crate) async fn add_chatgpt_account(
    runtime: &Arc<StateRuntime>,
    config: &Config,
    account_pool_override: Option<&str>,
    device_auth: bool,
) -> anyhow::Result<RegisteredAddAccount> {
    add_chatgpt_account_with_runner(
        runtime,
        config,
        account_pool_override,
        device_auth,
        &LiveChatgptRegistrationRunner,
    )
    .await
}
```

Keep the implementation conservative:

- resolve the target pool before token acquisition using:
  1. `--account-pool`
  2. current `effective_pool_id` from `read_current_diagnostic(...)`
  3. otherwise fail with guidance to configure a pool
- keep bare `accounts add` behavior in the top-level dispatch by mapping it to the browser ChatGPT path
- treat the ChatGPT token payload's `account_id` as `provider_account_id` throughout orchestration;
  keep the local control-plane `account_id` separate and use the value returned by
  `register_account(...)` for user-visible output and follow-up mutations
- acquire ChatGPT tokens first, then compute a deterministic key like
  `chatgpt-add:local:{provider_account_id}:{pool_id}`
- before a fresh registration attempt, run a narrow `reconcile_pending_add_registration(...)`
  helper for that deterministic key:
  - if the pending row already has both `backend_account_handle` and `account_id`, finalize it if
    the registered account already exists locally; otherwise attempt `delete_registered_account`
    for the recorded local `account_id`, then clear or preserve the row based on compensation
    success
  - if the row has only `account_id`, clear it if no local registered account exists; otherwise
    finalize it against the already-persisted local row
  - if the row has only `backend_account_handle`, fail closed with an explicit manual-recovery
    error in phase 1 because the local control plane cannot safely map that handle back to a local
    account id
  - if the row has neither, clear it and continue
- call `create_pending_account_registration(...)` only after provider identity is known
- use `LocalAccountPoolBackend::new(...)` and `register_account(...)`
- on success call `finalize_pending_account_registration(...)`
- if token acquisition fails before journaling, return the error directly
- if registration fails after journaling but before backend success, clear the pending row
- if backend registration succeeds but `finalize_pending_account_registration(...)` fails, call
  `delete_registered_account(registered.account_id.as_str())`; clear the pending row only if that
  compensation succeeds, otherwise leave the row in place for the next retry/reconciliation pass
- if the same provider identity is already registered with no active membership, assign that
  existing local `account_id` into the resolved pool and succeed
- do not mutate `account_startup_selection.default_pool_id`
- keep `import_legacy_account(...)` in this file; do not redesign it in this slice
- keep `api_key_add_is_unsupported()` as a small helper that always returns a clear phase-1 error

Do **not** add hidden CLI flags or env-based OAuth test hooks here. The fake runner seam is enough.

- [x] **Step 4: Run the targeted CLI tests**

Run: `cargo test -p codex-cli add_chatgpt_registration_ -- --nocapture`  
Run: `cargo test -p codex-cli reconcile_pending_add_registration -- --nocapture`  
Run: `cargo test -p codex-cli add_api_key_reports_phase_one_unsupported -- --nocapture`  
Expected: PASS.

- [x] **Step 5: Format, lint, and commit**

Run: `just fmt`  
Run: `just fix -p codex-cli`

```bash
git add codex-rs/cli/src/accounts/registration.rs codex-rs/cli/src/accounts/mod.rs
git commit -m "feat(cli): add pooled registration orchestration helpers"
```

### Task 2: Normalize Local Backend Handles Before Persisting Auth

**Files:**
- Modify: `codex-rs/account-pool/src/backend/local/mod.rs`
- Modify: `codex-rs/account-pool/src/backend/local/control.rs`
- Test: `codex-rs/account-pool/tests/lease_lifecycle.rs`

- [x] **Step 1: Write the failing backend tests**

Add tests in `codex-rs/account-pool/tests/lease_lifecycle.rs` that lock the new path-safety rule:

```rust
#[tokio::test]
async fn lease_lifecycle_register_account_encodes_provider_identity_for_backend_private_auth() {
    let backend = test_backend().await;

    let record = backend
        .register_account(pooled_registration(
            "acct-local-1",
            "team-main",
            "fingerprint:provider-acct/with/slash",
            Some(test_tokens("provider-acct/with/slash")),
        ))
        .await
        .expect("register account");

    assert_eq!(record.account_id, "acct-local-1");
    assert_ne!(record.backend_account_handle, "provider-acct/with/slash");
    assert!(record.backend_account_handle.starts_with("chatgpt-"));
    assert!(backend
        .backend_private_auth_home(record.backend_account_handle.as_str())
        .join("auth.json")
        .exists());
}

#[tokio::test]
async fn lease_lifecycle_register_account_reuses_encoded_handle_for_same_provider_identity() {
    let backend = test_backend().await;

    let first = backend
        .register_account(pooled_registration(
            "acct-local-1",
            "team-main",
            "fingerprint:provider-acct/with/slash",
            Some(test_tokens("provider-acct/with/slash")),
        ))
        .await
        .expect("first registration");
    let second = backend
        .register_account(pooled_registration(
            "acct-local-1",
            "team-main",
            "fingerprint:provider-acct/with/slash",
            Some(test_tokens("provider-acct/with/slash")),
        ))
        .await
        .expect("second registration");

    assert_eq!(first.account_id, second.account_id);
    assert_eq!(first.backend_account_handle, second.backend_account_handle);
}
```

- [x] **Step 2: Run the backend tests to verify failure**

Run: `cargo test -p codex-account-pool lease_lifecycle_register_account_ -- --nocapture`  
Expected: FAIL because the local backend still treats `request.backend_account_handle` as a raw path component.

- [x] **Step 3: Implement local-handle normalization inside the backend**

In `codex-rs/account-pool/src/backend/local/mod.rs`, add a tiny helper:

```rust
pub(crate) fn normalized_chatgpt_backend_account_handle(provider_account_id: &str) -> String {
    let mut encoded = String::with_capacity(provider_account_id.len() * 2);
    for byte in provider_account_id.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    format!("chatgpt-{encoded}")
}
```

In `codex-rs/account-pool/src/backend/local/control.rs`:

- compute a normalized handle from `pooled_registration_tokens.account_id` before
  `stage_pooled_registration_auth(...)`
- clone/update the incoming `RegisteredAccountUpsert` with the normalized handle
- use the normalized request for both auth staging and `upsert_registered_account(...)`
- keep `account_id`, `provider_fingerprint`, and membership semantics unchanged
- do **not** redesign `RegisteredAccountRegistration` or the control-plane trait in this slice

This is a deliberate compatibility move: the backend becomes the owner of local handle generation
without forcing broader request-shape churn across crates that are already working.

- [x] **Step 4: Run the targeted backend tests**

Run: `cargo test -p codex-account-pool lease_lifecycle_register_account_ -- --nocapture`  
Expected: PASS.

- [x] **Step 5: Format, lint, and commit**

Run: `just fmt`  
Run: `just fix -p codex-account-pool`

```bash
git add codex-rs/account-pool/src/backend/local/mod.rs codex-rs/account-pool/src/backend/local/control.rs codex-rs/account-pool/tests/lease_lifecycle.rs
git commit -m "fix(account-pool): normalize backend-private handles for pooled registration"
```

### Task 3: Wire `accounts add` Dispatch and Process-Level CLI Coverage

**Files:**
- Modify: `codex-rs/cli/src/accounts/mod.rs`
- Modify: `codex-rs/cli/tests/accounts.rs`

- [x] **Step 1: Replace the old gap tests with process-level add-command tests**

In `codex-rs/cli/tests/accounts.rs`, replace the current credential-gap assertions with command
behavior that can run without real OAuth:

```rust
#[tokio::test]
async fn accounts_add_chatgpt_without_resolved_pool_fails_before_auth() -> Result<()> {
    let codex_home = prepared_legacy_auth_only_home().await?;

    let output = run_codex(&codex_home, &["accounts", "add", "chatgpt"]).await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("configure a pool"));
    assert!(!state_db_path(codex_home.path()).exists());
    Ok(())
}

#[tokio::test]
async fn accounts_add_without_mode_uses_chatgpt_path_and_fails_before_auth_without_pool() -> Result<()> {
    let codex_home = prepared_legacy_auth_only_home().await?;

    let output = run_codex(&codex_home, &["accounts", "add"]).await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("configure a pool"));
    assert!(!output.stderr.contains("credential_ref"));
    Ok(())
}

#[tokio::test]
async fn accounts_add_api_key_reports_phase_one_unsupported() -> Result<()> {
    let codex_home = prepared_home().await?;

    let output = run_codex(&codex_home, &["accounts", "add", "api-key"]).await?;

    assert!(!output.success, "stdout: {}", output.stdout);
    assert!(output.stderr.contains("phase 1"));
    assert!(output.stderr.contains("chatgpt"));
    Ok(())
}
```

- [x] **Step 2: Run the integration test to verify failure**

Run: `cargo test -p codex-cli --test accounts accounts_add_ -- --nocapture`  
Expected: FAIL because `run_accounts_impl(...)` still bails out with the old credential-gap message before config loading and dispatch.

- [x] **Step 3: Remove the hard-coded add bailout and route through registration helpers**

In `codex-rs/cli/src/accounts/mod.rs`:

- delete `ACCOUNTS_ADD_CREDENTIAL_STORAGE_GAP`
- remove both early `AccountsSubcommand::Add(_)` bailouts
- after config/runtime initialization, dispatch:

```rust
AccountsSubcommand::Add(command) => {
    let added = match command.subcommand {
        None => add_chatgpt_account(&runtime, &config, account_pool.as_deref(), /*device_auth*/ false).await?,
        Some(AddAccountSubcommand::Chatgpt(command)) => {
            add_chatgpt_account(&runtime, &config, account_pool.as_deref(), command.device_auth).await?
        }
        Some(AddAccountSubcommand::ApiKey) => api_key_add_is_unsupported()?,
    };
    println!("registered account: {} pool={}", added.account_id, added.pool_id);
    Ok(())
}
```

Keep the current clap shape intact. This is not the slice to redesign the command grammar.

- [x] **Step 4: Run the targeted CLI tests**

Run: `cargo test -p codex-cli --test accounts accounts_add_ -- --nocapture`  
Run: `cargo test -p codex-cli add_chatgpt_registration_ -- --nocapture`  
Expected: PASS.

- [x] **Step 5: Run crate verification, then format/lint, and commit**

Run: `cargo test -p codex-cli`  
Run: `cargo test -p codex-account-pool`  
Run: `just fmt`  
Run: `just fix -p codex-cli`  
Run: `just fix -p codex-account-pool`

```bash
git add codex-rs/cli/src/accounts/mod.rs codex-rs/cli/tests/accounts.rs
git commit -m "feat(cli): enable pooled accounts add for chatgpt"
```

## Manual Smoke Checklist

After the code tasks above are complete, verify on this branch:

1. `codex accounts add api-key` prints the explicit phase-1 unsupported error.
2. `codex accounts add` on a home with no effective pool fails before browser/device auth starts.
3. `codex accounts --account-pool team-main add chatgpt --device-auth` registers a new account.
4. `codex accounts list` shows the new account in `team-main`.
5. `codex accounts current --json` still reports the original durable startup selection unchanged unless the operator changes it separately.
