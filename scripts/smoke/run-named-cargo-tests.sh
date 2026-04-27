#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
MANIFEST_PATH="$REPO_ROOT/codex-rs/Cargo.toml"
SKIP_SENTINEL="Skipping test because it cannot execute when network is disabled in a Codex sandbox."
ONE_PASS_SUMMARY_PATTERN='^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; [0-9][0-9]* filtered out; finished in [0-9][0-9]*(\.[0-9][0-9]*)?s$'

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
ACTIVE_TIMEOUT_PID=

cleanup() {
  rm -rf "$TMP_DIR"
}

handle_signal() {
  status=$1
  trap '' INT TERM HUP QUIT
  if [ -n "${ACTIVE_TIMEOUT_PID:-}" ]; then
    kill -TERM "$ACTIVE_TIMEOUT_PID" 2>/dev/null || true
    wait "$ACTIVE_TIMEOUT_PID" 2>/dev/null || true
    ACTIVE_TIMEOUT_PID=
  fi
  cleanup
  exit "$status"
}

trap cleanup EXIT
trap 'handle_signal 130' INT
trap 'handle_signal 143' TERM
trap 'handle_signal 129' HUP
trap 'handle_signal 131' QUIT

run_cargo_with_timeout() {
  timeout_secs=$1
  shift
  timeout_status_file="$TMP_DIR/timeout.$$.status"
  rm -f "$timeout_status_file"
  perl -e '
use strict;
use warnings;

my $status_file = shift @ARGV;
my $timeout_secs = shift @ARGV;
my $pid;

sub finish {
    my ($exit_status) = @_;
    my $tmp_status_file = "$status_file.$$";
    open my $fh, ">", $tmp_status_file or die "open status file failed: $!\n";
    print {$fh} "$exit_status\n";
    close $fh or die "close status file failed: $!\n";
    rename $tmp_status_file, $status_file or die "rename status file failed: $!\n";
    exit $exit_status;
}

sub terminate_child_group {
    my ($exit_status) = @_;
    alarm 0;
    if (!defined $pid) {
        finish($exit_status);
    }
    if ($pid == 0) {
        exit $exit_status;
    }
    kill "TERM", -$pid;
    kill "TERM", $pid;
    select undef, undef, undef, 0.5;
    kill "KILL", -$pid;
    kill "KILL", $pid;
    waitpid($pid, 0);
    finish($exit_status);
}

my %signal_status = (
    HUP  => 129,
    INT  => 130,
    QUIT => 131,
    TERM => 143,
);
for my $sig (keys %signal_status) {
    my $name = $sig;
    $SIG{$name} = sub {
        terminate_child_group($signal_status{$name});
    };
}

$pid = fork();
if (!defined $pid) {
    print STDERR "fork failed: $!\n";
    finish(255);
}

if ($pid == 0) {
    for my $sig (keys %signal_status) {
        $SIG{$sig} = "DEFAULT";
    }
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
    if ($@ ne "__codex_smoke_timeout__\n") {
        print STDERR $@;
        finish(255);
    }
    $timed_out = 1;
}

if ($timed_out) {
    print STDERR "timed out after ${timeout_secs}s; terminating process group $pid\n";
    kill "TERM", -$pid;
    kill "TERM", $pid;
    select undef, undef, undef, 0.5;
    kill "KILL", -$pid;
    kill "KILL", $pid;
    waitpid($pid, 0);
    print STDERR "timed out after ${timeout_secs}s; killed process group $pid\n";
    finish(124);
}

if ($status & 127) {
    finish(128 + ($status & 127));
}
finish($status >> 8);
' "$timeout_status_file" "$timeout_secs" "$@" &
  ACTIVE_TIMEOUT_PID=$!
  while [ ! -s "$timeout_status_file" ]; do
    if ! kill -0 "$ACTIVE_TIMEOUT_PID" 2>/dev/null; then
      set +e
      wait "$ACTIVE_TIMEOUT_PID" 2>/dev/null
      set -e
      ACTIVE_TIMEOUT_PID=
      echo "cargo helper exited before writing status; failing closed" >&2
      return 255
    fi
    sleep 1
  done
  status=$(sed -n '1p' "$timeout_status_file")
  rm -f "$timeout_status_file"
  set +e
  wait "$ACTIVE_TIMEOUT_PID" 2>/dev/null
  set -e
  ACTIVE_TIMEOUT_PID=
  return "$status"
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

  if [ -z "$package" ]; then
    echo "invalid package at $DESCRIPTOR_FILE:$line_number: package must be non-empty" >&2
    exit 2
  fi

  if [ -z "$exact_path" ]; then
    echo "invalid exact path at $DESCRIPTOR_FILE:$line_number: exact path must be non-empty" >&2
    exit 2
  fi

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
      echo "warming target package=$package target=$target_kind $target_name timeout=${timeout_secs}s"
      if [ "$target_kind" = "--lib" ]; then
        run_cargo_with_timeout "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" --no-run || {
          echo "failed to warm target package=$package target=$target_kind $target_name" >&2
          exit 1
        }
      else
        run_cargo_with_timeout "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$target_name" --no-run || {
          echo "failed to warm target package=$package target=$target_kind $target_name" >&2
          exit 1
        }
      fi
      targets_seen="${targets_seen}
$target_key"
      ;;
  esac

  tmp_list="$TMP_DIR/list.$line_number"
  tmp_run="$TMP_DIR/run.$line_number"

  echo "listing gate=$gate package=$package target=$target_kind $target_name timeout=${timeout_secs}s test=$exact_path"
  if [ "$target_kind" = "--lib" ]; then
    run_cargo_with_timeout "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$exact_path" -- --exact --list >"$tmp_list" 2>&1 || {
      echo "failed to list named regression: $exact_path" >&2
      cat "$tmp_list" >&2
      exit 1
    }
  else
    run_cargo_with_timeout "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$target_name" "$exact_path" -- --exact --list >"$tmp_list" 2>&1 || {
      echo "failed to list named regression: $exact_path" >&2
      cat "$tmp_list" >&2
      exit 1
    }
  fi

  list_candidate_count=$(grep -Ec ': test$' "$tmp_list" || true)
  exact_match_count=$(grep -Fxc "$exact_path: test" "$tmp_list" || true)
  if [ "$list_candidate_count" -ne 1 ] || [ "$exact_match_count" -ne 1 ]; then
    echo "named regression not found as the only listed test: $exact_path (list_candidates=$list_candidate_count exact_matches=$exact_match_count)" >&2
    cat "$tmp_list" >&2
    exit 1
  fi

  echo "running gate=$gate package=$package target=$target_kind $target_name timeout=${timeout_secs}s test=$exact_path notes=$notes"
  start_epoch=$(date +%s)

  if [ "$target_kind" = "--lib" ]; then
    run_cargo_with_timeout "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$exact_path" -- --exact --nocapture >"$tmp_run" 2>&1 || {
      cat "$tmp_run" >&2
      exit 1
    }
  else
    run_cargo_with_timeout "$timeout_secs" cargo test --manifest-path "$MANIFEST_PATH" -p "$package" "$target_kind" "$target_name" "$exact_path" -- --exact --nocapture >"$tmp_run" 2>&1 || {
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
  proof_candidate_count=$(grep -Fc "test $exact_path ... ok" "$tmp_run" || true)
  valid_proof_count=$(grep -Fxc "test $exact_path ... ok" "$tmp_run" || true)
  if [ "$proof_candidate_count" -ne 1 ] || [ "$valid_proof_count" -ne 1 ]; then
    echo "critical regression did not prove exact execution exactly once: $exact_path (proof_candidates=$proof_candidate_count valid_proofs=$valid_proof_count)" >&2
    cat "$tmp_run" >&2
    exit 1
  fi
  summary_candidate_count=$(grep -Fc "test result:" "$tmp_run" || true)
  valid_summary_count=$(grep -Ec "$ONE_PASS_SUMMARY_PATTERN" "$tmp_run" || true)
  if [ "$summary_candidate_count" -ne 1 ] || [ "$valid_summary_count" -ne 1 ]; then
    echo "critical regression did not report exactly one valid passing summary: $exact_path (summary_candidates=$summary_candidate_count valid_summaries=$valid_summary_count)" >&2
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
