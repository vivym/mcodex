# Mcodex Product Identity And First-Run Migration Design

This document defines how this fork should become a distinct installable
product named `mcodex` while keeping the core runtime as close as practical to
upstream Codex.

It is intentionally scoped to product identity and local installation safety.
It does not redesign the multi-account pool itself, and it does not yet cover
public npm/Homebrew/WinGet publishing.

## Summary

The recommended direction is to introduce a small, centralized product identity
layer and switch the fork's default runtime identity from upstream `codex` to
`mcodex`.

Approved product decisions:

- Product name: `mcodex`
- Primary command name: `mcodex`
- Default state directory: `~/.mcodex`
- Primary home override variable: `MCODEX_HOME`
- Normal runtime must not fall back to `CODEX_HOME`
- On first launch, if upstream `~/.codex` exists and `~/.mcodex` is not yet
  initialized, `mcodex` should show a blocking migration prompt
- Migration copies compatible config and auth only
- Migration must not import account-pool state, SQLite state, history, logs, or
  plugin caches
- Imported data is copied, not moved; upstream `codex` keeps working

The key architectural constraint is that fork identity changes must stay in
edge surfaces such as home/env resolution, startup migration, update checks,
installer paths, and user-visible product naming. Core crates, protocol types,
and runtime business logic should not be mass-renamed just to reflect branding.
That keeps merge friction with upstream manageable.

## Goals

- Make `mcodex` safe to install and use on the same machine as upstream
  `codex`.
- Prevent accidental state sharing between `mcodex` and upstream `codex`.
- Provide a first-run migration path that preserves useful user data without
  importing unsafe runtime state.
- Centralize product identity so future fork-specific distribution work does not
  require repo-wide string edits.
- Keep the implementation shape compatible with continued upstream merging.
- Cover macOS, Windows, and Linux for internal installation paths.

## Non-Goals

- Do not rename internal crate names such as `codex-core`, `codex-tui`, or
  `codex-cli`.
- Do not rename protocol-level concepts, event names, or internal data models
  just to reflect product branding.
- Do not implement public npm publishing, Homebrew tap publication, WinGet
  publication, or polished public installers in this slice.
- Do not import upstream runtime state such as SQLite DBs, lease health, logs,
  or session history.
- Do not attempt to synthesize account-pool state from upstream installs.
- Do not redesign the existing multi-account pool architecture.

## Constraints

- The fork needs to be usable internally first, but the design should not paint
  the project into a corner for later public release.
- The user wants strong coexistence with upstream Codex on the same machine.
- The user explicitly wants `mcodex` to become the real product identity now,
  not just a local wrapper convention.
- Upstream Codex currently uses `CODEX_HOME` and defaults to `~/.codex`; that
  must no longer be the primary runtime identity for this fork.
- Existing upstream assumptions are scattered across runtime config, update
  prompts, install scripts, npm packaging metadata, macOS managed config
  domains, and Windows install paths.
- Mergeability with upstream matters, so invasive repo-wide renaming is not
  acceptable.

## Approaches Considered

### Approach A: Hard fork identity everywhere immediately

Change all visible and internal `codex` identifiers to `mcodex`, including
crate names, internal type names, protocol text, and all docs.

Pros:

- Maximum internal/external naming consistency
- Very clear product separation

Cons:

- Extremely high merge risk
- Very large churn across core runtime and protocol surfaces
- Little practical value for installation safety

This approach is rejected.

### Approach B: Wrapper-only fork identity

Leave the runtime on `codex`/`CODEX_HOME` and rely on wrapper scripts to change
command names and directories.

Pros:

- Minimal implementation effort
- Lowest short-term code churn

Cons:

- Leaks upstream identity into normal runtime behavior
- Easy to misconfigure or bypass
- Public release would require a second, larger rewrite later
- Does not solve product identity at the source of truth

This approach is rejected.

### Approach C: Product identity abstraction plus default switch to `mcodex`

Introduce a small product identity layer, route edge surfaces through it, and
set this fork's defaults to `mcodex` while preserving legacy upstream identity
only for first-run migration detection.

Pros:

- Gives the fork a real product identity now
- Keeps merge-sensitive changes concentrated
- Solves coexistence and migration cleanly
- Scales to later installer/update/public release work

Cons:

- Requires disciplined boundary design up front
- Slightly more work now than a wrapper-only solution

This is the recommended approach.

## Product Behavior

### Primary runtime identity

`mcodex` becomes the active product identity for this fork:

- command name: `mcodex`
- default home dir: `~/.mcodex`
- home override variable: `MCODEX_HOME`

The runtime should resolve its own state using `MCODEX_HOME` first and
`~/.mcodex` second. It must not use `CODEX_HOME` during normal execution.

### Legacy identity

The following upstream identity values remain relevant only for migration
probing:

- upstream home dir: `~/.codex`
- upstream home env var: `CODEX_HOME`

Legacy identity should be treated as an import source, not as a secondary live
runtime location.

### First-run migration trigger

On startup, `mcodex` should enter a blocking migration prompt when all of the
following are true:

- `mcodex` home is not yet initialized
- no prior migration completion marker exists
- upstream `~/.codex` exists

If those conditions are not met, startup proceeds normally.

### First-run migration options

The prompt should offer:

- import config and login
- skip

It should not silently auto-import. The user should make an explicit choice.

### Migration result

If the user chooses import:

- create the `mcodex` home
- copy compatible config into the new home
- copy auth-related material into the new home
- record a migration completion marker

If the user chooses skip:

- record a migration completion marker
- continue startup without import

The point of the marker is to avoid repeatedly interrupting future launches.

### Data copied during migration

Allowed:

- compatible user config from upstream `config.toml`
- auth data needed to preserve login state

Not allowed:

- account-pool registration or pool membership state
- SQLite state DBs
- lease, health, or suppression runtime state
- rollout/session history
- logs
- plugin caches
- transient temp directories

The account-pool omission is intentional. Upstream Codex does not own a local
account-pool model for this fork, so there is no safe source of truth to import
from.

## Architecture

### 1. Centralize product identity

Create one small product identity unit that provides fork defaults and legacy
compatibility metadata.

It should define at least:

- `product_name = "mcodex"`
- `binary_name = "mcodex"`
- `default_home_dir_name = ".mcodex"`
- `home_env_var = "MCODEX_HOME"`
- `legacy_binary_name = "codex"`
- `legacy_home_dir_name = ".codex"`
- `legacy_home_env_var = "CODEX_HOME"`
- `github_repo_owner`
- `github_repo_name`
- `release API URLs`
- `installer default directories`
- `macOS managed-config domain`
- package-manager-facing names where needed

This layer should be the only place that knows both the active product identity
and the upstream legacy identity.

### 2. Keep identity changes at the edge

The fork should not push `mcodex` naming deep into core runtime concepts.

The following layers should consume product identity:

- home/env resolution
- startup migration/onboarding
- installer scripts
- update checks and update commands
- user-visible help or product strings where they must name the product
- release/distribution metadata

The following layers should remain largely untouched unless behavior truly
requires it:

- internal crate names
- protocol types and field names
- multi-account pool internals
- session runtime state machines
- core tool orchestration

This is the main mergeability rule. Upstream-facing core behavior should remain
structurally familiar, while product identity lives in a narrow edge band.

### 3. Resolve home from `MCODEX_HOME`, not `CODEX_HOME`

Runtime home resolution should change from:

- `CODEX_HOME`
- fallback `~/.codex`

to:

- `MCODEX_HOME`
- fallback `~/.mcodex`

The runtime must not silently fall back to `CODEX_HOME`, because doing so would
reintroduce accidental state sharing with upstream installs.

Legacy upstream home detection belongs in a separate migration helper, not in
the normal home-resolution path.

### 4. Add a first-run migration service

Migration should be handled by a small, explicit service with two
responsibilities:

- determine whether migration should be offered
- execute a copy-based import of approved data

Recommended phases:

1. Detect current `mcodex` home state
2. Detect legacy upstream home presence
3. Decide whether migration prompt should appear
4. If chosen, import config and auth into the new home
5. Record migration completion

This logic should be isolated from generic config loading so it can evolve
without contaminating the normal runtime path.

### 5. Treat config migration as transform, not blind copy

`config.toml` import should be:

- parse upstream config
- keep only compatible fields
- rewrite or drop product-specific values as needed
- write a new `mcodex` config

For pooled-account settings specifically:

- preserve policy fields that remain valid in the fork, such as lease timing,
  thresholds, backend choice, and pool-policy tables
- do not blindly preserve `accounts.default_pool` when pooled SQLite state is
  not being imported, because that selection would point at state that does not
  exist in the new product home

Blind file copy is not recommended. A transform keeps future divergence
manageable and avoids importing upstream-specific product assumptions
unchanged.

### 6. Treat auth migration as copy into the new product namespace

The auth import should preserve user login where possible, but it must write
the imported data into `mcodex`'s storage location.

The post-migration outcome should be:

- upstream `codex` still works with its own state
- `mcodex` starts with a copied auth baseline
- after import, the two products evolve independently

### 7. Update-check and installer behavior must switch product identity

If runtime identity switches to `mcodex` but update-check and installer logic
still point at upstream `openai/codex`, users will be prompted to install or
upgrade the wrong product.

Therefore, this design requires product identity to drive:

- release API endpoints
- release notes URLs
- package-manager update commands
- installer default directories
- product naming in prompts and notices

This applies to:

- TUI update logic
- update command rendering
- install.sh
- install.ps1
- npm/package staging metadata that is needed for internal install paths
- macOS managed-config domain
- Windows install location defaults

### 8. Keep public distribution channels out of phase 1

The fork should not try to solve all public release channels in this identity
slice.

Phase 1 should cover:

- local/internal installation paths
- fork-owned update source
- command/home/env identity
- first-run migration

Later phases may add:

- npm package publication
- Homebrew tap/cask
- WinGet
- notarization/code signing refinements
- broader branding/documentation sweep

## Expected Change Surfaces

The following areas are expected to change in the implementation plan:

- home-dir resolution utilities
- runtime config comments and docs that currently describe `CODEX_HOME` and
  `~/.codex`
- startup/onboarding flow for first-run migration
- update-check logic and update command rendering
- install scripts
- local/internal release metadata
- user-visible product strings in key startup/update surfaces

The following areas should be changed only if required by a concrete runtime
dependency:

- protocol crates
- multi-account pool control plane
- core runtime event models
- business logic unrelated to product identity or migration

## Error Handling

### Home resolution

- If `MCODEX_HOME` is set to a missing or invalid path, fail with a direct
  `MCODEX_HOME`-specific error.
- Do not suggest `CODEX_HOME` as a valid normal runtime workaround.

### Migration detection

- If upstream `~/.codex` is unreadable, treat migration as unavailable and log a
  startup warning rather than blocking all startup.

### Config import

- If config parsing fails, allow the user to continue without config import.
- The failure should not delete or mutate upstream config.

### Auth import

- If auth import fails, the prompt should surface that config may have imported
  successfully but login import did not.
- The user should still be able to continue and log in later.

### Marker persistence

- Failure to record the migration marker should warn, not brick startup, but it
  should be visible because it can cause repeated prompts.

## Testing Strategy

### Unit tests

Add targeted tests for:

- home resolution prefers `MCODEX_HOME`
- runtime does not fall back to `CODEX_HOME`
- default home resolves to `~/.mcodex`
- migration-offer decision matrix
- config import filtering/mapping
- migration marker behavior
- update source selection and update command rendering

### Integration tests

Add integration coverage for:

- first launch with only `~/.codex` present triggers migration prompt
- choosing import copies config/auth but not runtime state
- choosing skip suppresses future prompts
- post-migration startup enters the TUI normally
- `mcodex --help` and startup prompts use the new product name

### Manual validation

Manual smoke test on a machine that already has upstream `codex` installed:

1. confirm upstream `~/.codex` exists
2. install and launch `mcodex`
3. confirm migration prompt appears
4. import config/login
5. confirm `~/.mcodex` is created
6. confirm upstream `~/.codex` remains intact
7. confirm `mcodex` uses `~/.mcodex`, not `~/.codex`
8. confirm account-pool state starts fresh

## Acceptance Criteria

- `mcodex` launches and uses `MCODEX_HOME`/`~/.mcodex` by default.
- Normal runtime does not implicitly read `CODEX_HOME`.
- On first launch, `mcodex` offers a blocking migration prompt when upstream
  `~/.codex` exists and `~/.mcodex` is uninitialized.
- Import copies compatible config and auth only.
- Import never copies runtime SQLite state, logs, history, or account-pool
  state.
- Update prompts and installer paths no longer point at upstream `codex`.
- The implementation keeps fork identity changes localized to edge surfaces,
  with no unnecessary repo-wide internal rename.

## Follow-Up Work

Once this identity slice lands, the next plan should cover:

- exact implementation tasks for the identity layer
- runtime migration service wiring
- update/install edge adoption
- test coverage for migration and identity resolution

After that, separate follow-up plans can address:

- public npm/Homebrew/WinGet packaging
- notarization/code signing
- broader public branding/documentation updates
