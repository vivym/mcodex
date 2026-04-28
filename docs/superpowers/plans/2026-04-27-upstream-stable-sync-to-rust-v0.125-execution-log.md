# Upstream Stable Sync to rust-v0.125.0 Execution Log

## Ref Preflight

- `origin/sync/rust-v0.122.0`: `87ad4651fd1f18e135fb2c4d501f802103eda9d3`; plan-time expected `87ad4651fd1f18e135fb2c4d501f802103eda9d3`
- `origin/main`: `9914df948b714c1b4d854ec97a3c72e430a580e5`; plan-time expected `9914df948b714c1b4d854ec97a3c72e430a580e5`
- `rust-v0.123.0`: tag object `d1005e4215bbbfb295ecd154883161d4f63d2f6e`, commit `0785b66228dff87f891e291cb5686631865b6922`; plan-time expected tag object `d1005e4215bbbfb295ecd154883161d4f63d2f6e`, commit `0785b66228dff87f891e291cb5686631865b6922`
- `rust-v0.124.0`: tag object `e93a08390bf1350a7a7b128bd4310fd1429e6651`, commit `e9fb49366c93a1478ec71cc41ecee415a197d036`; plan-time expected tag object `e93a08390bf1350a7a7b128bd4310fd1429e6651`, commit `e9fb49366c93a1478ec71cc41ecee415a197d036`
- `rust-v0.125.0`: tag object `7d8152a5d74226ddaac12f93f7c5ed3f33a60d2a`, commit `637f7dd6d737f3961e6bf32fbb3861c4953269c5`; plan-time expected tag object `7d8152a5d74226ddaac12f93f7c5ed3f33a60d2a`, commit `637f7dd6d737f3961e6bf32fbb3861c4953269c5`

## Checkpoints

### sync/rust-v0.123.0

- Base: `87ad4651fd1f18e135fb2c4d501f802103eda9d3`
- Upstream tag: `rust-v0.123.0`
- Conflict files:
  - `codex-rs/Cargo.lock`
  - `codex-rs/Cargo.toml`
  - `codex-rs/analytics/src/client.rs`
  - `codex-rs/app-server-protocol/schema/typescript/ClientRequest.ts`
  - `codex-rs/app-server-protocol/schema/typescript/ServerNotification.ts`
  - `codex-rs/app-server/src/codex_message_processor.rs`
  - `codex-rs/app-server/tests/suite/v2/realtime_conversation.rs`
  - `codex-rs/cloud-requirements/src/lib.rs`
  - `codex-rs/codex-api/src/endpoint/realtime_websocket/methods.rs`
  - `codex-rs/core/src/config_loader/macos.rs`
  - `codex-rs/core/src/config_loader/mod.rs`
  - `codex-rs/core/src/context_manager/updates.rs`
  - `codex-rs/core/src/guardian/tests.rs`
  - `codex-rs/core/src/realtime_conversation_tests.rs`
  - `codex-rs/core/src/session/handlers.rs`
  - `codex-rs/core/src/session/session.rs`
  - `codex-rs/core/src/stream_events_utils.rs`
  - `codex-rs/core/src/tasks/mod_tests.rs`
  - `codex-rs/core/src/tools/handlers/js_repl.rs`
  - `codex-rs/core/src/tools/js_repl/mod.rs`
  - `codex-rs/state/src/runtime/threads.rs`
  - `codex-rs/tui/src/app.rs`
  - `codex-rs/tui/src/bottom_pane/snapshots/codex_tui__bottom_pane__title_setup__tests__terminal_title_setup_basic.snap`
  - `codex-rs/tui/src/bottom_pane/status_line_setup.rs`
  - `codex-rs/tui/src/bottom_pane/title_setup.rs`
  - `codex-rs/tui/src/debug_config.rs`
  - `codex-rs/tui/src/snapshots/codex_tui__app__tests__model_migration_prompt_shows_for_hidden_model.snap`
- Regenerated files:
  - `codex-rs/core/config.schema.json`
  - `codex-rs/app-server-protocol/schema/json/**`
  - `codex-rs/app-server-protocol/schema/typescript/**`
  - `sdk/python/src/codex_app_server/generated/**`
  - `MODULE.bazel.lock`
- Commands run:
  - `just write-config-schema`
  - `just write-app-server-schema`
  - `just write-app-server-schema --experimental`
  - `just bazel-lock-update`
  - `just bazel-lock-check`
  - `just fmt`
  - `cargo check -p codex-core --lib`
  - `cargo test -p codex-core --lib --no-run`
  - `cargo test -p codex-app-server --no-run`
  - touched crate gate: `cargo test -p <crate> --no-run` for all crates in `/tmp/sync-rust-v0.123.0-touched-crates.txt`
  - focused tests:
    - `cargo test -p codex-app-server merge_persisted_resume_metadata`
    - `cargo test -p codex-app-server source_thread_config_baseline`
    - `cargo test -p codex-app-server runtime_update_notification_keeps_live_fields_when_startup_suppression_is_cleared`
    - `cargo test -p codex-protocol resumed_history_prefers_matching_session_meta_for_thread_start_metadata`
    - `cargo test -p codex-tui app::tests::enqueue_primary_thread_session_replays_turns_before_initial_prompt_submit`
    - `cargo test -p codex-tui app::tests::inactive_thread_started_notification_initializes_replay_session`
    - `cargo test -p codex-tui app::tests::replace_chat_widget_reseeds_collab_agent_metadata_for_replay`
    - `cargo test -p codex-tui onboarding::onboarding_screen::tests::pooled_default_notice_enter_reveals_auth`
  - scoped lint fix: `just fix -p codex-app-server -p codex-tui -p codex-protocol`
  - integrity checks:
    - `git diff --check`
    - `git diff --cached --check`
    - `git diff --name-only --diff-filter=U`
    - `git grep -n -E '^(<<<<<<<|>>>>>>>)($|[[:space:]])' -- .` (no matches)
    - migration prefix duplicate scan (no duplicates)
    - `cargo insta pending-snapshots --manifest-path tui/Cargo.toml`
- Deferred coverage:
  - Workspace-wide `cargo test` / `just test` intentionally not run because local disk was down to ~14 GiB after scoped gates.
  - Full `cargo test -p codex-tui` intentionally not run; `--no-run`, no pending snapshots, and focused TUI tests covered the local fixture fixes.
  - Remote/CI should still run full matrix before promoting the final `sync/rust-v0.125.0` branch.
- Commit: pending local checkpoint commit; record with `git rev-parse HEAD` after commit creation

### sync/rust-v0.124.0

- Base: fill from Task 3 Step 2 command output
- Upstream tag: `rust-v0.124.0`
- Conflict files: fill from Task 3 Step 3 command output
- Regenerated files: fill during Task 3 Step 5
- Commands run: fill during Task 3 Step 6
- Deferred coverage: fill during Task 3 Step 6
- Commit: fill from Task 3 Step 7 command output

### sync/rust-v0.125.0

- Base: fill from Task 4 Step 2 command output
- Upstream tag: `rust-v0.125.0`
- Conflict files: fill from Task 4 Step 3 command output
- Regenerated files: fill during Task 4 Step 5
- Commands run: fill during Task 4 Step 6
- Deferred coverage: fill during Task 4 Step 6
- Commit: fill from Task 4 Step 7 command output

## Final origin/main Reconciliation

- Fetched `origin/main`: fill from Task 5 Step 1 command output
- Already contained or merged: fill from Task 5 Step 2 or Step 3 result
- Reconciliation base: fill from Task 5 Step 3 if merge occurs
- Conflict files: fill from Task 5 Step 3 if merge occurs
- Commands run: fill during Task 5 Step 6
- Deferred coverage: fill during Task 5 Step 6
- Commit: fill from Task 5 Step 7 command output

## Final Status

- Final branch: `sync/rust-v0.125.0`
- Final pushed commit: fill from Task 6 Step 4
- Contains `rust-v0.125.0^{commit}`:
- Contains final fetched `origin/main`:
- Ready for PR or merge:
