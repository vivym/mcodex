#!/bin/sh

set -eu

step() {
  printf '==> %s\n' "$1"
}

path_contains() {
  case ":${PATH:-}:" in
    *":$1:"*) return 0 ;;
    *) return 1 ;;
  esac
}

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
CODEX_RS_DIR="$REPO_ROOT/codex-rs"

MCODEX_ROOT="${MCODEX_ROOT:-$HOME/.mcodex-dev}"
MCODEX_HOME_DEFAULT="$MCODEX_ROOT/home"
INSTALL_BIN_DIR="$MCODEX_ROOT/bin"
WRAPPER_DIR="${MCODEX_WRAPPER_DIR:-$HOME/.local/bin}"
INSTALLED_BINARY="$INSTALL_BIN_DIR/mcodex"
WRAPPER_PATH="$WRAPPER_DIR/mcodex"
BUILD_OUTPUT_FILE="$(mktemp "${TMPDIR:-/tmp}/mcodex-build-output.XXXXXX")"

cleanup() {
  rm -f "$BUILD_OUTPUT_FILE"
}

trap cleanup EXIT HUP INT TERM

step "Building release mcodex binary"
if (
  cd "$CODEX_RS_DIR"
  cargo build --release --bin mcodex --message-format=json-render-diagnostics >"$BUILD_OUTPUT_FILE"
); then
  :
else
  cat "$BUILD_OUTPUT_FILE" >&2 || true
  exit 1
fi

SOURCE_BINARY="$(
  awk '
    /"reason":"compiler-artifact"/ && /"name":"mcodex"/ && /"executable":/ {
      line = $0;
      sub(/^.*"executable":"/, "", line);
      sub(/".*$/, "", line);
      print line;
      exit;
    }
  ' "$BUILD_OUTPUT_FILE"
)"

step "Installing local mcodex binary to $INSTALL_BIN_DIR"
mkdir -p "$INSTALL_BIN_DIR"
if [ -z "$SOURCE_BINARY" ]; then
  cat "$BUILD_OUTPUT_FILE" >&2 || true
  printf '%s\n' "failed to determine the built mcodex binary path from cargo output" >&2
  exit 1
fi
if [ ! -x "$SOURCE_BINARY" ]; then
  printf '%s\n' "built mcodex binary not found at $SOURCE_BINARY" >&2
  exit 1
fi
cp "$SOURCE_BINARY" "$INSTALLED_BINARY"
chmod 0755 "$INSTALLED_BINARY"

step "Installing mcodex launcher to $WRAPPER_DIR"
mkdir -p "$WRAPPER_DIR"
wrapper_tmp="$(mktemp "$WRAPPER_DIR/.mcodex.tmp.XXXXXX")"
cat >"$wrapper_tmp" <<EOF
#!/bin/sh

set -eu

MCODEX_ROOT="\${MCODEX_ROOT:-$MCODEX_ROOT}"
MCODEX_HOME="\${MCODEX_HOME:-\${MCODEX_ROOT}/home}"
MCODEX_BIN="\${MCODEX_BIN:-\${MCODEX_ROOT}/bin/mcodex}"

if [ ! -x "\$MCODEX_BIN" ]; then
  printf '%s\n' "mcodex binary not found at \$MCODEX_BIN; run $REPO_ROOT/scripts/dev/install-local.sh again." >&2
  exit 1
fi

mkdir -p "\$MCODEX_HOME"
export MCODEX_HOME="\$MCODEX_HOME"

exec "\$MCODEX_BIN" "\$@"
EOF
chmod 0755 "$wrapper_tmp"
mv "$wrapper_tmp" "$WRAPPER_PATH"

step "Installed isolated state home: $MCODEX_HOME_DEFAULT"
step "Installed binary: $INSTALLED_BINARY"
step "Installed launcher: $WRAPPER_PATH"

if path_contains "$WRAPPER_DIR"; then
  step "Run: mcodex"
else
  step "Run now: PATH=\"$WRAPPER_DIR:\$PATH\" mcodex"
  step "Optional: add $WRAPPER_DIR to your PATH for future shells"
fi
