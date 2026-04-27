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

status_json() {
  env -u CODEX_HOME -u CODEX_SQLITE_HOME \
    HOME="$SMOKE_HOME_ROOT" \
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
mkdir -p "$SMOKE_ROOT/version-home"
echo "version=$(env -u CODEX_HOME -u CODEX_SQLITE_HOME HOME="$SMOKE_HOME_ROOT" MCODEX_HOME="$SMOKE_ROOT/version-home" "$MCODEX_BIN" --version)"
echo "git_sha=$(git -C "$REPO_ROOT" rev-parse HEAD)"
echo "smoke_root=$SMOKE_ROOT"

mkdir -p "$SMOKE_ROOT/help-home"
env -u CODEX_HOME -u CODEX_SQLITE_HOME \
  HOME="$SMOKE_HOME_ROOT" \
  MCODEX_HOME="$SMOKE_ROOT/help-home" \
  "$MCODEX_BIN" --help >/dev/null

empty_home="$SMOKE_ROOT/empty"
mkdir -p "$empty_home"
empty_status=$(status_json "$empty_home")
assert_null "$empty_status" effectivePoolId
assert_null "$empty_status" startup.effectivePoolId
assert_null "$empty_status" poolObservability

sentinel_codex_home="$SMOKE_ROOT/codex-home-with-pool"
mkdir -p "$sentinel_codex_home"
cat >"$sentinel_codex_home/config.toml" <<'EOF'
[accounts]
default_pool = "team-main"

[accounts.pools.team-main]
allow_context_reuse = false
EOF
mcodex_home="$SMOKE_ROOT/mcodex-empty"
mkdir -p "$mcodex_home"
conflict_status=$(env -u CODEX_SQLITE_HOME \
  HOME="$SMOKE_HOME_ROOT" \
  MCODEX_HOME="$mcodex_home" CODEX_HOME="$sentinel_codex_home" \
  "$MCODEX_BIN" accounts status --json)
assert_null "$conflict_status" effectivePoolId
assert_null "$conflict_status" configuredDefaultPoolId
assert_null "$conflict_status" startup.effectivePoolId
assert_null "$conflict_status" poolObservability

single_home="$SMOKE_ROOT/single"
fixture "$single_home" single-pool >/dev/null
single_status=$(status_json "$single_home")
assert_path "$single_status" startup.effectivePoolResolutionSource singleVisiblePool
assert_path "$single_status" effectivePoolId team-main

multi_home="$SMOKE_ROOT/multi"
fixture "$multi_home" multi-pool >/dev/null
multi_status=$(status_json "$multi_home")
assert_path "$multi_status" startup.startupAvailability multiplePoolsRequireDefault
assert_null "$multi_status" effectivePoolId
assert_null "$multi_status" startup.effectivePoolId

default_home="$SMOKE_ROOT/default"
fixture "$default_home" multi-pool >/dev/null
env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$default_home" \
  HOME="$SMOKE_HOME_ROOT" "$MCODEX_BIN" accounts pool default set team-main >/dev/null
default_status=$(status_json "$default_home")
assert_path "$default_status" startup.effectivePoolResolutionSource persistedSelection
assert_path "$default_status" effectivePoolId team-main
env -u CODEX_HOME -u CODEX_SQLITE_HOME MCODEX_HOME="$default_home" \
  HOME="$SMOKE_HOME_ROOT" "$MCODEX_BIN" accounts pool default clear >/dev/null
cleared_status=$(status_json "$default_home")
assert_path "$cleared_status" startup.startupAvailability multiplePoolsRequireDefault
assert_null "$cleared_status" effectivePoolId
assert_null "$cleared_status" startup.effectivePoolId

config_home="$SMOKE_ROOT/config-conflict"
fixture "$config_home" config-default-conflict >/dev/null
config_status=$(status_json "$config_home")
assert_path "$config_status" startup.effectivePoolResolutionSource configDefault
assert_path "$config_status" startup.effectivePoolId team-main
assert_path "$config_status" effectivePoolId team-main
assert_path "$config_status" persistedDefaultPoolId team-other
assert_path "$config_status" configuredDefaultPoolId team-main

invalid_persisted_home="$SMOKE_ROOT/invalid-persisted"
fixture "$invalid_persisted_home" invalid-persisted-default >/dev/null
invalid_persisted_status=$(status_json "$invalid_persisted_home")
assert_null "$invalid_persisted_status" effectivePoolId
assert_null "$invalid_persisted_status" startup.effectivePoolId
assert_path "$invalid_persisted_status" startup.startupAvailability invalidExplicitDefault
assert_path "$invalid_persisted_status" startup.startupResolutionIssue.kind persistedDefaultPoolUnavailable
assert_path "$invalid_persisted_status" startup.startupResolutionIssue.source persistedSelection
assert_path "$invalid_persisted_status" startup.startupResolutionIssue.poolId missing-pool

invalid_config_home="$SMOKE_ROOT/invalid-config"
fixture "$invalid_config_home" invalid-config-default >/dev/null
invalid_config_status=$(status_json "$invalid_config_home")
assert_null "$invalid_config_status" effectivePoolId
assert_null "$invalid_config_status" startup.effectivePoolId
assert_path "$invalid_config_status" startup.startupAvailability invalidExplicitDefault
assert_path "$invalid_config_status" startup.startupResolutionIssue.kind configDefaultPoolUnavailable
assert_path "$invalid_config_status" startup.startupResolutionIssue.source configDefault
assert_path "$invalid_config_status" startup.startupResolutionIssue.poolId missing-pool

echo "smoke-mcodex-local: pass"
