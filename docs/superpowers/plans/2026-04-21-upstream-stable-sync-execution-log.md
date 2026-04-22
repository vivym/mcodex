# Upstream Stable Sync Execution Log

Spec: docs/superpowers/specs/2026-04-21-upstream-stable-sync-design.md

Plan: docs/superpowers/plans/2026-04-21-upstream-stable-sync-implementation.md

## Targets

- Checkpoint tag: rust-v0.121.0
- Final target tag: rust-v0.122.0

## Preflight

- Upstream remote: `https://github.com/openai/codex.git`
- rust-v0.121.0 commit: `d65ed92a5e440972626965d0af9a6345179783bc`
- rust-v0.122.0 commit: `230dcadee609fa99d6162fe1107457030e5270a7`
- main start commit: `020070e2a798a6dc4362b301e31d9a8b790aeee8`
- 0.121 merge base: `34a9ca083ee1e3ad478e51465e8a7fcfeabb1813`
- 0.122 merge base: `34a9ca083ee1e3ad478e51465e8a7fcfeabb1813`

## Conflict Decisions

## Commands Run

## Dry-Run Conflict Summary

### rust-v0.121.0
- Conflict count: 15
- High-risk groups: `codex-rs/Cargo.lock`; app-server message processor plus MCP/realtime test paths; `codex-rs/core-skills` loader and loader tests; core `codex.rs`, state service, and realtime/view-image test paths; TUI `app.rs`, `app_server_session.rs`, and `chatwidget.rs`

### rust-v0.122.0
- Conflict count: 48
- High-risk groups: `codex-rs/Cargo.lock` and Rust manifests; app-server protocol schemas, README, message processor, and v2 test paths; CLI `main.rs` and login flow; core client/config-loader/plugin/task/state/session paths plus `codex-rs/core/src/codex.rs` delete-vs-modify; login auth/test paths; state crate lib; TUI app/adapter/session/chatwidget/debug/update/onboarding/status/tooltips paths; `docs/config.md`; install scripts

## Deferred Non-Core Follow-Ups

## Final Artifact Checklist

- [ ] Cargo.lock reviewed or regenerated
- [ ] MODULE.bazel.lock refreshed when dependencies changed
- [ ] config schema regenerated when config types changed
- [ ] app-server schemas regenerated when protocol changed
- [ ] TUI snapshots reviewed and accepted when UI changed
- [ ] release/update/install paths checked for mcodex/OSS behavior
- [ ] full workspace test run locally or deferred to required CI with approval

## rust-v0.121.0 Checkpoint

- Merge command: git merge --no-ff --no-commit rust-v0.121.0
- Merge started at: 2026-04-21 20:10:22 +0800; base HEAD 020070e2a798a6dc4362b301e31d9a8b790aeee8; target rust-v0.121.0 tag b3442f5e856cf4daa3e168128af8ee4bff30b0f4, peeled commit d65ed92a5e440972626965d0af9a6345179783bc
- Unresolved conflicts:
  - codex-rs/Cargo.lock
  - codex-rs/app-server/src/message_processor.rs
  - codex-rs/app-server/tests/common/mcp_process.rs
  - codex-rs/app-server/tests/suite/v2/realtime_conversation.rs
  - codex-rs/core-skills/src/loader.rs
  - codex-rs/core-skills/src/loader_tests.rs
  - codex-rs/core/src/codex.rs
  - codex-rs/core/src/codex_tests.rs
  - codex-rs/core/src/realtime_conversation_tests.rs
  - codex-rs/core/src/state/service.rs
  - codex-rs/core/tests/suite/realtime_conversation.rs
  - codex-rs/core/tests/suite/view_image.rs
  - codex-rs/tui/src/app.rs
  - codex-rs/tui/src/app_server_session.rs
  - codex-rs/tui/src/chatwidget.rs

## Task 4 - App-Server Conflict Resolution

- Resolved and staged targets:
  - codex-rs/app-server/src/message_processor.rs
  - codex-rs/app-server/tests/common/mcp_process.rs
  - codex-rs/app-server/tests/suite/v2/realtime_conversation.rs
- `message_processor.rs`: kept upstream's `Arc<ConnectionSessionState>` / `InitializedConnectionSessionState` initialization model and split initialized request dispatch, while preserving fork transport propagation so `AccountLeaseRead` and `AccountLeaseResume` still receive the actual `AppServerTransport`.
- `mcp_process.rs`: kept fork `MCODEX_HOME` baseline and reduced child log noise with `RUST_LOG=warn`, merged upstream managed-config isolation, and restored `CODEX_HOME` in the helper baseline after quality review identified command-exec inheritance risk.
- `realtime_conversation.rs`: kept upstream multipart realtime call-create coverage and retained fork JSON-semantic comparison of the `session` part via `serde_json::Value` to avoid brittle key-order assertions.
- Verification:
  - `rustfmt codex-rs/app-server/tests/common/mcp_process.rs`
  - `git diff --check -- codex-rs/app-server/src/message_processor.rs codex-rs/app-server/tests/common/mcp_process.rs codex-rs/app-server/tests/suite/v2/realtime_conversation.rs`
  - `cargo test -p codex-app-server command_exec_env_overrides_merge_with_server_environment_and_support_unset -- --exact` blocked by unrelated third-party dependency build failure in `temporal_rs` / `icu_calendar`
- Review:
  - Spec review passed.
  - Quality review initially flagged `CODEX_HOME` removal as a high-risk regression for command-exec baseline env inheritance; fixed by exporting both `MCODEX_HOME` and `CODEX_HOME`.
  - Focused re-review passed after the fix.

## Task 5 - Core-Skills Conflict Resolution

- Resolved and staged targets:
  - codex-rs/core-skills/src/loader.rs
  - codex-rs/core-skills/src/loader_tests.rs
- `loader.rs`: preserved upstream async filesystem support via `ExecutorFileSystem` / `LOCAL_FS` while keeping mcodex test-only product identity coverage through `MCODEX`.
- `loader_tests.rs`: preserved fork mcodex home/admin-root assertions and converted the affected tests/helpers to upstream's async API.
- Quality fix: a focused review found that remote/non-local filesystem symlink aliases could be followed with raw-path identity when local `canonicalize()` failed. The loader now only follows symlink directories when canonicalization succeeds; otherwise it safely skips that symlink directory. Ordinary remote root/dir/file identities still fall back to raw paths.
- Added regression coverage with a fake remote filesystem: `remote_fs_skips_symlinked_subdir_when_local_canonicalize_is_unavailable`.
- Verification:
  - `rustfmt codex-rs/core-skills/src/loader.rs codex-rs/core-skills/src/loader_tests.rs`
  - `git diff --check -- codex-rs/core-skills/src/loader.rs codex-rs/core-skills/src/loader_tests.rs`
  - `cargo test -p codex-core-skills remote_fs_skips_symlinked_subdir_when_local_canonicalize_is_unavailable`
  - `cargo test -p codex-core-skills` passed: 82 passed, 0 failed
- Review:
  - Initial spec review passed.
  - Initial quality review flagged the remote symlink identity gap; fixed in this task.
  - Focused spec/quality re-review passed after the fix.
  - Residual risk: remote symlinked skill directories are skipped unless the loader can obtain a canonical local path; supporting remote canonical symlink identity would require a broader filesystem API/protocol extension.

## Task 6 - Core Runtime Conflict Resolution

- Resolved and staged targets:
  - codex-rs/core/src/codex.rs
  - codex-rs/core/src/codex_tests.rs
  - codex-rs/core/src/realtime_conversation_tests.rs
  - codex-rs/core/src/state/service.rs
  - codex-rs/core/tests/suite/realtime_conversation.rs
  - codex-rs/core/tests/suite/view_image.rs
- `codex.rs`: kept the 0.121 checkpoint as a temporary compatibility point while preserving fork `account_pool_manager` / `lease_auth` shutdown semantics and upstream agent-identity startup/reload behavior.
- Startup behavior fix:
  - initial `ensure_registered_identity()` failure now aborts `Session::new` before `SessionConfigured`
  - auth-state watcher subscribes before the initial ensure call
  - async auth reload failures emit an error event and trigger shutdown after `SessionConfigured`
- Shutdown behavior fix:
  - `Session` owns a `shutdown_requested: CancellationToken`
  - `Session` no longer owns `tx_sub`; only `Codex` retains the submission sender
  - the auth-state watcher exits on `shutdown_requested.cancelled()`
  - `handlers::shutdown()` cancels `shutdown_requested` so normal shutdown and fatal identity-registration shutdown both terminate the watcher and submission loop consistently
  - `submission_loop()` exits on either `Op::Shutdown` or `shutdown_requested`, then still drains guardian/account-pool shutdown paths
- `codex_tests.rs`: added/updated coverage for:
  - initial agent-identity registration failure before `SessionConfigured`
  - async agent-identity registration failure after auth reload
  - `fail_agent_identity_registration()` canceling shutdown and emitting shutdown
  - submission loop exit on direct shutdown-token cancellation
  - submission loop exit on normal `Op::Shutdown`
- Verification:
  - `rustfmt codex-rs/core/src/codex.rs codex-rs/core/src/codex_tests.rs`
  - `git diff --check -- codex-rs/core/src/codex.rs codex-rs/core/src/codex_tests.rs`
  - `cargo test -p codex-core submission_loop_shutdown_op_cancels_shutdown_requested --lib` blocked by unrelated third-party dependency build failure in `temporal_rs` / `icu_calendar`
- Review:
  - earlier focused review found two lifecycle regressions in an intermediate resolution: the auth-state watcher had no shutdown exit path, and normal `handlers::shutdown()` did not cancel the shutdown token
  - both issues were fixed locally before restaging
  - final `gpt-5.4 xhigh` focused re-review was re-dispatched after quota reset and is pending

## Task 7 - TUI Conflict Resolution

- Resolved and staged targets:
  - codex-rs/tui/src/app.rs
  - codex-rs/tui/src/app_server_session.rs
  - codex-rs/tui/src/chatwidget.rs
- Kept upstream 0.121 TUI runtime/session behavior while preserving fork startup access, pooled status presentation, `MCODEX` identity handling, `MergeStrategy`, and `MemoryResetResponse`.
- Conflict cleanup stayed local to the existing large module boundary in `chatwidget.rs`; no unrelated extraction was introduced in this checkpoint.
- Verification:
  - `rustfmt codex-rs/tui/src/app.rs codex-rs/tui/src/app_server_session.rs codex-rs/tui/src/chatwidget.rs`
  - `git diff --check -- codex-rs/tui/src/app.rs codex-rs/tui/src/app_server_session.rs codex-rs/tui/src/chatwidget.rs`
- Review:
  - focused TUI review passed after removing an unused `url::Url` import introduced during conflict cleanup

## rust-v0.121.0 Checkpoint Gate Progress

- Lockfile / conflict hygiene:
  - `cargo generate-lockfile`
  - `rg -n '^(<<<<<<<|>>>>>>>|=======$)' --glob '!target/**' .` returned no matches
- New gate failures found and fixed:
  - `cargo test -p codex-account-pool` first failed because `account-pool/src/backend/local/control.rs` still initialized `AuthDotJson` without the newly required `agent_identity` field
  - added TDD regression coverage in `codex-rs/account-pool/tests/lease_lifecycle.rs` asserting pooled backend-private auth persists `agent_identity: None`
  - fixed `account-pool/src/backend/local/control.rs` by explicitly writing `agent_identity: None`
  - full `cargo test -p codex-account-pool` then exposed a second root cause: duplicate state migration version `0025`
  - added `runtime::tests::state_migration_versions_are_unique` in `codex-rs/state/src/runtime.rs`
  - renumbered upstream thread timestamp migration from `codex-rs/state/migrations/0025_thread_timestamps_millis.sql` to `codex-rs/state/migrations/0033_thread_timestamps_millis.sql` so it lands after fork account-pool migrations
- Gate verification now passing:
  - `cargo test -p codex-account-pool` passed
  - `cargo test -p codex-state state_migration_versions_are_unique` passed
  - `cargo test -p codex-state` passed
  - `cargo test -p codex-app-server-protocol` passed
- Remaining gate blocker:
  - `cargo test -p codex-login` blocked by third-party dependency mismatch in `temporal_rs 0.1.2` against `icu_calendar`
  - `cargo test -p codex-app-server` blocked by the same `temporal_rs` / `icu_calendar` mismatch
  - `cargo test -p codex-core submission_loop_shutdown_op_cancels_shutdown_requested --lib` blocked by the same `temporal_rs` / `icu_calendar` mismatch
  - `cargo test -p codex-tui status_line_model_with_reasoning_context_remaining_percent_footer -- --exact` blocked by the same `temporal_rs` / `icu_calendar` mismatch
- Notes:
  - this blocker reproduces after a fresh `cargo generate-lockfile`, so it is not caused by unresolved merge markers
  - the state/account-pool fixes above are local checkpoint regressions and are already verified independently of the `temporal_rs` blocker

## Task 8 - Core Gate Follow-Up

- Resolved, focused-reviewed, and restaged:
  - `codex-rs/core/tests/common/lib.rs`
  - `codex-rs/core/tests/suite/exec.rs`
  - `codex-rs/core/tests/suite/unified_exec.rs`
  - `codex-rs/core/tests/suite/account_pool.rs`
- `core/tests/common/lib.rs`: added `resolved_python_executable()` so macOS seatbelt tests resolve the real interpreter path from `sys.executable` before entering seatbelt, instead of invoking the `/usr/bin/python3` CommandLineTools shim inside the sandbox.
- `exec.rs` / `unified_exec.rs`: switched the macOS seatbelt Python tests to use the resolved interpreter path and quoted the unified-exec startup command with `shlex::try_join(...)` so spaces in the absolute interpreter path do not break the command line.
- `account_pool.rs`: replaced timing-sensitive `min_switch_interval_secs = 3/5` test values with `300` in the soft-pressure suppression coverage so the integration tests keep exercising the suppression path instead of accidentally becoming rotation-allowed once a slow turn crosses the minimum-interval threshold.
- `account_pool.rs`: renamed `stale_soft_pressure_clears_after_window_without_forcing_rotation` to `soft_pressure_clears_on_subsequent_low_pressure_turn_without_forcing_rotation` and removed the sleep because the integration test is validating soft-pressure clearing on a later low-pressure turn, not the wall-clock staleness timeout itself.
- Verification:
  - `cargo test -p codex-core suite::exec::openpty_works_under_real_exec_seatbelt_path -- --exact --nocapture`
  - `cargo test -p codex-core suite::unified_exec::unified_exec_python_prompt_under_seatbelt -- --exact --nocapture`
  - `cargo test -p codex-core suite::account_pool:: -- --nocapture`
  - `cargo test -p codex-core`
- Gate status update:
  - with the current merged worktree and lockfile state, `cargo test -p codex-core` passed fully: lib tests `1622 passed, 0 failed, 3 ignored`; integration tests `967 passed, 0 failed, 13 ignored`; `responses_headers` `4 passed, 0 failed`
  - the earlier `temporal_rs` / `icu_calendar` blocker no longer reproduced on the `codex-core` package path in this worktree state
- Post-verification hygiene:
  - `just fmt`
  - `just fix -p codex-core`
  - `just fix -p codex-cli`
  - per repo instructions, tests were not rerun after `fmt` / `fix`
- Focused review:
  - `gpt-5.4 xhigh` review of the Python seatbelt helper, account-pool timing fixes, and shutdown-path updates: no findings
  - `gpt-5.4 xhigh` review of the websocket / rmcp / pooled-auth test updates: no findings
  - `gpt-5.4 xhigh` review of the `cli_stream` test relocation into `codex-cli`: no findings
