from __future__ import annotations

import hashlib
import json
import os
import shutil
import socket
import subprocess
import tarfile
import tempfile
import time
import unittest
import zipfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
INSTALL_SH = REPO_ROOT / "scripts" / "install" / "install.sh"
INSTALL_PS1 = REPO_ROOT / "scripts" / "install" / "install.ps1"
LATEST_MANIFEST_PATH = Path("repositories/mcodex/channels/stable/latest.json")
ARCHIVE_NAME = "mcodex-linux-x64.tar.gz"
WINDOWS_ARCHIVE_NAME = "mcodex-win32-x64.zip"
POWERSHELL = shutil.which("pwsh") or shutil.which("powershell")


class FixtureHttpServer:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.port = _find_free_port()
        self._log_file = tempfile.NamedTemporaryFile(mode="w+", encoding="utf-8")
        self._process: subprocess.Popen[str] | None = None

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def start(self) -> None:
        self._process = subprocess.Popen(
            ["python3", "-u", "-m", "http.server", str(self.port), "--bind", "127.0.0.1"],
            cwd=self.root,
            stdout=subprocess.DEVNULL,
            stderr=self._log_file,
            text=True,
        )
        deadline = time.time() + 5
        while time.time() < deadline:
            if self._process.poll() is not None:
                raise RuntimeError("http.server exited before accepting connections")
            try:
                with socket.create_connection(("127.0.0.1", self.port), timeout=0.2):
                    return
            except OSError:
                time.sleep(0.05)
        raise RuntimeError("timed out waiting for http.server")

    def stop(self) -> None:
        if self._process is None:
            return
        self._process.terminate()
        self._process.wait(timeout=5)
        self._process = None

    def request_log(self) -> str:
        self._log_file.flush()
        self._log_file.seek(0)
        return self._log_file.read()

    def close(self) -> None:
        self.stop()
        self._log_file.close()


class FakeOssRepository:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.repository_root = self.root / "repositories" / "mcodex"
        self.releases_root = self.repository_root / "releases"
        self.channels_root = self.repository_root / "channels" / "stable"

    def add_release(self, version: str, *, checksum_override: str | None = None) -> None:
        release_dir = self.releases_root / version
        release_dir.mkdir(parents=True, exist_ok=True)
        checksums: list[tuple[str, str]] = []

        archive_path = release_dir / ARCHIVE_NAME
        self._create_archive(archive_path, version)
        sha256 = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        checksums.append((ARCHIVE_NAME, checksum_override or sha256))

        windows_archive_path = release_dir / WINDOWS_ARCHIVE_NAME
        self._create_windows_archive(windows_archive_path, version)
        windows_sha256 = hashlib.sha256(windows_archive_path.read_bytes()).hexdigest()
        checksums.append((WINDOWS_ARCHIVE_NAME, checksum_override or windows_sha256))

        checksum_lines = [f"{checksum}  {name}" for name, checksum in checksums]
        (release_dir / "SHA256SUMS").write_text(
            "\n".join(checksum_lines) + "\n",
            encoding="utf-8",
        )

    def set_latest(self, version: str) -> None:
        self.channels_root.mkdir(parents=True, exist_ok=True)
        latest_manifest = {
            "product": "mcodex",
            "channel": "stable",
            "version": version,
            "publishedAt": "2026-04-20T12:00:00Z",
            "notesUrl": f"https://github.com/vivym/mcodex/releases/tag/rust-v{version}",
            "checksumsUrl": (
                "https://downloads.mcodex.sota.wiki/"
                f"repositories/mcodex/releases/{version}/SHA256SUMS"
            ),
            "install": {
                "unix": "curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh",
                "windows": (
                    "powershell -NoProfile -ExecutionPolicy Bypass -Command "
                    "\"iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 "
                    "-OutFile $env:TEMP\\mcodex-install.ps1; "
                    "& $env:TEMP\\mcodex-install.ps1\""
                ),
            },
        }
        (self.channels_root / "latest.json").write_text(
            json.dumps(latest_manifest, indent=2) + "\n",
            encoding="utf-8",
        )

    def _create_archive(self, archive_path: Path, version: str) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source_root = Path(tmp)
            bin_dir = source_root / "bin"
            bin_dir.mkdir(parents=True)
            mcodex = bin_dir / "mcodex"
            mcodex.write_text(
                "\n".join(
                    [
                        "#!/bin/sh",
                        f"printf 'version={version}\\n'",
                        "printf 'managed=%s\\n' \"$MCODEX_INSTALL_MANAGED\"",
                        "printf 'method=%s\\n' \"$MCODEX_INSTALL_METHOD\"",
                        "printf 'root=%s\\n' \"$MCODEX_INSTALL_ROOT\"",
                        "printf 'path=%s\\n' \"$PATH\"",
                        "exit 7",
                        "",
                    ]
                ),
                encoding="utf-8",
            )
            mcodex.chmod(0o755)
            rg = bin_dir / "rg"
            rg.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            rg.chmod(0o755)
            with tarfile.open(archive_path, "w:gz") as archive:
                archive.add(mcodex, arcname="bin/mcodex")
                archive.add(rg, arcname="bin/rg")

    def _create_windows_archive(self, archive_path: Path, version: str) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            source_root = Path(tmp)
            bin_dir = source_root / "bin"
            bin_dir.mkdir(parents=True)

            mcodex = bin_dir / "mcodex.exe"
            rg = bin_dir / "rg.exe"
            version_file = bin_dir / "version.txt"

            if os.name == "nt" and POWERSHELL:
                shutil.copyfile(POWERSHELL, mcodex)
                shutil.copyfile(POWERSHELL, rg)
            else:
                mcodex.write_text("placeholder\n", encoding="utf-8")
                rg.write_text("placeholder\n", encoding="utf-8")

            version_file.write_text(f"{version}\n", encoding="utf-8")

            with zipfile.ZipFile(
                archive_path,
                "w",
                compression=zipfile.ZIP_DEFLATED,
            ) as archive:
                archive.write(mcodex, arcname="bin/mcodex.exe")
                archive.write(rg, arcname="bin/rg.exe")
                archive.write(version_file, arcname="bin/version.txt")


def run_powershell(
    command: str,
    *,
    env: dict[str, str] | None = None,
    cwd: Path = REPO_ROOT,
    timeout: int = 10,
) -> subprocess.CompletedProcess[str]:
    if POWERSHELL is None:
        raise RuntimeError("PowerShell unavailable")

    return subprocess.run(
        [
            POWERSHELL,
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            command,
        ],
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        timeout=timeout,
    )


@unittest.skipUnless(POWERSHELL, "PowerShell unavailable")
class InstallPs1Tests(unittest.TestCase):
    maxDiff = None

    def test_normalizes_explicit_versions_without_downloading(self) -> None:
        command = (
            f". '{INSTALL_PS1}'; "
            "Normalize-Version '0.96.0'; "
            "Normalize-Version '0.96.0-alpha.1'; "
            "Normalize-Version 'v0.96.0'; "
            "Normalize-Version 'rust-v0.96.0'"
        )
        result = run_powershell(command)
        self.assertEqual(result.returncode, 0, msg=result.stderr)
        self.assertEqual(
            result.stdout.splitlines(),
            ["0.96.0", "0.96.0-alpha.1", "0.96.0", "0.96.0"],
        )

    def test_write_wrapper_outputs_literal_runtime_variables(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            wrapper_path = Path(tmp) / "mcodex.ps1"
            result = run_powershell(
                "\n".join(
                    [
                        "$ErrorActionPreference = 'Stop'",
                        f". '{INSTALL_PS1}'",
                        f"$wrapperPath = {_powershell_single_quote(str(wrapper_path))}",
                        "Write-Wrapper -BaseRoot 'C:\\tmp\\mcodex' -WrapperPath $wrapperPath",
                        "Get-Content -LiteralPath $wrapperPath -Raw",
                    ]
                )
            )

        self.assertEqual(result.returncode, 0, msg=result.stderr)
        self.assertIn(
            '$BaseRoot = if ($env:MCODEX_INSTALL_ROOT) { $env:MCODEX_INSTALL_ROOT } else { \'C:\\tmp\\mcodex\' }',
            result.stdout,
        )
        self.assertIn('if (-not (Test-Path -LiteralPath $Target)) {', result.stdout)
        self.assertIn('$env:MCODEX_INSTALL_ROOT = $BaseRoot', result.stdout)
        self.assertIn('& $Target @args', result.stdout)
        self.assertIn('exit $LASTEXITCODE', result.stdout)

    def test_path_update_helper_models_registry_branch_without_mutation(self) -> None:
        wrapper_dir = r"C:\Users\viv\AppData\Local\Programs\Mcodex\bin"
        result = run_powershell(
            "\n".join(
                [
                    "$ErrorActionPreference = 'Stop'",
                    f". '{INSTALL_PS1}'",
                    "$results = @(",
                    (
                        "    Get-WrapperDirPathUpdate "
                        f"-WrapperDir '{wrapper_dir}' "
                        "-UserPath 'C:\\Windows\\System32' "
                        "-ProcessPath 'C:\\Windows\\System32'"
                    ),
                    (
                        "    Get-WrapperDirPathUpdate "
                        f"-WrapperDir '{wrapper_dir}' "
                        f"-UserPath '{wrapper_dir};C:\\Windows\\System32' "
                        "-ProcessPath 'C:\\Windows\\System32'"
                    ),
                    (
                        "    Get-WrapperDirPathUpdate "
                        f"-WrapperDir '{wrapper_dir}' "
                        f"-UserPath '{wrapper_dir};C:\\Windows\\System32' "
                        f"-ProcessPath '{wrapper_dir};C:\\Windows\\System32'"
                    ),
                    ")",
                    "$results | ConvertTo-Json -Depth 4 -Compress",
                ]
            )
        )

        self.assertEqual(result.returncode, 0, msg=result.stderr)
        self.assertEqual(
            json.loads(result.stdout),
            [
                {
                    "Action": "added",
                    "UserPath": rf"{wrapper_dir};C:\Windows\System32",
                    "ProcessPath": rf"{wrapper_dir};C:\Windows\System32",
                    "UpdateUserPath": True,
                },
                {
                    "Action": "configured",
                    "UserPath": rf"{wrapper_dir};C:\Windows\System32",
                    "ProcessPath": rf"{wrapper_dir};C:\Windows\System32",
                    "UpdateUserPath": False,
                },
                {
                    "Action": "already",
                    "UserPath": rf"{wrapper_dir};C:\Windows\System32",
                    "ProcessPath": rf"{wrapper_dir};C:\Windows\System32",
                    "UpdateUserPath": False,
                },
            ],
        )

    def test_invalid_versions_fail_before_download(self) -> None:
        with self.install_fixture() as fixture:
            for version in ("invalid-version", "vlatest", "rust-vlatest"):
                with self.subTest(version=version):
                    result = run_powershell(
                        "\n".join(
                            [
                                "$ErrorActionPreference = 'Stop'",
                                f". '{INSTALL_PS1}'",
                                "try {",
                                (
                                    "    Resolve-RequestedVersion "
                                    f"-RequestedVersion '{version}' "
                                    f"-DownloadBaseUrl '{fixture.server.base_url}' | Out-Null"
                                ),
                                "    exit 0",
                                "} catch {",
                                "    Write-Output $_.Exception.Message",
                                "    exit 1",
                                "}",
                            ]
                        )
                    )
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("Invalid version", result.stdout + result.stderr)
            self.assertEqual(fixture.server.request_log(), "")

    @unittest.skipUnless(os.name == "nt", "Windows-only end-to-end install tests")
    def test_install_latest_when_version_is_omitted_or_explicit(self) -> None:
        for version in (None, "latest"):
            with self.subTest(version=version or "<omitted>"):
                with self.install_fixture() as fixture:
                    result = fixture.run_install_ps1(version)
                    self.assertEqual(result.returncode, 0, msg=result.stderr)
                    self.assert_install_layout(fixture, "0.96.0")

    @unittest.skipUnless(os.name == "nt", "Windows-only end-to-end install tests")
    def test_install_explicit_versions(self) -> None:
        versions = {
            "0.96.0": "0.96.0",
            "v0.96.0": "0.96.0",
            "rust-v0.96.0": "0.96.0",
            "0.96.0-alpha.1": "0.96.0-alpha.1",
        }
        for requested, resolved in versions.items():
            with self.subTest(version=requested):
                with self.install_fixture() as fixture:
                    result = fixture.run_install_ps1(requested)
                    self.assertEqual(result.returncode, 0, msg=result.stderr)
                    self.assert_install_layout(fixture, resolved)

    @unittest.skipUnless(os.name == "nt", "Windows-only end-to-end install tests")
    def test_repairs_incomplete_version_directory(self) -> None:
        with self.install_fixture() as fixture:
            version_dir = fixture.default_base_root / "install" / "0.96.0"
            (version_dir / "bin").mkdir(parents=True)
            (version_dir / "junk.txt").write_text("stale\n", encoding="utf-8")

            result = fixture.run_install_ps1("0.96.0")
            self.assertEqual(result.returncode, 0, msg=result.stderr)

            self.assert_install_layout(fixture, "0.96.0")
            self.assertFalse((version_dir / "junk.txt").exists())

    @unittest.skipUnless(os.name == "nt", "Windows-only end-to-end install tests")
    def test_repairs_marker_mismatched_version_directory(self) -> None:
        with self.install_fixture() as fixture:
            first = fixture.run_install_ps1("0.96.0")
            self.assertEqual(first.returncode, 0, msg=first.stderr)

            version_dir = fixture.default_base_root / "install" / "0.96.0"
            marker = version_dir / ".mcodex-install-complete.json"
            marker_data = json.loads(marker.read_text(encoding="utf-8"))
            marker_data["sha256"] = "broken"
            marker.write_text(json.dumps(marker_data), encoding="utf-8")
            (version_dir / "junk.txt").write_text("stale\n", encoding="utf-8")

            second = fixture.run_install_ps1("0.96.0")
            self.assertEqual(second.returncode, 0, msg=second.stderr)

            self.assert_install_layout(fixture, "0.96.0")
            self.assertFalse((version_dir / "junk.txt").exists())

    def assert_install_layout(self, fixture: "_InstallFixture", version: str) -> None:
        base_root = fixture.default_base_root
        version_dir = base_root / "install" / version
        current_link = base_root / "current"
        wrapper_path = fixture.localappdata / "Programs" / "Mcodex" / "bin" / "mcodex.ps1"
        metadata_path = base_root / "install.json"

        self.assertTrue((version_dir / "bin" / "mcodex.exe").exists())
        self.assertTrue((version_dir / "bin" / "rg.exe").exists())
        self.assertTrue((current_link / "bin" / "mcodex.exe").exists())
        self.assertTrue(wrapper_path.exists())
        self.assertTrue(metadata_path.exists())
        self.assertTrue(os.path.samefile(current_link, version_dir))

        link_type = run_powershell(f"(Get-Item '{current_link}').LinkType", env=fixture.powershell_env())
        self.assertEqual(link_type.returncode, 0, msg=link_type.stderr)
        self.assertEqual(link_type.stdout.strip(), "Junction")

        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
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
                "currentVersion": version,
                "baseRoot": str(base_root),
                "versionsDir": str(base_root / "install"),
                "currentLink": str(current_link),
                "wrapperPath": str(wrapper_path),
            },
        )
        self.assertRegex(metadata["installedAt"], r"^20[0-9]{2}-[0-9]{2}-[0-9]{2}T")

        marker = json.loads(
            (version_dir / ".mcodex-install-complete.json").read_text(encoding="utf-8")
        )
        self.assertEqual(marker["version"], version)
        self.assertEqual(marker["archiveName"], WINDOWS_ARCHIVE_NAME)
        self.assertRegex(marker["installedAt"], r"^20[0-9]{2}-[0-9]{2}-[0-9]{2}T")

        wrapper_text = wrapper_path.read_text(encoding="utf-8")
        self.assertIn('MCODEX_INSTALL_MANAGED = "1"', wrapper_text)
        self.assertIn('MCODEX_INSTALL_METHOD = "script"', wrapper_text)
        self.assertIn('MCODEX_INSTALL_ROOT = $BaseRoot', wrapper_text)
        self.assertIn('current\\bin\\mcodex.exe', wrapper_text)

        wrapper = fixture.run_wrapper_ps1()
        self.assertEqual(wrapper.returncode, 7, msg=wrapper.stderr)
        self.assertIn(f"version={version}", wrapper.stdout)
        self.assertIn("managed=1", wrapper.stdout)
        self.assertIn("method=script", wrapper.stdout)
        self.assertIn(f"root={base_root}", wrapper.stdout)
        self.assertIn(f"path={base_root / 'current' / 'bin'};", wrapper.stdout)

    def install_fixture(self) -> "_InstallFixture":
        return _InstallFixture()


class InstallShTests(unittest.TestCase):
    maxDiff = None

    def test_install_latest(self) -> None:
        with self.install_fixture() as fixture:
            result = fixture.run_install("latest")
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assert_latest_install_layout(fixture.home, "0.96.0")

    def test_install_explicit_version(self) -> None:
        with self.install_fixture() as fixture:
            result = fixture.run_install("0.96.0")
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assert_current_version(fixture.home, "0.96.0")

    def test_install_alpha_version(self) -> None:
        with self.install_fixture() as fixture:
            result = fixture.run_install("0.96.0-alpha.1")
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assert_current_version(fixture.home, "0.96.0-alpha.1")

    def test_install_version_with_v_prefix(self) -> None:
        with self.install_fixture() as fixture:
            result = fixture.run_install("v0.96.0")
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assert_current_version(fixture.home, "0.96.0")

    def test_install_version_with_rust_v_prefix(self) -> None:
        with self.install_fixture() as fixture:
            result = fixture.run_install("rust-v0.96.0")
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assert_current_version(fixture.home, "0.96.0")

    def test_invalid_version_fails_before_http_requests(self) -> None:
        with self.install_fixture() as fixture:
            result = fixture.run_install("invalid-version")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Invalid version", result.stderr)
            self.assertEqual(fixture.server.request_log(), "")

    def test_vlatest_fails_before_http_requests(self) -> None:
        with self.install_fixture() as fixture:
            result = fixture.run_install("vlatest")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Invalid version", result.stderr)
            self.assertEqual(fixture.server.request_log(), "")

    def test_rust_vlatest_fails_before_http_requests(self) -> None:
        with self.install_fixture() as fixture:
            result = fixture.run_install("rust-vlatest")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Invalid version", result.stderr)
            self.assertEqual(fixture.server.request_log(), "")

    def test_reinstall_reuses_valid_version_directory(self) -> None:
        with self.install_fixture() as fixture:
            first = fixture.run_install("0.96.0")
            self.assertEqual(first.returncode, 0, msg=first.stderr)
            sentinel = fixture.home / ".mcodex" / "install" / "0.96.0" / "sentinel.txt"
            sentinel.write_text("keep me\n", encoding="utf-8")
            second = fixture.run_install("0.96.0")
            self.assertEqual(second.returncode, 0, msg=second.stderr)
            self.assertTrue(sentinel.exists())
            self.assert_current_version(fixture.home, "0.96.0")

    def test_upgrade_switches_current_and_existing_wrapper(self) -> None:
        with self.install_fixture() as fixture:
            first = fixture.run_install("0.96.0")
            self.assertEqual(first.returncode, 0, msg=first.stderr)
            first_wrapper = fixture.run_wrapper()
            self.assertEqual(first_wrapper.returncode, 7, msg=first_wrapper.stderr)
            self.assertIn("version=0.96.0", first_wrapper.stdout)
            second = fixture.run_install("0.97.0")
            self.assertEqual(second.returncode, 0, msg=second.stderr)
            wrapper = fixture.run_wrapper()
            self.assertEqual(wrapper.returncode, 7, msg=wrapper.stderr)
            self.assertIn("version=0.97.0", wrapper.stdout)
            self.assert_current_version(fixture.home, "0.97.0")

    def test_marker_mismatched_version_directory_is_replaced(self) -> None:
        with self.install_fixture() as fixture:
            first = fixture.run_install("0.96.0")
            self.assertEqual(first.returncode, 0, msg=first.stderr)
            version_dir = fixture.home / ".mcodex" / "install" / "0.96.0"
            marker = version_dir / ".mcodex-install-complete.json"
            marker_data = json.loads(marker.read_text(encoding="utf-8"))
            marker_data["sha256"] = "broken"
            marker.write_text(json.dumps(marker_data), encoding="utf-8")
            junk = version_dir / "junk.txt"
            junk.write_text("stale\n", encoding="utf-8")
            inode_before = version_dir.stat().st_ino
            second = fixture.run_install("0.96.0")
            self.assertEqual(second.returncode, 0, msg=second.stderr)
            inode_after = version_dir.stat().st_ino
            self.assertNotEqual(inode_before, inode_after)
            self.assertFalse(junk.exists())
            self.assert_current_version(fixture.home, "0.96.0")

    def test_checksum_mismatch_does_not_switch_current(self) -> None:
        with self.install_fixture(corrupt_release="0.97.0") as fixture:
            first = fixture.run_install("0.96.0")
            self.assertEqual(first.returncode, 0, msg=first.stderr)
            second = fixture.run_install("0.97.0")
            self.assertNotEqual(second.returncode, 0)
            self.assertIn("checksum", second.stderr.lower())
            self.assert_current_version(fixture.home, "0.96.0")
            wrapper = fixture.run_wrapper()
            self.assertEqual(wrapper.returncode, 7, msg=wrapper.stderr)
            self.assertIn("version=0.96.0", wrapper.stdout)

    def test_install_fails_if_current_path_is_real_directory(self) -> None:
        with self.install_fixture() as fixture:
            current_path = fixture.home / ".mcodex" / "current"
            current_path.mkdir(parents=True)
            sentinel = current_path / "sentinel.txt"
            sentinel.write_text("keep me\n", encoding="utf-8")

            result = fixture.run_install("0.96.0")

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("exists and is not a symlink", result.stderr)
            self.assertTrue(current_path.is_dir())
            self.assertFalse(current_path.is_symlink())
            self.assertTrue(sentinel.exists())

    def test_wrapper_defaults_to_installed_custom_root(self) -> None:
        with self.install_fixture() as fixture:
            install_root = fixture.root / "custom-mcodex-root"
            result = fixture.run_install("0.96.0", install_root=install_root)
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            wrapper = fixture.run_wrapper()
            self.assertEqual(wrapper.returncode, 7, msg=wrapper.stderr)
            self.assertIn("version=0.96.0", wrapper.stdout)
            self.assertIn(f"root={install_root}", wrapper.stdout)

    def test_json_metadata_and_profile_escape_literal_paths(self) -> None:
        with self.install_fixture() as fixture:
            install_root = fixture.root / 'root"with\\slashes$and-dollar' / ".mcodex"
            wrapper_dir = fixture.root / 'bin"with\\slashes$and-dollar'
            result = fixture.run_install(
                "0.96.0",
                install_root=install_root,
                wrapper_dir=wrapper_dir,
            )
            self.assertEqual(result.returncode, 0, msg=result.stderr)

            metadata = json.loads((install_root / "install.json").read_text(encoding="utf-8"))
            self.assertEqual(metadata["baseRoot"], str(install_root))
            self.assertEqual(metadata["versionsDir"], str(install_root / "install"))
            self.assertEqual(metadata["currentLink"], str(install_root / "current"))
            self.assertEqual(metadata["wrapperPath"], str(wrapper_dir / "mcodex"))

            profile_path = fixture.home / ".zshrc"
            self.assertTrue(profile_path.exists())
            sourced = subprocess.run(
                [
                    "sh",
                    "-c",
                    '. "$HOME/.zshrc"; printf "%s\\n" "$PATH"',
                ],
                cwd=REPO_ROOT,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=_wrapper_env(fixture.home, path="/usr/bin:/bin"),
            )
            self.assertEqual(sourced.returncode, 0, msg=sourced.stderr)
            first_path_entry = sourced.stdout.rstrip("\n").split(":", 1)[0]
            self.assertEqual(first_path_entry, str(wrapper_dir))
            wrapper = fixture.run_wrapper(wrapper_dir=wrapper_dir)
            self.assertEqual(wrapper.returncode, 7, msg=wrapper.stderr)
            self.assertIn(f"root={install_root}", wrapper.stdout)

    def test_failed_repair_after_backup_restores_old_version_dir(self) -> None:
        with self.install_fixture() as fixture:
            first = fixture.run_install("0.96.0")
            self.assertEqual(first.returncode, 0, msg=first.stderr)
            version_dir = fixture.home / ".mcodex" / "install" / "0.96.0"
            sentinel = version_dir / "sentinel.txt"
            sentinel.write_text("keep me\n", encoding="utf-8")
            marker = version_dir / ".mcodex-install-complete.json"
            marker_data = json.loads(marker.read_text(encoding="utf-8"))
            marker_data["sha256"] = "broken"
            marker.write_text(json.dumps(marker_data), encoding="utf-8")

            failed = fixture.run_install(
                "0.96.0",
                extra_env={"MCODEX_TEST_FAIL_AFTER_BACKUP": "1"},
            )
            self.assertNotEqual(failed.returncode, 0)
            self.assertTrue(sentinel.exists())
            self.assertTrue((version_dir / "bin" / "mcodex").exists())
            self.assert_current_version(fixture.home, "0.96.0")
            wrapper = fixture.run_wrapper()
            self.assertEqual(wrapper.returncode, 7, msg=wrapper.stderr)
            self.assertIn("version=0.96.0", wrapper.stdout)

    def assert_latest_install_layout(self, home: Path, version: str) -> None:
        version_dir = home / ".mcodex" / "install" / version
        current_link = home / ".mcodex" / "current"
        wrapper_path = home / ".local" / "bin" / "mcodex"
        metadata_path = home / ".mcodex" / "install.json"
        self.assertTrue((version_dir / "bin" / "mcodex").exists())
        self.assertTrue((current_link / "bin" / "mcodex").exists())
        self.assertTrue(wrapper_path.exists())
        self.assertTrue(metadata_path.exists())
        self.assertTrue(current_link.is_symlink())
        self.assertEqual(current_link.resolve(), version_dir.resolve())
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
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
                "currentVersion": version,
                "baseRoot": str(home / ".mcodex"),
                "versionsDir": str(home / ".mcodex" / "install"),
                "currentLink": str(home / ".mcodex" / "current"),
                "wrapperPath": str(wrapper_path),
            },
        )
        self.assertRegex(metadata["installedAt"], r"^20[0-9]{2}-[0-9]{2}-[0-9]{2}T")
        marker = json.loads(
            (version_dir / ".mcodex-install-complete.json").read_text(encoding="utf-8")
        )
        self.assertEqual(marker["version"], version)
        self.assertEqual(marker["archiveName"], ARCHIVE_NAME)
        self.assertRegex(marker["installedAt"], r"^20[0-9]{2}-[0-9]{2}-[0-9]{2}T")
        wrapper_text = wrapper_path.read_text(encoding="utf-8")
        self.assertIn('target="$base_root/current/bin/mcodex"', wrapper_text)
        self.assertIn("export MCODEX_INSTALL_MANAGED=1", wrapper_text)
        self.assertIn("export MCODEX_INSTALL_METHOD=script", wrapper_text)
        self.assertIn('export MCODEX_INSTALL_ROOT="$base_root"', wrapper_text)
        self.assertIn('export PATH="$base_root/current/bin:$PATH"', wrapper_text)
        profile_path = home / ".zshrc"
        self.assertTrue(profile_path.exists())
        self.assertIn(
            f'export PATH="{home / ".local" / "bin"}:$PATH"',
            profile_path.read_text(encoding="utf-8"),
        )
        wrapper = subprocess.run(
            [str(wrapper_path)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=_wrapper_env(home),
            cwd=REPO_ROOT,
        )
        self.assertEqual(wrapper.returncode, 7, msg=wrapper.stderr)
        self.assertIn(f"version={version}", wrapper.stdout)
        self.assertIn("managed=1", wrapper.stdout)
        self.assertIn("method=script", wrapper.stdout)
        self.assertIn(f"root={home / '.mcodex'}", wrapper.stdout)
        self.assertIn(f"path={home / '.mcodex' / 'current' / 'bin'}:", wrapper.stdout)

    def assert_current_version(self, home: Path, version: str) -> None:
        self.assertEqual(
            (home / ".mcodex" / "current").resolve(),
            (home / ".mcodex" / "install" / version).resolve(),
        )

    def install_fixture(self, *, corrupt_release: str | None = None) -> "_InstallFixture":
        return _InstallFixture(corrupt_release=corrupt_release)


class _InstallFixture:
    def __init__(self, *, corrupt_release: str | None = None) -> None:
        self._tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self._tempdir.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.localappdata = self.root / "LocalAppData"
        self.localappdata.mkdir()
        self.oss_root = self.root / "oss"
        self.repository = FakeOssRepository(self.oss_root)
        self.repository.add_release("0.96.0")
        self.repository.add_release("0.96.0-alpha.1")
        self.repository.add_release(
            "0.97.0",
            checksum_override=(
                "0" * 64 if corrupt_release == "0.97.0" else None
            ),
        )
        self.repository.set_latest("0.96.0")
        self.server = FixtureHttpServer(self.oss_root)

    @property
    def default_base_root(self) -> Path:
        return self.localappdata / "Mcodex"

    def __enter__(self) -> "_InstallFixture":
        self.server.start()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.server.close()
        self._tempdir.cleanup()

    def run_install(
        self,
        version: str,
        *,
        install_root: Path | None = None,
        wrapper_dir: Path | None = None,
        extra_env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        env = _install_env(self.home, self.server.base_url)
        if install_root is not None:
            env["MCODEX_INSTALL_ROOT"] = str(install_root)
        if wrapper_dir is not None:
            env["MCODEX_WRAPPER_DIR"] = str(wrapper_dir)
        if extra_env is not None:
            env.update(extra_env)
        return subprocess.run(
            ["sh", str(INSTALL_SH), version],
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            timeout=10,
        )

    def run_wrapper(
        self,
        *,
        wrapper_dir: Path | None = None,
    ) -> subprocess.CompletedProcess[str]:
        env = _wrapper_env(self.home)
        wrapper_path = self.home / ".local" / "bin" / "mcodex"
        if wrapper_dir is not None:
            wrapper_path = wrapper_dir / "mcodex"
        return subprocess.run(
            [str(wrapper_path)],
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )

    def run_install_ps1(
        self,
        version: str | None = None,
        *,
        install_root: Path | None = None,
        wrapper_dir: Path | None = None,
        extra_env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        env = self.powershell_env()
        if install_root is not None:
            env["MCODEX_INSTALL_ROOT"] = str(install_root)
        if wrapper_dir is not None:
            env["MCODEX_WRAPPER_DIR"] = str(wrapper_dir)
        if extra_env is not None:
            env.update(extra_env)

        if version is None:
            command = f"& '{INSTALL_PS1}'"
        else:
            command = f"& '{INSTALL_PS1}' '{version}'"

        return run_powershell(command, env=env, timeout=20)

    def run_wrapper_ps1(
        self,
        *,
        wrapper_dir: Path | None = None,
        extra_env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        env = self.powershell_env()
        wrapper_dir = wrapper_dir or self.localappdata / "Programs" / "Mcodex" / "bin"
        env["PATH"] = str(wrapper_dir) + os.pathsep + env.get("PATH", "")
        if extra_env is not None:
            env.update(extra_env)
        command = (
            "mcodex -NoLogo -NoProfile -Command "
            "\"& { "
            "$version = (Get-Content -Raw (Join-Path $env:MCODEX_INSTALL_ROOT 'current\\bin\\version.txt')).Trim(); "
            "Write-Output ('version=' + $version); "
            "Write-Output ('managed=' + $env:MCODEX_INSTALL_MANAGED); "
            "Write-Output ('method=' + $env:MCODEX_INSTALL_METHOD); "
            "Write-Output ('root=' + $env:MCODEX_INSTALL_ROOT); "
            "Write-Output ('path=' + $env:Path); "
            "exit 7 }\""
        )
        return run_powershell(command, env=env, timeout=20)

    def powershell_env(self) -> dict[str, str]:
        return _powershell_env(self.localappdata, self.server.base_url)


def _find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _install_env(home: Path, base_url: str) -> dict[str, str]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["SHELL"] = "/bin/zsh"
    env["MCODEX_DOWNLOAD_BASE_URL"] = base_url
    env["MCODEX_TEST_UNAME_S"] = "Linux"
    env["MCODEX_TEST_UNAME_M"] = "x86_64"
    env.pop("MCODEX_INSTALL_ROOT", None)
    env.pop("MCODEX_WRAPPER_DIR", None)
    return env


def _wrapper_env(home: Path, *, path: str | None = None) -> dict[str, str]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    if path is not None:
        env["PATH"] = path
    env.pop("MCODEX_INSTALL_ROOT", None)
    env.pop("MCODEX_WRAPPER_DIR", None)
    return env


def _powershell_env(localappdata: Path, base_url: str) -> dict[str, str]:
    env = os.environ.copy()
    env["LOCALAPPDATA"] = str(localappdata)
    env["TEMP"] = str(localappdata / "Temp")
    env["TMP"] = env["TEMP"]
    env["MCODEX_DOWNLOAD_BASE_URL"] = base_url
    env["MCODEX_SKIP_USER_PATH_REGISTRY"] = "1"
    env.pop("MCODEX_INSTALL_ROOT", None)
    env.pop("MCODEX_WRAPPER_DIR", None)
    Path(env["TEMP"]).mkdir(parents=True, exist_ok=True)
    return env


def _powershell_single_quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


if __name__ == "__main__":
    unittest.main()
