# Mcodex Smoke Test Matrix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first practical mcodex smoke layer from
`docs/superpowers/specs/2026-04-27-mcodex-smoke-test-matrix-design.md`: a P0
manual runbook plus the smallest useful repeatable local/CLI smoke commands.

**Architecture:** Keep the smoke harness outside product runtime paths. Use a
small dev-only Rust fixture binary to seed isolated `MCODEX_HOME` directories
through `codex-state` APIs, then run the real `mcodex` product binary through
shell smoke scripts. Keep the entrypoints narrow (`MCODEX_BIN`, `MCODEX_HOME`,
CLI commands, and `just` recipes) so future upstream merges mostly touch
additive files rather than core product code.

**Tech Stack:** Rust, `codex-state`, `clap`, Tokio, shell scripts, Python
standard-library JSON assertions, `just`, `cargo test`, @superpowers:test-driven-development.

---

## Scope

In scope for this first slice:

- P0 manual runbook for M-01 through M-14, M-17, and M-23 where the row is
  already locally executable.
- A canonical local fixture helper that can seed isolated homes for single pool,
  multi-pool, default, invalid-default, and observability CLI smoke.
- `just smoke-mcodex-local` for binary identity, home isolation, empty home,
  `MCODEX_HOME` vs `CODEX_HOME`, single-pool fallback, multi-pool blocker,
  persisted default set/clear, and config default precedence.
- `just smoke-mcodex-cli` for `accounts status`, `accounts pool show`,
  `accounts diagnostics`, and `accounts events` over seeded local state.
- `just smoke-mcodex-all` as the aggregate for the first two commands.
- Operator-oriented output that prints the binary path, version, git SHA,
  fixture home, fixture scenario, and exact assertion marker.

Out of scope for this first slice:

- Headless TUI automation.
- Real-account quota exhaustion.
- Runtime turn, subagent lease, and quota-aware selection automation.
- App-server smoke automation.
- Installer release packaging smoke beyond the P0 runbook row.
- Fake remote backend smoke.
- Any production CLI surface for seeding smoke fixtures.

## Merge-Risk Boundaries

- Do not add smoke-only commands to `mcodex` itself.
- Do not add smoke-only code to `codex-core`.
- Do not change product startup, account-pool, runtime-lease, quota, TUI, or
  app-server behavior in this slice.
- Prefer new additive files:
  - `docs/superpowers/runbooks/...`
  - `codex-rs/smoke-fixtures/...`
  - `scripts/smoke/...`
- Touch existing files only for workspace/recipe registration:
  - `codex-rs/Cargo.toml`
  - `justfile`
- If adding the fixture crate changes `Cargo.lock`, update Bazel lock state in
  the same implementation task.

## Planned File Layout

- Add: `docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md`
  - Manual checklist and capture template for P0 rows.
- Add: `codex-rs/smoke-fixtures/Cargo.toml`
  - Dev-only fixture crate.
- Add: `codex-rs/smoke-fixtures/BUILD.bazel`
  - Bazel registration for the fixture crate.
- Add: `codex-rs/smoke-fixtures/src/lib.rs`
  - Fixture scenarios and state seeding helpers.
- Add: `codex-rs/smoke-fixtures/src/main.rs`
  - CLI wrapper around the fixture helpers.
- Modify: `codex-rs/Cargo.toml`
  - Add `smoke-fixtures` workspace member.
- Add: `scripts/smoke/assert-json-path.py`
  - Minimal JSON assertion helper used by shell scripts.
- Add: `scripts/smoke/mcodex-local.sh`
  - P0-A/P0-B local smoke automation.
- Add: `scripts/smoke/mcodex-cli.sh`
  - CLI observability smoke automation.
- Modify: `justfile`
  - Add `smoke-mcodex-local`, `smoke-mcodex-cli`, and
    `smoke-mcodex-all`.

## Execution Rules

- Use `@superpowers:test-driven-development` for Rust fixture work.
- Use `@superpowers:verification-before-completion` before claiming the smoke
  commands are working.
- Always run smoke scripts with an explicit `MCODEX_HOME`; never mutate a real
  `~/.mcodex` or `~/.codex`.
- Smoke scripts must clear inherited `CODEX_HOME` except for the deliberate
  conflict row.
- Smoke scripts must clear inherited `CODEX_SQLITE_HOME` on every product
  invocation. `CODEX_SQLITE_HOME` overrides the SQLite state/log location and
  can otherwise bypass the fixture home.
- Smoke scripts must require or derive an explicit `MCODEX_BIN`; they must not
  silently test an unrelated `mcodex` on `PATH`.
- `just` recipes should invoke smoke scripts with `sh`, and smoke scripts
  should invoke the JSON helper with `python3`, so the plan does not depend on
  executable bits being preserved for newly added script files.
- Keep Python helper usage to the standard library; do not add a `jq`
  dependency.
- Use fake credentials and fake account ids only.
- Run `just fmt` from `codex-rs/` after Rust changes.
- Run targeted fixture tests before running smoke scripts.

---

## Task 0: Preflight And Baseline

**Files:**

- Read: `docs/superpowers/specs/2026-04-27-mcodex-smoke-test-matrix-design.md`
- Read: `justfile`
- Read: `codex-rs/Cargo.toml`
- Read: `codex-rs/cli/tests/accounts.rs`
- Read: `codex-rs/cli/tests/accounts_observability.rs`

- [ ] **Step 1: Confirm branch and working tree**

Run:

```bash
git status --short
git branch --show-current
```

Expected: current branch is the intended implementation branch and unrelated
files are either absent or intentionally left unstaged.

- [ ] **Step 2: Confirm current smoke gaps**

Run:

```bash
rg -n "smoke-mcodex|mcodex-smoke|smoke-fixtures" justfile scripts codex-rs docs/superpowers
rg -n "seed_account\\(|upsert_registered_account|accounts status --json" codex-rs/cli/tests
```

Expected: no existing `just smoke-mcodex-*` recipes. Existing CLI tests have
local state seeding patterns that the fixture helper can copy or replace with
public `codex-state` APIs.

- [ ] **Step 3: Build the binary under test if needed**

Run:

```bash
cd codex-rs
cargo build -p codex-cli --bin mcodex
```

Expected: `codex-rs/target/debug/mcodex` exists and can be used as
`MCODEX_BIN` for local smoke.

---

## Task 1: Add The P0 Manual Runbook

**Files:**

- Add: `docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md`

- [ ] **Step 1: Create the runbook skeleton**

Add a runbook with these sections:

```markdown
# Mcodex P0 Smoke Runbook

## Required Inputs

- `MCODEX_BIN`: absolute path to the binary under test.
- `MCODEX_HOME`: isolated temporary or named smoke home.
- Git SHA: output of `git rev-parse HEAD`.
- Version: output of `"$MCODEX_BIN" --version`.

## Safety Rules

- Do not run P0 smoke against a real `~/.mcodex` unless the row explicitly says
  it is a real-account launch check.
- Do not rely on `CODEX_HOME`.
- Clear `CODEX_HOME` except for the home-conflict row.
- Clear `CODEX_SQLITE_HOME` for every command in this runbook.
- Do not intentionally exhaust real account quota.

## Capture Template

| Field | Value |
| --- | --- |
| Smoke row | |
| Binary | |
| Version | |
| Git SHA | |
| `MCODEX_HOME` | |
| Fixture class | |
| Credentials | absent / fake / real |
| Expected marker | |
| Actual marker | |
| Result | pass / fail |
| Notes | |
```

- [ ] **Step 2: Add P0-A rows**

Document exact commands for:

```bash
export MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex"
export SMOKE_ROOT="$(mktemp -d)"
git rev-parse HEAD
"$MCODEX_BIN" --version
"$MCODEX_BIN" --help
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/empty" \
  "$MCODEX_BIN" accounts status --json
env -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/mcodex" \
  CODEX_HOME="$SMOKE_ROOT/codex" \
  "$MCODEX_BIN" accounts status --json
```

The runbook must record the expected markers:

- version/help come from the intended `mcodex` binary
- empty home does not report pooled access
- `MCODEX_HOME` wins when `CODEX_HOME` is also set
- empty-home TUI launch reaches normal unauthenticated/no-account startup, not
  pooled access

- [ ] **Step 3: Add P0-B rows with fixture placeholders**

Document commands using the future fixture helper:

```bash
cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/single" --scenario single-pool
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/single" \
  "$MCODEX_BIN" accounts status --json

cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/multi" --scenario multi-pool
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/multi" \
  "$MCODEX_BIN" accounts status --json

cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/default" --scenario multi-pool
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/default" \
  "$MCODEX_BIN" accounts pool default set team-main
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/default" \
  "$MCODEX_BIN" accounts status --json
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/default" \
  "$MCODEX_BIN" accounts pool default clear
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/default" \
  "$MCODEX_BIN" accounts status --json

cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/config-conflict" --scenario config-default-conflict
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/config-conflict" \
  "$MCODEX_BIN" accounts status --json

cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/invalid-persisted" --scenario invalid-persisted-default
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/invalid-persisted" \
  "$MCODEX_BIN" accounts status --json

cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/invalid-config" --scenario invalid-config-default
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/invalid-config" \
  "$MCODEX_BIN" accounts status --json

cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/observability" --scenario observability
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/observability" \
  "$MCODEX_BIN" accounts status --json
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/observability" \
  "$MCODEX_BIN" accounts pool show --pool team-main --json
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/observability" \
  "$MCODEX_BIN" accounts diagnostics --pool team-main --json
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/observability" \
  "$MCODEX_BIN" accounts events --pool team-main --json
```

The runbook must name the exact JSON markers from the spec:

- `startup.effectivePoolResolutionSource == "singleVisiblePool"`
- `startup.startupAvailability == "multiplePoolsRequireDefault"`
- `startup.startupAvailability == "invalidExplicitDefault"`
- `startup.startupResolutionIssue.kind == "configDefaultPoolUnavailable"`
- `startup.startupResolutionIssue.kind == "persistedDefaultPoolUnavailable"`
- `startup.effectivePoolResolutionSource == "persistedSelection"` after
  `accounts pool default set`
- `startup.effectivePoolResolutionSource == "configDefault"` when a config
  default conflicts with a different persisted default
- `poolObservability.summary.totalAccounts == 2`
- `summary.activeLeases == 1` for `accounts pool show --pool team-main --json`

- [ ] **Step 4: Add manual TUI and installer notes**

Add exact manual rows for:

- empty home startup
- single pool startup
- multi-pool default-required startup
- pooled access paused/default-required surface
- local installer wrapper identity

Use commands like:

```bash
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/empty-tui" \
  "$MCODEX_BIN"

env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/single" \
  "$MCODEX_BIN"

env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/multi" \
  "$MCODEX_BIN"

MCODEX_ROOT="$SMOKE_ROOT/install-root" \
  MCODEX_WRAPPER_DIR="$SMOKE_ROOT/wrappers" \
  ./scripts/dev/install-local.sh
PATH="$SMOKE_ROOT/wrappers:$PATH" command -v mcodex
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  PATH="$SMOKE_ROOT/wrappers:$PATH" \
  MCODEX_HOME="$SMOKE_ROOT/wrapper-home" \
  mcodex --version
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  PATH="$SMOKE_ROOT/wrappers:$PATH" \
  MCODEX_HOME="$SMOKE_ROOT/wrapper-home" \
  mcodex accounts status --json
```

The installer row must capture `MCODEX_ROOT`, `MCODEX_WRAPPER_DIR`,
`command -v mcodex`, wrapper path, installed binary path, version output, and
the `MCODEX_HOME` used for the wrapper command.

Do not make these automated yet. Require a screenshot or terminal capture for
failures.

- [ ] **Step 5: Verify docs formatting**

Run:

```bash
git diff --check docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md
```

Expected: no whitespace errors.

---

## Task 2: Add The Canonical Local Fixture Helper

**Files:**

- Add: `codex-rs/smoke-fixtures/Cargo.toml`
- Add: `codex-rs/smoke-fixtures/BUILD.bazel`
- Add: `codex-rs/smoke-fixtures/src/lib.rs`
- Add: `codex-rs/smoke-fixtures/src/main.rs`
- Modify: `codex-rs/Cargo.toml`
- Possible modify: `Cargo.lock`
- Possible modify: `MODULE.bazel.lock`

- [ ] **Step 1: Create the crate skeleton and write failing fixture tests**

Create the minimal crate files from Step 2 first, with `seed_fixture(...)`
implemented as `todo!()` or returning an intentionally incomplete result. Then
write tests in `codex-rs/smoke-fixtures/src/lib.rs`. The tests should call
library functions directly and then verify the resulting startup status with
`StateRuntime`.

Start with these tests:

```rust
use codex_state::AccountStartupAvailability;
use codex_state::AccountStartupResolutionIssueKind;
use codex_state::EffectivePoolResolutionSource;

#[tokio::test]
async fn seed_single_pool_fixture_reports_single_visible_pool() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;

    seed_fixture(home.path(), SmokeScenario::SinglePool).await?;

    let runtime = StateRuntime::init(home.path().to_path_buf(), "smoke-test".to_string()).await?;
    let status = runtime.read_account_startup_status(None).await?;

    assert_eq!(status.preview.effective_pool_id.as_deref(), Some("team-main"));
    assert_eq!(
        status.effective_pool_resolution_source,
        EffectivePoolResolutionSource::SingleVisiblePool
    );
    assert_eq!(
        status.startup_availability,
        AccountStartupAvailability::Available
    );
    Ok(())
}

#[tokio::test]
async fn seed_multi_pool_fixture_requires_default() -> anyhow::Result<()> {
    let home = tempfile::tempdir()?;

    seed_fixture(home.path(), SmokeScenario::MultiPool).await?;

    let runtime = StateRuntime::init(home.path().to_path_buf(), "smoke-test".to_string()).await?;
    let status = runtime.read_account_startup_status(None).await?;

    assert_eq!(status.preview.effective_pool_id, None);
    assert_eq!(
        status.startup_availability,
        AccountStartupAvailability::MultiplePoolsRequireDefault
    );
    assert_eq!(
        status.startup_resolution_issue.as_ref().map(|issue| issue.kind),
        Some(AccountStartupResolutionIssueKind::MultiplePoolsRequireDefault)
    );
    Ok(())
}
```

Also add tests for:

- persisted default chooses `team-main`
- config default outranks a different persisted default by calling
  `read_account_startup_status(Some("team-main"))`; `StateRuntime` does not
  parse `config.toml` directly
- invalid persisted default reports `persistedDefaultPoolUnavailable`
- invalid config default reports `configDefaultPoolUnavailable` by calling
  `read_account_startup_status(Some("missing-pool"))`
- observability fixture produces one active lease, quota facts, and events

Run:

```bash
cd codex-rs
cargo test -p codex-smoke-fixtures
```

Expected red state: the `codex-smoke-fixtures` package is found and the tests
fail because fixture behavior is not implemented. A package-not-found failure
does not count as the TDD red state.

- [ ] **Step 2: Register the fixture crate**

Add the crate to `codex-rs/Cargo.toml`:

```toml
[workspace]
members = [
    # existing members...
    "smoke-fixtures",
]
```

Create `codex-rs/smoke-fixtures/Cargo.toml`:

```toml
[package]
name = "codex-smoke-fixtures"
version.workspace = true
edition.workspace = true
license.workspace = true

[[bin]]
name = "mcodex-smoke-fixture"
path = "src/main.rs"

[lib]
path = "src/lib.rs"

[lints]
workspace = true

[dependencies]
anyhow = { workspace = true }
chrono = { workspace = true }
clap = { workspace = true, features = ["derive"] }
codex-state = { workspace = true }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
tempfile = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[dev-dependencies]
pretty_assertions = { workspace = true }
```

Create `codex-rs/smoke-fixtures/BUILD.bazel`:

```python
load("//:defs.bzl", "codex_rust_crate")

codex_rust_crate(
    name = "smoke-fixtures",
    crate_name = "codex_smoke_fixtures",
)
```

- [ ] **Step 3: Implement scenario enum and summary output**

In `src/lib.rs`, add:

```rust
use std::path::Path;

use anyhow::Result;
use chrono::Duration;
use chrono::Utc;
use clap::ValueEnum;
use codex_state::AccountPoolEventRecord;
use codex_state::AccountQuotaStateRecord;
use codex_state::AccountStartupSelectionUpdate;
use codex_state::QuotaExhaustedWindows;
use codex_state::QuotaProbeResult;
use codex_state::RegisteredAccountMembership;
use codex_state::RegisteredAccountUpsert;
use codex_state::StateRuntime;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SmokeScenario {
    Empty,
    SinglePool,
    MultiPool,
    PersistedDefault,
    ConfigDefaultConflict,
    InvalidPersistedDefault,
    InvalidConfigDefault,
    Observability,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmokeFixtureSummary {
    pub home: String,
    pub scenario: String,
    pub pools: Vec<String>,
    pub accounts: Vec<String>,
    pub credentials: &'static str,
}
```

Implement:

```rust
pub async fn seed_fixture(home: &Path, scenario: SmokeScenario) -> Result<SmokeFixtureSummary> {
    std::fs::create_dir_all(home)?;
    let runtime = StateRuntime::init(home.to_path_buf(), "mcodex-smoke-fixture".to_string()).await?;

    match scenario {
        SmokeScenario::Empty => {}
        SmokeScenario::SinglePool => {
            seed_account(&runtime, "acct-main-1", "team-main", 0).await?;
        }
        SmokeScenario::MultiPool => {
            seed_account(&runtime, "acct-main-1", "team-main", 0).await?;
            seed_account(&runtime, "acct-other-1", "team-other", 0).await?;
        }
        SmokeScenario::PersistedDefault => {
            seed_account(&runtime, "acct-main-1", "team-main", 0).await?;
            seed_account(&runtime, "acct-other-1", "team-other", 0).await?;
            runtime
                .write_account_startup_selection(AccountStartupSelectionUpdate {
                    default_pool_id: Some("team-main".to_string()),
                    preferred_account_id: None,
                    suppressed: false,
                })
                .await?;
        }
        SmokeScenario::ConfigDefaultConflict => {
            seed_account(&runtime, "acct-main-1", "team-main", 0).await?;
            seed_account(&runtime, "acct-other-1", "team-other", 0).await?;
            runtime
                .write_account_startup_selection(AccountStartupSelectionUpdate {
                    default_pool_id: Some("team-other".to_string()),
                    preferred_account_id: None,
                    suppressed: false,
                })
                .await?;
            write_config_default(home, "team-main")?;
        }
        SmokeScenario::InvalidPersistedDefault => {
            seed_account(&runtime, "acct-main-1", "team-main", 0).await?;
            runtime
                .write_account_startup_selection(AccountStartupSelectionUpdate {
                    default_pool_id: Some("missing-pool".to_string()),
                    preferred_account_id: None,
                    suppressed: false,
                })
                .await?;
        }
        SmokeScenario::InvalidConfigDefault => {
            seed_account(&runtime, "acct-main-1", "team-main", 0).await?;
            write_config_default(home, "missing-pool")?;
        }
        SmokeScenario::Observability => {
            seed_account(&runtime, "acct-main-1", "team-main", 0).await?;
            seed_account(&runtime, "acct-main-2", "team-main", 1).await?;
            let lease = runtime
                .acquire_account_lease("team-main", "smoke-holder", Duration::seconds(300))
                .await?;
            seed_quota(&runtime, "acct-main-2").await?;
            runtime
                .append_account_pool_event(AccountPoolEventRecord {
                    event_id: "smoke-quota-observed".to_string(),
                    occurred_at: Utc::now(),
                    pool_id: "team-main".to_string(),
                    account_id: Some("acct-main-2".to_string()),
                    lease_id: None,
                    holder_instance_id: Some("smoke-holder".to_string()),
                    event_type: "quotaObserved".to_string(),
                    reason_code: Some("quotaNearExhausted".to_string()),
                    message: "smoke quota observation".to_string(),
                    details_json: Some(serde_json::json!({"fixture": "observability"})),
                })
                .await?;
            drop(lease);
        }
    }

    Ok(summary_for(home, scenario))
}
```

The helper should use `StateRuntime` APIs instead of raw SQL for account
registration, startup selection, quota state, lease, and events.

- [ ] **Step 4: Implement account/config/quota helpers**

Use `RegisteredAccountUpsert` so the helper remains compatible with future
local/remote-shaped account fields:

```rust
fn summary_for(home: &Path, scenario: SmokeScenario) -> SmokeFixtureSummary {
    let (pools, accounts) = match scenario {
        SmokeScenario::Empty => (vec![], vec![]),
        SmokeScenario::SinglePool
        | SmokeScenario::InvalidPersistedDefault
        | SmokeScenario::InvalidConfigDefault => (
            vec!["team-main".to_string()],
            vec!["acct-main-1".to_string()],
        ),
        SmokeScenario::MultiPool
        | SmokeScenario::PersistedDefault
        | SmokeScenario::ConfigDefaultConflict => (
            vec!["team-main".to_string(), "team-other".to_string()],
            vec!["acct-main-1".to_string(), "acct-other-1".to_string()],
        ),
        SmokeScenario::Observability => (
            vec!["team-main".to_string()],
            vec!["acct-main-1".to_string(), "acct-main-2".to_string()],
        ),
    };

    SmokeFixtureSummary {
        home: home.display().to_string(),
        scenario: format!("{scenario:?}"),
        pools,
        accounts,
        credentials: "fake",
    }
}

async fn seed_account(
    runtime: &StateRuntime,
    account_id: &str,
    pool_id: &str,
    position: i64,
) -> Result<()> {
    runtime
        .upsert_registered_account(RegisteredAccountUpsert {
            account_id: account_id.to_string(),
            backend_id: "smoke-local".to_string(),
            backend_family: "chatgpt".to_string(),
            workspace_id: Some("workspace-smoke".to_string()),
            backend_account_handle: account_id.to_string(),
            account_kind: "chatgpt".to_string(),
            provider_fingerprint: format!("smoke:{account_id}"),
            display_name: Some(format!("Smoke {account_id}")),
            source: None,
            enabled: true,
            healthy: true,
            membership: Some(RegisteredAccountMembership {
                pool_id: pool_id.to_string(),
                position,
            }),
        })
        .await?;
    Ok(())
}

fn write_config_default(home: &Path, pool_id: &str) -> Result<()> {
    std::fs::write(
        home.join("config.toml"),
        format!(
            r#"[accounts]
default_pool = "{pool_id}"

[accounts.pools.team-main]
allow_context_reuse = false

[accounts.pools.team-other]
allow_context_reuse = false
"#,
        ),
    )?;
    Ok(())
}

async fn seed_quota(runtime: &StateRuntime, account_id: &str) -> Result<()> {
    let now = Utc::now();
    runtime
        .upsert_account_quota_state(AccountQuotaStateRecord {
            account_id: account_id.to_string(),
            limit_id: "chatgpt".to_string(),
            primary_used_percent: Some(42.0),
            primary_resets_at: Some(now + Duration::minutes(30)),
            secondary_used_percent: Some(100.0),
            secondary_resets_at: Some(now + Duration::minutes(60)),
            observed_at: now,
            exhausted_windows: QuotaExhaustedWindows::Secondary,
            predicted_blocked_until: Some(now + Duration::minutes(60)),
            next_probe_after: Some(now + Duration::minutes(10)),
            probe_backoff_level: 1,
            last_probe_result: Some(QuotaProbeResult::StillBlocked),
        })
        .await?;
    Ok(())
}
```

If any field names have drifted, use the current exported `codex-state` types
rather than adding raw SQL to the fixture crate.

- [ ] **Step 5: Implement the fixture CLI**

In `src/main.rs`:

```rust
use std::path::PathBuf;

use anyhow::Result;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use codex_smoke_fixtures::SmokeScenario;
use codex_smoke_fixtures::seed_fixture;

#[derive(Debug, Parser)]
#[command(name = "mcodex-smoke-fixture")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Seed(SeedCommand),
}

#[derive(Debug, Args)]
struct SeedCommand {
    #[arg(long)]
    home: PathBuf,
    #[arg(long, value_enum)]
    scenario: SmokeScenario,
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Seed(command) => {
            let summary = seed_fixture(&command.home, command.scenario).await?;
            if command.json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                println!(
                    "seeded scenario={} home={}",
                    summary.scenario,
                    summary.home
                );
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 6: Run fixture tests and formatting**

Run:

```bash
cd codex-rs
cargo test -p codex-smoke-fixtures
just fmt
```

Expected: fixture crate tests pass. Do not run full workspace tests yet.

- [ ] **Step 7: Update Bazel lock state if needed**

If `Cargo.lock` changed or the new crate affects Bazel module resolution, run
from repo root:

```bash
just bazel-lock-update
just bazel-lock-check
```

Expected: lockfiles are in sync. If Bazel lock commands require network and
fail, record the exact failure and do not claim Bazel lock verification passed.

---

## Task 3: Add JSON Assertion Helper

**Files:**

- Add: `scripts/smoke/assert-json-path.py`

- [ ] **Step 1: Create a dependency-free JSON path assertion script**

Create a small Python script that reads JSON from stdin and validates simple dot
paths used by the smoke scripts:

```python
#!/usr/bin/env python3
import argparse
import json
import sys


def read_path(value, path):
    current = value
    for part in path.split("."):
        if isinstance(current, dict) and part in current:
            current = current[part]
        else:
            raise KeyError(path)
    return current


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--path", required=True)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--equals")
    group.add_argument("--is-null", action="store_true")
    group.add_argument("--is-not-null", action="store_true")
    args = parser.parse_args()

    payload = json.load(sys.stdin)
    actual = read_path(payload, args.path)

    if args.is_null:
        ok = actual is None
        expected = None
    elif args.is_not_null:
        ok = actual is not None
        expected = "not null"
    else:
        ok = str(actual) == args.equals
        expected = args.equals

    if not ok:
        print(
            f"assertion failed: {args.path}: expected {expected!r}, got {actual!r}",
            file=sys.stderr,
        )
        sys.exit(1)


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Verify helper with sample JSON**

Run:

```bash
printf '{"startup":{"effectivePoolResolutionSource":"singleVisiblePool","issue":null}}' \
  | python3 scripts/smoke/assert-json-path.py \
      --path startup.effectivePoolResolutionSource \
      --equals singleVisiblePool

printf '{"startup":{"issue":null}}' \
  | python3 scripts/smoke/assert-json-path.py --path startup.issue --is-null
```

Expected: both commands exit 0. Add a negative local check while developing,
but do not encode a failing command into `just`.

---

## Task 4: Add `smoke-mcodex-local`

**Files:**

- Add: `scripts/smoke/mcodex-local.sh`
- Modify: `justfile`

- [ ] **Step 1: Write the local smoke script**

Create `scripts/smoke/mcodex-local.sh`:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
ASSERT_JSON="$SCRIPT_DIR/assert-json-path.py"
MCODEX_BIN=${MCODEX_BIN:-"$REPO_ROOT/codex-rs/target/debug/mcodex"}
SMOKE_ROOT=${SMOKE_ROOT:-}

if [ ! -x "$MCODEX_BIN" ]; then
  echo "MCODEX_BIN is not executable: $MCODEX_BIN" >&2
  echo "Build one with: cd codex-rs && cargo build -p codex-cli --bin mcodex" >&2
  exit 2
fi

if [ -z "$SMOKE_ROOT" ]; then
  SMOKE_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/mcodex-smoke-local.XXXXXX")
  CLEANUP_SMOKE_ROOT=1
else
  mkdir -p "$SMOKE_ROOT"
  CLEANUP_SMOKE_ROOT=0
fi

cleanup() {
  if [ "$CLEANUP_SMOKE_ROOT" -eq 1 ]; then
    rm -rf "$SMOKE_ROOT"
  fi
}
trap cleanup EXIT INT TERM HUP

fixture() {
  echo "fixture_scenario=$2 fixture_home=$1" >&2
  env -u CODEX_HOME -u CODEX_SQLITE_HOME \
    cargo run --quiet --manifest-path "$REPO_ROOT/codex-rs/Cargo.toml" \
    -p codex-smoke-fixtures -- seed \
    --home "$1" --scenario "$2" --json
}

status_json() {
  env -u CODEX_HOME -u CODEX_SQLITE_HOME \
    MCODEX_HOME="$1" "$MCODEX_BIN" accounts status --json
}

assert_path() {
  echo "assert path=$2 expected=$3"
  printf '%s' "$1" | python3 "$ASSERT_JSON" --path "$2" --equals "$3"
}

assert_null() {
  echo "assert path=$2 expected=null"
  printf '%s' "$1" | python3 "$ASSERT_JSON" --path "$2" --is-null
}

echo "smoke=local"
echo "binary=$MCODEX_BIN"
echo "version=$("$MCODEX_BIN" --version)"
echo "git_sha=$(git -C "$REPO_ROOT" rev-parse HEAD)"
echo "smoke_root=$SMOKE_ROOT"

"$MCODEX_BIN" --help >/dev/null

empty_home="$SMOKE_ROOT/empty"
mkdir -p "$empty_home"
empty_status=$(status_json "$empty_home")
assert_null "$empty_status" startup.effectivePoolId

codex_home="$SMOKE_ROOT/codex-home-with-pool"
fixture "$codex_home" single-pool >/dev/null
mcodex_home="$SMOKE_ROOT/mcodex-empty"
mkdir -p "$mcodex_home"
conflict_status=$(env -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$mcodex_home" CODEX_HOME="$codex_home" \
  "$MCODEX_BIN" accounts status --json)
assert_null "$conflict_status" startup.effectivePoolId

single_home="$SMOKE_ROOT/single"
fixture "$single_home" single-pool >/dev/null
single_status=$(status_json "$single_home")
assert_path "$single_status" startup.effectivePoolResolutionSource singleVisiblePool

multi_home="$SMOKE_ROOT/multi"
fixture "$multi_home" multi-pool >/dev/null
multi_status=$(status_json "$multi_home")
assert_path "$multi_status" startup.startupAvailability multiplePoolsRequireDefault

default_home="$SMOKE_ROOT/default"
fixture "$default_home" multi-pool >/dev/null
env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$default_home" \
  "$MCODEX_BIN" accounts pool default set team-main >/dev/null
default_status=$(status_json "$default_home")
assert_path "$default_status" startup.effectivePoolResolutionSource persistedSelection
env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$default_home" \
  "$MCODEX_BIN" accounts pool default clear >/dev/null
cleared_status=$(status_json "$default_home")
assert_path "$cleared_status" startup.startupAvailability multiplePoolsRequireDefault

config_home="$SMOKE_ROOT/config-conflict"
fixture "$config_home" config-default-conflict >/dev/null
config_status=$(status_json "$config_home")
assert_path "$config_status" startup.effectivePoolResolutionSource configDefault

echo "smoke-mcodex-local: pass"
```

If current JSON field names differ, update the script to match current CLI JSON
without changing the spec intent.

- [ ] **Step 2: Add the just recipe**

In the root `justfile`, add:

```make
[no-cd]
smoke-mcodex-local *args:
    sh ./scripts/smoke/mcodex-local.sh "$@"
```

- [ ] **Step 3: Run the local smoke command**

Run:

```bash
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-local
```

Expected: prints binary/version/git SHA and exits with
`smoke-mcodex-local: pass`.

---

## Task 5: Add `smoke-mcodex-cli`

**Files:**

- Add: `scripts/smoke/mcodex-cli.sh`
- Modify: `justfile`

- [ ] **Step 1: Write the CLI smoke script**

Create `scripts/smoke/mcodex-cli.sh`:

```sh
#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
ASSERT_JSON="$SCRIPT_DIR/assert-json-path.py"
MCODEX_BIN=${MCODEX_BIN:-"$REPO_ROOT/codex-rs/target/debug/mcodex"}
SMOKE_ROOT=${SMOKE_ROOT:-}

if [ ! -x "$MCODEX_BIN" ]; then
  echo "MCODEX_BIN is not executable: $MCODEX_BIN" >&2
  exit 2
fi

if [ -z "$SMOKE_ROOT" ]; then
  SMOKE_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/mcodex-smoke-cli.XXXXXX")
  CLEANUP_SMOKE_ROOT=1
else
  mkdir -p "$SMOKE_ROOT"
  CLEANUP_SMOKE_ROOT=0
fi

cleanup() {
  if [ "$CLEANUP_SMOKE_ROOT" -eq 1 ]; then
    rm -rf "$SMOKE_ROOT"
  fi
}
trap cleanup EXIT INT TERM HUP

fixture() {
  echo "fixture_scenario=$2 fixture_home=$1" >&2
  env -u CODEX_HOME -u CODEX_SQLITE_HOME \
    cargo run --quiet --manifest-path "$REPO_ROOT/codex-rs/Cargo.toml" \
    -p codex-smoke-fixtures -- seed \
    --home "$1" --scenario "$2" --json
}

run_mcodex() {
  env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$1" "$MCODEX_BIN" "$@"
}

assert_path() {
  echo "assert path=$2 expected=$3"
  printf '%s' "$1" | python3 "$ASSERT_JSON" --path "$2" --equals "$3"
}

assert_not_null() {
  echo "assert path=$2 expected=not-null"
  printf '%s' "$1" | python3 "$ASSERT_JSON" --path "$2" --is-not-null
}

echo "smoke=cli"
echo "binary=$MCODEX_BIN"
echo "version=$("$MCODEX_BIN" --version)"
echo "git_sha=$(git -C "$REPO_ROOT" rev-parse HEAD)"
echo "smoke_root=$SMOKE_ROOT"

home="$SMOKE_ROOT/observability"
fixture "$home" observability >/dev/null

status_json=$(run_mcodex "$home" accounts status --json)
assert_path "$status_json" poolObservability.summary.totalAccounts 2

pool_json=$(run_mcodex "$home" accounts pool show --pool team-main --json)
assert_path "$pool_json" summary.totalAccounts 2
assert_path "$pool_json" summary.activeLeases 1

diagnostics_json=$(run_mcodex "$home" accounts diagnostics --pool team-main --json)
assert_not_null "$diagnostics_json" status

events_json=$(run_mcodex "$home" accounts events --pool team-main --json)
assert_not_null "$events_json" data

run_mcodex "$home" accounts pool show --pool team-main >/dev/null
run_mcodex "$home" accounts diagnostics --pool team-main >/dev/null
run_mcodex "$home" accounts events --pool team-main >/dev/null

echo "smoke-mcodex-cli: pass"
```

If current `accounts status --json` nests observability summary under a
different path, update only the assertion path after confirming the existing CLI
contract.

- [ ] **Step 2: Add just recipes**

In the root `justfile`, add:

```make
[no-cd]
smoke-mcodex-cli *args:
    sh ./scripts/smoke/mcodex-cli.sh "$@"

[no-cd]
smoke-mcodex-all *args:
    sh ./scripts/smoke/mcodex-local.sh "$@"
    sh ./scripts/smoke/mcodex-cli.sh "$@"
```

- [ ] **Step 3: Run the CLI smoke command**

Run:

```bash
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-cli
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-all
```

Expected: both commands pass without touching a real mcodex home.

---

## Task 6: Update Documentation And Future Rows

**Files:**

- Modify: `docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md`
- Modify: `docs/superpowers/specs/2026-04-27-mcodex-smoke-test-matrix-design.md` only if implementation uncovers a spec bug
- Modify: this plan document checkboxes as tasks complete

- [ ] **Step 1: Link the automated commands from the runbook**

Add:

````markdown
## Automated P0 Subset

Run:

```bash
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-local
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-cli
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-all
```

These commands cover the automated local/CLI subset only. Manual TUI,
installer-wrapper, app-server, runtime, quota, subagent, and remote rows remain
separate until their harnesses are added.
````

- [ ] **Step 2: Add a future-work section**

List the next smoke phases without implementing them:

- `just smoke-mcodex-app-server`
- `just smoke-mcodex-runtime`
- `just smoke-mcodex-quota`
- `just smoke-mcodex-installer`
- fake remote backend contract smoke
- minimal headless TUI startup smoke

Each future row should point back to the spec matrix IDs.

- [ ] **Step 3: Verify docs only diffs**

Run:

```bash
git diff --check docs/superpowers docs/superpowers/runbooks
```

Expected: no whitespace errors.

---

## Task 7: Final Verification

**Files:**

- All changed files in this plan

- [ ] **Step 1: Run targeted tests**

Run:

```bash
cd codex-rs
cargo test -p codex-smoke-fixtures
```

Expected: PASS.

- [ ] **Step 2: Run focused smoke commands**

Run from repo root:

```bash
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-local
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-cli
MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex" just smoke-mcodex-all
```

Expected: all PASS.

- [ ] **Step 3: Run formatting and lock checks**

Run:

```bash
cd codex-rs
just fix -p codex-smoke-fixtures
just fmt
```

Do not rerun tests after `just fix` or `just fmt`; this follows the repo's Rust
workflow. If `just fix` changes code, inspect the diff before finalizing.

If `Cargo.lock` or Bazel module locks changed, also run from repo root:

```bash
just bazel-lock-check
```

Expected: no formatting or lock drift remains.

- [ ] **Step 4: Check final diff**

Run:

```bash
git status --short
git diff --stat
git diff --check
```

Expected: only the planned files changed and no whitespace errors.

---

## Follow-Up Plans

Do not silently expand this first slice. After it lands, write separate plans
for:

1. App-server smoke:
   - `just smoke-mcodex-app-server`
   - `accountLease/read`
   - account-pool read/list/diagnostics/events/default mutation RPCs
2. Runtime/subagent smoke:
   - pooled runtime turn with mock responses
   - parent/subagent runtime lease authority inheritance
3. Quota smoke:
   - exhausted account skip
   - near-exhausted proactive switch damping
   - reprobe/backoff behavior
4. Installer smoke:
   - isolated install root
   - wrapper replacement
   - PATH and exit-code forwarding
5. Remote contract smoke:
   - fake backend inventory
   - startup snapshot
   - pause/drain/quota facts
   - absence of remote-only facts represented explicitly
