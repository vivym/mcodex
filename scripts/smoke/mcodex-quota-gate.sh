#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ -n "${CODEX_SANDBOX_NETWORK_DISABLED:-}" ]; then
  echo "quota gate cannot run with CODEX_SANDBOX_NETWORK_DISABLED set; rerun outside the Codex sandbox or in network-enabled CI" >&2
  exit 1
fi

echo "smoke=quota-gate"
echo "descriptor=$SCRIPT_DIR/mcodex-quota-gate.tests"
sh "$SCRIPT_DIR/run-named-cargo-tests.sh" "$SCRIPT_DIR/mcodex-quota-gate.tests"
echo "smoke-mcodex-quota-gate: pass"
