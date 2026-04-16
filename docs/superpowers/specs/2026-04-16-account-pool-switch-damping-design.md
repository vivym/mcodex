# Account Pool Switch Damping Design

This document defines how pooled automatic switching should be damped so one
runtime instance keeps a leased account stable for a minimum interval before
proactive rotation, while still allowing immediate failover for genuine hard
failures.

It is intentionally scoped to automatic switch timing, soft versus hard
rotation signals, and operator-visible diagnostics. It does not redesign the
broader remote backend contract, app-server multi-client semantics, or the
existing manual switch commands.

## Summary

The recommended direction is:

- treat `accounts.min_switch_interval_secs` as a runtime-local throttle on
  proactive automatic switching only
- distinguish soft near-limit observations from hard lease-health failures
- keep threshold-only proactive pressure out of authoritative durable
  `RateLimited` or cooldown persistence
- keep hard failures such as `usage_limit_reached`, unauthorized, lease loss,
  and backend revocation outside the damping window
- avoid persisting damping state in SQLite as though it were installation-wide
  startup selection
- expose additive live diagnostics so CLI, TUI, and future doctor surfaces can
  explain when proactive rotation was intentionally suppressed
- keep the design compatible with a future remote backend that may supply more
  authoritative cooldown hints or revocation signals

This addresses the current gap where the config schema already includes
`accounts.min_switch_interval_secs`, but the runtime does not yet have a clear,
shared semantic contract for it.

## Goals

- Reduce account flapping when usage hovers near the proactive switch threshold.
- Lower the risk of unnecessary provider-side risk signals caused by frequent
  account changes.
- Preserve sticky runtime-instance behavior where one instance keeps one
  account until there is a strong reason to move.
- Keep hard-failure failover responsive.
- Make switch suppression understandable in status and diagnostic surfaces.
- Keep the design merge-friendly by localizing new semantics to account-pool
  policy and additive diagnostics instead of rewriting auth or startup state.

## Non-Goals

- Do not change the meaning of manual `codex accounts switch`.
- Do not redesign the current-turn replay rules for hard failures.
- Do not introduce a large account-management UI.
- Do not require a remote backend implementation in this slice.
- Do not make pooled startup selection durable across product homes or shared
  across different runtime instances.

## Constraints

- Upstream mergeability matters, so the design should avoid broad rewrites of
  `codex-core` request execution and auth plumbing.
- Existing config already exposes:
  - `accounts.proactive_switch_threshold_percent`
  - `accounts.min_switch_interval_secs`
  - `accounts.lease_ttl_secs`
- Existing state already distinguishes startup-selection state from live lease
  and health state.
- A future remote backend may become authoritative for cooldown or lease
  revocation, so local damping must not assume the local backend owns all
  timing decisions forever.
- Multiple runtime instances may share one product home, so durable writes must
  not turn a runtime-local throttling decision into a global startup policy.

## Problem Statement

The current multi-account pool behavior has a threshold-based proactive switch
concept, but no shared semantic contract for a minimum stable-hold interval.

That leaves several problems:

- a runtime can rotate too quickly when usage repeatedly crosses the same
  threshold boundary
- operator intent in `accounts.min_switch_interval_secs` is not actually
  enforced end to end
- status surfaces cannot cleanly explain why an automatic switch did or did not
  happen
- future remote backend work has no stable local semantic boundary to conform
  to

There is also an important conceptual mismatch between two kinds of signals:

1. soft "near limit" observations derived from proactive threshold checks
2. hard failures such as `usage_limit_reached`, unauthorized, revoked lease, or
   missing lease renewal

Those signals should not be treated the same. Soft observations should not
force the runtime to behave as though the account is already exhausted or
durably unhealthy across the entire installation.

## Approaches Considered

### Approach A: Apply the minimum interval to every switch

Under this approach, once a runtime acquires an account, no automatic switch is
allowed until the interval expires, including hard failures.

Pros:

- very simple rule
- easy to explain

Cons:

- wrong for real failures
- can strand the runtime on an exhausted or unauthorized account
- conflicts with the explicit goal of responsive hard-failure failover

This approach is rejected.

### Approach B: Make the interval a runtime-local throttle on proactive switch only

Under this approach:

- soft threshold observations may request proactive rotation
- the runtime suppresses that rotation until the minimum interval expires
- hard failures bypass the interval immediately
- the suppression state is live/runtime-local, not durable installation state

Pros:

- matches the user goal of account stickiness without blocking real failover
- keeps semantics clear between soft and hard signals
- preserves flexibility for future remote-authoritative cooldown or revocation

Cons:

- requires a few new live diagnostic facts
- requires the runtime to track a lease-age-based suppression clock

This is the recommended approach.

### Approach C: Persist the minimum interval state in SQLite so all runtimes share it

Under this approach, once any runtime suppresses a proactive switch, the
suppression window would be written to shared state so other runtimes also
avoid switching.

Pros:

- produces installation-wide behavior

Cons:

- turns a runtime-local decision into shared mutable policy
- makes concurrent runtimes interfere with one another
- creates more merge risk and more chances for stale state

This approach is rejected.

## Recommended Design

### 1. Separate soft proactive pressure from hard lease failure

The runtime should model at least two classes of rotation triggers:

- `SoftProactivePressure`
  - derived from proactive rate-limit snapshots or other near-limit hints
  - means "prefer another account soon if policy allows"
- `HardFailure`
  - includes `usage_limit_reached`, unauthorized, revoked lease, missing lease,
    or other backend-authoritative lease loss
  - means "the current account should not continue as the active lease"

`accounts.min_switch_interval_secs` applies only to `SoftProactivePressure`.

This means a proactive near-limit observation must not be treated as the same
thing as a durable hard health event. In particular, the runtime should not
need to write a shared installation-wide unhealthy marker merely because a soft
threshold fired before the damping window expired.

That boundary must also be reflected in the health-reporting contract:

- threshold-only proactive pressure must not, by itself, durably persist
  `RateLimited`, `cooling_down`, or another authoritative degraded health state
- if an existing `report_rate_limits(...)` surface remains in use, it must be
  narrowed so threshold-only observations either feed a diagnostics-only/live
  pressure path or carry an explicit severity boundary that prevents a soft
  threshold from being mistaken for a hard exhaustion event
- authoritative degraded health persistence remains reserved for genuinely hard
  or backend-authoritative events, with recovery semantics continuing to follow
  the stricter existing lease-health model

### 2. Define the damping clock as lease-scoped and runtime-local

The damping interval should start when the current active lease is acquired.

Effective rule:

- `proactive_switch_allowed_at = active_lease.acquired_at + min_switch_interval`

Before `proactive_switch_allowed_at`, the runtime may note the soft pressure but
must keep the current lease active.

That noted soft pressure is live-only and not durably latched:

- a threshold crossing observed before `proactive_switch_allowed_at` may be
  remembered only as a transient live signal for status/diagnostics
- once the damping window opens, proactive rotation must still be gated by a
  current or freshly revalidated soft-pressure observation rather than by a
  stale earlier threshold crossing alone
- if pressure subsides before the damping window opens, the runtime should
  continue using the current lease without forcing a delayed proactive switch
- if pressure is still present when the next turn is prepared after the damping
  window opens, that fresh observation may trigger proactive rotation

This keeps the rule close to the user mental model:

- once a runtime instance is assigned an account, hold that account for at
  least a minimum stable interval unless a real failure occurs

The suppression state is live only:

- it belongs to the current runtime instance
- it is cleared when the runtime exits
- it is not persisted in startup-selection SQLite state
- it must not create a delayed mandatory switch solely from an old soft signal

### 3. Preserve immediate hard-failure failover

The following events bypass damping:

- `usage_limit_reached`
- unauthorized after lease-scoped auth recovery is exhausted
- lease renewal missing/revoked
- remote-backend hard revocation

When one of these occurs, the runtime should:

- mark the current lease unusable according to the existing hard-failure path
- schedule immediate future-turn failover under the existing replay rules
- expose a hard-failure switch reason rather than a damping reason

This preserves the core product promise: damping reduces churn, but does not
trap the runtime on a dead account.

### 4. Do not overload durable startup-selection state

The minimum-switch interval is not startup selection and must not be stored in:

- durable preferred account
- durable suppressed startup selection
- config migration markers
- cross-product migration state

This slice is intentionally runtime-local. It should not make future fresh
starts behave as though the runtime had explicitly changed durable pool or
account preference.

### 5. Avoid overloading `next_eligible_at`

Existing `next_eligible_at` surfaces already represent when a pool or account is
eligible again under the current lease/health model. That meaning should remain
stable.

Proactive switch damping is a different fact:

- it describes when the current runtime is next allowed to rotate away from the
  active lease for soft reasons
- it does not necessarily mean the active account or pool is globally
  unavailable

Therefore this design recommends additive live-only fields such as:

- `proactiveSwitchSuppressed`
- `proactiveSwitchAllowedAt`
- `proactiveSwitchSuppressionReason`

Exact field naming belongs to implementation planning, but the key boundary is:
do not silently reuse `next_eligible_at` for a different meaning.

### 6. Preserve stronger no-immediate-switch-back semantics after a proactive switch

When a proactive switch actually happens because the damping window has expired,
the runtime should avoid immediately reacquiring the same account in the same
selection attempt.

The existing account-pool design already treats "no immediate switch-back to the
just-replaced account" as a default safeguard. This slice must preserve that
behavior rather than weakening it to only "not in the same selector call."

This remains a local anti-flap rule, not a shared durable cooldown.

The minimal requirement is:

- when the runtime rotates away from account `A` for soft proactive pressure, it
  should not select `A` again in the same immediate reselection path
- after that proactive switch completes, the runtime should continue treating
  `A` as the just-replaced account for subsequent proactive reselection, not
  merely for one selector invocation
- the runtime may fall back to `A` only when no other eligible account exists,
  or when a later hard-failure path requires emergency fallback, or when a
  backend-authoritative recovery/newer eligibility fact materially changes the
  choice set

Cross-runtime durable cooldown for soft pressure remains deferred until a later
slice or a remote-authoritative backend contract makes it explicit.

### 7. Additive diagnostics for CLI, TUI, and doctor

Operator-facing surfaces should be able to distinguish:

- no switch because nothing requested it
- no switch because a hard failure path was not triggered
- no switch because proactive pressure existed but damping suppressed it
- switch performed because damping expired
- switch performed because a hard failure bypassed damping

This spec does not require changing existing JSON fields in place. Additive
diagnostic fields or codes are preferred.

At minimum, a doctor/status/debug surface should be able to answer:

- what is the active account
- when was its lease acquired
- what is the configured minimum switch interval
- whether proactive pressure is currently pending
- whether proactive switch is currently suppressed
- the earliest time a proactive switch may occur

### 8. Compatibility with a future remote backend

This local semantic contract should remain compatible with a future remote
backend:

- local damping still governs soft proactive rotation unless the backend emits a
  stronger authoritative hard signal
- remote-authoritative revocation or cooldown may bypass or supersede local
  heuristics
- local damping must not assume SQLite owns all lease timing forever

The important boundary is that damping is a local scheduling policy for soft
pressure, not a claim that the backend agrees the account is healthy or
unhealthy in any global sense.

## Acceptance Criteria

- The spec clearly distinguishes soft proactive pressure from hard failures.
- The spec clearly states that `accounts.min_switch_interval_secs` applies only
  to proactive automatic switching.
- The spec clearly states that threshold-only proactive pressure must not
  durably persist authoritative degraded health or cooldown state by itself.
- The spec clearly states that the damping clock is lease-scoped and
  runtime-local, not durable startup-selection state.
- The spec avoids redefining `next_eligible_at` to mean switch damping.
- The spec preserves the stronger "no immediate switch-back" safeguard across
  adjacent proactive reselection, not just within one selector invocation.
- The spec gives operator-visible additive diagnostic requirements.
- The spec remains compatible with future remote-authoritative lease signals.

## Deferred Scope

This slice intentionally does not decide:

- installation-wide durable soft cooldown semantics
- remote-backend-specific damping overrides
- a full doctor command schema
- a full fake-remote contract test harness
- multi-client app-server pooled runtime semantics
