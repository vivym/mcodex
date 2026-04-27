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
