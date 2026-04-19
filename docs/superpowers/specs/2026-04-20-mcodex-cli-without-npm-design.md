# mcodex CLI Without npm Packaging Design

## Summary

This document defines how `mcodex` CLI should stop using npm as its
distribution format while keeping installation and updates simple for domestic
users through OSS-hosted native archives and platform-specific install scripts.

Approved product decisions:

- CLI distribution stops using npm packaging and npm publish
- `install.sh` and `install.ps1` become the only supported install/update
  entrypoints for the CLI
- OSS/CDN becomes the sole binary distribution source
- GitHub Releases remain, but only as lightweight publication records
- The CLI ships native platform archives, not `codex-npm-*.tgz` packages
- PATH points to a thin launcher wrapper, not directly to the versioned binary
- Wrapper responsibilities stay minimal: locate the current install, inject
  install metadata env vars, prepend the managed `bin` dir to `PATH`, and
  exec the real binary
- Update prompts live only in Rust/TUI, not in shell or PowerShell wrappers
- This slice is stable-only; it does not introduce installer-selectable release
  channels
- Installers support both `latest` and explicit version installation
- Existing npm-installed users are not specially migrated; the cutover is
  clean and documented
- This change applies only to CLI distribution; repo-local TypeScript SDK and
  JS/TS tooling remain on npm/pnpm where they already belong

The recommended direction is to replace the current npm-based CLI packaging
layer with native `.tar.gz`/`.zip` release archives, teach the Rust runtime to
recognize script-managed installs, and move release distribution to OSS while
keeping GitHub Releases as the authoritative public release record and checksum
anchor.

## Problem

The current CLI install story still carries upstream npm packaging assumptions
even though the fork now wants:

- domestic-user-friendly downloads through OSS/CDN
- no npm dependency for CLI installation or updates
- lighter distribution surfaces that match the real runtime shape
- continued public version records through lightweight GitHub Releases

Today, the current branch still has mixed distribution behavior:

- installer scripts download `codex-npm-*.tgz` artifacts and extract files out
  of a package layout instead of consuming native CLI archives directly
- release workflow still stages and publishes npm packages for the CLI
- TUI update actions still assume npm, bun, or Homebrew as the primary managed
  install modes
- the Node launcher remains in the critical path for npm-installed CLI
  execution

This is not just stylistic drift. It leaves the fork with an unnecessary Node
packaging layer in front of a Rust-native CLI, keeps installer internals tied
to an upstream-oriented artifact format, and complicates the switch to an OSS
first distribution story.

## Goals

- Remove npm as a required distribution mechanism for the `mcodex` CLI.
- Make OSS/CDN the only binary download source used by install and update
  flows.
- Keep installation simple on macOS/Linux and Windows via maintained scripts.
- Keep update prompts inside Rust/TUI and make them point to script-based
  updates.
- Support both `latest` and explicit version installation through the same
  scripts.
- Keep versioned installs and current-version switching structured enough to
  support rollback and future self-update work.
- Preserve lightweight public release visibility through GitHub Releases.
- Keep the implementation shape friendly to continued upstream merging by
  concentrating changes in distribution and update edges.

## Non-Goals

- Do not remove npm/pnpm from the repository as a whole.
- Do not stop publishing or building the TypeScript SDK as an npm package.
- Do not design a full `mcodex self-update` command in this slice.
- Do not preserve a long-lived npm update path for already npm-installed CLI
  users.
- Do not implement a graphical installer or OS-native package manager support
  such as Homebrew tap publication or WinGet publication in this slice.
- Do not redesign unrelated runtime behavior, crate structure, or CLI command
  semantics.

## Constraints

- The user explicitly chose a clean cutover instead of a dual-path migration.
- The user wants a thin wrapper, not a second implementation of update logic in
  shell/PowerShell.
- Update prompts should live in Rust/TUI only.
- This slice is stable-only; channel selection and persistence are out of
  scope.
- OSS distribution must support both latest installs and explicit version pins.
- GitHub Releases should remain, but only as lightweight publication records.
- Domestic-user distribution quality matters more than keeping compatibility
  with upstream npm packaging conventions.
- Mergeability with upstream still matters, so the change should stay focused
  on distribution surfaces, not unrelated runtime internals.

## Approaches Considered

### Approach A: Keep CLI npm artifacts, but stop publishing them to npm

Continue producing the current `codex-npm-*.tgz` artifact layout, upload those
archives to OSS, and keep the existing installer extraction logic largely
unchanged.

Pros:

- Lowest short-term implementation effort
- Minimal installer changes
- Keeps current release staging logic mostly intact

Cons:

- Retains npm-shaped CLI artifacts even though npm is no longer the install
  channel
- Keeps the `codex-cli` wrapper layer and release staging complexity alive
- Preserves naming and directory shapes that no longer match the intended
  product story

This approach is rejected.

### Approach B: Native platform archives plus thin script-managed wrappers

Publish native `.tar.gz`/`.zip` archives for each CLI platform, install them
via `install.sh` and `install.ps1`, place a thin wrapper on `PATH`, and move
managed-update detection in Rust/TUI to a new script-managed installation mode.

Pros:

- Matches the Rust-native runtime shape directly
- Removes the Node packaging layer from the CLI user path
- Works naturally with OSS-hosted distribution
- Keeps wrappers simple and future-proof
- Supports versioned installs, rollback-friendly layout, and future self-update

Cons:

- Requires coordinated release, installer, and update-prompt changes
- Introduces a new managed-install identity path for the Rust runtime

This is the recommended approach.

### Approach C: Native platform archives with no wrapper layer

Install the real binary directly into a PATH directory and remove any wrapper
entrypoint entirely.

Pros:

- Smallest runtime indirection
- Fewer files in the visible PATH directory

Cons:

- Harder to inject managed-install metadata into the runtime consistently
- Makes future layout changes, rollback, and self-update work less flexible
- Couples the visible PATH binary to the versioned install layout

This approach is rejected.

## Design

### 1. Replace CLI npm artifacts with native platform archives

The CLI should stop publishing npm-shaped artifacts and instead publish
platform-native release archives:

- `mcodex-darwin-arm64.tar.gz`
- `mcodex-darwin-x64.tar.gz`
- `mcodex-linux-arm64.tar.gz`
- `mcodex-linux-x64.tar.gz`
- `mcodex-win32-arm64.zip`
- `mcodex-win32-x64.zip`

Each archive should contain only the current platform's needed runtime files.
The archive root should be simple and stable:

- macOS/Linux:
  - `bin/mcodex`
  - `bin/rg`
- Windows:
  - `bin/mcodex.exe`
  - `bin/rg.exe`
  - `bin/codex-command-runner.exe`
  - `bin/codex-windows-sandbox-setup.exe`

The archive format should be treated as the CLI's new distribution contract.
Install scripts and future self-update work should target this layout
directly, not a nested npm package structure.

### 2. Make OSS the only binary distribution source

OSS/CDN should become the sole location from which install and update flows
fetch CLI payloads.

Recommended OSS layout:

```text
/repositories/mcodex/
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

`latest.json` should be the only manifest the runtime and install scripts need
for latest-version discovery. It should stay intentionally small:

```json
{
  "product": "mcodex",
  "channel": "stable",
  "version": "0.96.0",
  "publishedAt": "2026-04-20T12:00:00Z",
  "notesUrl": "https://github.com/vivym/mcodex/releases/tag/rust-v0.96.0",
  "checksumsUrl": "https://downloads.mcodex.sota.wiki/repositories/mcodex/releases/0.96.0/SHA256SUMS",
  "install": {
    "unix": "curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh",
    "windows": "powershell -NoProfile -ExecutionPolicy Bypass -Command \"iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\\mcodex-install.ps1; & $env:TEMP\\mcodex-install.ps1\""
  }
}
```

The runtime does not need full platform asset URLs in `latest.json`. Installers
know how to map version + platform to archive path.

The Windows command intentionally downloads the hosted `install.ps1` into
`$env:TEMP` and then runs that local file. The canonical hosted source is
`https://downloads.mcodex.sota.wiki/install.ps1`; docs and TUI update prompts
should render the same command string.

### 3. Use a versioned install root plus a stable `current` pointer

Install scripts should not overwrite a single live binary in place. Instead,
they should maintain a versioned install tree with a stable `current`
reference.

Filesystem terms:

- `baseRoot`: the product-managed install root
- `versionsDir`: directory containing versioned installs
- `versionDir`: one immutable extracted version
- `currentLink`: stable pointer used by wrappers to find the active version
- `installMetadata`: diagnostic metadata written by installers

Selected layout:

| Term | macOS/Linux | Windows |
| --- | --- | --- |
| `baseRoot` | `~/.mcodex` | `%LOCALAPPDATA%\\Mcodex` |
| `versionsDir` | `~/.mcodex/install` | `%LOCALAPPDATA%\\Mcodex\\install` |
| `versionDir` | `~/.mcodex/install/<version>` | `%LOCALAPPDATA%\\Mcodex\\install\\<version>` |
| `currentLink` | `~/.mcodex/current` symlink | `%LOCALAPPDATA%\\Mcodex\\current` directory junction |
| `installMetadata` | `~/.mcodex/install.json` | `%LOCALAPPDATA%\\Mcodex\\install.json` |

`MCODEX_INSTALL_ROOT` should always point to `baseRoot`, not to
`versionsDir`, `versionDir`, or `currentLink`.

Wrappers should resolve the real binary through `currentLink/bin/mcodex` or
`currentLink\\bin\\mcodex.exe`. Installers are responsible for maintaining
`currentLink`.

This layout enables:

- explicit version installs
- safe latest upgrades
- rollback-friendly disk layout
- future `self-update` without redesigning on-disk structure

### 4. Put a thin wrapper on `PATH`

The visible PATH entry should be a thin wrapper, not the real versioned binary.

Recommended PATH entries:

- macOS/Linux: `~/.local/bin/mcodex`
- Windows PowerShell: `%LOCALAPPDATA%\\Programs\\Mcodex\\bin\\mcodex.ps1`

This slice supports PowerShell as the Windows command surface. `mcodex.cmd` is
explicitly out of scope.

Wrapper responsibilities:

- resolve `current/bin/mcodex` (or `mcodex.exe`)
- prepend `current/bin` to `PATH` so bundled `rg` is discoverable
- inject a small set of install metadata env vars
- forward arguments and exit status unchanged

Wrapper responsibilities explicitly exclude:

- network access
- version checks
- update installation
- install-root mutation
- complex diagnostics beyond "installation missing or corrupted; rerun the
  installer"

Recommended env vars:

- `MCODEX_INSTALL_MANAGED=1`
- `MCODEX_INSTALL_METHOD=script`
- `MCODEX_INSTALL_ROOT=<baseRoot>`

These env vars become the runtime-facing contract for script-managed installs.

### 5. Keep all disk mutation in the install scripts

`install.sh` and `install.ps1` become the only supported CLI install/update
entrypoints. They should own all disk mutation.

Responsibilities:

- accept `latest` or explicit version input
- resolve the target version from `latest.json` when needed
- determine the platform archive name
- download the archive plus `SHA256SUMS`
- verify checksums before switching live state
- extract into `install/<version>`
- atomically move the stable `current` pointer
- write/update installation metadata
- create the wrapper if missing
- update user PATH setup on first install using the platform's native
  profile/PATH mechanism

Selected PATH strategy:

- macOS/Linux: reuse the current installer behavior and write the wrapper
  directory into the user's shell startup file on first install (`.zshrc`,
  `.bashrc`, or `.profile` depending on the detected shell)
- Windows: write the wrapper directory into the user PATH environment variable
  in the registry, matching the current PowerShell installer behavior

This design intentionally keeps wrappers read-only and installers stateful.

### 6. Add an installation metadata file for diagnostics

Installers should write a small metadata file at `installMetadata`:

- macOS/Linux: `~/.mcodex/install.json`
- Windows: `%LOCALAPPDATA%\\Mcodex\\install.json`

Recommended shape:

```json
{
  "product": "mcodex",
  "installMethod": "script",
  "currentVersion": "0.96.0",
  "installedAt": "2026-04-20T12:00:00Z",
  "baseRoot": "/Users/alice/.mcodex",
  "versionsDir": "/Users/alice/.mcodex/install",
  "currentLink": "/Users/alice/.mcodex/current",
  "wrapperPath": "/Users/alice/.local/bin/mcodex"
}
```

This file is for diagnostics and supportability, not as the single source of
truth. The current version should still be derivable from the stable pointer
and versioned directories.

### 7. Move update prompts to a script-managed runtime action

The Rust runtime currently recognizes npm, bun, and Homebrew managed installs
for update prompts. This should change.

The runtime should add a script-managed update action, for example
`ScriptManagedLatest`, and detect it through wrapper-injected install metadata
env vars.

Selected behavior:

- if `MCODEX_INSTALL_MANAGED=1` is present, prefer the script-managed update
  action
- otherwise, retain any separately supported package-manager detection that the
  fork still intentionally supports
- npm/bun should no longer be the primary CLI install/update path for this fork

The script-managed update action should render the script-based update command
directly in the TUI. It should not introduce a new CLI helper command in this
slice.

Selected rendering:

- Unix: `curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh`
- Windows: `powershell -NoProfile -ExecutionPolicy Bypass -Command "iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $env:TEMP\mcodex-install.ps1; & $env:TEMP\mcodex-install.ps1"`

Because the user selected TUI-only update prompts, wrappers should not perform
their own version checks or print update notices.

### 8. Move runtime update checks from GitHub latest API to OSS manifest

The runtime update check should no longer depend on GitHub latest-release API
responses. Instead it should fetch the OSS stable `latest.json` manifest and
compare `version` against the current build version.

This keeps:

- domestic-user update checks on the same OSS/CDN path as installer downloads
- release notes discoverability through a `notesUrl`
- version comparison logic local to Rust/TUI

The runtime should continue caching update-check state locally on roughly the
same cadence it already uses today.

### 9. Keep GitHub Releases as lightweight publication records

GitHub Releases should remain, but not as the binary distribution channel.

Recommended GitHub Release contents:

- tag and release notes
- `SHA256SUMS`
- `SHA256SUMS.sig`
- source snapshot references
- OSS installation and download links in the release body

Large platform archives should live on OSS only. GitHub Releases remain useful
for:

- public version history
- changelog visibility
- checksum/signature anchoring
- a lightweight fallback publication record independent of OSS object listing

### 10. Split CLI distribution changes from SDK npm publishing

This CLI distribution redesign does not imply removing npm from the entire repo.

Specifically:

- the TypeScript SDK may continue to build and publish as an npm package
- repo-local JS/TS tooling may continue to use pnpm
- only the CLI packaging, CLI release staging, and CLI runtime update surfaces
  should be de-npm-ified in this slice

This boundary avoids unnecessary churn and keeps the change scoped to the
user-approved goal.

## Migration and Compatibility

The selected strategy is a clean cutover.

Implications:

- no dedicated runtime compatibility path for users who installed earlier
  versions of the CLI through npm
- no dual-track CLI update behavior for npm-installed users
- docs and release notes should clearly instruct old users to reinstall using
  the script-managed OSS installer if they want to move onto the new channel

This is intentionally simpler than supporting a prolonged dual-path migration.

## Release Workflow Changes

The release pipeline should change from:

- build native binaries
- stage npm packages
- publish npm packages
- attach everything to GitHub Release

to:

- build native binaries
- assemble native platform archives
- generate `SHA256SUMS` and signature artifacts
- upload those artifacts to OSS
- update `channels/stable/latest.json`
- create/update a lightweight GitHub Release containing release notes and
  checksum artifacts only

The OSS publish order must be atomic from the client's perspective:

1. upload versioned release artifacts
2. verify upload success
3. update or upload `stable/latest.json` last

That ensures clients never learn about a version before the corresponding
artifacts are available.

## Testing

Coverage should include:

- Rust unit tests for update-action detection of script-managed installs
- Rust unit tests for parsing OSS `latest.json`
- Rust unit tests ensuring TUI-rendered update commands no longer reference npm
  or bun for the CLI
- integration tests for `install.sh` latest install, explicit version install,
  and reinstall/update behavior
- integration tests for `install.ps1` latest install, explicit version install,
  and reinstall/update behavior
- wrapper smoke tests proving PATH injection and exit-code forwarding
- release-workflow tests or scripted validations for archive layout and
  checksum generation

Manual smoke should confirm:

- fresh macOS/Linux install from OSS works without Node/npm installed
- fresh Windows install from OSS works without Node/npm installed
- explicit version install works on both platforms
- upgrade from one script-installed version to another updates `current`
  without leaving PATH broken
- `mcodex` launched through the wrapper sees script-managed install metadata
- TUI update prompt points to the script-managed update path, not npm/bun
- lightweight GitHub Release still provides notes and checksum artifacts

## Risks and Mitigations

### Risk: Installer and runtime drift

If install scripts, wrapper env vars, and Rust update detection evolve
independently, update prompts can become wrong or stale.

Mitigation:

- define the wrapper env vars as a small stable contract
- cover them with unit tests in Rust and smoke tests in installer scripts

### Risk: Future pressure to restore npm semantics accidentally

The repo still contains npm-based SDK and tooling, so future edits could
accidentally pull CLI logic back toward npm assumptions.

Mitigation:

- document this design explicitly
- keep CLI distribution names, manifests, and workflow steps clearly separate
  from SDK npm publishing

### Risk: OSS publish races expose broken latest pointers

If `latest.json` updates before versioned archives exist, installs and updates
will fail.

Mitigation:

- publish versioned artifacts first
- only publish channel manifests last
- optionally verify object existence before channel-manifest update

### Risk: Thin wrapper grows into a second updater

There is natural pressure to add convenience checks to wrappers over time.

Mitigation:

- keep wrapper behavior intentionally minimal
- route update UX through Rust/TUI only

## Acceptance Criteria

- CLI distribution no longer depends on npm packaging or npm publish
- install scripts fetch native platform archives from OSS/CDN
- install scripts support both `latest` and explicit versions
- PATH points to a thin wrapper, not directly to the versioned binary
- wrappers inject script-managed install metadata and otherwise stay minimal
- TUI update prompts recognize script-managed installs and no longer direct the
  CLI user to npm/bun
- runtime update checks read the OSS stable manifest instead of GitHub latest
  API
- GitHub Releases remain lightweight publication records with notes and checksum
  artifacts, not the primary binary distribution channel
- TypeScript SDK npm publishing remains unaffected by the CLI cutover
