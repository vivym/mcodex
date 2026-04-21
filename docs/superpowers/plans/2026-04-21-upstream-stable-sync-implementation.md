# Upstream Stable Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sync the `mcodex` fork to upstream stable `rust-v0.122.0` while preserving the approved fork core runtime contract.

**Architecture:** Use a two-stage stable checkpoint sync: first merge upstream `rust-v0.121.0` into an internal checkpoint branch, then merge `rust-v0.122.0` from that checkpoint. Resolve conflicts by contract ownership rather than by broad `ours` or `theirs`, regenerate derived artifacts from source, and gate the final branch with focused crate tests plus required shared-crate and release/update/install checks.

**Tech Stack:** Git worktrees, Rust/Cargo, `just`, Bazel lockfile tooling, app-server schema generation, `insta` snapshots, GitHub Actions release workflows, POSIX shell, PowerShell.

---

## Source Spec

Spec: `docs/superpowers/specs/2026-04-21-upstream-stable-sync-design.md`

Relevant required skills during execution:

```text
@superpowers:subagent-driven-development
@superpowers:executing-plans
@superpowers:systematic-debugging
@superpowers:verification-before-completion
```

Repo rules that matter for this plan:

```text
Do not modify CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR or CODEX_SANDBOX_ENV_VAR code.
Run just fmt from codex-rs after Rust code changes.
Run scoped cargo test commands for changed crates.
Run just fix -p <crate> before finalizing large codex-rs changes.
If Cargo.toml or Cargo.lock changes, run just bazel-lock-update and just bazel-lock-check from repo root.
If ConfigToml or nested config types change, run just write-config-schema from codex-rs.
If app-server protocol shapes change, run just write-app-server-schema from codex-rs, plus --experimental when experimental fixtures are affected.
Ask before running the full workspace cargo test or just test locally.
```

## File Structure

Create during execution:

- `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`
  Responsibility: record merge bases, conflict counts, conflict-resolution decisions, commands run, deferred non-core follow-ups, and final artifact checklist.
  Create this tracked file first inside `.worktrees/sync-rust-v0.121.0-base`; do not commit it from the maintainer's active checkout.

Branches and worktrees:

- `.worktrees/sync-rust-v0.121.0-base`
  Responsibility: internal checkpoint branch for upstream `rust-v0.121.0`.
- `.worktrees/sync-rust-v0.122.0`
  Responsibility: final target branch for upstream `rust-v0.122.0`.

Expected `rust-v0.121.0` conflict paths:

```text
codex-rs/Cargo.lock
codex-rs/app-server/src/message_processor.rs
codex-rs/app-server/tests/common/mcp_process.rs
codex-rs/app-server/tests/suite/v2/realtime_conversation.rs
codex-rs/core-skills/src/loader.rs
codex-rs/core-skills/src/loader_tests.rs
codex-rs/core/src/codex.rs
codex-rs/core/src/codex_tests.rs
codex-rs/core/src/realtime_conversation_tests.rs
codex-rs/core/src/state/service.rs
codex-rs/core/tests/suite/realtime_conversation.rs
codex-rs/core/tests/suite/view_image.rs
codex-rs/tui/src/app.rs
codex-rs/tui/src/app_server_session.rs
codex-rs/tui/src/chatwidget.rs
```

Expected additional or changed `rust-v0.122.0` conflict paths:

```text
codex-rs/Cargo.toml
codex-rs/app-server-protocol/schema/json/ClientRequest.json
codex-rs/app-server-protocol/schema/typescript/ClientRequest.ts
codex-rs/app-server-protocol/schema/typescript/ServerNotification.ts
codex-rs/app-server/README.md
codex-rs/app-server/tests/suite/v2/command_exec.rs
codex-rs/app-server/tests/suite/v2/turn_start.rs
codex-rs/cli/src/login.rs
codex-rs/cli/src/main.rs
codex-rs/core/Cargo.toml
codex-rs/core/src/client.rs
codex-rs/core/src/client_tests.rs
codex-rs/core/src/codex.rs
codex-rs/core/src/config_loader/layer_io.rs
codex-rs/core/src/config_loader/mod.rs
codex-rs/core/src/guardian/tests.rs
codex-rs/core/src/mcp_openai_file.rs
codex-rs/core/src/plugins/manager.rs
codex-rs/core/src/session/tests.rs
codex-rs/core/src/state/service.rs
codex-rs/core/src/tasks/compact.rs
codex-rs/core/tests/suite/realtime_conversation.rs
codex-rs/core/tests/suite/view_image.rs
codex-rs/exec-server/tests/exec_process.rs
codex-rs/login/src/auth/mod.rs
codex-rs/login/tests/suite/mod.rs
codex-rs/rmcp-client/src/lib.rs
codex-rs/state/src/lib.rs
codex-rs/tui/src/app/app_server_adapter.rs
codex-rs/tui/src/debug_config.rs
codex-rs/tui/src/history_cell.rs
codex-rs/tui/src/onboarding/onboarding_screen.rs
codex-rs/tui/src/slash_command.rs
codex-rs/tui/src/tooltips.rs
codex-rs/tui/src/update_action.rs
docs/config.md
scripts/install/install.ps1
scripts/install/install.sh
```

Generated paths to regenerate instead of hand-resolving as final content:

```text
codex-rs/Cargo.lock
MODULE.bazel.lock
codex-rs/core/config.schema.json
codex-rs/app-server-protocol/schema/json/ClientRequest.json
codex-rs/app-server-protocol/schema/json/ServerNotification.json
codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.schemas.json
codex-rs/app-server-protocol/schema/json/codex_app_server_protocol.v2.schemas.json
codex-rs/app-server-protocol/schema/typescript/ClientRequest.ts
codex-rs/app-server-protocol/schema/typescript/ServerNotification.ts
codex-rs/app-server-protocol/schema/typescript/v2/index.ts
codex-rs/tui/src/**/*.snap
```

## Task 1: Preflight and Isolated Workspace

**Files:**
- Prepare scratch content for: `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`
- Read: `docs/superpowers/specs/2026-04-21-upstream-stable-sync-design.md`
- Read: `AGENTS.md`

- [ ] **Step 1: Confirm the active checkout is clean enough to start**

Run:

```bash
git status --short --branch
git worktree list
```

Expected: current branch may be ahead with docs commits, but there are no unstaged source changes. If unrelated user changes exist, stop and ask before continuing.

- [ ] **Step 2: Configure upstream remote if missing**

Run:

```bash
current_upstream="$(git remote get-url upstream 2>/dev/null || true)"
if [ -z "${current_upstream}" ]; then
  git remote add upstream https://github.com/openai/codex.git
else
  test "${current_upstream}" = "https://github.com/openai/codex.git" || \
    test "${current_upstream}" = "git@github.com:openai/codex.git"
fi
```

Expected: `upstream` exists and points to `openai/codex`. If it points elsewhere, stop and fix the remote before continuing.

- [ ] **Step 3: Fetch upstream stable tags through the local proxy when needed**

Run:

```bash
HTTPS_PROXY=http://127.0.0.1:7897 \
HTTP_PROXY=http://127.0.0.1:7897 \
ALL_PROXY=http://127.0.0.1:7897 \
git fetch upstream --force --tags
```

Expected: fetch succeeds. If the proxy is unavailable, retry with the user's current proxy settings.

- [ ] **Step 4: Verify upstream tags match local tag objects and record merge bases**

Run:

```bash
test "$(git remote get-url upstream)" = "https://github.com/openai/codex.git" || \
  test "$(git remote get-url upstream)" = "git@github.com:openai/codex.git"

HTTPS_PROXY=http://127.0.0.1:7897 \
HTTP_PROXY=http://127.0.0.1:7897 \
ALL_PROXY=http://127.0.0.1:7897 \
git ls-remote --tags upstream \
  "refs/tags/rust-v0.121.0^{}" \
  "refs/tags/rust-v0.122.0^{}"

test "$(git rev-parse --verify rust-v0.121.0^{})" = "$(
  HTTPS_PROXY=http://127.0.0.1:7897 \
  HTTP_PROXY=http://127.0.0.1:7897 \
  ALL_PROXY=http://127.0.0.1:7897 \
  git ls-remote --tags upstream "refs/tags/rust-v0.121.0^{}" | awk 'NR==1 {print $1}'
)"
test "$(git rev-parse --verify rust-v0.122.0^{})" = "$(
  HTTPS_PROXY=http://127.0.0.1:7897 \
  HTTP_PROXY=http://127.0.0.1:7897 \
  ALL_PROXY=http://127.0.0.1:7897 \
  git ls-remote --tags upstream "refs/tags/rust-v0.122.0^{}" | awk 'NR==1 {print $1}'
)"
git merge-base HEAD rust-v0.121.0
git merge-base HEAD rust-v0.122.0
```

Expected: `upstream` still resolves to `openai/codex`, the local `rust-v0.121.0` and `rust-v0.122.0` tag objects exactly match upstream's tag objects, and both merge-base commands print a commit hash. If any tag mismatch appears, stop and repair local refs before continuing.

- [ ] **Step 5: Prepare the execution log template outside the active checkout**

Create `/tmp/mcodex-upstream-stable-sync-execution-log.md`:

```markdown
# Upstream Stable Sync Execution Log

Spec: docs/superpowers/specs/2026-04-21-upstream-stable-sync-design.md
Plan: docs/superpowers/plans/2026-04-21-upstream-stable-sync-implementation.md

## Targets

- Checkpoint tag: rust-v0.121.0
- Final target tag: rust-v0.122.0

## Preflight

- Upstream remote:
- rust-v0.121.0 commit:
- rust-v0.122.0 commit:
- main start commit:
- 0.121 merge base:
- 0.122 merge base:

## Conflict Decisions

## Commands Run

## Deferred Non-Core Follow-Ups

## Final Artifact Checklist

- [ ] Cargo.lock reviewed or regenerated
- [ ] MODULE.bazel.lock refreshed when dependencies changed
- [ ] config schema regenerated when config types changed
- [ ] app-server schemas regenerated when protocol changed
- [ ] TUI snapshots reviewed and accepted when UI changed
- [ ] release/update/install paths checked for mcodex/OSS behavior
- [ ] full workspace test run locally or deferred to required CI with approval
```

Expected: the scratch template exists outside the repository so the maintainer's active checkout stays unchanged.

- [ ] **Step 6: Fill the preflight section**

Run:

```bash
{
  echo "- Upstream remote: $(git remote get-url upstream)"
  echo "- rust-v0.121.0 commit: $(git rev-parse rust-v0.121.0^{})"
  echo "- rust-v0.122.0 commit: $(git rev-parse rust-v0.122.0^{})"
  echo "- main start commit: $(git rev-parse HEAD)"
  echo "- 0.121 merge base: $(git merge-base HEAD rust-v0.121.0)"
  echo "- 0.122 merge base: $(git merge-base HEAD rust-v0.122.0)"
}
```

Expected: record these exact values in `/tmp/mcodex-upstream-stable-sync-execution-log.md` or another scratch note so they can be copied into the tracked log after the sync worktree exists.

- [ ] **Step 7: Verify the active checkout remains untouched**

Run:

```bash
git status --short --branch
```

Expected: no new tracked sync commit is created on the active checkout. If a tracked execution-log file was created in the active checkout by mistake, remove it before continuing.

## Task 2: Dry-Run Conflict Manifests

**Files:**
- Modify scratch notes only: `/tmp/mcodex-upstream-stable-sync-execution-log.md`

- [ ] **Step 1: Record the 0.121 dry-run conflict manifest**

Run:

```bash
git merge-tree --write-tree --name-only --messages HEAD rust-v0.121.0 \
  > /tmp/mcodex-merge-tree-0.121.txt 2>&1 || true
rg '^(CONFLICT|Auto-merging)' /tmp/mcodex-merge-tree-0.121.txt
```

Expected: output includes conflicts in `codex-rs/Cargo.lock`, `codex-rs/core/src/codex.rs`, app-server tests, core-skills, core state, and TUI files.

- [ ] **Step 2: Record the 0.122 dry-run conflict manifest**

Run:

```bash
git merge-tree --write-tree --name-only --messages HEAD rust-v0.122.0 \
  > /tmp/mcodex-merge-tree-0.122.txt 2>&1 || true
rg '^(CONFLICT|Auto-merging)' /tmp/mcodex-merge-tree-0.122.txt
```

Expected: output includes the 0.121 conflict families plus app-server schemas, CLI entry points, core client/config/plugin/task files, login auth, state, TUI update files, docs, and install scripts.

- [ ] **Step 3: Add a conflict summary to the scratch execution log**

Add a `## Dry-Run Conflict Summary` section to `/tmp/mcodex-upstream-stable-sync-execution-log.md`:

```markdown
## Dry-Run Conflict Summary

### rust-v0.121.0

- Conflict count:
- High-risk groups:

### rust-v0.122.0

- Conflict count:
- High-risk groups:
```

Fill counts with:

```bash
rg '^CONFLICT' /tmp/mcodex-merge-tree-0.121.txt | wc -l
rg '^CONFLICT' /tmp/mcodex-merge-tree-0.122.txt | wc -l
```

- [ ] **Step 4: Confirm dry-run prep did not dirty the active checkout**

Run:

```bash
git status --short --branch
```

Expected: no tracked sync log commit exists on the maintainer's active checkout. Dry-run notes remain only in scratch space until the checkpoint worktree is created.

## Task 3: Create the 0.121 Checkpoint Worktree

**Files:**
- Modify: `codex-rs/Cargo.lock`
- Modify dynamically: files conflicted by `git merge rust-v0.121.0`
- Modify: `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`

- [ ] **Step 1: Create the checkpoint worktree**

Run from the repository root, not from another worktree:

```bash
git worktree add .worktrees/sync-rust-v0.121.0-base \
  -b sync/rust-v0.121.0-base main
cd .worktrees/sync-rust-v0.121.0-base
```

Expected: new branch `sync/rust-v0.121.0-base` is checked out in the worktree and starts from the current `main`.

- [ ] **Step 2: Start the 0.121 merge**

Run:

```bash
git merge --no-ff --no-commit rust-v0.121.0
```

Expected: FAIL with merge conflicts. This is expected and is the starting point for the checkpoint.

- [ ] **Step 3: List unresolved conflicts**

Run:

```bash
git diff --name-only --diff-filter=U
```

Expected: the unresolved list matches the 0.121 conflict families from the spec and Task 2.

- [ ] **Step 4: Create the tracked execution log inside the checkpoint worktree and record merge start**

Create or update `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md` inside `.worktrees/sync-rust-v0.121.0-base`.

If `/tmp/mcodex-upstream-stable-sync-execution-log.md` exists, copy its contents first, then append:

```markdown
## rust-v0.121.0 Checkpoint

- Merge command: git merge --no-ff --no-commit rust-v0.121.0
- Merge started at:
- Unresolved conflicts:
```

Expected: the tracked execution log is created for the first time on `sync/rust-v0.121.0-base`, not on the maintainer's active checkout.

- [ ] **Step 5: Stage the execution log update**

Run:

```bash
git add docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md
```

Expected: the log is staged, but do not commit until all merge conflicts are resolved.

- [ ] **Step 6: Regenerate `Cargo.lock` before the first Cargo invocation**

Run:

```bash
rm -f codex-rs/Cargo.lock
cd codex-rs
cargo generate-lockfile
```

Expected: `codex-rs/Cargo.lock` no longer contains merge markers and is usable for the Cargo commands in Tasks 4-7. Do this before running any `cargo test` or `cargo check`.

- [ ] **Step 7: Stage the lockfile regeneration**

Run:

```bash
git add codex-rs/Cargo.lock
```

Expected: the regenerated lockfile is staged or restaged before any later Cargo command updates it again.

## Task 4: Resolve 0.121 App-Server and Protocol Conflicts

**Files:**
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Modify: `codex-rs/app-server/tests/common/mcp_process.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/realtime_conversation.rs`
- Review: `codex-rs/app-server/src/codex_message_processor.rs`
- Review: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Review: `codex-rs/app-server-protocol/src/protocol/v2.rs`

- [ ] **Step 1: Inspect all three stages for `message_processor.rs`**

Run:

```bash
git show :1:codex-rs/app-server/src/message_processor.rs > /tmp/message_processor.base.rs
git show :2:codex-rs/app-server/src/message_processor.rs > /tmp/message_processor.ours.rs
git show :3:codex-rs/app-server/src/message_processor.rs > /tmp/message_processor.theirs.rs
```

Expected: all three files are written.

- [ ] **Step 2: Resolve `message_processor.rs` by preserving upstream request routing and fork pooled-runtime gates**

Edit `codex-rs/app-server/src/message_processor.rs` so:

```text
upstream 0.121 request routing remains present
fork pooled lease / account-pool API routing remains present
thread start/resume/fork paths still enforce pooled-mode host constraints
no broad conflict markers remain
```

Run:

```bash
rg '<<<<<<<|=======|>>>>>>>' codex-rs/app-server/src/message_processor.rs
```

Expected: no matches.

- [ ] **Step 3: Resolve app-server realtime test conflicts**

Edit:

```text
codex-rs/app-server/tests/common/mcp_process.rs
codex-rs/app-server/tests/suite/v2/realtime_conversation.rs
```

Keep upstream realtime test additions and preserve fork pooled-mode setup helpers. Do not remove fork account-pool test utilities from `codex-rs/app-server/tests/common/lib.rs`.

- [ ] **Step 4: Run app-server protocol and app-server targeted tests**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server-protocol
cargo test -p codex-app-server
```

Expected: tests may fail because other 0.121 conflicts are unresolved, but failures should not be unresolved conflict markers in app-server files. If failures are app-server semantic failures, keep them and fix after core conflicts are resolved.

- [ ] **Step 5: Stage resolved app-server files**

Run:

```bash
git add \
  codex-rs/app-server/src/message_processor.rs \
  codex-rs/app-server/tests/common/mcp_process.rs \
  codex-rs/app-server/tests/suite/v2/realtime_conversation.rs
```

Expected: these paths disappear from `git diff --name-only --diff-filter=U`.

## Task 5: Resolve 0.121 Core, State, and Runtime Conflicts

**Files:**
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/codex_tests.rs`
- Modify: `codex-rs/core/src/realtime_conversation_tests.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/tests/suite/realtime_conversation.rs`
- Modify: `codex-rs/core/tests/suite/view_image.rs`
- Review: `codex-rs/core/src/codex_thread.rs`
- Review: `codex-rs/core/src/codex_delegate.rs`
- Review: `codex-rs/core/src/client.rs`
- Review: `codex-rs/core/tests/suite/account_pool.rs`

- [ ] **Step 1: Inspect `codex.rs` conflict stages**

Run:

```bash
git show :1:codex-rs/core/src/codex.rs > /tmp/codex.base.rs
git show :2:codex-rs/core/src/codex.rs > /tmp/codex.ours.rs
git show :3:codex-rs/core/src/codex.rs > /tmp/codex.theirs.rs
```

Expected: all three files exist for the 0.121 checkpoint.

- [ ] **Step 2: Resolve `codex.rs` as a temporary 0.121 compatibility point**

For the 0.121 checkpoint only, keep the upstream 0.121 runtime behavior and preserve fork account-pool lease acquisition, fail-closed behavior, and leased-auth request setup. Add a note to the execution log:

```markdown
- 0.121 kept `codex-rs/core/src/codex.rs` as a temporary compatibility point.
- 0.122 is expected to delete or migrate this file into upstream's newer runtime/thread structure.
```

- [ ] **Step 3: Resolve `codex_tests.rs` in lockstep with the temporary compatibility point**

Edit `codex-rs/core/src/codex_tests.rs` so upstream `0.121` behavioral coverage for `codex.rs` remains present and fork pooled-account or lease-auth assertions that still apply to the checkpoint remain meaningful. Verify no conflict markers:

```bash
rg '<<<<<<<|=======|>>>>>>>' codex-rs/core/src/codex_tests.rs
```

Expected: no matches. Do not silently drop `codex.rs`-adjacent regression coverage just to get the checkpoint compiling.

- [ ] **Step 4: Resolve core state service conflict**

Edit `codex-rs/core/src/state/service.rs` so upstream state-service changes remain and fork account-pool state writes, lease updates, and fail-closed diagnostics remain. Verify no conflict markers:

```bash
rg '<<<<<<<|=======|>>>>>>>' codex-rs/core/src/state/service.rs
```

Expected: no matches.

- [ ] **Step 5: Resolve realtime and view-image test conflicts**

Edit:

```text
codex-rs/core/src/realtime_conversation_tests.rs
codex-rs/core/tests/suite/realtime_conversation.rs
codex-rs/core/tests/suite/view_image.rs
```

Preserve upstream realtime behavior and fork auth/session setup. Do not remove pooled-account fixtures from `codex-rs/core/tests/suite/account_pool.rs`.

- [ ] **Step 6: Run core-focused tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core
```

Expected: no conflict-marker compilation failures. This run must exercise the newly resolved `codex_tests.rs` coverage alongside the temporary `codex.rs` compatibility point. If semantic failures appear, use @superpowers:systematic-debugging and record root cause in the execution log.

- [ ] **Step 7: Stage resolved core files**

Run:

```bash
git add \
  codex-rs/core/src/codex.rs \
  codex-rs/core/src/codex_tests.rs \
  codex-rs/core/src/realtime_conversation_tests.rs \
  codex-rs/core/src/state/service.rs \
  codex-rs/core/tests/suite/realtime_conversation.rs \
  codex-rs/core/tests/suite/view_image.rs \
  docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md
```

Expected: these paths disappear from unresolved conflict output.

## Task 6: Resolve 0.121 Core-Skills and TUI Conflicts

**Files:**
- Modify: `codex-rs/core-skills/src/loader.rs`
- Modify: `codex-rs/core-skills/src/loader_tests.rs`
- Modify: `codex-rs/tui/src/app.rs`
- Modify: `codex-rs/tui/src/app_server_session.rs`
- Modify: `codex-rs/tui/src/chatwidget.rs`
- Review: `codex-rs/tui/src/status/helpers.rs`
- Review: `codex-rs/tui/src/chatwidget/tests/app_server.rs`

- [ ] **Step 1: Resolve core-skills loader conflicts**

Edit:

```text
codex-rs/core-skills/src/loader.rs
codex-rs/core-skills/src/loader_tests.rs
```

Keep upstream loader behavior and preserve fork `mcodex` admin skills root behavior. Verify:

```bash
rg 'mcodex|MCODEX|/etc/mcodex|CODEX_HOME|~/.codex' codex-rs/core-skills/src
rg '<<<<<<<|=======|>>>>>>>' codex-rs/core-skills/src
```

Expected: intentional `mcodex` references remain; no conflict markers remain.

- [ ] **Step 2: Resolve TUI app and app-server session conflicts**

Edit:

```text
codex-rs/tui/src/app.rs
codex-rs/tui/src/app_server_session.rs
```

Keep upstream TUI runtime/session changes and preserve fork startup access, pooled status, and `mcodex` identity handling.

- [ ] **Step 3: Resolve `chatwidget.rs` conflict without expanding the large module**

Edit `codex-rs/tui/src/chatwidget.rs` only enough to resolve the conflict. Do not add new unrelated logic to this already-large module. If substantial new logic is needed, extract it to a focused module and document that choice in the execution log before editing.

- [ ] **Step 4: Run core-skills and TUI tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core-skills
cargo test -p codex-tui
```

Expected: no conflict-marker failures. Snapshot failures are allowed only if they reflect intentional UI output changes and are reviewed later.

- [ ] **Step 5: Stage resolved core-skills and TUI files**

Run:

```bash
git add \
  codex-rs/core-skills/src/loader.rs \
  codex-rs/core-skills/src/loader_tests.rs \
  codex-rs/tui/src/app.rs \
  codex-rs/tui/src/app_server_session.rs \
  codex-rs/tui/src/chatwidget.rs
```

Expected: these files are no longer unresolved.

## Task 7: Complete and Commit the 0.121 Checkpoint

**Files:**
- Modify: `codex-rs/Cargo.lock`
- Modify: `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`

- [ ] **Step 1: Refresh `Cargo.lock` after all 0.121 conflict work if needed**

Run:

```bash
cd codex-rs
cargo generate-lockfile
```

Expected: `Cargo.lock` remains coherent and updates only if dependency resolution changed during earlier conflict resolution. Do not carry forward a conflicted or stale lockfile.

- [ ] **Step 2: Confirm no unresolved conflicts remain**

Run:

```bash
git diff --name-only --diff-filter=U
rg '<<<<<<<|=======|>>>>>>>' .
```

Expected: both commands produce no unresolved project file output. If `rg` finds fixture text that intentionally contains conflict markers, document and ignore only that fixture.

- [ ] **Step 3: Run checkpoint gate tests**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool
cargo test -p codex-login
cargo test -p codex-state
cargo test -p codex-core
cargo test -p codex-app-server
cargo test -p codex-app-server-protocol
cargo test -p codex-tui
just fmt
```

Expected: all commands pass, except reviewed TUI snapshot updates may be pending.

- [ ] **Step 4: Run snapshot review if TUI snapshots changed**

Run:

```bash
cd codex-rs
cargo insta pending-snapshots -p codex-tui
```

Expected: no pending snapshots unless UI changes are intentional. For intentional changes, inspect each `.snap.new`, then run:

```bash
cargo insta accept -p codex-tui
```

- [ ] **Step 5: Update the execution log with checkpoint verification**

Append:

```markdown
### rust-v0.121.0 Checkpoint Verification

- cargo test -p codex-account-pool:
- cargo test -p codex-login:
- cargo test -p codex-state:
- cargo test -p codex-core:
- cargo test -p codex-app-server:
- cargo test -p codex-app-server-protocol:
- cargo test -p codex-tui:
- just fmt:
- Snapshot review:
- Deferred non-core follow-ups:
```

- [ ] **Step 6: Commit the 0.121 merge checkpoint**

Run:

```bash
git add -A
git commit -m "merge: sync upstream rust-v0.121.0 checkpoint"
```

Expected: merge commit succeeds and includes the upstream tag as the second parent.

## Task 8: Create the 0.122 Target Worktree

**Files:**
- Modify dynamically: files conflicted by `git merge rust-v0.122.0`
- Modify: `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`

- [ ] **Step 1: Create the final target worktree from the checkpoint**

Run from the repository root:

```bash
git worktree add .worktrees/sync-rust-v0.122.0 \
  -b sync/rust-v0.122.0 sync/rust-v0.121.0-base
cd .worktrees/sync-rust-v0.122.0
```

Expected: new branch `sync/rust-v0.122.0` starts at the 0.121 checkpoint.

- [ ] **Step 2: Start the 0.122 merge**

Run:

```bash
git merge --no-ff --no-commit rust-v0.122.0
```

Expected: FAIL with conflicts. This is expected.

- [ ] **Step 3: Record unresolved conflicts**

Run:

```bash
git diff --name-only --diff-filter=U
```

Expected: list includes generated schemas, CLI, core client/config/plugin/task files, login auth, state, TUI update/install docs and scripts.

- [ ] **Step 4: Add the 0.122 merge start to the execution log**

Append:

```markdown
## rust-v0.122.0 Final Target

- Merge command: git merge --no-ff --no-commit rust-v0.122.0
- Merge started at:
- Unresolved conflicts:
```

- [ ] **Step 5: Stage the execution log update**

Run:

```bash
git add docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md
```

Expected: log update is staged for the eventual merge commit.

## Task 9: Resolve 0.122 Workspace, Protocol, and App-Server Conflicts

**Files:**
- Modify: `codex-rs/Cargo.toml`
- Modify: `codex-rs/core/Cargo.toml`
- Modify: `codex-rs/app-server/README.md`
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Review or modify if contract drift appears: `codex-rs/app-server/src/codex_message_processor.rs`
- Review or modify if contract drift appears: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Review or modify if contract drift appears: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Modify: `codex-rs/app-server/tests/common/mcp_process.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/command_exec.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/realtime_conversation.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/turn_start.rs`
- Regenerate: `codex-rs/app-server-protocol/schema/json/ClientRequest.json`
- Regenerate: `codex-rs/app-server-protocol/schema/typescript/ClientRequest.ts`
- Regenerate: `codex-rs/app-server-protocol/schema/typescript/ServerNotification.ts`

- [ ] **Step 1: Resolve workspace manifest conflicts**

Edit:

```text
codex-rs/Cargo.toml
codex-rs/core/Cargo.toml
```

Preserve upstream dependency/version changes and keep fork crates such as `codex-account-pool` and `codex-product-identity` wired into the workspace. Do not delete fork crates to make dependency conflicts disappear.

- [ ] **Step 2: Resolve app-server code conflicts and review pooled routing seams**

Edit `codex-rs/app-server/src/message_processor.rs` so upstream `rust-v0.122.0` APIs remain and fork pooled lease/API behavior remains. Review `codex-rs/app-server/src/codex_message_processor.rs` at the same time and merge any required pooled-runtime or request-routing adjustments there as well. Do not use whole-file `ours` or `theirs`.

- [ ] **Step 3: Resolve app-server test conflicts**

Edit:

```text
codex-rs/app-server/tests/common/mcp_process.rs
codex-rs/app-server/tests/suite/v2/command_exec.rs
codex-rs/app-server/tests/suite/v2/realtime_conversation.rs
codex-rs/app-server/tests/suite/v2/turn_start.rs
```

Preserve upstream test coverage for new 0.122 behavior and fork pooled-mode fixtures.

- [ ] **Step 4: Review protocol source definitions before regenerating schemas**

Review:

```text
codex-rs/app-server-protocol/src/protocol/common.rs
codex-rs/app-server-protocol/src/protocol/v2.rs
```

Preserve upstream `0.122` protocol source changes and keep the fork's shipped pooled API semantics intact before any schema regeneration. Do not rely on generated JSON or TypeScript diffs to catch semantic protocol regressions after the fact.

- [ ] **Step 5: Resolve docs without dropping fork API documentation**

Edit `codex-rs/app-server/README.md`. Keep upstream 0.122 API documentation and preserve shipped pooled API documentation.

- [ ] **Step 6: Defer generated protocol files until source compiles**

For conflicted generated files, prefer removing conflict markers by regenerating later. Do not hand-select final JSON/TypeScript chunks as the final solution. Leave a note in the execution log:

```markdown
- App-server generated schema conflicts deferred until after protocol source resolution.
```

- [ ] **Step 7: Stage resolved non-generated app-server files**

Run:

```bash
git add \
  codex-rs/Cargo.toml \
  codex-rs/core/Cargo.toml \
  codex-rs/app-server/README.md \
  codex-rs/app-server/src/codex_message_processor.rs \
  codex-rs/app-server/src/message_processor.rs \
  codex-rs/app-server-protocol/src/protocol/common.rs \
  codex-rs/app-server-protocol/src/protocol/v2.rs \
  codex-rs/app-server/tests/common/mcp_process.rs \
  codex-rs/app-server/tests/suite/v2/command_exec.rs \
  codex-rs/app-server/tests/suite/v2/realtime_conversation.rs \
  codex-rs/app-server/tests/suite/v2/turn_start.rs \
  docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md
```

Expected: these files are no longer unresolved.

## Task 10: Resolve 0.122 Core Runtime Migration

**Files:**
- Modify or delete: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/client.rs`
- Modify: `codex-rs/core/src/client_tests.rs`
- Modify: `codex-rs/core/src/codex_delegate.rs`
- Modify: `codex-rs/core/src/codex_thread.rs`
- Modify: `codex-rs/core/src/config_loader/layer_io.rs`
- Modify: `codex-rs/core/src/config_loader/mod.rs`
- Modify: `codex-rs/core/src/guardian/tests.rs`
- Modify: `codex-rs/core/src/mcp_openai_file.rs`
- Modify: `codex-rs/core/src/plugins/manager.rs`
- Modify: `codex-rs/core/src/session/tests.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/src/tasks/compact.rs`
- Modify: `codex-rs/core/tests/suite/realtime_conversation.rs`
- Modify: `codex-rs/core/tests/suite/view_image.rs`

- [ ] **Step 1: Inspect upstream deletion of `codex.rs`**

Run:

```bash
git status --short codex-rs/core/src/codex.rs
git ls-tree --name-only rust-v0.122.0:codex-rs/core/src | rg 'codex|thread|delegate|session'
```

Expected: upstream has no `codex.rs` and uses newer runtime/thread/delegate/session files.

- [ ] **Step 2: List fork semantics currently in `codex.rs`**

Run:

```bash
rg -n 'account_pool|pooled|lease|Leased|auth|compact|review|realtime|websocket' \
  codex-rs/core/src/codex.rs
```

Expected: output identifies fork runtime semantics that must be ported, not blindly kept in the deleted file.

- [ ] **Step 3: Port fork semantics into upstream runtime structure**

Move or adapt the identified semantics into the upstream 0.122 structure:

```text
codex-rs/core/src/codex_thread.rs
codex-rs/core/src/codex_delegate.rs
codex-rs/core/src/client.rs
codex-rs/core/src/tasks/compact.rs
codex-rs/core/src/state/service.rs
```

Expected contract:

```text
pooled turns acquire and use lease-scoped auth
compact uses the active leased auth when pooled execution is active
review/subagent paths inherit or acquire the correct leased auth
realtime/websocket paths do not bypass pooled auth
unavailable pooled accounts fail closed
```

- [ ] **Step 4: Accept upstream deletion of `codex.rs` when port is complete**

Run:

```bash
git rm codex-rs/core/src/codex.rs
```

Expected: `codex.rs` is removed only after required fork semantics are present in upstream replacement files. If there is a documented architectural reason to keep it, record that reason in the execution log before not deleting it.

- [ ] **Step 5: Resolve core config-loader conflicts**

Edit:

```text
codex-rs/core/src/config_loader/layer_io.rs
codex-rs/core/src/config_loader/mod.rs
```

Keep upstream managed config changes and preserve fork active `mcodex` roots. Normal runtime must not read live `CODEX_HOME` or `~/.codex`.

- [ ] **Step 6: Resolve plugin, MCP, guardian, and session test conflicts**

Edit:

```text
codex-rs/core/src/guardian/tests.rs
codex-rs/core/src/mcp_openai_file.rs
codex-rs/core/src/plugins/manager.rs
codex-rs/core/src/session/tests.rs
codex-rs/core/tests/suite/view_image.rs
```

Preserve upstream 0.122 filesystem/plugin/MCP behavior and fork product identity expectations.

- [ ] **Step 7: Resolve core request path tests**

Edit:

```text
codex-rs/core/src/client_tests.rs
codex-rs/core/tests/suite/realtime_conversation.rs
```

Keep upstream 0.122 tests and ensure fork account-pool tests still assert lease-scoped request auth. If existing tests no longer cover compact/review/realtime leased auth after the port, add focused tests in `codex-rs/core/tests/suite/account_pool.rs`.

- [ ] **Step 8: Run core tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core
```

Expected: PASS. If failures occur, use @superpowers:systematic-debugging and record the failing test name and root cause.

- [ ] **Step 9: Stage core runtime migration files**

Run:

```bash
git add \
  codex-rs/core/src/client.rs \
  codex-rs/core/src/client_tests.rs \
  codex-rs/core/src/codex.rs \
  codex-rs/core/src/codex_delegate.rs \
  codex-rs/core/src/codex_thread.rs \
  codex-rs/core/src/config_loader/layer_io.rs \
  codex-rs/core/src/config_loader/mod.rs \
  codex-rs/core/src/guardian/tests.rs \
  codex-rs/core/src/mcp_openai_file.rs \
  codex-rs/core/src/plugins/manager.rs \
  codex-rs/core/src/session/tests.rs \
  codex-rs/core/src/state/service.rs \
  codex-rs/core/src/tasks/compact.rs \
  codex-rs/core/tests/suite/realtime_conversation.rs \
  codex-rs/core/tests/suite/view_image.rs
```

Expected: core files are no longer unresolved.

## Task 11: Resolve 0.122 Login, State, RMCP, CLI, and Exec Conflicts

**Files:**
- Modify: `codex-rs/login/src/auth/mod.rs`
- Modify: `codex-rs/login/tests/suite/mod.rs`
- Modify: `codex-rs/state/src/lib.rs`
- Modify: `codex-rs/rmcp-client/src/lib.rs`
- Modify: `codex-rs/exec-server/tests/exec_process.rs`
- Modify: `codex-rs/cli/src/login.rs`
- Modify: `codex-rs/cli/src/main.rs`

- [ ] **Step 1: Resolve login auth conflict**

Edit `codex-rs/login/src/auth/mod.rs`. Preserve upstream auth changes and fork auth seams:

```text
LegacyAuthView remains the compatibility auth surface
lease-scoped auth remains available for pooled execution
pooled registration remains out of shared mutable auth
```

- [ ] **Step 2: Resolve login test conflicts**

Edit `codex-rs/login/tests/suite/mod.rs`. Keep upstream test modules and fork pooled-registration/auth-seam modules.

- [ ] **Step 3: Run login tests**

Run:

```bash
cd codex-rs
cargo test -p codex-login
```

Expected: PASS.

- [ ] **Step 4: Resolve state conflict**

Edit `codex-rs/state/src/lib.rs`. Preserve upstream state exports and fork account-pool modules, migrations, runtime helpers, and quota/observability exports.

- [ ] **Step 5: Run state tests**

Run:

```bash
cd codex-rs
cargo test -p codex-state
```

Expected: PASS.

- [ ] **Step 6: Resolve RMCP and exec test conflicts**

Edit:

```text
codex-rs/rmcp-client/src/lib.rs
codex-rs/exec-server/tests/exec_process.rs
```

Preserve upstream 0.122 behavior and fork environment/test hardening changes. Do not modify sandbox env-var constants prohibited by `AGENTS.md`.

- [ ] **Step 7: Resolve CLI conflicts**

Edit:

```text
codex-rs/cli/src/login.rs
codex-rs/cli/src/main.rs
```

Preserve upstream CLI additions and fork behavior:

```text
mcodex command identity remains active
accounts CLI namespace remains wired
pooled registration command paths remain wired
native CLI npm publishing assumptions do not re-enter runtime CLI behavior
```

- [ ] **Step 8: Run CLI and related tests**

Run:

```bash
cd codex-rs
cargo test -p codex-cli
cargo test -p codex-rmcp-client
cargo test -p codex-exec-server
```

Expected: PASS.

- [ ] **Step 9: Stage login/state/CLI files**

Run:

```bash
git add \
  codex-rs/login/src/auth/mod.rs \
  codex-rs/login/tests/suite/mod.rs \
  codex-rs/state/src/lib.rs \
  codex-rs/rmcp-client/src/lib.rs \
  codex-rs/exec-server/tests/exec_process.rs \
  codex-rs/cli/src/login.rs \
  codex-rs/cli/src/main.rs
```

Expected: these files are no longer unresolved.

## Task 12: Resolve 0.122 TUI, Docs, Installer, and Update Conflicts

**Files:**
- Modify: `codex-rs/tui/src/app.rs`
- Modify: `codex-rs/tui/src/app/app_server_adapter.rs`
- Modify: `codex-rs/tui/src/app_server_session.rs`
- Modify: `codex-rs/tui/src/chatwidget.rs`
- Modify: `codex-rs/tui/src/debug_config.rs`
- Modify: `codex-rs/tui/src/history_cell.rs`
- Modify: `codex-rs/tui/src/onboarding/onboarding_screen.rs`
- Modify: `codex-rs/tui/src/slash_command.rs`
- Review or modify if contract drift appears: `codex-rs/tui/src/status/mod.rs`
- Review or modify if contract drift appears: `codex-rs/tui/src/status/account.rs`
- Review or modify if contract drift appears: `codex-rs/tui/src/status/helpers.rs`
- Review or modify if contract drift appears: `codex-rs/tui/src/status/format.rs`
- Review or modify if contract drift appears: `codex-rs/tui/src/status/rate_limits.rs`
- Review or modify if contract drift appears: `codex-rs/tui/src/status/card.rs`
- Review or modify if contract drift appears: `codex-rs/tui/src/status/tests.rs`
- Modify: `codex-rs/tui/src/tooltips.rs`
- Modify: `codex-rs/tui/src/update_action.rs`
- Modify: `docs/config.md`
- Modify: `scripts/install/install.ps1`
- Modify: `scripts/install/install.sh`
- Review or modify if contract drift appears: `.github/workflows/rust-release.yml`
- Review or modify if contract drift appears: `.github/workflows/rust-release-windows.yml`
- Review or modify if contract drift appears: `.github/workflows/rust-release-prepare.yml`
- Review or modify if contract drift appears: `.github/actions/linux-code-sign/action.yml`
- Review or modify if contract drift appears: `.github/actions/macos-code-sign/action.yml`
- Review or modify if contract drift appears: `.github/actions/windows-code-sign/action.yml`
- Review or modify if contract drift appears: `scripts/stage_cli_archives.py`
- Review or modify if contract drift appears: `scripts/stage_npm_packages.py`
- Review or modify if contract drift appears: `scripts/test_stage_cli_archives.py`
- Review or modify if contract drift appears: `scripts/test_stage_npm_packages.py`
- Review or modify if contract drift appears: `docs/release.md`
- Review or modify if contract drift appears: `README.md`
- Review or modify if contract drift appears: `docs/install.md`

- [ ] **Step 1: Resolve TUI app/session conflicts and review status integration surfaces**

Edit:

```text
codex-rs/tui/src/app.rs
codex-rs/tui/src/app/app_server_adapter.rs
codex-rs/tui/src/app_server_session.rs
codex-rs/tui/src/chatwidget.rs
codex-rs/tui/src/status/mod.rs
codex-rs/tui/src/status/account.rs
codex-rs/tui/src/status/helpers.rs
codex-rs/tui/src/status/format.rs
codex-rs/tui/src/status/rate_limits.rs
codex-rs/tui/src/status/card.rs
codex-rs/tui/src/status/tests.rs
```

Keep upstream 0.122 side conversation, queueing, plan mode, plugin, and permission behavior. Preserve fork pooled status, startup access, and `mcodex` identity surfaces. Review `codex-rs/tui/src/status/*` semantically even if they are not direct conflicts so pooled lease rendering, unavailable-account messaging, damping notes, and next-eligible-time displays remain correct. Avoid adding unrelated logic to `chatwidget.rs`.

- [ ] **Step 2: Resolve TUI identity/update/onboarding conflicts**

Edit:

```text
codex-rs/tui/src/debug_config.rs
codex-rs/tui/src/history_cell.rs
codex-rs/tui/src/onboarding/onboarding_screen.rs
codex-rs/tui/src/slash_command.rs
codex-rs/tui/src/tooltips.rs
codex-rs/tui/src/update_action.rs
```

Expected contract:

```text
TUI user-facing product name remains mcodex where fork adopted it
update actions use OSS/script-managed install behavior
upstream feature entries and slash commands remain available
pooled startup notice remains reachable
```

- [ ] **Step 3: Resolve installer script conflicts**

Edit:

```text
scripts/install/install.sh
scripts/install/install.ps1
```

Preserve fork OSS installer behavior:

```text
downloads.mcodex.sota.wiki is the download source
repositories/mcodex/channels/stable/latest.json is the stable manifest
native CLI archives are mcodex-named
installers do not point to upstream GitHub native assets
```

- [ ] **Step 4: Resolve `docs/config.md` conflict**

Keep upstream configuration documentation additions and fork `mcodex` home/config identity documentation.

- [ ] **Step 5: Audit release workflow, code-sign, and staging contracts even without direct merge conflicts**

Review:

```text
.github/workflows/rust-release.yml
.github/workflows/rust-release-windows.yml
.github/workflows/rust-release-prepare.yml
.github/actions/linux-code-sign/action.yml
.github/actions/macos-code-sign/action.yml
.github/actions/windows-code-sign/action.yml
scripts/stage_cli_archives.py
scripts/stage_npm_packages.py
scripts/test_stage_cli_archives.py
scripts/test_stage_npm_packages.py
docs/release.md
README.md
docs/install.md
```

Confirm this contract still holds after the merge:

```text
GitHub Releases remain lightweight release records and do not publish native CLI archives.
OSS upload order stays: versioned release artifacts first, root install scripts second, channels/stable/latest.json last.
stage_cli_archives emits mcodex-named native archives and the stable manifest.
stage_npm_packages remains limited to npm package staging and does not reintroduce npm as the primary mcodex CLI distribution path.
release workflows and code-sign actions still package/sign mcodex binaries rather than upstream codex names.
```

If any reviewed file violates this contract, fix it in this task rather than deferring to the final grep-only gate.

- [ ] **Step 6: Run TUI and installer tests**

Run:

```bash
cd codex-rs
cargo test -p codex-tui
cd ..
python3 -m unittest scripts.install.test_install_scripts
python3 -m unittest scripts.test_stage_cli_archives
python3 -m unittest scripts.test_stage_npm_packages
```

Expected: PASS, except TUI snapshots may require intentional review.

- [ ] **Step 7: Review TUI snapshots if needed**

Run:

```bash
cd codex-rs
cargo insta pending-snapshots -p codex-tui
```

Expected: no pending snapshots unless intentional. For intentional changes, inspect `.snap.new` files, then run:

```bash
cargo insta accept -p codex-tui
```

- [ ] **Step 8: Stage TUI, docs, installer, and release-contract files**

Run:

```bash
git add \
  .github/workflows/rust-release.yml \
  .github/workflows/rust-release-windows.yml \
  .github/workflows/rust-release-prepare.yml \
  .github/actions/linux-code-sign/action.yml \
  .github/actions/macos-code-sign/action.yml \
  .github/actions/windows-code-sign/action.yml \
  codex-rs/tui/src/app.rs \
  codex-rs/tui/src/app/app_server_adapter.rs \
  codex-rs/tui/src/app_server_session.rs \
  codex-rs/tui/src/chatwidget.rs \
  codex-rs/tui/src/debug_config.rs \
  codex-rs/tui/src/history_cell.rs \
  codex-rs/tui/src/onboarding/onboarding_screen.rs \
  codex-rs/tui/src/slash_command.rs \
  codex-rs/tui/src/status/mod.rs \
  codex-rs/tui/src/status/account.rs \
  codex-rs/tui/src/status/helpers.rs \
  codex-rs/tui/src/status/format.rs \
  codex-rs/tui/src/status/rate_limits.rs \
  codex-rs/tui/src/status/card.rs \
  codex-rs/tui/src/status/tests.rs \
  codex-rs/tui/src/tooltips.rs \
  codex-rs/tui/src/update_action.rs \
  docs/config.md \
  docs/install.md \
  docs/release.md \
  README.md \
  scripts/stage_cli_archives.py \
  scripts/stage_npm_packages.py \
  scripts/test_stage_cli_archives.py \
  scripts/test_stage_npm_packages.py \
  scripts/install/install.ps1 \
  scripts/install/install.sh
```

Expected: unresolved files are staged, and any non-conflicted release-contract fixes from this task are staged too.

## Task 13: Regenerate Derived Artifacts

**Files:**
- Regenerate: `codex-rs/Cargo.lock`
- Regenerate when needed: `MODULE.bazel.lock`
- Regenerate when needed: `codex-rs/core/config.schema.json`
- Regenerate: `codex-rs/app-server-protocol/schema/json/*.json`
- Regenerate: `codex-rs/app-server-protocol/schema/typescript/**/*.ts`
- Modify: `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`

- [ ] **Step 1: Confirm only generated files remain unresolved before regeneration**

Run:

```bash
git diff --name-only --diff-filter=U
```

Expected: unresolved paths, if any, are limited to generated schema files that will be overwritten by the generator. If a source file is still unresolved here, stop and resolve it before continuing.

- [ ] **Step 2: Regenerate app-server schemas**

Run:

```bash
cd codex-rs
just write-app-server-schema
just write-app-server-schema --experimental
```

Expected: schema generation succeeds and updates app-server protocol generated files.

- [ ] **Step 3: Regenerate config schema if config types changed**

Run if `codex-rs/config/src/config_toml.rs`, `codex-rs/config/src/types.rs`, or nested config types changed:

```bash
cd codex-rs
just write-config-schema
```

Expected: `codex-rs/core/config.schema.json` is updated if needed.

- [ ] **Step 4: Refresh Cargo lockfile**

Run:

```bash
cd codex-rs
cargo check -p codex-core
```

Expected: `Cargo.lock` is coherent and no dependency conflict markers remain.

- [ ] **Step 5: Refresh Bazel lockfile if dependencies changed**

Run from repo root if `codex-rs/Cargo.toml` or `codex-rs/Cargo.lock` changed:

```bash
just bazel-lock-update
just bazel-lock-check
```

Expected: both commands pass and `MODULE.bazel.lock` is updated if needed.

- [ ] **Step 6: Confirm no unresolved conflicts remain after regeneration**

Run:

```bash
git diff --name-only --diff-filter=U
rg '<<<<<<<|=======|>>>>>>>' .
```

Expected: no unresolved conflicts and no conflict markers remain in generated outputs.

- [ ] **Step 7: Update execution log artifact checklist**

Mark applicable checklist items as complete:

```markdown
- [x] Cargo.lock reviewed or regenerated
- [x] MODULE.bazel.lock refreshed when dependencies changed
- [x] config schema regenerated when config types changed
- [x] app-server schemas regenerated when protocol changed
```

- [ ] **Step 8: Stage generated artifacts**

Run:

```bash
git add \
  codex-rs/Cargo.lock \
  MODULE.bazel.lock \
  codex-rs/core/config.schema.json \
  codex-rs/app-server-protocol/schema/json \
  codex-rs/app-server-protocol/schema/typescript \
  docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md
```

Expected: generated artifacts are staged. If a listed path did not change, Git ignores it or reports no matching change.

## Task 14: Final 0.122 Verification Gate

**Files:**
- Modify: `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`

- [ ] **Step 1: Run focused crate tests**

Run:

```bash
cd codex-rs
cargo test -p codex-account-pool
cargo test -p codex-login
cargo test -p codex-state
cargo test -p codex-core
cargo test -p codex-app-server
cargo test -p codex-app-server-protocol
cargo test -p codex-tui
```

Expected: PASS. This is only the baseline crate gate. Critical named runtime regressions in later steps still must actually execute locally or be satisfied by required CI.

- [ ] **Step 2: Run formatting**

Run:

```bash
cd codex-rs
just fmt
```

Expected: PASS or formats files. Do not rerun tests solely because formatting changed.

- [ ] **Step 3: Run scoped lint fixes**

Run for each changed crate with code changes:

```bash
cd codex-rs
just fix -p codex-core-skills
just fix -p codex-cli
just fix -p codex-core
just fix -p codex-rmcp-client
just fix -p codex-exec-server
just fix -p codex-login
just fix -p codex-state
just fix -p codex-app-server
just fix -p codex-app-server-protocol
just fix -p codex-tui
```

Expected: PASS. Skip crates that were not changed. Do not rerun tests after `fix` or `fmt` unless a command reports a semantic issue that requires code changes.

- [ ] **Step 4: Ask before running the full workspace test suite locally**

Ask the maintainer:

```text
The sync changed shared crates. Do you want me to run the full workspace test suite locally (`just test` if nextest is installed, otherwise `cargo test`), or rely on required CI for the final broad gate?
```

Expected: either run the approved command or record CI deferral in the execution log. If the local environment later proves unable to execute critical named runtime tests, required CI becomes mandatory for those checks too.

- [ ] **Step 5: Run the full workspace suite if approved**

Run:

```bash
cd codex-rs
if command -v cargo-nextest >/dev/null 2>&1; then
  just test
else
  cargo test
fi
```

Expected: PASS. If not approved locally, skip this step, document the reason, and treat required CI green as a mandatory pre-merge broad gate.

- [ ] **Step 6: Detect sandbox-limited environments and run explicit release/install/update contract checks**

Run:

```bash
printf 'CODEX_SANDBOX_NETWORK_DISABLED=%s\n' "${CODEX_SANDBOX_NETWORK_DISABLED:-not-set}"
printf 'CODEX_SANDBOX=%s\n' "${CODEX_SANDBOX:-not-set}"
rg -n 'openai/codex|api.github.com/repos/openai/codex|npm.*codex|@openai/codex' \
  scripts/install .github/workflows .github/actions codex-rs/tui/src codex-rs/cli/src README.md docs/install.md docs/release.md
rg -n 'downloads\\.mcodex\\.sota\\.wiki|repositories/mcodex|install\\.sh|install\\.ps1' \
  scripts/install .github/workflows codex-rs/tui/src docs/release.md README.md
```

Expected: upstream `openai/codex` references remain only where intentionally historical or non-CLI-package related. OSS references exist for installer/update/release paths. If `CODEX_SANDBOX_NETWORK_DISABLED=1`, `CODEX_SANDBOX=seatbelt`, or a later named test reports `ignored`, `0 tests`, or `skipping test` because of those sandbox variables, do not count the critical runtime gate as satisfied locally; record the affected tests as CI-required and require required CI to execute them before merge.

- [ ] **Step 7: Run targeted product-identity, home-dir, CLI identity, and startup regressions**

Run:

```bash
cd codex-rs
cargo test -p codex-product-identity mcodex_identity_defines_active_and_legacy_roots
cargo test -p codex-utils-home-dir find_codex_home_prefers_mcodex_home_env
cargo test -p codex-utils-home-dir find_codex_home_without_env_uses_dot_mcodex
cargo test -p codex-utils-home-dir find_codex_home_ignores_codex_home_when_mcodex_home_is_unset
cargo test -p codex-cli runtime_display_identity_version_uses_mcodex_identity
cargo test -p codex-cli runtime_display_identity_help_uses_mcodex_identity
cargo test -p codex-tui startup_decision_uses_pooled_only_notice_when_pooled_access_exists
cargo test -p codex-tui startup_probe_failure_falls_back_to_needs_login
cargo test -p codex-tui pooled_only_notice_starts_hidden_auth_and_reveals_it_with_l
cargo test -p codex-tui pooled_only_notice_enter_dismisses_notice_before_trust_step
cargo test -p codex-tui pooled_only_notice_hide_failure_still_continues
rg -n 'CODEX_HOME|~/.codex|MCODEX_HOME|~/.mcodex|mcodex' \
  codex-rs/core/src codex-rs/cli/src codex-rs/tui/src codex-rs/product-identity/src
```

Expected: fresh and existing `mcodex` startup paths still resolve `MCODEX_HOME` and `~/.mcodex` correctly, CLI identity remains forked, pooled-startup onboarding still works, and legacy `CODEX_HOME` and `~/.codex` references stay limited to migration or compatibility code.

- [ ] **Step 8: Run targeted migration, leased-auth, realtime, review, and subagent regressions**

Run:

```bash
cd codex-rs
cargo test -p codex-core auth_import_failure_records_warning_without_blocking_migration
cargo test -p codex-core pending_migration_resumes_auth_import_without_prompting
cargo test -p codex-core pending_migration_resumes_config_import_without_prompting
cargo test -p codex-core skips_when_user_declines_migration
cargo test -p codex-app-server app_server_fails_noninteractively_when_product_identity_migration_needs_prompt
cargo test -p codex-app-server app_server_resumes_pending_product_identity_migration_noninteractively
cargo test -p codex-login auth_seams_leased_turn_auth_does_not_read_shared_auth_manager
cargo test -p codex-login auth_seams_local_lease_scoped_session_refresh_fails_closed_on_account_rebind
cargo test -p codex-login auth_seams_local_lease_scoped_session_refresh_fails_closed_on_lease_epoch_supersession
cargo test -p codex-login pooled_registration_browser_returns_tokens_without_writing_shared_auth
cargo test -p codex-login pooled_registration_failure_completes_without_hanging
cargo test -p codex-core pooled_request_uses_lease_scoped_auth_session_not_shared_auth_snapshot
cargo test -p codex-core pooled_manual_remote_compact_uses_leased_account_for_compact_and_follow_up
cargo test -p codex-core pooled_pre_turn_remote_compact_uses_leased_account_for_compact_and_follow_up
cargo test -p codex-core websocket_fallback_in_pooled_mode_uses_leased_account_for_first_websocket_attempt
cargo test -p codex-core conversation_start_defaults_to_v2_and_gpt_realtime_1_5
cargo test -p codex-core conversation_user_text_turn_is_sent_to_realtime_when_active
cargo test -p codex-core review_op_emits_lifecycle_and_review_output
cargo test -p codex-core subagent_notification_is_included_without_wait
cargo test -p codex-core responses_api_proxy_dumps_parent_and_subagent_identity_headers
```

Expected: migration copy boundaries remain fail-closed, pooled request and compact flows keep lease-scoped auth, websocket and realtime paths stay wired, and review/subagent flows keep the fork contract. Each named cargo invocation must actually execute its selected test locally; `ignored`, `0 tests`, or sandbox skip output does not satisfy this step and must roll over to required CI before merge. If this merge touches a pooled-auth path that is not directly covered by one of the named tests, add a focused regression before merging and run it here.

- [ ] **Step 9: Run app-server lease-notification and TUI status/update regressions**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_lease_updated_emits_on_resume
cargo test -p codex-app-server account_lease_updated_emits_when_automatic_switch_changes_live_snapshot
cargo test -p codex-tui status_account_lease_display_from_response_formats_pool_details
cargo test -p codex-tui status_account_lease_display_from_response_formats_damped_proactive_switch
cargo test -p codex-tui status_account_lease_display_from_response_formats_non_replayable_turn_reason
cargo test -p codex-tui account_lease_updated_adds_automatic_switch_notice_when_account_changes
cargo test -p codex-tui account_lease_updated_adds_non_replayable_turn_notice
cargo test -p codex-tui account_lease_updated_adds_no_eligible_account_error_notice
cargo test -p codex-tui update_prompt_snapshot
```

Expected: app-server pooled read/notification behavior stays coherent, TUI lease-status rendering still exposes fork-specific pool state, and update-prompt UI still points at the intended release metadata flow. As above, treat `ignored`, `0 tests`, or sandbox skip output as unsatisfied locally and require CI execution before merge.

- [ ] **Step 10: Update final verification log**

Append:

```markdown
### rust-v0.122.0 Final Verification

- cargo test -p codex-account-pool:
- cargo test -p codex-login:
- cargo test -p codex-state:
- cargo test -p codex-core:
- cargo test -p codex-app-server:
- cargo test -p codex-app-server-protocol:
- cargo test -p codex-tui:
- just fmt:
- just fix -p results:
- full workspace test or required CI green:
- sandbox-gated runtime regressions executed locally or CI:
- release/install/update grep review:
- product identity/home-dir/startup regressions:
- migration/leased-auth/realtime/review/subagent regressions:
- app-server lease/TUI status regressions:
- identity grep review:
- remaining non-core follow-ups:
```

- [ ] **Step 11: Stage final verification log**

Run:

```bash
git add docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md
```

Expected: execution log is staged.

## Task 15: Commit Final Sync and Prepare Handoff

**Files:**
- Modify: `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`
- Dynamic: all files changed by the final sync branch

- [ ] **Step 1: Confirm final branch has no unresolved conflicts**

Run:

```bash
git status --short --branch
git diff --name-only --diff-filter=U
```

Expected: no unresolved conflict paths.

- [ ] **Step 2: Review final diff summary**

Run:

```bash
git diff --stat HEAD
git diff --name-status HEAD
```

Expected: changed files match the sync scope. Unexpected unrelated changes should be investigated before committing.

- [ ] **Step 3: Commit the final 0.122 merge**

Run:

```bash
git add -A
git commit -m "merge: sync upstream rust-v0.122.0"
```

Expected: merge commit succeeds and includes `rust-v0.122.0` as a parent.

- [ ] **Step 4: Push the final sync branch**

Run:

```bash
git push origin sync/rust-v0.122.0
```

Expected: push succeeds.

- [ ] **Step 5: Prepare PR description**

Use this structure:

```markdown
## Summary

- Syncs mcodex to upstream stable rust-v0.122.0 through an internal rust-v0.121.0 checkpoint.
- Preserves mcodex identity, account-pool, lease-auth, app-server pooled API, and OSS release/update/install contracts.
- Regenerates derived schemas/lockfiles/snapshots as required.

## Core Contract Checks

- mcodex identity:
- login/startup:
- account-pool/lease auth:
- app-server pooled API/schema:
- release/update/install:

## Verification

- cargo test -p codex-account-pool:
- cargo test -p codex-login:
- cargo test -p codex-state:
- cargo test -p codex-core:
- cargo test -p codex-app-server:
- cargo test -p codex-app-server-protocol:
- cargo test -p codex-tui:
- just fmt:
- just fix -p:
- bazel lock:
- schema generation:
- full workspace test or CI:

## Follow-Ups

- None, or list only non-core deferred items.
```

- [ ] **Step 6: If the broad gate was deferred locally, wait for required CI before any merge decision**

Check the pushed branch or PR in GitHub and confirm the required status checks are green.

Expected: if Task 14 deferred the full workspace suite to CI, or if any sandbox-gated runtime regressions from Task 14 were marked CI-required, do not ask to merge and do not merge until required CI is green. Recording the deferral in the execution log is not sufficient by itself.

- [ ] **Step 7: Decide when to merge to main**

Because the user is developing on `main`, do not merge automatically. Ask:

```text
The sync branch is ready. Do you want to merge it after current main development lands, or should I rebase/merge latest main into sync/rust-v0.122.0 first and re-run the final gate?
```

Expected: ask this only after the broad gate is satisfied by either a local full-workspace pass or required CI green, then wait for maintainer direction.

## Task 16: Post-Merge Cleanup

**Files:**
- Read: `docs/superpowers/plans/2026-04-21-upstream-stable-sync-execution-log.md`

- [ ] **Step 1: After final branch merges to main, verify main contains the target tag merge**

Run:

```bash
git switch main
git pull --ff-only origin main
git merge-base --is-ancestor rust-v0.122.0 main
```

Expected: command exits 0.

- [ ] **Step 2: Delete ephemeral local worktrees only after review/bisect no longer needs them**

Run only after maintainer approval:

```bash
git worktree remove .worktrees/sync-rust-v0.121.0-base
git worktree remove .worktrees/sync-rust-v0.122.0
```

Expected: worktrees removed.

- [ ] **Step 3: Delete checkpoint branch only after approval**

Run:

```bash
git branch -d sync/rust-v0.121.0-base
```

Expected: branch deletion succeeds. Do not delete `sync/rust-v0.122.0` until the PR is merged and no longer needed.

- [ ] **Step 4: Record final sync status**

Append to the execution log on `main` if it is retained:

```markdown
## Final Status

- Merged to main:
- Upstream stable tag contained by main:
- Checkpoint branch cleanup:
- Remaining follow-ups:
```

Expected: final project state is auditable.
