# mcodex Active Runtime Display Identity Design

## Summary

This document defines a narrow follow-up to the `mcodex` product-identity work:
all user-visible runtime surfaces reached through the `mcodex` binary must
present `mcodex` as the active product name, while preserving clear attribution
that the fork is derived from OpenAI Codex.

The goal is to remove the current runtime branding drift where path, home, and
installer identity have already switched to `mcodex`, but release binary output
and onboarding/login surfaces still present the active product as `Codex`.

## Problem

Release smoke tests on April 18, 2026 showed that the current branch still has
mixed active-runtime identity:

- `mcodex --version` reports `codex-cli 0.0.0`
- interactive onboarding renders `Welcome to Codex`
- onboarding auth copy says `Sign in with ChatGPT to use Codex`
- device-code auth prints `Welcome to Codex`

This is no longer just documentation drift. Users now launch a binary named
`mcodex`, store runtime state under `MCODEX_HOME` / `~/.mcodex`, and install
from `mcodex` release/update surfaces, but the first runtime screens still
describe the current product as upstream `Codex`. That makes the fork feel
internally inconsistent and undermines the product-identity work already landed.

## Goals

- Ensure user-visible runtime surfaces reached through `mcodex` present
  `mcodex` as the active product.
- Use a consistent secondary description that preserves source attribution,
  such as `an OpenAI Codex-derived command-line coding agent`.
- Keep the fix upstream-friendly by limiting changes to display identity, not
  crate names, protocol names, or unrelated repository text.
- Keep all changes inside the existing low-conflict identity-only slice.

## Non-Goals

- Do not rename workspace crate/package names such as `codex-cli`.
- Do not perform a repo-wide `Codex -> mcodex` rewrite.
- Do not rewrite upstream-facing docs, internal prompts, protocol names, or
  implementation identifiers that are not part of active runtime display.
- Do not change config, home-dir, migration, installer, or state semantics in
  this follow-up.

## Scope

This design applies to active runtime display surfaces only:

- CLI version/help-facing identity shown directly to the user
- TUI onboarding welcome/auth copy
- device-code login prompt
- adjacent onboarding runtime strings and snapshots that would otherwise leave
  the same screen internally inconsistent

This design does not automatically include:

- README or broader docs branding
- internal prompt templates
- crate names, package names, or binary target names other than what is shown
  to the user at runtime
- unrelated historical references to upstream Codex

## Design

### 1. Add a Small Display-Identity Layer

The existing `codex-product-identity` crate already owns runtime identity for
binary name, home/env names, admin config roots, release endpoints, and legacy
migration metadata. The current gap is that it does not expose enough
display-oriented metadata for user-facing runtime text.

Extend `ProductIdentity` with a small display-identity surface for active
runtime copy. It should remain intentionally narrow:

- active display name: `mcodex`
- active runtime tagline: `an OpenAI Codex-derived command-line coding agent`

This layer should centralize only reusable display primitives. It should not
turn `ProductIdentity` into a store for every complete sentence used by the
CLI/TUI/login flows.

### 2. Keep Sentence Assembly Local

Call sites should continue assembling complete UI sentences locally, but they
must source product name and tagline from the shared display-identity layer.

Examples:

- `Welcome to mcodex`
- `mcodex, an OpenAI Codex-derived command-line coding agent`
- `Sign in with ChatGPT to use mcodex as part of your paid plan`

This keeps product naming consistent without overfitting the identity crate to
one specific screen layout.

### 3. Fix User-Facing Version Identity Without Renaming the Crate

`mcodex --version` must no longer expose `codex-cli`.

The crate/package name should remain `codex-cli` for workspace stability and
upstream mergeability. The design therefore requires adjusting the user-facing
version output layer, not the package identity itself.

The selected implementation should ensure:

- `mcodex --version` presents `mcodex`
- `mcodex --help` remains aligned with the same active binary identity
- crate/package metadata remains unchanged unless a future independent effort
  explicitly chooses to rename it

### 4. Update Onboarding and Device-Code Surfaces Together

The onboarding welcome screen, onboarding auth picker, and device-code prompt
must move together. Fixing only one of them would preserve a split-brain first
run experience.

The minimum runtime copy change set is:

- welcome header: `Welcome to mcodex`
- welcome tagline: `an OpenAI Codex-derived command-line coding agent`
- auth picker sentence: `Sign in with ChatGPT to use mcodex as part of your paid plan`
- device-code prompt header and tagline with the same identity

Any adjacent runtime copy on the same onboarding path that still identifies the
active product as `Codex` should be updated in the same change if leaving it
behind would make the screen internally inconsistent.

### 5. Treat Snapshots as Part of the Product Surface

This branch already relies on TUI snapshot coverage for onboarding behavior.
Because these changes are user-visible and intentional, the corresponding
snapshots are part of the implementation, not a follow-up chore.

## Testing

The implementation should verify both direct output and rendered UI:

- `cargo test -p codex-cli`
- `cargo test -p codex-login`
- `cargo test -p codex-tui`

Coverage should include:

- a CLI assertion that the version/help-facing identity shown to the user uses
  `mcodex`
- a device-code prompt assertion for the updated title/tagline
- onboarding snapshot coverage for the updated welcome/auth surfaces

Manual smoke should confirm:

- release `mcodex --version` no longer prints `codex-cli`
- release TTY startup reaches onboarding with `mcodex` as the active product
- release login/device-code prompts no longer say `Welcome to Codex`

## Risks and Mitigations

### Scope Creep Into Full Rebranding

The largest risk is turning a targeted runtime-identity fix into a repo-wide
branding campaign. To avoid that, this change should only touch surfaces that
describe the currently running product to the user.

### Mixing Runtime Identity With Upstream Attribution

The fork should not present itself as the official upstream product, but it also
should not hide its origin. The mitigation is to use `mcodex` as the active
product name while keeping attribution in the shared tagline.

### Overloading `ProductIdentity`

If `ProductIdentity` starts storing every complete sentence, it will become hard
to evolve and awkward to reuse. The mitigation is to add only small display
primitives and keep full sentence composition in each UI surface.

## Acceptance Criteria

- `mcodex --version` presents `mcodex`, not `codex-cli`
- onboarding welcome/auth screens no longer identify the active product as
  `Codex`
- device-code login prompts no longer identify the active product as `Codex`
- the affected onboarding path uses one consistent runtime identity on-screen
- the fix remains confined to active runtime display surfaces and does not
  expand into an unrelated repo-wide rebrand
