# mcodex CLI Without npm Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace npm-based `mcodex` CLI distribution with OSS-hosted native archives, script-managed installs, and Rust/TUI update prompts.

**Architecture:** Keep the change at distribution edges: release workflow builds native archives and uploads them to OSS, install scripts own disk mutation and PATH setup, wrappers inject script-managed metadata, and Rust/TUI reads the OSS stable manifest for update prompts. GitHub Releases remain lightweight records with notes and checksum artifacts; npm stays only for non-CLI package surfaces that still require it.

**Tech Stack:** POSIX shell, PowerShell, Python standard library tests, GitHub Actions, Aliyun OSS via `ossutil`, Rust `codex-product-identity`, Rust `codex-tui`, Rust `codex-cli`, `insta` snapshots.

---

## Source Spec

Spec: `docs/superpowers/specs/2026-04-20-mcodex-cli-without-npm-design.md`

Implementation constants:

```text
OSS_BASE_URL=https://downloads.mcodex.sota.wiki
OSS_RELEASE_PREFIX=repositories/mcodex
OSS_LATEST_MANIFEST=https://downloads.mcodex.sota.wiki/repositories/mcodex/channels/stable/latest.json
UNIX_INSTALL_COMMAND=curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh
WINDOWS_INSTALL_COMMAND=powershell -NoProfile -ExecutionPolicy Bypass -Command "iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\mcodex-install.ps1; & $env:TEMP\mcodex-install.ps1"
WINDOWS_INSTALL_RUNNER_COMMAND=iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\mcodex-install.ps1; & $env:TEMP\mcodex-install.ps1
```

Aliyun OSS workflow commands should use `ossutil` v2 on the Ubuntu release runner:

```bash
curl -fsSL -o "$RUNNER_TEMP/ossutil.zip" \
  https://gosspublic.alicdn.com/ossutil/v2/2.2.2/ossutil-2.2.2-linux-amd64.zip
unzip -q "$RUNNER_TEMP/ossutil.zip" -d "$RUNNER_TEMP/ossutil"
OSSUTIL="$RUNNER_TEMP/ossutil/ossutil-2.2.2-linux-amd64/ossutil"
```

This matches the official Alibaba Cloud OSS ossutil 2.0 Linux x86_64 installation pattern.

## File Structure

- Modify: `codex-rs/product-identity/src/lib.rs`
  Responsibility: product-level URLs plus display/runner install commands used by runtime, docs, and tests.
- Modify: `codex-rs/tui/src/update_action.rs`
  Responsibility: script-managed install detection and update command rendering/execution metadata.
- Modify: `codex-rs/tui/src/updates.rs`
  Responsibility: cached update checks from OSS `latest.json`.
- Modify: `codex-rs/tui/src/update_prompt.rs`
  Responsibility: update modal command display, release-notes link rendering, and snapshot coverage.
- Modify: `codex-rs/tui/src/history_cell.rs`
  Responsibility: startup update history cell text and release-notes link when an update action is available.
- Modify: `codex-rs/tui/src/app.rs`
  Responsibility: thread cached update version + notes URL into the startup history banner path.
- Modify: `codex-rs/cli/src/main.rs`
  Responsibility: run the selected script update command after TUI exits.
- Modify: `scripts/install/install.sh`
  Responsibility: macOS/Linux latest and explicit-version install/update from OSS native archives.
- Modify: `scripts/install/install.ps1`
  Responsibility: Windows PowerShell latest and explicit-version install/update from OSS native zip archives.
- Create: `scripts/install/test_install_scripts.py`
  Responsibility: Python `unittest` integration tests for installer version parsing, local OSS fixture installs, wrapper env injection, checksum validation, and repair semantics.
- Create: `scripts/stage_cli_archives.py`
  Responsibility: assemble native platform archives with the exact `bin/` layout, generate `latest.json`, generate `SHA256SUMS`, and validate archive contents before upload.
- Create: `scripts/test_stage_cli_archives.py`
  Responsibility: Python `unittest` coverage for archive names, archive layouts, checksum file generation, and latest manifest shape.
- Create: `scripts/test_stage_npm_packages.py`
  Responsibility: Python `unittest` coverage for the CLI npm publish guard while preserving SDK/proxy npm staging.
- Modify: `.github/workflows/rust-release.yml`
  Responsibility: stop CLI npm staging/publishing, stage native CLI archives, sign checksums, upload to OSS, update `latest.json` last, and attach lightweight GitHub Release assets.
- Modify: `.github/workflows/rust-release-windows.yml`
  Responsibility: stage `mcodex.exe` artifacts and helpers for native Windows CLI archives.
- Modify: `.github/actions/linux-code-sign/action.yml`
  Responsibility: sign `mcodex` release binaries instead of the removed CLI `codex` binary.
- Modify: `.github/actions/macos-code-sign/action.yml`
  Responsibility: sign/notarize `mcodex` release binaries and stop relying on `codex` DMG naming.
- Modify: `.github/actions/windows-code-sign/action.yml`
  Responsibility: sign `mcodex.exe` release binaries instead of `codex.exe`.
- Modify: `codex-cli/scripts/install_native_deps.py`
  Responsibility: provide an `rg`-only vendor install path that does not download GitHub Actions artifacts and can parse the checked-in `rg` manifest without requiring DotSlash.
- Create: `release/minisign.pub`
  Responsibility: committed public key for auditing `SHA256SUMS.sig`.
- Modify: `scripts/stage_npm_packages.py`
  Responsibility: reject CLI npm package staging while preserving non-CLI npm packages.
- Modify: `codex-cli/scripts/build_npm_package.py`
  Responsibility: preserve SDK/proxy npm staging without injecting or building CLI npm packages.
- Modify: `sdk/typescript/src/exec.ts`
  Responsibility: default SDK CLI discovery should prefer explicit override/PATH `mcodex` instead of requiring a removed CLI npm package.
- Modify: SDK tests under `sdk/typescript/tests/`
  Responsibility: cover SDK CLI discovery behavior after CLI npm cutover.
- Modify: `README.md`
  Responsibility: top-level install instructions use OSS scripts and explain lightweight GitHub Releases.
- Modify: `docs/install.md`
  Responsibility: install/build docs describe script-managed installs, explicit versions, and npm cutover.
- Modify: `codex-cli/scripts/README.md`
  Responsibility: clarify that npm release helpers no longer stage the CLI package.

## Task 1: Product Identity and Update Command Contract

**Files:**
- Modify: `codex-rs/product-identity/src/lib.rs`
- Modify: `codex-rs/tui/src/update_action.rs`
- Modify: `codex-rs/cli/src/main.rs`

- [ ] **Step 1: Add failing product identity assertions**

In `codex-rs/product-identity/src/lib.rs`, extend `mcodex_identity_defines_active_and_legacy_roots` with assertions for the OSS URL and installer commands before adding fields:

```rust
assert_eq!(MCODEX.download_base_url, "https://downloads.mcodex.sota.wiki");
assert_eq!(
    MCODEX.stable_latest_manifest_url,
    "https://downloads.mcodex.sota.wiki/repositories/mcodex/channels/stable/latest.json"
);
assert_eq!(
    MCODEX.unix_install_command,
    "curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh"
);
assert!(MCODEX.windows_install_command.contains("install.ps1"));
assert!(MCODEX.windows_install_command.contains("$env:TEMP\\mcodex-install.ps1"));
assert_eq!(
    MCODEX.windows_install_runner_command,
    "iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\\mcodex-install.ps1; & $env:TEMP\\mcodex-install.ps1"
);
assert!(!MCODEX.windows_install_runner_command.contains("powershell"));
```

- [ ] **Step 2: Run the failing product identity test**

Run:

```bash
cd codex-rs
cargo test -p codex-product-identity mcodex_identity_defines_active_and_legacy_roots
```

Expected: FAIL because the new `ProductIdentity` fields do not exist.

- [ ] **Step 3: Add product-level distribution fields**

Modify `ProductIdentity` in `codex-rs/product-identity/src/lib.rs`:

```rust
pub download_base_url: &'static str,
pub stable_latest_manifest_url: &'static str,
pub unix_install_command: &'static str,
pub windows_install_command: &'static str,
pub windows_install_runner_command: &'static str,
```

Set `MCODEX` values to the constants from the plan header.

- [ ] **Step 4: Run the product identity test**

Run:

```bash
cd codex-rs
cargo test -p codex-product-identity mcodex_identity_defines_active_and_legacy_roots
```

Expected: PASS.

- [ ] **Step 5: Write failing update action tests**

Replace `detects_update_action_without_env_mutation` and `update_commands_use_mcodex_identity` expectations in `codex-rs/tui/src/update_action.rs` so the contract is script-only:

```rust
#[test]
fn detects_script_managed_update_action_only() {
    assert_eq!(
        detect_update_action(/*managed*/ false, /*method*/ None),
        None
    );
    assert_eq!(
        detect_update_action(/*managed*/ true, /*method*/ Some("npm")),
        None
    );
    assert_eq!(
        detect_update_action(/*managed*/ true, /*method*/ Some("script")),
        Some(UpdateAction::ScriptManagedLatest)
    );
}

#[test]
fn script_update_commands_use_oss_installers() {
    assert_eq!(
        UpdateAction::ScriptManagedLatest.display_command_for_platform(UpdatePlatform::Unix),
        MCODEX.unix_install_command
    );
    assert_eq!(
        UpdateAction::ScriptManagedLatest.display_command_for_platform(UpdatePlatform::Windows),
        MCODEX.windows_install_command
    );
}

#[test]
fn script_update_runner_uses_single_powershell_invocation() {
    assert_eq!(
        UpdateAction::ScriptManagedLatest.shell_invocation_for_platform(UpdatePlatform::Windows),
        (
            "powershell",
            &[
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                MCODEX.windows_install_runner_command,
            ],
        )
    );
}
```

If exposing `UpdatePlatform` is too much public surface, keep it `pub(crate)` and test inside the module.

- [ ] **Step 6: Run the failing update action tests**

Run:

```bash
cd codex-rs
cargo test -p codex-tui update_action -- --nocapture
```

Expected: FAIL because `ScriptManagedLatest`, `UpdatePlatform`, and the new detection signature do not exist.

- [ ] **Step 7: Implement script-managed update actions**

In `codex-rs/tui/src/update_action.rs`:

- Scope guard: `UpdateAction` is local to the `mcodex` TUI/CLI path in this fork. Do not broaden this task into SDK or non-CLI packaging behavior; those changes belong only in the later release and SDK tasks.
- Replace `NpmGlobalLatest`, `BunGlobalLatest`, and `BrewUpgrade` with `ScriptManagedLatest`.
- Replace package-manager detection with `MCODEX_INSTALL_MANAGED=1` plus `MCODEX_INSTALL_METHOD=script`.
- Keep tests free of process environment mutation by passing booleans/strings into a private detection helper.
- Treat `command_str()` / `display_command_for_platform()` as user-facing display strings only.
- Add `MCODEX.windows_install_runner_command` so the CLI runner can execute a single PowerShell invocation without nesting the full display command inside another `powershell -Command`.
- Add a public command invocation method for the CLI runner plus a platform-specific helper for tests:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    ScriptManagedLatest,
}

#[cfg_attr(test, derive(Debug, Clone, Copy, PartialEq, Eq))]
pub(crate) enum UpdatePlatform {
    Unix,
    Windows,
}

impl UpdateAction {
    pub fn command_str(self) -> String {
        self.display_command_for_platform(UpdatePlatform::current()).to_string()
    }

    pub fn shell_invocation(self) -> (&'static str, &'static [&'static str]) {
        self.shell_invocation_for_platform(UpdatePlatform::current())
    }

    pub(crate) fn display_command_for_platform(self, platform: UpdatePlatform) -> &'static str {
        match (self, platform) {
            (UpdateAction::ScriptManagedLatest, UpdatePlatform::Unix) => MCODEX.unix_install_command,
            (UpdateAction::ScriptManagedLatest, UpdatePlatform::Windows) => {
                MCODEX.windows_install_command
            }
        }
    }

    pub(crate) fn shell_invocation_for_platform(
        self,
        platform: UpdatePlatform,
    ) -> (&'static str, &'static [&'static str]) {
        match platform {
            UpdatePlatform::Unix => ("sh", &["-c", MCODEX.unix_install_command]),
            UpdatePlatform::Windows => (
                "powershell",
                &[
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    MCODEX.windows_install_runner_command,
                ],
            ),
        }
    }
}
```

Adjust the exact method names if borrowing rules require a static slice constant.

- [ ] **Step 8: Update CLI update runner**

Modify `run_update_action` in `codex-rs/cli/src/main.rs` to call the new public `UpdateAction::shell_invocation()` method instead of `command_args()`.

The Unix branch should no longer split the pipeline command into `curl` arguments. It should run `sh -c <display command>`. The Windows branch should run PowerShell directly rather than `cmd /C`, because the selected command is PowerShell syntax.
Do not pass the full display string `powershell ... -Command ...` as the `-Command` payload of another PowerShell process.

- [ ] **Step 9: Run focused Rust tests**

Run:

```bash
cd codex-rs
cargo test -p codex-product-identity mcodex_identity_defines_active_and_legacy_roots
cargo test -p codex-tui update_action -- --nocapture
```

Expected: PASS.

- [ ] **Step 10: Commit Task 1**

```bash
git add codex-rs/product-identity/src/lib.rs codex-rs/tui/src/update_action.rs codex-rs/cli/src/main.rs
git commit -m "feat: add script-managed update action contract"
```

## Task 2: OSS Manifest Update Checks and TUI Text

**Files:**
- Modify: `codex-rs/tui/src/updates.rs`
- Modify: `codex-rs/tui/src/update_prompt.rs`
- Modify: `codex-rs/tui/src/history_cell.rs`
- Modify: `codex-rs/tui/src/app.rs`
- Test: `codex-rs/tui/src/snapshots/codex_tui__update_prompt__tests__update_prompt_modal.snap`

- [ ] **Step 1: Write failing OSS manifest parser tests**

In `codex-rs/tui/src/updates.rs`, replace GitHub tag parser tests with OSS manifest tests:

```rust
#[test]
fn parses_latest_manifest_version() {
    let manifest_json = r#"{
        "product": "mcodex",
        "channel": "stable",
        "version": "0.96.0",
        "publishedAt": "2026-04-20T12:00:00Z",
        "notesUrl": "https://github.com/vivym/mcodex/releases/tag/rust-v0.96.0",
        "checksumsUrl": "https://downloads.mcodex.sota.wiki/repositories/mcodex/releases/0.96.0/SHA256SUMS",
        "install": {
            "unix": "curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh",
            "windows": "powershell -NoProfile -ExecutionPolicy Bypass -Command \"iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\\\\mcodex-install.ps1; & $env:TEMP\\\\mcodex-install.ps1\""
        }
    }"#;
    let manifest: LatestManifest = serde_json::from_str(manifest_json).expect("manifest");
    assert_eq!(manifest.version, "0.96.0");
    assert_eq!(
        manifest.notes_url,
        "https://github.com/vivym/mcodex/releases/tag/rust-v0.96.0"
    );
}
```

- [ ] **Step 2: Run the failing update tests**

Run:

```bash
cd codex-rs
cargo test -p codex-tui --release updates -- --nocapture
```

Expected: FAIL because `LatestManifest` does not exist and old GitHub tag parsing is still present.

- [ ] **Step 3: Implement OSS manifest fetch**

In `codex-rs/tui/src/updates.rs`:

- Remove `ReleaseInfo`, `HomebrewCaskInfo`, `homebrew_cask_api_url()`, and `extract_version_from_latest_tag()`.
- Add `LatestManifest { version: String, notes_url: String }` with `#[serde(rename_all = "camelCase")]`.
- Make `check_for_update()` fetch `MCODEX.stable_latest_manifest_url`.
- Extend cached `VersionInfo` so it keeps both `latest_version` and `latest_notes_url`.
- Store `latest_version = manifest.version` and `latest_notes_url = Some(manifest.notes_url)`.
- Keep `latest_notes_url` optional with `#[serde(default)]` so existing `version.json` cache files continue to parse after the schema change.
- Keep the existing cache file, dismissal behavior, and semver comparison.
- Add an internal handoff type such as `CachedUpdateInfo { latest_version: String, latest_notes_url: Option<String> }` so callers do not need to reopen `version.json` themselves.
- Update the `get_upgrade_version*` helpers to return that handoff type (or an equivalent `(version, notes_url)` bundle) all the way out to callers.
- Update `codex-rs/tui/src/app.rs` so the startup `UpdateAvailableHistoryCell::new(...)` call receives both `latest_version` and `latest_notes_url`.
- Update `update_prompt.rs` and `history_cell.rs` to prefer the cached `latest_notes_url` over the static product default when rendering the release-notes link.

- [ ] **Step 4: Update update prompt test fixture**

In `codex-rs/tui/src/update_prompt.rs`, change `new_prompt()` to use:

```rust
UpdateAction::ScriptManagedLatest
```

- [ ] **Step 5: Add history cell coverage for script command**

Add a `#[cfg(test)] mod update_available_tests` at the bottom of `codex-rs/tui/src/history_cell.rs`:

```rust
#[test]
fn update_available_history_cell_mentions_script_update() {
    let cell = UpdateAvailableHistoryCell::new(
        "9.9.9".to_string(),
        Some("https://github.com/vivym/mcodex/releases/tag/rust-v9.9.9".to_string()),
        Some(UpdateAction::ScriptManagedLatest),
    );
    let text = cell
        .display_lines(/*width*/ 120)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("downloads.mcodex.sota.wiki/install.sh"));
    assert!(!text.contains("npm"));
    assert!(!text.contains("bun"));
    assert!(!text.contains("brew"));
}
```

- [ ] **Step 6: Run focused TUI tests and generate snapshot**

Run:

```bash
cd codex-rs
cargo test -p codex-tui --release updates -- --nocapture
cargo test -p codex-tui --release update_prompt -- --nocapture
cargo test -p codex-tui update_available_history_cell -- --nocapture
cargo insta pending-snapshots -p codex-tui
```

Expected: tests pass except `update_prompt_modal.snap.new` may be pending because the command changed. The `updates` and `update_prompt` commands use `--release` because those modules are compiled behind `not(debug_assertions)`.

- [ ] **Step 7: Review and accept the update prompt snapshot**

Run:

```bash
cd codex-rs
cargo insta show -p codex-tui tui/src/snapshots/codex_tui__update_prompt__tests__update_prompt_modal.snap.new
cargo insta accept -p codex-tui
```

Expected: snapshot shows the OSS script command, not npm/bun/Homebrew.

- [ ] **Step 8: Commit Task 2**

```bash
git add codex-rs/tui/src/updates.rs codex-rs/tui/src/update_prompt.rs codex-rs/tui/src/history_cell.rs codex-rs/tui/src/app.rs codex-rs/tui/src/snapshots
git commit -m "feat: check mcodex updates from OSS manifest"
```

## Task 3: Native CLI Archive Staging

**Files:**
- Create: `scripts/stage_cli_archives.py`
- Create: `scripts/test_stage_cli_archives.py`
- Create: `codex-cli/scripts/test_install_native_deps.py`
- Modify: `.github/workflows/rust-release.yml`
- Modify: `.github/workflows/rust-release-windows.yml`
- Modify: `.github/actions/linux-code-sign/action.yml`
- Modify: `.github/actions/macos-code-sign/action.yml`
- Modify: `.github/actions/windows-code-sign/action.yml`
- Modify: `codex-cli/scripts/install_native_deps.py`

- [ ] **Step 1: Write failing archive staging tests**

Create `scripts/test_stage_cli_archives.py` using Python `unittest`. Start with tests that call pure functions, not GitHub Actions:

```python
import json
import tarfile
import tempfile
import unittest
from pathlib import Path

import scripts.stage_cli_archives as stage


class StageCliArchivesTests(unittest.TestCase):
    def test_archive_names_match_distribution_contract(self) -> None:
        self.assertEqual(
            stage.archive_name_for_platform("linux", "x64"),
            "mcodex-linux-x64.tar.gz",
        )
        self.assertEqual(
            stage.archive_name_for_platform("win32", "arm64"),
            "mcodex-win32-arm64.zip",
        )

    def test_latest_manifest_shape(self) -> None:
        manifest = stage.build_latest_manifest(
            version="0.96.0",
            release_tag="rust-v0.96.0",
            published_at="2026-04-20T12:00:00Z",
        )
        self.assertEqual(manifest["product"], "mcodex")
        self.assertEqual(manifest["channel"], "stable")
        self.assertEqual(manifest["version"], "0.96.0")
        self.assertRegex(manifest["publishedAt"], r"^20[0-9]{2}-[0-9]{2}-[0-9]{2}T")
        self.assertEqual(
            manifest["notesUrl"],
            "https://github.com/vivym/mcodex/releases/tag/rust-v0.96.0",
        )
        self.assertEqual(
            manifest["checksumsUrl"],
            "https://downloads.mcodex.sota.wiki/repositories/mcodex/releases/0.96.0/SHA256SUMS",
        )
        self.assertIn("install.sh", manifest["install"]["unix"])
        self.assertIn("install.ps1", manifest["install"]["windows"])

    def test_unix_archive_contains_bin_layout(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            src = root / "src"
            out = root / "out"
            (src / "bin").mkdir(parents=True)
            (src / "bin" / "mcodex").write_text("#!/bin/sh\n", encoding="utf-8")
            (src / "bin" / "rg").write_text("#!/bin/sh\n", encoding="utf-8")
            archive = stage.create_unix_archive(src, out, "mcodex-linux-x64.tar.gz")
            members = stage.list_archive_members(archive)
            self.assertEqual(members, ["bin/mcodex", "bin/rg"])

    def test_release_artifact_resolver_consumes_downloaded_workflow_archives(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            target_dir = root / "dist" / "x86_64-unknown-linux-musl"
            target_dir.mkdir(parents=True)
            binary = root / "mcodex"
            binary.write_text("#!/bin/sh\n", encoding="utf-8")
            with tarfile.open(
                target_dir / "mcodex-x86_64-unknown-linux-musl.tar.gz",
                "w:gz",
            ) as archive:
                archive.add(binary, arcname="mcodex")
            resolved = stage.resolve_release_binary(
                root / "dist",
                "x86_64-unknown-linux-musl",
                "mcodex",
            )
            self.assertEqual(resolved.read_text(encoding="utf-8"), "#!/bin/sh\n")


if __name__ == "__main__":
    unittest.main()
```

Create `codex-cli/scripts/test_install_native_deps.py` with a focused `rg`-only test:

```python
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import install_native_deps


class InstallNativeDepsTests(unittest.TestCase):
    def test_load_manifest_parses_checked_in_shebang_without_dotslash(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            manifest_path = Path(tmp) / "rg"
            manifest_path.write_text(
                "#!/usr/bin/env dotslash\n"
                + json.dumps({"platforms": {"linux-x86_64": {"providers": []}}})
                + "\n",
                encoding="utf-8",
            )
            with mock.patch("subprocess.check_output") as check_output:
                manifest = install_native_deps._load_manifest(manifest_path)
            self.assertEqual(
                manifest,
                {"platforms": {"linux-x86_64": {"providers": []}}},
            )
            check_output.assert_not_called()

    def test_rg_only_install_skips_workflow_download_and_dotslash_parse(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            (bin_dir / "rg").write_text(
                "#!/usr/bin/env dotslash\n"
                + json.dumps({"platforms": {}})
                + "\n",
                encoding="utf-8",
            )
            with mock.patch.object(install_native_deps, "RG_MANIFEST", bin_dir / "rg"), \
                 mock.patch.object(install_native_deps, "DEFAULT_RG_TARGETS", []), \
                 mock.patch.object(install_native_deps, "_download_artifacts") as download, \
                 mock.patch("subprocess.check_output") as check_output, \
                 mock.patch("sys.argv", ["install_native_deps.py", "--component", "rg", str(root)]):
                self.assertEqual(install_native_deps.main(), 0)
            download.assert_not_called()
            check_output.assert_not_called()


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the failing archive tests**

Run:

```bash
python3 -m unittest scripts.test_stage_cli_archives
python3 codex-cli/scripts/test_install_native_deps.py
```

Expected: FAIL because `scripts/stage_cli_archives.py` does not exist and `install_native_deps.py` still calls the GitHub Actions download path / DotSlash parser for `--component rg`.

- [ ] **Step 3: Implement archive staging helper**

Create `scripts/stage_cli_archives.py` with:

- `DOWNLOAD_BASE_URL = "https://downloads.mcodex.sota.wiki"`
- `RELEASE_PREFIX = "repositories/mcodex"`
- platform map from Rust targets to archive names:
  - `aarch64-apple-darwin -> darwin-arm64`
  - `x86_64-apple-darwin -> darwin-x64`
  - `aarch64-unknown-linux-musl -> linux-arm64`
  - `x86_64-unknown-linux-musl -> linux-x64`
  - `aarch64-pc-windows-msvc -> win32-arm64`
  - `x86_64-pc-windows-msvc -> win32-x64`
- pure helpers:
  - `archive_name_for_platform(os_name: str, arch: str) -> str`
  - `build_latest_manifest(version: str, release_tag: str, published_at: str) -> dict`
  - `create_unix_archive(src_dir: Path, out_dir: Path, archive_name: str) -> Path`
  - `create_windows_archive(src_dir: Path, out_dir: Path, archive_name: str) -> Path`
  - `write_sha256sums(paths: list[Path], output: Path) -> None`
  - `list_archive_members(path: Path) -> list[str]`
  - `resolve_release_binary(artifacts_dir: Path, target: str, binary_name: str) -> Path`
- Reuse the existing `codex-cli/scripts/install_native_deps.py` vendor layout for ripgrep input. `stage_cli_archives.py` should accept `--vendor-src <dir>` and copy `rg` from `vendor/<target>/path/rg` or `vendor/<target>/path/rg.exe` into the archive as `bin/rg` / `bin/rg.exe`.
- Treat `--artifacts-dir` as the tree produced by `actions/download-artifact` after the current release build jobs: one subdirectory per Rust target under `dist/<target>/`.
- Do not consume already assembled `mcodex-linux-x64.tar.gz` / `mcodex-win32-x64.zip` native archives as inputs. Those are this script's outputs.
- Resolve each required release binary from `dist/<target>/` by preferring a raw executable if present, then the per-binary `.tar.gz` or `.zip` produced by the workflow, and finally `.zst` only when needed. Non-Windows build jobs currently remove raw binaries after creating `.zst`, so `.tar.gz` is the expected portable input for Unix/macOS. Windows jobs keep raw `.exe` files, but the resolver should still handle `.zip`/`.tar.gz` for helper binaries.

The CLI should accept:

```bash
scripts/stage_cli_archives.py \
  --version 0.96.0 \
  --release-tag rust-v0.96.0 \
  --published-at 2026-04-20T12:00:00Z \
  --artifacts-dir dist \
  --vendor-src dist/vendor \
  --output-dir dist/oss/repositories/mcodex/releases/0.96.0 \
  --manifest-output dist/oss/repositories/mcodex/channels/stable/latest.json
```

`--version` must be the normalized semver string, not `v0.96.0` or `rust-v0.96.0`. Allow stable versions plus existing prerelease forms such as `0.96.0-alpha.1` / `0.96.0-beta.1`, because versioned OSS release directories must work for every published tag. `--manifest-output` is stable-only: reject it when `--version` contains a prerelease suffix so `channels/stable/latest.json` cannot drift onto alpha/beta releases.

The script should validate every archive before returning. Validation must assert the exact member lists:

```text
Unix:    bin/mcodex, bin/rg
Windows: bin/mcodex.exe, bin/rg.exe, bin/codex-command-runner.exe, bin/codex-windows-sandbox-setup.exe
```

- [ ] **Step 4: Run archive tests**

Run:

```bash
python3 -m unittest scripts.test_stage_cli_archives
python3 codex-cli/scripts/test_install_native_deps.py
python3 -m py_compile codex-cli/scripts/install_native_deps.py
```

Expected: PASS.

- [ ] **Step 5: Update release build binary names**

In `.github/workflows/rust-release.yml`:

- Build `--bin mcodex` for CLI release artifacts.
- Stage `mcodex-${{ matrix.target }}` instead of `codex-${{ matrix.target }}`.
- Remove the macOS DMG build/sign/stage path that produces `codex-${{ matrix.target }}.dmg`; GitHub Releases no longer carry native CLI installers.
- Update all stage, sigstore copy, compression, comments, and checks that refer to the CLI binary as `codex-*` so the CLI artifact names are consistently `mcodex-*`.
- Keep `codex-responses-api-proxy`; it is a non-CLI npm package and remains published.

In `.github/workflows/rust-release-windows.yml`:

- Build `--bin mcodex` in the primary Windows bundle.
- Stage `mcodex-${{ matrix.target }}.exe`.
- Update archive, verify, compression, comments, and helper bundle conditions that refer to the CLI executable as `codex-${{ matrix.target }}.exe`.
- Keep helper binaries staged for the Windows native archive.

In `.github/actions/linux-code-sign/action.yml`:

- Sign `mcodex` and `codex-responses-api-proxy`; do not sign the removed CLI binary name `codex`.

In `.github/actions/macos-code-sign/action.yml`:

- Sign/notarize `mcodex` and `codex-responses-api-proxy`.
- Remove or update logic that assumes a CLI DMG named `codex-${TARGET}.dmg`; this workflow slice does not publish GitHub Release DMGs for the CLI.

In `.github/actions/windows-code-sign/action.yml`:

- Sign `mcodex.exe`, `codex-responses-api-proxy.exe`, `codex-command-runner.exe`, and `codex-windows-sandbox-setup.exe`.
- Do not sign the removed CLI executable name `codex.exe`.

- [ ] **Step 6: Add release workflow archive staging step**

First update `codex-cli/scripts/install_native_deps.py` so `--component rg` works as a release-runner-only dependency install:

- When the requested component set is exactly `{"rg"}`, skip workflow lookup and `_download_artifacts()` entirely.
- Parse `codex-cli/bin/rg` without invoking DotSlash by reading the file, dropping a leading `#!/usr/bin/env dotslash` line if present, and JSON-decoding the remaining manifest.
- Keep the existing DotSlash path for non-`rg` components so this task does not regress development installs.
- Install ripgrep into the same vendor layout that `stage_cli_archives.py --vendor-src dist/vendor` reads.

In `.github/workflows/rust-release.yml`, after artifact download and cleanup, replace the old `Define release name` step with a transitional `Define release contract` step that still preserves the existing `name` output for the rest of the workflow, then add an `rg` vendor install step plus the archive step:

```yaml
- name: Define release contract
  id: release_contract
  run: |
    set -euo pipefail
    version="${GITHUB_REF_NAME#rust-v}"
    echo "name=${version}" >> "$GITHUB_OUTPUT"
    echo "release_version=${version}" >> "$GITHUB_OUTPUT"
    echo "release_tag=${GITHUB_REF_NAME}" >> "$GITHUB_OUTPUT"
    if [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "stable_release_version=${version}" >> "$GITHUB_OUTPUT"
    else
      echo "stable_release_version=" >> "$GITHUB_OUTPUT"
    fi
    if [[ "$version" == *-* ]]; then
      echo "release_is_prerelease=true" >> "$GITHUB_OUTPUT"
    else
      echo "release_is_prerelease=false" >> "$GITHUB_OUTPUT"
    fi
```

```yaml
- name: Install release rg payloads
  run: |
    set -euo pipefail
    python3 codex-cli/scripts/install_native_deps.py --component rg dist
```

Task 6 will finish this transition by renaming the job outputs to `release_version` / `release_tag` / `release_is_prerelease` and migrating every downstream consumer. Task 3 only needs the transitional contract so this task is implementable on its own.

Then add:

```yaml
- name: Stage native CLI archives
  env:
    RELEASE_VERSION: ${{ steps.release_contract.outputs.release_version }}
    RELEASE_IS_PRERELEASE: ${{ steps.release_contract.outputs.release_is_prerelease }}
    RELEASE_TAG: ${{ steps.release_contract.outputs.release_tag }}
  run: |
    set -euo pipefail
    if [[ ! "$RELEASE_VERSION" =~ ^[0-9]+[.][0-9]+[.][0-9]+(-((alpha|beta)[.][0-9]+))?$ ]]; then
      echo "Expected normalized release semver, got: $RELEASE_VERSION" >&2
      exit 1
    fi
    cmd=(
      python3 scripts/stage_cli_archives.py
      --version "$RELEASE_VERSION"
      --release-tag "$RELEASE_TAG"
      --published-at "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      --artifacts-dir dist
      --vendor-src dist/vendor
      --output-dir "dist/oss/repositories/mcodex/releases/$RELEASE_VERSION"
    )
    if [[ "$RELEASE_IS_PRERELEASE" != "true" ]]; then
      cmd+=(--manifest-output "dist/oss/repositories/mcodex/channels/stable/latest.json")
    fi
    "${cmd[@]}"
```

Stable tags write `channels/stable/latest.json`; prerelease tags publish only versioned OSS artifacts and do not touch the stable manifest.

- [ ] **Step 7: Commit Task 3**

```bash
git add scripts/stage_cli_archives.py scripts/test_stage_cli_archives.py codex-cli/scripts/install_native_deps.py codex-cli/scripts/test_install_native_deps.py .github/workflows/rust-release.yml .github/workflows/rust-release-windows.yml .github/actions/linux-code-sign/action.yml .github/actions/macos-code-sign/action.yml .github/actions/windows-code-sign/action.yml
git commit -m "feat: stage native mcodex cli archives"
```

## Task 4: macOS/Linux Installer

**Files:**
- Modify: `scripts/install/install.sh`
- Create/modify: `scripts/install/test_install_scripts.py`

- [ ] **Step 1: Write failing installer tests for Unix**

In `scripts/install/test_install_scripts.py`, add a `unittest` that:

- creates a local fake OSS tree under a temp directory
- writes `channels/stable/latest.json`
- writes `releases/0.96.0/mcodex-linux-x64.tar.gz`
- writes `SHA256SUMS`
- runs a local `python3 -m http.server`
- runs `scripts/install/install.sh` with a temp `HOME`
- sets `MCODEX_DOWNLOAD_BASE_URL=http://127.0.0.1:<port>` for testability
- sets `MCODEX_TEST_UNAME_S=Linux` and `MCODEX_TEST_UNAME_M=x86_64` for deterministic platform mapping

The fake `bin/mcodex` should print the wrapper-injected env:

```sh
#!/bin/sh
printf 'managed=%s\n' "$MCODEX_INSTALL_MANAGED"
printf 'method=%s\n' "$MCODEX_INSTALL_METHOD"
printf 'root=%s\n' "$MCODEX_INSTALL_ROOT"
printf 'path=%s\n' "$PATH"
exit 7
```

Assert:

```python
self.assertTrue((home / ".mcodex/install/0.96.0/bin/mcodex").exists())
self.assertTrue((home / ".mcodex/current/bin/mcodex").exists())
self.assertTrue((home / ".local/bin/mcodex").exists())
self.assertTrue((home / ".mcodex/install.json").exists())
metadata = json.loads((home / ".mcodex/install.json").read_text(encoding="utf-8"))
self.assertEqual(
    {
        "product": metadata["product"],
        "installMethod": metadata["installMethod"],
        "currentVersion": metadata["currentVersion"],
        "baseRoot": metadata["baseRoot"],
        "versionsDir": metadata["versionsDir"],
        "currentLink": metadata["currentLink"],
        "wrapperPath": metadata["wrapperPath"],
    },
    {
        "product": "mcodex",
        "installMethod": "script",
        "currentVersion": "0.96.0",
        "baseRoot": str(home / ".mcodex"),
        "versionsDir": str(home / ".mcodex/install"),
        "currentLink": str(home / ".mcodex/current"),
        "wrapperPath": str(home / ".local/bin/mcodex"),
    },
)
self.assertRegex(metadata["installedAt"], r"^20[0-9]{2}-[0-9]{2}-[0-9]{2}T")
wrapper = subprocess.run(
    [str(home / ".local/bin/mcodex")],
    text=True,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
)
self.assertEqual(wrapper.returncode, 7)
self.assertIn("managed=1", wrapper.stdout)
self.assertIn("method=script", wrapper.stdout)
self.assertIn(f"root={home / '.mcodex'}", wrapper.stdout)
```

- [ ] **Step 2: Run the failing Unix installer test**

Run:

```bash
python3 -m unittest scripts.install.test_install_scripts.InstallShTests
```

Expected: FAIL because the installer still downloads GitHub npm tarballs and does not support test base URL overrides.

- [ ] **Step 3: Rewrite `install.sh` around OSS native archives**

Implement the shell installer with these contracts:

- default `BASE_ROOT="${MCODEX_INSTALL_ROOT:-$HOME/.mcodex}"`
- `VERSIONS_DIR="$BASE_ROOT/install"`
- `CURRENT_LINK="$BASE_ROOT/current"`
- `METADATA_FILE="$BASE_ROOT/install.json"`
- `WRAPPER_DIR="${MCODEX_WRAPPER_DIR:-$HOME/.local/bin}"`
- `DOWNLOAD_BASE_URL="${MCODEX_DOWNLOAD_BASE_URL:-https://downloads.mcodex.sota.wiki}"`
- platform detection should honor `MCODEX_TEST_UNAME_S` and `MCODEX_TEST_UNAME_M` first in tests, then fall back to real `uname -s` / `uname -m`
- version input is first positional argument, default `latest`
- accepted explicit versions match `^[0-9]+[.][0-9]+[.][0-9]+(-((alpha|beta)[.][0-9]+))?$` after stripping optional `v` or `rust-v`
- invalid versions fail before download
- latest resolves `repositories/mcodex/channels/stable/latest.json`
- platform archive names match the spec
- checksum verification uses `sha256sum` if available, else `shasum -a 256`
- extraction always happens in a staging directory under `VERSIONS_DIR`
- completion marker is `.mcodex-install-complete.json`
- matching complete `versionDir` is reused
- incomplete or checksum-mismatched `versionDir` is replaced only after staging succeeds
- `current` symlink is switched by creating a temporary symlink in `baseRoot`
  and using a platform-specific no-target-directory rename: `mv -Tf` on Linux
  or `mv -fh` on macOS/BSD
- wrapper is written to a temp file and moved into place on every successful run
- PATH profile update reuses current `.zshrc`/`.bashrc`/`.profile` behavior

Use this `current` switch shape; do not use plain `mv -f` or `ln -sfn` for a symlink-to-directory:

```sh
switch_current_link() {
  target_dir="$1"
  tmp_link="$BASE_ROOT/.current.$$.tmp"
  rm -f "$tmp_link"
  ln -s "$target_dir" "$tmp_link"
  case "$os" in
    linux)
      mv -Tf "$tmp_link" "$CURRENT_LINK"
      ;;
    darwin)
      mv -fh "$tmp_link" "$CURRENT_LINK"
      ;;
    *)
      rm -f "$tmp_link"
      echo "unsupported platform for current link switch: $os" >&2
      exit 1
      ;;
  esac
}
```

Wrapper body should be thin:

```sh
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
```

- [ ] **Step 4: Add Unix installer repair and validation tests**

In `scripts/install/test_install_scripts.py`, add tests for:

- `install.sh latest`
- `install.sh 0.96.0`
- `install.sh 0.96.0-alpha.1`
- `install.sh v0.96.0`
- `install.sh rust-v0.96.0`
- invalid version syntax fails before HTTP requests
- reinstalling with a valid completion marker reuses `versionDir`
- upgrading from `0.96.0` to `0.97.0` changes `current` and the existing
  wrapper launches the new target
- marker-mismatched `versionDir` is replaced
- checksum mismatch does not switch `current`

- [ ] **Step 5: Run Unix installer tests**

Run:

```bash
python3 -m unittest scripts.install.test_install_scripts.InstallShTests
```

Expected: PASS.

- [ ] **Step 6: Commit Task 4**

```bash
git add scripts/install/install.sh scripts/install/test_install_scripts.py
git commit -m "feat: install mcodex from OSS on unix"
```

## Task 5: Windows PowerShell Installer

**Files:**
- Modify: `scripts/install/install.ps1`
- Modify: `scripts/install/test_install_scripts.py`

- [ ] **Step 1: Add PowerShell installer tests**

Extend `scripts/install/test_install_scripts.py` with Windows-focused tests that run only when PowerShell is available:

```python
@unittest.skipUnless(shutil.which("pwsh") or shutil.which("powershell"), "PowerShell unavailable")
class InstallPs1Tests(unittest.TestCase):
    def test_normalizes_explicit_versions_without_downloading(self) -> None:
        script = REPO_ROOT / "scripts/install/install.ps1"
        command = (
            f". '{script}'; "
            "Normalize-Version '0.96.0'; "
            "Normalize-Version '0.96.0-alpha.1'; "
            "Normalize-Version 'v0.96.0'; "
            "Normalize-Version 'rust-v0.96.0'"
        )
        result = run_powershell(command)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(
            result.stdout.splitlines(),
            ["0.96.0", "0.96.0-alpha.1", "0.96.0", "0.96.0"],
        )
```

Use a local fake OSS tree with `mcodex-win32-x64.zip` and `SHA256SUMS`. On non-Windows hosts, only run parser/static tests. On Windows CI, run end-to-end install tests using temp `LOCALAPPDATA`.

To support parser/static tests, structure `install.ps1` so its helper functions can be dot-sourced without immediately running the installer. Put the executable install body in a trailing `Invoke-McodexInstall` function and end the file with:

```powershell
if ($MyInvocation.InvocationName -ne ".") {
    Invoke-McodexInstall -Version $Version
}
```

That keeps normal CLI behavior unchanged while allowing tests to call `Normalize-Version` safely.

PowerShell tests should cover:

- omitted/latest install
- explicit `latest`
- explicit `0.96.0`, `v0.96.0`, `rust-v0.96.0`
- explicit `0.96.0-alpha.1`
- invalid version syntax fails before download
- wrapper file is `%LOCALAPPDATA%\Programs\Mcodex\bin\mcodex.ps1`
- PowerShell can invoke the wrapper as `mcodex`
- wrapper injects `MCODEX_INSTALL_MANAGED=1`, `MCODEX_INSTALL_METHOD=script`, and `MCODEX_INSTALL_ROOT`
- `install.json` matches the spec shape for `product`, `installMethod`, `currentVersion`, `baseRoot`, `versionsDir`, `currentLink`, and `wrapperPath`
- incomplete or marker-mismatched version directories are repaired from staging

- [ ] **Step 2: Run the failing PowerShell tests**

Run on a machine with PowerShell:

```bash
python3 -m unittest scripts.install.test_install_scripts.InstallPs1Tests
```

Expected: FAIL because the PowerShell installer still consumes GitHub npm tarballs.

- [ ] **Step 3: Rewrite `install.ps1` around OSS native zips**

Implement the PowerShell installer with these contracts:

- `$BaseRoot = $env:MCODEX_INSTALL_ROOT` or `%LOCALAPPDATA%\Mcodex`
- `$VersionsDir = Join-Path $BaseRoot "install"`
- `$CurrentLink = Join-Path $BaseRoot "current"`
- `$MetadataFile = Join-Path $BaseRoot "install.json"`
- `$WrapperDir = $env:MCODEX_WRAPPER_DIR` or `%LOCALAPPDATA%\Programs\Mcodex\bin`
- `$DownloadBaseUrl = $env:MCODEX_DOWNLOAD_BASE_URL` or `https://downloads.mcodex.sota.wiki`
- accepted version normalization matches Unix
- latest resolves OSS `latest.json`
- archive name is `mcodex-win32-x64.zip` or `mcodex-win32-arm64.zip`
- checksum validation uses `Get-FileHash -Algorithm SHA256`
- extraction uses `Expand-Archive` into a staging directory
- complete version directories get `.mcodex-install-complete.json`
- existing valid complete directories may be reused
- invalid existing directories are replaced only after staging succeeds
- `current` is a directory junction on Windows
- replace `current` by creating a new junction path then renaming with a backup, so failure leaves the previous current directory reachable
- write/replace `mcodex.ps1` on every successful run
- update user PATH registry entry on first install

Wrapper body:

```powershell
$BaseRoot = if ($env:MCODEX_INSTALL_ROOT) { $env:MCODEX_INSTALL_ROOT } else { Join-Path $env:LOCALAPPDATA "Mcodex" }
$Target = Join-Path $BaseRoot "current\bin\mcodex.exe"
if (-not (Test-Path $Target)) {
    Write-Error "mcodex installation missing or corrupted; rerun the installer."
    exit 1
}
$env:MCODEX_INSTALL_MANAGED = "1"
$env:MCODEX_INSTALL_METHOD = "script"
$env:MCODEX_INSTALL_ROOT = $BaseRoot
$env:Path = "$(Join-Path $BaseRoot "current\bin");$env:Path"
& $Target @args
exit $LASTEXITCODE
```

- [ ] **Step 4: Run PowerShell installer tests**

Run:

```bash
python3 -m unittest scripts.install.test_install_scripts.InstallPs1Tests
```

Expected: PASS on Windows or PASS/SKIP for end-to-end Windows-only tests on non-Windows hosts.

- [ ] **Step 5: Commit Task 5**

```bash
git add scripts/install/install.ps1 scripts/install/test_install_scripts.py
git commit -m "feat: install mcodex from OSS on windows"
```

## Task 6: Release Workflow, OSS Upload, and Lightweight GitHub Release

**Files:**
- Modify: `.github/workflows/rust-release.yml`
- Modify: `scripts/stage_npm_packages.py`
- Modify: `codex-cli/scripts/build_npm_package.py`
- Create: `scripts/test_stage_npm_packages.py`
- Create: `release/minisign.pub`

- [ ] **Step 1: Write failing npm staging guard tests**

Create `scripts/test_stage_npm_packages.py` with focused Python tests that assert CLI npm staging is rejected while `codex-sdk` and `codex-responses-api-proxy` remain accepted.

Example assertion:

```python
for package in [
    "codex",
    "codex-linux-x64",
    "codex-linux-arm64",
    "codex-darwin-x64",
    "codex-darwin-arm64",
    "codex-win32-x64",
    "codex-win32-arm64",
]:
    with self.subTest(package=package):
        with self.assertRaisesRegex(ValueError, "CLI npm package is no longer published"):
            stage_npm_packages.expand_packages([package])
self.assertEqual(
    stage_npm_packages.expand_packages(["codex-sdk"]),
    ["codex-sdk"],
)
```

- [ ] **Step 2: Run failing staging tests**

Run:

```bash
python3 -m unittest scripts.test_stage_npm_packages
```

Expected: FAIL until the npm staging guard exists.

- [ ] **Step 3: Remove CLI npm staging from release without breaking downstream publishing**

In `.github/workflows/rust-release.yml`:

- Remove `--package codex` from `Stage npm packages`.
- Replace `Define release name` with `Define release contract` (`id: release_contract`) and make it emit four outputs:
  - `release_version`: semver from `${{ github.ref_name }}` after stripping `rust-v`; this may include `-alpha.N` or `-beta.N`
  - `release_tag`: canonical GitHub release tag, which remains `${{ github.ref_name }}`
  - `stable_release_version`: the same value as `release_version` for stable tags, otherwise the empty string
  - `release_is_prerelease`: `true` when `release_version` contains `-`, otherwise `false`
- Make the release job outputs explicit, for example:

```yaml
outputs:
  release_version: ${{ steps.release_contract.outputs.release_version }}
  release_tag: ${{ steps.release_contract.outputs.release_tag }}
  stable_release_version: ${{ steps.release_contract.outputs.stable_release_version }}
  release_is_prerelease: ${{ steps.release_contract.outputs.release_is_prerelease }}
  should_publish_npm: ${{ steps.npm_publish_settings.outputs.should_publish }}
  npm_tag: ${{ steps.npm_publish_settings.outputs.npm_tag }}
```

- Update the release job outputs and every downstream reference end-to-end (`Determine npm publish settings`, `Stage native CLI archives`, `Sign CLI checksums`, `Upload CLI archives to OSS`, `Create GitHub Release`, `Trigger developers.openai.com deploy`, `publish-npm`, and any `if:` conditions) to use `steps.release_contract.outputs.*` / `needs.release.outputs.*`, not the old `steps.release_name.outputs.name`.
- Keep `--package codex-responses-api-proxy`; it is a non-CLI npm package and remains published.
- Keep `--package codex-sdk` because the spec says SDK npm publishing remains.
- Remove CLI npm tarball patterns from `publish-npm`.
- Keep only the non-CLI npm tarball patterns in `publish-npm`, explicitly:
  - `codex-responses-api-proxy-npm-${version}.tgz`
  - `codex-sdk-npm-${version}.tgz`
- Keep GitHub Release upload of `dist/npm/codex-responses-api-proxy-npm-<release_version>.tgz` and `dist/npm/codex-sdk-npm-<release_version>.tgz`, because the existing `publish-npm` job still downloads those tarballs from the release record.
- Remove the CLI `facebook/dotslash-publish-release` invocation that uses `.github/dotslash-config.json`, because GitHub Releases no longer carry CLI native assets in this slice.
- Leave the non-CLI `facebook/dotslash-publish-release` actions for `.github/dotslash-zsh-config.json` and `.github/dotslash-argument-comment-lint-config.json` unchanged.
- Publish versioned OSS CLI archives for every release tag, including prereleases; GitHub Releases remain lightweight records and are not a CLI binary channel.
- Only stable releases may write `repositories/mcodex/channels/stable/latest.json`. Prereleases must not touch the stable manifest, but their versioned OSS directories still publish under `repositories/mcodex/releases/<release_version>/`.
- Remove the `winget` job from this workflow because OS-native package manager publication is out of scope for this slice.

- [ ] **Step 4: Preserve SDK publishability without CLI npm**

In `codex-cli/scripts/build_npm_package.py`:

- For `package == "codex-sdk"`, stop injecting `dependencies[CODEX_NPM_NAME] = version`.
- Keep the SDK package's own build/publish path intact.
- Define a single blocked CLI npm package set that includes `codex` plus the platform packages:
  - `codex-linux-x64`
  - `codex-linux-arm64`
  - `codex-darwin-x64`
  - `codex-darwin-arm64`
  - `codex-win32-x64`
  - `codex-win32-arm64`
- For `package in BLOCKED_CLI_NPM_PACKAGES`, fail with a clear error such as `CLI npm package is no longer published; use scripts/stage_cli_archives.py`.

In `scripts/stage_npm_packages.py`, mirror the same blocked package set and guard both requested packages and expanded packages so CI fails early if a caller asks for `codex` or any `codex-<platform>` CLI package.

- [ ] **Step 5: Add minisign checksum signing**

Generate the release signing key once on a maintainer machine before enabling the workflow:

```bash
mkdir -p release
minisign -G -W -p release/minisign.pub -s /tmp/mcodex-minisign.key
base64 < /tmp/mcodex-minisign.key
```

Store the base64 private key output in the GitHub Actions secret `MINISIGN_PRIVATE_KEY_B64` and commit `release/minisign.pub`. Do not create or reference a minisign password secret for this workflow; `-W` creates an unencrypted CI signing key and avoids an interactive password prompt. Do not commit `/tmp/mcodex-minisign.key`.

In `.github/workflows/rust-release.yml`, add a signing step after `Stage native CLI archives` for every release tag:

```yaml
- name: Sign CLI checksums
  env:
    MINISIGN_PRIVATE_KEY_B64: ${{ secrets.MINISIGN_PRIVATE_KEY_B64 }}
    RELEASE_VERSION: ${{ steps.release_contract.outputs.release_version }}
  run: |
    set -euo pipefail
    checksum_dir="dist/oss/repositories/mcodex/releases/$RELEASE_VERSION"
    test -s "$checksum_dir/SHA256SUMS"
    printf '%s' "$MINISIGN_PRIVATE_KEY_B64" | base64 -d > "$RUNNER_TEMP/minisign.key"
    test -s release/minisign.pub
    minisign -S -s "$RUNNER_TEMP/minisign.key" -m "$checksum_dir/SHA256SUMS" -x "$checksum_dir/SHA256SUMS.sig"
    minisign -V -p release/minisign.pub -m "$checksum_dir/SHA256SUMS" -x "$checksum_dir/SHA256SUMS.sig"
    test -s "$checksum_dir/SHA256SUMS.sig"
```

Add this install step before signing:

```yaml
- name: Install minisign
  run: |
    set -euo pipefail
    sudo apt-get update
    sudo apt-get install -y minisign
```

- [ ] **Step 6: Upload OSS artifacts before the stable manifest**

Add an OSS upload step that uploads the versioned release directory for every tag, uploads the root installer scripts, and uploads the stable channel manifest last only for stable releases:

```yaml
- name: Upload CLI archives to OSS
  env:
    ALIYUN_ACCESS_KEY_ID: ${{ secrets.ALIYUN_ACCESS_KEY_ID }}
    ALIYUN_ACCESS_KEY_SECRET: ${{ secrets.ALIYUN_ACCESS_KEY_SECRET }}
    ALIYUN_OSS_ENDPOINT: ${{ secrets.ALIYUN_OSS_ENDPOINT }}
    ALIYUN_OSS_BUCKET: ${{ secrets.ALIYUN_OSS_BUCKET }}
    RELEASE_VERSION: ${{ steps.release_contract.outputs.release_version }}
    STABLE_RELEASE_VERSION: ${{ steps.release_contract.outputs.stable_release_version }}
    RELEASE_TAG: ${{ steps.release_contract.outputs.release_tag }}
  run: |
    set -euo pipefail
    if [[ ! "$RELEASE_VERSION" =~ ^[0-9]+[.][0-9]+[.][0-9]+(-((alpha|beta)[.][0-9]+))?$ ]]; then
      echo "Expected normalized release semver, got: $RELEASE_VERSION" >&2
      exit 1
    fi
    if [[ ! "$RELEASE_TAG" =~ ^rust-v[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta)\.[0-9]+)?$ ]]; then
      echo "Expected canonical GitHub release tag rust-v<semver>, got: $RELEASE_TAG" >&2
      exit 1
    fi
    release_dir="dist/oss/repositories/mcodex/releases/$RELEASE_VERSION"
    channel_manifest="dist/oss/repositories/mcodex/channels/stable/latest.json"
    mkdir -p dist
    cp scripts/install/install.sh dist/install.sh
    cp scripts/install/install.ps1 dist/install.ps1
    curl -fsSL -o "$RUNNER_TEMP/ossutil.zip" \
      https://gosspublic.alicdn.com/ossutil/v2/2.2.2/ossutil-2.2.2-linux-amd64.zip
    unzip -q "$RUNNER_TEMP/ossutil.zip" -d "$RUNNER_TEMP/ossutil"
    OSSUTIL="$RUNNER_TEMP/ossutil/ossutil-2.2.2-linux-amd64/ossutil"
    "$OSSUTIL" config -e "$ALIYUN_OSS_ENDPOINT" -i "$ALIYUN_ACCESS_KEY_ID" -k "$ALIYUN_ACCESS_KEY_SECRET"
    "$OSSUTIL" cp -r "$release_dir/" "oss://$ALIYUN_OSS_BUCKET/repositories/mcodex/releases/$RELEASE_VERSION/"
    "$OSSUTIL" cp "dist/install.sh" "oss://$ALIYUN_OSS_BUCKET/install.sh"
    "$OSSUTIL" cp "dist/install.ps1" "oss://$ALIYUN_OSS_BUCKET/install.ps1"
    if [[ -n "$STABLE_RELEASE_VERSION" ]]; then
      "$OSSUTIL" cp "$channel_manifest" "oss://$ALIYUN_OSS_BUCKET/repositories/mcodex/channels/stable/latest.json"
    fi
```

The versioned release directory upload must happen before any stable `latest.json` mutation. The root installer script upload must happen before `latest.json`, because `latest.json`, docs, release notes, and TUI update prompts all advertise `https://downloads.mcodex.sota.wiki/install.sh` and `https://downloads.mcodex.sota.wiki/install.ps1`.
Prerelease tags still publish versioned OSS artifacts and signed checksums, but they must not upload the stable channel manifest.

- [ ] **Step 7: Make every GitHub Release lightweight**

Use a single lightweight GitHub Release / prerelease path for all tags. The attached files remain small and never include the native `mcodex-*` CLI archives:

```text
dist/oss/repositories/mcodex/releases/<release_version>/SHA256SUMS
dist/oss/repositories/mcodex/releases/<release_version>/SHA256SUMS.sig
dist/install.sh
dist/install.ps1
dist/config-schema.json
dist/npm/codex-responses-api-proxy-npm-<release_version>.tgz
dist/npm/codex-sdk-npm-<release_version>.tgz
```

Do not attach the large native `mcodex-*` platform archives to GitHub Release.

Append release body text with:

```bash
{
  cat <<'EOF'
Install mcodex CLI from OSS:

macOS/Linux:
curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh

Windows PowerShell:
powershell -NoProfile -ExecutionPolicy Bypass -Command "iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\mcodex-install.ps1; & $env:TEMP\mcodex-install.ps1"

This release uses the script-managed OSS distribution channel. npm, bun, Homebrew, and WinGet are no longer advertised CLI update paths for mcodex.

If you previously installed mcodex through npm, reinstall now with the OSS installer to move onto the supported update channel.

If this release is a prerelease, install it explicitly by passing the version to the installer script (for example `sh -s -- 0.96.0-alpha.1` on macOS/Linux or `mcodex-install.ps1 0.96.0-alpha.1` on PowerShell). `latest` remains the stable channel.

Checksum signature audit:
The public minisign key for SHA256SUMS.sig is committed at release/minisign.pub and is repeated below:
EOF
  cat release/minisign.pub
} >> "${{ steps.release_notes.outputs.path }}"
```

Run this before the GitHub Release step for every tag so the literal contents of `release/minisign.pub` are included in every release note.

- [ ] **Step 8: Run workflow/script checks**

Run:

```bash
python3 -m unittest scripts.test_stage_cli_archives
python3 -m unittest scripts.test_stage_npm_packages
python3 -m py_compile scripts/stage_cli_archives.py scripts/stage_npm_packages.py codex-cli/scripts/build_npm_package.py
python3 codex-cli/scripts/test_install_native_deps.py
if rg -n "codex-npm-(linux|darwin|win32)|codex-npm-\\$\\{|codex-(x86_64|aarch64).*\\.(zst|zip|tar\\.gz|dmg)|mcodex-(linux|darwin|win32)-(x64|arm64)\\.(tar\\.gz|zip)|files:\\s*dist/\\*\\*" .github/workflows/rust-release.yml .github/workflows/rust-release-windows.yml; then
  echo "disallowed CLI release artifact pattern still present" >&2
  exit 1
fi
```

Expected: Python checks PASS. The `rg` command should return no old CLI npm tarball patterns, no native `mcodex-*` GitHub Release attachment references, no native GitHub Release attachment globs, and no old CLI release asset names. Mentions for `codex-responses-api-proxy`, `codex-command-runner`, `codex-windows-sandbox-setup`, non-CLI npm packages, and historical cutover docs are acceptable.

- [ ] **Step 9: Commit Task 6**

```bash
git add .github/workflows/rust-release.yml scripts/stage_npm_packages.py codex-cli/scripts/build_npm_package.py scripts/test_stage_npm_packages.py release/minisign.pub
git commit -m "feat: publish mcodex cli archives to OSS"
```

## Task 7: SDK CLI Discovery After npm Cutover

**Files:**
- Modify: `sdk/typescript/src/exec.ts`
- Modify: `sdk/typescript/tests/exec.test.ts`

- [ ] **Step 1: Write failing SDK discovery tests**

In `sdk/typescript/tests/exec.test.ts`, add tests for default CLI discovery:

```ts
it("prefers mcodex from PATH when no explicit executable is provided", async () => {
  const { _findCodexPathForTesting } = await import("../src/exec");
  const separator = process.platform === "win32" ? ";" : ":";
  const envPath = ["/tmp/nope", "/tmp/mcodex-bin"].join(separator);
  const resolved = _findCodexPathForTesting({
    envPath,
    platform: "linux",
    arch: "x64",
    pathExists: (candidate: string) => candidate === "/tmp/mcodex-bin/mcodex",
    resolvePackageJson: () => {
      throw new Error("npm package should not be required");
    },
  });
  expect(resolved).toBe("/tmp/mcodex-bin/mcodex");
});

it("reports script install guidance when no CLI can be found", async () => {
  const { _findCodexPathForTesting } = await import("../src/exec");
  expect(() =>
    _findCodexPathForTesting({
      envPath: "/tmp/empty",
      platform: "linux",
      arch: "x64",
      pathExists: () => false,
      resolvePackageJson: () => {
        throw new Error("missing package");
      },
    }),
  ).toThrow(/downloads\.mcodex\.sota\.wiki\/install\.(sh|ps1)/);
});
```

Export `_findCodexPathForTesting` from `sdk/typescript/src/exec.ts` as an underscored helper. Keep the public `CodexExec` constructor unchanged.

- [ ] **Step 2: Run failing SDK tests**

Run:

```bash
cd sdk/typescript
pnpm test -- exec.test.ts
```

Expected: FAIL until `exec.ts` searches PATH.

- [ ] **Step 3: Implement PATH-first CLI discovery**

In `sdk/typescript/src/exec.ts`:

- Keep explicit constructor `executablePath` as the highest priority.
- Search `process.env.PATH` for `mcodex` before npm package lookup.
- Keep npm package lookup only as a legacy fallback if the package exists locally.
- Update error text to point users to the platform-appropriate OSS installer, for example: `Install mcodex with https://downloads.mcodex.sota.wiki/install.sh (macOS/Linux) or https://downloads.mcodex.sota.wiki/install.ps1 (Windows), or pass an explicit executable path.`
- Prefer `mcodex.exe` on Windows.

- [ ] **Step 4: Run SDK tests**

Run:

```bash
cd sdk/typescript
pnpm test -- exec.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit Task 7**

```bash
git add sdk/typescript/src/exec.ts sdk/typescript/tests/exec.test.ts
git commit -m "fix(sdk): discover script-installed mcodex cli"
```

## Task 8: Documentation Cutover

**Files:**
- Modify: `README.md`
- Modify: `docs/install.md`
- Modify: `codex-cli/scripts/README.md`

- [ ] **Step 1: Update README install commands**

Replace top-level npm/Homebrew install instructions with:

```markdown
```shell
# macOS/Linux
curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh
```

```powershell
# Windows PowerShell
powershell -NoProfile -ExecutionPolicy Bypass -Command "iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\mcodex-install.ps1; & $env:TEMP\mcodex-install.ps1"
```
```

Add explicit version examples:

```shell
curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh -s -- 0.96.0
```

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\mcodex-install.ps1; & $env:TEMP\mcodex-install.ps1 0.96.0"
```

- [ ] **Step 2: Update install docs**

In `docs/install.md`, add a "Script-managed CLI install" section covering:

- install latest
- install explicit version
- update by rerunning the installer
- install root layout
- wrapper path
- `MCODEX_INSTALL_MANAGED`, `MCODEX_INSTALL_METHOD`, `MCODEX_INSTALL_ROOT`
- npm/bun/Homebrew/WinGet are not advertised update paths for `mcodex`
- DotSlash is no longer advertised for the `mcodex` CLI, because lightweight GitHub Releases no longer carry native CLI assets
- GitHub Releases are release records, not the primary binary download path
- release notes and docs must explicitly tell existing npm-installed CLI users to reinstall via the OSS installer to join the new update channel

- [ ] **Step 3: Update npm helper docs**

In `codex-cli/scripts/README.md`, change the title from `npm releases` to `non-CLI npm releases` and remove `--package codex` from examples.

The example should stage only:

```bash
./scripts/stage_npm_packages.py \
  --release-version 0.96.0 \
  --package codex-responses-api-proxy \
  --package codex-sdk
```

- [ ] **Step 4: Run documentation sanity checks**

Run:

```bash
rg -n "npm install -g|brew install --cask|bun install -g|codex-npm|GitHub Release.*download the appropriate binary" README.md docs/install.md codex-cli/scripts/README.md
```

Expected: no CLI install/update instructions point users to npm, bun, Homebrew, or GitHub binary downloads. Mentions that explicitly describe old cutover or non-CLI npm packaging are acceptable.

- [ ] **Step 5: Commit Task 8**

```bash
git add README.md docs/install.md codex-cli/scripts/README.md
git commit -m "docs: document OSS script-managed mcodex installs"
```

## Task 9: Final Verification

**Files:**
- All files changed by Tasks 1-8

- [ ] **Step 1: Run Python tests**

```bash
python3 -m unittest scripts.test_stage_cli_archives
python3 -m unittest scripts.install.test_install_scripts
python3 codex-cli/scripts/test_install_native_deps.py
python3 -m py_compile scripts/stage_cli_archives.py scripts/stage_npm_packages.py codex-cli/scripts/build_npm_package.py codex-cli/scripts/install_native_deps.py
```

Expected: PASS, with Windows-only PowerShell end-to-end tests skipped on non-Windows hosts if PowerShell is unavailable.

- [ ] **Step 2: Run Rust formatting**

```bash
cd codex-rs
just fmt
```

Expected: PASS.

- [ ] **Step 3: Run focused Rust tests**

```bash
cd codex-rs
cargo test -p codex-product-identity
cargo test -p codex-tui update_action -- --nocapture
cargo test -p codex-tui update_available_history_cell -- --nocapture
cargo test -p codex-tui --release updates -- --nocapture
cargo test -p codex-tui --release update_prompt -- --nocapture
cargo test -p codex-cli update -- --nocapture
```

Expected: PASS.

- [ ] **Step 4: Run TUI snapshot check**

```bash
cd codex-rs
cargo insta pending-snapshots -p codex-tui
```

Expected: no pending snapshots.

- [ ] **Step 5: Run TypeScript SDK tests**

```bash
cd sdk/typescript
pnpm test -- exec.test.ts
```

Expected: PASS.

- [ ] **Step 6: Run release/docs grep checks**

```bash
rg -n "codex-npm|npm install -g|bun install -g|brew upgrade|brew install --cask|WinGet|winget" \
  scripts/install .github/workflows/rust-release.yml README.md docs/install.md
if rg -n "codex-npm-(linux|darwin|win32)|codex-npm-\\$\\{|codex-(x86_64|aarch64).*\\.(zst|zip|tar\\.gz|dmg)|mcodex-(linux|darwin|win32)-(x64|arm64)\\.(tar\\.gz|zip)|files:\\s*dist/\\*\\*" .github/workflows/rust-release.yml .github/workflows/rust-release-windows.yml; then
  echo "disallowed CLI release artifact pattern still present" >&2
  exit 1
fi
```

Expected: no active CLI install/update path references remain. Historical explanatory text is acceptable only in docs that explicitly discuss the cutover.

- [ ] **Step 7: Run manual installer smoke on real platforms**

Confirm on actual hosts, not only fixture tests:

- macOS or Linux: fresh latest install from `https://downloads.mcodex.sota.wiki/install.sh`
- macOS or Linux: explicit version install, then upgrade to a newer version and verify `current`
- Windows PowerShell: fresh latest install from `https://downloads.mcodex.sota.wiki/install.ps1`
- Windows PowerShell: explicit version install, then upgrade to a newer version and verify `current`
- both platforms: rerun installer after corrupting the target version marker and verify repair without breaking the last working wrapper
- both platforms: TUI update prompt opens the GitHub release notes from `latest_notes_url`

- [ ] **Step 8: Run lints/fixes for changed Rust crates**

```bash
cd codex-rs
just fix -p codex-product-identity
just fix -p codex-tui
just fix -p codex-cli
```

Expected: completes without unreviewed semantic changes. Do not rerun tests after `just fix` unless the command reports a substantive code change that needs targeted re-checking.

- [ ] **Step 9: Review git diff**

```bash
git status --short
git diff --stat
git diff --check
```

Expected: only intended files changed, no whitespace errors.

- [ ] **Step 10: Final commit if needed**

If Task 9 introduced formatting or lint-only edits:

```bash
git add codex-rs scripts README.md docs .github codex-cli sdk
git commit -m "chore: verify mcodex cli distribution cutover"
```

Expected: clean working tree after commit.

## Implementation Notes

- Do not remove npm/pnpm from the repo; only remove CLI npm distribution paths.
- Do not add release-channel selection in this slice; only stable latest is supported.
- Keep wrapper logic minimal; wrappers must not perform network checks or installs.
- Keep `SHA256SUMS.sig` release-record-only for installers; installers verify `SHA256SUMS`, not the signature.
- Use staging directories for every installer mutation that can fail.
- Make the OSS base URL a single named constant in Rust and in scripts to avoid drift.
- For Windows `current` replacement, implement the junction switch carefully: create a new junction path, validate it, rename the old junction to a backup, rename the new junction to `current`, then remove the backup. If any step fails before `current` changes, leave the previous `current` intact.
