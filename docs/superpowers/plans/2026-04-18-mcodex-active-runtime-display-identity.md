# mcodex Active Runtime Display Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Align the active runtime display identity of `mcodex` across CLI help/version, TUI onboarding, and the full first-run login flow without widening the change into a repo-wide rebrand.

**Architecture:** Add a tiny shared display-identity surface to `codex-product-identity`, then route CLI, TUI, and login copy through it while keeping full sentence assembly local to each surface. Treat the browser login pages and server-generated auth errors as part of the same first-run runtime identity path, and verify them with black-box CLI tests plus targeted login/TUI rendering coverage.

**Tech Stack:** Rust, clap, ratatui, insta snapshots, tiny_http templating in `codex-login`, `assert_cmd`, `codex_utils_cargo_bin`

---

## File Map

- `codex-rs/product-identity/src/lib.rs`
  Shared runtime display primitives for `mcodex` (`display_name`, `runtime_tagline`) alongside existing binary/home/release identity.
- `codex-rs/cli/src/main.rs`
  Top-level clap metadata and public subcommand help strings shown by `mcodex --help`.
- `codex-rs/cli/tests/runtime_display_identity.rs`
  New black-box integration coverage for `mcodex --version` and `mcodex --help`.
- `codex-rs/tui/src/onboarding/welcome.rs`
  Welcome header/tagline shown on first-run onboarding.
- `codex-rs/tui/src/onboarding/auth.rs`
  Login picker copy that currently says `use Codex`.
- `codex-rs/tui/src/onboarding/onboarding_screen.rs`
  Snapshot-driving onboarding screen tests; add auth-visible coverage here rather than scattering one-off render helpers.
- `codex-rs/tui/src/onboarding/snapshots/*.snap`
  Updated welcome/auth snapshots after the user-visible copy change.
- `codex-rs/login/src/device_code_auth.rs`
  Terminal device-code prompt; likely needs a small formatter helper so tests can assert the full rendered text.
- `codex-rs/login/src/server.rs`
  Login success/error HTML serving plus server-generated auth error copy; add a success-page renderer parallel to the existing error-page renderer.
- `codex-rs/login/src/assets/success.html`
  Browser-visible success page template; replace hardcoded `Codex` text with render-time placeholders.
- `codex-rs/login/src/assets/error.html`
  Browser-visible error page template; route title/brand copy through render-time placeholders.
- `codex-rs/login/tests/suite/login_server_e2e.rs`
  End-to-end browser login assertions for success/error identity.
- `codex-rs/login/tests/suite/device_code_login.rs`
  Integration coverage for the device-code flow if prompt rendering needs end-to-end assertion.

## Task 1: Add Shared Display Identity Primitives

**Files:**
- Modify: `codex-rs/product-identity/src/lib.rs`
- Test: `codex-rs/product-identity/src/lib.rs`

- [ ] **Step 1: Add failing unit assertions for display identity**

  Extend the existing `mcodex_identity_defines_active_and_legacy_roots` test with:

  ```rust
  assert_eq!(MCODEX.display_name, "mcodex");
  assert_eq!(
      MCODEX.runtime_tagline,
      "an OpenAI Codex-derived command-line coding agent"
  );
  ```

- [ ] **Step 2: Run the product-identity test to confirm the new fields do not exist yet**

  Run:

  ```bash
  cargo test -p codex-product-identity mcodex_identity_defines_active_and_legacy_roots
  ```

  Expected: FAIL to compile because `display_name` / `runtime_tagline` are missing.

- [ ] **Step 3: Add the minimal `ProductIdentity` fields and initialize `MCODEX`**

  Add two narrow fields:

  ```rust
  pub display_name: &'static str,
  pub runtime_tagline: &'static str,
  ```

  Initialize them on `MCODEX` with:

  ```rust
  display_name: "mcodex",
  runtime_tagline: "an OpenAI Codex-derived command-line coding agent",
  ```

  Do not add sentence-level login/help strings to `ProductIdentity`.

- [ ] **Step 4: Re-run the product-identity test**

  Run:

  ```bash
  cargo test -p codex-product-identity mcodex_identity_defines_active_and_legacy_roots
  ```

  Expected: PASS.

- [ ] **Step 5: Commit the shared identity primitive change**

  Run:

  ```bash
  git add codex-rs/product-identity/src/lib.rs
  git commit -m "feat(identity): add runtime display metadata"
  ```

## Task 2: Fix Black-Box CLI Version And Help Identity

**Files:**
- Modify: `codex-rs/cli/src/main.rs`
- Create: `codex-rs/cli/tests/runtime_display_identity.rs`

- [ ] **Step 1: Write failing black-box CLI tests for `mcodex --version` and `mcodex --help`**

  Create `codex-rs/cli/tests/runtime_display_identity.rs` with a helper that targets the `mcodex` binary:

  ```rust
  fn mcodex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
      let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("mcodex")?);
      cmd.env("MCODEX_HOME", codex_home);
      Ok(cmd)
  }
  ```

  Add tests that assert:

  ```rust
  cmd.arg("--version")
      .assert()
      .success()
      .stdout(predicates::str::contains("mcodex "))
      .stdout(predicates::str::contains("codex-cli").not());
  ```

  and

  ```rust
  cmd.arg("--help")
      .assert()
      .success()
      .stdout(predicates::str::contains("mcodex CLI"))
      .stdout(predicates::str::contains("Run Codex non-interactively.").not());
  ```

- [ ] **Step 2: Run the new CLI tests and verify they fail against current output**

  Run:

  ```bash
  cargo test -p codex-cli runtime_display_identity -- --nocapture
  ```

  Expected: FAIL because `--version` still prints `codex-cli` and public help text still contains `Codex`.

- [ ] **Step 3: Update clap-facing runtime identity in `cli/src/main.rs`**

  Make the minimum changes needed so release output presents `mcodex`:

  - keep the existing top-level parser banner and `bin_name = "mcodex"` aligned with `mcodex`
  - set the top-level clap command metadata so `--version` shows `mcodex`
  - keep crate/package identity as `codex-cli`
  - rewrite public subcommand doc comments that currently present the active product as `Codex`, for example:

  ```rust
  /// Run mcodex non-interactively.
  /// Manage external MCP servers for mcodex.
  /// Start mcodex as an MCP server (stdio).
  ```

  Limit this to public help output surfaced by `mcodex --help`.

- [ ] **Step 4: Tighten the in-file help test only if it still adds signal**

  Keep `help_uses_mcodex_binary_name` if it remains useful, but do not rely on it as the only guard. The black-box integration tests are the source of truth for this task.

- [ ] **Step 5: Re-run the targeted CLI tests**

  Run:

  ```bash
  cargo test -p codex-cli runtime_display_identity -- --nocapture
  cargo test -p codex-cli help_uses_mcodex_binary_name -- --nocapture
  cargo test -p codex-cli login_help_uses_mcodex_api_key_hint -- --nocapture
  ```

  Expected: PASS.

- [ ] **Step 6: Commit the CLI identity change**

  Run:

  ```bash
  git add codex-rs/cli/src/main.rs codex-rs/cli/tests/runtime_display_identity.rs
  git commit -m "fix(cli): align mcodex version and help identity"
  ```

## Task 3: Update TUI Onboarding Welcome And Auth Copy

**Files:**
- Modify: `codex-rs/tui/src/onboarding/welcome.rs`
- Modify: `codex-rs/tui/src/onboarding/auth.rs`
- Modify: `codex-rs/tui/src/onboarding/onboarding_screen.rs`
- Modify: `codex-rs/tui/src/onboarding/snapshots/codex_tui__onboarding__onboarding_screen__tests__pooled_only_notice_screen_initial.snap`
- Modify: `codex-rs/tui/src/onboarding/snapshots/codex_tui__onboarding__onboarding_screen__tests__pooled_paused_notice_inline_error.snap`
- Create: `codex-rs/tui/src/onboarding/snapshots/codex_tui__onboarding__onboarding_screen__tests__pooled_only_notice_screen_auth_revealed.snap`

- [ ] **Step 1: Add failing onboarding snapshot coverage for the auth-visible state**

  In `onboarding_screen.rs`, extend the existing `pooled_only_notice_starts_hidden_auth_and_reveals_it_with_l` test with:

  ```rust
  assert_snapshot!(
      "pooled_only_notice_screen_auth_revealed",
      render_to_string(&screen)
  );
  ```

  Keep the existing initial snapshot so the welcome header remains covered too.

- [ ] **Step 2: Run the onboarding snapshot tests and confirm they fail or produce `.snap.new` files**

  Run:

  ```bash
  cargo test -p codex-tui onboarding_screen::tests::pooled_only_notice_starts_hidden_auth_and_reveals_it_with_l -- --nocapture
  cargo test -p codex-tui onboarding_screen::tests::pooled_paused_resume_failure_keeps_notice_visible_with_inline_error -- --nocapture
  ```

  Expected: snapshot drift showing `Welcome to Codex` / `use Codex`.

- [ ] **Step 3: Update onboarding welcome/auth rendering to use shared display identity**

  In `welcome.rs`, replace the hardcoded welcome line with `MCODEX.display_name` and `MCODEX.runtime_tagline`.

  In `auth.rs`, replace the hardcoded product noun in the existing sentence while preserving the current login/plan logic, for example:

  ```rust
  format!(
      "Sign in with ChatGPT to use {} as part of your paid plan",
      MCODEX.display_name
  )
  ```

  Do not move sentence assembly into `codex-product-identity`.

- [ ] **Step 4: Review and accept the intended snapshot updates**

  Run:

  ```bash
  cargo insta pending-snapshots -p codex-tui
  cargo insta show -p codex-tui src/onboarding/snapshots/codex_tui__onboarding__onboarding_screen__tests__pooled_only_notice_screen_initial.snap.new
  cargo insta show -p codex-tui src/onboarding/snapshots/codex_tui__onboarding__onboarding_screen__tests__pooled_paused_notice_inline_error.snap.new
  cargo insta show -p codex-tui src/onboarding/snapshots/codex_tui__onboarding__onboarding_screen__tests__pooled_only_notice_screen_auth_revealed.snap.new
  cargo insta accept -p codex-tui
  ```

  Expected: only the onboarding snapshots above change, and each diff is limited to runtime identity copy.

- [ ] **Step 5: Re-run the targeted TUI tests**

  Run:

  ```bash
  cargo test -p codex-tui onboarding_screen::tests::pooled_only_notice_starts_hidden_auth_and_reveals_it_with_l -- --nocapture
  cargo test -p codex-tui onboarding_screen::tests::pooled_paused_resume_failure_keeps_notice_visible_with_inline_error -- --nocapture
  ```

  Expected: PASS with accepted snapshots.

- [ ] **Step 6: Commit the onboarding identity change**

  Run:

  ```bash
  git add codex-rs/tui/src/onboarding/welcome.rs codex-rs/tui/src/onboarding/auth.rs codex-rs/tui/src/onboarding/onboarding_screen.rs codex-rs/tui/src/onboarding/snapshots
  git commit -m "fix(tui): align onboarding runtime identity"
  ```

## Task 4: Update Device-Code, Browser Login Pages, And Server Auth Errors

**Files:**
- Modify: `codex-rs/login/src/device_code_auth.rs`
- Modify: `codex-rs/login/src/server.rs`
- Modify: `codex-rs/login/src/assets/success.html`
- Modify: `codex-rs/login/src/assets/error.html`
- Modify: `codex-rs/login/tests/suite/device_code_login.rs`
- Modify: `codex-rs/login/tests/suite/login_server_e2e.rs`

- [ ] **Step 1: Add failing tests for the login text surfaces**

  Add one focused unit test near `print_device_code_prompt` by first extracting a small formatter used by both runtime code and tests, for example:

  ```rust
  fn device_code_prompt_text(verification_url: &str, code: &str) -> String
  ```

  Then assert it contains:

  ```rust
  "Welcome to mcodex"
  "an OpenAI Codex-derived command-line coding agent"
  ```

  In `login_server_e2e.rs`, strengthen existing success/error tests so they assert:

  - success body contains `Signed in to mcodex`
  - success body does not contain `Signed in to Codex`
  - error bodies and terminal errors contain `mcodex` where they identify the active product

  In `server.rs` unit tests, update the entitlement error-page assertions to match the new product identity.

- [ ] **Step 2: Run the targeted login tests and verify they fail**

  Run:

  ```bash
  cargo test -p codex-login device_code_login -- --nocapture
  cargo test -p codex-login login_server_e2e -- --nocapture
  cargo test -p codex-login render_login_error_page_uses_entitlement_copy -- --nocapture
  ```

  Expected: FAIL because the current prompt/pages still say `Codex`.

- [ ] **Step 3: Route the device-code prompt through shared display identity**

  Update `device_code_auth.rs` so the printed prompt uses `MCODEX.display_name` and `MCODEX.runtime_tagline`, while keeping the existing device-code instructions unchanged.

- [ ] **Step 4: Parameterize the browser success/error pages instead of hardcoding `Codex`**

  In `server.rs`, add a success-page renderer parallel to the existing error-page renderer, for example:

  ```rust
  fn render_login_success_page() -> Vec<u8>
  ```

  Convert `success.html` and `error.html` to template-driven pages that render:

  - `MCODEX.display_name`
  - `MCODEX.runtime_tagline`

  Continue using `compile_data` assets; do not introduce a second source of branding truth inside the HTML files.

- [ ] **Step 5: Update server-generated auth error copy**

  Change `oauth_callback_error_message` and any related login failure messages so user-facing product references identify the active product as `mcodex`.

  Preserve semantic behavior:

  - known entitlement errors stay mapped to a friendly message
  - generic OAuth errors still echo the specific backend detail

- [ ] **Step 6: Re-run the targeted login tests**

  Run:

  ```bash
  cargo test -p codex-login device_code_login -- --nocapture
  cargo test -p codex-login login_server_e2e -- --nocapture
  cargo test -p codex-login oauth_access_denied_missing_entitlement_blocks_login_with_clear_error -- --nocapture
  cargo test -p codex-login oauth_access_denied_unknown_reason_uses_generic_error_page -- --nocapture
  ```

  Expected: PASS.

- [ ] **Step 7: Commit the login identity change**

  Run:

  ```bash
  git add codex-rs/login/src/device_code_auth.rs codex-rs/login/src/server.rs codex-rs/login/src/assets/success.html codex-rs/login/src/assets/error.html codex-rs/login/tests/suite/device_code_login.rs codex-rs/login/tests/suite/login_server_e2e.rs
  git commit -m "fix(login): align mcodex runtime identity"
  ```

## Task 5: Final Verification, Formatting, And Manual Smoke

**Files:**
- Modify: any files touched by formatting/lint follow-up only

- [ ] **Step 1: Run crate-targeted automated tests**

  Run:

  ```bash
  cargo test -p codex-product-identity
  cargo test -p codex-cli
  cargo test -p codex-login
  cargo test -p codex-tui
  ```

  Expected: PASS.

- [ ] **Step 2: Run formatter after Rust changes**

  Run:

  ```bash
  cd codex-rs && just fmt
  ```

  Expected: no formatting drift remains.

- [ ] **Step 3: Run scoped clippy autofix passes**

  Run:

  ```bash
  cd codex-rs && just fix -p codex-product-identity
  cd codex-rs && just fix -p codex-cli
  cd codex-rs && just fix -p codex-login
  cd codex-rs && just fix -p codex-tui
  ```

  Expected: PASS or only unrelated pre-existing warnings that are documented before continuing.

- [ ] **Step 4: Build and smoke the release binary**

  Run:

  ```bash
  cd codex-rs && cargo build --release --bin mcodex
  cd codex-rs && ./target/release/mcodex --version
  cd codex-rs && ./target/release/mcodex --help
  ```

  Expected:

  - `--version` shows `mcodex`
  - `--help` uses `mcodex` across public help output

- [ ] **Step 5: Re-run the first-run login smoke**

  Use an isolated `HOME` / `MCODEX_HOME` and verify:

  - TTY onboarding shows `Welcome to mcodex`
  - auth picker says `use mcodex`
  - device-code prompt says `Welcome to mcodex`
  - browser success/error pages no longer identify the active product as `Codex`

- [ ] **Step 6: Commit any verification-only follow-up**

  If formatter, snapshot acceptance, or small verification fixes changed tracked files, run:

  ```bash
  git add -A
  git commit -m "test(identity): verify runtime display identity"
  ```

  If the worktree is already clean after the earlier commits, skip this step.
