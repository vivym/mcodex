# Mcodex P0 Smoke Runbook

This runbook covers the P0 manual smoke rows from the mcodex smoke-test matrix.
P0-A rows are runnable with an isolated home and a known binary. P0-B rows
require the future `codex-smoke-fixtures` helper or an equivalent documented
manual fixture setup before they count as executable.

## Required Inputs

- Repository root as the current working directory. Commands in this runbook
  use repo-relative paths such as `codex-rs/Cargo.toml` and
  `./scripts/dev/install-local.sh`.
- `MCODEX_BIN`: absolute path to the binary under test.
- `MCODEX_HOME`: isolated temporary or named smoke home.
- Git SHA: output of `git rev-parse HEAD`.
- Version: output of the isolated `"$MCODEX_BIN" --version` command below.
- `SMOKE_ROOT`: temporary directory used only for this smoke run.
- Fixture class: empty isolated home, local state fixture, installer fixture,
  or real account home when a row explicitly allows it.

Recommended setup:

```bash
export MCODEX_BIN="$PWD/codex-rs/target/debug/mcodex"
export SMOKE_ROOT="$(mktemp -d)"
if [ ! -x "$MCODEX_BIN" ]; then
  printf '%s\n' \
    'MCODEX_BIN is not executable; run cargo build --manifest-path codex-rs/Cargo.toml --bin mcodex first.' >&2
  exit 1
fi
git rev-parse HEAD
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/identity" \
  "$MCODEX_BIN" --version
```

## Safety Rules

- Do not run P0 smoke against a real `~/.mcodex` unless the row explicitly says
  it is a real-account launch check.
- Do not rely on `CODEX_HOME`.
- Clear `CODEX_HOME` from every product command except the home-conflict row.
- Clear `CODEX_SQLITE_HOME` for every product command in this runbook.
- Always pass an explicit `MCODEX_HOME` to product commands.
- Use `"$MCODEX_BIN"` for product checks unless the row is specifically testing
  an installed wrapper.
- Bare `mcodex` is allowed only for wrapper rows that also record `PATH`,
  `command -v mcodex`, and wrapper metadata.
- Use fake credentials and fake account ids for local fixture rows.
- Do not intentionally exhaust real account quota.
- Capture a screenshot or terminal transcript for any TUI failure.

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

For wrapper rows, also capture:

| Field | Value |
| --- | --- |
| `MCODEX_ROOT` | |
| `MCODEX_WRAPPER_DIR` | |
| `PATH` used for wrapper command | |
| `command -v mcodex` | |
| Wrapper path | |
| Installed binary path | |
| Wrapper metadata | |
| Wrapper version output | |
| Wrapper `MCODEX_HOME` | |
| Wrapper command `MCODEX_BIN` override | |

## Exact JSON Markers

Use these exact markers when deciding pass/fail for status, pool, diagnostics,
and events output:

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
- `summary.activeLeases == 1` for
  `accounts pool show --pool team-main --json`

## P0-A: Direct Binary Identity

Rows: M-03.

Commands:

```bash
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/identity" \
  "$MCODEX_BIN" --version
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/identity" \
  "$MCODEX_BIN" --help
```

Expected markers:

- Version output comes from the intended `mcodex` binary.
- Help text identifies the intended `mcodex` product binary.
- Record `MCODEX_BIN`, version output, and Git SHA in the capture template.

## P0-A: Isolated Empty Home

Rows: M-01.

Commands:

```bash
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/empty" \
  "$MCODEX_BIN" accounts status --json
```

Expected markers:

- `effectivePoolId == null`
- `startup.effectivePoolId == null`
- `poolObservability == null`
- Empty home does not report pooled access.
- The command uses `$SMOKE_ROOT/empty` and does not read upstream `~/.codex`.
- `CODEX_HOME` and `CODEX_SQLITE_HOME` are both cleared for this product
  command.

## P0-A: `MCODEX_HOME` / `CODEX_HOME` Conflict

Rows: M-02.

Commands:

```bash
mkdir -p "$SMOKE_ROOT/codex"
cat >"$SMOKE_ROOT/codex/config.toml" <<'EOF'
[accounts]
default_pool = "team-main"

[accounts.pools.team-main]
allow_context_reuse = false
EOF
env -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/mcodex" \
  CODEX_HOME="$SMOKE_ROOT/codex" \
  "$MCODEX_BIN" accounts status --json
```

Expected markers:

- `MCODEX_HOME` wins when `CODEX_HOME` is also set.
- `effectivePoolId == null`
- `configuredDefaultPoolId == null`
- `startup.effectivePoolId == null`
- `poolObservability == null`
- Runtime state comes from `$SMOKE_ROOT/mcodex` and does not read
  `$SMOKE_ROOT/codex`.
- `$SMOKE_ROOT/codex/config.toml` contains a `team-main` default sentinel. If
  the product reads `CODEX_HOME`, `configuredDefaultPoolId` exposes the
  sentinel instead of remaining `null`.
- This is the only product command in this runbook that intentionally preserves
  `CODEX_HOME`.
- `CODEX_SQLITE_HOME` is still cleared.

## P0-A: Empty TUI Launch

Rows: M-05.

Commands:

```bash
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/empty-tui" \
  "$MCODEX_BIN"
```

Expected markers:

- Empty-home TUI launch reaches normal unauthenticated or no-account startup.
- It does not report pooled access.
- Capture whether the ChatGPT login or no-account surface appears.
- Capture a screenshot or terminal transcript for any failure.

## P0-B: Single Pool Fixture

Rows: M-06, M-07.

Fixture commands:

```bash
cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/single" --scenario single-pool
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/single" \
  "$MCODEX_BIN" accounts status --json
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/single" \
  "$MCODEX_BIN"
```

Expected markers:

- `startup.effectivePoolResolutionSource == "singleVisiblePool"`
- TUI startup uses pooled access without
  `-c accounts.default_pool="..."`.
- TUI does not show the ChatGPT login surface.

## P0-B: Multi Pool Fixture

Rows: M-08, M-09.

Fixture commands:

```bash
cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/multi" --scenario multi-pool
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/multi" \
  "$MCODEX_BIN" accounts status --json
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/multi" \
  "$MCODEX_BIN"
```

Expected markers:

- `startup.startupAvailability == "multiplePoolsRequireDefault"`
- TUI startup shows the pooled access paused/default-required surface.
- Capture whether the pooled access paused/default-required title appears.

## P0-B: Default Set And Clear

Rows: M-10, M-11.

Fixture commands:

```bash
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
```

Expected markers:

- After `accounts pool default set team-main`:
  `startup.effectivePoolResolutionSource == "persistedSelection"`.
- After `accounts pool default clear`:
  `startup.startupAvailability == "multiplePoolsRequireDefault"`.
- After `accounts pool default clear`: `effectivePoolId == null`.
- After `accounts pool default clear`: `startup.effectivePoolId == null`.
- The persisted default is written, then cleared, only under
  `$SMOKE_ROOT/default`.

## P0-B: Config Default Precedence

Rows: M-12.

Fixture commands:

```bash
cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/config-conflict" --scenario config-default-conflict
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/config-conflict" \
  "$MCODEX_BIN" accounts status --json
```

Expected markers:

- The `config-default-conflict` fixture writes a persisted default of
  `team-other` and a config default of `team-main`.
- `startup.effectivePoolResolutionSource == "configDefault"`
- `startup.effectivePoolId == "team-main"`
- `effectivePoolId == "team-main"`
- `persistedDefaultPoolId == "team-other"`
- `configuredDefaultPoolId == "team-main"`

## P0-B: Invalid Persisted Default

Rows: M-14.

Fixture commands:

```bash
cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/invalid-persisted" --scenario invalid-persisted-default
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/invalid-persisted" \
  "$MCODEX_BIN" accounts status --json
```

Expected markers:

- `effectivePoolId == null`
- `startup.effectivePoolId == null`
- `startup.startupAvailability == "invalidExplicitDefault"`
- `startup.startupResolutionIssue.kind == "persistedDefaultPoolUnavailable"`
- `startup.startupResolutionIssue.source == "persistedSelection"`
- `startup.startupResolutionIssue.poolId == "missing-pool"`

## P0-B: Invalid Config Default

Rows: M-13.

Fixture commands:

```bash
cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- \
  seed --home "$SMOKE_ROOT/invalid-config" --scenario invalid-config-default
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/invalid-config" \
  "$MCODEX_BIN" accounts status --json
```

Expected markers:

- `effectivePoolId == null`
- `startup.effectivePoolId == null`
- `startup.startupAvailability == "invalidExplicitDefault"`
- `startup.startupResolutionIssue.kind == "configDefaultPoolUnavailable"`
- `startup.startupResolutionIssue.source == "configDefault"`
- `startup.startupResolutionIssue.poolId == "missing-pool"`

## P0-B: Observability CLI

Rows: M-17.

Fixture commands:

```bash
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
  "$MCODEX_BIN" accounts events --pool team-main --type quotaObserved --limit 1 --json
```

Expected markers:

- `poolObservability.summary.totalAccounts == 2`
- `summary.activeLeases == 1` for
  `accounts pool show --pool team-main --json`
- `status == "degraded"` for
  `accounts diagnostics --pool team-main --json`
- `issues[0].reasonCode == "cooldownActive"` for
  `accounts diagnostics --pool team-main --json`
- `data[0].eventType == "quotaObserved"` for
  `accounts events --pool team-main --type quotaObserved --limit 1 --json`
- `data[0].details.fixture == "observability"` for
  `accounts events --pool team-main --type quotaObserved --limit 1 --json`
- Diagnostics and events output expose startup, lease, and quota facts without
  exposing tokens or credentials.

## P0-A Wrapper: Local Installer Wrapper Identity

Rows: M-04, M-23.

Commands:

```bash
MCODEX_ROOT="$SMOKE_ROOT/install-root" \
  MCODEX_WRAPPER_DIR="$SMOKE_ROOT/wrappers" \
  ./scripts/dev/install-local.sh
PATH="$SMOKE_ROOT/wrappers:$PATH" command -v mcodex
WRAPPER_PATH="$(PATH="$SMOKE_ROOT/wrappers:$PATH" command -v mcodex)"
printf '%s\n' "$WRAPPER_PATH"
sed -n '1,120p' "$WRAPPER_PATH"
ls -l "$WRAPPER_PATH" "$SMOKE_ROOT/install-root/bin/mcodex"
INSTALLED_VERSION="$(env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/installed-binary-home" \
  "$SMOKE_ROOT/install-root/bin/mcodex" --version)"
printf 'installed binary version: %s\n' "$INSTALLED_VERSION"
WRAPPER_VERSION="$(env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  PATH="$SMOKE_ROOT/wrappers:$PATH" \
  MCODEX_HOME="$SMOKE_ROOT/wrapper-home" \
  MCODEX_BIN="$SMOKE_ROOT/install-root/bin/mcodex" \
  mcodex --version)"
printf 'wrapper version: %s\n' "$WRAPPER_VERSION"
test "$WRAPPER_VERSION" = "$INSTALLED_VERSION"
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  PATH="$SMOKE_ROOT/wrappers:$PATH" \
  MCODEX_HOME="$SMOKE_ROOT/wrapper-home" \
  MCODEX_BIN="$SMOKE_ROOT/install-root/bin/mcodex" \
  mcodex accounts status --json
DIRECT_INVALID_STATUS=0
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  MCODEX_HOME="$SMOKE_ROOT/installed-binary-home" \
  "$SMOKE_ROOT/install-root/bin/mcodex" __mcodex_smoke_invalid_subcommand__ || DIRECT_INVALID_STATUS=$?
printf 'direct invalid status: %s\n' "$DIRECT_INVALID_STATUS"
if [ "$DIRECT_INVALID_STATUS" -eq 0 ]; then
  printf '%s\n' 'expected direct invalid command to fail' >&2
  exit 1
fi
WRAPPER_INVALID_STATUS=0
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  PATH="$SMOKE_ROOT/wrappers:$PATH" \
  MCODEX_HOME="$SMOKE_ROOT/wrapper-home" \
  MCODEX_BIN="$SMOKE_ROOT/install-root/bin/mcodex" \
  mcodex __mcodex_smoke_invalid_subcommand__ || WRAPPER_INVALID_STATUS=$?
printf 'wrapper invalid status: %s\n' "$WRAPPER_INVALID_STATUS"
if [ "$WRAPPER_INVALID_STATUS" -eq 0 ]; then
  printf '%s\n' 'expected wrapper invalid command to fail' >&2
  exit 1
fi
test "$WRAPPER_INVALID_STATUS" = "$DIRECT_INVALID_STATUS"
```

Expected markers:

- Wrapper forwards arguments to the intended installed binary.
- Wrapper `--version` output equals direct installed binary `--version` output.
- Wrapper preserves exit codes; the direct and wrapper invalid subcommands both
  fail, and `WRAPPER_INVALID_STATUS == DIRECT_INVALID_STATUS`.
- Wrapper commands pin `MCODEX_BIN="$SMOKE_ROOT/install-root/bin/mcodex"` so
  inherited setup or debug-binary values cannot change the wrapper target.
- Wrapper uses mcodex home identity and does not fall back to upstream
  `~/.codex`.
- The wrapper product commands clear `CODEX_HOME` and `CODEX_SQLITE_HOME`.
- The wrapper status command uses `$SMOKE_ROOT/wrapper-home`.

Capture wrapper metadata:

- `MCODEX_ROOT="$SMOKE_ROOT/install-root"`
- `MCODEX_WRAPPER_DIR="$SMOKE_ROOT/wrappers"`
- `PATH="$SMOKE_ROOT/wrappers:$PATH"`
- `command -v mcodex`
- wrapper path from `WRAPPER_PATH`
- installed binary path: `$SMOKE_ROOT/install-root/bin/mcodex`
- wrapper script contents or checksum
- `ls -l` metadata for the wrapper and installed binary
- installed binary version output
- wrapper `mcodex --version` output
- direct invalid subcommand `DIRECT_INVALID_STATUS`
- wrapper invalid subcommand `WRAPPER_INVALID_STATUS`
- `MCODEX_HOME="$SMOKE_ROOT/wrapper-home"` used for wrapper commands
- `MCODEX_BIN="$SMOKE_ROOT/install-root/bin/mcodex"` used for wrapper commands

If using the release installer instead of `scripts/dev/install-local.sh`, also
capture `$SMOKE_ROOT/install-root/.install.json` when present. Its metadata is
expected to include `product`, `installMethod`, `currentVersion`,
`installedAt`, `baseRoot`, `versionsDir`, `currentLink`, and `wrapperPath`.

## Manual TUI Rows

Run these rows only after defining the expected visible marker before launch.
For failures, capture a screenshot or terminal transcript and the matching
`accounts status --json` output for the same `MCODEX_HOME`.

| Manual check | Related matrix row | Fixture | Command | Expected marker |
| --- | --- | --- | --- | --- |
| Empty home startup | M-05 | Empty isolated home | `env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$SMOKE_ROOT/empty-tui" "$MCODEX_BIN"` | Normal unauthenticated or no-account startup, not pooled access |
| Single pool startup | M-07 | `single-pool` | `env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$SMOKE_ROOT/single" "$MCODEX_BIN"` | Pooled access; `startup.effectivePoolResolutionSource == "singleVisiblePool"` |
| Multi-pool default-required startup | M-09 | `multi-pool` | `env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$SMOKE_ROOT/multi" "$MCODEX_BIN"` | Pooled access paused/default-required surface; `startup.startupAvailability == "multiplePoolsRequireDefault"` |
| Invalid config follow-up | M-13 | `invalid-config-default` | `env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$SMOKE_ROOT/invalid-config" "$MCODEX_BIN"` | Pooled access paused/invalid default surface; `startup.startupResolutionIssue.kind == "configDefaultPoolUnavailable"` |
| Invalid persisted follow-up | M-14 | `invalid-persisted-default` | `env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$SMOKE_ROOT/invalid-persisted" "$MCODEX_BIN"` | Pooled access paused/invalid default surface; `startup.startupResolutionIssue.kind == "persistedDefaultPoolUnavailable"` |

## Notes

- This runbook intentionally does not implement the fixture crate or smoke
  scripts.
- P0-B fixture commands document the planned canonical helper interface:
  `cargo run --manifest-path codex-rs/Cargo.toml -p codex-smoke-fixtures -- seed`.
- If current JSON field names differ from the markers above, update the
  implementation plan or product output before counting the row as passing.
