# Account Pool Quota-Aware Selection Design

This document defines how pooled account selection should incorporate live
quota knowledge so account choice is stable, explainable, and correct in the
presence of multiple quota windows, shared product homes, and provider-side
early resets.

It intentionally focuses on account-pool selection and recovery semantics. It
does not redesign the broader remote backend contract, introduce a background
quota-recovery worker, or add a large account-management UI.

## Post-RuntimeLeaseAuthority Baseline

This spec was originally written before the runtime lease authority work landed.
As of the main branch containing
`runtime-lease-authority-for-subagents-implementation`, pooled runtime selection
and live quota reporting no longer belong to a per-session failover path.

The current integration boundary is:

- `RuntimeLeaseAuthority` owns pooled lease generations, request admission,
  hard-failure rotation, proactive soft rotation, and collaboration-tree scoped
  invalidation.
- `RuntimeLeaseHost` shares that authority across the top-level runtime and its
  child subagents.
- admitted provider requests carry a `LeaseSnapshot` with the resolved
  `selection_family`; live rate-limit and usage-limit reports must write quota
  state against that snapshot family instead of recomputing the family later.
- the original quota-aware plan's `codex-core` runtime integration task is
  superseded by the runtime authority implementation; remaining quota-aware
  work should extend observability and UI consumers on top of that boundary.

## Summary

The recommended direction is:

- treat backend live quota observations as the only quota truth
- stop overloading coarse account health with quota-exhaustion semantics
- persist per-account quota knowledge separately from lease ownership and auth
  state
- introduce one shared selection policy engine used by startup selection,
  proactive rotation, and hard-failure failover
- veto accounts that are currently predicted blocked by exhausted quota windows
  before ordinary ranking begins
- rank otherwise-eligible accounts primarily by short-window safety so the
  runtime minimizes avoidable account switching
- treat long-window exhaustion as a veto condition, not the primary ranking key
- allow low-frequency reprobe of predicted-blocked accounts so local state does
  not lag provider-side early reset behavior
- expose richer operator-visible selection reasons and quota facts instead of
  collapsing everything into `RateLimited`

## Goals

- Minimize avoidable account switching during steady-state pooled execution.
- Avoid selecting accounts that are already known to be exhausted in a longer
  quota window.
- Recover quickly when the provider resets quota earlier than the last observed
  reset timestamp predicted.
- Share one coherent policy between startup selection and live runtime
  rotation/failover.
- Keep account-pool decisions explainable in CLI, TUI, and app-server
  observability surfaces.
- Build the domain model cleanly enough that future remote backends can remain
  authoritative for quota truth without forcing another state-model rewrite.

## Non-Goals

- Do not add a continuous background probe worker in this slice.
- Do not introduce a large configuration matrix for quota weights or custom
  ranking formulas.
- Do not redesign manual `accounts switch` behavior.
- Do not require a remote backend implementation before this design is useful.
- Do not force all selection logic into SQL ordering expressions.

## Constraints

- Before the runtime authority and quota-state slices, runtime proactive
  switching only looked at `RateLimitSnapshot.primary` usage percentage.
- Before this design, durable health state was too coarse for quota-aware
  selection:
  `Healthy`, `RateLimited`, and `Unauthorized`.
- Before the runtime authority work, lease acquisition chose the first eligible
  account by pool position. New runtime changes should preserve the authority
  boundary rather than reintroducing a separate per-session manager path.
- The provider protocol already exposes `limit_id`, `primary`, and `secondary`
  windows; remaining app-server and UI work must expose the durable multi-family
  quota model instead of only the legacy singular `quota` projection.
- Provider behavior is not perfectly aligned with predicted reset times;
  provider-side early reset must be treated as real and must be discoverable.
- Multiple runtime instances may share one product home, so any retry/probe
  throttling should coordinate through shared state rather than in-memory
  heuristics alone.

## Problem Statement

The original account-pool implementation had three mismatches:

1. quota observations were session-local and not modeled per account
2. quota exhaustion was collapsed into a coarse durable `RateLimited` health
   state
3. selector logic did not use quota knowledge when choosing the next account

That left several correctness and UX gaps that this design addresses and that
remaining observability/UI work must keep explainable:

- proactive switching only saw the active account's primary window and ignored
  longer-window risk when choosing the next account
- a weekly-exhausted account could only be represented as generically
  `RateLimited`, with no durable notion of which window is exhausted
- local predicted cooldown could become stale when the provider resets early
- startup selection and runtime failover did not share a single explainable
  decision model

The design therefore needs a richer domain boundary:

- auth health is not quota health
- quota exhaustion is not the same thing as authorization failure
- predicted cooldown is not durable truth
- selection should be derived from multiple fact sources, not from one coarse
  enum

## Approaches Considered

### Approach A: Keep `RateLimited` as the main durable truth and add smarter ordering around it

Under this approach, quota exhaustion would continue to be represented mainly as
coarse account health, with a small amount of extra ordering logic layered on
top.

Pros:

- smallest short-term patch
- least schema change

Cons:

- keeps quota semantics mixed into the wrong domain
- cannot cleanly distinguish primary exhausted, secondary exhausted, and both
- makes early-reset recovery awkward because a coarse health enum becomes a
  pseudo-quota cache
- does not provide a clean policy engine boundary

This approach is rejected.

### Approach B: Add a minimal blocked-until cache and keep the rest of the model mostly unchanged

Under this approach, the system would add a small durable quota cache with
`blocked_until`-style fields while leaving the overall selector and
observability model largely unchanged.

Pros:

- solves immediate practical failures more directly than Approach A
- smaller than a full domain cleanup

Cons:

- still leaves selection logic too tied to legacy surfaces
- risks turning `blocked_until` into de facto truth unless reprobe semantics are
  modeled carefully
- keeps startup, proactive switch, and failover behavior less unified than they
  should be

This approach is acceptable but not preferred.

### Approach C: Separate lease, auth, quota knowledge, and selection policy into explicit layers

Under this approach, account-pool selection becomes a fact-driven decision over
multiple specialized state layers:

- registry and membership
- lease ownership
- auth state
- quota knowledge
- selection policy

Pros:

- correct domain boundaries
- clean handling of multi-window quota semantics
- shared decision model across startup and runtime paths
- natural place to encode reprobe as part of selection rather than as a patch

Cons:

- larger initial schema and policy refactor
- requires richer observability payloads

This is the recommended approach.

## Recommended Design

### 1. Separate the domain into five layers

The account-pool system should model five distinct layers:

1. `Registry`
   - static account facts such as account id, account kind, enabled flag, pool
     membership, and pool position
2. `LeaseOwnership`
   - active lease holder, lease epoch, acquisition time, and expiry
3. `AuthState`
   - authorization and credential validity facts
   - quota exhaustion must not live here
4. `QuotaKnowledge`
   - latest known quota state per account and per limit family
5. `SelectionPolicy`
   - a pure policy engine that reads facts from the first four layers and emits
     a selection plan

This means durable quota exhaustion no longer depends on reusing
`AccountHealthState::RateLimited` as the system's main selection signal.

### 2. Introduce durable per-account quota knowledge

The state layer should add a durable `account_quota_state` model keyed by:

- `account_id`
- `limit_id`

The record should contain at least:

- `primary_used_percent`
- `primary_resets_at`
- `secondary_used_percent`
- `secondary_resets_at`
- `observed_at`
- `exhausted_windows`
  - `none`
  - `primary`
  - `secondary`
  - `both`
  - `unknown`
- `predicted_blocked_until`
- `next_probe_after`
- `probe_backoff_level`
- `last_probe_result`
  - `success`
  - `still_blocked`
  - `ambiguous`

Semantics:

- `observed_at` is when the live backend observation was captured
- `predicted_blocked_until` is a pessimistic prediction derived from the last
  known exhausted window reset timestamp
- `next_probe_after` is a throttle boundary for reprobe attempts and must not
  be treated as quota truth
- `exhausted_windows` records what the latest known quota state actually says
- `unknown` is necessary because some `usage_limit_reached` paths may not let us
  reliably infer which window was exhausted
- `probe_backoff_level` starts at `0`
- `last_probe_result` starts as `null` until the first completed reprobe

### 3. Make quota truth live-backend authoritative

Quota truth should follow this rule:

- the newest backend live observation wins
- durable quota state is a shared knowledge cache, not the source of truth

Therefore:

- a new live observation may immediately clear a predicted block
- a successful reprobe may invalidate `predicted_blocked_until` before the
  predicted reset time arrives
- stale durable quota state must never permanently block an account that the
  backend now reports as recovered

### 4. Introduce one shared selection policy engine

The account-pool crate should own a shared policy engine that is used by:

- startup automatic selection
- proactive runtime rotation
- hard-failure failover
- explicit reprobe recovery paths

The engine should accept a `SelectionIntent` such as:

- `Startup`
- `SoftRotation`
- `HardFailover`
- `ProbeRecovery`

`SelectionIntent` must remain orthogonal to switch damping. The runtime-local
minimum-hold behavior for proactive switching continues to live in the separate
`2026-04-16-account-pool-switch-damping-design.md` design. This quota-aware
selector should consume the intent it is given and must not silently duplicate
or reinterpret `accounts.min_switch_interval_secs` inside quota ranking.

The engine input should include:

- selection context facts:
  - `current_account_id`
  - `just_replaced_account_id`
  - `selection_family`
- registry facts
- lease facts
- auth facts
- caller-resolved quota facts for `selection_family`
- optional fallback quota facts for the default `codex` family
- current time
- proactive threshold

The engine output should be a structured `SelectionPlan` containing:

- `eligible_candidates`
- `probe_candidate`
- `rejected_candidates` with reasons
- `decision_reason`
- a terminal action:
  - `Select(account_id)`
  - `Probe(account_id)`
  - `StayOnCurrent`
  - `NoCandidate`

This shared output is also the right source for diagnostics and operator-facing
selection explanations.

If acquiring an ordinary ranked candidate fails because lease state changed
under contention, the runtime reruns selection immediately rather than walking
the remainder of the stale candidate list from the old plan.

`just_replaced_account_id` is a first-class input owned by the shared selector,
not an implicit caller-side prefilter. `core` provides the current runtime-local
anti-bounce fact, and the policy engine itself decides how that fact affects
`SoftRotation` and `HardFailover`.

### 5. Define per-intent admissibility explicitly

The selector must not leave admissibility to caller convention. In the first
slice, each `SelectionIntent` has a fixed candidate contract.

`Startup`

- there is no current leased account
- all accounts that pass hard filtering are admissible to ordinary ranking
- blocked accounts may become reprobe candidates only after ordinary candidates
  are exhausted

`SoftRotation`

- the current active account is never an ordinary candidate
- the current active account is never a reprobe candidate
- the most recently proactively replaced account from the switch-damping design
  is excluded from ordinary ranking and reprobe during the same soft-rotation
  attempt in order to avoid immediate bounce-back
- if no admissible non-blocked candidate exists and no probe succeeds, the
  selector returns `StayOnCurrent`; soft rotation does not release and reacquire
  the current account merely to pick it again

`HardFailover`

- the current failed account is never admissible
- the most recently proactively replaced account is admissible again; hard
  failure is allowed to override the soft no-bounce rule
- blocked accounts may become reprobe candidates only after ordinary candidates
  are exhausted

`ProbeRecovery`

- no ordinary ranking is performed
- the selector operates only on the single reserved probe target handed to it
- success writes refreshed quota knowledge and then reruns the original caller
  intent from scratch rather than directly selecting the probed account by
  special case

This contract is intentionally aligned with
`2026-04-16-account-pool-switch-damping-design.md`: quota-aware selection must
respect the runtime-local "do not immediately switch back" rule during
`SoftRotation`, while `HardFailover` may bypass that soft anti-flap constraint.

### 6. Use a four-stage selection pipeline

Selection should proceed in four stages.

#### Stage 1: Hard filter

Remove accounts that are definitely unavailable:

- disabled
- unauthorized
- currently leased by another holder
- otherwise failing required hard constraints

Then apply the intent-specific exclusions from the previous section before
ordinary ranking begins.

#### Stage 2: Soft-block classification

For each remaining account, classify quota state into one of:

- `NotBlocked`
- `PredictedBlocked`
- `ProbeEligibleBlocked`

An account is `PredictedBlocked` when:

- its latest known quota state says a required window is exhausted
- the prediction is still recent enough to trust
- `next_probe_after` has not yet elapsed

An account is `ProbeEligibleBlocked` when:

- it is currently predicted blocked
- `now >= next_probe_after`
- and the current selection attempt has exhausted ordinary `NotBlocked`
  candidates, or the intent is `ProbeRecovery`

In the first slice, "stale enough to reprobe" is defined only through
`next_probe_after`. Planners should not invent an additional implicit staleness
timeout in the selector. Writers compute `next_probe_after` from the last
`observed_at`, the exhausted-window shape, and the probe backoff level; readers
simply compare `now` against that stored boundary.

`predicted_blocked_until` expiring by itself does not move an account back to
`NotBlocked`. If `exhausted_windows != none`, the account remains blocked until
a fresh live observation or successful reprobe clears or overwrites that
exhausted state. `predicted_blocked_until` is a recovery prediction, not an
automatic state-transition timer.

Missing or partial quota data must default deterministically:

- if there is no quota row for the relevant limit family and no fallback row
  for the default family, classify the account as `NotBlocked` with low
  confidence
- if a present row marks `exhausted_windows != none`, classify using that
  exhausted state even if only one window snapshot is populated
- if a row has partial window data but no exhausted signal, classify the
  account as `NotBlocked` with low confidence and rank it below otherwise
  comparable candidates that have fresher complete quota data
- the selector must never hard-block an account solely because quota knowledge
  is absent or partial

Weekly or other long-window exhaustion must act as a veto before ordinary
ranking begins. That is the safeguard that prevents a 5-hour reset alone from
making a weekly-exhausted account look freely selectable.

#### Stage 3: Rank ordinary candidates

Only `NotBlocked` candidates enter ordinary ranking.

Ranking rules:

1. prefer accounts whose `primary.used_percent` is below
   `proactive_switch_threshold_percent`
2. within that set, sort by descending primary safety margin
3. break ties by descending secondary safety margin
4. use reset proximity as a later tie-breaker, not the primary key
5. prefer candidates backed by complete, fresher quota rows over low-confidence
   candidates that were admitted through missing or partial quota fallback
6. use pool position and account id as stable final tie-breakers

This makes short-window stability the primary optimization target while still
ensuring long-window exhaustion acts as a veto condition earlier in the
pipeline.

#### Stage 4: Reprobe fallback

If no ordinary candidate remains, the selector may choose one blocked account
for reprobe.

Reprobe is an execution contract, not only a classification:

1. the selector nominates at most one `Probe(account_id)` action
2. the state layer attempts an atomic probe reservation by advancing
   `next_probe_after` from a value `<= now` to a short reservation horizon
3. if that compare-and-set fails, the runtime reruns selection and does not
   probe opportunistically outside shared-state coordination
4. if reservation succeeds, the runtime acquires a short-lived verification
   lease for that account
5. while holding that verification lease, the backend performs a lightweight
   lease-scoped quota refresh request that does not consume the next user turn
6. the probe result writes fresh quota knowledge:
   - success clears exhaustion for the relevant family and updates `observed_at`
   - still blocked refreshes exhausted windows, `predicted_blocked_until`, and
     `next_probe_after`
   - ambiguous keeps the account blocked conservatively and raises backoff
7. after the probe lease is released, the runtime reruns the original
   non-probe selection intent from scratch using the refreshed quota rows

Verification-lease mechanics are fixed in the first slice:

- `SoftRotation` keeps the current active lease held while probing
- probe execution does not use release-and-reacquire semantics
- the verification lease uses a dedicated probe holder identity derived from the
  runtime's main holder identity
- the lease model therefore permits one main active lease plus at most one
  auxiliary verification lease per runtime instance
- the verification lease never becomes the runtime's selected active lease by
  promotion; after probe completion it is always released, and any real switch
  reacquires the chosen account under the runtime's primary holder identity

Probe reservation is therefore coordinated through shared quota state, while
probe execution uses an ordinary exclusive lease so two runtimes do not probe
the same account concurrently with live backend traffic.

Probe capability is explicit at the core/backend boundary:

- the backend interface must expose a quota-refresh capability that can perform
  a lease-scoped quota observation without consuming the next user turn
- if a backend does not support that capability, `ProbeRecovery` is disabled for
  that backend in the first slice
- when probe capability is unavailable, selection may still use durable quota
  knowledge, but blocked accounts do not become executable reprobe candidates

If probe reservation succeeds but verification-lease acquisition then fails
because another holder wins the exclusive lease first:

- the reservation is not rolled back
- it is left to expire at the short reservation horizon already written into
  `next_probe_after`
- the runtime immediately reruns selection without probing that account

This keeps reservation and exclusive lease acquisition simple in the first
slice while still avoiding unbounded repeated probe races.

Reprobe preference:

- older or more stale primary-only blocked accounts first
- then primary-only blocked accounts closer to predicted recovery
- then stale secondary blocked accounts
- fresh secondary blocked accounts last

Each selection attempt should have a very small reprobe budget. The default
budget should be one reprobe candidate per attempt.

### 7. Make 5-hour safety the primary ranking objective

The system should explicitly optimize for minimizing avoidable account
switching. That means the selector should prefer the account least likely to
trigger another proactive switch soon.

This is why:

- primary short-window safety is the main ranking key
- longer-window safety is a veto and secondary ranking signal
- a single weighted global score is not recommended for the first design

Using layered comparisons is easier to reason about and easier to expose in
operator-facing explanations.

### 8. Distinguish prediction from probe throttle

The design must keep these concepts separate:

- `predicted_blocked_until`
- `next_probe_after`

They are not interchangeable.

`predicted_blocked_until` means:

- based on the last observed reset timestamp, this account is likely still
  blocked until this time

`next_probe_after` means:

- even if we want to validate early recovery, do not spend another probe before
  this time

This distinction is what makes provider-side early reset compatible with stable
runtime behavior.

### 9. Scope reprobe differently for startup and runtime

Startup behavior:

- if there is at least one ordinary candidate, startup should not reprobe
- if there are no ordinary candidates, startup may choose one reprobe candidate
- each startup attempt should probe at most one blocked account

Runtime behavior:

- no background quota probing while the current account is healthy
- reprobe is only considered when the runtime actually needs another account
- each failover or proactive-switch attempt should probe at most one blocked
  candidate before deciding whether the pool currently has no usable account

This keeps reprobe a recovery mechanism rather than a constant source of churn.

### 10. Resolve first-slice multi-family handling now

The first slice must not leave family resolution open-ended.

The selector consumes exactly two quota views per candidate:

- `selection_family`
- optional `codex` fallback family

The selector does not ingest or rank across an arbitrary set of families in the
first slice.

Caller responsibility:

- `Startup` always resolves `selection_family = codex`
- `SoftRotation` and `HardFailover` resolve `selection_family` from the active
  runtime limit family when known
- if the active runtime family is unknown or a `usage_limit_reached` path is
  ambiguous, the caller passes `selection_family = codex`

Selector responsibility:

- use the row for `selection_family` when present
- fall back to the `codex` row only when the selected family row is absent
- ignore all other observed families in the first slice

Write-side family resolution follows the same contract:

- live observations and hard errors write the row for the caller-resolved active
  family when known
- if a `usage_limit_reached` path is ambiguous and there is no known active
  family, the writer updates the `codex` row
- the first slice does not introduce a synthetic unknown-family quota row;
  ambiguity is represented inside the chosen row via
  `exhausted_windows = unknown`

This keeps the selector API narrow enough for planning while still supporting
the active-family-plus-`codex` contract already implied by current runtime
surfaces.

### 11. Integrate through shared policy helpers, not SQL-only ordering

The state layer may still perform a coarse candidate fetch, but the full
selection policy should run in Rust, not in a single SQL `ORDER BY`.

Reasons:

- the policy depends on staleness and throttling semantics
- it needs structured veto reasoning
- it needs a reprobe phase that is not naturally represented as one database
  ordering expression
- pool sizes are expected to remain small enough that in-memory policy
  evaluation is acceptable

The state layer should therefore provide:

- coarse candidate enumeration
- durable quota-state lookup
- durable quota-state writes
- coordinated probe-throttle updates

The account-pool crate should provide:

- the policy engine
- ranking helpers
- soft-block evaluation
- reprobe candidate selection

### 12. Expand observability to reflect real quota knowledge

Observability should stop collapsing quota state into one opaque remaining
percentage.

Account-pool account surfaces should eventually expose a collection of quota
families rather than one singular quota projection:

- `quotas: Vec<AccountPoolQuotaFamily>`

Where each `AccountPoolQuotaFamily` contains:

- `limit_id`
- `primary`
  - used percent
  - resets at
- `secondary`
  - used percent
  - resets at
- `exhausted_windows`
- `predicted_blocked_until`
- `next_probe_after`
- `observed_at`

First-slice surfaced `quotas` rows are ordered by `limit_id` ascending so UI,
tests, and snapshots remain deterministic.

First-slice observability boundary:

- account-row quota facts are expanded as typed `quotas` collection fields in
  scope
- app-server compatibility is additive in the first slice: the existing
  singular `quota` field remains, and the new `quotas` field is added beside it
- the singular compatibility `quota` field is projected only from the `codex`
  family row; if no `codex` row exists, `quota` is `null`
- new CLI/TUI and app-server consumers should migrate to `quotas`; legacy
  consumers may continue reading `quota` during the transition
- diagnostics and selection explanations may still project one family-specific
  explanation when a runtime supplies `selection_family`, but the underlying
  account API shape does not collapse multi-family state into one singular quota
- no new top-level account-pool event enums are introduced in the first slice
- existing event families stay wire-compatible
- richer probe and multi-window details are carried in structured
  `details_json`

This keeps the first implementation planning scope bounded across
`codex-state`, app-server protocol, CLI, and TUI while still giving operators
enough information to understand selection decisions.

### 13. Keep coarse auth health, but remove quota from its semantic center

`Unauthorized` should remain a durable auth-state concept.

Quota exhaustion should not remain the semantic center of coarse account
health. Existing compatibility surfaces may still expose a broad cooling-down or
rate-limited status where needed, but selector truth must come from
`QuotaKnowledge`, not from a single coarse durable health enum.

First-slice coexistence rule:

- `account_quota_state` is the only authoritative source for quota blocking and
  selector eligibility
- startup-selection and lease-acquisition paths must stop consulting legacy
  `RateLimited` state or `healthy` as quota veto signals
- in the first slice, legacy `healthy` continues only as an auth-health
  compatibility projection:
  - `Unauthorized` may still drive unhealthy auth state
  - quota exhaustion must not
- `AccountHealthState::RateLimited` is removed from selector-facing truth
- durable auth truth continues to use `AccountHealthState::Unauthorized`
- legacy cooling-down and coarse rate-limited read surfaces are derived from
  quota rows at read time rather than treated as an independent durable quota
  signal
- therefore, first-slice writers stop treating `RateLimited` as a standalone
  source of truth, even if the legacy enum and compatibility projections remain
  in storage and API code during migration
- upgrade handling for preexisting persisted `RateLimited` rows is explicit:
  after migration they are ignored by selector paths and are normalized into
  compatibility projections derived from quota rows rather than preserved as
  blocking truth

This names one source of truth for planning and prevents `RateLimited` from
competing with quota rows during the migration.

## Data Flow

### 1. Live quota observation

When the runtime receives a new rate-limit snapshot:

- resolve the active account
- resolve the relevant `limit_id`
- update `account_quota_state`
- derive `exhausted_windows`
- refresh or clear predicted blocking
- append quota-observation events

### 2. Hard usage-limit error

When a turn ends in `usage_limit_reached`:

- update the caller-resolved quota-family row using the best available error and
  header data; if the family is ambiguous and there is no known active family,
  update `codex` with `exhausted_windows = unknown`
- mark the current lease unusable for the current turn path
- trigger hard-failure selection intent for the next attempt

### 3. Selection attempt

When startup or runtime selection needs an account:

- load coarse candidates
- attach lease, auth, and caller-resolved family-scoped quota facts
- run the shared policy engine
- try ordered ordinary candidates first
- if no ordinary candidate exists, try the single reprobe candidate if present
- update quota knowledge based on reprobe result

## Module Boundaries

### `codex-rs/state`

Owns:

- durable `account_quota_state`
- quota observation persistence
- coordinated `next_probe_after` updates
- atomic probe reservation updates
- coarse candidate fetches with attached quota state
- read-time compatibility projection for legacy coarse cooldown and
  rate-limited surfaces

### `codex-rs/account-pool`

Owns:

- quota domain types
- exhausted-window derivation
- blocked and reprobe classification
- candidate comparison rules
- shared selection policy engine
- per-intent admissibility rules

### `codex-rs/core`

Owns:

- feeding live quota observations into state
- resolving the per-attempt `selection_family`
- invoking the shared policy engine for runtime rotation and failover
- adapting selection outcomes into existing turn/failover flows
- invoking the backend quota-refresh probe while holding a verification lease

### `app-server` and CLI/TUI surfaces

Own:

- presentation of richer quota facts
- selection explanations derived from the shared policy plan
- event and diagnostics display

## Testing Strategy

The first implementation should include at least:

### Policy tests

- weekly exhausted vetoes an account before primary ranking
- primary-safe account outranks another candidate with worse short-window margin
- stale blocked state becomes reprobe-eligible
- missing or partial quota snapshots degrade ranking confidence but do not
  always hard-block the account

### State tests

- quota observations persist per account and per limit family
- `predicted_blocked_until` and `next_probe_after` evolve independently
- successful reprobe clears blocked state
- failed reprobe refreshes block prediction and probe throttle

### Integration tests

- startup selection chooses the expected account under mixed primary and
  secondary pressure
- hard failover uses the shared policy engine rather than legacy position-only
  ordering
- provider-side early reset is discoverable through reprobe
- diagnostics and account list surfaces expose the new quota facts correctly

## Open Questions

- Whether a later slice should extend the selector from
  `selection_family + codex fallback` to true multi-family joint reasoning.
- Whether a later slice should promote probe outcomes from `details_json` into
  explicit top-level event enums.
- Whether a later slice should introduce an optional background recovery worker
  for very large pools, or whether on-demand reprobe remains sufficient.

## Recommendation

Adopt Approach C.

The design should solve this problem by making quota knowledge a first-class
layer in account-pool state and making selection a fact-driven policy engine,
not by adding more special cases around coarse `RateLimited` state or pool
position ordering.

That is the cleanest way to satisfy the real product requirements:

- minimize avoidable account switching
- respect long-window exhaustion before ranking
- recover from provider-side early reset
- keep startup and runtime behavior aligned
- make the system explainable instead of heuristic and opaque
