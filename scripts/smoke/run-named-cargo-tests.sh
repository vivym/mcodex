#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
MANIFEST_PATH="$REPO_ROOT/codex-rs/Cargo.toml"
SKIP_SENTINEL="Skipping test because it cannot execute when network is disabled in a Codex sandbox."

if [ "$#" -ne 1 ]; then
  echo "usage: sh scripts/smoke/run-named-cargo-tests.sh <descriptor-file>" >&2
  exit 2
fi

DESCRIPTOR_FILE=$1

if [ ! -f "$DESCRIPTOR_FILE" ]; then
  echo "descriptor file not found: $DESCRIPTOR_FILE" >&2
  exit 2
fi

if [ -n "${CODEX_SANDBOX_NETWORK_DISABLED:-}" ]; then
  echo "named cargo smoke tests cannot run with CODEX_SANDBOX_NETWORK_DISABLED set; rerun outside the Codex sandbox or in network-enabled CI" >&2
  exit 1
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
