import json
import re
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import scripts.stage_npm_packages as stage


build = stage._BUILD_MODULE

EXPECTED_BLOCKED_CLI_NPM_PACKAGES = {
    "codex",
    "codex-linux-x64",
    "codex-linux-arm64",
    "codex-darwin-x64",
    "codex-darwin-arm64",
    "codex-win32-x64",
    "codex-win32-arm64",
}
ALLOWED_PACKAGES = ["codex-sdk", "codex-responses-api-proxy"]


class StageNpmPackagesTests(unittest.TestCase):
    def test_blocked_cli_package_policy_matches_expected_contract(self) -> None:
        self.assertEqual(stage.BLOCKED_CLI_NPM_PACKAGES, EXPECTED_BLOCKED_CLI_NPM_PACKAGES)
        self.assertEqual(build.BLOCKED_CLI_NPM_PACKAGES, EXPECTED_BLOCKED_CLI_NPM_PACKAGES)
        self.assertEqual(
            stage.CLI_NPM_PACKAGE_BLOCKED_ERROR,
            build.CLI_NPM_PACKAGE_BLOCKED_ERROR,
        )

    def test_expand_packages_rejects_requested_cli_packages(self) -> None:
        for package in sorted(EXPECTED_BLOCKED_CLI_NPM_PACKAGES):
            with self.subTest(package=package):
                with self.assertRaisesRegex(
                    RuntimeError,
                    re.escape(stage.CLI_NPM_PACKAGE_BLOCKED_ERROR),
                ):
                    stage.expand_packages([package])

    def test_expand_packages_rejects_blocked_expansions(self) -> None:
        with mock.patch.dict(
            stage.PACKAGE_EXPANSIONS,
            {"meta-package": ["codex-sdk", "codex-linux-x64"]},
            clear=False,
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                re.escape(stage.CLI_NPM_PACKAGE_BLOCKED_ERROR),
            ):
                stage.expand_packages(["meta-package"])

    def test_expand_packages_allows_non_cli_packages(self) -> None:
        self.assertEqual(stage.expand_packages(ALLOWED_PACKAGES), ALLOWED_PACKAGES)


class BuildNpmPackageTests(unittest.TestCase):
    def test_build_script_rejects_cli_packages(self) -> None:
        for package in sorted(EXPECTED_BLOCKED_CLI_NPM_PACKAGES):
            with self.subTest(package=package):
                with self.assertRaisesRegex(
                    RuntimeError,
                    re.escape(build.CLI_NPM_PACKAGE_BLOCKED_ERROR),
                ):
                    build.raise_for_blocked_cli_npm_package(package)

    def test_build_script_allows_non_cli_packages(self) -> None:
        for package in ALLOWED_PACKAGES:
            with self.subTest(package=package):
                self.assertIsNone(build.raise_for_blocked_cli_npm_package(package))

    def test_stage_sources_for_codex_sdk_does_not_inject_codex_dependency(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sdk_root = root / "sdk"
            sdk_root.mkdir()
            staging_dir = root / "staging"
            staging_dir.mkdir()

            (sdk_root / "package.json").write_text(
                json.dumps(
                    {
                        "name": "codex-sdk",
                        "version": "0.0.0",
                        "scripts": {
                            "prepare": "pnpm run build",
                            "test": "vitest run",
                        },
                        "dependencies": {
                            "zod": "^3.0.0",
                        },
                    }
                ),
                encoding="utf-8",
            )

            with (
                mock.patch.object(build, "CODEX_SDK_ROOT", sdk_root),
                mock.patch.object(build, "stage_codex_sdk_sources") as stage_codex_sdk_sources,
            ):
                build.stage_sources(staging_dir, "1.2.3", "codex-sdk")

            stage_codex_sdk_sources.assert_called_once_with(staging_dir)
            staged_package_json = json.loads((staging_dir / "package.json").read_text())
            self.assertEqual(
                staged_package_json,
                {
                    "name": "codex-sdk",
                    "version": "1.2.3",
                    "scripts": {
                        "test": "vitest run",
                    },
                    "dependencies": {
                        "zod": "^3.0.0",
                    },
                },
            )


if __name__ == "__main__":
    unittest.main()
