# Mcodex Smoke E2E Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first runtime/quota mcodex smoke E2E merge gate from
`docs/superpowers/specs/2026-04-27-mcodex-smoke-e2e-expansion-design.md`.

**Architecture:** Keep the merge gate additive and outside normal product
startup paths: shell scripts enumerate exact named Rust regressions, `just`
recipes expose stable smoke commands, and two missing behavior gaps are covered
in the existing `codex-core --test all` account-pool integration surface. The
gate fails closed when critical tests are skipped, ignored, missing, duplicated,
or run from a Codex sandbox with `CODEX_SANDBOX_NETWORK_DISABLED` set.

**Tech Stack:** Rust integration tests, `codex-core` test support,
`wiremock`, POSIX shell, `just`, `cargo test`, @superpowers:test-driven-development,
@superpowers:verification-before-completion.

---

## Scope

Implement this first expansion slice:

- `just smoke-mcodex-runtime-gate`
- `just smoke-mcodex-quota-gate`
- `just smoke-mcodex-gate`
- compatibility aliases `just smoke-mcodex-runtime` and
  `just smoke-mcodex-quota`
- one named-test runner script that verifies exact test discovery and exact
  execution
- two missing `codex-core --test all` regressions:
  - `suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure`
  - `suite::account_pool::second_runtime_skips_account_leased_by_first_runtime`
- P0 smoke runbook updates documenting automated rows, command compatibility,
  cost measurement, and deferred app-server/TUI/remote rows.

Do not implement in this slice:

- `smoke-mcodex-app-server-gate`
- `smoke-mcodex-tui-gate`
- `smoke-mcodex-e2e`
- `smoke-mcodex-installer`
- `smoke-mcodex-remote-contract`
- new `codex-smoke-fixtures` scenarios
- real account quota exhaustion
- product runtime or account-pool policy rewrites
- any change to the existing meaning of `just smoke-mcodex-all`

## Merge-Risk Boundaries

- Keep all new command orchestration in `scripts/smoke/` and `justfile`.
- Keep new product behavior assertions in `codex-rs/core/tests/suite/account_pool.rs`.
- Do not add a new crate or Rust dependency.
- Do not touch `codex-core` production modules unless a new regression exposes
  a real bug.
- Do not change `ConfigToml`; no schema regeneration should be needed.
- Do not change `Cargo.toml` or `Cargo.lock`; no Bazel lock update should be
  needed.
- Leave proxy and artifact environment variables untouched:
  `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`,
  `CARGO_NET_GIT_FETCH_WITH_CLI`, `RUSTY_V8_ARCHIVE`, `LK_CUSTOM_WEBRTC`, and
  `CARGO_TARGET_DIR`.
- Do not rely on executable bits for new scripts; call them through `sh`.

## Planned File Layout

- Add: `scripts/smoke/run-named-cargo-tests.sh`
  - Generic named-test runner for runtime/quota smoke gates.
- Add: `scripts/smoke/test-run-named-cargo-tests.sh`
  - Fast shell self-test using a fake `cargo` executable so runner failure
    modes are covered without compiling Rust.
- Add: `scripts/smoke/mcodex-runtime-gate.tests`
  - Runtime gate descriptor file.
- Add: `scripts/smoke/mcodex-quota-gate.tests`
  - Quota gate descriptor file.
- Add: `scripts/smoke/mcodex-runtime-gate.sh`
  - Thin wrapper over the runner for runtime descriptors.
- Add: `scripts/smoke/mcodex-quota-gate.sh`
  - Thin wrapper over the runner for quota descriptors.
- Add: `scripts/smoke/mcodex-gate.sh`
  - Aggregate local + CLI + runtime + quota gate without redefining
    `smoke-mcodex-all`.
- Modify: `justfile`
  - Add runtime/quota/gate recipes and runtime/quota aliases.
- Modify: `codex-rs/core/tests/suite/account_pool.rs`
  - Add the two missing account-pool regressions and any shared helper only if
    it is used at least twice.
- Modify: `docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md`
  - Update automated command rows, compatibility table, run guidance, cost
    measurement, and deferred rows.

## Descriptor Format

Use a small pipe-delimited descriptor file so recipes stay short and future
gates can reuse the same runner:

```text
# gate|package|target_kind|target_name|exact_test_path|timeout_secs|notes
runtime|codex-core|--test|all|suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure|120|sticky account
runtime|codex-core|--lib|-|codex_delegate::tests::run_codex_thread_interactive_inherits_parent_runtime_lease_host|120|subagent lease inheritance
```

Rules:

- `gate` must be `runtime` or `quota` in this slice.
- `target_kind` must be `--test` or `--lib`.
- `target_name` is required for `--test` and must be `-` for `--lib`.
- `exact_test_path` is passed to cargo as the test filter and verified with
  `-- --exact`.
- `timeout_secs` is per-test and must be a positive integer.
- `notes` is for operator output only.
- Blank lines and `#` comments are ignored.

## Shared Verification Environment

When running cargo-based smoke locally, prefer:

```bash
export HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}"
export LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}"
```

Do not hardcode these values into scripts. They are local operator inputs, not
repo defaults.

Before runtime/quota smoke, verify the command is not running inside a
network-disabled Codex sandbox:

```bash
test -z "${CODEX_SANDBOX_NETWORK_DISABLED:-}"
```

Expected: exit code `0`. If it fails, runtime/quota smoke must fail with a
message telling the operator to rerun outside the Codex sandbox or in
network-enabled CI.

---

## Task 0: Preflight And Cost Baseline

**Files:**

- Read: `docs/superpowers/specs/2026-04-27-mcodex-smoke-e2e-expansion-design.md`
- Read: `justfile`
- Read: `scripts/smoke/mcodex-local.sh`
- Read: `scripts/smoke/mcodex-cli.sh`
- Read: `codex-rs/core/tests/suite/account_pool.rs`
- Read: `docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md`

- [ ] **Step 1: Confirm branch and worktree**

Run:

```bash
git status --short --untracked-files=all
git branch --show-current
```

Expected: working tree is clean or only this task's intentional changes are
present. Branch is the intended implementation branch.

- [ ] **Step 2: Confirm current command baseline**

Run:

```bash
rg -n "smoke-mcodex-(local|cli|all|runtime|quota|gate)" justfile scripts docs/superpowers/runbooks
```

Expected: `smoke-mcodex-local`, `smoke-mcodex-cli`, and `smoke-mcodex-all`
exist; runtime/quota/gate commands do not exist yet, or only appear in docs as
future rows.

- [ ] **Step 3: Confirm required exact tests currently present or missing**

Run:

```bash
rg -n "normal_turns_remain_on_same_account_without_quota_pressure|second_runtime_skips_account_leased_by_first_runtime|pooled_request_uses_lease_scoped_auth_session_not_shared_auth_snapshot|long_running_turn_heartbeat_keeps_lease_exclusive|nearing_limit_snapshot_rotates_the_next_turn_before_exhaustion|pooled_fail_closed_turn_without_eligible_lease_does_not_open_startup_websocket|run_codex_thread_interactive_inherits_parent_runtime_lease_host" codex-rs/core
```

Expected:

- The two new required tests are absent.
- Existing runtime/quota/subagent exact tests are present.

- [ ] **Step 4: Record local disk baseline for the runbook**

Run:

```bash
df -h .
du -sh codex-rs/target 2>/dev/null || true
```

Expected: capture free disk and current target size. The runbook later uses
these commands and warns if free disk is below 20 GB before cold runtime/quota
smoke.

- [ ] **Step 5: Commit preflight documentation only if it changed files**

If Task 0 only inspected state, do not commit.

---

## Task 1: Add Runner Self-Tests First

**Files:**

- Add: `scripts/smoke/test-run-named-cargo-tests.sh`
- Test target: `scripts/smoke/run-named-cargo-tests.sh` once Task 2 adds it

- [ ] **Step 1: Write a failing shell self-test**

Create `scripts/smoke/test-run-named-cargo-tests.sh` with this structure:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
RUNNER="$SCRIPT_DIR/run-named-cargo-tests.sh"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/mcodex-runner-test.XXXXXX")

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

assert_fails() {
  name=$1
  shift
  if "$@" >"$TMP_DIR/$name.out" 2>"$TMP_DIR/$name.err"; then
    echo "expected failure for $name" >&2
    cat "$TMP_DIR/$name.out" >&2
    cat "$TMP_DIR/$name.err" >&2
    exit 1
  fi
}

assert_passes() {
  name=$1
  shift
  if ! "$@" >"$TMP_DIR/$name.out" 2>"$TMP_DIR/$name.err"; then
    echo "expected success for $name" >&2
    cat "$TMP_DIR/$name.out" >&2
    cat "$TMP_DIR/$name.err" >&2
    exit 1
  fi
}

write_descriptor() {
  file=$1
  exact=${2:-suite::account_pool::exact_test}
  cat >"$file" <<EOF
runtime|codex-core|--test|all|$exact|30|fake descriptor
EOF
}

write_fake_cargo() {
  mode=$1
  fake_dir="$TMP_DIR/$mode-bin"
  mkdir -p "$fake_dir"
  cat >"$fake_dir/cargo" <<'EOF'
#!/bin/sh
set -eu
mode=${FAKE_CARGO_MODE:?}
args=" $* "
exact="suite::account_pool::exact_test"
printf '%s\n' "$args" >> "${FAKE_CARGO_ARGS_LOG:-/dev/null}"
if printf '%s' "$args" | grep -Fq " --no-run"; then
  printf 'fake cargo warm build\n'
  exit 0
fi
case "$mode" in
  ok)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
    else
      echo "unexpected ok invocation: $args" >&2
      exit 2
    fi
    ;;
  missing)
    if printf '%s' "$args" | grep -Fq " --list"; then
      true
    else
      echo "missing mode should fail during list, not run" >&2
      exit 2
    fi
    ;;
  duplicate)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n%s: test\n' "$exact" "$exact"
    else
      echo "duplicate mode should fail during list, not run" >&2
      exit 2
    fi
    ;;
  skipped)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'Skipping test because it cannot execute when network is disabled in a Codex sandbox.\n'
      printf 'test %s ... ok\n' "$exact"
    else
      echo "unexpected skipped invocation: $args" >&2
      exit 2
    fi
    ;;
  ignored)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ignored\n' "$exact"
    else
      echo "unexpected ignored invocation: $args" >&2
      exit 2
    fi
    ;;
  no-proof)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
    else
      echo "unexpected no-proof invocation: $args" >&2
      exit 2
    fi
    ;;
  *)
    echo "unknown fake cargo mode: $mode" >&2
    exit 2
    ;;
esac
EOF
  chmod +x "$fake_dir/cargo"
  printf '%s\n' "$fake_dir"
}

descriptor="$TMP_DIR/tests.txt"
write_descriptor "$descriptor"

ok_bin=$(write_fake_cargo ok)
missing_bin=$(write_fake_cargo missing)
duplicate_bin=$(write_fake_cargo duplicate)
skipped_bin=$(write_fake_cargo skipped)
ignored_bin=$(write_fake_cargo ignored)
no_proof_bin=$(write_fake_cargo no-proof)

assert_passes ok env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$TMP_DIR/ok-cargo-args.log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$descriptor"
if ! grep -Fq -- " --exact --nocapture" "$TMP_DIR/ok-cargo-args.log"; then
  echo "expected runner to invoke cargo with --exact --nocapture" >&2
  cat "$TMP_DIR/ok-cargo-args.log" >&2
  exit 1
fi
assert_fails missing env FAKE_CARGO_MODE=missing PATH="$missing_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails duplicate env FAKE_CARGO_MODE=duplicate PATH="$duplicate_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails skipped env FAKE_CARGO_MODE=skipped PATH="$skipped_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails ignored env FAKE_CARGO_MODE=ignored PATH="$ignored_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails no_proof env FAKE_CARGO_MODE=no-proof PATH="$no_proof_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails sandbox env CODEX_SANDBOX_NETWORK_DISABLED=1 FAKE_CARGO_MODE=ok PATH="$ok_bin:$PATH" sh "$RUNNER" "$descriptor"

echo "test-run-named-cargo-tests: pass"
```

The test intentionally references the runner before it exists.

- [ ] **Step 2: Run the self-test and verify it fails for the right reason**

Run:

```bash
sh scripts/smoke/test-run-named-cargo-tests.sh
```

Expected: fails because `scripts/smoke/run-named-cargo-tests.sh` is missing.

- [ ] **Step 3: Keep the failing self-test uncommitted**

Do not commit a deliberately failing test by itself. Leave it in the working
tree and commit it together with the runner after Task 2 makes the self-test
pass.

---

## Task 2: Implement The Named Cargo Test Runner

**Files:**

- Add: `scripts/smoke/run-named-cargo-tests.sh`
- Test: `scripts/smoke/test-run-named-cargo-tests.sh`

- [ ] **Step 1: Add the runner implementation**

Create `scripts/smoke/run-named-cargo-tests.sh`:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
MANIFEST_PATH="$REPO_ROOT/codex-rs/Cargo.toml"
SKIP_SENTINEL="Skipping test because it cannot execute when network is disabled in a Codex sandbox."
DESCRIPTOR_FILE=${1:-}

if [ -z "$DESCRIPTOR_FILE" ]; then
  echo "usage: sh scripts/smoke/run-named-cargo-tests.sh <descriptor-file>" >&2
  exit 2
fi

if [ ! -f "$DESCRIPTOR_FILE" ]; then
  echo "descriptor file not found: $DESCRIPTOR_FILE" >&2
  exit 2
fi

line_number=0
targets_seen=""
while IFS= read -r line || [ -n "$line" ]; do
  line_number=$((line_number + 1))
  case "$line" in
    ""|\#*) continue ;;
  esac

  old_ifs=$IFS
  IFS='|'
  set -- $line
  IFS=$old_ifs

  if [ "$#" -ne 7 ]; then
    echo "invalid descriptor at $DESCRIPTOR_FILE:$line_number: expected 7 fields" >&2
    exit 2
  fi

  gate=$1
  package=$2
  target_kind=$3
  target_name=$4
  exact_path=$5
  timeout_secs=$6
  notes=$7

  case "$gate" in
    runtime|quota) ;;
    *)
      echo "invalid gate '$gate' at $DESCRIPTOR_FILE:$line_number" >&2
      exit 2
      ;;
  esac

  if [ -n "${CODEX_SANDBOX_NETWORK_DISABLED:-}" ]; then
    echo "$gate gate cannot run with CODEX_SANDBOX_NETWORK_DISABLED set; rerun outside the Codex sandbox or in network-enabled CI" >&2
    exit 1
  fi

  case "$target_kind:$target_name" in
    --test:-)
      echo "invalid target '$target_kind|$target_name' at $DESCRIPTOR_FILE:$line_number: --test requires a target name" >&2
      exit 2
      ;;
    --test:*) ;;
    --lib:-) ;;
    --lib:*)
      echo "invalid target '$target_kind|$target_name' at $DESCRIPTOR_FILE:$line_number: --lib target name must be '-'" >&2
      exit 2
      ;;
    *)
      echo "invalid target '$target_kind|$target_name' at $DESCRIPTOR_FILE:$line_number" >&2
      exit 2
      ;;
  esac

  case "$timeout_secs" in
    ''|*[!0-9]*)
      echo "invalid timeout '$timeout_secs' at $DESCRIPTOR_FILE:$line_number" >&2
      exit 2
      ;;
  esac
  if [ "$timeout_secs" -le 0 ]; then
    echo "invalid timeout '$timeout_secs' at $DESCRIPTOR_FILE:$line_number" >&2
    exit 2
  fi

  target_key="$package|$target_kind|$target_name"
  case "
$targets_seen
" in
    *"
$target_key
"*) ;;
    *)
      echo "warming target package=$package target=$target_kind $target_name"
      if [ "$target_kind" = "--lib" ]; then
        cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" --no-run
      else
        cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$target_name" --no-run
      fi
      targets_seen="${targets_seen}
$target_key"
      ;;
  esac

  tmp_list=$(mktemp "${TMPDIR:-/tmp}/mcodex-smoke-list.XXXXXX")
  tmp_run=$(mktemp "${TMPDIR:-/tmp}/mcodex-smoke-run.XXXXXX")
  cleanup_current() {
    rm -f "$tmp_list" "$tmp_run"
  }

  echo "listing gate=$gate package=$package target=$target_kind $target_name test=$exact_path"
  if [ "$target_kind" = "--lib" ]; then
    cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$exact_path" -- --exact --list >"$tmp_list" 2>&1 || {
      echo "failed to list named regression: $exact_path" >&2
      cat "$tmp_list" >&2
      cleanup_current
      exit 1
    }
  else
    cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$target_name" "$exact_path" -- --exact --list >"$tmp_list" 2>&1 || {
      echo "failed to list named regression: $exact_path" >&2
      cat "$tmp_list" >&2
      cleanup_current
      exit 1
    }
  fi

  match_count=$(grep -Fxc "$exact_path: test" "$tmp_list" || true)
  if [ "$match_count" -ne 1 ]; then
    echo "named regression not found exactly once: $exact_path (matches=$match_count)" >&2
    cat "$tmp_list" >&2
    cleanup_current
    exit 1
  fi

  echo "running gate=$gate package=$package target=$target_kind $target_name timeout=${timeout_secs}s test=$exact_path notes=$notes"
  start_epoch=$(date +%s)
  if ! command -v perl >/dev/null 2>&1; then
    echo "perl is required to enforce per-test smoke timeouts" >&2
    cleanup_current
    exit 2
  fi

  if [ "$target_kind" = "--lib" ]; then
    perl -e 'alarm shift; exec @ARGV' "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$exact_path" -- --exact --nocapture >"$tmp_run" 2>&1 || {
      cat "$tmp_run" >&2
      cleanup_current
      exit 1
    }
  else
    perl -e 'alarm shift; exec @ARGV' "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$target_name" "$exact_path" -- --exact --nocapture >"$tmp_run" 2>&1 || {
      cat "$tmp_run" >&2
      cleanup_current
      exit 1
    }
  fi
  elapsed=$(( $(date +%s) - start_epoch ))

  if grep -Fq "$SKIP_SENTINEL" "$tmp_run"; then
    echo "critical regression skipped because network is disabled: $exact_path" >&2
    cat "$tmp_run" >&2
    cleanup_current
    exit 1
  fi
  if grep -Fq "test $exact_path ... ignored" "$tmp_run"; then
    echo "critical regression ignored: $exact_path" >&2
    cat "$tmp_run" >&2
    cleanup_current
    exit 1
  fi
  proof_count=$(grep -Fxc "test $exact_path ... ok" "$tmp_run" || true)
  if [ "$proof_count" -ne 1 ]; then
    echo "critical regression did not prove exact execution once: $exact_path (proof=$proof_count)" >&2
    cat "$tmp_run" >&2
    cleanup_current
    exit 1
  fi

  cat "$tmp_run"
  echo "passed gate=$gate package=$package target=$target_kind $target_name elapsed=${elapsed}s test=$exact_path"
  cleanup_current
done <"$DESCRIPTOR_FILE"

echo "run-named-cargo-tests: pass descriptor=$DESCRIPTOR_FILE"
```

Implementation notes:

- Keep this POSIX shell compatible.
- The `perl` alarm wrapper provides a per-test timeout on macOS without adding
  a GNU `timeout` dependency. If `perl` is unavailable, the runner must fail
  before running cargo because runtime/quota gates require explicit per-test
  timeouts.
- If shellcheck is available and complains about the multi-line `targets_seen`
  pattern, prefer a temp file over a shell array because POSIX `sh` arrays are
  unavailable.

- [ ] **Step 2: Run the self-test**

Run:

```bash
sh scripts/smoke/test-run-named-cargo-tests.sh
```

Expected:

```text
test-run-named-cargo-tests: pass
```

- [ ] **Step 3: Syntax-check smoke scripts**

Run:

```bash
sh -n scripts/smoke/run-named-cargo-tests.sh
sh -n scripts/smoke/test-run-named-cargo-tests.sh
```

Expected: no output and exit code `0`.

- [ ] **Step 4: Commit the runner**

```bash
git add scripts/smoke/run-named-cargo-tests.sh scripts/smoke/test-run-named-cargo-tests.sh
git commit -m "test: add named cargo smoke runner"
```

---

## Task 3: Add Sticky Account Regression

**Files:**

- Modify: `codex-rs/core/tests/suite/account_pool.rs`

- [ ] **Step 1: Add the failing regression before changing product code**

Add this test near the existing quota rotation tests in
`codex-rs/core/tests/suite/account_pool.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn normal_turns_remain_on_same_account_without_quota_pressure() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_response_sequence(
        &server,
        vec![
            sse_with_primary_usage_percent("resp-1", 12.0),
            sse_with_primary_usage_percent("resp-2", 13.0),
        ],
    )
    .await;

    let mut builder = pooled_accounts_builder().with_config(|config| {
        config
            .accounts
            .as_mut()
            .expect("pooled accounts config")
            .min_switch_interval_secs = Some(0);
    });
    let test = builder.build(&server).await?;
    seed_two_accounts(&test).await?;

    let first_turn_error = submit_turn_and_wait(&test, "normal sticky turn 1").await?;
    assert!(first_turn_error.is_none());

    let second_turn_error = submit_turn_and_wait(&test, "normal sticky turn 2").await?;
    assert!(second_turn_error.is_none());

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2, "expected one request per normal turn");
    assert_account_ids_in_order(&requests, &[PRIMARY_ACCOUNT_ID, PRIMARY_ACCOUNT_ID]);

    let selected_event_type = event_type_name(AccountPoolEventType::ProactiveSwitchSelected);
    let events = list_account_pool_events(&test).await?;
    assert!(
        events
            .iter()
            .all(|event| event.event_type != selected_event_type),
        "normal turns without quota pressure should not emit automatic switch events: {events:#?}"
    );

    Ok(())
}
```

If the test passes immediately, treat that as existing behavior now covered by
an explicit regression. Do not invent product changes just to force a red test.

- [ ] **Step 2: Run the exact test**

Run:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test -p codex-core --test all suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure -- --exact --nocapture
```

Expected:

- If it fails, failure should identify a real sticky-account bug.
- If it passes, output includes:

```text
test suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure ... ok
```

- [ ] **Step 3: Fix product code only if the new regression exposes a bug**

If the test fails because account selection switches without quota/lease
pressure, inspect the account-pool authority and runtime lease selection path.
Keep any fix minimal and covered by the new test.

Do not change product code if the regression already passes.

- [ ] **Step 4: Format after Rust changes**

Run:

```bash
cd codex-rs
just fmt
```

Expected: rustfmt completes successfully.

- [ ] **Step 5: Commit the sticky regression**

```bash
git add codex-rs/core/tests/suite/account_pool.rs
git commit -m "test: cover sticky pooled account turns"
```

---

## Task 4: Add Multi-Instance Lease Regression

**Files:**

- Modify: `codex-rs/core/tests/suite/account_pool.rs`

- [ ] **Step 1: Add the two-account branch first**

Add this test near `shutdown_releases_active_lease_for_next_runtime` and
`long_running_turn_heartbeat_keeps_lease_exclusive`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_runtime_skips_account_leased_by_first_runtime() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response_mock = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_assistant_message("m1", "first runtime"),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("m2", "second runtime"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    let shared_home = Arc::new(TempDir::new()?);
    let mut first_builder = pooled_accounts_builder().with_home(Arc::clone(&shared_home));
    let first = first_builder.build(&server).await?;
    seed_two_accounts(&first).await?;

    let first_turn_error = submit_turn_and_wait(&first, "first runtime turn").await?;
    assert!(first_turn_error.is_none());
    wait_for_active_pool_lease(&first, PRIMARY_ACCOUNT_ID, Duration::from_secs(30)).await?;

    let mut second_builder = pooled_accounts_builder().with_home(Arc::clone(&shared_home));
    let second = second_builder.build(&server).await?;

    let second_turn_error = submit_turn_and_wait(&second, "second runtime turn").await?;
    assert!(
        second_turn_error.is_none(),
        "second runtime should use another eligible account while the first lease is live"
    );

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2, "expected one request per runtime");
    assert_account_ids_in_order(&requests, &[PRIMARY_ACCOUNT_ID, SECONDARY_ACCOUNT_ID]);

    second.codex.shutdown_and_wait().await?;
    first.codex.shutdown_and_wait().await?;

    Ok(())
}
```

This branch proves runtime B does not reuse account A while account A has a
live lease and account B is eligible.

- [ ] **Step 2: Run the exact test**

Run:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test -p codex-core --test all suite::account_pool::second_runtime_skips_account_leased_by_first_runtime -- --exact --nocapture
```

Expected:

```text
test suite::account_pool::second_runtime_skips_account_leased_by_first_runtime ... ok
```

or a real bug showing runtime B can reuse runtime A's live lease.

- [ ] **Step 3: Add the all-unavailable branch without creating a one-use helper**

Extend the same test after the successful two-account branch, or add a nested
block in the same function, to prove a second runtime fails closed when all
available accounts are already leased. Use a fresh `MockServer` and
`TempDir` so requests from the first branch do not affect the assertion:

```rust
{
    let server = start_mock_server().await;
    mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_assistant_message("m1", "single account holder"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let shared_home = Arc::new(TempDir::new()?);
    let mut first_builder = pooled_accounts_builder().with_home(Arc::clone(&shared_home));
    let first = first_builder.build(&server).await?;
    seed_account(&first, PRIMARY_ACCOUNT_ID).await?;

    let first_turn_error = submit_turn_and_wait(&first, "single account first runtime").await?;
    assert!(first_turn_error.is_none());
    wait_for_active_pool_lease(&first, PRIMARY_ACCOUNT_ID, Duration::from_secs(30)).await?;

    let mut second_builder = pooled_accounts_builder().with_home(Arc::clone(&shared_home));
    let second = second_builder.build(&server).await?;
    let second_turn_error = submit_turn_and_wait(&second, "single account second runtime").await?;
    let second_turn_error = second_turn_error
        .expect("second runtime should fail closed when every account is leased");
    assert!(
        second_turn_error
            .message
            .to_ascii_lowercase()
            .contains("pooled account"),
        "unexpected fail-closed error: {}",
        second_turn_error.message
    );

    let requests = server.received_requests().await.unwrap_or_default();
    let responses_requests = requests
        .iter()
        .filter(|request| {
            request.method == Method::POST && request.url.path().ends_with("/responses")
        })
        .count();
    assert_eq!(
        responses_requests, 1,
        "second runtime must fail closed instead of sending a request with the leased account"
    );

    second.codex.shutdown_and_wait().await?;
    first.codex.shutdown_and_wait().await?;
}
```

If this nested block makes the function too long, extract a helper only if it
is used by both branches. Otherwise keep the test explicit.

- [ ] **Step 4: Re-run the exact test**

Run:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test -p codex-core --test all suite::account_pool::second_runtime_skips_account_leased_by_first_runtime -- --exact --nocapture
```

Expected:

```text
test suite::account_pool::second_runtime_skips_account_leased_by_first_runtime ... ok
```

- [ ] **Step 5: Fix product code only if the regression exposes a real lease bug**

If runtime B reuses runtime A's live account, inspect runtime lease authority
selection. Keep the fix narrow and do not rewrite account-pool abstractions.

- [ ] **Step 6: Format after Rust changes**

Run:

```bash
cd codex-rs
just fmt
```

Expected: rustfmt completes successfully.

- [ ] **Step 7: Commit the multi-instance regression**

```bash
git add codex-rs/core/tests/suite/account_pool.rs
git commit -m "test: cover pooled lease exclusivity across runtimes"
```

---

## Task 5: Add Runtime And Quota Gate Descriptors And Wrappers

**Files:**

- Add: `scripts/smoke/mcodex-runtime-gate.tests`
- Add: `scripts/smoke/mcodex-quota-gate.tests`
- Add: `scripts/smoke/mcodex-runtime-gate.sh`
- Add: `scripts/smoke/mcodex-quota-gate.sh`
- Test: `scripts/smoke/run-named-cargo-tests.sh`

- [ ] **Step 1: Add runtime descriptors**

Create `scripts/smoke/mcodex-runtime-gate.tests`:

```text
# gate|package|target_kind|target_name|exact_test_path|timeout_secs|notes
runtime|codex-core|--test|all|suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure|120|sticky account without quota pressure
runtime|codex-core|--test|all|suite::account_pool::second_runtime_skips_account_leased_by_first_runtime|180|cross-runtime lease exclusivity
runtime|codex-core|--test|all|suite::account_pool::pooled_request_uses_lease_scoped_auth_session_not_shared_auth_snapshot|120|lease-scoped auth snapshot
runtime|codex-core|--test|all|suite::account_pool::pooled_request_ignores_shared_external_auth_when_lease_is_active|120|active lease ignores shared external auth
runtime|codex-core|--test|all|suite::account_pool::lease_rotation_updates_live_snapshot_to_the_new_lease|120|live snapshot follows lease rotation
runtime|codex-core|--test|all|suite::account_pool::long_running_turn_heartbeat_keeps_lease_exclusive|60|known slow heartbeat exclusivity test
runtime|codex-core|--test|all|suite::account_pool::shutdown_releases_active_lease_for_next_runtime|120|shutdown releases lease
runtime|codex-core|--lib|-|codex_delegate::tests::run_codex_thread_interactive_inherits_parent_runtime_lease_host|120|subagent inherits parent runtime lease host
runtime|codex-core|--lib|-|codex_delegate::tests::run_codex_thread_interactive_drops_inherited_lease_auth_when_runtime_host_exists|120|subagent drops inherited auth when runtime host exists
```

The known-slow heartbeat descriptor intentionally has a shorter explicit
timeout than a generic "hang forever" run, but it must be high enough for the
20-second delayed response plus test overhead.

- [ ] **Step 2: Add quota descriptors**

Create `scripts/smoke/mcodex-quota-gate.tests`:

```text
# gate|package|target_kind|target_name|exact_test_path|timeout_secs|notes
quota|codex-core|--test|all|suite::account_pool::nearing_limit_snapshot_rotates_the_next_turn_before_exhaustion|120|soft quota rotates future turn
quota|codex-core|--test|all|suite::account_pool::usage_limit_reached_rotates_only_future_turns_on_responses_transport|120|hard usage limit rotates only future turns
quota|codex-core|--test|all|suite::account_pool::hard_failover_uses_active_limit_family_through_runtime_authority|120|hard failover uses active limit family
quota|codex-core|--test|all|suite::account_pool::proactive_rotation_does_not_immediately_switch_back_to_just_replaced_account|120|damping prevents switch churn
quota|codex-core|--test|all|suite::account_pool::account_lease_snapshot_reports_proactive_switch_suppression_without_rate_limited_health|120|snapshot reports damping without rate-limited health
quota|codex-core|--test|all|suite::account_pool::exhausted_pool_fails_closed_without_legacy_auth_fallback|120|no eligible account fails closed
quota|codex-core|--test|all|suite::client_websockets::pooled_fail_closed_turn_without_eligible_lease_does_not_open_startup_websocket|120|fail closed does not open startup websocket
```

- [ ] **Step 3: Add runtime wrapper**

Create `scripts/smoke/mcodex-runtime-gate.sh`:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
echo "smoke=runtime-gate"
echo "descriptor=$SCRIPT_DIR/mcodex-runtime-gate.tests"
sh "$SCRIPT_DIR/run-named-cargo-tests.sh" "$SCRIPT_DIR/mcodex-runtime-gate.tests"
echo "smoke-mcodex-runtime-gate: pass"
```

- [ ] **Step 4: Add quota wrapper**

Create `scripts/smoke/mcodex-quota-gate.sh`:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
echo "smoke=quota-gate"
echo "descriptor=$SCRIPT_DIR/mcodex-quota-gate.tests"
sh "$SCRIPT_DIR/run-named-cargo-tests.sh" "$SCRIPT_DIR/mcodex-quota-gate.tests"
echo "smoke-mcodex-quota-gate: pass"
```

- [ ] **Step 5: Syntax-check wrappers**

Run:

```bash
sh -n scripts/smoke/mcodex-runtime-gate.sh
sh -n scripts/smoke/mcodex-quota-gate.sh
```

Expected: no output and exit code `0`.

- [ ] **Step 6: Verify sandbox fail-closed behavior**

Run:

```bash
CODEX_SANDBOX_NETWORK_DISABLED=1 sh scripts/smoke/mcodex-runtime-gate.sh
```

Expected: non-zero exit and stderr includes:

```text
runtime gate cannot run with CODEX_SANDBOX_NETWORK_DISABLED set
```

Run:

```bash
CODEX_SANDBOX_NETWORK_DISABLED=1 sh scripts/smoke/mcodex-quota-gate.sh
```

Expected: non-zero exit and stderr includes:

```text
quota gate cannot run with CODEX_SANDBOX_NETWORK_DISABLED set
```

- [ ] **Step 7: Commit descriptors and wrappers**

```bash
git add scripts/smoke/mcodex-runtime-gate.tests scripts/smoke/mcodex-quota-gate.tests scripts/smoke/mcodex-runtime-gate.sh scripts/smoke/mcodex-quota-gate.sh
git commit -m "test: add runtime and quota smoke gate descriptors"
```

---

## Task 6: Add The Aggregate Gate And Just Recipes

**Files:**

- Add: `scripts/smoke/mcodex-gate.sh`
- Modify: `justfile`
- Test: `just --list`

- [ ] **Step 1: Add aggregate gate script**

Create `scripts/smoke/mcodex-gate.sh`:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

echo "smoke=gate"
sh "$SCRIPT_DIR/mcodex-local.sh" "$@"
sh "$SCRIPT_DIR/mcodex-cli.sh" "$@"
sh "$SCRIPT_DIR/mcodex-runtime-gate.sh"
sh "$SCRIPT_DIR/mcodex-quota-gate.sh"
echo "smoke-mcodex-gate: pass"
```

This script intentionally composes local + CLI + runtime + quota and does not
change `scripts/smoke/mcodex-local.sh`, `scripts/smoke/mcodex-cli.sh`, or
`just smoke-mcodex-all`.

- [ ] **Step 2: Add just recipes**

Modify the smoke section of `justfile`:

```make
[no-cd]
smoke-mcodex-runtime-gate *args:
    sh "{{ justfile_directory() }}/scripts/smoke/mcodex-runtime-gate.sh" "$@"

[no-cd]
smoke-mcodex-quota-gate *args:
    sh "{{ justfile_directory() }}/scripts/smoke/mcodex-quota-gate.sh" "$@"

[no-cd]
smoke-mcodex-gate *args:
    sh "{{ justfile_directory() }}/scripts/smoke/mcodex-gate.sh" "$@"

[no-cd]
smoke-mcodex-runtime *args:
    sh "{{ justfile_directory() }}/scripts/smoke/mcodex-runtime-gate.sh" "$@"

[no-cd]
smoke-mcodex-quota *args:
    sh "{{ justfile_directory() }}/scripts/smoke/mcodex-quota-gate.sh" "$@"
```

Do not modify:

```make
smoke-mcodex-all *args:
```

- [ ] **Step 3: Syntax-check aggregate script**

Run:

```bash
sh -n scripts/smoke/mcodex-gate.sh
```

Expected: no output and exit code `0`.

- [ ] **Step 4: Verify recipes are visible**

Run:

```bash
just --list | rg "smoke-mcodex-(runtime-gate|quota-gate|gate|runtime|quota|all)"
```

Expected: output contains all six command names, and `smoke-mcodex-all`
remains present.

- [ ] **Step 5: Verify alias fail-closed behavior**

Run:

```bash
CODEX_SANDBOX_NETWORK_DISABLED=1 just smoke-mcodex-runtime
```

Expected: non-zero exit and stderr includes the runtime sandbox failure
message.

Run:

```bash
CODEX_SANDBOX_NETWORK_DISABLED=1 just smoke-mcodex-quota
```

Expected: non-zero exit and stderr includes the quota sandbox failure message.

- [ ] **Step 6: Commit aggregate gate and recipes**

```bash
git add justfile scripts/smoke/mcodex-gate.sh
git commit -m "test: add mcodex smoke merge gate recipes"
```

---

## Task 7: Update The P0 Smoke Runbook

**Files:**

- Modify: `docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md`

- [ ] **Step 1: Add the new automated command table**

In the automated subset section, add or update a table like:

```markdown
| Command | Status | Coverage |
| --- | --- | --- |
| `just smoke-mcodex-local` | Automated | binary identity, home isolation, default-pool startup rows |
| `just smoke-mcodex-cli` | Automated | account status, pool show, diagnostics, events |
| `just smoke-mcodex-all` | Automated, compatibility aggregate | local + CLI only |
| `just smoke-mcodex-runtime-gate` | Automated, network-enabled shell/CI required | sticky account, lease-scoped auth, subagent lease inheritance, cross-runtime lease exclusivity, shutdown release |
| `just smoke-mcodex-quota-gate` | Automated, network-enabled shell/CI required | soft quota rotation, hard usage-limit failover, damping, fail-closed no eligible account |
| `just smoke-mcodex-gate` | Automated merge gate | local + CLI + runtime + quota |
```

- [ ] **Step 2: Add command compatibility notes**

Document:

```markdown
- `just smoke-mcodex-runtime` is a compatibility alias for
  `just smoke-mcodex-runtime-gate`.
- `just smoke-mcodex-quota` is a compatibility alias for
  `just smoke-mcodex-quota-gate`.
- `just smoke-mcodex-app-server` remains deferred until
  `smoke-mcodex-app-server-gate` lands.
- `just smoke-mcodex-all` remains the local+CLI compatibility aggregate. It is
  not silently broadened to include runtime/quota gates.
```

- [ ] **Step 3: Add network/sandbox failure guidance**

Document that runtime/quota/gate commands must be run from a network-enabled
local shell or CI. Include:

```bash
test -z "${CODEX_SANDBOX_NETWORK_DISABLED:-}"
```

Expected: exit code `0`.

If the variable is set, the expected failure message is:

```text
<runtime|quota> gate cannot run with CODEX_SANDBOX_NETWORK_DISABLED set; rerun outside the Codex sandbox or in network-enabled CI
```

- [ ] **Step 4: Add local artifact and proxy guidance**

Document optional local env:

```bash
export HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}"
export LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}"
```

Also document that scripts inherit these variables and do not set them.

- [ ] **Step 5: Add cost-measurement instructions and warning threshold**

Add a runbook subsection:

````markdown
## Runtime/Quota Gate Cost Capture

Before a cold run:

```bash
df -h .
du -sh codex-rs/target 2>/dev/null || true
```

Warn if free disk is below 20 GB before a cold `codex-core --test all` run.

Record cold and warm timings for:

```bash
time cargo test --manifest-path codex-rs/Cargo.toml -p codex-core --test all --no-run
time cargo test --manifest-path codex-rs/Cargo.toml -p codex-core --lib --no-run
time cargo test --manifest-path codex-rs/Cargo.toml -p codex-app-server --test all --no-run
time cargo test --manifest-path codex-rs/Cargo.toml -p codex-tui --lib --no-run
time just smoke-mcodex-runtime-gate
time just smoke-mcodex-quota-gate
time just smoke-mcodex-gate
```

After the cold runtime/quota gate:

```bash
du -sh codex-rs/target 2>/dev/null || true
```
````

Note that exact filters reduce test execution, not compile graph size, and
`codex-core --test all` may compile large dependencies such as `v8`.

- [ ] **Step 6: Add deferred app-server/TUI rows with exact future tests**

Keep the exact lists from the spec under a "Deferred Gate Rows" section and
state they are not part of this implementation slice.

- [ ] **Step 7: Add deferred remote contract note**

Document that remote contract smoke remains deferred, but list the exact rows
so future remote work does not lose the current contract shape:

```markdown
### Deferred Remote Contract Smoke

`just smoke-mcodex-remote-contract` is deferred until fake remote backend
support exists. The future gate must include:

| Row | Expected behavior |
| --- | --- |
| remote pool inventory read | Remote source exposes pool inventory without persisting remote-only secrets locally. |
| remote backend unavailable | Fails closed when no valid active lease exists. |
| remote pause state | Blocks startup with explicit pause source/provenance. |
| remote drain state | Prevents new selection while preserving observability facts. |
| remote quota facts | Reports authoritative remote quota facts with source/provenance. |
| absent remote-only facts | Represents facts explicitly as absent instead of synthesizing local SQLite facts. |
| remote lease acquire/release | Preserves the same sticky and fail-closed semantics as local leases. |
| remote lease expiry/revocation | Invalidates the active lease immediately. |
| remote lease-auth unavailable | Fails closed without falling back to local shared auth. |
| mirrored account identities | Uses stable mirrored ids for preferred/excluded accounts, not provider secrets. |
| secret non-persistence | Does not persist remote secrets or authority facts as local source-of-truth rows. |
| authority/source provenance | Every remote-derived output distinguishes remote facts from local cached observations. |
```

- [ ] **Step 8: Commit runbook updates**

```bash
git add docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md
git commit -m "docs: document runtime quota smoke gates"
```

---

## Task 8: Run Targeted Verification And Measure The Gate

**Files:**

- Read/verify: `scripts/smoke/*.sh`
- Read/verify: `scripts/smoke/*.tests`
- Read/verify: `justfile`
- Read/verify: `docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md`

- [ ] **Step 1: Run shell syntax checks**

Run:

```bash
sh -n scripts/smoke/run-named-cargo-tests.sh
sh -n scripts/smoke/test-run-named-cargo-tests.sh
sh -n scripts/smoke/mcodex-runtime-gate.sh
sh -n scripts/smoke/mcodex-quota-gate.sh
sh -n scripts/smoke/mcodex-gate.sh
```

Expected: no output and exit code `0`.

- [ ] **Step 2: Run runner self-tests**

Run:

```bash
sh scripts/smoke/test-run-named-cargo-tests.sh
```

Expected:

```text
test-run-named-cargo-tests: pass
```

- [ ] **Step 3: Verify the self-test covers exact execution flags**

Run:

```bash
rg -n -- "--exact --nocapture|FAKE_CARGO_ARGS_LOG" scripts/smoke/test-run-named-cargo-tests.sh
```

Expected: output shows the fake cargo argument log and an assertion that the
successful runner invocation included `--exact --nocapture`.

Then run:

```bash
sh scripts/smoke/test-run-named-cargo-tests.sh
```

Expected:

```text
test-run-named-cargo-tests: pass
```

- [ ] **Step 4: Measure package/target no-run cost**

Run from repo root in a network-enabled shell with local artifact/proxy env
already exported if needed:

```bash
df -h .
du -sh codex-rs/target 2>/dev/null || true
time cargo test --manifest-path codex-rs/Cargo.toml -p codex-core --test all --no-run
time cargo test --manifest-path codex-rs/Cargo.toml -p codex-core --lib --no-run
time cargo test --manifest-path codex-rs/Cargo.toml -p codex-app-server --test all --no-run
time cargo test --manifest-path codex-rs/Cargo.toml -p codex-tui --lib --no-run
du -sh codex-rs/target 2>/dev/null || true
```

Expected: commands complete successfully, or the implementer records the exact
blocked command and environment reason. Capture cold and warm timings when the
target was not already built. Warn before running if free disk is below 20 GB.

- [ ] **Step 5: Run exact new Rust regressions**

Run:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test -p codex-core --test all suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure -- --exact --nocapture
```

Expected:

```text
test suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure ... ok
```

Run:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test -p codex-core --test all suite::account_pool::second_runtime_skips_account_leased_by_first_runtime -- --exact --nocapture
```

Expected:

```text
test suite::account_pool::second_runtime_skips_account_leased_by_first_runtime ... ok
```

- [ ] **Step 6: Run existing subagent lease exact tests**

Run:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test -p codex-core --lib codex_delegate::tests::run_codex_thread_interactive_inherits_parent_runtime_lease_host -- --exact --nocapture
```

Expected:

```text
test codex_delegate::tests::run_codex_thread_interactive_inherits_parent_runtime_lease_host ... ok
```

Run:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test -p codex-core --lib codex_delegate::tests::run_codex_thread_interactive_drops_inherited_lease_auth_when_runtime_host_exists -- --exact --nocapture
```

Expected:

```text
test codex_delegate::tests::run_codex_thread_interactive_drops_inherited_lease_auth_when_runtime_host_exists ... ok
```

- [ ] **Step 7: Run the changed crate test suite**

Run after the exact new regressions pass:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test -p codex-core
```

Expected: `codex-core` tests pass. This is required because this plan modifies
`codex-rs/core/tests/suite/account_pool.rs`; the exact smoke gate proves named
coverage, but it does not replace the changed crate's test suite.

- [ ] **Step 8: Prepare full workspace verification checkpoint**

Because this plan changes `codex-core`, repo instructions require considering
the complete workspace test suite after targeted tests pass. Full `cargo test`
is disk-heavy, so stop at this checkpoint and ask the user before running:

```bash
cd codex-rs
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
cargo test
```

Expected if approved: full workspace tests pass. If the user defers this
because the smoke gate will be merged with other branches and tested together,
record that decision in the final implementation summary.

- [ ] **Step 9: Run runtime gate**

Run from repo root:

```bash
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
time just smoke-mcodex-runtime-gate
```

Expected:

```text
smoke-mcodex-runtime-gate: pass
```

and each descriptor prints a `passed gate=runtime ... test=<exact path>` line.

- [ ] **Step 10: Run quota gate**

Run from repo root:

```bash
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
time just smoke-mcodex-quota-gate
```

Expected:

```text
smoke-mcodex-quota-gate: pass
```

and each descriptor prints a `passed gate=quota ... test=<exact path>` line.

- [ ] **Step 11: Run aggregate gate**

Build `mcodex` if needed:

```bash
cd codex-rs
cargo build -p codex-cli --bin mcodex
```

Then from repo root:

```bash
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" \
HTTPS_PROXY="${HTTPS_PROXY:-http://127.0.0.1:7897}" \
LK_CUSTOM_WEBRTC="${LK_CUSTOM_WEBRTC:-/Users/viv/.cache/mcodex-webrtc/mac-arm64-release}" \
time just smoke-mcodex-gate
```

Expected:

```text
smoke-mcodex-local: pass
smoke-mcodex-cli: pass
smoke-mcodex-runtime-gate: pass
smoke-mcodex-quota-gate: pass
smoke-mcodex-gate: pass
```

- [ ] **Step 12: Verify compatibility aggregate is unchanged**

Run:

```bash
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-all
```

Expected:

```text
smoke-mcodex-local: pass
smoke-mcodex-cli: pass
```

No runtime/quota gate output should appear.

- [ ] **Step 13: Run Rust formatting and targeted lint fix**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-core
```

Expected: commands complete successfully. Do not re-run tests solely because
`just fmt` or `just fix -p codex-core` ran, unless they changed code in a way
that affects the tests above.

- [ ] **Step 14: Run docs and whitespace checks**

Run from repo root:

```bash
git diff --check
```

Expected: no whitespace errors.

- [ ] **Step 15: Record measured cost in the runbook if local gate ran**

If Steps 4 and 9-11 ran in a real local shell or network-enabled CI, update the
runbook with observed cold/warm timings and target-size delta. If they were
blocked by environment, record that verification was blocked and why in the
final message, not as a false pass.

- [ ] **Step 16: Commit final verification/runbook cost updates**

If Step 15 modified docs:

```bash
git add docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md
git commit -m "docs: record mcodex smoke gate cost"
```

If there are no doc updates, do not create an empty commit.

---

## Task 9: Final Review Checklist

**Files:**

- Verify: all files touched by previous tasks

- [ ] **Step 1: Inspect final diff**

Run:

```bash
git status --short --untracked-files=all
git diff --stat HEAD
git diff --check
```

Expected: only intentional files are modified or all changes are committed.
`git diff --check` reports no whitespace errors.

- [ ] **Step 2: Confirm acceptance criteria coverage**

Run:

```bash
rg -n "smoke-mcodex-runtime-gate|smoke-mcodex-quota-gate|smoke-mcodex-gate|smoke-mcodex-runtime|smoke-mcodex-quota|smoke-mcodex-all" justfile docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md
rg -n "normal_turns_remain_on_same_account_without_quota_pressure|second_runtime_skips_account_leased_by_first_runtime" codex-rs/core/tests/suite/account_pool.rs scripts/smoke
rg -n "CODEX_SANDBOX_NETWORK_DISABLED|Skipping test because it cannot execute when network is disabled in a Codex sandbox|ignored|named regression not found" scripts/smoke/run-named-cargo-tests.sh scripts/smoke/test-run-named-cargo-tests.sh
```

Expected:

- Runtime/quota/gate recipes exist.
- Runtime/quota aliases are documented or implemented.
- `smoke-mcodex-all` remains documented as local+CLI only.
- Both new exact tests exist in Rust and in runtime descriptors.
- Runner has explicit checks for sandbox env, skip sentinel, ignored tests, and
  missing/duplicate exact tests.

- [ ] **Step 3: Request implementation code review**

Use `@superpowers:requesting-code-review` after implementation is complete.
Ask the reviewer to focus on:

- shell runner exactness and failure modes
- whether new Rust regressions really prove sticky and multi-runtime lease
  behavior
- whether `smoke-mcodex-all` compatibility was preserved
- whether docs overpromise app-server/TUI/remote gates

- [ ] **Step 4: Fix review findings and re-run affected checks**

For each real issue, apply the smallest fix and re-run the relevant command
from Task 8.

- [ ] **Step 5: Final commit if review fixes changed files**

```bash
git add <changed files>
git commit -m "fix: address mcodex smoke gate review"
```

---

## Expected End State

- `just smoke-mcodex-runtime-gate` runs the exact runtime regression set and
  fails closed if a critical test is skipped, ignored, missing, duplicated, or
  run from a network-disabled Codex sandbox.
- `just smoke-mcodex-quota-gate` runs the exact quota regression set with the
  same fail-closed behavior.
- `just smoke-mcodex-gate` composes local + CLI + runtime + quota.
- `just smoke-mcodex-all` remains local + CLI only.
- The sticky-account and cross-runtime lease gaps have explicit
  `codex-core --test all` coverage.
- The P0 smoke runbook explains what is automated now, what remains deferred,
  how to run from a network-enabled shell, and how to capture runtime/quota
  gate cost.
