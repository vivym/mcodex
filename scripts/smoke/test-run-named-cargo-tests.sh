#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
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
  prefixed-proof)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$exact"
    elif printf '%s' "$args" | grep -Fq " --nocapture"; then
      printf 'running 1 test\n'
      printf 'hello'
      printf 'test %s ... ok\n' "$exact"
      printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n'
    else
      echo "unexpected prefixed-proof invocation: $args" >&2
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
  timeout-child)
    if printf '%s' "$args" | grep -Fq " --list"; then
      printf '%s: test\n' "$exact"
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
      printf '%s: test\n' "$exact"
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

descriptor="$TMP_DIR/tests.txt"
write_descriptor "$descriptor"

ok_bin=$(write_fake_cargo ok)
missing_bin=$(write_fake_cargo missing)
duplicate_bin=$(write_fake_cargo duplicate)
skipped_bin=$(write_fake_cargo skipped)
ignored_bin=$(write_fake_cargo ignored)
no_proof_bin=$(write_fake_cargo no-proof)
prefixed_proof_bin=$(write_fake_cargo prefixed-proof)
timeout_child_bin=$(write_fake_cargo timeout-child)
interrupt_child_bin=$(write_fake_cargo interrupt-child)

assert_passes ok env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$TMP_DIR/ok-cargo-args.log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$descriptor"
if ! grep -Fq -- " --exact --nocapture" "$TMP_DIR/ok-cargo-args.log"; then
  echo "expected runner to invoke cargo with --exact --nocapture" >&2
  cat "$TMP_DIR/ok-cargo-args.log" >&2
  exit 1
fi
assert_passes prefixed_proof env FAKE_CARGO_MODE=prefixed-proof PATH="$prefixed_proof_bin:$PATH" sh "$RUNNER" "$descriptor"

empty_test_target_descriptor="$TMP_DIR/empty-test-target.txt"
empty_test_target_log="$TMP_DIR/empty-test-target-cargo-args.log"
write_descriptor_line "$empty_test_target_descriptor" "runtime|codex-core|--test||suite::account_pool::exact_test|30|fake descriptor"
assert_fails empty_test_target env FAKE_CARGO_MODE=ok FAKE_CARGO_ARGS_LOG="$empty_test_target_log" PATH="$ok_bin:$PATH" sh "$RUNNER" "$empty_test_target_descriptor"
if [ -s "$empty_test_target_log" ]; then
  echo "expected invalid empty --test target to fail before cargo invocation" >&2
  cat "$empty_test_target_log" >&2
  exit 1
fi

empty_notes_descriptor="$TMP_DIR/empty-notes.txt"
write_descriptor_line "$empty_notes_descriptor" "runtime|codex-core|--test|all|suite::account_pool::exact_test|30|"
assert_passes empty_notes env FAKE_CARGO_MODE=ok PATH="$ok_bin:$PATH" sh "$RUNNER" "$empty_notes_descriptor"

ignored_lines_descriptor="$TMP_DIR/ignored-lines.txt"
cat >"$ignored_lines_descriptor" <<EOF
   
  # indented comment
runtime|codex-core|--test|all|suite::account_pool::exact_test|30|fake descriptor
EOF
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
interrupt_child_pid_file="$TMP_DIR/interrupt-child.pid"
interrupt_runner_pid_file="$TMP_DIR/interrupt-runner.pid"
interrupt_status_file="$TMP_DIR/interrupt-runner.status"
write_descriptor_line "$interrupt_descriptor" "runtime|codex-core|--test|all|suite::account_pool::exact_test|30|fake descriptor"
env FAKE_CARGO_MODE=interrupt-child \
  FAKE_CARGO_CHILD_PID="$interrupt_child_pid_file" \
  FAKE_CARGO_WATCHDOG_PID="$TMP_DIR/interrupt-watchdog.pid" \
  PATH="$interrupt_child_bin:$PATH" \
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
' "$interrupt_runner_pid_file" "$interrupt_status_file" "$RUNNER" "$interrupt_descriptor" >"$TMP_DIR/interrupt_child.out" 2>"$TMP_DIR/interrupt_child.err" &
interrupt_helper_pid=$!
wait_for_file "$interrupt_runner_pid_file" interrupt-runner-pid
wait_for_file "$interrupt_child_pid_file" interrupt-child-pid
interrupt_start_epoch=$(date +%s)
kill -INT "$(sed -n '1p' "$interrupt_runner_pid_file")"
if ! wait "$interrupt_helper_pid"; then
  echo "interrupt helper failed" >&2
  cat "$TMP_DIR/interrupt_child.out" >&2
  cat "$TMP_DIR/interrupt_child.err" >&2
  exit 1
fi
wait_for_file "$interrupt_status_file" interrupt-status
interrupt_status=$(sed -n '1p' "$interrupt_status_file")
interrupt_elapsed=$(( $(date +%s) - interrupt_start_epoch ))
if [ "$interrupt_status" -ne 130 ]; then
  echo "expected interrupted runner to exit 130, got $interrupt_status" >&2
  cat "$TMP_DIR/interrupt_child.out" >&2
  cat "$TMP_DIR/interrupt_child.err" >&2
  exit 1
fi
if [ "$interrupt_elapsed" -ge 10 ]; then
  echo "expected interrupted runner to exit promptly, took ${interrupt_elapsed}s" >&2
  cat "$TMP_DIR/interrupt_child.out" >&2
  cat "$TMP_DIR/interrupt_child.err" >&2
  exit 1
fi
assert_process_exits "$(sed -n '1p' "$interrupt_child_pid_file")" interrupt-child

assert_fails missing env FAKE_CARGO_MODE=missing PATH="$missing_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails duplicate env FAKE_CARGO_MODE=duplicate PATH="$duplicate_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails skipped env FAKE_CARGO_MODE=skipped PATH="$skipped_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails ignored env FAKE_CARGO_MODE=ignored PATH="$ignored_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails no_proof env FAKE_CARGO_MODE=no-proof PATH="$no_proof_bin:$PATH" sh "$RUNNER" "$descriptor"
assert_fails sandbox env CODEX_SANDBOX_NETWORK_DISABLED=1 FAKE_CARGO_MODE=ok PATH="$ok_bin:$PATH" sh "$RUNNER" "$descriptor"

echo "test-run-named-cargo-tests: pass"
