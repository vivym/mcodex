## Releasing mcodex

This document describes the maintainer release flow for the `mcodex` CLI
cutover to OSS-hosted native archives.

### Distribution model

`mcodex` now ships through three channels with different roles:

- Aliyun OSS at `https://downloads.mcodex.sota.wiki` is the only binary
  distribution channel for the CLI.
- GitHub Releases remain lightweight release records with notes, checksums,
  signatures, installer scripts, and non-CLI npm tarballs.
- npm remains only for non-CLI packages:
  - `codex-sdk`
  - `codex-responses-api-proxy`

The CLI is no longer published through npm, Bun, Homebrew, WinGet, or native
GitHub Release binary attachments.

### Release inputs

Before cutting a tag, confirm:

- The target version in `codex-rs/Cargo.toml` already matches the version you
  want to release.
- `release/minisign.pub` is committed.
- GitHub Actions secret `MINISIGN_PRIVATE_KEY_B64` contains the base64-encoded
  private key matching `release/minisign.pub`.
- Aliyun OSS secrets are configured in GitHub Actions:
  - `ALIYUN_ACCESS_KEY_ID`
  - `ALIYUN_ACCESS_KEY_SECRET`
  - `ALIYUN_OSS_ENDPOINT`
  - `ALIYUN_OSS_BUCKET`

### Tag format

The release workflow triggers on `rust-v*.*.*`.

Supported tags:

- Stable: `rust-v0.96.0`
- Prerelease: `rust-v0.96.0-alpha.1`
- Prerelease: `rust-v0.96.0-beta.1`

The tag must match the Cargo workspace version or the workflow fails in the
`tag-check` job.

### Cutting a release

Create and push an annotated tag:

```bash
git tag -a rust-v0.96.0 -m "Release 0.96.0"
git push origin rust-v0.96.0
```

For a prerelease:

```bash
git tag -a rust-v0.96.0-alpha.1 -m "Release 0.96.0-alpha.1"
git push origin rust-v0.96.0-alpha.1
```

### What the workflow publishes

The release workflow is implemented in
[`rust-release.yml`](../.github/workflows/rust-release.yml).

It derives a release contract from the tag:

- `release_version`: normalized semver, for example `0.96.0`
- `release_tag`: canonical git tag, for example `rust-v0.96.0`
- `stable_release_version`: set only for stable tags
- `release_is_prerelease`: `true` for alpha/beta tags

It then:

1. Builds platform binaries on macOS, Linux, and Windows.
2. Stages native CLI archives with `scripts/stage_cli_archives.py`.
3. Produces `SHA256SUMS` for each release directory.
4. Signs `SHA256SUMS` with minisign and verifies the signature against
   `release/minisign.pub`.
5. Uploads versioned CLI archives to OSS.
6. Creates a lightweight GitHub Release.
7. Publishes only non-CLI npm tarballs.

### OSS layout

CLI archives are published under:

```text
https://downloads.mcodex.sota.wiki/repositories/mcodex/
  channels/
    stable/latest.json
  releases/
    0.96.0/
      mcodex-darwin-arm64.tar.gz
      mcodex-darwin-x64.tar.gz
      mcodex-linux-arm64.tar.gz
      mcodex-linux-x64.tar.gz
      mcodex-win32-arm64.zip
      mcodex-win32-x64.zip
      SHA256SUMS
      SHA256SUMS.sig
```

The installer scripts fetch from that layout directly.

### Upload ordering

Upload order matters and is enforced in the workflow:

1. Upload the versioned release directory to OSS for every tag.
2. For stable releases only, upload root `install.sh` and `install.ps1`.
3. For stable releases only, upload `channels/stable/latest.json` last.

This ordering prevents `latest.json` from pointing at artifacts that have not
finished uploading yet.

Prereleases still publish versioned artifacts and signed checksums, but they do
not update the stable channel and do not replace the root installer scripts.

### GitHub Release contents

GitHub Releases no longer carry native CLI archives.

Attached assets are intentionally lightweight:

- `dist/oss/repositories/mcodex/releases/<version>/SHA256SUMS`
- `dist/oss/repositories/mcodex/releases/<version>/SHA256SUMS.sig`
- `dist/install.sh`
- `dist/install.ps1`
- `dist/config-schema.json`
- `dist/npm/codex-responses-api-proxy-npm-<version>.tgz`
- `dist/npm/codex-sdk-npm-<version>.tgz`

The release notes append:

- OSS installer commands
- cutover guidance for former npm CLI users
- prerelease install guidance
- the literal minisign public key from `release/minisign.pub`

### Stable vs prerelease behavior

Stable tags:

- publish versioned OSS artifacts
- upload root `install.sh` and `install.ps1`
- update `channels/stable/latest.json`
- create a normal GitHub Release
- may publish non-CLI npm packages with the default npm tag

Prerelease tags:

- publish versioned OSS artifacts
- sign and publish checksums
- skip root installer upload
- skip `channels/stable/latest.json`
- create a GitHub prerelease
- may publish non-CLI npm packages only when the tag format matches the npm
  prerelease policy in the workflow

### Installer commands advertised to users

macOS/Linux:

```bash
curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh
```

Windows PowerShell:

```powershell
$installer = Join-Path $env:TEMP "mcodex-install.ps1"
iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $installer
& $installer
```

Explicit prerelease or pinned-version install uses the same scripts with a
version argument.

### Post-release checks

After the workflow succeeds, verify:

- The GitHub Release exists and has only lightweight assets.
- The release notes include the OSS install commands and minisign public key.
- The versioned OSS release directory exists and contains archives,
  `SHA256SUMS`, and `SHA256SUMS.sig`.
- For stable releases, `https://downloads.mcodex.sota.wiki/install.sh`,
  `install.ps1`, and `repositories/mcodex/channels/stable/latest.json` point
  at the new stable release.
- For prereleases, stable `latest.json` and root installer scripts remain
  unchanged.
- Non-CLI npm packages, if expected for that tag, were published successfully.

### Manual smoke checks

These checks are not fully covered by local fixture tests and should be run on
real published artifacts:

- macOS or Linux latest install from the hosted `install.sh`
- macOS or Linux pinned install, then upgrade to a newer version
- Windows PowerShell latest install from the hosted `install.ps1`
- Windows PowerShell pinned install, then upgrade to a newer version
- repair flow after corrupting the install marker
- TUI update prompt opening the GitHub release notes URL from `latest.json`

### Notes

- Installers verify archive SHA256 via `SHA256SUMS`.
- Installers do not verify `SHA256SUMS.sig` in this slice.
- If the minisign key ever rotates, update `release/minisign.pub`, replace
  `MINISIGN_PRIVATE_KEY_B64`, and ensure release notes reference the new key.
