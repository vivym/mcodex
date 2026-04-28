#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

echo "smoke=gate"
sh "$SCRIPT_DIR/mcodex-local.sh" "$@"
sh "$SCRIPT_DIR/mcodex-cli.sh" "$@"
sh "$SCRIPT_DIR/mcodex-runtime-gate.sh"
sh "$SCRIPT_DIR/mcodex-quota-gate.sh"
echo "smoke-mcodex-gate: pass"
