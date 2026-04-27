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

if ! command -v perl >/dev/null 2>&1; then
  echo "perl is required to enforce per-test smoke timeouts" >&2
  exit 2
fi

TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/mcodex-smoke.XXXXXX")

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

run_with_timeout() {
  timeout_secs=$1
  shift
  perl -e '
use strict;
use warnings;

my $timeout_secs = shift @ARGV;
my $pid = fork();
die "fork failed: $!\n" if !defined $pid;

if ($pid == 0) {
    setpgrp(0, 0) or die "setpgrp failed: $!\n";
    exec @ARGV or die "exec failed: $!\n";
}

my $status;
my $timed_out = 0;
eval {
    local $SIG{ALRM} = sub { die "__codex_smoke_timeout__\n"; };
    alarm $timeout_secs;
    my $waited = waitpid($pid, 0);
    die "waitpid failed: $!\n" if $waited < 0;
    $status = $?;
    alarm 0;
};

if ($@) {
    die $@ if $@ ne "__codex_smoke_timeout__\n";
    $timed_out = 1;
}

if ($timed_out) {
    print STDERR "timed out after ${timeout_secs}s; terminating process group $pid\n";
    kill "TERM", -$pid;
    select undef, undef, undef, 0.5;
    kill "KILL", -$pid;
    waitpid($pid, 0);
    print STDERR "timed out after ${timeout_secs}s; killed process group $pid\n";
    exit 124;
}

if ($status & 127) {
    exit 128 + ($status & 127);
}
exit($status >> 8);
' "$timeout_secs" "$@"
}

line_number=0
tests_seen=0
targets_seen=""
while IFS= read -r line || [ -n "$line" ]; do
  line_number=$((line_number + 1))
  stripped_line=$(printf '%s\n' "$line" | sed 's/^[[:space:]]*//')
  case "$stripped_line" in
    ""|\#*) continue ;;
  esac

  pipe_count=$(printf '%s\n' "$line" | tr -cd '|' | wc -c | tr -d '[:space:]')
  if [ "$pipe_count" -ne 6 ]; then
    echo "invalid descriptor at $DESCRIPTOR_FILE:$line_number: expected 7 fields" >&2
    exit 2
  fi

  rest=$line
  gate=${rest%%|*}
  rest=${rest#*|}
  package=${rest%%|*}
  rest=${rest#*|}
  target_kind=${rest%%|*}
  rest=${rest#*|}
  target_name=${rest%%|*}
  rest=${rest#*|}
  exact_path=${rest%%|*}
  rest=${rest#*|}
  timeout_secs=${rest%%|*}
  notes=${rest#*|}

  case "$gate" in
    runtime|quota) ;;
    *)
      echo "invalid gate '$gate' at $DESCRIPTOR_FILE:$line_number" >&2
      exit 2
      ;;
  esac

  case "$target_kind" in
    --test)
      case "$target_name" in
        ""|-)
          echo "invalid target '$target_kind|$target_name' at $DESCRIPTOR_FILE:$line_number: --test requires a target name" >&2
          exit 2
          ;;
      esac
      ;;
    --lib)
      if [ "$target_name" != "-" ]; then
        echo "invalid target '$target_kind|$target_name' at $DESCRIPTOR_FILE:$line_number: --lib target name must be '-'" >&2
        exit 2
      fi
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

  tests_seen=$((tests_seen + 1))

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

  tmp_list="$TMP_DIR/list.$line_number"
  tmp_run="$TMP_DIR/run.$line_number"

  echo "listing gate=$gate package=$package target=$target_kind $target_name test=$exact_path"
  if [ "$target_kind" = "--lib" ]; then
    cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$exact_path" -- --exact --list >"$tmp_list" 2>&1 || {
      echo "failed to list named regression: $exact_path" >&2
      cat "$tmp_list" >&2
      exit 1
    }
  else
    cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$target_name" "$exact_path" -- --exact --list >"$tmp_list" 2>&1 || {
      echo "failed to list named regression: $exact_path" >&2
      cat "$tmp_list" >&2
      exit 1
    }
  fi

  match_count=$(grep -Fxc "$exact_path: test" "$tmp_list" || true)
  if [ "$match_count" -ne 1 ]; then
    echo "named regression not found exactly once: $exact_path (matches=$match_count)" >&2
    cat "$tmp_list" >&2
    exit 1
  fi

  echo "running gate=$gate package=$package target=$target_kind $target_name timeout=${timeout_secs}s test=$exact_path notes=$notes"
  start_epoch=$(date +%s)

  if [ "$target_kind" = "--lib" ]; then
    run_with_timeout "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$exact_path" -- --exact --nocapture >"$tmp_run" 2>&1 || {
      cat "$tmp_run" >&2
      exit 1
    }
  else
    run_with_timeout "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$target_name" "$exact_path" -- --exact --nocapture >"$tmp_run" 2>&1 || {
      cat "$tmp_run" >&2
      exit 1
    }
  fi
  elapsed=$(( $(date +%s) - start_epoch ))

  if grep -Fq "$SKIP_SENTINEL" "$tmp_run"; then
    echo "critical regression skipped because network is disabled: $exact_path" >&2
    cat "$tmp_run" >&2
    exit 1
  fi
  if grep -Fq "test $exact_path ... ignored" "$tmp_run"; then
    echo "critical regression ignored: $exact_path" >&2
    cat "$tmp_run" >&2
    exit 1
  fi
  proof_count=$(grep -Fxc "test $exact_path ... ok" "$tmp_run" || true)
  if [ "$proof_count" -ne 1 ]; then
    echo "critical regression did not prove exact execution once: $exact_path (proof=$proof_count)" >&2
    cat "$tmp_run" >&2
    exit 1
  fi

  cat "$tmp_run"
  echo "passed gate=$gate package=$package target=$target_kind $target_name elapsed=${elapsed}s test=$exact_path"
done <"$DESCRIPTOR_FILE"

if [ "$tests_seen" -eq 0 ]; then
  echo "descriptor contains no runnable tests: $DESCRIPTOR_FILE" >&2
  exit 2
fi

echo "run-named-cargo-tests: pass descriptor=$DESCRIPTOR_FILE"
