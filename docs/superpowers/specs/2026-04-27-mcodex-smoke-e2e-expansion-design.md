# Mcodex Smoke And E2E Expansion Design

This document extends
`docs/superpowers/specs/2026-04-27-mcodex-smoke-test-matrix-design.md` after
the first automated smoke slice landed. The first slice added local and CLI
smoke coverage. This follow-up defines how to add broader smoke and E2E
coverage, with the first implementation slice focused on quota-aware account
switching and merge-gate confidence.

The goal is maximum useful coverage without turning smoke into a full workspace
test run or creating a high-churn harness that conflicts with upstream merges.

## Current Baseline

The repository currently has:

- `just smoke-mcodex-local`
- `just smoke-mcodex-cli`
- `just smoke-mcodex-all`, currently aggregating the local and CLI smoke
  scripts
- `codex-smoke-fixtures`, which seeds isolated account-pool homes for local and
  CLI smoke
- a P0 runbook at
  `docs/superpowers/runbooks/2026-04-27-mcodex-smoke-p0.md`
- crate-level runtime, quota, app-server, TUI, and subagent regressions that
  already exercise important account-pool behavior

The current automated smoke suite proves startup selection, local home
isolation, default-pool precedence, and observability output. It does not prove
that runtime quota pressure actually causes the next safe request to use a
different account, nor does it provide a single merge-gate recipe that includes
the most important named regressions.

## Goals

- Add automated smoke/E2E coverage for quota-aware account switching.
- Verify that a selected account remains sticky until quota or lease facts make
  switching necessary.
- Verify that soft quota pressure, hard quota exhaustion, damping, and no
  eligible account behavior are covered by a repeatable gate.
- Verify that lease exclusivity prevents multiple runtime instances from using
  the same account simultaneously.
- Surface app-server and TUI regressions that would make automatic switching
  invisible or misleading to users.
- Provide one local merge-gate command that runs the cheap product smoke checks
  plus the most important named account-pool regressions.
- Keep the design compatible with future remote account-pool backends.
- Keep upstream merge risk low by adding harnesses, fixtures, scripts, and
  narrowly targeted tests instead of product-path rewrites.

## Non-Goals

- Do not run full workspace `cargo test` as part of the default smoke gate.
- Do not intentionally exhaust real account quota.
- Do not require real ChatGPT accounts for automated switching tests.
- Do not require a production remote account-pool service.
- Do not implement headless TUI automation in the first expansion slice.
- Do not broaden `smoke-mcodex-all` in a surprising way without a documented
  compatibility decision.
- Do not duplicate every unit test branch in shell smoke.

## Design Principles

### Smoke Tests Are Product Gates, Not Unit Test Replacements

Smoke should answer whether a developer can trust a local build or merged
branch for the account-pool flows that matter. Branch-level correctness stays in
unit and integration tests. Smoke should stitch together a small number of
high-value entrypoints and named regressions.

### Prefer Existing Targeted Tests Before Writing New Harnesses

The codebase already has focused tests for runtime quota switching, app-server
lease notifications, TUI status rendering, and subagent lease inheritance. The
first expansion should reuse those tests through stable recipes and add only the
missing tests needed to close clear behavior gaps.

### Fail Closed On Skipped Critical Regressions

Some runtime tests use environment guards. A smoke gate must not report success
if a critical named regression was skipped, ignored, or matched zero tests. The
gate should parse command output enough to fail with an explicit message when a
critical test did not actually execute.

### Keep Fixtures Isolated

All automated smoke must use temporary or explicitly provided `MCODEX_HOME`
paths. Smoke must clear `CODEX_HOME` and `CODEX_SQLITE_HOME` except for rows
that intentionally test conflicts. Real account homes are manual canaries only.

### Preserve Remote Backend Shape

The first implementation remains local, but test names, fixtures, and expected
facts should describe authority-neutral behavior: pool inventory, lease
ownership, quota facts, pause/drain facts, and observability events. Local
SQLite should not become the only conceptual source of truth.

## Proposed Command Taxonomy

### Existing Commands

Keep the existing commands stable:

| Command | Meaning |
| --- | --- |
| `just smoke-mcodex-local` | Product binary, home isolation, startup/default local rows |
| `just smoke-mcodex-cli` | CLI account-pool observability rows |
| `just smoke-mcodex-all` | Current local+CLI aggregate until a compatibility decision changes it |

`smoke-mcodex-all` should not be silently redefined as a heavy E2E command. It
can later become an alias for `smoke-mcodex-gate` only after the runbook and
developer documentation say so.

### New Commands

Add narrowly named commands:

| Command | Default Weight | Coverage |
| --- | --- | --- |
| `just smoke-mcodex-runtime-gate` | Medium | Runtime lease, sticky account, quota switch, fail-closed behavior |
| `just smoke-mcodex-quota-gate` | Medium | Quota near/exhausted/damping named regressions |
| `just smoke-mcodex-app-server-gate` | Medium | Lease read/update, default mutation, auto-switch notification |
| `just smoke-mcodex-tui-gate` | Medium | TUI status/onboarding snapshots and automatic switch notices |
| `just smoke-mcodex-gate` | Medium | Local + CLI + runtime + quota gate |
| `just smoke-mcodex-e2e` | Heavy | Gate plus app-server and TUI gates |
| `just smoke-mcodex-installer` | Manual/medium | Local install wrapper identity and path forwarding |
| `just smoke-mcodex-remote-contract` | Deferred | Fake remote backend contract rows |

The first expansion should implement `runtime-gate`, `quota-gate`, and
`gate`. App-server and TUI gates can follow immediately after if the named
tests are stable and do not add significant disk pressure.

## First Expansion Scope

The first implementation slice should add the smallest useful automatic
switching gate.

### Runtime Gate

`smoke-mcodex-runtime-gate` should run named tests that prove runtime account
selection uses lease-scoped auth and remains exclusive:

- `pooled_request_uses_lease_scoped_auth_session_not_shared_auth_snapshot`
- `pooled_request_ignores_shared_external_auth_when_lease_is_active`
- `lease_rotation_updates_live_snapshot_to_the_new_lease`
- `long_running_turn_heartbeat_keeps_lease_exclusive`
- `shutdown_releases_active_lease_for_next_runtime`
- `run_codex_thread_interactive_inherits_parent_runtime_lease_host`
- `run_codex_thread_interactive_drops_inherited_lease_auth_when_runtime_host_exists`

If any of these names are renamed upstream, the gate should fail with a clear
"named regression not found" message rather than silently passing.

### Quota Gate

`smoke-mcodex-quota-gate` should run named tests that prove quota pressure
changes account choice safely:

- `nearing_limit_snapshot_rotates_the_next_turn_before_exhaustion`
- `usage_limit_reached_rotates_only_future_turns_on_responses_transport`
- `hard_failover_uses_active_limit_family_through_runtime_authority`
- `proactive_rotation_does_not_immediately_switch_back_to_just_replaced_account`
- `account_lease_snapshot_reports_proactive_switch_suppression_without_rate_limited_health`
- `exhausted_pool_fails_closed_without_legacy_auth_fallback`
- `pooled_fail_closed_turn_without_eligible_lease_does_not_open_startup_websocket`

These tests cover the important policy:

- near-limit telemetry does not replay the current turn
- the next safe request can move from account A to account B
- a hard quota failure blocks future use of the exhausted account
- damping prevents switch churn
- no eligible account fails closed instead of falling back to shared auth

### Sticky Account Gap

The current named set has strong switching coverage, but the first expansion
should also ensure an explicit sticky-account regression exists. If no existing
test proves this exact behavior, add one focused test before wiring the gate:

1. Seed two eligible accounts in one pool.
2. Submit two normal turns with no quota pressure.
3. Assert both outbound requests use the same first account.
4. Assert no automatic switch event is emitted.

This test belongs in the existing runtime account-pool test surface, not in a
shell script.

### Multi-Instance Lease Gap

The gate should prove that separate runtime holders do not share one account.
If existing coverage only proves heartbeat/exclusivity inside one runtime, add
one focused regression:

1. Seed two accounts in one pool.
2. Start runtime A and acquire account A.
3. Start runtime B against the same isolated home.
4. Assert runtime B uses account B or fails closed when no eligible account is
   available.
5. Assert runtime B does not use account A while A's lease is live.

This can be a crate-level E2E test with mock responses. It should not require
real processes unless the existing harness already makes that cheap and stable.

## App-Server And TUI Expansion

After the first gate is stable, add app-server and TUI gates by composing
existing named tests.

### App-Server Gate

`smoke-mcodex-app-server-gate` should include:

- `account_lease_read_includes_startup_snapshot_for_single_pool_fallback`
- `account_lease_read_preserves_candidate_pools_for_multi_pool_blocker`
- `account_lease_read_reports_live_active_lease_fields_after_turn_start`
- `account_lease_read_and_update_report_live_proactive_switch_suppression_fields`
- `account_lease_updated_emits_on_resume`
- `account_lease_updated_emits_when_automatic_switch_changes_live_snapshot`
- `account_pool_default_set_reuses_cli_mutation_matrix`
- `account_pool_default_clear_noop_does_not_emit_notification`
- `websocket_account_pool_default_set_and_clear_mutate_startup_intent_without_runtime_admission`

This gate proves app-server clients can see the same startup, lease, damping,
and automatic-switch facts that runtime and CLI use.

### TUI Gate

`smoke-mcodex-tui-gate` should include:

- `pooled_notice_does_not_show_login_screen_until_requested`
- `status_command_renders_pooled_lease_details`
- `account_lease_updated_adds_automatic_switch_notice_when_account_changes`
- `account_lease_updated_adds_non_replayable_turn_notice`
- `account_lease_updated_adds_no_eligible_account_error_notice`
- `status_command_renders_damped_account_lease_without_next_eligible_hint`
- `status_snapshot_shows_active_pool_and_next_eligible_time`
- `status_snapshot_shows_auto_switch_and_remote_reset_messages`
- `status_snapshot_shows_damped_account_lease_without_next_eligible_time`
- `status_snapshot_shows_no_available_account_error_state`

This gate remains snapshot/unit level initially. Headless TUI automation should
be a later P2 item because it is more likely to be flaky and more sensitive to
terminal details.

## Gate Runner Behavior

The implementation should add a small runner script rather than duplicating
long `cargo test` invocations in `justfile`.

Suggested shape:

- `scripts/smoke/run-named-cargo-tests.sh`
- accepts crate package plus one or more test names
- runs each named test separately, or in small groups when stable
- passes through `HTTPS_PROXY`, `LK_CUSTOM_WEBRTC`, and user-provided cargo env
- records command, package, test name, and elapsed time
- treats these as failure:
  - cargo exits nonzero
  - output reports `running 0 tests`
  - output reports the test was ignored or skipped for a critical gate
  - output does not mention the requested test name

Running each named test separately is slower than one broad package command but
gives better failure messages and prevents renamed tests from disappearing.
After the gate stabilizes, groups can be batched if runtime becomes too slow.

## Fake Fixture Strategy

The existing `codex-smoke-fixtures` crate should remain the fixture owner for
state-only smoke. Runtime E2E should continue to use core test support and mock
responses because the critical assertion is which account authenticated the
outbound request.

Add new fixture scenarios only when shell or CLI smoke needs persisted state:

- `sticky-two-accounts`
- `quota-soft-pressure`
- `quota-hard-blocked`
- `busy-lease`
- `no-eligible`

Do not seed real credentials. The fixture summary should identify whether
credentials are fake, absent, or real. Automated gates should only use fake or
absent credentials.

## Real Account Canary

Real accounts should stay out of automated smoke by default. A manual canary can
exist in the runbook:

- launch installed `mcodex` with a real `MCODEX_HOME`
- verify `accounts status`
- run one low-risk prompt if the user explicitly wants to spend quota
- do not attempt quota exhaustion
- record the account pool and effective account id before and after

This canary validates local usability, not switching policy.

## Future Remote Contract Smoke

When fake remote backend support is available, add
`smoke-mcodex-remote-contract` with these rows:

- remote pool inventory read
- remote pause state blocks startup with a clear source
- remote drain state prevents new selection but preserves observability facts
- remote quota facts appear as authoritative remote facts
- absent remote-only facts are represented explicitly, not synthesized from
  local SQLite
- remote lease acquisition and release preserve the same sticky and fail-closed
  semantics as local leases

The remote contract should use the same user-facing output checks as local
smoke where possible, so future production remote support does not require a
second UX.

## Merge And Disk Policy

The default merge gate should stay below full workspace test cost:

1. Build `mcodex` only when a product binary smoke command needs it.
2. Run local and CLI smoke against the selected `MCODEX_BIN`.
3. Run named cargo regressions for runtime and quota.
4. Do not run full `cargo test`.
5. Do not require `--all-features`.
6. Let heavier app-server/TUI/installer gates be opt-in or release-candidate
   gates until their runtime cost is known.

If disk pressure is high, the gate should prefer existing build artifacts and
should not trigger release builds unless the installer or wrapper smoke row is
requested.

## Acceptance Criteria

- `just smoke-mcodex-gate` exists and runs local, CLI, runtime, and quota gates.
- `just smoke-mcodex-runtime-gate` proves lease-scoped auth, runtime lease
  inheritance, lease exclusivity, and release behavior.
- `just smoke-mcodex-quota-gate` proves sticky behavior, near-limit future-turn
  rotation, hard quota failover, damping, and fail-closed no-eligible behavior.
- The gate fails if a critical named regression is skipped, ignored, or not
  found.
- No automated gate uses real account quota.
- Existing `smoke-mcodex-local`, `smoke-mcodex-cli`, and `smoke-mcodex-all`
  behavior remains compatible unless explicitly migrated.
- The runbook identifies which rows are automated by `smoke-mcodex-gate` and
  which remain manual or deferred.
- App-server and TUI gates are either implemented as follow-up commands or
  documented as the next expansion slice with exact named tests.
- Remote contract smoke remains deferred but has explicit expected rows and
  does not force local SQLite assumptions into future remote behavior.

## Recommended Implementation Order

1. Add the named-test runner and verify it fails on a deliberately missing test
   name.
2. Add `smoke-mcodex-runtime-gate` using existing named tests.
3. Add or confirm the sticky-account regression.
4. Add or confirm the multi-instance lease regression.
5. Add `smoke-mcodex-quota-gate`.
6. Add `smoke-mcodex-gate` as local + CLI + runtime + quota.
7. Update the P0 runbook with the new automated gate rows.
8. Add app-server and TUI gates as the next slice once runtime/quota gate cost
   is measured.

## Decision

Adopt a layered smoke/E2E expansion:

- keep existing local and CLI smoke stable
- add runtime and quota gates first
- fail closed when named critical regressions do not actually execute
- add app-server and TUI gates next
- leave installer, headless TUI, real-account canary, and fake remote contract
  as explicit later rows

This gives mcodex a practical merge gate for automatic account switching while
keeping the work mostly in scripts, tests, fixtures, and docs, which minimizes
upstream merge risk.
