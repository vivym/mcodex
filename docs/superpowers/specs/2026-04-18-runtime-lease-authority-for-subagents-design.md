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
- define request admission at every outbound provider round-trip boundary, not
  at turn or `ModelClientSession` construction time
- make `usage_limit_reached` close the current lease to future new requests
  without killing in-flight siblings
- make terminal `401 unauthorized` invalidate the current lease generation and
  trigger tree-scoped cancellation
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
- Do not broaden pooled mode to stdio app-server shapes that load or run more
  than one top-level thread context at a time.
- Do not redesign manual account switching semantics for fresh runtime
  instances.
- Do not introduce a background lease-rotation worker or a new operator UI.
- Do not change the existing authenticated no-commit replay guard or
  transport-reset model from the 2026-04-10 pooled account design.

## Constraints

- The original multi-account pool design already defines pooled allocation as
  runtime-scoped, not request-scoped, and explicitly says threads in the same
  CLI or TUI runtime share one pooled lease context.
- The original pooled-runtime applicability boundary also remains in force:
  pooled mode is valid for CLI/TUI runtimes and for stdio app-server only while
  at most one top-level thread context is loaded or running. This design does
  not broaden that boundary.
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

Introduce a runtime-shared `RuntimeLeaseHost` as the owner of the runtime's
pooled lease authority. The host contains one `RuntimeLeaseAuthority` and is
the only place inside a pooled runtime that may create, renew, close, or release
an account-pool lease.

Responsibilities:

- hold the runtime's current active lease generation
- decide whether the current generation still accepts new requests
- run the equivalent of today's `prepare_turn` logic
- own proactive rotation, hard-failure rotation, and switch suppression state
- report authoritative read-only lease facts to request callers
- process rate-limit observations, `usage_limit_reached`, and terminal `401`
  unauthorized failures
- reuse the existing durable lease tables, fencing, health persistence, and
  account selection machinery where possible

This moves switching state such as `pending_rotation` and proactive-switch
state out of per-session control and back into one runtime-owned control plane.

Placement:

- `RuntimeLeaseHost` lives above individual `Codex` session services.
- For CLI and TUI runtimes, the host key is the process runtime.
- For stdio app-server runtimes, that host is scoped to the one pooled
  selection context already permitted by the earlier design. In practice, the
  host key must be the loaded top-level thread or equivalent pooled-selection
  context, not the process-global `ThreadManager`.
- Starting, resuming, forking, or loading a second top-level thread while pooled
  mode is active must fail clearly or make pooled mode unavailable, matching
  the earlier app-server restriction.
- Sessions receive shared handles to this host, but no child session owns its
  lifetime.
- A session that receives a runtime host handle must not create its own
  per-session `AccountPoolManager`.

### 2. Introduce explicit request-scoped lease snapshots

Every new remote model request must first acquire a `LeaseSnapshot` from
`RuntimeLeaseAuthority`.

The snapshot should contain at least:

- `account_id`
- `lease_epoch` or equivalent generation token
- `auth_handle`
- `admission_id` or equivalent request-completion token
- `session_id` or local thread id for the requesting session
- `collaboration_tree_id`
- request-boundary kind for diagnostics
- whether the generation currently accepts new requests
- optional switch metadata useful for observability and debugging

This snapshot is the admission token for one new request. Callers must not
cache pooled auth as an open-ended capability for future requests.

The key rule is:

- new requests acquire a fresh snapshot
- in-flight requests may continue using the snapshot they started with
- late reports from an old snapshot may only affect that same generation

For this spec, a "request boundary" means every outbound provider operation
that can start or advance remote model work. It is intentionally narrower than
a local Codex turn and broader than only the first sampling call in a turn.

Examples that must acquire a fresh lease admission:

- ordinary HTTP or WebSocket model sampling calls
- continuation sampling after tool output
- auth-recovery retry attempts that issue another provider round-trip
- WebSocket preconnect, prewarm, and streaming setup that can create or advance
  provider-side session state
- realtime or auxiliary provider calls that use the same pooled auth path
- compaction, memory-summary, or background model calls that execute inside the
  same pooled runtime

Local shell execution, local state mutation, and local-only tool handling do not
create a provider request boundary unless they themselves call the provider.

All pooled `ModelClient` entry points that can call `current_client_setup()` or
otherwise materialize auth for provider I/O must route through the same request
admission helper. Unsupported pooled paths must fail clearly rather than
falling back to direct shared auth or stale session-scoped lease auth.

The acquisition step also records one admitted request against the current
generation. The caller receives a `LeaseAdmissionGuard` or equivalent
request-scoped RAII guard. The guard must release that admission exactly once
when the request finishes, whether it succeeds, fails, or is cancelled. The
authority must tolerate duplicate or late release attempts without corrupting
generation state, but the intended API shape should make double release
unrepresentable.

The guard's lifetime must match provider work completion, not the lexical scope
that creates provider work. For streaming APIs that return a stream, connection,
or response handle before SSE or WebSocket work is complete, the returned object
must own the `LeaseAdmissionGuard` until EOF, explicit cancel, drop, or
transport failure. A guard acquired inside a function that returns a
`ResponseStream`-like value must not be dropped before the returned stream is
done.

Transport caching rules:

- active response streams hold admission until stream completion, cancellation,
  drop, or transport failure
- WebSocket preconnect or prewarm that only establishes an idle reusable
  transport is handshake-scoped; its admission is released when the handshake
  finishes
- idle cached transports must not hold a lease admission open indefinitely
- idle cached transports must be tagged with the account id, lease generation,
  and transport-reset generation they were created under
- before a cached transport is reused for new provider work, the caller must
  reacquire admission and verify the cached transport tags match the newly
  admitted snapshot and current session transport generation
- if the current lease generation is closed, invalidated, or different from
  the cached transport generation, the cached transport must be closed or
  discarded instead of reused

`acquire_request_lease()` behavior:

- if the current generation is `accepting`, it returns a snapshot plus
  admission guard
- if the current generation is `draining`, it waits asynchronously until the
  generation releases and the authority acquires a replacement generation
- the wait must be cancellable by the caller's turn/request cancellation token
- it must not return a non-accepting snapshot
- typed rejection is reserved for no eligible account, runtime shutdown,
  unsupported pooled path, non-pooled fallback, or caller cancellation

Lease liveness model:

- this slice keeps one leased generation active for a pooled runtime at a time
- a generation may be in one of three relevant states:
  - `accepting`
  - `draining`
  - `released`
- when the authority acquires a generation, it starts a runtime-owned
  heartbeat for that generation
- when the generation is closed to new work or invalidated, it becomes
  `draining`; the heartbeat continues while admitted work remains
- when the admitted-request count reaches zero, the authority releases that
  generation and may acquire the next one
- future new requests are blocked during `draining`; this slice does not
  require overlapping dual leases for one runtime
- therefore no replacement generation exists until the current draining
  generation has released

### 3. Make remote-context continuity session-scoped

Replace the current "session-local optional current lease auth" role with a
session-local `SessionLeaseView`.

`SessionLeaseView` should:

- consume runtime lease snapshots
- track the session's last successfully used `account_id`
- decide whether this session must reset remote conversation or session state
  before the next request
- expose the auth view the `ModelClient` should use for the current request
- drive the existing transport-reset generation and remote-session reset
  mechanics rather than introducing a second reset state machine

This preserves the important distinction between:

- which account the runtime is currently using
- whether a specific session can safely continue its own remote context on that
  account

The runtime chooses the account. The session decides whether its own remote
context can continue across that account boundary.

The reset boundary remains the same as the 2026-04-10 pooled design:

- when a cross-account change requires remote-context reset,
  `SessionLeaseView` must drive the existing `transport_reset_generation`
  boundary used by `ModelClient`
- when remote-context reset is required, the session must mint a fresh outbound
  remote session identity instead of reusing the old remote conversation or
  thread id
- when pool policy disallows cross-account context reuse, remote conversation
  ids, thread ids, and equivalent transport handles must not be carried across
  the account boundary
- this design does not add a second independent generation mechanism for
  transport reset
- reset must clear cached WebSocket session state and previous response ids
  through the existing `ModelClient` reset seam
- late provider events from an abandoned transport generation must be ignored or
  rejected by the same generation checks used for account-lease reports

### 4. Make cancellation and fault scope collaboration-tree-scoped

Introduce a `CollaborationTreeRegistry` that tracks parent-child relationships
for active sessions and exposes tree-scoped cancellation targets.

Responsibilities:

- extend the existing collaboration tree model rather than creating a second
  unrelated tree subsystem
- register collaboration trees and active members
- identify which sessions belong to the same parent task tree
- broadcast cancellation to cancellable members of one tree when required
- keep this scope separate from runtime-wide lease ownership

Tree-membership model:

- `ThreadSpawn` relationships should continue to use the existing live and
  persisted spawn-edge tree as their canonical source
- non-`ThreadSpawn` delegated sessions such as `review` and guardian reviewer
  flows should be attached to that same logical collaboration tree through
  runtime-scoped membership edges rooted at their parent thread or turn
- the registry is therefore a logical extension of the existing tree, not a
  second independent hierarchy with different semantics
- delegated membership is invocation-scoped unless the delegated thread is a
  normal `ThreadSpawn` child with persisted spawn edges
- one-shot review sessions bind to the parent turn's collaboration tree for
  that review invocation and unregister on completion or cancellation
- reusable guardian sessions must rebind membership for each review invocation
  or parent turn they are evaluating, and must unregister that binding when the
  invocation finishes, even if the hidden `Codex` instance is retained for reuse
- lease admissions and fault reports from delegated sessions must carry the
  active `collaboration_tree_id`; the registry must not infer the reporting
  tree only from `SubAgentSource::Review` or `SubAgentSource::Other`
- memory consolidation, compact, agent-job, and other background model work
  must also register a deterministic collaboration tree before acquiring a
  lease admission
- background work initiated by a parent thread or turn should bind to that
  parent tree for the duration of the invocation
- background work with no live parent context should bind to a synthetic
  runtime-scoped background tree, so terminal `401` cancellation remains
  deterministic and does not become runtime-wide by default
- synthetic background-tree membership is also invocation-scoped and must be
  unregistered on completion or cancellation
- the synthetic background tree should be per invocation, for example
  `background:<runtime-host-id>:<invocation-id>`, not one singleton tree for
  all background work in the runtime

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

1. The caller reaches an outbound provider request boundary.
2. The caller asks `RuntimeLeaseAuthority` for `acquire_request_lease()`.
3. If the current generation is accepting, the caller receives a
   `LeaseSnapshot` and `LeaseAdmissionGuard` for that generation.
4. If the current generation is draining, the caller waits cancellably for the
   replacement generation; it must not receive a non-accepting snapshot.
5. If no replacement can be acquired or the caller is cancelled, the caller
   receives a typed rejection.
6. The caller hands the accepted snapshot to
   `SessionLeaseView::before_request(...)`.
7. `SessionLeaseView` decides whether the session must reset its remote context
   before the request and returns the auth view the client should use.
8. The request runs.
9. On success, the caller reports rate-limit observations using the same
   snapshot.
10. On `usage_limit_reached` or terminal `401`, the caller reports the fault
   using the same snapshot.
11. The caller's `LeaseAdmissionGuard` releases the admission when the request
   ends or is dropped.

This makes lease admission, execution, and reporting consistent across parent
threads, spawned agents, and internal worker sessions.

Any reused child `Codex` instance, including guardian or review sessions that
survive across multiple requests or turns, must reacquire a fresh lease
snapshot for each new request boundary. Reuse of a child session must not pin
that session to creation-time pooled auth.

Prewarmed or cached `ModelClientSession` values must not carry pooled lease
auth as a long-lived capability. They may cache transport state only when the
session-local continuity rules allow it; auth and admission must be reacquired
at the next provider request boundary.

Cached transport reuse is allowed only after the new provider request boundary
has acquired an accepting lease snapshot and verified that the cached
transport's account id, lease generation, and transport-reset generation still
match that snapshot. Generation close or invalidation must make cached
transports for that generation ineligible for reuse.

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
- future callers attempting `acquire_request_lease()` must be blocked until the
  current generation drains, releases, and a replacement generation is acquired
- the exhausted generation should still accept late completion or telemetry
  reports from already admitted requests

This preserves the agreed behavior: block later work, not in-flight siblings.

The rotation sequence is:

- close the current generation to new work
- let already admitted work drain under the same generation heartbeat
- once the generation finishes draining, release it
- only then acquire the replacement generation for future new requests

This preserves single-generation lease ownership for one runtime in this slice.

#### Unauthorized reporting boundary

A raw wire-level `401` does not immediately invalidate the pooled generation on
its own.

Before reporting unauthorized to `RuntimeLeaseAuthority`, the caller must first
exhaust the existing auth-recovery or backend revalidation path for that
request:

- for local managed auth, attempt the normal auth recovery path once
- for remote leased auth, let the backend renew or revalidate the leased
  credential and treat backend-confirmed revocation or unrecoverable failure as
  authoritative
- only terminal unauthorized or unrecoverable auth failure reports invalidate
  the generation

This spec changes lease ownership and cancellation scope. It does not change
the earlier replay-guard or authenticated no-commit policy for current-turn
retry.

#### terminal `401 unauthorized`

When any session reports terminal `401 unauthorized` for the current generation:

- the runtime immediately invalidates that generation for future new requests
- the collaboration tree containing the reporting session should receive
  best-effort cancellation for all cancellable members
- already admitted work outside the reporting tree enters drain-only behavior:
  no further requests may be admitted on that generation, but unrelated work is
  allowed to complete or fail naturally unless a future slice chooses broader
  interruption semantics
- new requests are blocked until the invalidated generation drains, releases,
  and the authority acquires a replacement generation
- late responses from the invalidated generation must be treated as stale and
  may not damage later generations

This preserves the agreed behavior: treat the current shared account as
invalid, and quickly stop tree-local work that is still cancelable.

State table:

| Event | Admission after event | Cancellation | Existing admitted work | Replacement generation |
| --- | --- | --- | --- | --- |
| Soft rate pressure | remains open unless policy chooses future rotation | none | continues | not acquired immediately |
| `usage_limit_reached` | closed to new work | none | all admitted work drains naturally | acquired only after drain and release |
| terminal `401 unauthorized` | invalidated for new work | reporting tree best-effort cancel | other admitted work drains naturally | acquired only after drain and release |

This table is authoritative for this slice. There is no fast-replacement or
overlapping second lease generation in this design.

### 8. Use generation rules to make races explicit

All health and fault reports must include the `LeaseSnapshot` or at least its
generation identity.

Required rules:

- reports must carry `admission_id`, `account_id`, generation, requesting
  session/thread id, and `collaboration_tree_id`
- a report may only mutate the generation it belongs to
- a late report from generation `N` may not poison generation `N+1`
- callers may not start a new request from a stale snapshot after the runtime
  has closed or invalidated that generation
- auth access without lease admission is not allowed for pooled requests
- a draining generation may continue to receive completion and telemetry
  reports, but may not admit new requests
- if the same account is reacquired with a new generation, stale reports from
  the old generation still remain stale and must not affect the new generation

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

1. Introduce `RuntimeLeaseHost` and thread it through `CodexSpawnArgs` and
   `SessionServices` before changing child behavior. The first safe cut is the
   API seam: sessions either receive a runtime host handle or explicitly run in
   non-pooled fallback mode.
2. Enforce the invariant that a session with a runtime host handle cannot build
   a per-session `AccountPoolManager`. This cut must land before any
   `ThreadSpawn`, review, or guardian path is rerouted, so one runtime cannot
   temporarily contain two pooled control planes.
3. Move pooled lease lifetime, heartbeat, request admission, and draining
   ownership into the host. The host contains `RuntimeLeaseAuthority`, backed
   by the current account-pool machinery instead of a second selection engine.
4. Introduce `LeaseSnapshot` and `LeaseAdmissionGuard` at every outbound
   provider request boundary, and stop relying on session-construction-time
   pooled-auth snapshots as the primary control seam.
5. Replace the primary role of `SessionLeaseAuth` with `SessionLeaseView`,
   explicitly reusing the existing transport-reset generation, WebSocket reset,
   previous-response-id clearing, and remote identity reset mechanics.
6. Extend the existing collaboration tree into `CollaborationTreeRegistry` for
   delegated non-`ThreadSpawn` child sessions instead of creating a second tree
   system. Add invocation-scoped registration and cleanup for review and
   guardian reuse, plus deterministic parent-bound or synthetic background-tree
   registration for memory, compact, agent-job, and other background model
   work.
7. Route `ThreadSpawn`, `Review`, guardian, and other child-thread creation
   through the runtime host instead of separate per-session ownership.
8. Convert fault reporting APIs to require explicit admission and generation
   context, including session/thread id and `collaboration_tree_id`, and only
   emit terminal unauthorized after existing recovery or revalidation paths
   fail.
9. Retire `inherited_lease_auth_session` as the main pooled-runtime path,
   keeping only a narrow compatibility shim during migration if needed.

This lets the implementation change the ownership boundary without rewriting
the durable lease database or account ranking logic in the same slice.

## Testing

The design requires concurrency-focused tests, not just happy-path selection
tests.

At minimum, add coverage for:

- parent session plus multiple child sessions concurrently sharing the same
  `account_id` and `lease_epoch`
- every pooled `ModelClient` provider entry point acquiring request admission,
  including HTTP streaming, WebSocket streaming, prewarm/preconnect, auth
  recovery retry, compaction, memory-summary, and background model calls
- streaming admissions being held by the returned stream/connection object
  until EOF, cancellation, drop, or transport failure
- WebSocket preconnect/prewarm admissions being released after idle transport
  handshake, and cached transports being discarded rather than reused after
  generation close, invalidation, account change, or transport-reset generation
  change
- runtime-owned heartbeat continuing while a closed or invalidated generation
  drains already admitted work
- new requests being blocked while a generation is draining
- draining acquisition waiting cancellably for the replacement generation and
  never returning a non-accepting snapshot
- no replacement generation being acquired until the draining generation has
  released
- `usage_limit_reached` from one child closing the current generation to later
  new requests without cancelling already admitted siblings
- terminal `401` only invalidating the generation after existing recovery or
  backend revalidation paths fail
- `401` from one child invalidating the generation, triggering tree-scoped
  cancellation for the reporting tree, and leaving unrelated already admitted
  work in drain-only behavior
- late `401` or quota-failure reports from an old generation not affecting the
  next generation
- a child session following a later runtime rotation automatically on its next
  new request
- per-session remote-context reset decisions remaining local to that session
  even though lease ownership is runtime-shared
- remote-context reset clearing WebSocket session state and previous response
  ids, minting a fresh remote identity when required, and ignoring late events
  from abandoned transport generations
- `spawn_agent`, `review`, guardian, and other child-thread sources all
  exercising the same request-admission and fault-reporting path
- one-shot review registration cleanup and reused guardian per-invocation
  registration cleanup
- parent-bound and synthetic background-tree registration for memory,
  compact, agent-job, and other background model work, including terminal
  `401` cancellation scope
- synthetic background tree ids being per background invocation rather than one
  singleton runtime-wide tree
- admission release on success, error, cancellation/drop, double-release
  attempts, release after generation close, heartbeat renewal failure, and
  missing lease during drain
- stale-generation reports being ignored after the same account is reacquired
  under a new generation
- pooled-mode applicability remaining unchanged for stdio app-server: one
  loaded top-level thread context only
- stdio app-server pooled-mode gates covering top-level thread start, resume,
  fork, load, and unload paths

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

Adopt a runtime-owned pooled lease host for all child sessions in the same
pooled runtime, keep remote-context continuity session-scoped, and keep
terminal-`401` cancellation collaboration-tree-scoped.

This is the cleanest way to restore the original sticky-lease design while
supporting multi-agent execution without fragmented rotation behavior.
