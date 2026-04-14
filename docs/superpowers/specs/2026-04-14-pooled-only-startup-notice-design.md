# Pooled-Only Startup Notice Design

This document defines the startup UX for the case where Codex has no shared
ChatGPT login, but pooled account access is already available through the local
multi-account pool.

It is an additive follow-up to:

- `docs/superpowers/specs/2026-04-10-multi-account-pool-design.md`
- `docs/superpowers/specs/2026-04-13-pooled-account-registration-design.md`

## Summary

Today the TUI startup flow treats "not logged in via shared auth" as equivalent
to "cannot continue", even when pooled accounts have already been registered
and can be leased successfully at turn time.

The recommended fix is to keep shared-login detection unchanged, but add a
separate startup notice path:

- if shared login is present, skip the notice
- if shared login is absent and pooled access is not available, show the
  existing login onboarding
- if shared login is absent but pooled access is available, show a lightweight
  notice that allows the user to continue into the TUI, jump to shared login,
  or hide the reminder for future launches

This preserves the semantic distinction between shared auth and pooled access,
keeps merge risk low, and remains compatible with a future remote pool backend.

## Goals

- Stop treating pooled-only installations as blocked from entering the TUI.
- Preserve a lightweight startup explanation instead of silently skipping login.
- Let the user continue immediately, optionally perform shared login, or hide
  the reminder permanently.
- Keep `account/read` and `LoginStatus` semantics centered on shared auth.
- Reuse existing config notice persistence instead of introducing a new storage
  system.

## Non-Goals

- Do not redefine pooled access as shared login.
- Do not expand the app-server protocol.
- Do not redesign the onboarding flow outside this specific startup branch.
- Do not require pooled accounts to be immediately lease-eligible at startup.
- Do not change `/status` or lease display semantics beyond existing behavior.

## Product Behavior

### Startup states

The TUI startup flow should distinguish three states:

1. `NeedsLogin`
   - The provider requires OpenAI auth.
   - No shared login is present.
   - Pooled-only continuation is not available.
   - Result: show the existing login onboarding.

2. `PooledOnlyNotice`
   - The provider requires OpenAI auth.
   - No shared login is present.
   - The current effective pool contains at least one enabled registered
     account.
   - The user has not hidden the notice.
   - Result: show a lightweight pooled-access notice before entering the TUI.

3. `NoPrompt`
   - Shared login is present, or the provider does not require OpenAI auth, or
     the pooled-only notice is hidden.
   - Result: skip the pooled-only notice and proceed normally.

### Pooled-only notice interactions

The pooled-only notice should offer:

- `Enter`: continue into the TUI immediately
- `L`: open the existing shared-login onboarding flow
- `N`: persist "do not show again", then continue into the TUI

The notice is informational. It must not block the user from entering the TUI
when pooled access is available.

### User-facing copy

The notice must avoid implying that the user is already logged in via shared
ChatGPT auth.

Recommended copy shape:

- pooled account access is available
- no local shared ChatGPT login is configured
- Codex can continue using the current account pool

The copy may mention that shared login is still available, but it should not
explain backend-private auth, lease managers, or storage internals.

## Detection Rules

### Shared login

Shared login detection remains unchanged:

- continue to use the current `LoginStatus` path
- continue to derive it from `account/read`
- continue to let `account/read` reflect shared auth only

This design explicitly rejects changing `LoginStatus` to treat pooled accounts
as logged-in shared auth.

### Pooled-only continuation

Pooled-only continuation should be considered available when all of the
following are true:

- the configured model provider requires OpenAI auth
- `LoginStatus` is `NotAuthenticated`
- the current effective pool resolves successfully
- that pool contains at least one enabled registered account

This check does not require the account to be healthy or immediately eligible
for lease acquisition. Startup should only answer "is there a pooled account
surface that makes continuing reasonable?", not "would lease acquisition
succeed right now?"

## Architecture

### 1. Keep `LoginStatus` semantics intact

`LoginStatus` should remain a shared-auth signal only.

The startup behavior should be driven by a higher-level decision layer, for
example:

- `NeedsLogin`
- `PooledOnlyNotice`
- `NoPrompt`

This avoids contaminating auth semantics, telemetry, and existing app-server
account display behavior.

### 2. Add a narrow pooled startup probe

The TUI should add a small startup helper that determines whether pooled-only
continuation is available.

That helper should:

- resolve the effective pool using existing startup-selection logic
- inspect pool diagnostics or membership records
- return a small result suitable for onboarding decisions

It should not import CLI modules into the TUI. If code reuse is needed, prefer
extracting a small shared helper over reaching into `cli/src/accounts/*`.

### 3. Reuse the existing onboarding shell

The onboarding flow should gain a dedicated pooled-access step rather than
overloading the current auth widget.

Recommended ordering:

- `Welcome`
- `PooledAccessNotice` when applicable
- existing `Auth` step only if the user chooses to log in
- existing trust-directory step as today

This keeps the auth widget focused on login behavior and prevents pooled-only
UX from being represented as a fake auth success state.

### 4. Persist "do not show again" through `notice`

The hide flag should reuse the existing config-backed notice system.

Add a new boolean under `[notice]`, for example:

- `hide_pooled_only_startup_notice`

Persistence should reuse the existing config edit and app-server config write
path already used for other TUI acknowledgements.

## Error Handling

- If the pooled startup probe fails, fail open to the existing login behavior.
- If persisting the hide flag fails, do not block entry into the TUI.
- If the user selects `L`, hand off to the existing login onboarding without
  changing pooled state.

In all cases, prefer warning logs and preserved forward progress over startup
hard failures.

## Testing Strategy

### TUI startup decision tests

Add focused tests for the startup decision helper:

- shared login present -> `NoPrompt`
- pooled-only available and notice visible -> `PooledOnlyNotice`
- pooled-only available and notice hidden -> `NoPrompt`
- no shared login and no pooled account -> `NeedsLogin`

### Onboarding interaction tests

Add focused tests for the new notice step:

- `Enter` continues into the TUI
- `L` reveals the existing auth step
- `N` persists the hide flag and continues

### Integration coverage

Add a narrow startup integration test that proves:

- pooled-only startup shows the new notice instead of the existing login step

No protocol expansion or broad end-to-end harness changes are required.

## Rationale

This design is the best fit for the branch goals because it:

- fixes the user-facing gap without redefining shared auth
- keeps changes localized to TUI startup and config notice persistence
- preserves compatibility with future remote account-pool backends
- minimizes upstream merge surface by avoiding protocol and auth-core churn
