#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
echo "smoke=quota-gate"
echo "descriptor=$SCRIPT_DIR/mcodex-quota-gate.tests"
sh "$SCRIPT_DIR/run-named-cargo-tests.sh" "$SCRIPT_DIR/mcodex-quota-gate.tests"
echo "smoke-mcodex-quota-gate: pass"
