#!/usr/bin/env python3
"""Stage OSS-native mcodex CLI release archives."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path


DOWNLOAD_BASE_URL = "https://downloads.mcodex.sota.wiki"
RELEASE_PREFIX = "repositories/mcodex"
UNIX_INSTALL_COMMAND = f"curl -fsSL {DOWNLOAD_BASE_URL}/install.sh | sh"
WINDOWS_INSTALL_COMMAND = (
    "powershell -NoProfile -ExecutionPolicy Bypass -Command "
    f'"iwr -UseBasicParsing {DOWNLOAD_BASE_URL}/install.ps1 -OutFile '
    '$env:TEMP\\mcodex-install.ps1; & $env:TEMP\\mcodex-install.ps1"'
)

TARGET_TO_PLATFORM = {
    "aarch64-apple-darwin": ("darwin", "arm64"),
    "x86_64-apple-darwin": ("darwin", "x64"),
    "aarch64-unknown-linux-musl": ("linux", "arm64"),
    "x86_64-unknown-linux-musl": ("linux", "x64"),
    "aarch64-pc-windows-msvc": ("win32", "arm64"),
    "x86_64-pc-windows-msvc": ("win32", "x64"),
}

UNIX_ARCHIVE_MEMBERS = sorted(["bin/mcodex", "bin/rg"])
WINDOWS_ARCHIVE_MEMBERS = sorted(
    [
        "bin/mcodex.exe",
        "bin/rg.exe",
        "bin/codex-command-runner.exe",
        "bin/codex-windows-sandbox-setup.exe",
    ]
)
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$")


def archive_name_for_platform(os_name: str, arch: str) -> str:
    suffix = "zip" if os_name == "win32" else "tar.gz"
    return f"mcodex-{os_name}-{arch}.{suffix}"


def build_latest_manifest(version: str, release_tag: str, published_at: str) -> dict:
    return {
        "product": "mcodex",
        "channel": "stable",
        "version": version,
        "publishedAt": published_at,
        "notesUrl": f"https://github.com/vivym/mcodex/releases/tag/{release_tag}",
        "checksumsUrl": (
            f"{DOWNLOAD_BASE_URL}/{RELEASE_PREFIX}/releases/{version}/SHA256SUMS"
        ),
        "install": {
            "unix": UNIX_INSTALL_COMMAND,
            "windows": WINDOWS_INSTALL_COMMAND,
        },
    }


def create_unix_archive(src_dir: Path, out_dir: Path, archive_name: str) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    archive_path = out_dir / archive_name
    with tarfile.open(archive_path, "w:gz") as archive:
        for path in _iter_files(src_dir):
            archive.add(path, arcname=path.relative_to(src_dir).as_posix(), recursive=False)
    return archive_path


def create_windows_archive(src_dir: Path, out_dir: Path, archive_name: str) -> Path:
    out_dir.mkdir(parents=True, exist_ok=True)
    archive_path = out_dir / archive_name
    with zipfile.ZipFile(
        archive_path,
        mode="w",
        compression=zipfile.ZIP_DEFLATED,
    ) as archive:
        for path in _iter_files(src_dir):
            archive.write(path, arcname=path.relative_to(src_dir).as_posix())
    return archive_path


def write_sha256sums(paths: list[Path], output: Path) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    lines = []
    for path in sorted(paths, key=lambda candidate: candidate.name):
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        lines.append(f"{digest}  {path.name}")
    output.write_text("\n".join(lines) + "\n", encoding="utf-8")


def list_archive_members(path: Path) -> list[str]:
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "r:gz") as archive:
            return sorted(member.name for member in archive.getmembers() if member.isfile())

    if path.suffix == ".zip":
        with zipfile.ZipFile(path) as archive:
            return sorted(
                name for name in archive.namelist() if not name.endswith("/")
            )

    raise RuntimeError(f"Unsupported archive format: {path}")


def resolve_release_binary(
    artifacts_dir: Path,
    target: str,
    binary_name: str,
    scratch_dir: Path,
) -> Path:
    target_dir = artifacts_dir / target
    if not target_dir.exists():
        raise FileNotFoundError(f"Target artifact directory not found: {target_dir}")

    is_windows = "windows" in target
    for candidate in _raw_binary_candidates(target_dir, target, binary_name, is_windows):
        if candidate.is_file():
            return candidate

    for candidate in _archive_candidates(target_dir, target, binary_name, is_windows):
        if candidate.is_file():
            return _extract_release_archive(
                candidate,
                scratch_dir / target,
                target,
                binary_name,
            )

    raise FileNotFoundError(
        f"Unable to resolve release binary '{binary_name}' for target '{target}' in {target_dir}"
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="Normalized semver release version.")
    parser.add_argument("--release-tag", required=True, help="Git tag for release notes.")
    parser.add_argument("--published-at", required=True, help="ISO 8601 publication timestamp.")
    parser.add_argument(
        "--artifacts-dir",
        type=Path,
        default=Path("dist"),
        help="Directory containing downloaded release artifacts by Rust target.",
    )
    parser.add_argument(
        "--vendor-src",
        type=Path,
        default=Path("dist/vendor"),
        help="Directory containing ripgrep binaries in vendor/<target>/path/ layout.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="Directory to write staged release archives and SHA256SUMS.",
    )
    parser.add_argument(
        "--manifest-output",
        type=Path,
        default=None,
        help="Optional path for channels/stable/latest.json.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    _validate_version(args.version)
    if _is_prerelease(args.version) and args.manifest_output is not None:
        raise SystemExit("--manifest-output is only valid for stable release versions.")

    artifacts_dir = args.artifacts_dir.resolve()
    vendor_src = args.vendor_src.resolve()
    output_dir = (
        args.output_dir.resolve()
        if args.output_dir is not None
        else (Path("dist") / "oss" / RELEASE_PREFIX / "releases" / args.version).resolve()
    )

    archives = []
    for target, (os_name, arch) in TARGET_TO_PLATFORM.items():
        archives.append(
            _stage_release_archive(
                artifacts_dir=artifacts_dir,
                vendor_src=vendor_src,
                output_dir=output_dir,
                target=target,
                os_name=os_name,
                arch=arch,
            )
        )

    write_sha256sums(archives, output_dir / "SHA256SUMS")

    if args.manifest_output is not None:
        manifest_output = args.manifest_output.resolve()
        manifest_output.parent.mkdir(parents=True, exist_ok=True)
        manifest_output.write_text(
            json.dumps(
                build_latest_manifest(
                    version=args.version,
                    release_tag=args.release_tag,
                    published_at=args.published_at,
                ),
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )

    return 0


def _validate_version(version: str) -> None:
    if not SEMVER_RE.fullmatch(version):
        raise SystemExit(f"Version must be normalized semver, got '{version}'.")


def _is_prerelease(version: str) -> bool:
    return "-" in version


def _iter_files(src_dir: Path):
    for path in sorted(src_dir.rglob("*")):
        if path.is_file():
            yield path


def _stage_release_archive(
    *,
    artifacts_dir: Path,
    vendor_src: Path,
    output_dir: Path,
    target: str,
    os_name: str,
    arch: str,
) -> Path:
    is_windows = os_name == "win32"
    archive_name = archive_name_for_platform(os_name, arch)
    expected_members = WINDOWS_ARCHIVE_MEMBERS if is_windows else UNIX_ARCHIVE_MEMBERS

    with tempfile.TemporaryDirectory(
        prefix=f"stage-cli-{target}-"
    ) as staging_dir_str, tempfile.TemporaryDirectory(
        prefix=f"stage-cli-resolved-{target}-"
    ) as scratch_dir_str:
        staging_dir = Path(staging_dir_str)
        scratch_dir = Path(scratch_dir_str)
        bin_dir = staging_dir / "bin"
        bin_dir.mkdir(parents=True, exist_ok=True)

        _copy_release_binary(
            resolve_release_binary(artifacts_dir, target, "mcodex", scratch_dir),
            bin_dir / ("mcodex.exe" if is_windows else "mcodex"),
            make_executable=not is_windows,
        )
        _copy_vendor_rg(vendor_src, target, bin_dir, is_windows)

        if is_windows:
            for binary_name in (
                "codex-command-runner",
                "codex-windows-sandbox-setup",
            ):
                _copy_release_binary(
                    resolve_release_binary(artifacts_dir, target, binary_name, scratch_dir),
                    bin_dir / f"{binary_name}.exe",
                )

        archive_path = (
            create_windows_archive(staging_dir, output_dir, archive_name)
            if is_windows
            else create_unix_archive(staging_dir, output_dir, archive_name)
        )

    actual_members = list_archive_members(archive_path)
    if actual_members != expected_members:
        raise RuntimeError(
            f"Archive {archive_path} had unexpected members: {actual_members!r}"
        )

    return archive_path


def _copy_release_binary(source: Path, dest: Path, *, make_executable: bool = False) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, dest)
    if make_executable:
        dest.chmod(0o755)


def _copy_vendor_rg(vendor_src: Path, target: str, bin_dir: Path, is_windows: bool) -> None:
    rg_name = "rg.exe" if is_windows else "rg"
    rg_source = vendor_src / target / "path" / rg_name
    if not rg_source.exists():
        raise FileNotFoundError(f"ripgrep vendor binary not found: {rg_source}")

    _copy_release_binary(rg_source, bin_dir / rg_name, make_executable=not is_windows)


def _raw_binary_candidates(
    target_dir: Path,
    target: str,
    binary_name: str,
    is_windows: bool,
) -> list[Path]:
    if is_windows:
        names = [
            f"{binary_name}.exe",
            f"{binary_name}-{target}.exe",
            binary_name,
            f"{binary_name}-{target}",
        ]
    else:
        names = [binary_name, f"{binary_name}-{target}"]

    return [target_dir / name for name in _unique(names)]


def _archive_candidates(
    target_dir: Path,
    target: str,
    binary_name: str,
    is_windows: bool,
) -> list[Path]:
    stems = []
    if is_windows:
        stems.extend(
            [
                f"{binary_name}-{target}.exe",
                f"{binary_name}-{target}",
                f"{binary_name}.exe",
                binary_name,
            ]
        )
    else:
        stems.extend([f"{binary_name}-{target}", binary_name])

    candidates = []
    for stem in _unique(stems):
        candidates.extend(
            [
                target_dir / f"{stem}.tar.gz",
                target_dir / f"{stem}.zip",
                target_dir / f"{stem}.zst",
            ]
        )
    return candidates


def _extract_release_archive(
    archive_path: Path,
    output_dir: Path,
    target: str,
    binary_name: str,
) -> Path:
    output_dir.mkdir(parents=True, exist_ok=True)
    is_windows = "windows" in target
    dest_name = f"{binary_name}.exe" if is_windows else binary_name
    dest = output_dir / dest_name
    dest.unlink(missing_ok=True)

    if archive_path.name.endswith(".tar.gz"):
        with tarfile.open(archive_path, "r:gz") as archive:
            members = [member for member in archive.getmembers() if member.isfile()]
            member = _select_archive_member(
                [entry.name for entry in members],
                target,
                binary_name,
            )
            extracted = archive.extractfile(member)
            if extracted is None:
                raise RuntimeError(f"Unable to extract {member} from {archive_path}")
            with extracted, open(dest, "wb") as out:
                shutil.copyfileobj(extracted, out)
    elif archive_path.suffix == ".zip":
        with zipfile.ZipFile(archive_path) as archive:
            names = [name for name in archive.namelist() if not name.endswith("/")]
            member = _select_archive_member(names, target, binary_name)
            with archive.open(member) as src, open(dest, "wb") as out:
                shutil.copyfileobj(src, out)
    elif archive_path.suffix == ".zst":
        subprocess.check_call(["zstd", "-f", "-d", str(archive_path), "-o", str(dest)])
    else:
        raise RuntimeError(f"Unsupported release archive format: {archive_path}")

    if not is_windows:
        dest.chmod(0o755)
    return dest


def _select_archive_member(members: list[str], target: str, binary_name: str) -> str:
    basenames = {
        binary_name,
        f"{binary_name}.exe",
        f"{binary_name}-{target}",
        f"{binary_name}-{target}.exe",
    }
    matches = [member for member in members if Path(member).name in basenames]
    if len(matches) == 1:
        return matches[0]
    if not matches and len(members) == 1:
        return members[0]
    if not matches:
        raise RuntimeError(f"Unable to determine archive member from {members!r}")
    raise RuntimeError(f"Ambiguous archive members for {binary_name}: {matches!r}")


def _unique(values: list[str]) -> list[str]:
    seen = set()
    result = []
    for value in values:
        if value in seen:
            continue
        seen.add(value)
        result.append(value)
    return result


if __name__ == "__main__":
    raise SystemExit(main())
