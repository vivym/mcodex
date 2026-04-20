from __future__ import annotations

import hashlib
import json
import os
import socket
import subprocess
import tarfile
import tempfile
import time
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
INSTALL_SH = REPO_ROOT / "scripts" / "install" / "install.sh"
LATEST_MANIFEST_PATH = Path("repositories/mcodex/channels/stable/latest.json")
ARCHIVE_NAME = "mcodex-linux-x64.tar.gz"


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
        archive_path = release_dir / ARCHIVE_NAME
        self._create_archive(archive_path, version)
        sha256 = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        checksum = checksum_override or sha256
        (release_dir / "SHA256SUMS").write_text(
            f"{checksum}  {ARCHIVE_NAME}\n",
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

    def __enter__(self) -> "_InstallFixture":
        self.server.start()
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        self.server.close()
        self._tempdir.cleanup()

    def run_install(self, version: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["sh", str(INSTALL_SH), version],
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=_install_env(self.home, self.server.base_url),
            timeout=10,
        )

    def run_wrapper(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(self.home / ".local" / "bin" / "mcodex")],
            cwd=REPO_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=_wrapper_env(self.home),
        )


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


def _wrapper_env(home: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["HOME"] = str(home)
    return env


if __name__ == "__main__":
    unittest.main()
