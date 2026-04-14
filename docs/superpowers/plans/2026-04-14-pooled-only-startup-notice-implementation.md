# Pooled-Only Startup Notice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let TUI startup distinguish pooled-only access from shared login so pooled users see a lightweight continue-or-login notice, while durably suppressed pooled startup shows a separate paused notice instead of the existing login wall.

**Architecture:** Keep `LoginStatus` and `account/read` semantics unchanged, then add a small startup-decision layer in `codex-tui` that combines shared-auth state, pooled startup probe results, and the persisted notice-hide flag. Implement the new UX as dedicated onboarding steps plus startup-local persistence helpers, reusing existing config edits for local mode and existing `config/batchWrite` plus `accountLease/*` RPCs for remote mode.

**Tech Stack:** Rust workspace crates (`codex-tui`, `codex-core`, `codex-config`), ratatui onboarding widgets, existing `AppServerSession` RPC helpers, `ConfigEditsBuilder`, `pretty_assertions`, `insta`, and targeted crate tests.

---

## Scope

In scope:

- pooled-only startup notice for embedded and remote TUI sessions
- paused pooled-startup notice for `suppressed == true`
- `Enter`, `L`, and `N` notice interactions as defined in the spec
- config-backed persistence for `notice.hide_pooled_only_startup_notice`
- snapshot coverage for the new onboarding UI

Out of scope:

- any change to `LoginStatus` or `account/read`
- new app-server protocol methods or fields
- redesigning the rest of onboarding
- remote-pool backend work beyond reusing the branch-local `accountLease/read`, `accountLease/resume`, and `config/batchWrite` surfaces
- workspace-wide `cargo test` without explicit user approval

## Execution Rules

- Use `@superpowers:test-driven-development` before each implementation task.
- Use `@superpowers:verification-before-completion` before claiming a task is done.
- Run the targeted crate tests listed in each task before `just fmt` or `just fix -p ...`.
- Run `just fmt` from `codex-rs/` after each Rust code task.
- Run `just fix -p codex-config`, `just fix -p codex-core`, and `just fix -p codex-tui` for the crates touched by that task.
- Do not rerun tests after `just fmt` or `just fix -p ...`.
- If any task changes `ConfigToml` or nested config types, run `just write-config-schema`.
- If the final implementation touches `codex-core`, ask the user before running full `cargo test`.

## Planned File Layout

- Modify `codex-rs/config/src/types.rs` to add the new `[notice]` boolean and keep config serialization/schema ownership in `codex-config`.
- Modify `codex-rs/core/src/config/edit.rs` to add a typed config edit plus builder helper for the new notice flag instead of open-coded path edits from the TUI.
- Modify `codex-rs/core/src/config/edit_tests.rs` to cover local persistence for the new flag and protect against key-path drift.
- Regenerate `codex-rs/core/config.schema.json` via `just write-config-schema` because `ConfigToml.notice` changes.
- Create `codex-rs/tui/src/startup_access.rs` to own pooled startup probing, remote/local decision mapping, and focused unit tests. Keep this logic out of `tui/src/lib.rs`.
- Modify `codex-rs/tui/src/app_server_session.rs` to expose narrow helpers for raw remote startup lease reads, remote pooled-startup resume, and remote notice persistence.
- Reuse existing state/runtime APIs from `codex-rs/state/src/runtime/account_pool.rs`, specifically `StateRuntime::preview_account_startup_selection(...)` and `StateRuntime::read_account_pool_diagnostic(...)`; this slice does not add new `codex-state` APIs.
- Reuse existing app-server protocol surfaces already present in this branch, specifically `accountLease/read`, `accountLease/resume`, and `config/batchWrite` from `codex-rs/app-server-protocol/src/protocol/v2.rs`; this slice does not add new protocol methods or fields.
- Create `codex-rs/tui/src/onboarding/pooled_access_notice.rs` to render the pooled-only and pooled-paused notice widgets plus interaction tests and snapshots.
- Modify `codex-rs/tui/src/onboarding/mod.rs` to register the new notice module.
- Modify `codex-rs/tui/src/onboarding/onboarding_screen.rs` to add dedicated onboarding steps, prebuild the hidden auth step, and return the local/remote side effects needed by startup.
- Modify `codex-rs/tui/src/lib.rs` to compute the startup decision, extend the onboarding gate, wire the new onboarding args, and reload config when startup-local notice persistence succeeds.

### Task 1: Add Config-Backed Notice Persistence

**Files:**
- Modify: `codex-rs/config/src/types.rs`
- Modify: `codex-rs/core/src/config/edit.rs`
- Test: `codex-rs/core/src/config/edit_tests.rs`
- Generate: `codex-rs/core/config.schema.json`

- [x] **Step 1: Write the failing config edit tests**

Add focused tests to `codex-rs/core/src/config/edit_tests.rs` that exercise the typed edit and builder API instead of raw TOML mutation:

```rust
#[test]
fn set_hide_pooled_only_startup_notice_writes_notice_flag() {
    let tmp = tempdir().expect("tmpdir");
    let codex_home = tmp.path();

    ConfigEditsBuilder::new(codex_home)
        .set_hide_pooled_only_startup_notice(true)
        .apply_blocking()
        .expect("persist");

    let contents = std::fs::read_to_string(codex_home.join(CONFIG_TOML_FILE)).expect("read config");
    let expected = r#"[notice]
hide_pooled_only_startup_notice = true
"#;
    assert_eq!(contents, expected);
}

#[test]
fn set_hide_pooled_only_startup_notice_false_writes_visible_state() {
    let tmp = tempdir().expect("tmpdir");
    let codex_home = tmp.path();
    std::fs::write(
        codex_home.join(CONFIG_TOML_FILE),
        "[notice]\nhide_pooled_only_startup_notice = true\n",
    )
    .expect("seed config");

    ConfigEditsBuilder::new(codex_home)
        .set_hide_pooled_only_startup_notice(false)
        .apply_blocking()
        .expect("persist");

    let contents = std::fs::read_to_string(codex_home.join(CONFIG_TOML_FILE)).expect("read config");
    assert_eq!(contents, "[notice]\nhide_pooled_only_startup_notice = false\n");
}
```

- [x] **Step 2: Run the targeted tests to verify they fail**

Run: `cargo test -p codex-core config_edit_tests::set_hide_pooled_only_startup_notice -- --nocapture`

Expected: FAIL because `ConfigEditsBuilder` and `ConfigEdit` do not yet expose the new notice key.

- [x] **Step 3: Implement the typed notice flag**

Update `codex-rs/config/src/types.rs` and `codex-rs/core/src/config/edit.rs`:

```rust
pub struct Notice {
    pub hide_full_access_warning: Option<bool>,
    pub hide_world_writable_warning: Option<bool>,
    pub hide_rate_limit_model_nudge: Option<bool>,
    pub hide_pooled_only_startup_notice: Option<bool>,
    // ...
}

pub enum ConfigEdit {
    SetNoticeHidePooledOnlyStartupNotice(bool),
    // ...
}

pub fn set_hide_pooled_only_startup_notice(mut self, acknowledged: bool) -> Self {
    self.edits
        .push(ConfigEdit::SetNoticeHidePooledOnlyStartupNotice(acknowledged));
    self
}
```

Apply the edit in `ConfigEditor::apply`, keeping the path hard-coded in one place: `notice.hide_pooled_only_startup_notice`.

- [x] **Step 4: Regenerate schema and rerun the focused tests**

Run:

```bash
just write-config-schema
cargo test -p codex-core config_edit_tests::set_hide_pooled_only_startup_notice -- --nocapture
cargo test -p codex-core config_edit_tests::set_hide_pooled_only_startup_notice_false_writes_visible_state -- --nocapture
```

Expected: PASS, and `codex-rs/core/config.schema.json` contains the new `notice.hide_pooled_only_startup_notice` property.

- [x] **Step 5: Format, lint, and commit**

Run:

```bash
just fmt
just fix -p codex-config
just fix -p codex-core
git add codex-rs/config/src/types.rs codex-rs/core/src/config/edit.rs codex-rs/core/src/config/edit_tests.rs codex-rs/core/config.schema.json
git commit -m "feat(config): persist pooled startup notice preference"
```

### Task 2: Add Startup Access Probing and Session Helpers

**Files:**
- Create: `codex-rs/tui/src/startup_access.rs`
- Modify: `codex-rs/tui/src/app_server_session.rs`
- Modify: `codex-rs/tui/src/lib.rs`
- Test: `codex-rs/tui/src/startup_access.rs`

- [x] **Step 1: Write the failing startup-decision tests**

Add unit tests in the new `codex-rs/tui/src/startup_access.rs` covering local and remote mapping without touching onboarding yet:

```rust
#[test]
fn startup_decision_is_no_prompt_when_shared_login_exists() {
    let decision = decide_startup_access(
        /*login_status*/ LoginStatus::AuthMode(AppServerAuthMode::Chatgpt),
        /*provider_requires_openai_auth*/ true,
        /*notice_hidden*/ false,
        /*probe*/ StartupProbe::PooledAvailable { remote: false },
    );

    assert_eq!(decision, StartupPromptDecision::NoPrompt);
}

#[test]
fn startup_decision_uses_pooled_only_notice_when_pooled_access_exists() {
    let decision = decide_startup_access(
        LoginStatus::NotAuthenticated,
        true,
        false,
        StartupProbe::PooledAvailable { remote: false },
    );

    assert_eq!(decision, StartupPromptDecision::PooledOnlyNotice);
}

#[test]
fn startup_decision_uses_paused_notice_when_probe_is_suppressed() {
    let decision = decide_startup_access(
        LoginStatus::NotAuthenticated,
        true,
        false,
        StartupProbe::PooledSuppressed { remote: true },
    );

    assert_eq!(decision, StartupPromptDecision::PooledAccessPausedNotice);
}

#[test]
fn startup_decision_honors_hidden_notice_without_redefining_login() {
    let decision = decide_startup_access(
        LoginStatus::NotAuthenticated,
        true,
        true,
        StartupProbe::PooledAvailable { remote: false },
    );

    assert_eq!(decision, StartupPromptDecision::NoPrompt);
}

#[tokio::test]
async fn startup_probe_failure_falls_back_to_needs_login() {
    let decision = resolve_startup_prompt_decision_with_probe(
        LoginStatus::NotAuthenticated,
        true,
        false,
        Err(anyhow!("probe failed")),
    )
    .await
    .expect("probe failure should not bubble");

    assert_eq!(decision, StartupPromptDecision::NeedsLogin);
}
```

- [x] **Step 2: Run the targeted tests to verify they fail**

Run: `cargo test -p codex-tui startup_access::tests::startup_decision_ -- --nocapture`

Expected: FAIL because the new module and decision types do not exist.

- [x] **Step 3: Implement the startup-access module and app-server helpers**

Create `codex-rs/tui/src/startup_access.rs` with:

```rust
pub(crate) enum StartupProbe {
    Unavailable,
    PooledAvailable { remote: bool },
    PooledSuppressed { remote: bool },
}

pub(crate) enum StartupPromptDecision {
    NeedsLogin,
    PooledOnlyNotice,
    PooledAccessPausedNotice,
    NoPrompt,
}
```

Implement:

- pure decision logic
- local exact probe in `codex-rs/tui/src/startup_access.rs` by initializing `codex_state::StateRuntime` from `config.sqlite_home` and calling `StateRuntime::preview_account_startup_selection(...)` plus `StateRuntime::read_account_pool_diagnostic(...)` directly
- remote best-effort probe using raw `accountLease/read`
- fail-open mapping so probe errors log and fall back to `NeedsLogin`

These dependencies are already available in this branch:

- `codex-rs/state/src/runtime/account_pool.rs`
- `codex-rs/app-server-protocol/src/protocol/v2.rs`

Do not add new state-layer or protocol API in this task.

Extend `codex-rs/tui/src/app_server_session.rs` with narrow helpers such as:

```rust
pub(crate) async fn read_account_lease_startup_probe(
    &self,
) -> Result<Option<AccountLeaseReadResponse>>;

pub(crate) async fn resume_pooled_startup(&self) -> Result<AccountLeaseResumeResponse>;

pub(crate) async fn write_hide_pooled_only_startup_notice(&mut self, hide: bool) -> Result<()>;
```

Keep these wrappers narrow and reuse existing `ClientRequest::{AccountLeaseRead, AccountLeaseResume, ConfigBatchWrite}` instead of adding new protocol.

- [x] **Step 4: Rerun the targeted tests**

Run:

```bash
cargo test -p codex-tui startup_access::tests::startup_decision_ -- --nocapture
cargo test -p codex-tui startup_access::tests::remote_probe_ -- --nocapture
cargo test -p codex-tui startup_access::tests::startup_probe_failure_falls_back_to_needs_login -- --nocapture
```

Expected: PASS. The remote probe tests should prove:

- `suppressed == true` maps to `PooledAccessPausedNotice`
- visible remote pooled surface maps to `PooledOnlyNotice`
- empty remote lease data falls back to `NeedsLogin`

- [x] **Step 5: Format, lint, and commit**

Run:

```bash
just fmt
just fix -p codex-tui
git add codex-rs/tui/src/startup_access.rs codex-rs/tui/src/app_server_session.rs codex-rs/tui/src/lib.rs
git commit -m "feat(tui): add pooled startup access decision layer"
```

### Task 3: Build the Pooled Notice Widgets and Snapshot Coverage

**Files:**
- Create: `codex-rs/tui/src/onboarding/pooled_access_notice.rs`
- Modify: `codex-rs/tui/src/onboarding/mod.rs`
- Test: `codex-rs/tui/src/onboarding/pooled_access_notice.rs`
- Snapshot: `codex-rs/tui/src/onboarding/snapshots/*pooled_access_notice*.snap`

- [x] **Step 1: Write the failing widget and interaction tests**

Add focused tests directly next to the new widget module:

```rust
#[test]
fn pooled_only_notice_enter_marks_continue() {
    let mut widget = PooledAccessNoticeWidget::pooled_only(/*animations_enabled*/ false);
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(widget.outcome(), Some(PooledAccessNoticeOutcome::Continue));
}

#[test]
fn pooled_only_notice_l_requests_login_handoff() {
    let mut widget = PooledAccessNoticeWidget::pooled_only(false);
    widget.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    assert_eq!(widget.outcome(), Some(PooledAccessNoticeOutcome::OpenLogin));
}

#[test]
fn pooled_paused_notice_enter_requests_resume() {
    let mut widget = PooledAccessNoticeWidget::pooled_paused(false);
    widget.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(widget.outcome(), Some(PooledAccessNoticeOutcome::ResumeAndContinue));
}

#[test]
fn pooled_paused_notice_l_requests_login_handoff() {
    let mut widget = PooledAccessNoticeWidget::pooled_paused(false);
    widget.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    assert_eq!(widget.outcome(), Some(PooledAccessNoticeOutcome::OpenLogin));
}

#[test]
fn pooled_paused_notice_shows_inline_error() {
    let mut widget = PooledAccessNoticeWidget::pooled_paused(false);
    widget.set_error("resume failed".to_string());
    assert!(widget.rendered_text().contains("resume failed"));
}

#[test]
fn pooled_only_notice_renders_snapshot() {
    let widget = PooledAccessNoticeWidget::pooled_only(false);
    // draw to VT100Backend and assert_snapshot!(...)
}
```

- [x] **Step 2: Run the targeted tests to verify they fail**

Run: `cargo test -p codex-tui pooled_access_notice -- --nocapture`

Expected: FAIL because the widget module does not exist and no snapshots have been recorded.

- [x] **Step 3: Implement the dedicated onboarding widget**

Create `codex-rs/tui/src/onboarding/pooled_access_notice.rs` with:

- a small `PooledAccessNoticeKind` enum: `PooledOnly` vs `PooledPaused`
- a small outcome enum: `Continue`, `OpenLogin`, `HideAndContinue`, `ResumeAndContinue`
- concise ratatui rendering using existing onboarding styling conventions
- inline error display for paused-resume failures
- explicit `L` handling for both notice kinds

Register the module in `codex-rs/tui/src/onboarding/mod.rs`.

- [x] **Step 4: Rerun tests and accept snapshots intentionally**

Run:

```bash
cargo test -p codex-tui pooled_access_notice -- --nocapture
cargo insta pending-snapshots -p codex-tui
cargo insta accept -p codex-tui
```

Expected: PASS, with accepted snapshots for both the pooled-only and pooled-paused notice renderings.

- [x] **Step 5: Format, lint, and commit**

Run:

```bash
just fmt
just fix -p codex-tui
git add codex-rs/tui/src/onboarding/mod.rs codex-rs/tui/src/onboarding/pooled_access_notice.rs codex-rs/tui/src/onboarding/snapshots
git commit -m "feat(tui): add pooled startup notice widgets"
```

### Task 4: Wire Onboarding Flow, Persistence, and Startup Integration

**Files:**
- Modify: `codex-rs/tui/src/onboarding/onboarding_screen.rs`
- Modify: `codex-rs/tui/src/lib.rs`
- Modify: `codex-rs/tui/src/app_server_session.rs`
- Modify: `codex-rs/tui/src/startup_access.rs`
- Test: `codex-rs/tui/src/onboarding/onboarding_screen.rs`
- Test: `codex-rs/tui/src/lib.rs`

- [x] **Step 1: Write the failing onboarding/integration tests**

Add focused tests for the new startup path. Put the interaction harness in `codex-rs/tui/src/onboarding/onboarding_screen.rs` as a local `#[cfg(test)]` helper so the plan does not depend on an undefined shared harness:

```rust
#[tokio::test]
async fn pooled_only_startup_shows_notice_instead_of_auth_step() {
    let harness = OnboardingHarness::pooled_only_notice().await;
    let result = harness.run_onboarding().await;
    assert_eq!(result.visible_steps(), vec!["welcome", "pooled_notice"]);
}

#[tokio::test]
async fn pooled_only_notice_l_reveals_prebuilt_auth_step() {
    let mut harness = OnboardingHarness::pooled_only_notice().await;
    harness.press('l').await;
    assert_eq!(harness.visible_steps(), vec!["welcome", "pooled_notice", "auth"]);
}

#[tokio::test]
async fn paused_notice_enter_resumes_remote_pooled_startup() {
    let mut harness = OnboardingHarness::remote_paused_notice().await;
    harness.press_enter().await;
    assert_eq!(harness.resume_requests(), 1);
    assert!(harness.completed_onboarding());
}

#[tokio::test]
async fn paused_notice_l_reveals_prebuilt_auth_step() {
    let mut harness = OnboardingHarness::remote_paused_notice().await;
    harness.press('l').await;
    assert_eq!(harness.visible_steps(), vec!["welcome", "pooled_paused_notice", "auth"]);
}

#[tokio::test]
async fn paused_notice_resume_failure_stays_visible_with_error() {
    let mut harness = OnboardingHarness::remote_paused_notice_with_resume_error().await;
    harness.press_enter().await;
    assert_eq!(harness.visible_steps(), vec!["welcome", "pooled_paused_notice"]);
    assert!(harness.rendered_text().contains("resume failed"));
}

#[tokio::test]
async fn pooled_only_notice_n_persists_flag_and_skips_future_notice() {
    let mut harness = OnboardingHarness::embedded_pooled_only_notice().await;
    harness.press('n').await;
    assert!(harness.notice_flag_persisted());
    assert_eq!(harness.next_launch_decision(), StartupPromptDecision::NoPrompt);
}
```

In the same task, add narrow `codex-rs/tui/src/lib.rs` tests for the outer gate so `should_show_onboarding(...)` and `should_show_login_screen(...)` do not regress once pooled notice states are introduced.

- [x] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test -p codex-tui pooled_only_startup_shows_notice_instead_of_auth_step -- --nocapture
cargo test -p codex-tui pooled_only_notice_l_reveals_prebuilt_auth_step -- --nocapture
```

Expected: FAIL because onboarding still only knows `Welcome`, `Auth`, and `TrustDirectory`, and `should_show_onboarding` does not include pooled notice states.

- [x] **Step 3: Implement the minimal onboarding wiring**

Update `codex-rs/tui/src/onboarding/onboarding_screen.rs`:

- extend `Step` with pooled-only and paused notice variants
- extend `OnboardingScreenArgs` with a `startup_prompt_decision` and any per-mode dependencies needed to persist/ resume
- represent the auth step as a hidden prebuilt variant, for example `Step::Auth { widget, revealed: bool }`, so `L` flips `revealed = true` without rebuilding `AuthModeWidget`
- prebuild the auth step whenever OpenAI auth is required and `app_server_request_handle` exists
- keep the auth step hidden until either `NeedsLogin` or notice `L` reveals it
- persist local hide-flag changes via `ConfigEditsBuilder`
- use `AppServerSession` wrappers for remote resume and remote notice persistence
- keep paused-notice resume failures on the same step and surface the error inline instead of exiting onboarding

Update `codex-rs/tui/src/lib.rs`:

- compute `StartupPromptDecision` before `should_show_onboarding`
- update `should_show_onboarding` to include both notice states
- reload local config if the startup-local persistence path changed the file
- continue passing `None` for onboarding app-server access only when no startup interaction needs it

- [x] **Step 4: Rerun targeted tests plus crate-level coverage**

Run:

```bash
cargo test -p codex-tui pooled_only_startup_shows_notice_instead_of_auth_step -- --nocapture
cargo test -p codex-tui pooled_only_notice_l_reveals_prebuilt_auth_step -- --nocapture
cargo test -p codex-tui paused_notice_enter_resumes_remote_pooled_startup -- --nocapture
cargo test -p codex-tui paused_notice_l_reveals_prebuilt_auth_step -- --nocapture
cargo test -p codex-tui paused_notice_resume_failure_stays_visible_with_error -- --nocapture
cargo test -p codex-tui pooled_only_notice_n_persists_flag_and_skips_future_notice -- --nocapture
cargo test -p codex-tui
```

Expected: PASS. Verify that:

- pooled-only startup surfaces the notice instead of the login wall
- paused startup surfaces the paused notice
- `L` reveals the prebuilt auth step instead of rebuilding onboarding
- `N` persists the new config key and suppresses future pooled-only prompts

- [x] **Step 5: Format, lint, verify crate boundaries, and commit**

Run:

```bash
just fmt
just fix -p codex-tui
git add codex-rs/tui/src/onboarding/onboarding_screen.rs codex-rs/tui/src/lib.rs codex-rs/tui/src/app_server_session.rs codex-rs/tui/src/startup_access.rs
git commit -m "feat(tui): wire pooled startup notices into onboarding"
```

## Final Verification

- [x] **Step 1: Run the touched-crate verification suite**

Run:

```bash
cargo test -p codex-core
cargo test -p codex-tui
```

Expected: PASS.

- [x] **Step 2: Run formatting and scoped linting**

Run:

```bash
just fmt
just fix -p codex-config
just fix -p codex-core
just fix -p codex-tui
```

Expected: PASS with no additional source edits required.

- [ ] **Step 3: Ask before full workspace test** (Deferred: full workspace `cargo test` still requires explicit approval.)

If the user approves, run:

```bash
cargo test
```

Expected: PASS. Do not run this step without explicit approval because the work touches `codex-core`.

- [x] **Step 4: Final commit if verification uncovered follow-up changes**

Run:

```bash
git status --short
git add <any follow-up files>
git commit -m "test(tui): close pooled startup notice verification gaps"
```

Only create this commit if the verification steps above required additional code or snapshot adjustments.
