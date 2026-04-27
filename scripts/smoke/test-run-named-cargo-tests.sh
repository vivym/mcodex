#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RUNNER=${RUNNER:-"$SCRIPT_DIR/run-named-cargo-tests.sh"}

if [ ! -f "$RUNNER" ]; then
  echo "runner not found: $RUNNER" >&2
  exit 2
fi

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

write_descriptor_line() {
  file=$1
  line=$2
  printf '%s\n' "$line" >"$file"
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
requested_exact="suite::account_pool::exact_test"
previous_arg=
for arg do
  if [ "$arg" = "--" ]; then
    requested_exact=$previous_arg
    break
  fi
  previous_arg=$arg
done
printf '%s\n' "$args" >> "${FAKE_CARGO_ARGS_LOG:-/dev/null}"
case "$mode" in
  interrupt-warm-child|interrupt-list-child) ;;
  *)
    if printf '%s' "$args" | grep -Fq " --no-run"; then
      printf 'fake cargo warm build\n'
      exit 0
    fi
    ;;
esac
case "$mode" in
  ok)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected ok invocation: $args" >&2
      exit 2
    fi
    ;;
  prefixed-proof)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'hello'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected prefixed-proof invocation: $args" >&2
      exit 2
    fi
    ;;
  suffixed-proof)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok extra\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected suffixed-proof invocation: $args" >&2
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
      printf '%s: test\n%s: test\n' "$requested_exact" "$requested_exact"
    else
      echo "duplicate mode should fail during list, not run" >&2
      exit 2
    fi
    ;;
  skipped)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'Skipping test because it cannot execute when network is disabled in a Codex sandbox.\n'
      printf 'test %s ... ok\n' "$requested_exact"
    else
      echo "unexpected skipped invocation: $args" >&2
      exit 2
    fi
    ;;
  ignored)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ignored\n' "$requested_exact"
    else
      echo "unexpected ignored invocation: $args" >&2
      exit 2
    fi
    ;;
  no-proof)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected no-proof invocation: $args" >&2
      exit 2
    fi
    ;;
  duplicate-proof)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected duplicate-proof invocation: $args" >&2
      exit 2
    fi
    ;;
  eleven-passed)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected eleven-passed invocation: $args" >&2
      exit 2
    fi
    ;;
  duplicate-summary)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected duplicate-summary invocation: $args" >&2
      exit 2
    fi
    ;;
  missing-finished-summary)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out\n'
    else
      echo "unexpected missing-finished-summary invocation: $args" >&2
      exit 2
    fi
    ;;
  suffixed-summary)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s extra\n'
    else
      echo "unexpected suffixed-summary invocation: $args" >&2
      exit 2
    fi
    ;;
  prefixed-summary)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'hello test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected prefixed-summary invocation: $args" >&2
      exit 2
    fi
    ;;
  nonzero-failed-summary)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 1 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected nonzero-failed-summary invocation: $args" >&2
      exit 2
    fi
    ;;
  nonzero-ignored-summary)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... ok\n' "$requested_exact"
      printf 'test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 42 filtered out; finished in 0.00s\n'
    else
      echo "unexpected nonzero-ignored-summary invocation: $args" >&2
      exit 2
    fi
    ;;
  run-fails)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'test %s ... FAILED\n' "$requested_exact"
      exit 101
    else
      echo "unexpected run-fails invocation: $args" >&2
      exit 2
    fi
    ;;
  timeout-child)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      (
        trap 'exit 0' INT TERM HUP
        while :; do
          sleep 1
        done
      ) &
      child_pid=$!
      printf '%s\n' "$child_pid" > "${FAKE_CARGO_CHILD_PID:?}"
      wait "$child_pid"
    else
      echo "unexpected timeout-child invocation: $args" >&2
      exit 2
    fi
    ;;
  interrupt-child)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$requested_exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      (
        trap 'exit 0' INT TERM HUP QUIT
        while :; do
          sleep 1
        done
      ) &
      child_pid=$!
      watchdog_pid=$(ps -o ppid= -p "$$" | sed 's/[[:space:]]//g')
      printf '%s\n' "$child_pid" > "${FAKE_CARGO_CHILD_PID:?}"
      printf '%s\n' "$watchdog_pid" > "${FAKE_CARGO_WATCHDOG_PID:?}"
      wait "$child_pid"
    else
      echo "unexpected interrupt-child invocation: $args" >&2
      exit 2
    fi
    ;;
  interrupt-warm-child)
    if printf '%s' "$args" | grep -Fq " --no-run"; then
      (
        trap 'exit 0' INT TERM HUP QUIT
        while :; do
          sleep 1
        done
      ) &
      child_pid=$!
      watchdog_pid=$(ps -o ppid= -p "$$" | sed 's/[[:space:]]//g')
      printf '%s\n' "$child_pid" > "${FAKE_CARGO_CHILD_PID:?}"
      printf '%s\n' "$watchdog_pid" > "${FAKE_CARGO_WATCHDOG_PID:?}"
      wait "$child_pid"
    else
      echo "unexpected interrupt-warm-child invocation: $args" >&2
      exit 2
    fi
    ;;
  interrupt-list-child)
    if printf '%s' "$args" | grep -Fq " --no-run"; then
      printf 'fake cargo warm build\n'
    elif printf '%s' "$args" | grep -Fq " --list"; then
      (
        trap 'exit 0' INT TERM HUP QUIT
        while :; do
          sleep 1
        done
      ) &
      child_pid=$!
      watchdog_pid=$(ps -o ppid= -p "$$" | sed 's/[[:space:]]//g')
      printf '%s\n' "$child_pid" > "${FAKE_CARGO_CHILD_PID:?}"
      printf '%s\n' "$watchdog_pid" > "${FAKE_CARGO_WATCHDOG_PID:?}"
      wait "$child_pid"
    else
      echo "unexpected interrupt-list-child invocation: $args" >&2
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

write_no_perl_path() {
  fake_dir=$1
  no_perl_dir="$TMP_DIR/no-perl-bin"
  mkdir -p "$no_perl_dir"
  for tool in dirname grep sed tr wc; do
    ln -s "$(command -v "$tool")" "$no_perl_dir/$tool"
  done
  ln -s "$fake_dir/cargo" "$no_perl_dir/cargo"
  printf '%s\n' "$no_perl_dir"
}

assert_process_exits() {
  pid=$1
  name=$2
  attempt=0
  while [ "$attempt" -lt 20 ]; do
    if ! kill -0 "$pid" 2>/dev/null; then
      return
    fi
    sleep 1
    attempt=$((attempt + 1))
  done
  kill "$pid" 2>/dev/null || true
  echo "expected $name process $pid to be cleaned up" >&2
  exit 1
}

wait_for_file() {
  file=$1
  name=$2
  attempt=0
  while [ "$attempt" -lt 20 ]; do
    if [ -s "$file" ]; then
      return
    fi
    sleep 1
    attempt=$((attempt + 1))
  done
  echo "expected $name file: $file" >&2
  exit 1
}

assert_interrupt_cleans_child() {
  name=$1
  mode=$2
  fake_bin=$3
  descriptor_file=$4
  signal_name=$5
  expected_status=$6
  child_pid_file="$TMP_DIR/$name-child.pid"
  runner_pid_file="$TMP_DIR/$name-runner.pid"
  status_file="$TMP_DIR/$name-runner.status"
  out_file="$TMP_DIR/$name.out"
  err_file="$TMP_DIR/$name.err"

  env FAKE_CARGO_MODE="$mode" \
    FAKE_CARGO_CHILD_PID="$child_pid_file" \
    FAKE_CARGO_WATCHDOG_PID="$TMP_DIR/$name-watchdog.pid" \
    PATH="$fake_bin:$PATH" \
    perl -e '
use strict;
use warnings;

$SIG{INT} = "DEFAULT";
my ($runner_pid_file, $status_file, $runner, $descriptor) = @ARGV;
my $pid = fork();
die "fork failed: $!\n" if !defined $pid;

if ($pid == 0) {
    $SIG{INT} = "DEFAULT";
    exec "sh", $runner, $descriptor or die "exec failed: $!\n";
}

open my $pid_fh, ">", $runner_pid_file or die "open runner pid file failed: $!\n";
print {$pid_fh} "$pid\n";
close $pid_fh or die "close runner pid file failed: $!\n";

waitpid($pid, 0);
my $exit_status = ($? & 127) ? 128 + ($? & 127) : ($? >> 8);
open my $status_fh, ">", $status_file or die "open status file failed: $!\n";
print {$status_fh} "$exit_status\n";
close $status_fh or die "close status file failed: $!\n";
' "$runner_pid_file" "$status_file" "$RUNNER" "$descriptor_file" >"$out_file" 2>"$err_file" &
  helper_pid=$!
  wait_for_file "$runner_pid_file" "$name-runner-pid"
  wait_for_file "$child_pid_file" "$name-child-pid"
  start_epoch=$(date +%s)
  kill "-$signal_name" "$(sed -n '1p' "$runner_pid_file")"
  if ! wait "$helper_pid"; then
    echo "$name interrupt helper failed" >&2
    cat "$out_file" >&2
    cat "$err_file" >&2
    exit 1
  fi
  wait_for_file "$status_file" "$name-status"
  interrupt_status=$(sed -n '1p' "$status_file")
  interrupt_elapsed=$(( $(date +%s) - start_epoch ))
  if [ "$interrupt_status" -ne "$expected_status" ]; then
    echo "expected interrupted $name runner to exit $expected_status, got $interrupt_status" >&2
    cat "$out_file" >&2
    cat "$err_file" >&2
    exit 1
  fi
  if [ "$interrupt_elapsed" -ge 10 ]; then
    echo "expected interrupted $name runner to exit promptly, took ${interrupt_elapsed}s" >&2
    cat "$out_file" >&2
    cat "$err_file" >&2
    exit 1
  fi
  assert_process_exits "$(sed -n '1p' "$child_pid_file")" "$name-child"
}

descriptor="$TMP_DIR/tests.txt"
write_descriptor "$descriptor"

ok_bin=$(write_fake_cargo ok)
missing_bin=$(write_fake_cargo missing)
duplicate_bin=$(write_fake_cargo duplicate)
skipped_bin=$(write_fake_cargo skipped)
ignored_bin=$(write_fake_cargo ignored)
no_proof_bin=$(write_fake_cargo no-proof)
prefixed_proof_bin=$(write_fake_cargo prefixed-proof)
suffixed_proof_bin=$(write_fake_cargo suffixed-proof)
duplicate_proof_bin=$(write_fake_cargo duplicate-proof)
eleven_passed_bin=$(write_fake_cargo eleven-passed)
duplicate_summary_bin=$(write_fake_cargo duplicate-summary)
missing_finished_summary_bin=$(write_fake_cargo missing-finished-summary)
suffixed_summary_bin=$(write_fake_cargo suffixed-summary)
prefixed_summary_bin=$(write_fake_cargo prefixed-summary)
nonzero_failed_summary_bin=$(write_fake_cargo nonzero-failed-summary)
nonzero_ignored_summary_bin=$(write_fake_cargo nonzero-ignored-summary)
run_fails_bin=$(write_fake_cargo run-fails)
timeout_child_bin=$(write_fake_cargo timeout-child)
interrupt_child_bin=$(write_fake_cargo interrupt-child)
interrupt_warm_child_bin=$(write_fake_cargo interrupt-warm-child)
interrupt_list_child_bin=$(write_fake_cargo interrupt-list-child)

assert_fails no_descriptor sh "$RUNNER"
if ! grep -Fq "usage: sh scripts/smoke/run-named-cargo-tests.sh <descriptor-file>" "$TMP_DIR/no_descriptor.err"; then
  echo "expected no descriptor usage message" >&2
  cat "$TMP_DIR/no_descriptor.err" >&2
  exit 1
fi

assert_fails missing_descriptor sh "$RUNNER" "$TMP_DIR/missing-descriptor.txt"
if ! grep -Fq "descriptor file not found" "$TMP_DIR/missing_descriptor.err"; then
  echo "expected missing descriptor message" >&2
  cat "$TMP_DIR/missing_descriptor.err" >&2
  exit 1
fi

if [ -z "${CHECK_MISSING_RUNNER_CHILD:-}" ]; then
  assert_fails missing_runner env CHECK_MISSING_RUNNER_CHILD=1 RUNNER="$TMP_DIR/missing-runner.sh" sh "$0"
  if ! grep -Fq "runner not found" "$TMP_DIR/missing_runner.err"; then
    echo "expected missing runner message" >&2
    cat "$TMP_DIR/missing_runner.err" >&2
    exit 1
  fi
fi

assert_passes ok env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$TMP_DIR/ok-cargo-args.log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$descriptor"
if [ "$(wc -l <"$TMP_DIR/ok-cargo-args.log" | tr -d '[:space:]')" -ne 3 ]; then
  echo "expected --test all descriptor to invoke cargo three times" >&2
  cat "$TMP_DIR/ok-cargo-args.log" >&2
  exit 1
fi
if ! grep -Fq -- " -p codex-core --test all --no-run " "$TMP_DIR/ok-cargo-args.log"; then
  echo "expected --test all warm cargo args" >&2
  cat "$TMP_DIR/ok-cargo-args.log" >&2
  exit 1
fi
if ! grep -Fq -- " -p codex-core --test all suite::account_pool::exact_test -- --exact --list " "$TMP_DIR/ok-cargo-args.log"; then
  echo "expected --test all list cargo args" >&2
  cat "$TMP_DIR/ok-cargo-args.log" >&2
  exit 1
fi
if ! grep -Fq -- " --exact --nocapture" "$TMP_DIR/ok-cargo-args.log"; then
  echo "expected runner to invoke cargo with --exact --nocapture" >&2
  cat "$TMP_DIR/ok-cargo-args.log" >&2
  exit 1
fi
if ! grep -Fq -- " -p codex-core --test all suite::account_pool::exact_test -- --exact --nocapture " "$TMP_DIR/ok-cargo-args.log"; then
  echo "expected --test all run cargo args" >&2
  cat "$TMP_DIR/ok-cargo-args.log" >&2
  exit 1
fi
assert_fails prefixed_proof env FAKE_CARGO_MODE=prefixed-proof PATH="$prefixed_proof_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails suffixed_proof env FAKE_CARGO_MODE=suffixed-proof PATH="$suffixed_proof_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails duplicate_proof env FAKE_CARGO_MODE=duplicate-proof PATH="$duplicate_proof_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails eleven_passed env FAKE_CARGO_MODE=eleven-passed PATH="$eleven_passed_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails duplicate_summary env FAKE_CARGO_MODE=duplicate-summary PATH="$duplicate_summary_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails missing_finished_summary env FAKE_CARGO_MODE=missing-finished-summary PATH="$missing_finished_summary_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails suffixed_summary env FAKE_CARGO_MODE=suffixed-summary PATH="$suffixed_summary_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails prefixed_summary env FAKE_CARGO_MODE=prefixed-summary PATH="$prefixed_summary_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails nonzero_failed_summary env FAKE_CARGO_MODE=nonzero-failed-summary PATH="$nonzero_failed_summary_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails nonzero_ignored_summary env FAKE_CARGO_MODE=nonzero-ignored-summary PATH="$nonzero_ignored_summary_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails run_fails env FAKE_CARGO_MODE=run-fails PATH="$run_fails_bin:$PATH" sh "$RUNNER" "$descriptor"
if ! grep -Fq "proofs=0" "$TMP_DIR/prefixed_proof.err"; then
  echo "expected prefixed proof failure to report proofs=0" >&2
  cat "$TMP_DIR/prefixed_proof.err" >&2
  exit 1
fi
if ! grep -Fq "proofs=0" "$TMP_DIR/suffixed_proof.err"; then
  echo "expected suffixed proof failure to report proofs=0" >&2
  cat "$TMP_DIR/suffixed_proof.err" >&2
  exit 1
fi
if ! grep -Fq "proofs=2" "$TMP_DIR/duplicate_proof.err"; then
  echo "expected duplicate proof failure to report proofs=2" >&2
  cat "$TMP_DIR/duplicate_proof.err" >&2
  exit 1
fi

empty_test_target_descriptor="$TMP_DIR/empty-test-target.txt"
empty_test_target_log="$TMP_DIR/empty-test-target-cargo-args.log"
write_descriptor_line "$empty_test_target_descriptor" "runtime|codex-core|--test||suite::account_pool::exact_test|30|fake descriptor"
assert_fails empty_test_target env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$empty_test_target_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$empty_test_target_descriptor"
if [ -s "$empty_test_target_log" ]; then
  echo "expected invalid empty --test target to fail before cargo invocation" >&2
  cat "$empty_test_target_log" >&2
  exit 1
fi

dash_test_target_descriptor="$TMP_DIR/dash-test-target.txt"
dash_test_target_log="$TMP_DIR/dash-test-target-cargo-args.log"
write_descriptor_line "$dash_test_target_descriptor" "runtime|codex-core|--test|-|suite::account_pool::exact_test|30|fake descriptor"
assert_fails dash_test_target env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$dash_test_target_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$dash_test_target_descriptor"
if [ -s "$dash_test_target_log" ]; then
  echo "expected invalid '-' --test target to fail before cargo invocation" >&2
  cat "$dash_test_target_log" >&2
  exit 1
fi

empty_package_descriptor="$TMP_DIR/empty-package.txt"
empty_package_log="$TMP_DIR/empty-package-cargo-args.log"
write_descriptor_line "$empty_package_descriptor" "runtime||--test|all|suite::account_pool::exact_test|30|fake descriptor"
assert_fails empty_package env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$empty_package_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$empty_package_descriptor"
if [ -s "$empty_package_log" ]; then
  echo "expected invalid empty package to fail before cargo invocation" >&2
  cat "$empty_package_log" >&2
  exit 1
fi

empty_exact_path_descriptor="$TMP_DIR/empty-exact-path.txt"
empty_exact_path_log="$TMP_DIR/empty-exact-path-cargo-args.log"
write_descriptor_line "$empty_exact_path_descriptor" "runtime|codex-core|--test|all||30|fake descriptor"
assert_fails empty_exact_path env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$empty_exact_path_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$empty_exact_path_descriptor"
if [ -s "$empty_exact_path_log" ]; then
  echo "expected invalid empty exact path to fail before cargo invocation" >&2
  cat "$empty_exact_path_log" >&2
  exit 1
fi

invalid_pipe_count_descriptor="$TMP_DIR/invalid-pipe-count.txt"
invalid_pipe_count_log="$TMP_DIR/invalid-pipe-count-cargo-args.log"
write_descriptor_line "$invalid_pipe_count_descriptor" "runtime|codex-core|--test|all|suite::account_pool::exact_test|30"
assert_fails invalid_pipe_count env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$invalid_pipe_count_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$invalid_pipe_count_descriptor"
if [ -s "$invalid_pipe_count_log" ]; then
  echo "expected invalid pipe count to fail before cargo invocation" >&2
  cat "$invalid_pipe_count_log" >&2
  exit 1
fi

invalid_gate_descriptor="$TMP_DIR/invalid-gate.txt"
invalid_gate_log="$TMP_DIR/invalid-gate-cargo-args.log"
write_descriptor_line "$invalid_gate_descriptor" "wrong|codex-core|--test|all|suite::account_pool::exact_test|30|fake descriptor"
assert_fails invalid_gate env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$invalid_gate_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$invalid_gate_descriptor"
if [ -s "$invalid_gate_log" ]; then
  echo "expected invalid gate to fail before cargo invocation" >&2
  cat "$invalid_gate_log" >&2
  exit 1
fi

invalid_target_kind_descriptor="$TMP_DIR/invalid-target-kind.txt"
invalid_target_kind_log="$TMP_DIR/invalid-target-kind-cargo-args.log"
write_descriptor_line "$invalid_target_kind_descriptor" "runtime|codex-core|--bin|all|suite::account_pool::exact_test|30|fake descriptor"
assert_fails invalid_target_kind env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$invalid_target_kind_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$invalid_target_kind_descriptor"
if [ -s "$invalid_target_kind_log" ]; then
  echo "expected invalid target_kind to fail before cargo invocation" >&2
  cat "$invalid_target_kind_log" >&2
  exit 1
fi

lib_descriptor="$TMP_DIR/lib-target.txt"
lib_log="$TMP_DIR/lib-cargo-args.log"
write_descriptor_line "$lib_descriptor" "runtime|codex-core|--lib|-|suite::account_pool::exact_test|30|fake descriptor"
assert_passes lib_target env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$lib_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$lib_descriptor"
if ! grep -Fq -- " -p codex-core --lib --no-run " "$lib_log"; then
  echo "expected --lib warm cargo args" >&2
  cat "$lib_log" >&2
  exit 1
fi
if ! grep -Fq -- " -p codex-core --lib suite::account_pool::exact_test -- --exact --list " "$lib_log"; then
  echo "expected --lib list cargo args" >&2
  cat "$lib_log" >&2
  exit 1
fi
if ! grep -Fq -- " -p codex-core --lib suite::account_pool::exact_test -- --exact --nocapture " "$lib_log"; then
  echo "expected --lib run cargo args" >&2
  cat "$lib_log" >&2
  exit 1
fi
if grep -Fq -- " --lib - " "$lib_log"; then
  echo "expected --lib cargo args not to include '-' target name" >&2
  cat "$lib_log" >&2
  exit 1
fi

invalid_lib_descriptor="$TMP_DIR/invalid-lib-target.txt"
invalid_lib_log="$TMP_DIR/invalid-lib-target-cargo-args.log"
write_descriptor_line "$invalid_lib_descriptor" "runtime|codex-core|--lib|name|suite::account_pool::exact_test|30|fake descriptor"
assert_fails invalid_lib_target env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$invalid_lib_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$invalid_lib_descriptor"
if [ -s "$invalid_lib_log" ]; then
  echo "expected invalid --lib target name to fail before cargo invocation" >&2
  cat "$invalid_lib_log" >&2
  exit 1
fi

zero_timeout_descriptor="$TMP_DIR/zero-timeout.txt"
zero_timeout_log="$TMP_DIR/zero-timeout-cargo-args.log"
write_descriptor_line "$zero_timeout_descriptor" "runtime|codex-core|--test|all|suite::account_pool::exact_test|0|fake descriptor"
assert_fails zero_timeout env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$zero_timeout_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$zero_timeout_descriptor"
if [ -s "$zero_timeout_log" ]; then
  echo "expected zero timeout to fail before cargo invocation" >&2
  cat "$zero_timeout_log" >&2
  exit 1
fi

non_integer_timeout_descriptor="$TMP_DIR/non-integer-timeout.txt"
non_integer_timeout_log="$TMP_DIR/non-integer-timeout-cargo-args.log"
write_descriptor_line "$non_integer_timeout_descriptor" "runtime|codex-core|--test|all|suite::account_pool::exact_test|abc|fake descriptor"
assert_fails non_integer_timeout env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$non_integer_timeout_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$non_integer_timeout_descriptor"
if [ -s "$non_integer_timeout_log" ]; then
  echo "expected non-integer timeout to fail before cargo invocation" >&2
  cat "$non_integer_timeout_log" >&2
  exit 1
fi

duplicate_target_descriptor="$TMP_DIR/duplicate-target.txt"
duplicate_target_log="$TMP_DIR/duplicate-target-cargo-args.log"
{
  printf '%s\n' "runtime|codex-core|--test|all|suite::account_pool::exact_test|30|fake descriptor"
  printf '%s\n' "runtime|codex-core|--test|all|suite::account_pool::second_exact_test|30|fake descriptor"
} >"$duplicate_target_descriptor"
assert_passes duplicate_target env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$duplicate_target_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$duplicate_target_descriptor"
warm_count=$(grep -Fc -- " -p codex-core --test all --no-run " "$duplicate_target_log" || true)
if [ "$warm_count" -ne 1 ]; then
  echo "expected duplicate package/target descriptors to warm once, got $warm_count" >&2
  cat "$duplicate_target_log" >&2
  exit 1
fi

if ! grep -Fq '[ ! -s "$timeout_status_file" ]' "$RUNNER"; then
  echo "expected runner to wait for non-empty timeout status file" >&2
  exit 1
fi

empty_notes_descriptor="$TMP_DIR/empty-notes.txt"
write_descriptor_line "$empty_notes_descriptor" "runtime|codex-core|--test|all|suite::account_pool::exact_test|30|"
assert_passes empty_notes env FAKE_CARGO_MODE=ok PATH="$ok_bin:$PATH" sh "$RUNNER" "$empty_notes_descriptor"

ignored_lines_descriptor="$TMP_DIR/ignored-lines.txt"
{
  printf '%s\n' '   '
  printf '%s\n' '  # indented comment'
  printf '%s\n' 'runtime|codex-core|--test|all|suite::account_pool::exact_test|30|fake descriptor'
} >"$ignored_lines_descriptor"
assert_passes ignored_lines env FAKE_CARGO_MODE=ok PATH="$ok_bin:$PATH" sh "$RUNNER" "$ignored_lines_descriptor"

comment_only_descriptor="$TMP_DIR/comment-only.txt"
{
  printf '   \n'
  printf '  # indented comment\n'
} >"$comment_only_descriptor"
assert_fails comment_only env FAKE_CARGO_MODE=ok PATH="$ok_bin:$PATH" sh "$RUNNER" "$comment_only_descriptor"
if ! grep -Fq "descriptor contains no runnable tests" "$TMP_DIR/comment_only.err"; then
  echo "expected comment-only descriptor to report no runnable tests" >&2
  cat "$TMP_DIR/comment_only.err" >&2
  exit 1
fi

no_perl_bin=$(write_no_perl_path "$ok_bin")
no_perl_log="$TMP_DIR/no-perl-cargo-args.log"
assert_fails no_perl env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$no_perl_log" PATH="$no_perl_bin" /bin/sh "$RUNNER" "$descriptor"
if [ -s "$no_perl_log" ]; then
  echo "expected missing perl to fail before cargo invocation" >&2
  cat "$no_perl_log" >&2
  exit 1
fi
if ! grep -Fq "perl is required to enforce per-test smoke timeouts" "$TMP_DIR/no_perl.err"; then
  echo "expected missing perl message" >&2
  cat "$TMP_DIR/no_perl.err" >&2
  exit 1
fi

timeout_descriptor="$TMP_DIR/timeout-child.txt"
timeout_child_pid_file="$TMP_DIR/timeout-child.pid"
write_descriptor_line "$timeout_descriptor" "runtime|codex-core|--test|all|suite::account_pool::exact_test|1|fake descriptor"
assert_fails timeout_child env FAKE_CARGO_MODE=timeout-child FAKE_CARGO_CHILD_PID="$timeout_child_pid_file" PATH="$timeout_child_bin:$PATH" sh "$RUNNER" "$timeout_descriptor"
if [ ! -s "$timeout_child_pid_file" ]; then
  echo "expected timeout child pid file" >&2
  exit 1
fi
assert_process_exits "$(sed -n '1p' "$timeout_child_pid_file")" timeout-child
if ! grep -Fq "timed out after 1s" "$TMP_DIR/timeout_child.err"; then
  echo "expected timeout message" >&2
  cat "$TMP_DIR/timeout_child.err" >&2
  exit 1
fi

interrupt_descriptor="$TMP_DIR/interrupt-child.txt"
write_descriptor_line "$interrupt_descriptor" "runtime|codex-core|--test|all|suite::account_pool::exact_test|30|fake descriptor"
assert_interrupt_cleans_child interrupt interrupt-child "$interrupt_child_bin" "$interrupt_descriptor" INT 130
assert_interrupt_cleans_child warm-interrupt interrupt-warm-child "$interrupt_warm_child_bin" "$interrupt_descriptor" TERM 143
assert_interrupt_cleans_child list-interrupt interrupt-list-child "$interrupt_list_child_bin" "$interrupt_descriptor" TERM 143

assert_fails missing env FAKE_CARGO_MODE=missing PATH="$missing_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails duplicate env FAKE_CARGO_MODE=duplicate PATH="$duplicate_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails skipped env FAKE_CARGO_MODE=skipped PATH="$skipped_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails ignored env FAKE_CARGO_MODE=ignored PATH="$ignored_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails no_proof env FAKE_CARGO_MODE=no-proof PATH="$no_proof_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails sandbox env CODEX_SANDBOX_NETWORK_DISABLED=1 FAKE_CARGO_MODE=ok PATH="$ok_bin:$PATH" sh "$RUNNER" "$descriptor"

echo "test-run-named-cargo-tests: pass"
