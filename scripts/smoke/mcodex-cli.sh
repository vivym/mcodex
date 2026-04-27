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
  SMOKE_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/mcodex-smoke-cli.XXXXXX")
  CLEANUP_SMOKE_ROOT=1
else
  mkdir -p "$SMOKE_ROOT"
  CLEANUP_SMOKE_ROOT=0
fi
SMOKE_HOME_ROOT="$SMOKE_ROOT/home"
mkdir -p "$SMOKE_HOME_ROOT"

cleanup() {
  if [ "$CLEANUP_SMOKE_ROOT" -eq 1 ]; then
    rm -rf "$SMOKE_ROOT"
  fi
}
interrupt() {
  cleanup
  exit 130
}
terminate() {
  cleanup
  exit 143
}
trap cleanup EXIT
trap interrupt INT HUP
trap terminate TERM

fixture() {
  home=$1
  scenario=$2
  echo "fixture_scenario=$scenario fixture_home=$home" >&2
  env -u MCODEX_HOME -u CODEX_HOME -u CODEX_SQLITE_HOME \
    cargo run --quiet --manifest-path "$REPO_ROOT/codex-rs/Cargo.toml" \
      -p codex-smoke-fixtures -- seed \
      --home "$home" --scenario "$scenario" --json
}

run_mcodex() {
  home=$1
  shift
  env -u CODEX_HOME -u CODEX_SQLITE_HOME \
    HOME="$SMOKE_HOME_ROOT" \
    MCODEX_HOME="$home" "$MCODEX_BIN" "$@"
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
mkdir -p "$SMOKE_ROOT/version-home"
echo "version=$(env -u CODEX_HOME -u CODEX_SQLITE_HOME HOME="$SMOKE_HOME_ROOT" MCODEX_HOME="$SMOKE_ROOT/version-home" "$MCODEX_BIN" --version)"
echo "git_sha=$(git -C "$REPO_ROOT" rev-parse HEAD)"
echo "smoke_root=$SMOKE_ROOT"

home="$SMOKE_ROOT/observability"
fixture "$home" observability >/dev/null

status_json=$(run_mcodex "$home" accounts status --json)
assert_path "$status_json" effectivePoolId team-main
assert_path "$status_json" poolObservability.summary.totalAccounts 2
assert_path "$status_json" poolObservability.summary.activeLeases 1

pool_json=$(run_mcodex "$home" accounts pool show --pool team-main --json)
assert_path "$pool_json" poolId team-main
assert_path "$pool_json" summary.totalAccounts 2
assert_path "$pool_json" summary.activeLeases 1

diagnostics_json=$(run_mcodex "$home" accounts diagnostics --pool team-main --json)
assert_path "$diagnostics_json" poolId team-main
assert_path "$diagnostics_json" status degraded
assert_path "$diagnostics_json" 'issues[0].reasonCode' cooldownActive
assert_not_null "$diagnostics_json" generatedAt

events_json=$(run_mcodex "$home" accounts events --pool team-main --type quotaObserved --limit 1 --json)
assert_path "$events_json" poolId team-main
assert_path "$events_json" 'data[0].eventType' quotaObserved
assert_path "$events_json" 'data[0].details.fixture' observability

run_mcodex "$home" accounts pool show --pool team-main >/dev/null
run_mcodex "$home" accounts diagnostics --pool team-main >/dev/null
run_mcodex "$home" accounts events --pool team-main --type quotaObserved >/dev/null

echo "smoke-mcodex-cli: pass"
