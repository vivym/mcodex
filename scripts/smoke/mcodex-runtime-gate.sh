#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
echo "smoke=runtime-gate"
echo "descriptor=$SCRIPT_DIR/mcodex-runtime-gate.tests"
sh "$SCRIPT_DIR/run-named-cargo-tests.sh" "$SCRIPT_DIR/mcodex-runtime-gate.tests"
echo "smoke-mcodex-runtime-gate: pass"
