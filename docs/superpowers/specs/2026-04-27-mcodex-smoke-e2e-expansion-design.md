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

For runtime and quota gates, `CODEX_SANDBOX_NETWORK_DISABLED` is a blocking
environment condition. Several critical tests use `skip_if_no_network!`, which
prints:

`Skipping test because it cannot execute when network is disabled in a Codex sandbox.`

and then returns success from the test body. Because the Rust test harness
captures stdout by default for passing tests, the runner must not rely on cargo
exit status alone. Runtime and quota gates must either fail before invoking
cargo when `CODEX_SANDBOX_NETWORK_DISABLED` is set, or run with
`-- --nocapture` and treat the exact skip sentinel as a failed gate. The
preferred first implementation should do both: fail early on the environment
variable and still run critical tests with `--nocapture` so accidental skips are
visible.

This means `smoke-mcodex-runtime-gate`, `smoke-mcodex-quota-gate`, and
`smoke-mcodex-gate` must run from a network-enabled local shell or CI
environment. If a developer runs them from a Codex sandbox where
`CODEX_SANDBOX_NETWORK_DISABLED` is set, the expected result is an explicit
failure that says to rerun outside the Codex sandbox or in network-enabled CI.

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

| Command | Level | Weight | Coverage |
| --- | --- | --- | --- |
| `just smoke-mcodex-runtime-gate` | P1 gate | Medium | Runtime lease, sticky account, quota switch, fail-closed behavior |
| `just smoke-mcodex-quota-gate` | P1 gate | Medium | Quota near/exhausted/damping named regressions |
| `just smoke-mcodex-app-server-gate` | P1/P2 gate | Medium | Lease read/update, default mutation, auto-switch notification |
| `just smoke-mcodex-tui-gate` | P1/P2 gate | Medium | TUI status/onboarding snapshots and automatic switch notices |
| `just smoke-mcodex-gate` | P1 merge gate | Medium | Local + CLI + runtime + quota gate |
| `just smoke-mcodex-e2e` | P2/release gate | Heavy | Gate plus app-server and TUI gates |
| `just smoke-mcodex-installer` | P2/manual | Manual/medium | Local install wrapper identity and path forwarding |
| `just smoke-mcodex-remote-contract` | Deferred P2 | Deferred | Fake remote backend contract rows |

The first expansion should implement `runtime-gate`, `quota-gate`, and
`gate`. App-server and TUI gates can follow immediately after if the named
tests are stable and do not add significant disk pressure.

### Compatibility With Earlier Matrix Names

The earlier smoke matrix reserved broader command names such as
`smoke-mcodex-runtime`, `smoke-mcodex-quota`, and
`smoke-mcodex-app-server`. The `*-gate` names in this spec are not a second
taxonomy; they are the merge-gate subsets of those broader groups.

| Earlier reserved name | First expansion behavior | Longer-term behavior |
| --- | --- | --- |
| `smoke-mcodex-runtime` | Alias to `smoke-mcodex-runtime-gate`, or documented as not implemented yet | Broader runtime smoke may include non-gate slow checks |
| `smoke-mcodex-quota` | Alias to `smoke-mcodex-quota-gate`, or documented as not implemented yet | Broader quota smoke may include probe/backoff rows |
| `smoke-mcodex-app-server` | Alias to `smoke-mcodex-app-server-gate` when app-server gate lands | Broader app-server smoke may include websocket and remote-client rows |
| `smoke-mcodex-all` | Remains the existing local+CLI aggregate | May only change after runbook/docs explicitly migrate it |
| `smoke-mcodex-gate` | New local merge gate | Stable command for main-branch merge validation |

The implementation plan must update the P0 runbook so the future rows and
command names do not diverge.

## First Expansion Scope

The first implementation slice should add the smallest useful automatic
switching gate.

### Runtime Gate

`smoke-mcodex-runtime-gate` should run named tests that prove runtime account
selection uses lease-scoped auth, remains sticky when no switch fact exists,
and remains exclusive across runtime holders:

| Package | Target | Exact Test Path | Status |
| --- | --- | --- | --- |
| `codex-core` | `--test all` | `suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure` | New required test before gate is complete |
| `codex-core` | `--test all` | `suite::account_pool::second_runtime_skips_account_leased_by_first_runtime` | New required test before gate is complete |
| `codex-core` | `--test all` | `suite::account_pool::pooled_request_uses_lease_scoped_auth_session_not_shared_auth_snapshot` | Existing |
| `codex-core` | `--test all` | `suite::account_pool::pooled_request_ignores_shared_external_auth_when_lease_is_active` | Existing |
| `codex-core` | `--test all` | `suite::account_pool::lease_rotation_updates_live_snapshot_to_the_new_lease` | Existing |
| `codex-core` | `--test all` | `suite::account_pool::long_running_turn_heartbeat_keeps_lease_exclusive` | Existing, known-slow |
| `codex-core` | `--test all` | `suite::account_pool::shutdown_releases_active_lease_for_next_runtime` | Existing |
| `codex-core` | `--lib` | `codex_delegate::tests::run_codex_thread_interactive_inherits_parent_runtime_lease_host` | Existing |
| `codex-core` | `--lib` | `codex_delegate::tests::run_codex_thread_interactive_drops_inherited_lease_auth_when_runtime_host_exists` | Existing |

If any of these names are renamed upstream, the gate should fail with a clear
"named regression not found" message rather than silently passing.

### Quota Gate

`smoke-mcodex-quota-gate` should run named tests that prove quota pressure
changes account choice safely:

| Package | Target | Exact Test Path | Status |
| --- | --- | --- | --- |
| `codex-core` | `--test all` | `suite::account_pool::nearing_limit_snapshot_rotates_the_next_turn_before_exhaustion` | Existing |
| `codex-core` | `--test all` | `suite::account_pool::usage_limit_reached_rotates_only_future_turns_on_responses_transport` | Existing |
| `codex-core` | `--test all` | `suite::account_pool::hard_failover_uses_active_limit_family_through_runtime_authority` | Existing |
| `codex-core` | `--test all` | `suite::account_pool::proactive_rotation_does_not_immediately_switch_back_to_just_replaced_account` | Existing |
| `codex-core` | `--test all` | `suite::account_pool::account_lease_snapshot_reports_proactive_switch_suppression_without_rate_limited_health` | Existing |
| `codex-core` | `--test all` | `suite::account_pool::exhausted_pool_fails_closed_without_legacy_auth_fallback` | Existing |
| `codex-core` | `--test all` | `suite::client_websockets::pooled_fail_closed_turn_without_eligible_lease_does_not_open_startup_websocket` | Existing |

These tests cover the important policy:

- near-limit telemetry does not replay the current turn
- the next safe request can move from account A to account B
- a hard quota failure blocks future use of the exhausted account
- damping prevents switch churn
- no eligible account fails closed instead of falling back to shared auth

### Sticky Account Gap

The current named set has strong switching coverage, but the first expansion
must also add an explicit sticky-account regression before
`smoke-mcodex-runtime-gate` can be considered implemented:

1. Seed two eligible accounts in one pool.
2. Submit two normal turns with no quota pressure.
3. Assert both outbound requests use the same first account.
4. Assert no automatic switch event is emitted.

This test belongs in the existing runtime account-pool test surface, not in a
shell script. Its required test path is
`suite::account_pool::normal_turns_remain_on_same_account_without_quota_pressure`
under `codex-core --test all`.

### Multi-Instance Lease Gap

The gate should prove that separate runtime holders do not share one account.
The first expansion must add one focused regression before
`smoke-mcodex-runtime-gate` can be considered implemented:

1. Seed two accounts in one pool.
2. Start runtime A and acquire account A.
3. Start runtime B against the same isolated home.
4. Assert runtime B uses account B while account B is eligible.
5. Assert runtime B does not use account A while A's lease is live.
6. Add a separate single-account or all-accounts-unavailable branch where
   runtime B fails closed instead of using account A.

This can be a crate-level E2E test with mock responses. It should not require
real processes unless the existing harness already makes that cheap and stable.
Its required test path is
`suite::account_pool::second_runtime_skips_account_leased_by_first_runtime`
under `codex-core --test all`.

## App-Server And TUI Expansion

After the first gate is stable, add app-server and TUI gates by composing
existing named tests.

### App-Server Gate

`smoke-mcodex-app-server-gate` should include:

| Package | Target | Exact Test Path | Status |
| --- | --- | --- | --- |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::account_lease_read_includes_startup_snapshot_for_single_pool_fallback` | Existing |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::account_lease_read_preserves_candidate_pools_for_multi_pool_blocker` | Existing |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::account_lease_read_reports_live_active_lease_fields_after_turn_start` | Existing |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::account_lease_read_and_update_report_live_proactive_switch_suppression_fields` | Existing |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::account_lease_updated_emits_on_resume` | Existing |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::account_lease_updated_emits_when_automatic_switch_changes_live_snapshot` | Existing |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::account_pool_default_set_reuses_cli_mutation_matrix` | Existing |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::account_pool_default_clear_noop_does_not_emit_notification` | Existing |
| `codex-app-server` | `--test all` | `suite::v2::account_lease::websocket_account_pool_default_set_and_clear_mutate_startup_intent_without_runtime_admission` | Existing |

This gate proves app-server clients can see the same startup, lease, damping,
and automatic-switch facts that runtime and CLI use.

### TUI Gate

`smoke-mcodex-tui-gate` should include:

| Package | Target | Exact Test Path | Status |
| --- | --- | --- | --- |
| `codex-tui` | `--lib` | `tests::pooled_notice_does_not_show_login_screen_until_requested` | Existing |
| `codex-tui` | `--lib` | `chatwidget::tests::status_command_tests::status_command_renders_pooled_lease_details` | Existing |
| `codex-tui` | `--lib` | `chatwidget::tests::status_command_tests::account_lease_updated_adds_automatic_switch_notice_when_account_changes` | Existing |
| `codex-tui` | `--lib` | `chatwidget::tests::status_command_tests::account_lease_updated_adds_non_replayable_turn_notice` | Existing |
| `codex-tui` | `--lib` | `chatwidget::tests::status_command_tests::account_lease_updated_adds_no_eligible_account_error_notice` | Existing |
| `codex-tui` | `--lib` | `chatwidget::tests::status_command_tests::status_command_renders_damped_account_lease_without_next_eligible_hint` | Existing |
| `codex-tui` | `--lib` | `status::tests::status_snapshot_shows_active_pool_and_next_eligible_time` | Existing |
| `codex-tui` | `--lib` | `status::tests::status_snapshot_shows_auto_switch_and_remote_reset_messages` | Existing |
| `codex-tui` | `--lib` | `status::tests::status_snapshot_shows_damped_account_lease_without_next_eligible_time` | Existing |
| `codex-tui` | `--lib` | `status::tests::status_snapshot_shows_no_available_account_error_state` | Existing |

This gate remains snapshot/unit level initially. Headless TUI automation should
be a later P2 item because it is more likely to be flaky and more sensitive to
terminal details.

## Gate Runner Behavior

The implementation should add a small runner script rather than duplicating
long `cargo test` invocations in `justfile`.

Suggested shape:

- `scripts/smoke/run-named-cargo-tests.sh`
- accepts one or more test descriptors with:
  - package, for example `codex-core`
  - target kind, for example `--lib` or `--test`
  - target name when needed, for example `all`
  - exact test path, for example
    `suite::account_pool::nearing_limit_snapshot_rotates_the_next_turn_before_exhaustion`
- runs with `--manifest-path "$REPO_ROOT/codex-rs/Cargo.toml"`
- runs with an explicit target, never a package-wide substring-only filter
- runs critical tests with `-- --exact --nocapture`
- performs preflight listing with the same package, target, and exact filter:
  `cargo test --manifest-path "$REPO_ROOT/codex-rs/Cargo.toml" -p "$package" "$target_kind" "$target_name" "$exact_path" -- --exact --list`
- accepts preflight success only when exactly one list line equals
  `$exact_path: test`
- runs the test with the same package, target, and exact filter:
  `cargo test --manifest-path "$REPO_ROOT/codex-rs/Cargo.toml" -p "$package" "$target_kind" "$target_name" "$exact_path" -- --exact --nocapture`
- records command, package, target, exact test path, and elapsed time
- inherits cargo and network environment unchanged, including:
  - `HTTPS_PROXY`
  - `HTTP_PROXY`
  - `ALL_PROXY`
  - `NO_PROXY`
  - `CARGO_NET_GIT_FETCH_WITH_CLI`
  - `RUSTY_V8_ARCHIVE`
  - `LK_CUSTOM_WEBRTC`
  - `CARGO_TARGET_DIR`
- treats these as failure:
  - cargo exits nonzero
  - `CODEX_SANDBOX_NETWORK_DISABLED` is set for runtime or quota gates
  - `-- --list` does not show the exact requested test path in the requested
    target
  - `-- --list` shows more than one exact requested test path match
  - the run output contains the exact `skip_if_no_network!` sentinel
  - the run output reports the exact test as ignored
  - the run output does not prove that the requested exact test executed once

The runner must not treat any global `running 0 tests` line as failure when it
runs package-wide commands, because cargo can print that line for non-target
test binaries while the requested target still executes. The preferred design is
to avoid that ambiguity entirely by always using explicit targets. If the
runner later batches tests by package/target, it must verify the expected set
from the target's `-- --list` output and then verify the executed set from the
run output instead of relying on substring filters.

Top-level `just` recipes must follow the existing smoke pattern:

- mark recipes `[no-cd]`
- resolve scripts through `{{ justfile_directory() }}`
- pass the repo root or derive it inside the script
- call cargo with `--manifest-path "$REPO_ROOT/codex-rs/Cargo.toml"`

Running each exact test separately gives the clearest failure messages. To
control cost, the implementation should first run `cargo test --no-run` once
per package/target pair and then reuse the warmed target directory for exact
test invocations. The known-slow
`suite::account_pool::long_running_turn_heartbeat_keeps_lease_exclusive` test
should be called out in command output and should have an explicit per-test
timeout budget rather than inheriting a short generic timeout.

## Fake Fixture Strategy

The existing `codex-smoke-fixtures` crate should remain the fixture owner for
state-only smoke. Runtime E2E should continue to use core test support and mock
responses because the critical assertion is which account authenticated the
outbound request.

Add new fixture scenarios only when shell or CLI smoke needs persisted state:

| Scenario | Consumer | Priority |
| --- | --- | --- |
| `busy-lease` | CLI/app-server observability smoke and future remote contract parity | P1 after runtime/quota gate |
| `no-eligible` | CLI/app-server observability smoke and TUI error display setup | P1/P2 |
| `quota-soft-pressure` | CLI/app-server diagnostics and events smoke | P2, because runtime switching uses core test support first |
| `quota-hard-blocked` | CLI/app-server diagnostics and events smoke | P2, because runtime switching uses core test support first |
| `sticky-two-accounts` | Only if shell/CLI needs a persisted-state status row | Deferred; runtime sticky behavior belongs in core test support |

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
- remote backend unavailable fails closed when no valid active lease exists
- remote pause state blocks startup with a clear source
- remote drain state prevents new selection but preserves observability facts
- remote quota facts appear as authoritative remote facts
- absent remote-only facts are represented explicitly, not synthesized from
  local SQLite
- remote lease acquisition and release preserve the same sticky and fail-closed
  semantics as local leases
- remote lease expiry or revocation invalidates the active lease immediately
- missing or unavailable remote lease auth fails closed without falling back to
  local shared auth
- preferred and excluded account identities use stable mirrored ids rather than
  provider secrets
- local `MCODEX_HOME` never persists remote secrets or remote-only authority
  facts as if they were local source-of-truth rows
- every remote-derived output includes authority/source provenance so operators
  can distinguish remote facts from local cached observations

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

The runtime and quota gates are mandatory for `smoke-mcodex-gate`; they are not
optional just because they are more expensive than local/CLI smoke. However,
their first cold build may still pull large dependencies. In particular,
`codex-core --test all` can compile the `codex-code-mode` dependency graph,
which includes `v8`. Exact test filters reduce how many tests run, but they do
not shrink the target's compile graph.

The implementation plan should include a cost-measurement step for each new
gate. Record cold and warm timings for at least:

- `codex-core --test all`
- `codex-core --lib`
- `codex-app-server --test all`
- `codex-tui --lib`

The implementation plan must record target directory size before and after the
new runtime/quota gate on a cold build and define a local free-disk warning
threshold. If the cold-build cost is too high for routine local use,
`smoke-mcodex-gate` still includes runtime/quota, but the runbook should offer a
documented lighter command for local smoke and reserve `smoke-mcodex-gate` for
merge or CI use. App-server and TUI gates may stay under `smoke-mcodex-e2e`
until their cost is understood. The plan should also document local artifact
requirements for large dependencies, especially `RUSTY_V8_ARCHIVE` and
`LK_CUSTOM_WEBRTC`, and should leave proxy variables untouched.

## Acceptance Criteria

- `just smoke-mcodex-gate` exists and runs local, CLI, runtime, and quota gates.
- `just smoke-mcodex-runtime-gate` proves sticky account behavior,
  lease-scoped auth, runtime lease inheritance, cross-runtime lease
  exclusivity, and release behavior.
- `just smoke-mcodex-quota-gate` proves near-limit future-turn rotation, hard
  quota failover, damping, and fail-closed no-eligible behavior.
- The gate fails if a critical named regression is skipped, ignored, not found,
  matched more than once, or bypassed by `CODEX_SANDBOX_NETWORK_DISABLED`.
- No automated gate uses real account quota.
- Existing `smoke-mcodex-local`, `smoke-mcodex-cli`, and `smoke-mcodex-all`
  behavior remains compatible unless explicitly migrated.
- Earlier reserved names such as `smoke-mcodex-runtime`, `smoke-mcodex-quota`,
  and `smoke-mcodex-app-server` are explicitly aliased, deferred, or documented
  as broader non-gate commands.
- The runbook identifies which rows are automated by `smoke-mcodex-gate` and
  which remain manual or deferred.
- App-server and TUI gates are either implemented as follow-up commands or
  documented as the next expansion slice with exact named tests.
- Remote contract smoke remains deferred but has explicit expected rows for
  unavailable backend, pause, drain, quota, lease expiry/revocation, lease-auth
  unavailability, mirrored ids, secret non-persistence, and authority/source
  provenance.

## Recommended Implementation Order

1. Add the named-test runner and verify it fails on a deliberately missing test
   name.
2. Verify the runner fails when `CODEX_SANDBOX_NETWORK_DISABLED=1` for a
   runtime/quota gate.
3. Add the sticky-account regression under `codex-core --test all`.
4. Add the multi-instance lease regression under `codex-core --test all`.
5. Add `smoke-mcodex-runtime-gate` using exact package/target/test
   descriptors.
6. Add `smoke-mcodex-quota-gate` using exact package/target/test descriptors.
7. Measure cold and warm runtime cost for runtime and quota gates.
8. Add `smoke-mcodex-gate` as local + CLI + runtime + quota. If the runtime or
   quota cold-build cost is high, document a lighter local-only command
   separately; do not redefine the merge gate to omit runtime/quota.
9. Update the P0 runbook with the new automated gate rows and command-name
   compatibility table.
10. Add app-server and TUI gates as the next slice once runtime/quota gate cost
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
