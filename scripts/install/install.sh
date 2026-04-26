#!/bin/sh

set -eu

RELEASE="latest"

BASE_ROOT="${MCODEX_INSTALL_ROOT:-$HOME/.mcodex}"
VERSIONS_DIR="$BASE_ROOT/install"
CURRENT_LINK="$BASE_ROOT/current"
METADATA_FILE="$BASE_ROOT/install.json"
WRAPPER_DIR="${MCODEX_WRAPPER_DIR:-$HOME/.local/bin}"
WRAPPER_PATH="$WRAPPER_DIR/mcodex"
DOWNLOAD_BASE_URL="${MCODEX_DOWNLOAD_BASE_URL:-https://downloads.mcodex.sota.wiki}"
LOCK_FILE="$BASE_ROOT/install.lock"
LOCK_DIR="$BASE_ROOT/install.lock.d"
LOCK_STALE_AFTER_SECS=600

path_action="already"
path_profile=""
TMP_ROOT=""
STAGING_DIR=""
REPLACE_BACKUP_DIR=""
REPLACE_TARGET_DIR=""
REPLACE_ACTIVE=0
lock_kind=""
conflict_path=""

step() {
  printf '==> %s\n' "$1"
}

warn() {
  printf 'WARNING: %s\n' "$1" >&2
}

fail() {
  printf '%s\n' "$1" >&2
  exit 1
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

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --release)
        if [ "$#" -lt 2 ]; then
          fail "--release requires a value."
        fi
        RELEASE="$2"
        shift
        ;;
      --help | -h)
        cat <<EOF
Usage: install.sh [VERSION] [--release VERSION]
EOF
        exit 0
        ;;
      --*)
        fail "Unknown argument: $1"
        ;;
      *)
        if [ "$RELEASE" != "latest" ]; then
          fail "Version was already specified; use either a positional VERSION or --release VERSION."
        fi
        RELEASE="$1"
        ;;
    esac
    shift
  done
}

cleanup() {
  release_install_lock
  if [ -n "$STAGING_DIR" ] && [ -d "$STAGING_DIR" ]; then
    rm -rf "$STAGING_DIR"
  fi
  if [ -n "$TMP_ROOT" ] && [ -d "$TMP_ROOT" ]; then
    rm -rf "$TMP_ROOT"
  fi
}

restore_replaced_version_dir() {
  if [ "$REPLACE_ACTIVE" = "1" ] && [ -n "$REPLACE_BACKUP_DIR" ] && [ -d "$REPLACE_BACKUP_DIR" ]; then
    set +e
    if [ -n "$REPLACE_TARGET_DIR" ]; then
      rm -rf "$REPLACE_TARGET_DIR"
      mv "$REPLACE_BACKUP_DIR" "$REPLACE_TARGET_DIR"
    fi
  fi
}

on_exit() {
  status="$?"
  trap - EXIT INT TERM
  if [ "$status" -ne 0 ]; then
    restore_replaced_version_dir
  fi
  cleanup
  exit "$status"
}

on_signal() {
  status="$1"
  trap - EXIT INT TERM
  restore_replaced_version_dir
  cleanup
  exit "$status"
}

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

validate_version() {
  version="$1"

  if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+[.][0-9]+[.][0-9]+(-((alpha|beta)[.][0-9]+))?$'; then
    fail "Invalid version: $version"
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

  case "$os/$arch" in
    darwin/arm64)
      PLATFORM_LABEL="macOS (Apple Silicon)"
      ;;
    darwin/x64)
      PLATFORM_LABEL="macOS (Intel)"
      ;;
    linux/arm64)
      PLATFORM_LABEL="Linux (ARM64)"
      ;;
    linux/x64)
      PLATFORM_LABEL="Linux (x64)"
      ;;
  esac
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

resolve_version() {
  if [ -z "$RELEASE" ] || [ "$RELEASE" = "latest" ]; then
    resolve_latest_version
    return
  fi

  normalized_version="$(normalize_version "$RELEASE")"
  validate_version "$normalized_version"
  printf '%s\n' "$normalized_version"
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

file_sha256() {
  file="$1"

  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{ print $1 }'
    return
  fi

  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{ print $1 }'
    return
  fi

  if command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$file" | sed 's/^.*= //'
    return
  fi

  fail "sha256sum, shasum, or openssl is required to install mcodex."
}

timestamp() {
  date -u '+%Y-%m-%dT%H:%M:%SZ'
}

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

shell_quote() {
  printf '%s\n' "$1" | sed "s/'/'\\\\''/g; 1s/^/'/; \$s/\$/'/"
}

write_completion_marker() {
  directory="$1"
  version="$2"
  archive_name="$3"
  sha256="$4"
  installed_at="$5"
  marker_path="$directory/.mcodex-install-complete.json"
  marker_version="$(json_escape "$version")"
  marker_archive_name="$(json_escape "$archive_name")"
  marker_sha256="$(json_escape "$sha256")"
  marker_installed_at="$(json_escape "$installed_at")"

  cat >"$marker_path" <<EOF
{
  "version": "$marker_version",
  "archiveName": "$marker_archive_name",
  "sha256": "$marker_sha256",
  "installedAt": "$marker_installed_at"
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
  actual_sha="$(file_sha256 "$archive_path")"

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
    REPLACE_TARGET_DIR="$version_dir"
    REPLACE_BACKUP_DIR="$VERSIONS_DIR/.replace.$$.old"
    rm -rf "$REPLACE_BACKUP_DIR"
    REPLACE_ACTIVE=1
    mv "$version_dir" "$REPLACE_BACKUP_DIR"
    if [ "${MCODEX_TEST_FAIL_AFTER_BACKUP:-}" = "1" ]; then
      fail "Test failure after backing up existing version directory."
    fi
    mv "$STAGING_DIR" "$version_dir"
    rm -rf "$REPLACE_BACKUP_DIR"
    REPLACE_ACTIVE=0
    REPLACE_BACKUP_DIR=""
    REPLACE_TARGET_DIR=""
  else
    mv "$STAGING_DIR" "$version_dir"
  fi

  STAGING_DIR=""
}

switch_current_link() {
  target_dir="$1"
  tmp_link="$BASE_ROOT/.current.$$.tmp"

  if [ -e "$CURRENT_LINK" ] && [ ! -L "$CURRENT_LINK" ]; then
    fail "$CURRENT_LINK exists and is not a symlink. Move it aside and rerun the installer."
  fi

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
  base_root_literal="$(shell_quote "$BASE_ROOT")"

  cat >"$wrapper_tmp" <<EOF
#!/bin/sh
set -eu
if [ -n "\${MCODEX_INSTALL_ROOT:-}" ]; then
  base_root="\$MCODEX_INSTALL_ROOT"
else
  base_root=$base_root_literal
fi
target="\$base_root/current/bin/mcodex"
if [ ! -x "\$target" ]; then
  echo "mcodex installation missing or corrupted; rerun the installer." >&2
  exit 1
fi
export MCODEX_INSTALL_MANAGED=1
export MCODEX_INSTALL_METHOD=script
export MCODEX_INSTALL_ROOT="\$base_root"
export PATH="\$base_root/current/bin:\$PATH"
exec "\$target" "\$@"
EOF

  chmod 0755 "$wrapper_tmp"
  mv -f "$wrapper_tmp" "$WRAPPER_PATH"
}

write_metadata() {
  version="$1"
  installed_at="$2"
  metadata_tmp="$BASE_ROOT/.install.json.tmp"
  metadata_version="$(json_escape "$version")"
  metadata_installed_at="$(json_escape "$installed_at")"
  metadata_base_root="$(json_escape "$BASE_ROOT")"
  metadata_versions_dir="$(json_escape "$VERSIONS_DIR")"
  metadata_current_link="$(json_escape "$CURRENT_LINK")"
  metadata_wrapper_path="$(json_escape "$WRAPPER_PATH")"

  cat >"$metadata_tmp" <<EOF
{
  "product": "mcodex",
  "installMethod": "script",
  "currentVersion": "$metadata_version",
  "installedAt": "$metadata_installed_at",
  "baseRoot": "$metadata_base_root",
  "versionsDir": "$metadata_versions_dir",
  "currentLink": "$metadata_current_link",
  "wrapperPath": "$metadata_wrapper_path"
}
EOF

  mv -f "$metadata_tmp" "$METADATA_FILE"
}

pick_profile() {
  case "$os:${SHELL:-}" in
    darwin:*/zsh)
      printf '%s\n' "$HOME/.zprofile"
      ;;
    darwin:*/bash)
      printf '%s\n' "$HOME/.bash_profile"
      ;;
    linux:*/zsh)
      printf '%s\n' "$HOME/.zshrc"
      ;;
    linux:*/bash)
      printf '%s\n' "$HOME/.bashrc"
      ;;
    *)
      printf '%s\n' "$HOME/.profile"
      ;;
  esac
}

append_path_block() {
  profile="$1"
  begin_marker="$2"
  end_marker="$3"
  path_line="$4"

  {
    printf '\n%s\n' "$begin_marker"
    printf '%s\n' "$path_line"
    printf '%s\n' "$end_marker"
  } >>"$profile"
}

rewrite_path_block() {
  profile="$1"
  begin_marker="$2"
  end_marker="$3"
  path_line="$4"
  tmp_profile="$TMP_ROOT/profile.$$.tmp"

  awk -v begin="$begin_marker" -v end="$end_marker" -v line="$path_line" '
    BEGIN {
      in_block = 0
      replaced = 0
    }
    $0 == begin {
      if (!replaced) {
        print begin
        print line
        print end
        replaced = 1
      }
      in_block = 1
      next
    }
    in_block {
      if ($0 == end) {
        in_block = 0
      }
      next
    }
    {
      print
    }
    END {
      if (in_block != 0) {
        exit 1
      }
    }
  ' "$profile" >"$tmp_profile"
  mv "$tmp_profile" "$profile"
}

add_to_path() {
  path_action="already"
  path_profile=""

  case ":$PATH:" in
    *":$WRAPPER_DIR:"*)
      return
      ;;
  esac

  profile="$(pick_profile)"
  path_profile="$profile"
  begin_marker="# >>> mcodex installer >>>"
  end_marker="# <<< mcodex installer <<<"
  # shellcheck disable=SC2016
  wrapper_dir_escaped="$(printf '%s' "$WRAPPER_DIR" | sed 's/\\/\\\\/g; s/"/\\"/g; s/\$/\\$/g; s/`/\\`/g')"
  path_line="export PATH=\"$wrapper_dir_escaped:\$PATH\""

  if [ -f "$profile" ] && grep -F "$begin_marker" "$profile" >/dev/null 2>&1; then
    if grep -F "$path_line" "$profile" >/dev/null 2>&1; then
      path_action="configured"
      return
    fi

    if grep -F "$end_marker" "$profile" >/dev/null 2>&1; then
      rewrite_path_block "$profile" "$begin_marker" "$end_marker" "$path_line"
      path_action="updated"
      return
    fi
  fi

  append_path_block "$profile" "$begin_marker" "$end_marker" "$path_line"
  path_action="added"
}

mkdir_lock_is_stale() {
  [ -d "$LOCK_DIR" ] || return 1

  pid="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
  started_at="$(cat "$LOCK_DIR/started_at" 2>/dev/null || true)"
  now="$(date +%s 2>/dev/null || printf '0')"

  case "$started_at" in
    '' | *[!0-9]*)
      started_at=0
      ;;
  esac

  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    return 1
  fi

  if [ "$started_at" -eq 0 ] || [ "$now" -eq 0 ]; then
    return 0
  fi

  [ $((now - started_at)) -ge "$LOCK_STALE_AFTER_SECS" ]
}

acquire_install_lock() {
  mkdir -p "$BASE_ROOT"

  if [ "$HOST_OS" = "darwin" ] && command -v lockf >/dev/null 2>&1; then
    : >>"$LOCK_FILE"
    exec 9<>"$LOCK_FILE"
    lockf 9
    lock_kind="lockf"
    return
  fi

  if command -v flock >/dev/null 2>&1; then
    exec 9>"$LOCK_FILE"
    flock 9
    lock_kind="flock"
    return
  fi

  while ! mkdir "$LOCK_DIR" 2>/dev/null; do
    if mkdir_lock_is_stale; then
      warn "Removing stale installer lock at $LOCK_DIR"
      rm -rf "$LOCK_DIR"
      continue
    fi
    sleep 1
  done

  printf '%s\n' "$$" >"$LOCK_DIR/pid"
  date +%s >"$LOCK_DIR/started_at" 2>/dev/null || true
  lock_kind="mkdir"
}

release_install_lock() {
  if [ "$lock_kind" = "mkdir" ]; then
    rm -rf "$LOCK_DIR" 2>/dev/null || true
  elif [ "$lock_kind" = "flock" ] || [ "$lock_kind" = "lockf" ]; then
    exec 9>&- 2>/dev/null || true
  fi
  lock_kind=""
}

remove_glob_matches() {
  directory="$1"
  pattern="$2"
  set -- "$directory"/$pattern
  if [ ! -e "$1" ]; then
    return 0
  fi
  rm -rf "$@"
}

cleanup_stale_install_artifacts() {
  mkdir -p "$VERSIONS_DIR" "$BASE_ROOT"

  remove_glob_matches "$VERSIONS_DIR" '.staging.*'
  remove_glob_matches "$VERSIONS_DIR" '.replace.*.old'
  remove_glob_matches "$BASE_ROOT" '.current.*'

  if [ -d "$WRAPPER_DIR" ]; then
    set -- "$WRAPPER_DIR"/.mcodex-wrapper.*
    if [ -e "$1" ]; then
      rm -f "$@"
    fi
  fi
}

version_from_binary() {
  binary_path="$1"

  if [ ! -x "$binary_path" ]; then
    return 1
  fi

  "$binary_path" --version 2>/dev/null | sed -n 's/.* \([0-9][0-9A-Za-z.+-]*\)$/\1/p' | head -n 1
}

current_installed_version() {
  version="$(version_from_binary "$CURRENT_LINK/bin/mcodex" || true)"
  if [ -n "$version" ]; then
    printf '%s\n' "$version"
  fi
}

resolve_existing_mcodex() {
  command -v mcodex 2>/dev/null || true
}

detect_conflicting_install() {
  existing_path="$(resolve_existing_mcodex)"

  case "$existing_path" in
    '' | "$WRAPPER_PATH" | "$CURRENT_LINK/bin/mcodex")
      return
      ;;
  esac

  conflict_path="$existing_path"
  step "Detected existing mcodex command at $existing_path"
  warn "Multiple mcodex installs can be ambiguous because PATH order decides which one runs."
}

handle_conflicting_install() {
  if [ -n "$conflict_path" ]; then
    warn "Leaving the existing mcodex command installed at $conflict_path."
  fi
}

print_launch_instructions() {
  case "$path_action" in
    added)
      step "Current terminal: export PATH=\"$WRAPPER_DIR:\$PATH\" && mcodex"
      step "Future terminals: open a new terminal and run: mcodex"
      step "PATH was added to $path_profile"
      ;;
    updated)
      step "Current terminal: export PATH=\"$WRAPPER_DIR:\$PATH\" && mcodex"
      step "Future terminals: open a new terminal and run: mcodex"
      step "PATH was updated in $path_profile"
      ;;
    configured)
      step "Current terminal: export PATH=\"$WRAPPER_DIR:\$PATH\" && mcodex"
      step "Future terminals: open a new terminal and run: mcodex"
      step "PATH is already configured in $path_profile"
      ;;
    *)
      step "Current terminal: mcodex"
      step "Future terminals: open a new terminal and run: mcodex"
      ;;
  esac
}

verify_visible_command() {
  if ! "$WRAPPER_PATH" --version >/dev/null 2>&1; then
    fail "Installed mcodex command failed verification: $WRAPPER_PATH --version"
  fi
}

parse_args "$@"

trap on_exit EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

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
RESOLVED_VERSION="$(resolve_version)"
current_version="$(current_installed_version)"

if [ -n "$current_version" ] && [ "$current_version" != "$RESOLVED_VERSION" ]; then
  step "Updating mcodex CLI from $current_version to $RESOLVED_VERSION"
elif [ -n "$current_version" ]; then
  step "Updating mcodex CLI"
else
  step "Installing mcodex CLI"
fi
step "Detected platform: $PLATFORM_LABEL"
step "Resolved version: $RESOLVED_VERSION"

detect_conflicting_install
acquire_install_lock
cleanup_stale_install_artifacts

CHECKSUMS_FILE="$(download_checksums "$RESOLVED_VERSION")"
EXPECTED_SHA="$(expected_sha_for_archive "$CHECKSUMS_FILE" "$ARCHIVE_NAME")"

if [ -z "$EXPECTED_SHA" ]; then
  fail "No checksum entry found for $ARCHIVE_NAME."
fi

VERSION_DIR="$VERSIONS_DIR/$RESOLVED_VERSION"

if ! version_dir_complete "$VERSION_DIR" "$RESOLVED_VERSION" "$ARCHIVE_NAME" "$EXPECTED_SHA"; then
  if [ -e "$VERSION_DIR" ] || [ -L "$VERSION_DIR" ]; then
    warn "Found incomplete existing release at $VERSION_DIR; reinstalling."
  fi
  stage_version_dir "$RESOLVED_VERSION" "$ARCHIVE_NAME" "$EXPECTED_SHA"
  publish_version_dir "$VERSION_DIR"
fi

switch_current_link "$VERSION_DIR"
write_wrapper
write_metadata "$RESOLVED_VERSION" "$(timestamp)"
add_to_path
verify_visible_command
release_install_lock
handle_conflicting_install

case "$path_action" in
  added | updated | configured)
    print_launch_instructions
    ;;
  *)
    step "$WRAPPER_DIR is already on PATH"
    print_launch_instructions
    ;;
esac

printf 'mcodex CLI %s installed successfully.\n' "$RESOLVED_VERSION"
