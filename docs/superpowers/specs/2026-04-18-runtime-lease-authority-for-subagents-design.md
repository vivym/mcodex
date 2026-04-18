# Runtime Lease Authority For Subagents Design

This document defines how pooled account leases should behave when a runtime
creates multiple child threads or agents. It resolves the current mismatch
between the original sticky-lease model, which is runtime-scoped, and the
current implementation split where some child sessions inherit a static lease
auth view while others create their own account-pool manager.

The design intentionally focuses on lease ownership, request admission,
rotation, and failure propagation for pooled multi-agent execution. It does not
redesign backend lease persistence, broaden pooled mode to unsupported
multi-client app-server shapes, or introduce a large account-management UI.

## Summary

The recommended direction is:

- restore runtime-scoped sticky lease semantics for pooled execution
- make one pooled runtime own one active lease authority, not one per thread
- make every child session in that runtime consume the same runtime lease by
  default
- separate three concerns that are currently entangled:
  - runtime-scoped lease ownership and rotation
  - session-scoped remote-context continuity
  - collaboration-tree-scoped cancellation and fault propagation
- replace static inherited lease auth snapshots with dynamic per-request lease
  acquisition
- make `usage_limit_reached` close the current lease to future new requests
  without killing in-flight siblings
- make `401 unauthorized` invalidate the current lease generation and trigger
  tree-scoped cancellation
- use explicit lease generations so late results from an old lease cannot
  corrupt the current lease state
- converge `spawn_agent`, `review`, `guardian`, and other child-thread paths on
  one shared pooled-lease model

## Goals

- Restore the original account-pool contract: one pooled runtime instance holds
  one sticky active lease.
- Make child-thread behavior consistent across `spawn_agent`, review flows, and
  internal subthreads.
- Support concurrent child requests on the same leased account without forcing
  per-thread lease allocation.
- Ensure that any child thread created inside a pooled runtime follows later
  lease rotations automatically for future new requests.
- Centralize switching and health decisions so rotation is explainable and does
  not fragment across threads.
- Preserve per-session remote-context continuity and reset behavior where that
  continuity matters.
- Eliminate static inherited lease snapshots as the primary runtime mechanism.

## Non-Goals

- Do not redesign durable backend lease storage or the existing lease SQL
  schema in this slice.
- Do not introduce per-request account load balancing or round-robin
  distribution.
- Do not broaden pooled mode to multi-client WebSocket app-server processes.
- Do not redesign manual account switching semantics for fresh runtime
  instances.
- Do not introduce a background lease-rotation worker or a new operator UI.

## Constraints

- The original multi-account pool design already defines pooled allocation as
  runtime-scoped, not request-scoped, and explicitly says threads in the same
  CLI or TUI runtime share one pooled lease context.
- The current implementation does not fully honor that contract:
  - `ThreadSpawn` child threads create fresh session-local account-pool
    managers instead of inheriting shared pooled lease ownership.
  - `review` and `guardian` child threads inherit a static
    `LeaseScopedAuthSession`, but that inheritance is creation-time state, not
    a dynamic view of the runtime's current lease.
  - per-session account-pool managers currently own local
    `pending_rotation`, proactive-switch state, and failure handling, which
    makes pooled switching effectively thread-local.
- `ModelClient` already has the correct high-level preference order: use
  lease-scoped auth when available, otherwise fall back to shared auth.
- Existing durable lease ownership, fencing, and account-health persistence are
  valuable and should be reused rather than replaced.

## Problem Statement

The current account-pool implementation has drifted into a mixed model:

1. the original design says pooled lease ownership is runtime-scoped
2. some child threads currently behave as independent pooled holders
3. some child threads currently behave as static auth inheritors

That split creates several practical problems:

- `spawn_agent` children can independently switch accounts even when the user
  expects them to be collaborators under one parent task
- child-thread behavior is inconsistent across `ThreadSpawn`, `Review`, and
  other internal subthread types
- lease switching state such as `pending_rotation` is fragmented across
  sessions instead of being a property of the pooled runtime
- static inherited auth cannot express "follow the latest pooled lease after a
  later rotation"
- failure attribution is unclear because a child thread may fail on a shared
  account but have no authoritative path to mutate the runtime's current lease
  state

The design therefore needs to make one fact explicit:

> In pooled mode, lease ownership belongs to the runtime. Child-thread
> relationships affect cancellation and fault scope, not lease ownership.

## Approaches Considered

### Approach A: Extend static inherited lease auth to every child thread

Under this approach, `ThreadSpawn` would be changed to behave more like today's
`review` and `guardian` paths by inheriting a static
`LeaseScopedAuthSession` at creation time.

Pros:

- smallest short-term patch
- aligns more child threads with the user's expectation that they share one
  account

Cons:

- still models inheritance as a creation-time snapshot instead of a dynamic
  view of the runtime's current lease
- makes "parent rotated, child's next request should follow the new account"
  awkward and race-prone
- keeps ownership tied to a specific parent session instead of the runtime
- encourages more compatibility-layer logic around
  `inherited_lease_auth_session`

This approach is rejected.

### Approach B: Parent-thread lease owner plus dynamic child bridge

Under this approach, one root or parent session would remain the lease owner,
and child sessions would dynamically fetch the parent's current lease before
new requests.

Pros:

- better than static inheritance
- satisfies most parent-child sharing requirements
- smaller change than a full runtime-level ownership refactor

Cons:

- still ties lease ownership to one specific session instead of the runtime
- makes ownership awkward if the parent session idles, shuts down, or stops
  being the most natural control point
- mixes "collaboration tree root" with "pooled runtime lease owner" even
  though they are different concerns

This approach is acceptable but not preferred.

### Approach C: Runtime lease authority plus session lease views and tree registry

Under this approach, pooled lease selection and rotation are owned by one
runtime-level authority, while each session keeps its own remote-context
continuity state and each collaboration tree keeps its own cancellation scope.

Pros:

- matches the original sticky-lease model
- gives one clear control plane for account choice and rotation
- cleanly separates runtime ownership, session continuity, and tree-scoped
  cancellation
- naturally supports child threads following later rotations
- avoids growing the static inheritance compatibility path

Cons:

- larger refactor than a creation-time inheritance patch
- requires explicit lease snapshot and generation handling

This is the recommended approach.

## Recommended Design

### 1. Make pooled lease ownership runtime-scoped

Introduce a runtime-shared `RuntimeLeaseAuthority` as the only pooled lease
owner inside a pooled runtime.

Responsibilities:

- hold the runtime's current active lease generation
- decide whether the current generation still accepts new requests
- run the equivalent of today's `prepare_turn` logic
- own proactive rotation, hard-failure rotation, and switch suppression state
- report authoritative read-only lease facts to request callers
- process rate-limit observations, `usage_limit_reached`, and `401`
  unauthorized failures
- reuse the existing durable lease tables, fencing, health persistence, and
  account selection machinery where possible

This moves switching state such as `pending_rotation` and proactive-switch
state out of per-session control and back into one runtime-owned control plane.

### 2. Introduce explicit request-scoped lease snapshots

Every new remote model request must first acquire a `LeaseSnapshot` from
`RuntimeLeaseAuthority`.

The snapshot should contain at least:

- `account_id`
- `lease_epoch` or equivalent generation token
- `auth_handle`
- whether the generation currently accepts new requests
- optional switch metadata useful for observability and debugging

This snapshot is the admission token for one new request. Callers must not
cache pooled auth as an open-ended capability for future requests.

The key rule is:

- new requests acquire a fresh snapshot
- in-flight requests may continue using the snapshot they started with
- late reports from an old snapshot may only affect that same generation

### 3. Make remote-context continuity session-scoped

Replace the current "session-local optional current lease auth" role with a
session-local `SessionLeaseView`.

`SessionLeaseView` should:

- consume runtime lease snapshots
- track the session's last successfully used `account_id`
- decide whether this session must reset remote conversation or session state
  before the next request
- expose the auth view the `ModelClient` should use for the current request

This preserves the important distinction between:

- which account the runtime is currently using
- whether a specific session can safely continue its own remote context on that
  account

The runtime chooses the account. The session decides whether its own remote
context can continue across that account boundary.

### 4. Make cancellation and fault scope collaboration-tree-scoped

Introduce a `CollaborationTreeRegistry` that tracks parent-child relationships
for active sessions and exposes tree-scoped cancellation targets.

Responsibilities:

- register collaboration trees and active members
- identify which sessions belong to the same parent task tree
- broadcast cancellation to cancellable members of one tree when required
- keep this scope separate from runtime-wide lease ownership

This separation is intentional:

- lease switching scope is runtime-wide
- cancellation scope is collaboration-tree-wide
- remote-context continuity scope is session-wide

Those scopes must not collapse into one object.

### 5. Unify child-thread lease behavior

Inside a pooled runtime, every child session that issues remote model requests
should consume the shared `RuntimeLeaseAuthority` by default.

That includes:

- `SubAgentSource::ThreadSpawn`
- `SubAgentSource::Review`
- guardian reviewer sessions
- `SubAgentSource::MemoryConsolidation`
- any future internal child-thread source that executes in the same pooled
  runtime

This means the runtime has one active pooled lease context even when it
contains multiple child sessions.

If a future workload truly needs independent lease ownership, that should be an
explicit opt-out mode with separate product semantics. It should not be the
default behavior for ordinary child threads.

### 6. Define one request lifecycle for all pooled sessions

The pooled request lifecycle should be:

1. The caller asks `RuntimeLeaseAuthority` for `acquire_request_lease()`.
2. The caller receives a `LeaseSnapshot` for the current active generation, or
   a clear rejection if no current generation may accept new work.
3. The caller hands that snapshot to `SessionLeaseView::before_request(...)`.
4. `SessionLeaseView` decides whether the session must reset its remote context
   before the request and returns the auth view the client should use.
5. The request runs.
6. On success, the caller reports rate-limit observations using the same
   snapshot.
7. On `usage_limit_reached` or `401`, the caller reports the fault using the
   same snapshot.

This makes lease admission, execution, and reporting consistent across parent
threads, spawned agents, and internal worker sessions.

### 7. Define fault semantics explicitly

#### Soft rate pressure

Rate-limit observations that imply increasing pressure should update the
runtime's future rotation decision for the current generation, but should not
invalidate already admitted requests.

This remains a runtime-owned decision, not a per-thread decision.

#### `usage_limit_reached`

When any session reports `usage_limit_reached` for the current generation:

- the runtime marks that generation as closed to future new requests
- already admitted in-flight requests are not cancelled
- future callers attempting `acquire_request_lease()` must be blocked or routed
  to a rotated generation before they may start a new request
- the exhausted generation should still accept late completion or telemetry
  reports from already admitted requests

This preserves the agreed behavior: block later work, not in-flight siblings.

#### `401 unauthorized`

When any session reports `401 unauthorized` for the current generation:

- the runtime immediately invalidates that generation for future new requests
- the collaboration tree containing the reporting session should receive
  best-effort cancellation for all cancellable members
- new requests must reacquire against the next valid generation
- late responses from the invalidated generation must be treated as stale and
  may not damage later generations

This preserves the agreed behavior: treat the current shared account as
invalid, and quickly stop tree-local work that is still cancelable.

### 8. Use generation rules to make races explicit

All health and fault reports must include the `LeaseSnapshot` or at least its
generation identity.

Required rules:

- a report may only mutate the generation it belongs to
- a late report from generation `N` may not poison generation `N+1`
- callers may not start a new request from a stale snapshot after the runtime
  has closed or invalidated that generation
- auth access without lease admission is not allowed for pooled requests

These rules are what make dynamic following safe:

- a child session may have used account `A` for one request
- the runtime may later rotate to account `B`
- that child session's next new request will acquire a fresh snapshot for `B`
- in-flight work on `A` is allowed to finish, but it cannot reopen `A` for new
  work once `A`'s generation is closed

### 9. Evolve the current implementation toward the new boundary

The existing state model and durable lease machinery should be reused, but the
ownership boundary must move.

Migration direction:

1. Introduce `RuntimeLeaseAuthority` backed by the current account-pool
   machinery instead of inventing a second selection engine.
2. Introduce `LeaseSnapshot` and route pooled request admission through it.
3. Replace the primary role of `SessionLeaseAuth` with `SessionLeaseView`.
4. Route `ThreadSpawn`, `Review`, guardian, and other child-thread creation
   through the runtime authority instead of separate per-session ownership.
5. Convert fault reporting APIs to require explicit lease snapshot or
   generation context.
6. Add `CollaborationTreeRegistry` and route `401` cancellation through it.
7. Retire `inherited_lease_auth_session` as the main pooled-runtime path,
   keeping only a narrow compatibility shim during migration if needed.

This lets the implementation change the ownership boundary without rewriting
the durable lease database or account ranking logic in the same slice.

## Testing

The design requires concurrency-focused tests, not just happy-path selection
tests.

At minimum, add coverage for:

- parent session plus multiple child sessions concurrently sharing the same
  `account_id` and `lease_epoch`
- `usage_limit_reached` from one child closing the current generation to later
  new requests without cancelling already admitted siblings
- `401` from one child invalidating the generation and triggering tree-scoped
  cancellation
- late `401` or quota-failure reports from an old generation not affecting the
  next generation
- a child session following a later runtime rotation automatically on its next
  new request
- per-session remote-context reset decisions remaining local to that session
  even though lease ownership is runtime-shared
- `spawn_agent`, `review`, guardian, and other child-thread sources all
  exercising the same request-admission and fault-reporting path

## Risks And Mitigations

### Risk: hidden bypasses around the runtime authority

If any pooled path continues to fetch auth directly without acquiring a
snapshot, the runtime will end up with two partially overlapping control
planes.

Mitigation:

- make request admission the only supported way to obtain pooled auth for a new
  request
- audit request call sites during migration

### Risk: stale snapshots reused after rotation

If callers treat a snapshot as a long-lived capability, later rotations will
not take effect consistently.

Mitigation:

- document snapshots as request-scoped only
- require generation identity on all fault and health reports

### Risk: cancellation scope becomes too broad

If `401` cancellation is broadcast to the whole runtime, unrelated work may be
needlessly interrupted.

Mitigation:

- keep cancellation authority in `CollaborationTreeRegistry`
- keep lease invalidation runtime-wide, but cancellation tree-scoped

### Risk: session continuity logic is lost during refactor

If runtime ownership absorbs remote-context reset behavior, sessions will lose
their own continuity guarantees.

Mitigation:

- keep `SessionLeaseView` session-local
- keep "account choice" and "remote context reuse" as separate decisions

### Risk: the compatibility layer never goes away

If `inherited_lease_auth_session` remains a first-class mechanism forever, the
codebase will retain two overlapping pooled-lease models.

Mitigation:

- treat static inherited auth as transitional only
- plan explicit removal once all pooled child-thread paths use the runtime
  authority

## Decision

Adopt a runtime-owned pooled lease authority for all child sessions in the same
pooled runtime, keep remote-context continuity session-scoped, and keep
`401`-driven cancellation collaboration-tree-scoped.

This is the cleanest way to restore the original sticky-lease design while
supporting multi-agent execution without fragmented rotation behavior.
