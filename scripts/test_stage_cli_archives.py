import unittest
import tempfile
import tarfile
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
            )

            self.assertEqual(resolved.read_text(encoding="utf-8"), "#!/bin/sh\n")


if __name__ == "__main__":
    unittest.main()
