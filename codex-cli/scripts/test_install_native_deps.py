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

            with (
                mock.patch.object(install_native_deps, "RG_MANIFEST", bin_dir / "rg"),
                mock.patch.object(install_native_deps, "DEFAULT_RG_TARGETS", []),
                mock.patch.object(install_native_deps, "_download_artifacts") as download,
                mock.patch("subprocess.check_output") as check_output,
                mock.patch("sys.argv", ["install_native_deps.py", "--component", "rg", str(root)]),
            ):
                self.assertEqual(install_native_deps.main(), 0)

            download.assert_not_called()
            check_output.assert_not_called()


if __name__ == "__main__":
    unittest.main()
