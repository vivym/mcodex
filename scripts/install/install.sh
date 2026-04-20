#!/bin/sh

set -eu

VERSION_INPUT="${1:-latest}"
BASE_ROOT="${MCODEX_INSTALL_ROOT:-$HOME/.mcodex}"
VERSIONS_DIR="$BASE_ROOT/install"
CURRENT_LINK="$BASE_ROOT/current"
METADATA_FILE="$BASE_ROOT/install.json"
WRAPPER_DIR="${MCODEX_WRAPPER_DIR:-$HOME/.local/bin}"
WRAPPER_PATH="$WRAPPER_DIR/mcodex"
DOWNLOAD_BASE_URL="${MCODEX_DOWNLOAD_BASE_URL:-https://downloads.mcodex.sota.wiki}"

path_action="already"
path_profile=""
TMP_ROOT=""
STAGING_DIR=""

step() {
  printf '==> %s\n' "$1"
}

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

cleanup() {
  if [ -n "$STAGING_DIR" ] && [ -d "$STAGING_DIR" ]; then
    rm -rf "$STAGING_DIR"
  fi
  if [ -n "$TMP_ROOT" ] && [ -d "$TMP_ROOT" ]; then
    rm -rf "$TMP_ROOT"
  fi
}

trap cleanup EXIT INT TERM

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    fail "$1 is required to install mcodex."
  fi
}

download_file() {
  url="$1"
  output="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
    return
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -q -O "$output" "$url"
    return
  fi

  fail "curl or wget is required to install mcodex."
}

json_string_field() {
  file="$1"
  key="$2"

  sed -n "s/.*\"$key\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p" "$file" | head -n 1
}

normalize_version() {
  version="$1"

  case "$version" in
    "" | latest)
      printf 'latest\n'
      ;;
    rust-v*)
      printf '%s\n' "${version#rust-v}"
      ;;
    v*)
      printf '%s\n' "${version#v}"
      ;;
    *)
      printf '%s\n' "$version"
      ;;
  esac
}

validate_version() {
  version="$1"

  if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+[.][0-9]+[.][0-9]+(-((alpha|beta)[.][0-9]+))?$'; then
    fail "Invalid version: $VERSION_INPUT"
  fi
}

target_uname_s() {
  if [ -n "${MCODEX_TEST_UNAME_S:-}" ]; then
    printf '%s\n' "$MCODEX_TEST_UNAME_S"
    return
  fi

  uname -s
}

target_uname_m() {
  if [ -n "${MCODEX_TEST_UNAME_M:-}" ]; then
    printf '%s\n' "$MCODEX_TEST_UNAME_M"
    return
  fi

  uname -m
}

host_platform() {
  case "$(uname -s)" in
    Linux)
      printf 'linux\n'
      ;;
    Darwin)
      printf 'darwin\n'
      ;;
    *)
      fail "Unsupported host platform: $(uname -s)"
      ;;
  esac
}

detect_platform() {
  uname_s="$(target_uname_s)"
  uname_m="$(target_uname_m)"

  case "$uname_s" in
    Linux)
      os="linux"
      ;;
    Darwin)
      os="darwin"
      ;;
    *)
      fail "install.sh supports macOS and Linux. Use install.ps1 on Windows."
      ;;
  esac

  case "$uname_m" in
    x86_64 | amd64)
      arch="x64"
      ;;
    arm64 | aarch64)
      arch="arm64"
      ;;
    *)
      fail "Unsupported architecture: $uname_m"
      ;;
  esac

  if [ "$os" = "darwin" ] && [ "$arch" = "x64" ]; then
    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = "1" ]; then
      arch="arm64"
    fi
  fi

  ARCHIVE_NAME="mcodex-$os-$arch.tar.gz"
}

resolve_latest_version() {
  manifest_path="$TMP_ROOT/latest.json"
  download_file "$DOWNLOAD_BASE_URL/repositories/mcodex/channels/stable/latest.json" "$manifest_path"
  resolved_version="$(json_string_field "$manifest_path" "version")"

  if [ -z "$resolved_version" ]; then
    fail "Failed to resolve the latest mcodex release version."
  fi

  validate_version "$resolved_version"
  printf '%s\n' "$resolved_version"
}

download_checksums() {
  version="$1"
  checksums_path="$TMP_ROOT/SHA256SUMS"
  download_file "$DOWNLOAD_BASE_URL/repositories/mcodex/releases/$version/SHA256SUMS" "$checksums_path"
  printf '%s\n' "$checksums_path"
}

expected_sha_for_archive() {
  checksums_path="$1"
  archive_name="$2"

  awk -v name="$archive_name" '
    {
      file = $2
      sub(/^[*]/, "", file)
      if (file == name) {
        print $1
        exit
      }
    }
  ' "$checksums_path"
}

compute_sha256() {
  file="$1"

  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{ print $1 }'
    return
  fi

  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{ print $1 }'
    return
  fi

  fail "sha256sum or shasum is required to install mcodex."
}

timestamp() {
  date -u '+%Y-%m-%dT%H:%M:%SZ'
}

write_completion_marker() {
  directory="$1"
  version="$2"
  archive_name="$3"
  sha256="$4"
  installed_at="$5"
  marker_path="$directory/.mcodex-install-complete.json"

  cat >"$marker_path" <<EOF
{
  "version": "$version",
  "archiveName": "$archive_name",
  "sha256": "$sha256",
  "installedAt": "$installed_at"
}
EOF
}

version_dir_complete() {
  directory="$1"
  version="$2"
  archive_name="$3"
  sha256="$4"
  marker_path="$directory/.mcodex-install-complete.json"

  if [ ! -f "$marker_path" ]; then
    return 1
  fi

  if [ ! -x "$directory/bin/mcodex" ] || [ ! -x "$directory/bin/rg" ]; then
    return 1
  fi

  marker_version="$(json_string_field "$marker_path" "version")"
  marker_archive_name="$(json_string_field "$marker_path" "archiveName")"
  marker_sha256="$(json_string_field "$marker_path" "sha256")"

  [ "$marker_version" = "$version" ] || return 1
  [ "$marker_archive_name" = "$archive_name" ] || return 1
  [ "$marker_sha256" = "$sha256" ] || return 1
}

stage_version_dir() {
  version="$1"
  archive_name="$2"
  expected_sha="$3"
  archive_url="$DOWNLOAD_BASE_URL/repositories/mcodex/releases/$version/$archive_name"

  STAGING_DIR="$(mktemp -d "$VERSIONS_DIR/.staging.$version.XXXXXX")"
  archive_path="$STAGING_DIR/$archive_name"

  download_file "$archive_url" "$archive_path"
  actual_sha="$(compute_sha256 "$archive_path")"

  if [ "$actual_sha" != "$expected_sha" ]; then
    fail "Archive checksum mismatch for $archive_name."
  fi

  tar -xzf "$archive_path" -C "$STAGING_DIR"
  rm -f "$archive_path"

  if [ ! -x "$STAGING_DIR/bin/mcodex" ] || [ ! -x "$STAGING_DIR/bin/rg" ]; then
    fail "Archive layout for $archive_name is invalid."
  fi

  write_completion_marker "$STAGING_DIR" "$version" "$archive_name" "$expected_sha" "$(timestamp)"
}

publish_version_dir() {
  version_dir="$1"

  if [ -e "$version_dir" ]; then
    previous_dir="$VERSIONS_DIR/.replace.$$.old"
    rm -rf "$previous_dir"
    mv "$version_dir" "$previous_dir"
    mv "$STAGING_DIR" "$version_dir"
    rm -rf "$previous_dir"
  else
    mv "$STAGING_DIR" "$version_dir"
  fi

  STAGING_DIR=""
}

switch_current_link() {
  target_dir="$1"
  tmp_link="$BASE_ROOT/.current.$$.tmp"
  rm -f "$tmp_link"
  ln -s "$target_dir" "$tmp_link"

  case "$HOST_OS" in
    linux)
      mv -Tf "$tmp_link" "$CURRENT_LINK"
      ;;
    darwin)
      mv -fh "$tmp_link" "$CURRENT_LINK"
      ;;
    *)
      rm -f "$tmp_link"
      fail "unsupported platform for current link switch: $HOST_OS"
      ;;
  esac
}

write_wrapper() {
  mkdir -p "$WRAPPER_DIR"
  wrapper_tmp="$(mktemp "$WRAPPER_DIR/.mcodex-wrapper.XXXXXX")"

  cat >"$wrapper_tmp" <<'EOF'
#!/bin/sh
set -eu
base_root="${MCODEX_INSTALL_ROOT:-$HOME/.mcodex}"
target="$base_root/current/bin/mcodex"
if [ ! -x "$target" ]; then
  echo "mcodex installation missing or corrupted; rerun the installer." >&2
  exit 1
fi
export MCODEX_INSTALL_MANAGED=1
export MCODEX_INSTALL_METHOD=script
export MCODEX_INSTALL_ROOT="$base_root"
export PATH="$base_root/current/bin:$PATH"
exec "$target" "$@"
EOF

  chmod 0755 "$wrapper_tmp"
  mv -f "$wrapper_tmp" "$WRAPPER_PATH"
}

write_metadata() {
  version="$1"
  installed_at="$2"
  metadata_tmp="$BASE_ROOT/.install.json.tmp"

  cat >"$metadata_tmp" <<EOF
{
  "product": "mcodex",
  "installMethod": "script",
  "currentVersion": "$version",
  "installedAt": "$installed_at",
  "baseRoot": "$BASE_ROOT",
  "versionsDir": "$VERSIONS_DIR",
  "currentLink": "$CURRENT_LINK",
  "wrapperPath": "$WRAPPER_PATH"
}
EOF

  mv -f "$metadata_tmp" "$METADATA_FILE"
}

add_to_path() {
  path_action="already"
  path_profile=""

  case ":$PATH:" in
    *":$WRAPPER_DIR:"*)
      return
      ;;
  esac

  profile="$HOME/.profile"
  case "${SHELL:-}" in
    */zsh)
      profile="$HOME/.zshrc"
      ;;
    */bash)
      profile="$HOME/.bashrc"
      ;;
  esac

  path_profile="$profile"
  path_line="export PATH=\"$WRAPPER_DIR:\$PATH\""

  if [ -f "$profile" ] && grep -F "$path_line" "$profile" >/dev/null 2>&1; then
    path_action="configured"
    return
  fi

  {
    printf '\n# Added by mcodex installer\n'
    printf '%s\n' "$path_line"
  } >>"$profile"
  path_action="added"
}

require_command awk
require_command grep
require_command mktemp
require_command mv
require_command sed
require_command tar

mkdir -p "$BASE_ROOT" "$VERSIONS_DIR"
TMP_ROOT="$(mktemp -d "$BASE_ROOT/.install.XXXXXX")"
HOST_OS="$(host_platform)"

detect_platform

resolved_input="$(normalize_version "$VERSION_INPUT")"
if [ "$VERSION_INPUT" = "latest" ]; then
  RESOLVED_VERSION="$(resolve_latest_version)"
else
  validate_version "$resolved_input"
  RESOLVED_VERSION="$resolved_input"
fi

CHECKSUMS_FILE="$(download_checksums "$RESOLVED_VERSION")"
EXPECTED_SHA="$(expected_sha_for_archive "$CHECKSUMS_FILE" "$ARCHIVE_NAME")"

if [ -z "$EXPECTED_SHA" ]; then
  fail "No checksum entry found for $ARCHIVE_NAME."
fi

VERSION_DIR="$VERSIONS_DIR/$RESOLVED_VERSION"

step "Installing mcodex CLI $RESOLVED_VERSION"

if ! version_dir_complete "$VERSION_DIR" "$RESOLVED_VERSION" "$ARCHIVE_NAME" "$EXPECTED_SHA"; then
  stage_version_dir "$RESOLVED_VERSION" "$ARCHIVE_NAME" "$EXPECTED_SHA"
  publish_version_dir "$VERSION_DIR"
fi

switch_current_link "$VERSION_DIR"
write_wrapper
write_metadata "$RESOLVED_VERSION" "$(timestamp)"
add_to_path

case "$path_action" in
  added)
    step "PATH updated for future shells in $path_profile"
    ;;
  configured)
    step "PATH is already configured for future shells in $path_profile"
    ;;
  *)
    step "$WRAPPER_DIR is already on PATH"
    ;;
esac

printf 'mcodex CLI %s installed successfully.\n' "$RESOLVED_VERSION"
