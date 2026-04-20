import unittest
import io
import json
import sys
import tempfile
import tarfile
import zipfile
from pathlib import Path
from unittest import mock

import scripts.stage_cli_archives as stage


def _write_tar_gz_archive(archive_path: Path, member_name: str, contents: bytes) -> None:
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive_path, "w:gz") as archive:
        info = tarfile.TarInfo(name=member_name)
        info.size = len(contents)
        info.mode = 0o755
        archive.addfile(info, io.BytesIO(contents))


def _write_zip_archive(archive_path: Path, member_name: str, contents: bytes) -> None:
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(member_name, contents)


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

            self.assertEqual(stage.list_archive_members(archive), ["bin/mcodex", "bin/rg"])

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
                root / "scratch",
            )

            self.assertEqual(resolved.read_text(encoding="utf-8"), "#!/bin/sh\n")

    def test_main_stages_release_archives_without_leaking_extraction_scratch(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            dist = root / "dist"
            vendor_dir = dist / "vendor"
            version = "0.96.0"
            output_dir = dist / "oss" / stage.RELEASE_PREFIX / "releases" / version
            manifest_output = (
                dist / "oss" / stage.RELEASE_PREFIX / "channels" / "stable" / "latest.json"
            )
            expected_input_files: dict[str, list[str]] = {}

            for target, (os_name, _arch) in stage.TARGET_TO_PLATFORM.items():
                target_dir = dist / target
                target_dir.mkdir(parents=True, exist_ok=True)
                expected_input_files[target] = []

                is_windows = os_name == "win32"
                if is_windows:
                    for binary_name in (
                        "mcodex",
                        "codex-command-runner",
                        "codex-windows-sandbox-setup",
                    ):
                        archive_path = target_dir / f"{binary_name}-{target}.zip"
                        _write_zip_archive(
                            archive_path,
                            f"{binary_name}.exe",
                            f"{binary_name}-{target}\n".encode("utf-8"),
                        )
                        expected_input_files[target].append(archive_path.name)
                else:
                    archive_path = target_dir / f"mcodex-{target}.tar.gz"
                    _write_tar_gz_archive(
                        archive_path,
                        "mcodex",
                        f"{target}-mcodex\n".encode("utf-8"),
                    )
                    expected_input_files[target].append(archive_path.name)

                rg_name = "rg.exe" if is_windows else "rg"
                rg_path = vendor_dir / target / "path" / rg_name
                rg_path.parent.mkdir(parents=True, exist_ok=True)
                rg_path.write_bytes(f"{target}-{rg_name}\n".encode("utf-8"))

            argv = [
                "stage_cli_archives.py",
                "--version",
                version,
                "--release-tag",
                "rust-v0.96.0",
                "--published-at",
                "2026-04-20T12:00:00Z",
                "--artifacts-dir",
                str(dist),
                "--vendor-src",
                str(vendor_dir),
                "--output-dir",
                str(output_dir),
                "--manifest-output",
                str(manifest_output),
            ]
            with mock.patch.object(sys, "argv", argv):
                self.assertEqual(stage.main(), 0)

            expected_archives = {
                stage.archive_name_for_platform(os_name, arch)
                for os_name, arch in stage.TARGET_TO_PLATFORM.values()
            }
            self.assertEqual(
                {path.name for path in output_dir.iterdir()},
                expected_archives | {"SHA256SUMS"},
            )

            checksums = (output_dir / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
            self.assertEqual(
                sorted(line.rsplit("  ", 1)[1] for line in checksums),
                sorted(expected_archives),
            )

            manifest = json.loads(manifest_output.read_text(encoding="utf-8"))
            self.assertEqual(manifest["channel"], "stable")
            self.assertEqual(manifest["version"], version)

            for target, expected_files in expected_input_files.items():
                target_dir = dist / target
                self.assertFalse((target_dir / ".resolved").exists())
                self.assertEqual(
                    sorted(
                        path.relative_to(target_dir).as_posix()
                        for path in target_dir.rglob("*")
                        if path.is_file()
                    ),
                    sorted(expected_files),
                )

            windows_archive = output_dir / stage.archive_name_for_platform("win32", "x64")
            self.assertEqual(
                stage.list_archive_members(windows_archive),
                stage.WINDOWS_ARCHIVE_MEMBERS,
            )


if __name__ == "__main__":
    unittest.main()
