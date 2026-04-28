#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ -n "${CODEX_SANDBOX_NETWORK_DISABLED:-}" ]; then
  echo "runtime gate cannot run with CODEX_SANDBOX_NETWORK_DISABLED set; rerun outside the Codex sandbox or in network-enabled CI" >&2
  exit 1
fi

echo "smoke=runtime-gate"
echo "descriptor=$SCRIPT_DIR/mcodex-runtime-gate.tests"
sh "$SCRIPT_DIR/run-named-cargo-tests.sh" "$SCRIPT_DIR/mcodex-runtime-gate.tests"
echo "smoke-mcodex-runtime-gate: pass"
