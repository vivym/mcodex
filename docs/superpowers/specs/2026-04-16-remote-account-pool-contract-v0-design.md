# Remote Account Pool Contract V0 Design

This document defines a merge-friendly contract for a future remote
account-pool backend used by a company-managed shared account catalog.

It is intentionally scoped to ownership boundaries, the minimum execution-plane
contract, the minimum operator-visible control/read contract, and local versus
remote state responsibilities. It does not implement a production remote
service, and it does not redesign local account-pool startup selection into a
remote-only system.

It should be read as a forward-looking contract addendum to:

- `2026-04-10-multi-account-pool-design.md`
- `2026-04-13-pooled-account-registration-design.md`
- `2026-04-16-account-pool-selection-state-policy-design.md`

## Summary

The recommended direction is:

- treat the remote backend as authoritative for execution-plane lease state
- keep installation-local startup selection, preferred account, and suppression
  in the local product home
- avoid persisting remote private credential material in the local home while
  still allowing non-secret stable control-plane identifiers needed for local
  selection and diagnostics
- define a narrow execution-plane contract first, then a minimal read-oriented
  control-plane contract
- keep remote-specific behavior behind backend-neutral account-pool seams
  instead of scattering it through `codex-core`, app-server, and TUI
- require a fake remote backend and contract tests before any production remote
  implementation ships

This keeps the path to company-managed pooled access open without forcing a
rewrite of the existing local backend or creating a fork-only auth store.

## Goals

- Preserve a clean path to a company-managed remote account pool.
- Prevent future remote support from forcing a broad rewrite of request-path
  auth, startup selection, or TUI logic.
- Keep local product homes safe to use alongside upstream and alongside
  independent local pooled installs.
- Clearly separate what the remote backend owns from what remains local policy
  or local installation state.
- Keep future remote implementation testable through backend contract tests
  instead of end-to-end-only validation.

## Non-Goals

- Do not implement a production remote backend in this slice.
- Do not build a remote control-plane UI or enterprise admin portal.
- Do not require `accounts add api-key` or local self-service registration in
  the first remote contract slice.
- Do not move startup selection entirely out of local state.
- Do not mirror the local SQLite lease schema into a fake "remote mode" just to
  minimize code changes.

## Constraints

- The long-term primary model is a remote/shared account pool used for company
  procurement and centralized account ownership.
- Existing local pooled behavior already depends on:
  - installation-local startup selection state
  - pooled config policy
  - lease-scoped auth sessions
- Mergeability with upstream matters, so the remote design should prefer narrow
  adapters and additive types over pervasive fork-only rewrites.
- Local product homes must not become caches of remote private credential state.
- Future remote semantics must coexist with local backend support rather than
  replacing it outright.

## Problem Statement

The current codebase already points toward backend-neutral account-pool seams,
but much of the concrete behavior is still shaped around local ownership:

- local SQLite is authoritative for lease and health state
- the local backend stores backend-private auth namespaces under the product
  home
- runtime diagnostics assume local lease state is readable from the local home

That is reasonable for the local backend, but it becomes dangerous if remote
support is added without a prior contract:

- local and remote authority boundaries will blur
- request-path code may accidentally depend on local persistence details that do
  not exist for a remote pool
- future company-managed catalogs may be forced into local self-registration
  flows that do not match the product goal

The missing piece is not another local implementation detail. It is a clear
contract for what a remote backend must provide and what the local runtime must
not assume.

## Approaches Considered

### Approach A: Treat remote as a thin variation of the local backend

Under this approach, the remote backend would try to reuse the current local
SQLite/state model as much as possible and merely replace where credentials come
from.

Pros:

- superficially minimizes interface churn
- may speed up a first prototype

Cons:

- incorrectly treats local persistence as the natural source of truth for remote
  lease state
- encourages leaking remote-private semantics into the local product home
- makes it harder to explain ownership boundaries later

This approach is rejected.

### Approach B: Remote-authoritative execution plane, local-authoritative startup intent

Under this approach:

- remote owns leases, revocation, expiry, and remote cooldown hints
- local owns product-home startup selection, preferred account, suppression, and
  operator policy config
- request execution consumes lease-scoped auth sessions regardless of backend
- remote-specific types stay behind a narrow account-pool seam

Pros:

- clean ownership model
- consistent with the long-term company-managed account-pool goal
- minimizes future refactor pressure on auth and request execution

Cons:

- requires clearer boundary definitions now
- may require evolving current backend traits rather than reusing them exactly

This is the recommended approach.

### Approach C: Integrate remote directly in core/app-server without a stable backend contract

Under this approach, remote support would be wired directly into high-churn core
runtime and app-server paths, leaving backend abstraction secondary.

Pros:

- fast for a one-off prototype

Cons:

- highest merge risk
- remote semantics would leak across unrelated crates
- local and remote behavior would become harder to reason about and test

This approach is rejected.

## Recommended Design

### 1. Keep startup selection and operator intent local

The following facts remain installation-local and live in the local product
home:

- configured `accounts.default_pool`
- configured pool policy such as:
  - `allow_context_reuse`
  - `account_kinds`
  - `min_switch_interval_secs`
  - allocation mode
- durable preferred account selection
- durable suppression
- process-level pool override

These are local operator/runtime facts. They should not be outsourced to the
remote backend merely because leases become remote-owned.

This matches the existing selection-state design: product-home startup intent is
local, while account/lease execution may come from either backend.

In remote mode, that local startup intent must still point at a stable account
identity. Therefore the local home may persist a non-secret backend-exposed
control-plane identifier for a remote account, or a local mirrored `account_id`
bound to that remote identifier, so durable preferred-account state and
diagnostics have something stable to reference across restarts.

For local pooled state and selectors, this slice recommends one canonical local
namespace:

- durable preferred-account state should point at the local mirrored
  `account_id`
- exclusion lists such as anti-flap `exclude_account_ids` should also use the
  local mirrored `account_id`
- backend-owned remote references remain stable correlated metadata associated
  with that local `account_id`, not a second selector namespace exposed
  everywhere in runtime code

This keeps remote mode compatible with the existing selection-state model, which
already stores preferred-account facts in local state.

That stable identifier must not be:

- an active lease id
- a bearer token
- a refresh token
- an ephemeral per-session transport identity

### 2. Make the remote backend authoritative for execution-plane lease state

The remote backend is authoritative for:

- lease acquisition success or failure
- lease ownership
- lease expiry
- lease renewal or revocation
- backend-authoritative cooldown or next-eligible hints
- lease-scoped auth material usable for request execution

The local product home must not pretend it owns those facts simply because it
can cache a diagnostic snapshot.

### 3. Do not persist remote private credential material locally

The local product home must not persist:

- remote refresh tokens
- remote provider account credentials
- secret-bearing credential locators or other backend-private handles whose
  possession would imply local ownership of remote credential retrieval

The local runtime may hold lease-scoped auth material in memory for the active
lease lifetime, but long-lived secret storage remains owned by the remote
backend or service.

This keeps the remote contract compatible with the company's centralized
procurement and account-ownership model.

However, the local home may persist non-secret control-plane metadata required
for normal pooled behavior, such as:

- a stable remote account/catalog reference exposed by the backend
- a local mirrored `account_id` mapped to that remote reference
- durable preferred-account state that targets that stable id
- additive diagnostics and cached pool summaries that do not contain secret
  material

This preserves compatibility with the existing pooled registration/control-plane
design, which expects stable backend-owned opaque identifiers for correlation,
dedupe, and operator-visible state, while still keeping real secret material
remote-owned.

### 4. Evolve the execution-plane contract around requests, not local storage

The remote execution-plane contract should be defined in terms of what request
execution actually needs.

The minimum contract shape is:

- acquire a lease for a pool and runtime instance
- optionally honor local startup intent such as preferred account when the
  backend supports it
- renew or validate an active lease
- release a lease
- report hard health signals
- return a lease-scoped auth session or equivalent lease-bound auth capability

The request surface should be described as a structured request, not just a
bare pool id, so future remote implementations can evolve without forcing new
ad hoc parameters everywhere.

At minimum, a future acquire request should be able to carry:

- `pool_id`
- `holder_instance_id`
- optional local mirrored preferred `account_id`
- optional local mirrored `exclude_account_ids` for anti-flap reselection

Exact Rust trait shape belongs to implementation planning, but the contract
must be more remote-ready than "just pass pool id and assume local state owns
the rest."

The runtime/backend adapter is responsible for translating those local mirrored
ids to the backend's stable remote control-plane references before issuing the
remote lease request.

If the backend no longer recognizes the mapped remote reference, or that remote
account no longer exists in the catalog, the runtime should degrade through the
existing preferred-account-missing/ineligible semantics rather than inventing a
remote-only selection state model.

### 5. Keep local SQLite out of the remote source-of-truth path

Remote pooled mode must not reuse local shared SQLite lease tables as the
authoritative lease store.

Those tables are local-backend state.

For remote pooled mode:

- local SQLite may continue to store local startup selection and other
  installation-local settings
- live runtime snapshots may cache additive remote diagnostic data for display
  or crash reporting if needed
- local SQLite must not masquerade as the authoritative remote lease registry

This avoids stale local contention rules being mistaken for remote truth.

### 6. Define a minimal read-oriented control-plane contract first

The first remote contract slice should not require full write control-plane
flows.

The minimum remote-readable control plane is:

- list available pools for the current user or installation
- inspect one pool summary/status
- inspect the active lease/account summary for the current runtime

Registration, import, or account mutation flows should remain deferred until
there is a real product need. This is consistent with the company-managed
catalog goal, where the operator often needs to consume centrally provisioned
accounts rather than self-register them locally.

### 7. Define failure semantics explicitly

The remote contract must define what happens when the backend is unreachable.

Recommended baseline:

- no active lease and remote unavailable: fail closed
- active lease exists and remote becomes temporarily unavailable: continue only
  while the current lease-scoped auth remains locally usable, and in no case
  beyond lease expiry or explicit local invalidation
- lease expiry reached and remote still unavailable: fail closed
- remote explicit revocation: immediately invalidate the lease
- if the lease-scoped auth session needs remote-mediated refresh or revalidation
  before lease expiry and the remote backend is unavailable, treat the lease as
  unusable at that point and fail closed rather than pretending the remaining
  lease lifetime is still usable

There should be no silent fallback to legacy single-account auth or to the
local pooled backend merely because the remote service is unavailable.

### 8. Local policy may narrow remote behavior, but not override remote authority

Local policy such as:

- `min_switch_interval_secs`
- `allow_context_reuse`
- local startup preferred account

may influence when or how the local runtime asks for another lease.

However, local policy must not overrule remote-authoritative facts such as:

- lease revocation
- remote expiry
- remote-provided next-eligible or cooldown hints

In other words:

- local policy may be more conservative
- remote authority remains final for lease validity

### 9. Require a fake remote backend and contract tests

Before any production remote backend ships, the codebase should have:

- a fake remote backend implementation or harness
- contract tests that exercise the same execution-plane invariants every remote
  implementation must satisfy

Those tests should cover at least:

- lease acquisition and renewal
- revocation
- cooldown or next-eligible hints
- preferred-account requests when supported
- failure when remote is unavailable
- no secret persistence in the local product home

This keeps future remote work from turning into a one-off integration hidden in
core runtime code.

## Acceptance Criteria

- The spec clearly separates local-owned startup intent from remote-owned lease
  execution state.
- The spec clearly states that remote private credential material is not
  persisted in the local home.
- The spec clearly allows non-secret stable control-plane identifiers needed for
  preferred-account state, dedupe, and diagnostics.
- The spec defines a minimum execution-plane contract centered on lease-scoped
  auth and lease authority.
- The spec defines a minimal read-oriented control plane without requiring full
  write/self-registration support.
- The spec defines what stable identity a local preferred-account reference is
  allowed to target in remote mode.
- The spec explicitly defines baseline remote-unavailable behavior as fail
  closed, including the case where token usability ends before lease expiry.
- The spec requires a fake backend and contract tests before production remote
  rollout.

## Deferred Scope

This slice intentionally does not decide:

- remote self-service registration or `accounts add` semantics
- remote admin portal behavior
- public network/API schema details
- multi-client app-server pooled semantics
- remote pool catalog caching policy beyond additive diagnostics
- full enterprise capability/profile modeling beyond current pool and account
  concepts
