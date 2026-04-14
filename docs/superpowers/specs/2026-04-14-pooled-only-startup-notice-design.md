# Pooled-Only Startup Notice Design

This document defines the startup UX for the case where Codex has no shared
ChatGPT login, but pooled account access is already available through the
multi-account pool surface.

It is an additive follow-up to:

- `docs/superpowers/specs/2026-04-10-multi-account-pool-design.md`
- `docs/superpowers/specs/2026-04-13-pooled-account-registration-design.md`

## Summary

Today the TUI startup flow treats "not logged in via shared auth" as equivalent
to "cannot continue", even when pooled accounts have already been registered
and pooled startup access should be available.

The recommended fix is to keep shared-login detection unchanged, but add a
separate startup notice path:

- if shared login is present, skip the notice
- if shared login is absent and pooled access is not available, show the
  existing login onboarding
- if shared login is absent but pooled access is available, show a lightweight
  notice that allows the user to continue into the TUI, jump to shared login,
  or hide the reminder for future launches
- if shared login is absent and pooled startup is durably suppressed, show a
  separate paused notice that offers resume-or-login behavior instead of
  treating the user as ready to continue

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

The TUI startup flow should distinguish four states:

1. `NeedsLogin`
   - The provider requires OpenAI auth.
   - No shared login is present.
   - No pooled startup surface is available.
   - Result: show the existing login onboarding.

2. `PooledOnlyNotice`
   - The provider requires OpenAI auth.
   - No shared login is present.
   - Pooled-only continuation is available.
   - The user has not hidden the notice.
   - Result: show a lightweight pooled-access notice before entering the TUI.

3. `PooledAccessPausedNotice`
   - The provider requires OpenAI auth.
   - No shared login is present.
   - The pooled startup surface exists, but startup selection is durably
     suppressed.
   - Result: show a paused notice instead of the lightweight continue notice.

4. `NoPrompt`
   - Shared login is present, or the provider does not require OpenAI auth, or
     pooled-only continuation is available and the pooled-only notice is
     hidden.
   - Result: skip the pooled-only notice and proceed normally.

### Pooled-only notice interactions

The pooled-only notice should offer:

- `Enter`: continue into the TUI immediately
- `L`: open the existing shared-login onboarding flow
- `N`: persist "do not show again", then continue into the TUI

The notice is informational. It must not block the user from entering the TUI
when pooled access is available.

### Pooled-access-paused notice interactions

The paused notice should not behave like the pooled-only continue notice,
because current runtime behavior treats durable suppression as fail-closed for
fresh pooled startup.

The paused notice should offer:

- `Enter`: resume pooled startup and continue into the TUI
- `L`: open the existing shared-login onboarding flow

The paused notice should not offer "don't show again". It represents an active
state mismatch that requires the user either to resume pooled startup or choose
another auth path.

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
- the pooled startup probe resolves a pooled startup surface
- the pooled startup surface is not durably suppressed

This check does not require the account to be healthy or immediately eligible
for lease acquisition. Startup should only answer "is there a pooled account
surface that makes continuing reasonable?", not "would lease acquisition
succeed right now?"

### Pooled-access-paused detection

Paused pooled startup should be considered present when all of the following
are true:

- the configured model provider requires OpenAI auth
- `LoginStatus` is `NotAuthenticated`
- the pooled startup probe resolves a pooled startup surface
- that surface is marked durably suppressed

This is intentionally separate from pooled-only continuation because current
suppressed pooled startup is expected to fail closed until resumed.

## Architecture

### 1. Keep `LoginStatus` semantics intact

`LoginStatus` should remain a shared-auth signal only.

The startup behavior should be driven by a higher-level decision layer, for
example:

- `NeedsLogin`
- `PooledOnlyNotice`
- `PooledAccessPausedNotice`
- `NoPrompt`

This avoids contaminating auth semantics, telemetry, and existing app-server
account display behavior.

### 2. Add a narrow pooled startup probe

The TUI should add a small startup helper that determines whether the startup
surface is:

- absent
- pooled-only and resumable
- pooled but durably suppressed

The probe must be remote-capable.

That helper should:

- return a small result suitable for onboarding decisions
- avoid changing `account/read` semantics
- avoid introducing a new protocol surface in this slice

Recommended shape:

- embedded/local app-server: exact probe using startup-selection preview and
  pool diagnostics from existing runtime/state APIs
- remote app-server: best-effort probe using the already-added
  `accountLease/read` response fields in this branch, such as `suppressed`,
  `pool_id`, `account_id`, `health_state`, `switch_reason`, and
  `suppression_reason`

For embedded/local mode, pooled-only continuation should require:

- an effective pool
- at least one enabled registered account in that pool
- `suppressed == false`

For remote mode, the probe may fall back to the `accountLease/read` visible
state heuristic because the TUI does not own remote runtime state directly. In
that mode:

- `suppressed == true` maps to `PooledAccessPausedNotice`
- a visible pooled lease surface with `pool_id` set maps to `PooledOnlyNotice`
- an empty lease response falls back to `NeedsLogin`

This remote heuristic is intentionally best-effort and should be documented as
such until a stronger remote startup probe is justified. It reuses the
existing branch-local `accountLease/read` surface; this slice does not add new
protocol fields or methods.

It should not import CLI modules into the TUI. If code reuse is needed, prefer
extracting a small shared helper over reaching into `cli/src/accounts/*`.

### 3. Reuse the existing onboarding shell

The onboarding flow should gain a dedicated pooled-access step rather than
overloading the current auth widget.

Recommended ordering:

- `Welcome`
- `PooledAccessNotice` when applicable
- `PooledAccessPausedNotice` when applicable
- existing `Auth` step when login is required or when a pooled notice may hand
  off to login
- existing trust-directory step as today

This keeps the auth widget focused on login behavior and prevents pooled-only
UX from being represented as a fake auth success state.

The outer onboarding gate must also be updated. Entering onboarding can no
longer depend only on trust-screen or login-screen booleans; it must also enter
when the startup decision resolves to either pooled notice state.

The `L` handoff must be explicit in the ownership model. The recommended design
is to prebuild the auth step whenever the provider requires OpenAI auth and an
app-server request handle is available, but keep that step hidden unless login
is required immediately or a pooled notice reveals it. This avoids rebuilding
the onboarding shell mid-flight.

### 4. Persist "do not show again" through `notice`

The hide flag should reuse the existing config-backed notice system.

Add a new boolean under `[notice]`, for example:

- `hide_pooled_only_startup_notice`

Persistence boundaries should be explicit:

- embedded/local mode: persist directly from the onboarding/startup path using
  the same underlying `ConfigEditsBuilder` mechanism as other TUI notices
- remote mode: persist the same config field through the existing
  `config/batchWrite` path exposed by the remote app-server

This keeps the config key stable while allowing the persistence transport to
match the current session mode. The startup notice lives before the main `App`
event loop exists, so its persistence path must be startup-local rather than
an `AppEvent` round trip.

## Error Handling

- If the pooled startup probe fails, fail open to the existing login behavior.
- If persisting the hide flag fails, do not block entry into the TUI.
- If the user selects `L`, hand off to the existing login onboarding without
  changing pooled state.
- If the user resumes paused pooled startup and resume fails, stay on the
  paused notice and surface the error inline.

In all cases, prefer warning logs and preserved forward progress over startup
hard failures.

## Testing Strategy

### TUI startup decision tests

Add focused tests for the startup decision helper:

- shared login present -> `NoPrompt`
- pooled-only available and notice visible -> `PooledOnlyNotice`
- pooled-only available and notice hidden -> `NoPrompt`
- pooled startup suppressed -> `PooledAccessPausedNotice`
- no shared login and no pooled account -> `NeedsLogin`

### Onboarding interaction tests

Add focused tests for the new notice step:

- `Enter` continues into the TUI
- `L` reveals the existing auth step
- `N` persists the hide flag and continues

Add focused tests for the paused notice:

- `Enter` issues resume and continues on success
- `L` reveals the existing auth step
- resume failure stays on the paused notice and renders the error

### Snapshot coverage

Add rendered snapshot coverage for:

- the pooled-only notice widget
- the pooled-access-paused notice widget

### Integration coverage

Add a narrow startup integration test that proves:

- pooled-only startup shows the new notice instead of the existing login step
- paused pooled startup shows the paused notice instead of the continue notice
- remote-mode startup can surface the pooled-only notice from existing
  `accountLease/read` state without protocol expansion

No protocol expansion or broad end-to-end harness changes are required.

## Rationale

This design is the best fit for the branch goals because it:

- fixes the user-facing gap without redefining shared auth
- keeps changes localized to TUI startup and config notice persistence
- preserves compatibility with future remote account-pool backends
- minimizes upstream merge surface by avoiding protocol and auth-core churn
