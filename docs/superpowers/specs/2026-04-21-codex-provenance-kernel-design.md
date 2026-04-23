# Codex Trace Kernel Design

This document defines the mcodex-side kernel for tracing how code and execution
state evolve during Codex activity.

The first release should not be framed as a diff feature, a `git blame`
feature, or an extension of `thread/read`. It should be framed as a local trace
kernel with scoped hash-chained ledgers:

- `GlobalExecution`: what Codex executed, when, under which thread/turn/tool/process
- `WorkspaceCustody`: what workspace state, file state, hunk lineage, and
  revision alias facts were observed
- `AccessAudit`: which raw/blob reads were disclosed locally

The primary job of mcodex is to produce durable, queryable, exportable local
trace facts. Forgeloop remains the online product for organization-wide review,
incident analysis, entitlement, retention, and postmortem workflows.

## Summary

Build a Git-first local trace kernel with these properties:

1. Record execution lifecycle facts and code provenance facts as separate but
   explicitly linked journals.
2. Enforce a mandatory mutator boundary for every file-writing execution path,
   instead of relying on turn-scoped net diffs.
3. Use anchored code coordinates for queries:
   - live workspace queries: `workspace stream/head selector + path + line/range`
   - recorded-state queries: `workspace_state_id + path + line/range`
   - historical queries: `git commit/tree/alias selector + path + line/range`
4. Treat turn-level and workspace-level summaries as derived read models, not
   the primary truth source.
5. Capture synchronous chain-of-custody facts first, then run heavier hunk and
   range indexing asynchronously when needed.
6. Make selector status, freshness, certainty, coverage, and indexing orthogonal
   result dimensions instead of collapsing them into one status.
7. Keep v1 Git-first for line-level provenance. Non-Git workspaces may still
   record execution and file-level facts, but they are not first-class range
   provenance targets.
8. Expose a stable export contract that later maps cleanly into Forgeloop's
   execution and evidence plane.

Recommended implementation:

- add `codex-provenance` for models and lineage/projector logic
- add `codex-provenance-store` for a dedicated provenance journal and query
  index, backed by a separate local SQLite database
- add a `MutationSupervisor` integration layer in `codex-core`
- add provenance-specific app-server v2 APIs instead of extending `thread/read`

## Why This Is Not a Diff Feature

The target question is not "what changed in this turn?" It is:

- which request initiated the logic
- which assistant turn and tool activity produced it
- which later turns reshaped it
- whether the attribution is exact, stale, ambiguous, partial, or unavailable
- how the current or historical code range maps back to those observed facts

A net turn diff is a useful presentation artifact, but it is not strong enough
to preserve chain-of-custody across shell writes, long-lived processes, bulk
rewrites, branch switches, or external drift.

## Goals

- Answer: "How did the logic at this anchored code coordinate get here?"
- Preserve chain-of-custody for Codex-driven file mutations without pretending
  certainty where the recorder cannot prove it.
- Record enough execution context for future systems to reconstruct work traces
  without reverse-engineering rollouts.
- Support exact historical lookup for Git-backed workspaces when a selected
  revision can be mapped to recorded workspace state.
- Provide export-safe local facts for future Forgeloop ingestion.
- Keep Codex language-agnostic; symbol, issue, incident, and actor semantics
  remain outside the kernel.

## Non-Goals

- Do not make `git blame` the primary provenance mechanism.
- Do not require symbol parsing inside Codex v1.
- Do not build the online postmortem platform inside mcodex.
- Do not backfill arbitrary historical repository history or historical rollouts.
- Do not make incident, issue, PR, employee, or performance objects first-class
  provenance keys inside mcodex.
- Do not promise line-level provenance for non-Git workspaces in v1.
- Do not implement SaaS upload, tenant authorization, retention, or raw-content
  approval inside mcodex.

## Relationship to Forgeloop

Forgeloop is the future online consumer of this kernel's facts.

### mcodex owns

- local capture of execution and code provenance facts
- local workspace and stream identity
- workspace state capture and revision alias mapping
- hunk lineage and range projection
- local provenance query APIs
- local blob manifests and export cursors

### Forgeloop owns

- SaaS tenant and repo enrollment
- actor identity and organization mapping
- ExecutionPackage, RunSession, Review, Release, Incident, and related business
  objects
- cross-user repo aggregation
- policy, raw-content entitlement, retention, legal hold, and cloud blob
  lifecycle
- online review, postmortem, and learning workflows

## Design Principles

1. **Separate execution facts from code facts.** Code provenance without
   execution provenance leaves future systems guessing how work happened.
2. **Capture first, project second.** Chain-of-custody facts must be durable
   before expensive line-level indexing finishes.
3. **Git-first for exact range provenance.** Git is the historical coordinate
   system for v1.
4. **Derived summaries are not the truth source.** Turn summaries, file
   summaries, and collapsed range summaries are projections.
5. **Never guess across ambiguous identity or drift boundaries.**
6. **Stable export contract beats convenient local-only shapes.**

## Current Reality

Useful existing building blocks:

- turn-scoped diff tracking in `core/src/turn_diff_tracker.rs`
- tool event emission in `core/src/tools/events.rs`
- shell, `apply_patch`, `js_repl`, and unified exec handlers in `codex-core`
- ghost snapshot and git helper machinery in `codex-git-utils`
- existing app-server v2 infrastructure

Important current limitations:

1. `TurnDiffTracker` is turn-scoped and effectively patch-seeded, not a shared
   write barrier for all file mutations.
2. `shell`, `js_repl`, and unified exec do not currently flow through one common
   mutation observation path.
3. ghost snapshots are undo-oriented Git snapshots, not provenance baselines.
4. current local SQLite state is rollout/thread metadata oriented, not a
   replayable provenance journal.
5. current app-server read surfaces are thread-oriented, not provenance-oriented.

These limitations mean the optimal solution is a new trace subsystem, not a
small extension of the current diff and thread read models.

## V1 Scope

### Supported in v1

- Git-backed workspaces
- exact and bulk file mutations caused by Codex tools
- long-lived process sessions when mutations remain attributable to a process
  activity or child mutation interval
- line-level provenance queries for Git-backed workspaces
- local export of execution and code provenance facts

### Limited in v1

- non-Git workspaces may record execution and file-level mutation facts
- non-Git workspaces are not guaranteed line-level or historical range
  provenance
- manifest-only states are not queryable as exact line-range provenance targets

### Excluded from v1

- symbol-aware queries
- cross-repo semantic equivalence
- SaaS upload and online access control

## Architecture

The trace kernel is defined by six contracts. These contracts are more important
than the crate split because they define which data is authoritative and which
data is derived.

1. capture and mutation observation contract
2. ledger contract
3. state and continuity contract
4. projection contract
5. query contract
6. blob and export contract

### 1. Capture and Mutation Observation Contract

Add a mandatory `MutationSupervisor` in `codex-core`.

Its job is to own execution activities, mutation intervals, and immutable
evidence before any provenance-specific indexing exists. It does not compute
hunk lineage itself. It does:

- sit at the unified tool orchestration boundary for every mutating tool class
- classify write-capable activity
- open and close activities and mutation intervals
- capture pre-state and post-state anchors
- record file-change evidence
- write synchronous execution and code journal facts
- schedule projector work when heavy indexing is needed

The supervisor must be realized through shared ingress adapters, not only
call-site discipline:

- `SupervisedExecutorFileSystem`
  - wraps the concrete `ExecutorFileSystem` used by app-server/core write APIs
- `SupervisedCommandExec`
  - wraps `CommandExecManager` entrypoints that can mutate the workspace

Exact-custody paths use these wrapped ingress objects. Raw filesystem or command
implementations that bypass them are not exact by contract.

#### Tool classes

All first-party mutating tools, dynamic/MCP tools declared as write-capable,
agent jobs that can write the workspace, and future code-mode execution paths
must either enter the `MutationSupervisor` or be explicitly recorded as
`unsupervisedCodexObservation` or `externalObservation` / out-of-scope for exact
attribution.

- `ExactMutator`
  - direct file-changing tools with structured change payloads, such as
    `apply_patch`
- `OpaqueMutator`
  - tools that may write files indirectly, such as shell and `js_repl`
- `LongRunningMutator`
  - tools whose writing lifetime extends beyond the initial call, such as
    unified exec sessions with `exec_command` and `write_stdin`
- `BulkMutator`
  - operations expected to rewrite many files. Bulk mutators may emit one or
    more `MutationObservation`s, each classified independently before
    projection:
    - `RevisionTransition`, such as `git checkout` and `git switch`
    - `FormatterRewrite`, such as `cargo fmt`
    - `GeneratorOutput`, such as large code generators
    - `OpaqueBulkRewrite`, when semantics cannot be proven
- `NonMutator`
  - read-only tools that do not open mutation intervals

#### Exact custody boundary

v1 exact provenance is guaranteed only for writes that pass through shared
write-capable ingress owned or supervised by Codex.

The exact boundary should sit at concrete write entry points such as:

- `ExecutorFileSystem`-backed file mutation paths when the trait object is wrapped
  by the supervisor-owned provenance ingress
- structured first-party mutators such as `apply_patch`
- supervised command execution paths such as `CommandExecManager`, but only when
  the execution runtime can produce explicit provenance write fences or route
  writes through a supervisor-owned structured file shim
- write-capable MCP or dynamic tools that are explicitly routed through the same
  mutation gate

If app-server `fs/writeFile`, `fs/remove`, `fs/copy`, or `fs/createDirectory`
use the wrapped `ExecutorFileSystem`, they are exact-custody paths. File or
command mutations that bypass the wrapped ingress, including one-off
`command/exec` paths not yet routed through the supervisor, must not claim exact
custody in v1. They either need to adopt the shared ingress or emit
`code.unsupervised_workspace_mutation_observed` /
`unsupervisedCodexObservation`. True outside drift still uses
`code.external_workspace_mutation_observed` / `externalObservation`.

This keeps the v1 contract narrow and verifiable. A future path can graduate
from external observation to exact provenance without changing the logical model
once it is routed through the shared ingress.

When writes flow through supervisor-owned exact ingress, artifact capture should
be write-through: post-image content, deltas, and other exact replay artifacts
should stream into immutable storage during the interval rather than waiting for
close-path rescans. The close path should primarily seal the interval,
workspace-state refs, and manifests. Large synchronous close-path capture
remains the fallback for opaque write paths that cannot emit incremental exact
artifacts through shared ingress.

For `CommandExecManager`, canonical process identity is kernel-owned. The
provenance kernel mints an opaque `process_activity_id` when the process
activity is recorded. Client-facing ids such as app-server `(connection_id,
process_id)` remain local metadata and must not be used as ledger identity or
cross-scope cause targets.

In v1, generic shell/unified-exec workloads are exact only at the process
activity boundary unless they opt into one of the exact child-interval
attribution mechanisms below. A supervised command path that cannot emit those
signals still records execution, custody, and coarse drift facts, but must not
claim exact line-level child-interval attribution.

#### Activity and interval rules

Do not make one object cover both execution lifetime and file mutation
intervals. The kernel has two related primitives:

- `TraceActivity`
  - session, thread, turn, item, tool call, process, baseline capture, external
    observation, projector job, alias resolution, or repair activity
- `MutationInterval`
  - a bounded interval where file-system state may have changed and pre/post
    evidence must be captured

Every `MutationInterval` records:

- execution context
- tool or process identity
- exactly one workspace instance and one workspace stream identity
- pre-state anchor
- post-state anchor
- evidence collection mode
- observed changed paths
- immutable file-change evidence refs
- recorder result dimensions

Trace activities and mutation intervals are the primary recording primitives.
Turn summaries and workspace summaries are derived later.

`MutationInterval` is the execution-side envelope for one workspace-local write
window. The custody-side state transition it closes is a separate first-class
fact:

```text
WorkspaceTransition
- transition_id: String
- primary_cause: CauseRef
- supporting_evidence_event_refs: Vec<EventRef>
- workspace_stream_id: String
- workspace_instance_id: String
- pre_workspace_state_id: Option<String>
- post_workspace_state_id: String
- recorder_status: RecorderStatus
```

Rules:

- every closed `MutationInterval` that observes or reconciles workspace changes
  must produce exactly one `WorkspaceTransition`
- a single tool call or process activity may emit multiple
  `WorkspaceTransition`s, but each transition is scoped to exactly one
  `workspace_stream_id`
- a tool call that touches multiple workspaces must split custody capture into
  one workspace-local `MutationInterval` plus one `WorkspaceTransition` per
  affected workspace; implementations must not silently drop one workspace or
  fold multiple workspace streams into one interval
- drift observations that become authoritative custody facts must also mint a
  new `post_workspace_state_id` and advance the workspace head through a
  `WorkspaceTransition`; recording only a drift event without a new recorded
  state is invalid
- `primary_cause = observationSet` and `supporting_evidence_event_refs` must
  be de-duplicated and ordered by the
  canonical `(ledger_scope, stream_id, stream_epoch, sequence)` of the
  referenced events
- observation refs in `WorkspaceTransition` point to reconcile/drift journal
  facts such as `code.unsupervised_workspace_mutation_observed` or
  `code.external_workspace_mutation_observed`; they are not required to be
  `MutationObservation`s
- the authoritative establishing event for a transition is the enclosing
  `code.workspace_state_captured` event that carries this
  `WorkspaceTransition`; canonical transition payloads must not embed an
  `EventRef` back to that same event
- `primary_cause = mutationInterval` is valid only for Codex-attributable
  writes that closed an `execution.mutation_interval_closed` event
- when a transition closes a Codex-attributable write window,
  `primary_cause` must be `mutationInterval`; any reconcile, drift,
  unsupervised, or external observations from the same close path must be
  recorded in `supporting_evidence_event_refs`
- when a transition is an authoritative reconcile with no owning mutation
  interval, `primary_cause` must be `observationSet`; implementations must not
  invent a synthetic Codex write interval for that transition
- `supporting_evidence_event_refs` is explanatory evidence only; it must not
  override the single authoritative `primary_cause`
- `primary_cause.observationSet.observation_event_refs` and
  `supporting_evidence_event_refs` must be disjoint
- when `primary_cause = observationSet`, every observation that participates in
  the authoritative reconcile must appear in
  `primary_cause.observationSet.observation_event_refs`; implementations must
  not split one authoritative reconcile set across both fields
- when `primary_cause = mutationInterval`, reconcile/drift observations from the
  same close path belong in `supporting_evidence_event_refs`, not in an
  additional `observationSet`
- the `code.workspace_state_captured` event that establishes a transition must
  carry a `causality_refs.caused_by` that is byte-for-byte equivalent to
  `WorkspaceTransition.primary_cause`; implementations must not serialize a
  separate semantically-equivalent cause shape for the same authoritative
  transition
- `pre_workspace_state_id = None` is valid only for the first authoritative
  recorded state in a workspace stream and only when `primary_cause` is
  `genesisBootstrap` or `baselineCapture`
- every non-initial transition must set `pre_workspace_state_id = Some(...)`

`MutationInterval` owns timing and evidence boundaries. `WorkspaceTransition`
owns the authoritative `pre_workspace_state_id -> post_workspace_state_id`
change that later code facts, range queries, and repairs must reference.

Semantic classification belongs below the interval boundary:

```text
MutationObservation
- observation_id: String
- mutation_interval_id: String
- workspace_stream_id: String
- observation_kind
  - AuthoredEdit
  - RevisionTransition
  - FormatterRewrite
  - GeneratorOutput
  - OpaqueBulkRewrite
- touched_paths: Vec<String>
- source_file_version_ids: Vec<String>
- target_file_version_ids: Vec<String>
- evidence_refs: Vec<EventRef>
- reason_codes: Vec<String>
```

Rules:

- a `MutationInterval` may contain one or more `MutationObservation`s
- bulk semantic classification is observation-scoped, not interval-scoped; one
  shell command may legitimately emit authored edits, formatter rewrites, and
  generator output observations in the same interval
- projector jobs, hunk lineage, and repair semantics attach to
  `MutationObservation` granularity when a single interval contains mixed kinds
- every `MutationObservation` must be carried by exactly one hash-chained
  `code.mutation_observation_recorded` event in the same workspace custody
  stream as the owning transition

Code events must not require an owning mutation interval. They must use an
explicit cause:

```text
StateRef
- `pointInTime`
  - `workspace_state_id: String`
- `transition`
  - `pre_workspace_state_id: String`
  - `post_workspace_state_id: String`

CauseRef
- `executionActivity`
  - `cause_event_ref: EventRef`
  - `state_ref: StateRef`
- `mutationInterval`
  - `cause_event_ref: EventRef`
  - `state_ref: StateRef`
- `observationSet`
  - `observation_event_refs: Vec<EventRef>`
  - `state_ref: StateRef`
- `baselineCapture`
  - `cause_event_ref: EventRef`
  - `state_ref: StateRef`
- `genesisBootstrap`
  - `state_ref: StateRef`
- `unsupervisedCodexObservation`
  - `cause_event_ref: EventRef`
  - `state_ref: StateRef`
- `externalObservation`
  - `cause_event_ref: EventRef`
  - `state_ref: StateRef`
- `projectorJob`
  - `cause_event_ref: EventRef`
  - `state_ref: StateRef`
- `aliasResolution`
  - `cause_event_ref: EventRef`
  - `state_ref: StateRef`
- `systemRepair`
  - `cause_event_ref: EventRef`
  - `state_ref: StateRef`
```

`CauseRef` is a tagged union, not a bag of optional fields. Implementations must
serialize one concrete variant and must not emit semantically-equivalent
alternative shapes for the same cause.

Singular journaled causes carry exactly one `cause_event_ref`. Multi-event
reconcile causes use the dedicated `observationSet` variant and therefore carry
only ordered `observation_event_refs`. The only non-journaled cause is
`genesisBootstrap`.

Canonical cause targets are deterministic:

- `executionActivity` points to the activity's opening lifecycle event
- `mutationInterval` points to `execution.mutation_interval_closed`
- `observationSet` points to no single `cause_event_ref`; instead it carries the
  authoritative ordered `observation_event_refs`
- `unsupervisedCodexObservation` and `externalObservation` point to the
  corresponding recorded observation event
- `projectorJob` points to the replayable projector job event whose payload
  carries the referenced `ProjectionJobRecord`
- `baselineCapture` points to the journaled baseline-capture activity event and
  must use `state_ref = pointInTime`
- `aliasResolution` points to the alias-creation or alias-supersession event and
  must use `state_ref = pointInTime`
- `systemRepair` points to `system.repair_applied` and must use
  `state_ref = pointInTime`
- `genesisBootstrap` carries no `cause_event_ref` and must use
  `state_ref = pointInTime`

Point-in-time causes use `state_ref = pointInTime`. Causes that are inherently
about a mutation or projection transition must use
`state_ref = transition(pre_workspace_state_id, post_workspace_state_id)`. The
kernel must not collapse a two-state transition into one opaque
`workspace_state_id`.

For the first authoritative recorded state in a workspace stream, a
`code.workspace_state_captured` event may therefore pair
`pre_workspace_state_id = None` with a point-in-time cause such as
`genesisBootstrap` or `baselineCapture`. Implementations must not invent a fake
predecessor state solely to satisfy transition encoding.

Append validation must enforce an explicit event-family x cause-variant
compatibility matrix. At minimum:

- `code.workspace_state_captured` may use only `mutationInterval`,
  `observationSet`, `baselineCapture`, or `genesisBootstrap`
- `code.recorded_state_alias_created` and
  `code.recorded_state_alias_superseded` may use only `aliasResolution`
- repair-generated `code.hunk_observed`,
  `code.segment_projection_applied`, and `code.projection_job_repaired` may use
  only `projectorJob`; `systemRepair` is invalid for those code facts
- `systemRepair` is reserved for system-scoped repair evidence such as
  `system.repair_applied`; it must not become a second authoritative cause
  surface for repaired lineage facts

Examples:

- a tool write uses `mutationInterval`
- bootstrap seeding uses `genesisBootstrap` before a journaled baseline event
  exists, then `baselineCapture` once the baseline activity is recorded
- a known Codex-owned write path that bypassed exact ingress uses
  `unsupervisedCodexObservation`
- external drift uses `externalObservation`
- hunk projection uses `projectorJob`
- revision alias creation uses `aliasResolution`

#### Long-running process rules

Long-running process activity must not silently disappear at turn end.

- `exec_command` creates a `ProcessActivity` when the process can outlive the
  initial tool call
- exact child intervals for long-running processes require one of these
  attribution sources:
  - `StructuredWriteTap`
    - the write flowed through supervisor-owned file mutation callbacks bound to
      the current `process_activity_id`
  - `RuntimeWriteFence`
    - the supervised runtime emitted explicit begin/end write fences for the
      child interval and an external-writer exclusion guarantee covered the same
      fence window
- `FileWatchOnly`
  - is sufficient for drift detection and candidate path narrowing, but never
    for exact child-interval attribution in v1
- each `write_stdin` creates a child `MutationInterval` only when one of the
  exact attribution sources above is present; otherwise it creates process-level
  ambiguous drift evidence
- a `StructuredWriteTap` child interval is exact only for supervisor-owned write
  APIs that expose an explicit begin/end mutation scope; generic callback bursts
  from arbitrary processes are not sufficient
- a `RuntimeWriteFence` child interval opens only on an explicit runtime
  `beginWriteFence` signal and closes only on the matching `endWriteFence`
  signal; missing or overlapping fences invalidate exact attribution
- a `RuntimeWriteFence` without an external-writer exclusion guarantee may
  narrow drift to a bounded window, but it must not claim `certainty = Exact`
- process exit creates a final reconcile interval when needed
- background writes while a process is alive create child intervals only when
  they can be attributed safely
- writes that occur while the process is idle and outside a supervised child
  interval are recorded as drift and default to `certainty = Ambiguous`

`provenanceTurn/read` shows intervals attributable to the selected turn. A
separate `provenanceActivity/read` / `provenanceMutationInterval/read` surface exposes the full process
activity and its child intervals.

To preserve custody without serializing unrelated work for the lifetime of a
background process, the kernel enforces one active Codex writer lease per
`workspace_stream_id` and only for bounded mutation intervals:

- short-lived mutators hold the writer lease for the lifetime of their
  `MutationInterval`
- a `LongRunningMutator` keeps a durable `ProcessActivity`, but does not hold the
  workspace writer lease while idle
- each supervised child `MutationInterval` reacquires the writer lease, captures
  pre/post state, records evidence, and releases the lease
- no two Codex mutation intervals may hold the writer lease for the same
  workspace at the same time
- if a long-running process writes outside supervised child intervals, the
  affected state must be recorded as ambiguous or external drift before the next
  exact Codex interval is attributed
- `write_stdin` input alone does not prove an exact write window; exact child
  intervals require `StructuredWriteTap` or `RuntimeWriteFence`
- v1 may use cheap dirty markers and reconcile checks instead of a full
  filesystem watcher; without an exact attribution source, idle-process writes
  are never exact child-interval attribution, and `write_stdin`-adjacent writes
  collapse to process-level ambiguous drift
- if a command path cannot provide explicit mutation-scope boundaries, the
  kernel must keep exact custody at the process activity level only and publish
  child writes as ambiguous drift or coarse bulk mutation facts
- exact-child-interval tests must cover both positive and negative cases:
  `StructuredWriteTap` / `RuntimeWriteFence` writes resolve to exact child
  intervals, while writes observed only through dirty markers or file watching
  degrade to ambiguous drift
- if a process cannot be supervised safely, it should be explicitly detached;
  later writes are treated as external observations unless a new attributable
  interval is opened
- any supervised child interval that detects conflicting external drift during
  reconcile must downgrade affected paths to `certainty = Ambiguous` before
  projection or range attribution

External writers cannot be blocked and are recorded as external observations or
ambiguity.

#### Bulk mutation rules

Bulk mutations should not eagerly compute line-level projections inside the
interactive path.

Capture tiers:

- Tier 0 synchronous anchors
  - execution context, pre/post workspace cheap anchors, Git HEAD/index/worktree
    identity where available, and status dimensions
- Tier 1 synchronous bounded immutable capture
  - changed-path discovery, small changed blobs, descriptor records, and any
    manifest fragments or immutable evidence refs that later projection will be
    allowed to read, all below configured size/count limits
- Tier 2 asynchronous compute
  - hunk normalization, formatter/codegen mapping, expensive file continuity
    inference, and range projection over already captured immutable inputs

Synchronous path must stay bounded:

- capture pre/post workspace state anchors
- capture execution context
- capture file-change summary
- record immutable file-change evidence within configured limits when the active
  mutator already published exact artifacts through supervised write-through
  capture
- record the interval and its status
- if supervised exact-ingress coverage is absent, the close path must switch to
  a truth-first slow path that still captures an exact immutable source before
  minting the new authoritative workspace state
- budget pressure may delay close completion or move the mutator onto a slower
  exact capture path, but it must not be the reason that `path_replay_status`
  becomes `partial` or `unavailable`
- truth-first slow path is an accepted v1 operational cost for opaque or
  non-write-through mutators; implementations must surface close-path telemetry
  that distinguishes bounded close from slow exact capture, and operators must
  be able to configure alerting/SLA thresholds for slow-path frequency and age
- structured first-party mutators should converge toward zero steady-state
  slow-path usage; recurring slow-path fallback on those paths is an ingress
  coverage gap, not a reason to weaken exactness guarantees
- descriptor-only evidence is valid only when the exact immutable replay input
  for the affected content is genuinely unavailable, unreadable, redacted, or
  otherwise not capturable; it must not be used merely because exact capture was
  expensive
- use `indexing_status = Pending` only for deferred compute over immutable
  inputs that were already captured at interval close

Asynchronous path:

- compute hunks from immutable inputs
- update range projection
- create or update file continuity edges
- never perform new live-filesystem capture or retroactive blob capture

While async indexing is pending, the kernel must preserve chain-of-custody and
return explicit `indexing_status = Pending`. `Pending` must not mean "the
projector still needs to go read the live filesystem later."

Bulk operation semantics:

- these semantics apply per `MutationObservation`, not per whole interval
- `RevisionTransition`
  - records revision alias, workspace-state observation, and file/entity
    continuity edges; it must not generate Codex-authored hunk lineage for code
    that already existed in the target Git revision
- `FormatterRewrite`
  - records formatter activity and mechanical rewrite evidence; it should not be
    treated as authored logic unless a later projector can prove exact parent
    mapping
  - immutable evidence must include formatter identity, formatter version,
    effective config digest, and relevant invocation flags so later repair can
    distinguish true mechanical rewrites from environment drift
- `GeneratorOutput`
  - records generator activity, inputs, output evidence, and coverage limits
- `OpaqueBulkRewrite`
  - defaults to `coverage = Partial` or `certainty = Ambiguous` for exact range
    lineage until repaired

Projectors must never read the live filesystem as their source of truth. They
must read immutable artifacts captured at interval close.

```text
FileChangeEvidence
- path_before: Option<String>
- path_after: Option<String>
- change_kind
  - Create
  - Modify
  - Delete
  - Rename
  - ModeOnly
  - Unknown
- pre_file_version_ref: Option<String>
- post_file_version_ref: Option<String>
- pre_git_blob_oid: Option<String>
- post_git_blob_oid: Option<String>
- pre_blob_ref: Option<ExactArtifactRef>
- post_blob_ref: Option<ExactArtifactRef>
- tombstone_ref: Option<ExactArtifactRef>
- diff_basis
  - GitTree
  - GitWorkingTree
  - ContentBlob
  - ManifestOnly
- evidence_coverage
  - Complete
  - Partial
  - Unavailable
```

Create and delete operations must not invent fake file versions:

- `Create` has no `pre_file_version_ref`
- `Delete` has no `post_file_version_ref` and must include a tombstone or
  equivalent delete evidence when line-level reconstruction matters
- `Modify` and `ModeOnly` normally set both `path_before` and `path_after` to the
  same path
- `Create` sets only `path_after`
- `Delete` sets only `path_before`
- `Rename` must set both `path_before` and `path_after`; file-entity continuity
  is still proven through `FileEntityEdge`, but immutable evidence must carry the
  path mapping needed to replay the observation
- `ModeOnly` may have both file version refs but no line-level hunk

For Git-backed workspaces, prefer tree/blob OIDs. If Git cannot provide a
stable artifact for an untracked or generated file, store local
content-addressed blobs. For manifest-only evidence, line-level projection is
not available.

### 2. Ledger Contract

The kernel must write to a dedicated provenance store, not rely on rollout JSONL
or thread metadata tables.

Recommended split:

- `codex-provenance`
  - pure models
  - event schemas
  - projector logic
  - query result assembly
- `codex-provenance-store`
  - SQLite-backed provenance journal and query index
  - migrations
  - blob manifests
  - async projector jobs
- `codex-core`
  - capture plane integration
  - baseline orchestration
  - process lifecycle hooks
- `codex-app-server-protocol`
  - v2 provenance request and response types
- `codex-app-server`
  - provenance request routing and handlers

Dependency direction:

- `codex-provenance` owns canonical model types and hash schemas
- `codex-provenance-store` depends on `codex-provenance`
- `codex-app-server-protocol` may mirror wire DTOs but must not be depended on by
  the store
- `codex-app-server` maps protocol DTOs to provenance model types explicitly
- generated TypeScript/schema artifacts are outputs of the protocol crate, not
  inputs to the provenance store

Recommended local database:

- `provenance.db`, separate from the current rollout metadata database

This avoids coupling a replayable provenance journal to an existing thread-first
state model.

The provenance store owns:

- stream creation for execution, workspace custody, and access audit streams
- epoch rotation
- atomic sequence allocation
- event idempotency
- hash chaining
- blob descriptor and local locator records
- projector job state

Rollouts may remain useful for UI history, but they are not authoritative
provenance storage.

The ledger is the only replayable truth source. Projection tables, materialized
range indexes, blob locator rows, and query summaries are derived state. If a
state transition matters for custody, export, repair, or future replay, it must
be represented as a hash-chained `TraceKernelEvent`.

The trace ledger is scoped. A single workspace stream is not enough because
execution can be workspace-less or span multiple workspaces, and raw-content
reads must be auditable without mutating sealed workspace custody epochs.

Ledger scopes:

- `GlobalExecution`
  - session, thread, turn, item, tool, process, and platform-run execution facts
- `WorkspaceCustody`
  - workspace state, code, blob custody, projection, repair, stream, and epoch
    facts for one workspace stream
- `AccessAudit`
  - local raw-content and blob-read audit facts

Cross-scope relationships must use a complete event reference, not a bare event
id:

```text
EventRef
- ledger_scope: LedgerScope
  - GlobalExecution
  - WorkspaceCustody
  - AccessAudit
- stream_id: String
- stream_epoch: u64
- sequence: u64
- event_id: String
- event_hash: String
```

`stream_id` is the scope-local stream id. For `WorkspaceCustody` it is the
`workspace_stream_id`; for `GlobalExecution` it is the `execution_stream_id`; for
`AccessAudit` it is the `audit_stream_id`.

All ledger-facing contracts should use the same canonical identity tuple:

- `ledger_scope`
- `stream_id`
- `stream_epoch`
- `sequence`

Scope-specific fields such as `workspace_instance_id` are metadata, not part of
the canonical stream identity.

Cross-scope causality needs explicit commit semantics:

```text
AppendBatch
- batch_id
- writes
  - one or more `TraceKernelEvent` appends across one or more ledger scopes
- referenced_event_refs
- outcome
  - committed
  - aborted
  - repaired
```

`AppendBatch` is not only an internal transaction helper. It is a replayable
ledger fact whenever a batch crosses ledger scopes or contains same-batch
references. A batch that needs replayable semantics is called a
`ReplayableAppendBatch`.

Rules:

- an `EventRef` may target either an already durable historical event or another
  event in the same `AppendBatch`
- cross-scope batches and batches with same-batch refs must either commit every
  referenced append or record an explicit repair/abort fact; dangling causes are
  invalid
- append validation must verify the referenced `EventRef.event_hash`
- idempotent retry with the same `idempotency_key` and identical canonical
  content returns the existing `EventRef`
- retry with the same `idempotency_key` and different canonical content or
  previous hash is a conflict
- sequence allocation and hash materialization for same-batch references must be
  finalized before the batch becomes externally visible
- implementations may use one SQL transaction or a durable outbox/commit marker,
  but readers must never observe a partially committed batch as committed
- if a batch fails after reserving append state in any target stream epoch, the
  writer must record an explicit abort/repair outcome before later appends may
  reference the affected partial work
- sequence numbers remain gapless because reservation happens before sequence
  allocation; a reservation that never materializes a hash-chained event must
  not consume a stream-epoch sequence number
- every participant event in a `ReplayableAppendBatch` must include canonical
  batch metadata:
  - `append_batch_id`
  - `append_batch_index`
  - `append_batch_size`
- same-batch `EventRef`s may only reference events with a lower
  `append_batch_index`; append validation must reject same-batch cycles
- each `ReplayableAppendBatch` has exactly one global outcome kind:
  - `committed`
  - `aborted`
  - `repaired`
- every participating stream must expose exactly one stream-local outcome
  terminus after its last participant event and before any later non-batch
  event in that stream
- by default, the stream-local outcome terminus is the stream's
  matching `system.append_batch_*` marker for the batch outcome kind
- the only v1 exception is the old stream's `system.stream_sealed` in a
  writable stream-handoff batch; that terminal event serves as the old stream's
  committed stream-local outcome terminus and no additional
  `system.append_batch_committed` may appear after it in that stream
- once a stream has materialized participant events for a replayable batch,
  implementations must durably append that stream's stream-local outcome
  terminus before allowing later same-epoch events in that stream to become
  externally visible. A replayable batch may therefore stall a stream only at
  the head, not leave a permanently tentative hole in the middle of visible
  history
- head-of-line blocking at the stream head is an accepted consequence of
  checkpointable committed export in v1; implementations must surface the age of
  the oldest head-blocking replayable batch and enforce operator-configurable
  finalize/repair SLA thresholds rather than weakening export correctness
- outcome marker events are not counted in `append_batch_size`; they carry the
  `append_batch_id` in payload and may omit participant index fields
- batch outcome payloads carry `append_batch_id`, participating streams, ordered
  event refs for the whole batch when materialized, explicit per-stream outcome
  termini, and the outcome reason. Export collectors verify batch completeness by
  scanning forward in each participant stream until the recorded stream-local
  outcome terminus. A participant event with no visible stream-local outcome
  terminus is tentative, not committed.
- export APIs must withhold tentative participant events from checkpointable
  pages. Default checkpointable export may include a batch participant only when
  the stream-local outcome terminus is durable, the exported row identifies that
  terminus, and the batch outcome kind is `committed`.
- materialized participant events from `system.append_batch_aborted` batches are
  forensic/raw history only. They must not appear in default checkpointable
  export or be treated as committed recorder truth.
- materialized participant events from `system.append_batch_repaired` batches
  remain raw repair evidence. Default checkpointable export must surface the
  later authoritative replacement facts, not replay repaired participants as if
  they were committed truth.
- `system.append_batch_repaired` payloads must describe the raw repaired batch's
  own materialized participant set. They must not be overloaded to describe the
  later authoritative replacement facts emitted by projector or system repair.
  If the repaired batch also had reserved-but-unmaterialized append slots, the
  payload must include those `AppendReservationRef`s explicitly.
- every `WorkspaceTransition` close path is a required `ReplayableAppendBatch`
  boundary whenever it emits both execution-side interval facts and
  workspace-custody facts
- the batch for a closed `MutationInterval` must include all facts needed to
  make the transition replayable and non-dangling:
  - `execution.mutation_interval_closed`
  - the establishing workspace-custody event for the new
    `post_workspace_state_id`
  - any same-transition drift, unsupervised, or external observation facts that
    explain why the new state was minted
- a durable `execution.mutation_interval_closed` without its matching
  establishing workspace-custody transition fact, or vice versa, is invalid
- when a writable predecessor stream is rotated into a successor stream, the old
  stream's `system.stream_sealed`, the successor stream's epoch-opening event,
  and the successor stream's `system.stream_handoff_recorded` must commit in the
  same `ReplayableAppendBatch`

`AppendReservationRef` is the identity for reserved append slots that never
materialized as `TraceKernelEvent`s:

```text
AppendReservationRef
- ledger_scope: LedgerScope
- stream_id: String
- stream_epoch: u64
- idempotency_key: String
- reservation_id: String
```

Abort payloads must use `AppendReservationRef` for unmaterialized reservations.
They must not use `EventRef` unless the event row and event hash actually exist.
Repaired payloads follow the same rule for any reserved-but-unmaterialized
members that survived into the repaired-batch record.

`LedgerStreamRef` is the scope-local stream coordinate used in batch and export
payloads:

```text
LedgerStreamRef
- ledger_scope: LedgerScope
- stream_id: String
- stream_epoch: u64
```

`StreamLocalBatchOutcome` binds each participating stream to its exact replay
terminus for one batch:

```text
StreamLocalBatchOutcome
- stream_ref: LedgerStreamRef
- terminus_event_ref: EventRef
- terminus_kind
  - appendBatchCommitted
  - appendBatchAborted
  - appendBatchRepaired
  - streamSealed
```

`streamSealed` is valid only for the predecessor stream in a committed writable
handoff batch. All other participating streams must use the matching
`appendBatch*` terminus kind for the batch's global outcome.

`EventRef` resolution must be exact: `(ledger_scope, stream_id, stream_epoch,
sequence)`, `event_id`, and `event_hash` must all resolve to the same event.
Any coordinate/id/hash mismatch is invalid.

`TraceKernelEvent.event_type` is a stable mcodex kernel event name. Exporters may
map kernel names to Forgeloop canonical names, but they must not rewrite the
hash-chained kernel event or make the ledger depend on Forgeloop naming changes.
When a future Forgeloop name is already stable, mcodex may intentionally choose
the same string as its kernel event name.

### 3. State and Continuity Contract

The kernel needs stable ordering and explicit rotation semantics.

#### Identity types

- `repo_scope_id: Option<String>`
  - optional logical repo identity when a stable canonical remote can be proven
- `execution_stream_id: String`
  - local execution ledger stream for one mcodex installation or launched
    platform run boundary
- `workspace_instance_id: String`
  - physical local workspace instance identity
  - rotate on reclone, worktree replacement, root fingerprint change, or repo
    reinitialization
- `workspace_stream_id: String`
  - durable local journal stream identity for one workspace instance
  - must rotate whenever `workspace_instance_id` rotates
- `stream_epoch: u64`
  - rotate when the journal must start a new hash-chain era, such as repair,
    conflict resolution, or explicit reset
- `audit_stream_id: String`
  - local access-audit ledger stream for raw-content and blob reads
- `workspace_state_id: String`
  - immutable recorded state anchor
- `stream_head`
  - the latest known workspace state for a stream and epoch, with the exact
    workspace-custody event that established that state

#### Ordering rules

- ordering key is `(ledger_scope, stream_id, stream_epoch, sequence)`
- `sequence` is gapless within a single scope stream epoch
- the export contract must expose epoch descriptors so collectors can enumerate
  every epoch for a stream
- `event_id` identifies an event
- `idempotency_key` identifies a retry-safe append attempt
- `head_event_ref` identifies the current head event of a stream epoch when one
  exists
- `current_workspace_state_event_ref` identifies the exact workspace-custody
  event that established `current_workspace_state_id`

The spec must not leave these rotations implicit. Identity mistakes here break
chain-of-custody.

When `workspace_instance_id` rotates and the old workspace custody stream is
still writable, the kernel must:

- append a hash-chained `system.stream_sealed` terminal event to the old stream
  and mark the old stream closed only after that append commits
- mint a new `workspace_stream_id`
- append the old stream's seal, the successor epoch-opening event, and the new
  stream's `system.stream_handoff_recorded` in one `ReplayableAppendBatch`
- append a hash-chained `system.stream_handoff_recorded` event in the new stream
  pointing to the previous stream and its terminal seal event hash
- require future queries and exports to treat the new stream as a distinct local
  custody chain

The old stream closure must be verifiable from the old stream alone. The new
stream handoff is a forward link, not a substitute for the old stream's terminal
fact.

If a local failure is discovered after reserving a writable-rotation batch but
before the paired handoff is externally visible, the successor stream must not
accept code/blob custody facts until a later committed recovery batch appends
an authoritative `system.stream_handoff_recorded`. If the predecessor stream is
still writable at recovery time, that handoff must bind the successor to a
newly appended predecessor seal; recovery must not downgrade to a
last-trusted-event handoff while sealing is still possible. Any accompanying
`system.append_batch_repaired` or `system.repair_applied` facts are diagnostic
only; they must not become a second authority source for predecessor linkage.

If rotation is discovered after the old workspace stream is unavailable or
unwritable, the kernel must not claim the old stream was sealed. The new stream
still records `system.stream_handoff_recorded` with an `UnknownClosure` or
`Abandoned` previous-stream status, and consumers treat the old stream as
unsealed at its last verified event. This is a distinct `unsealedPredecessorRecovery`
handoff shape, not an alternative encoding of the sealed path. The handoff
payload must include that last trusted predecessor `EventRef` when one is
known. `system.repair_applied` may accompany the handoff as supplemental repair
evidence, but it must not replace the handoff record that proves predecessor
linkage. Any repair fact used during this path must name both predecessor and
successor streams, repeat the last trusted predecessor event when known, and
reference the successor handoff event so replay/export consumers can verify
that the repair supplemented rather than replaced the handoff record.

If a workspace custody stream has a predecessor, the new stream's epoch opening
event uses `UnknownPrevious(reason = streamHandoff)` and must be followed by
`system.stream_handoff_recorded` before any code/blob custody facts are
appended. Epoch continuity describes previous epochs within the same scoped
stream; stream handoff describes previous streams. The two facts must not be
folded into one payload.

When `stream_epoch` rotates, the new epoch descriptor must expose one of these
continuity states:

- `VerifiedPrevious`
  - carries previous epoch, previous epoch last sequence, and previous epoch last
    event hash
- `BrokenChain`
  - carries the detected break reason, repair event id, and the last trusted
    sequence/hash when known
- `UnknownPrevious`
  - allowed only for bootstrap, import, explicit local reset, or the first epoch
    of a successor stream created by stream handoff

The kernel must never fabricate a predecessor hash for repair or conflict
resolution. Broken continuity is itself a hash-chained system fact.

Continuity authority must not be split across multiple event families:

- `system.epoch_started` / `system.epoch_rotated` are the only authoritative
  continuity-opening facts for an epoch
- when `continuity_state = BrokenChain`, the new epoch's
  `system.epoch_rotated` event is the authoritative statement of the break that
  verifiers, descriptors, and exporters must follow
- `system.chain_broken_recorded` is optional diagnostic evidence about when or
  why the break was detected; it must not override or compete with the opening
  continuity event
- if `system.chain_broken_recorded` is emitted, it must refer to the same break
  described by the authoritative opening event and must not appear without a
  corresponding broken-chain epoch opening fact

#### Hash-chain rules

- for a single append, sequence allocation and event row durability happen in one
  DB transaction; cross-scope visibility follows the `AppendBatch` rules above
- `(ledger_scope, stream_id, stream_epoch, sequence)` is unique
- `event_id` is unique
- `idempotency_key` is unique within the scope stream epoch
- first event in an epoch uses an explicit genesis previous hash constant
- the canonical genesis previous hash is the 64-hex zero value
  `0000000000000000000000000000000000000000000000000000000000000000`
- every epoch in every `LedgerScope` must begin with a continuity opening event
- genesis epochs use `system.epoch_started` with an explicit genesis/unknown
  previous state and the canonical genesis previous hash
- non-genesis epochs use `system.epoch_rotated` as the continuity opening event
  for all ledger scopes
- `system.epoch_rotated` belongs to the new epoch, uses the epoch genesis
  previous hash, and its payload carries the continuity union for the previous
  epoch
- event hashes are computed over canonical serialization of the event envelope
  with `event_hash` omitted, `previous_event_hash` included, and only canonical
  fields included

Canonical hash input must be implementation-independent:

- digest algorithm is `sha256`
- serialization is RFC 8785 JSON Canonicalization Scheme over UTF-8 bytes
- wire/export field names use camelCase after serde `rename_all = "camelCase"`
- enum values use the exact string spellings defined in this spec
- timestamps are integer Unix seconds
- canonical ledger/export hash input omits absent optional fields; explicit JSON
  `null` is hash input only for DTO fields whose canonical schema declares a
  nullable value
- app-server response DTO serialization is not the canonical hash algorithm.
  Response `Option` fields must serialize as explicit JSON `null`; omission is
  reserved for optional client-to-server request fields and the v2 no-params
  exception
- storage-local locator fields, delivery chunks, process-local paths, and debug
  metadata are never part of the canonical hash input
- event DTOs must split `canonical_payload` from `local_metadata`; excluded fields
  may appear only in `local_metadata`
- `ledger_context.scope_metadata` is excluded from the canonical hash input and
  must be treated as local metadata even when surfaced in wire DTOs
- metadata-only `ExecutionContext` fields are excluded from the canonical hash
  input:
  - `client_process_id`
  - `client_connection_id`
- unless a DTO explicitly opts out, canonical wire/hash enum values use
  lower-camel-case strings; Rust variant names may differ

#### Execution Fact Payloads

The execution journal must be first-class. Do not force future systems to infer
execution spans from code facts.

Required event families:

- `execution.session_started`
- `execution.session_finished`
- `execution.thread_started`
- `execution.thread_finished`
- `execution.turn_started`
- `execution.turn_finished`
- `execution.item_recorded`
- `execution.tool_call_started`
- `execution.tool_call_finished`
- `execution.process_started`
- `execution.process_finished`
- `execution.activity_started`
- `execution.activity_finished`
- `execution.mutation_interval_opened`
- `execution.mutation_interval_closed`

Execution facts must carry typed context when available:

```text
ExecutionContext
- execution_origin
  - InteractiveUser
  - PlatformAutomation
  - Unknown
- session_id: Option<String>
- thread_id: Option<String>
- turn_id: Option<String>
- item_id: Option<String>
- tool_call_id: Option<String>
- process_activity_id: Option<String>
- client_process_id: Option<String>
- client_connection_id: Option<String>
- activity_id: Option<String>
- spec_revision_id: Option<String>
- plan_revision_id: Option<String>
- execution_package_id: Option<String>
- run_session_id: Option<String>
- launcher_actor_ref: Option<String>
- launcher_actor_kind: Option<String>
- launcher_context_refs: Vec<String>
```

```text
CausalityRefs
- caused_by: Option<CauseRef>
- parent_activity_event_ref: Option<EventRef>
- follows_activity_event_ref: Option<EventRef>
- supersedes_event_ref: Option<EventRef>
```

Minimum rules:

- launcher-provided actor or attribution refs must survive export without
  lossy flattening
- `execution_origin` is mandatory even when all other launcher refs are absent
- interactive Codex work should populate `session_id` and `thread_id`; platform
  automation may instead populate `execution_package_id`, `run_session_id`, and
  launcher refs
- process-backed activity must carry both `tool_call_id` and the kernel-minted
  `process_activity_id`
- app-server or client-supplied process identifiers, such as
  `(connection_id, process_id)`, are metadata (`client_connection_id` and
  `client_process_id`), not canonical provenance identity
- causality edges live in `CausalityRefs`; `ExecutionContext` identifies the
  current execution scope but must not duplicate ancestry
- a child mutation interval created by `write_stdin` carries the current
  interaction `tool_call_id`, the original `process_activity_id`, optional
  client process metadata, and a `parent_activity_event_ref` pointing at the
  process activity event
- canonical lifecycle refs are deterministic:
  - `parent_activity_event_ref` and `follows_activity_event_ref` must point to
    the opening lifecycle event for the referenced activity span
  - `mutationInterval` causes must point to the
    `execution.mutation_interval_closed` event for that interval
- append validation must reject execution events whose current span fields and
  causality refs contradict each other
- app-server DTOs should expose `ExecutionContext` directly instead of forcing
  clients to reverse-engineer it from free-form metadata

Opaque ref bags are allowed as extensions, but they are not sufficient as the
primary execution contract.

Scope/category compatibility must be explicit:

- `GlobalExecution`
  - allowed categories: `Execution`, `System`
- `WorkspaceCustody`
  - allowed categories: `Code`, `Blob`, `System`
- `AccessAudit`
  - allowed categories: `AccessAudit`, `System`

Append validation must reject category/scope combinations outside this matrix.

#### Code Fact Payloads

Code provenance facts should be linked to execution facts through explicit
causality.

Required code event families:

- `code.workspace_state_captured`
- `code.mutation_observation_recorded`
- `code.bootstrap_seeded`
- `code.unsupervised_workspace_mutation_observed`
- `code.external_workspace_mutation_observed`
- `code.file_version_observed`
- `code.file_entity_edge_recorded`
- `code.projection_job_started`
- `code.projection_job_finished`
- `code.projection_job_repaired`
- `code.hunk_observed`
- `code.segment_projection_applied`
- `code.recorded_state_alias_created`
- `code.recorded_state_alias_superseded`

Every code event references:

- a mandatory `CauseRef`
- `CausalityRefs` when ancestry or supersession exists
- the execution event that caused it, when available
- the relevant workspace state anchor

Append validation must reject any `TraceKernelEvent` with `category = code` and
no `causality_refs.caused_by`. `genesisBootstrap` is the only non-journaled code
cause allowed, and it must be represented as an explicit `CauseRef` variant.

The authoritative custody-transition carrier is fixed:

- `code.workspace_state_captured` is the only event family that may establish a
  new authoritative `WorkspaceTransition`
- its typed payload must carry both the `WorkspaceTransition` and the captured
  `WorkspaceStateRecord`
- bootstrap or baseline seeding may emit `code.bootstrap_seeded`, but any
  resulting recorded workspace state is still established by
  `code.workspace_state_captured`
- `code.unsupervised_workspace_mutation_observed` and
  `code.external_workspace_mutation_observed` may explain a reconcile
  transition, but they are not themselves the state-establishing event

Every required code event family must have a versioned typed payload DTO before
implementation. At minimum, define payloads for:

- `code.workspace_state_captured`
- `code.mutation_observation_recorded`
- `code.bootstrap_seeded`
- `code.unsupervised_workspace_mutation_observed`
- `code.external_workspace_mutation_observed`
- `code.file_version_observed`
- `code.file_entity_edge_recorded`
- `code.projection_job_started`
- `code.projection_job_finished`
- `code.projection_job_repaired`
- `code.hunk_observed`
- `code.segment_projection_applied`
- `code.recorded_state_alias_created`
- `code.recorded_state_alias_superseded`

#### Blob and System Payloads

Blob and system custody transitions must also be hash-chained facts, not only
derived local table state.

Required blob event families:

- `blob.descriptor_recorded`
- `blob.availability_changed`

Required access audit event families:

- `access.blob_read_authorized`
- `access.blob_read_delivered`
- `access.raw_content_read_authorized`
- `access.raw_content_read_delivered`

Required system event families:

- `system.epoch_started`
- `system.epoch_rotated`
- `system.append_batch_committed`
- `system.append_batch_aborted`
- `system.append_batch_repaired`
- `system.stream_sealed`
- `system.stream_handoff_recorded`
- `system.stream_claim_recorded`
- `system.stream_registration_recorded`
- `system.repair_applied`

Optional supplemental diagnostic system event families:

- `system.chain_broken_recorded`

Rules:

- every blob availability transition must correspond to a replayable blob or
  system event in the journal
- redaction, checksum mismatch, expiry, and missing-content detection must not
  be represented only as mutable row updates
- export consumers must be able to replay blob custody from `TraceKernelEvent`
  rows alone
- blob reads do not append to sealed workspace custody epochs; read auditing is
  written to the `AccessAudit` ledger and references the target blob/custody
  event with `EventRef`

#### Workspace State Records

The kernel does not reason directly over "the current filesystem." It reasons
over recorded workspace states.

#### Workspace state classes

- `ExactGitState`
  - Git tree backed
  - `git_tree_oid` is the authoritative replayable manifest source
- `CheckpointState`
  - blob-backed snapshot state
- `DerivedState`
  - parent state plus exact deltas
- `ManifestState`
  - coarse file-level manifest only

Rules:

- Git-backed line-level provenance is built only from `ExactGitState`,
  `CheckpointState`, and `DerivedState`
- `ManifestState` is not an exact range-queryable state
- non-Git workspaces may produce `ManifestState` for coarse attribution, but not
  exact line-level provenance in v1
- reading Git objects addressed by recorded `git_tree_oid` / `git_blob_oid` is
  replay over immutable artifacts, not live-filesystem capture
- a large `git checkout` / `git switch` that lands on an `ExactGitState` does
  not need to inline a full path map during the close path; the recorded Git
  tree is itself the exact post-state manifest source

#### Exact artifact store

All immutable non-event content required for exact replay belongs to one
content-addressed `ExactArtifactStore`.

```text
ExactArtifactRef
- artifact_id: String
- artifact_kind
  - promotedGitObject
  - stateManifest
  - pathDelta
  - filesystemDeltaSnapshot
  - filesystemSnapshot
  - workingTreeBlob
  - tombstoneBlob
  - other
- content_digest: String
- storage_class
  - localHot
  - localDurable
  - exportedBundle
  - coldArchived
- retention_class
  - ephemeral
  - sessionBound
  - stateBound
  - exportBound
  - archival
```

It covers:

- promoted unreachable Git trees and blobs
- manifest checkpoints and path deltas
- filesystem delta and full snapshot artifacts
- immutable working-tree content captured in `blob_ref`, `pre_blob_ref`,
  `post_blob_ref`, and `tombstone_ref`
- any other immutable content artifact needed by exact workspace replay,
  projector input, or blob-read continuation

Rules:

- exact artifact refs are immutable and globally deduplicated by content or
  object identity as appropriate
- `ExactArtifactRef` is the canonical typed handle for exact replay artifacts;
  workspace states, file versions, blob locators, and file-change evidence must
  use it instead of untyped string locators
- retained projector inputs, persisted exports, active blob-read sessions, and
  explicit archival policies are retention roots for exact artifacts
- ordinary exact workspace states and file versions are retention roots only
  while they remain inside the implementation's configured local retention
  window or another stronger retention root still depends on their artifacts
- retention roots determine artifact lifetime; `retention_class` communicates
  why an artifact is pinned, while the store remains globally deduplicated
- the local retention window and archive migration policy must be explicit
  operator-visible configuration; implementations must not silently treat local
  exact retention as either unbounded forever or zero-length best effort
- implementations must support tiered retention:
  - local hot / local durable retention for ordinary recent states and file
    versions
  - export-bound or archival retention for long-lived externally persisted
    replay material
  - cold or archive migration for artifacts that must remain logically
    replayable after leaving the local hot window
- if exact replay is still promised after an artifact leaves local hot/durable
  storage, the implementation must retain that artifact inside
  `ExactArtifactStore` with `storage_class = coldArchived`; archive migration
  may move bytes to colder backing storage, but it must not introduce a second
  untyped locator model or silently break the replay contract when the local
  retention window expires
- hot indexes, tree-entry caches, and other derived acceleration structures are
  not retention roots and may be evicted without weakening exact replay
- supervised exact-ingress paths should publish replay artifacts into
  `ExactArtifactStore` incrementally during the mutation interval so the close
  path can seal refs and manifests instead of re-reading already captured bytes
- `filesystemDeltaSnapshotState` is the fallback when exact write-through
  capture is unavailable or when an opaque write path still needs an immutable
  sparse snapshot at close time; it is not the preferred capture mode for
  structured first-party mutators

#### Baselines

Add a provenance-specific baseline service.

- it may reuse ghost commit creation internally when useful
- it must not inherit ghost snapshot's undo semantics
- it must support non-Git coarse baseline capture separately
- it must produce a `workspace_state_id` suitable for the provenance store

Bootstrap prehistory should remain bounded:

- tracked text files first
- ignore rules and size limits respected
- incremental seeding allowed
- unseeded files remain `coverage = Partial`

#### File Identity Records

The kernel must distinguish "a file state version exists" from "continuity of
the same file entity is proven."

#### Required identity objects

- `file_version_id`
  - mandatory for every observed file state
- `file_entity_id: Option<String>`
  - present only when continuity is proven
- `file_entity_edge`
  - `Preserved`
  - `Renamed`
  - `Recreated`
  - `Ambiguous`

Implications:

- `HunkRecord` and range projection should attach to `file_version_id`
- `file_entity_id` is optional across ambiguous boundaries
- any query that must cross an ambiguous file-entity boundary returns
  `certainty = Ambiguous`

This avoids forcing a fake `file_id` where the recorder cannot actually prove
continuity.

`file_entity_edge` must be a directed relation:

```text
FileEntityEdge
- edge_id
- edge_kind
  - Preserved
  - Renamed
  - Recreated
  - Ambiguous
- from_file_version_id: Option<String>
- to_file_version_id: String
- mutation_interval_id: Option<String>
- cause_ref
- path_before: Option<String>
- path_after: Option<String>
- evidence_refs
```

Copy, split, merge, and unsupported continuity cases default to `Ambiguous`
in the file-entity graph in v1. Exact copy/split/merge provenance is expressed
only through hunk-level parent edges with explicit source/target file-version
dependencies; it must not be promoted into file-level continuity until a future
file-entity edge model adds explicit fan-out/fan-in cardinality.

Range lookup requires a replayable path-to-file-version map. The following
records are canonical payload content, not only index tables:

```text
WorkspaceStateRecord
- workspace_state_id: String
- state_kind: WorkspaceStateKind
  - exactGitState
  - filesystemDeltaSnapshotState
  - filesystemSnapshotState
  - checkpointState
  - derivedState
  - manifestState
- workspace_stream_id: String
- workspace_instance_id: String
- repo_scope_id: Option<String>
- git_commit_oid: Option<String>
- git_tree_oid: Option<String>
- git_tree_durability
  - reachable
  - storedUnreachable
  - hashOnly
  - notGitBacked
- parent_workspace_state_ids: Vec<String>
- path_replay_parent_workspace_state_id: Option<String>
- path_map_digest: Option<String>
- path_replay_status
  - exact
  - partial
  - unavailable
- state_manifest_ref: Option<ExactArtifactRef>
- filesystem_delta_snapshot_ref: Option<ExactArtifactRef>
- filesystem_snapshot_ref: Option<ExactArtifactRef>
- path_delta_ref: Option<ExactArtifactRef>
- changed_path_entries: Vec<WorkspacePathEntry>
- checkpoint_kind
  - none
  - fullPathMap
  - compactedPathMap
```

`WorkspaceStateRecord` is delta-first. Ordinary mutation intervals must not put
the full repository path map into the synchronous canonical payload. They record
changed paths and immutable refs only. Full path maps are allowed only for
bootstrap, explicit checkpoint, compaction, import, or small repositories under
the configured capture budget.

`ExactGitState` is the exception for repository-wide manifest replay. For an
exact Git-backed post-state, the kernel may rely on recorded `git_tree_oid` as
the authoritative path map source and materialize path entries lazily or
asynchronously without degrading exactness.

`filesystemDeltaSnapshotState` is the non-Git runtime exact fallback for
over-budget local rewrites. It must point to an immutable sparse snapshot
artifact captured at close time for exactly the changed path set relative to the
nominated `path_replay_parent_workspace_state_id`. Later path replay and
file-version lookup must use that sparse immutable artifact plus the nominated
parent state; they must not reread the live filesystem to recover truth.

`filesystem_delta_snapshot_ref` is required when
`state_kind = filesystemDeltaSnapshotState` and must resolve to an immutable
sparse snapshot artifact that is sufficient, together with the nominated parent
exact state, to reconstruct exact path membership and file-version inputs for
the captured path set.

`filesystemSnapshotState` is reserved for bootstrap, import, or explicit full
local checkpoint capture. It must point to an immutable full-workspace snapshot
artifact and must not be used as the ordinary runtime overflow fallback merely
because bounded delta or manifest emission exceeded the close-path budget.

`filesystem_snapshot_ref` is required when `state_kind = filesystemSnapshotState`
and must resolve to an immutable full-workspace artifact that is sufficient to
reconstruct path membership and exact file-version inputs for that state.

If a state has multiple logical parents, it must still nominate exactly one
`path_replay_parent_workspace_state_id` for deterministic path replay. Any
additional ancestry is explanatory metadata only unless the state is promoted to
`checkpointState` or `manifestState` with a complete manifest snapshot.

```text
WorkspacePathEntry
- path
- file_version_id: Option<String>
- entry_kind
  - file
  - directory
  - symlink
  - submodule
  - tombstone
- file_mode: Option<String>
- line_count: Option<u32>
- line_index_digest: Option<String>
```

`storedUnreachable` means the referenced Git object has been copied into the
provenance-managed `ExactArtifactStore` so exact replay does not depend on the
repository continuing to retain that object.

```text
FileVersionRecord
- file_version_id
- path
- file_kind
  - text
  - binary
  - symlink
  - submodule
  - tombstone
- file_mode: Option<String>
- git_blob_oid: Option<String>
- git_object_durability
  - reachable
  - storedUnreachable
  - hashOnly
  - notGitBacked
- blob_ref: Option<ExactArtifactRef>
- byte_size: Option<u64>
- line_count: Option<u32>
- line_index_digest: Option<String>
```

Rules:

- `(workspace_state_id, path)` resolves by applying the nearest trusted
  checkpoint `state_manifest_ref` plus ordered `path_delta_ref` /
  `changed_path_entries` following `path_replay_parent_workspace_state_id`
  before any hunk or segment projection is queried
- for `filesystemDeltaSnapshotState`, `(workspace_state_id, path)` resolves by
  consulting `filesystem_delta_snapshot_ref` for the captured path set and
  falling back to the nominated `path_replay_parent_workspace_state_id` for
  all other paths
- for `ExactGitState`, `(workspace_state_id, path)` may resolve directly against
  recorded `git_tree_oid` and tree-walk-derived path entries; that lookup is
  exact even if no inline `state_manifest_ref` or `changed_path_entries` were
  stored for the whole tree
- `ExactGitState` may claim `path_replay_status = exact` only when
  `git_tree_durability` is `reachable` or `storedUnreachable`
- `parent_workspace_state_ids` may contain more than one entry, but
  deterministic path replay in v1 must follow only
  `path_replay_parent_workspace_state_id`
- if the checkpoint/delta chain needed for a path cannot be replayed, the query
  returns `recorder_status.coverage = Unavailable` for that path
- `path_map_digest` is present only when `path_replay_status = exact` or when a
  complete checkpoint/manifest digest is available. It must be absent for
  `partial` or `unavailable` states that do not have a complete replayable path
  map.
- synchronous capture may record only changed path entries; asynchronous workers
  may materialize checkpoint manifests from already-captured immutable refs, but
  must not read the live filesystem to fill gaps retroactively
- path replay status and indexing status are orthogonal:
  - `path_replay_status = exact` means an exact immutable source exists for path
    membership and file-version lookup
  - `indexing_status = Pending` means that exact path replay or segment
    materialization may succeed by reading already-captured immutable sources
  - `indexing_status = Blocked` means a predecessor projection or repair must
    complete before exact replay can proceed
  - `coverage = Unavailable` means no exact replayable immutable source exists
- close-path budget pressure must not be the reason truth becomes inexact
- if the changed-path delta exceeds the synchronous capture budget, the state
  must still close with an exact immutable truth source by doing one of:
  - externalizing the delta into `path_delta_ref` backed by immutable blob
    chunks, with `changed_path_entries` containing only a bounded inline prefix
  - publishing a `checkpointState` / `manifestState` with a manifest snapshot
  - capturing a `filesystemDeltaSnapshotState` backed by an immutable sparse
    snapshot artifact when the post-state is not representable as exact Git
    state and a bounded delta/manifest cannot be emitted in-budget
- `path_replay_status = partial` or `unavailable` is valid only when the
  immutable replay input itself is missing, unreadable, redacted, or otherwise
  not exact. It must not be used merely because synchronous indexing or query
  acceleration would exceed the close-path budget.
- `filesystemDeltaSnapshotState` artifacts must be path-scoped, chunked, and
  deduplicated against already-captured immutable content where practical. The
  ordinary runtime overflow path must not silently escalate into a repo-wide
  full snapshot capture.
- `checkpointCompaction` is a query-acceleration / checkpoint-promotion job, not
  a truth-recovery job. It may compact or materialize faster exact lookup state
  from already-captured immutable truth, but it must not be the first place
  where exact custody truth becomes available.
- a Git-backed `RevisionTransition` to an `ExactGitState` must not degrade to
  `partial` or `unavailable` only because the repository-wide path map was too
  large to inline. Degrade only when the recorded `git_tree_oid` / required Git
  objects are themselves unavailable as immutable replay inputs.
- a `partial` or `unavailable` path replay transition is not a negative
  membership proof. Any path lookup that crosses that transition must return
  `coverage = Unavailable` unless the specific path is covered by an exact
  immutable delta entry or a later exact checkpoint.
- uncommitted or untracked content must use `blob_ref` unless the Git object is
  proven stored and readable later
- if an exact Git-backed post-state would otherwise depend on `git_tree_oid` or
  `git_blob_oid` objects that are only `hashOnly`, capture must either promote
  those objects to durable local storage and record `storedUnreachable` or
  downgrade the state away from exact replay
- `git_blob_oid` with `git_object_durability = hashOnly` is not an immutable
  artifact for projector input
- `storedUnreachable` Git trees/blobs live in `ExactArtifactStore`, which must
  be content-addressed and deduplicated by object identity and bytes; promotion
  must not create one retained copy per workspace state or per file version
- projector inputs, persisted export bundles, active blob-read sessions, and
  any exact workspace state or file version that still lies inside the
  configured local retention window hold the retention references for promoted
  content
- once an ordinary state/file-version root ages out of the local retention
  window, implementations may release its local exact artifacts only if another
  surviving retention root or cold/archive tier still preserves exact replay
- an exact artifact may be garbage-collected only after no retention root still
  references it and any configured grace period has elapsed
- local hot caches may evict derived tree-entry indexes without affecting
  `ExactArtifactStore`'s exact replay guarantee
- `FileChangeEvidence` that points to working-tree content must include
  `pre_blob_ref` or `post_blob_ref` unless the referenced Git object durability
  is `reachable` or `storedUnreachable`
- exact path lookup must not require unbounded delta replay. Implementations
  must capture a new exact checkpoint or compacted manifest before configured
  delta depth / replay-cost thresholds are exceeded for a workspace stream

### 4. Projection Contract

The stored line-level truth is hunk-based, but projected into range segments.

Rules:

- normalized hunks describe edited regions only
- diff context may be stored as matching aid or debug evidence, but never as
  authored lineage
- range projection runs only on exact line-level states
- projection rows are versioned by workspace state validity

Recommended projection record:

```text
ProjectedSegment
- segment_id
- lineage_revision_id
- workspace_instance_id
- workspace_state_id
- file_version_id
- line_start
- line_end
- terminal_hunk_id
- valid_from_projection_version
- valid_to_projection_version: Option<i64>
- range_fingerprint
```

`projection_version` is the monotonically increasing local materialization
version for one `(workspace_state_id, file_version_id)` pair. It is a query
optimization, not the authoritative repair contract. Rebuild or repair of the
same pair may emit a higher `projection_version`, but authoritative lineage
selection must follow append-only lineage revision supersession, not mutable
projection rows alone. Queries select the highest ready non-superseded lineage
revision for that pair.

Historical range queries resolve against the rows valid for the selected exact
recorded workspace state.

Projection jobs form a DAG:

- every projection job declares its input workspace state, output workspace
  state, touched file versions, affected file entities or path ancestry, and
  dependent job ids
- a projection job must be scoped to one file version or one tightly-coupled
  file-entity/path-ancestry set. Independent files from a bulk mutation must be
  split into separate jobs so unrelated paths do not share a blocking unit
- a job may read only immutable `FileChangeEvidence`, workspace-state records,
  and projection outputs for the same file/entity/path ancestry
- dependency ordering is file/entity scoped, not workspace-global; pending work
  for one path must not block unrelated file versions
- copy/split/merge lineage uses explicit source and target `file_version_id`
  dependencies recorded on the projection job; it does not require proving a
  stable `file_entity_id` across that boundary
- if the projector cannot enumerate those source/target file-version
  dependencies exactly, it must downgrade copy/split/merge parent edges to
  `mapping_kind = ambiguous` instead of inventing a file-entity ancestry key
- if an affected predecessor projection is pending, only dependent candidates for
  that file/entity remain `indexing_status = Pending`
- if an affected predecessor projection failed or only has manifest-level
  evidence, dependent jobs either remain blocked for repair or publish
  `coverage = Partial` with reason codes
- a later mutation may still be captured synchronously while earlier projection
  is pending, but hunk parentage for the affected file/entity must not be guessed
  until the predecessor projection is ready
- every emitted hunk fact must mint a new `hunk_id`; repair must not reuse a
  prior revision's `hunk_id`, even when the logical hunk content is unchanged
- if implementations need a stable logical-equivalence handle across revisions,
  they must model that separately from `hunk_id`

This separates custody capture from lineage projection: the ledger can continue
to record facts, while exact range lineage waits for the projection DAG to catch
up.

Projection jobs are replayable code facts, not only local queue rows:

```text
ProjectionJobRecord
- projection_job_id: String
- lineage_revision_id: String
- job_kind
  - hunkNormalization
  - segmentProjection
  - checkpointCompaction
  - formatterMapping
  - generatorMapping
  - repair
- input_workspace_state_id: String
- output_workspace_state_id: String
- touched_file_version_ids: Vec<String>
- source_file_version_ids: Vec<String>
- target_file_version_ids: Vec<String>
- dependent_projection_job_refs: Vec<EventRef>
- superseded_lineage_revision_ids: Vec<String>
- status
  - started
  - finished
  - blocked
  - failed
  - repaired
  - superseded
- reason_codes: Vec<String>
```

Operational queueing is not authoritative truth:

- for one effective target workspace state, implementations may coalesce
  multiple queued `checkpointCompaction` jobs into one runnable job
- a newly enqueued queued-but-not-started compaction or repair job may
  supersede an older queued job for the same effective target
  (`output_workspace_state_id` for checkpoint compaction,
  `(output_workspace_state_id, touched_file_version_ids)` for file-scoped
  repair/projection) when the newer job would compute an equal-or-better
  authoritative result from a strictly newer exact input set
- a started compaction or repair job may transition to `status = superseded` at
  an implementation-defined safe interruption boundary if a strictly newer exact
  input set now dominates its target and continuing the older job would no
  longer improve authoritative output
- hot query indexes may retain only the latest effective ready revision plus a
  bounded recent failure/staleness window for scheduling and diagnostics
- coalescing, pruning, or queue supersession must never delete or rewrite the
  replayable ledger events that established authoritative truth

`projectorJob` causes must reference `code.projection_job_started`,
`code.projection_job_finished`, or `code.projection_job_repaired` events whose
payload includes `ProjectionJobRecord`. `code.hunk_observed` and
`code.segment_projection_applied` must not use `projectorJob` as a cause unless
the referenced projector job event is already hash-chained.

Repair and supersession are append-only:

- a repair that changes authoritative lineage for a
  `(workspace_state_id, file_version_id)` pair must emit a new
  `lineage_revision_id`
- repair-generated `code.hunk_observed` and `code.segment_projection_applied`
  events that replace prior authoritative facts must carry
  `causality_refs.supersedes_event_ref`
- revision-level supersession authority lives only in the
  `code.projection_job_repaired` payload's
  `ProjectionJobRecord.superseded_lineage_revision_ids`
- `causality_refs.supersedes_event_ref` identifies the specific emitted facts
  replaced by that authoritative revision; it is event-level evidence, not a
  competing revision-level authority source
- if a repaired revision's emitted `supersedes_event_ref` evidence is
  incomplete or conflicts with the revision-level supersession set, replay must
  reject that repaired revision as invalid rather than inventing a tie-breaker
- `system.repair_applied.superseded_event_refs` may mirror affected facts for
  cross-stream/system diagnostics, but it must not override lineage authority
  established by `code.projection_job_repaired`
- replay from `TraceKernelEvent`s alone must be sufficient to determine the
  latest authoritative lineage revision; local projection tables may accelerate
  lookup but must not be the only source of truth

`HunkRecord` is a graph node, not just terminal attribution:

```text
HunkRecord
- hunk_id
- lineage_revision_id: String
- mutation_interval_id: Option<String>
- cause_ref
- input_file_version_id: Option<String>
- output_file_version_id: Option<String>
- input_range: Option<PatchLineSpan>
- output_range: Option<PatchLineSpan>
- operation
  - Add
  - Replace
  - Delete
- parent_edges: Vec<HunkParentEdge>
- evidence_refs
- certainty
- coverage
```

```text
HunkParentEdge
- edge_id
- edge_order: u32
- parent_hunk_id
- parent_range: Option<PatchLineSpan>
- child_range: Option<PatchLineSpan>
- mapping_kind
  - directEdit
  - moved
  - copied
  - split
  - merged
  - weightedOverlap
  - formatterRewrite
  - generatorInput
  - ambiguous
- overlap_weight: Option<f32>
- merge_group_id: Option<String>
- contribution_kind
  - sole
  - mergeContributor
  - weightedContributor
- certainty
- coverage
```

Graph rules:

- edges are traversed in `(edge_order, edge_id)` order
- exact parent edges must be acyclic within the hunk graph
- exact non-merge child ranges for one child hunk must form a non-overlapping
  partition of the child span
- exact merge edges may share the same child span only when they carry the same
  `merge_group_id` and `contribution_kind = mergeContributor`
- weighted overlap edges must use `mapping_kind = weightedOverlap`,
  `contribution_kind = weightedContributor`, and explicit `overlap_weight`
- duplicate exact edges for the same `(parent_hunk_id, child_range)` are invalid
- when the projector cannot prove ordering, partitioning, or acyclicity, it must
  downgrade the edge to `mapping_kind = ambiguous` with `certainty = Ambiguous`

Lineage traversal walks from derived projected terminal segments to
`HunkRecord`, then through durable `parent_edges`, input/output file-version
ids, and immutable evidence refs. `ProjectedSegment.segment_id` is a query index
identifier, not a durable lineage edge, and must not be stored as a parent fact in
`HunkRecord`. Traversal stops and returns the corresponding status when it
reaches an ambiguous identity edge, unavailable evidence, or manifest-only state.

#### Revision Alias Records

Historical Git queries must use exact aliases only.

`RevisionAlias`

- `alias_id: String`
- `workspace_instance_id`
- `workspace_state_id`
- `git_commit_oid`
- `git_tree_oid`
- `created_event_ref: EventRef`
- `superseded_by_event_ref: Option<EventRef>`
- `alias_source`
  - `RecordedPostState`
  - `ExactTreeMatch`

Rules:

- no fuzzy commit matching
- accepted historical selectors must be full commit OIDs, exact tree OIDs,
  recorded alias ids, or explicitly resolved fully qualified moving refs
- moving refs such as `refs/heads/main` or `refs/tags/v1.2.3` must be resolved
  to commit/tree before provenance lookup
- if multiple workspaces could satisfy the same selector, return
  `query_status.selector_status = Ambiguous`
- if no exact recorded state matches, return
  `query_status.selector_status = Unavailable`
- aliases bind a Git revision to `workspace_state_id`, not to one
  `projection_version`
- `CodeRangeSelector.recordedAlias.alias_id` resolves against `RevisionAlias.alias_id`
- range queries resolve the latest ready projection for the selected
  `workspace_state_id`
- if no ready projection exists yet, return `indexing_status = Pending`
- if an alias must be superseded, record a hash-chained alias supersession event
  instead of mutating the alias in place

### 5. Query Contract

Queries are projections over recorded facts. They must not require callers to
know internal state ids for ordinary live lookups, and they must not collapse
selector ambiguity into transport errors.

Selector classes:

- `LiveWorkspace`
  - resolves the current stream head for a workspace stream, workspace instance,
    or repo scope
- `RecordedState`
  - resolves an exact `workspace_state_id`
- `RecordedAlias`
  - resolves an exact `alias_id`
- `GitCommit`
  - resolves an exact commit OID plus optional repo/workspace scoping and
    expected tree OID
- `GitTree`
  - resolves an exact tree OID and optional repo scope
- `MovingRef`
  - resolves a fully qualified branch/tag ref to a commit/tree at query time
    and compares the result with the optional expected commit OID

Rules:

- `LiveWorkspace` is the default live query shape; clients should not need to
  call `provenanceStreamHead/read` just to query the current editor-visible file range
- live selector resolution binds exactly one concrete `resolved_workspace_state_id`
  per candidate before reading projection data
- after reading that bound state, live queries must perform a bounded
  live-anchor revalidation before returning `freshness = Current`; if drift is
  detected, the kernel either records `code.external_workspace_mutation_observed`
  or returns `query_status.freshness = Stale`
- stale live responses must still report evidence for the bound
  `resolved_workspace_state_id`; the server must not silently retry against a
  newer state within the same response
- `RecordedState` is for advanced and forensic callers that already have a
  stable state anchor
- selector mismatch is represented by `query_status.selector_status`, not by
  `freshness = Stale`
- `freshness` is reserved for live drift between the selector resolution and the
  final read; non-live surfaces set `freshness = NotApplicable`
- multiple exact matches are valid provenance outcomes and must return multiple
  candidates with `query_status.selector_status = Ambiguous`
- every candidate carries its own resolved state, range, status, and evidence
  envelope

## Status Model

Do not overload one enum with multiple dimensions.

Recorder facts and query resolution facts should be modeled separately.

`RecorderStatus`

- `certainty`
  - `Exact`
  - `Ambiguous`
- `coverage`
  - `Complete`
  - `Partial`
  - `Unavailable`
- `indexing_status`
  - `Ready`
  - `Pending`
  - `Blocked`
  - `Failed`
- `reason_codes: Vec<String>`
- `status_anchor`
  - the exact `StatusAnchor` against which the recorder status was evaluated

`QueryResolutionStatus`

- `selector_status`
  - `Matched`
  - `ConstraintMismatch`
  - `Ambiguous`
  - `Unavailable`
- `freshness`
  - `Current`
  - `Stale`
  - `NotApplicable`
- `reason_codes: Vec<String>`
- `status_anchor`
  - the exact `StatusAnchor` for the selector, stream, epoch, blob, entity, or
    event against which query resolution was evaluated

`ProvenanceStatus`

- `recorder_status: RecorderStatus`
- `query_status: QueryResolutionStatus`

Examples:

- expected tree or range fingerprint mismatch:
  - `query_status.selector_status = ConstraintMismatch`
- drift after capture but before query:
  - `query_status.freshness = Stale`
- non-live query surfaces such as export or blob reads:
  - `query_status.freshness = NotApplicable`
- missing required blob:
  - `recorder_status.coverage = Unavailable`
- ambiguous delete/recreate boundary:
  - `recorder_status.certainty = Ambiguous`
- bulk indexing still running:
  - `recorder_status.coverage = Partial`
  - `recorder_status.indexing_status = Pending`

Mutation intervals persist `RecorderStatus`. Entity-resolving query responses
compose the relevant `RecorderStatus` with a `QueryResolutionStatus`.
Stream/export APIs that validate selectors or resume anchors but do not resolve
a recorder-backed entity may return only `QueryResolutionStatus`.
Schema-bundle reads use `SchemaBundleReadStatus` instead of
`QueryResolutionStatus` because they resolve version artifacts, not selectors or
freshness.

## Blob and Export Contract

The export contract must be stable enough for a future Forgeloop collector.

### Registration and stream handoff

mcodex owns local stream claims. Forgeloop owns registered cloud stream
identity.

`LocalStreamClaim`

- `ledger_scope: LedgerScope`
- `stream_id: String`
- `workspace_stream_id: Option<String>`
- `workspace_instance_id: Option<String>`
- `stream_epoch: u64`
- `workspace_root_fingerprint: Option<String>`
- `device_claim: String`
- `repo_scope_claim: Option<String>`
- `canonical_git_remote_claim: Option<String>`

Local stream claims and cloud registration observations are replay-relevant
export identity facts. Descriptors expose the latest projected values, but the
authoritative source of truth is hash-chained `system.stream_claim_recorded` and
`system.stream_registration_recorded` events.

`RegisteredTraceStream`

- assigned by Forgeloop after enrollment or collector registration
- maps local scoped stream epochs to cloud `stream_id`
- is ordered by `(org_id, ledger_scope, stream_id, stream_epoch, sequence)`
- may quarantine conflicting local claims or hash-chain gaps

Local export must not assume that any local scoped stream id is the cloud
`stream_id`.

`LedgerStreamDescriptor`

- `ledger_scope: LedgerScope`
  - `GlobalExecution`
  - `WorkspaceCustody`
  - `AccessAudit`
- `stream_id: String`
- `current_stream_epoch: u64`
- `head_event_ref: Option<EventRef>`
- `current_epoch_descriptor_ref: Option<EventRef>`
- `current_workspace_state_event_ref: Option<EventRef>`
- `stream_status: StreamStatus`
  - `Open`
  - `Sealed`
  - `Abandoned`
  - `UnknownClosure`
- `registered_stream_ref: Option<String>`
- `workspace_stream_descriptor: Option<WorkspaceStreamDescriptor>`

`registered_stream_ref` is a projection of the latest
`system.stream_registration_recorded` event for the stream. Export of a specific
epoch must use the epoch-scoped identity refs on `StreamEpochDescriptor`, not
only this stream-level latest projection.

### Workspace stream descriptor

`WorkspaceStreamDescriptor`

- `workspace_stream_id: String`
- `workspace_instance_id: String`
- `current_stream_epoch: u64`
- `current_workspace_state_id: Option<String>`
- `head_event_ref: Option<EventRef>`
- `current_epoch_descriptor_ref: Option<EventRef>`
- `current_workspace_state_event_ref: Option<EventRef>`
- `stream_closure_status: StreamClosureStatus`
  - `Open`
  - `Sealed`
  - `Abandoned`
  - `UnknownClosure`
- `workspace_root_fingerprint: String`
- `device_id: String`
- `canonical_git_remote: Option<String>`
- `repo_scope_id: Option<String>`
- `local_claims: Vec<LocalStreamClaim>`
- `registered_stream_ref: Option<String>`

`local_claims` is a projection of `system.stream_claim_recorded` events.
`registered_stream_ref` is a projection of `system.stream_registration_recorded`
events. These descriptor fields are convenience projections; they are not a
substitute for the hash-chained epoch-scoped claim/registration facts.
`head_event_ref` is the current stream tip. `current_workspace_state_event_ref`
is the exact workspace-custody event that established
`current_workspace_state_id`; later alias, repair, blob, or system events must
not overwrite that distinction.

`StreamEpochDescriptor`

- `ledger_scope: LedgerScope`
- `stream_id: String`
- `stream_epoch: u64`
- `epoch_status: EpochStatus`
  - `Open`
  - `Sealed`
  - `Broken`
  - `Superseded`
- `first_sequence: u64`
- `last_sequence: Option<u64>`
- `last_event_hash: Option<String>`
- `continuity_state: EpochContinuityState`
  - `VerifiedPrevious`
    - `previous_epoch: u64`
    - `previous_epoch_last_sequence: u64`
    - `previous_epoch_last_event_hash: String`
  - `BrokenChain`
    - `break_reason: String`
    - `last_trusted_sequence: Option<u64>`
    - `last_trusted_event_hash: Option<String>`
    - `repair_event_ref: Option<EventRef>`
  - `UnknownPrevious`
    - `reason: String`
- `continuity_event_ref: EventRef`
  - points to the epoch opening event, either `system.epoch_started` for genesis
    epochs or `system.epoch_rotated` for non-genesis epochs
- `claim_event_ref: Option<EventRef>`
  - the claim fact that applies to this exact scoped stream epoch
- `registration_event_ref: Option<EventRef>`
  - the cloud registration fact that applies to this exact scoped stream epoch

`UnknownPrevious.reason = streamHandoff` is valid only for the first epoch of a
new workspace custody stream. The predecessor relationship for that case is
validated through the following `system.stream_handoff_recorded` event, not
through previous-epoch continuity.

`claim_event_ref` and `registration_event_ref` must resolve to events whose
payload targets the same `(ledger_scope, stream_id, stream_epoch)` as the
descriptor. They may be absent only before local claim or cloud registration is
known; once known, export must expose the exact refs applicable to the requested
epoch.

### Canonical event envelope

`TraceKernelEvent`

- `event_id`
- `idempotency_key`
- `append_batch_id: Option<String>`
- `append_batch_index: Option<u32>`
- `append_batch_size: Option<u32>`
- `ledger_context`
  - `category`
    - `execution`
    - `code`
    - `blob`
    - `system`
    - `accessAudit`
  - `ledger_scope`
  - `stream_id`
  - `stream_epoch`
  - `sequence`
  - `scope_metadata`
    - optional scope-specific metadata such as `workspace_instance_id`
- `schema_version`
- `event_type`
- `event_hash`
- `previous_event_hash`
- `occurred_at`
- `recorded_at`
- `execution_context: Option<ExecutionContext>`
- `causality_refs`
- `canonical_payload`
- `local_metadata`
- `blob_refs`

`canonical_payload` is externally discriminated by the envelope `event_type`,
but it is still schema-visible. The export schema must define a
`TraceSchemaBundle` whose `EventPayloadRegistryEntry` rows map every
`(schema_version, event_type)` pair to exactly one versioned payload DTO. Append
validation must reject events whose `schema_version`, `event_type`, and payload
shape do not match the registered entry.

`canonical_payload` does not use the generic `type` discriminator required by
app-server DTO tagged unions. Export consumers must not need local SQLite tables
to understand event semantics. `local_metadata` is never hashed and may contain
local paths, locators, delivery state, and debug fields.

`event_type` is the stable mcodex kernel event name and is part of the canonical
hash input. Exporters may add a mapped Forgeloop canonical name in export
metadata, but they must not replace `event_type` inside the ledger event.
`ledger_context.category` uses the lower-camel wire strings above, and those
exact values are part of the canonical hash input.

When events are exported through `provenanceEvent/exportRawReplay` or
`provenanceEvent/exportCommitted`, the mapped name is surfaced as
`RawReplayExportedTraceEvent.external_event_name` or
`CommittedExportedTraceEvent.external_event_name` under the selected
`export_contract_version`.

Events created outside a session or thread, such as bootstrap, blob custody,
stream repair, and external observations, set `execution_context = None` and
must still carry explicit `causality_refs` where a cause is known.

Export mapping must be deterministic and versioned. `export_contract_version`
selects a frozen mapping table:

- if Forgeloop has an adopted canonical event name for that kernel event in the
  selected contract version, export that canonical name
- otherwise export the stable mcodex extension name `codex.<kernel_event_type>`
- collectors must never drop a required event family only because Forgeloop does
  not yet have a first-party canonical name for it
- the same stored event exported under the same `export_contract_version` must
  always produce the same external event name, even if a later contract version
  adopts a different Forgeloop canonical name

Deterministic mapping examples:

- `execution.tool_call_started` -> `execution.tool_call_started`
- `execution.tool_call_finished` -> `execution.tool_call_completed`
- `code.workspace_state_captured` -> `code.workspace_state_created`
- `code.mutation_observation_recorded` ->
  `codex.code.mutation_observation_recorded`
- `code.file_version_observed` -> `code.file_delta_observed`
- `code.hunk_observed` -> `code.hunk_observed`
- `code.recorded_state_alias_created` -> `code.recorded_state_alias_created`
- `system.stream_sealed` -> `codex.system.stream_sealed`
- `system.stream_handoff_recorded` -> `codex.system.stream_handoff_recorded`
- `system.append_batch_committed` -> `codex.system.append_batch_committed`
- `access.blob_read_authorized` -> `codex.access.blob_read_authorized`
- `access.blob_read_delivered` -> `codex.access.blob_read_delivered`
- `access.raw_content_read_authorized` ->
  `codex.access.raw_content_read_authorized`
- `access.raw_content_read_delivered` ->
  `codex.access.raw_content_read_delivered`

Generic internal activity events must still be exportable. Until Forgeloop owns
canonical names for them, export them under the stable mcodex extension namespace:

- `execution.activity_started` -> `codex.execution.activity_started`
- `execution.activity_finished` -> `codex.execution.activity_finished`

Forgeloop may project those extension events into higher-level RunSession or
actor timeline views, but collectors must not drop them.

Required typed payloads:

- `blob.descriptor_recorded`
  - `blob_descriptor: BlobDescriptor`
  - `initial_availability: BlobAvailability`
- `blob.availability_changed`
  - `blob_ref_id: String`
  - `descriptor_event_ref: EventRef`
  - `previous_status: BlobAvailability`
  - `new_status: BlobAvailability`
  - `reason_code: String`
  - `evidence_refs: Vec<EventRef>`
- `access.blob_read_authorized`
  - `requester: AccessRequester`
  - `blob_read_session_id: String`
  - `opening_request_id: String`
  - `target_blob_ref_id: String`
  - `authorizing_custody_event_ref: EventRef`
  - `target_descriptor_event_ref: EventRef`
  - `target_availability: BlobAvailability`
  - `target_availability_event_ref: EventRef`
  - `target_workspace_event_ref: Option<EventRef>`
  - `access_purpose: String`
  - `result`
    - `allowed`
    - `denied`
    - `unavailable`
  - `opening_requested_byte_range: Option<ByteRange>`
  - `authorized_session_scope`
    - `wholeBlob`
    - `contiguousForwardRead`
      - `start_offset: u64`
      - `max_offset_exclusive: Option<u64>`
- `access.blob_read_delivered`
  - `authorization_event_ref: EventRef`
  - `blob_read_session_id: String`
  - `delivery_checkpoint_index: u32`
  - `delivery_result`
    - `delivered`
    - `failed`
    - `truncated`
    - `unavailable`
  - `delivered_byte_range: Option<ByteRange>`
  - `delivered_byte_count_cumulative: Option<u64>`
  - `delivered_content_digest_cumulative: Option<String>`
  - `failure_reason: Option<String>`
- `access.raw_content_read_authorized`
  - `requester: AccessRequester`
  - `request_id: String`
  - `target_event_ref: EventRef`
  - `content_kind: String`
  - `access_purpose: String`
  - `result`
    - `allowed`
    - `denied`
    - `unavailable`
  - `requested_byte_range: Option<ByteRange>`
- `access.raw_content_read_delivered`
  - `authorization_event_ref: EventRef`
  - `delivery_result`
    - `delivered`
    - `failed`
    - `truncated`
    - `unavailable`
  - `delivered_byte_range: Option<ByteRange>`
  - `delivered_byte_count: Option<u64>`
  - `delivered_content_digest: Option<String>`
  - `redaction_state`
  - `failure_reason: Option<String>`
- `system.epoch_started`
  - `stream_epoch`
  - `continuity_state`
  - `start_reason`
    - `bootstrap`
    - `import`
    - `explicitLocalReset`
    - `streamHandoff`
- `system.epoch_rotated`
  - `previous_stream_epoch`
  - `new_stream_epoch`
  - `continuity_state`
- `system.chain_broken_recorded`
  - `break_reason`
  - `last_trusted_event_ref: Option<EventRef>`
  - `detected_by_event_ref: Option<EventRef>`
- `system.append_batch_committed`
  - `append_batch_id`
  - `participating_stream_refs: Vec<LedgerStreamRef>`
  - `ordered_event_refs: Vec<EventRef>`
  - `stream_local_outcomes: Vec<StreamLocalBatchOutcome>`
  - `outcome_reason: String`
- `system.append_batch_aborted`
  - `append_batch_id`
  - `participating_stream_refs: Vec<LedgerStreamRef>`
  - `materialized_event_refs_ordered: Vec<EventRef>`
  - `reserved_append_refs: Vec<AppendReservationRef>`
  - `stream_local_outcomes: Vec<StreamLocalBatchOutcome>`
  - `abort_reason`
- `system.append_batch_repaired`
  - `append_batch_id`
  - `participating_stream_refs: Vec<LedgerStreamRef>`
  - `materialized_event_refs_ordered: Vec<EventRef>`
  - `reserved_append_refs: Vec<AppendReservationRef>`
  - `stream_local_outcomes: Vec<StreamLocalBatchOutcome>`
  - `repair_reason`
- `system.stream_sealed`
  - `sealed_stream_id`
  - `sealed_epoch`
  - `sealed_previous_event_ref: Option<EventRef>`
  - `seal_reason`
- `system.stream_handoff_recorded`
  - `previous_stream_ref: LedgerStreamRef`
  - `handoff_kind`
    - `sealedPredecessor`
    - `unsealedPredecessorRecovery`
  - `previous_stream_closure_status`
  - `previous_terminal_event_ref: Option<EventRef>`
  - `previous_last_trusted_event_ref: Option<EventRef>`
  - `new_stream_ref: LedgerStreamRef`
  - `handoff_reason`
- `system.stream_claim_recorded`
  - `local_stream_claim: LocalStreamClaim`
  - `claim_reason: String`
- `system.stream_registration_recorded`
  - `target_stream_ref: LedgerStreamRef`
  - `registered_stream_ref: String`
  - `registration_source: String`
  - `registration_recorded_at: i64`
- `system.repair_applied`
  - `repair_kind: String`
  - `affected_stream_refs: Vec<LedgerStreamRef>`
  - `repair_reason: String`
  - `superseded_event_refs: Vec<EventRef>`
  - `predecessor_stream_ref: Option<LedgerStreamRef>`
  - `successor_stream_ref: Option<LedgerStreamRef>`
  - `paired_handoff_event_ref: Option<EventRef>`
  - `previous_stream_closure_status: Option<String>`
  - `previous_last_trusted_event_ref: Option<EventRef>`

Every required event family must have a versioned payload DTO before
implementation. The list above is the minimum v1 shape; event-specific specs may
add fields but must not move replay-critical data into `local_metadata`.

When `system.repair_applied` is emitted as supplemental evidence for successor
stream handoff, `predecessor_stream_ref`, `successor_stream_ref`, and
`paired_handoff_event_ref` are mandatory. If the predecessor was `Abandoned` or
`UnknownClosure`, both the handoff and repair payloads must include
`previous_last_trusted_event_ref`. The paired handoff event remains the
authoritative predecessor-link fact; repair payloads provide context about why
the predecessor stream could not be cleanly sealed.

When `system.repair_applied` is emitted for lineage/projector repair, it is
system-scoped diagnostic context only. Revision-level lineage authority must be
derived from the corresponding `code.projection_job_repaired` event and its
`ProjectionJobRecord`.

`system.stream_handoff_recorded` is a strict state machine:

- `handoff_kind = sealedPredecessor`
  - `previous_stream_closure_status` must be `Sealed`
  - `previous_terminal_event_ref` is mandatory and must resolve to the
    predecessor stream's `system.stream_sealed` event
  - `previous_last_trusted_event_ref` must be absent
- `handoff_kind = unsealedPredecessorRecovery`
  - `previous_stream_closure_status` must be `Abandoned` or `UnknownClosure`
  - `previous_terminal_event_ref` must be absent
  - `previous_last_trusted_event_ref` is mandatory when any trusted predecessor
    event is known
  - append validation must reject this handoff kind if the predecessor stream
    was still writable when the successor handoff was recorded

Access audit is two-phase. Authorization events must commit before bytes are
released. Delivery events record what actually happened after the server attempts
the disclosure. On successful delivery, `delivered_byte_range`,
`delivered_byte_count_cumulative`, and `delivered_content_digest_cumulative`
are mandatory. Denied or unavailable authorization results do not have a
delivery event. Failed or truncated delivery attempts must write a delivery
event with `delivery_result` and `failure_reason` before the response returns.

Append validation must reject `access.blob_read_authorized` events whose
`authorizing_custody_event_ref` does not resolve to an exact hash-chained
`blob.descriptor_recorded` event for the requested `blob_ref_id`. In v1,
`authorizing_custody_event_ref` and `target_descriptor_event_ref` must resolve
to the same exact descriptor event. `target_workspace_event_ref` may carry
supporting workspace-state context, but it is not the authorization anchor.
Weak stream/blob selectors are query input only; they must be resolved to one
exact descriptor `EventRef` before an authorization event enters the ledger.

`AccessRequester`

- `requester_kind`
  - `interactiveUser`
  - `platformAutomation`
  - `localClient`
  - `toolCall`
  - `unknownLocalActor`
- `local_principal_ref: Option<String>`
- `client_id: Option<String>`
- `session_id: Option<String>`
- `thread_id: Option<String>`
- `turn_id: Option<String>`
- `tool_call_id: Option<String>`
- `process_activity_id: Option<String>`
- `client_process_id: Option<String>`
- `client_connection_id: Option<String>`

When a disclosure request comes from a journaled caller, the
`TraceKernelEvent.execution_context` and `causality_refs.caused_by` on the access
audit event must point at that caller. `AccessRequester` is the requester's
stable actor identity; it does not replace ledger causality.

`AccessRequester` is server-derived provenance identity. Clients may not set
these fields directly on read RPCs. The server derives them from the
authenticated caller, session/thread/tool context, or launcher metadata that is
already bound to the current request.

Raw/blob disclosure must be fail-closed: the authorization event must commit
before bytes are released to the caller. If the authorization append fails,
`provenanceBlob/read` or any future raw-content read must not disclose content.

### Event payload registry

The payload registry is a generated schema artifact, not prose-only
documentation.

```text
TraceSchemaBundle
- schema_bundle_id: String
- schema_bundle_digest: String
- schema_versions: Vec<String>
- export_contract_version: String
- payload_registry_entries: Vec<EventPayloadRegistryEntry>
- export_name_mappings: Vec<ExportEventNameMapping>
- schema_documents: Vec<SchemaDocument>
```

```text
EventPayloadRegistryEntry
- schema_version: String
- event_type: String
- payload_type_name: String
- payload_schema_ref: String
- payload_schema_digest: String
- canonical_payload_version: String
```

```text
ExportEventNameMapping
- export_contract_version: String
- kernel_event_type: String
- external_event_name: String
```

```text
SchemaDocument
- schema_ref: String
- schema_digest: String
- schema_content: String
```

```text
SchemaBundleRef
- schema_bundle_id: String
- schema_bundle_digest: String
```

The registry key is `(schema_version, event_type)`. A bundle may contain entries
for multiple schema versions because a single exported page can include older
and newer events. `schema_versions` lists every schema version covered by the
bundle. Append validation and export validation must reject a payload whose DTO
shape does not match its event's `(schema_version, event_type)` pair. A later
schema may evolve a payload only by registering a new `(schema_version,
event_type)` entry. `TraceSchemaBundle` is self-contained: every
`payload_schema_ref` must have a matching `SchemaDocument` with the declared
digest, and `schema_bundle_digest` covers the canonicalized bundle metadata plus
schema documents.

`TraceSchemaBundle` canonicalization must fix array order and uniqueness before
hashing:

- `schema_versions`: unique, sorted ascending
- `payload_registry_entries`: unique by `(schema_version, event_type)`, sorted
  by that tuple
- `export_name_mappings`: unique by `(export_contract_version, kernel_event_type)`,
  sorted by that tuple
- `schema_documents`: unique by `schema_ref`, sorted by `schema_ref`

The following v1 event types must have schema-visible versioned payload DTOs
before implementation. These names are normative for export schema generation:

- `execution.session_started` -> `ExecutionSessionStartedPayload`
- `execution.session_finished` -> `ExecutionSessionFinishedPayload`
- `execution.thread_started` -> `ExecutionThreadStartedPayload`
- `execution.thread_finished` -> `ExecutionThreadFinishedPayload`
- `execution.turn_started` -> `ExecutionTurnStartedPayload`
- `execution.turn_finished` -> `ExecutionTurnFinishedPayload`
- `execution.item_recorded` -> `ExecutionItemRecordedPayload`
- `execution.tool_call_started` -> `ExecutionToolCallStartedPayload`
- `execution.tool_call_finished` -> `ExecutionToolCallFinishedPayload`
- `execution.process_started` -> `ExecutionProcessStartedPayload`
- `execution.process_finished` -> `ExecutionProcessFinishedPayload`
- `execution.activity_started` -> `ExecutionActivityStartedPayload`
- `execution.activity_finished` -> `ExecutionActivityFinishedPayload`
- `execution.mutation_interval_opened` -> `MutationIntervalOpenedPayload`
- `execution.mutation_interval_closed` -> `MutationIntervalClosedPayload`
- `code.workspace_state_captured` -> `WorkspaceStateCapturedPayload`
- `code.mutation_observation_recorded` -> `MutationObservationRecordedPayload`
- `code.bootstrap_seeded` -> `BootstrapSeededPayload`
- `code.unsupervised_workspace_mutation_observed` ->
  `UnsupervisedWorkspaceMutationObservedPayload`
- `code.external_workspace_mutation_observed` ->
  `ExternalWorkspaceMutationObservedPayload`
- `code.file_version_observed` -> `FileVersionObservedPayload`
- `code.file_entity_edge_recorded` -> `FileEntityEdgeRecordedPayload`
- `code.projection_job_started` -> `ProjectionJobStartedPayload`
- `code.projection_job_finished` -> `ProjectionJobFinishedPayload`
- `code.projection_job_repaired` -> `ProjectionJobRepairedPayload`
- `code.hunk_observed` -> `HunkObservedPayload`
- `code.segment_projection_applied` -> `SegmentProjectionAppliedPayload`
- `code.recorded_state_alias_created` -> `RecordedStateAliasCreatedPayload`
- `code.recorded_state_alias_superseded` ->
  `RecordedStateAliasSupersededPayload`
- `blob.descriptor_recorded` -> `BlobDescriptorRecordedPayload`
- `blob.availability_changed` -> `BlobAvailabilityChangedPayload`
- `access.blob_read_authorized` -> `AccessBlobReadAuthorizedPayload`
- `access.blob_read_delivered` -> `AccessBlobReadDeliveredPayload`
- `access.raw_content_read_authorized` -> `AccessRawContentReadAuthorizedPayload`
- `access.raw_content_read_delivered` -> `AccessRawContentReadDeliveredPayload`
- `system.epoch_started` -> `SystemEpochStartedPayload`
- `system.epoch_rotated` -> `SystemEpochRotatedPayload`
- `system.chain_broken_recorded` -> `SystemChainBrokenRecordedPayload`
- `system.append_batch_committed` -> `SystemAppendBatchCommittedPayload`
- `system.append_batch_aborted` -> `SystemAppendBatchAbortedPayload`
- `system.append_batch_repaired` -> `SystemAppendBatchRepairedPayload`
- `system.stream_sealed` -> `SystemStreamSealedPayload`
- `system.stream_handoff_recorded` -> `SystemStreamHandoffRecordedPayload`
- `system.stream_claim_recorded` -> `SystemStreamClaimRecordedPayload`
- `system.stream_registration_recorded` ->
  `SystemStreamRegistrationRecordedPayload`
- `system.repair_applied` -> `SystemRepairAppliedPayload`

Export schema generation must include `TraceSchemaBundle` so downstream
collectors can validate `event_type`, `schema_version`, export name mapping, and
`canonical_payload` without linking mcodex internals.

### Blob contract

Do not make the canonical blob shape depend on a local file path.

`BlobAvailability`

- `available`
- `pending`
- `missing`
- `checksumMismatch`
- `redacted`
- `expired`

`BlobDescriptor`

- `blob_ref_id: String`
- `digest_algorithm: String`
- `expected_digest: String`
- `byte_size: u64`
- `content_kind: String`
- `content_type: Option<String>`
- `ref_kind: String`
- `scope: String`
- `sensitivity: String`
- `retention_hint: Option<String>`
- `legal_hold_hint: Option<String>`
- `required_for_reconstruction: bool`

`BlobDescriptor` is immutable content identity. Current availability is not part
of the descriptor. Availability changes are represented by replayable
availability facts and resolved separately at query time.

`blob_ref_id` is the immutable blob-identity key in v1. A given
`(workspace_stream_id, stream_epoch, blob_ref_id)` must resolve to exactly one
visible `blob.descriptor_recorded` event. v1 does not define blob-descriptor
supersession. If content identity changes, the recorder must mint a new
`blob_ref_id` rather than append a second competing descriptor for the same id.
Append validation must reject a second visible descriptor event for the same
epoch-local `blob_ref_id`.

`BlobManifestEntry`

- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`

`LocalBlobLocator`

- `blob_ref_id: String`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`
- `local_storage_ref: ExactArtifactRef`

Canonical export should depend on `BlobDescriptor`. The local locator is an
implementation detail for the local collector, but it must still bind to one
exact descriptor event rather than only a mutable `blob_ref_id`.

Descriptor selection and availability resolution are separate steps:

- `blob.descriptor_recorded` selects immutable blob identity
- the effective current availability for that descriptor is resolved from the
  latest visible availability fact in the same snapshot
- if no later `blob.availability_changed` exists, the descriptor event itself is
  the authoritative `availability_event_ref` and its
  `initial_availability` remains the current availability
- the authoritative descriptor event for a blob identity is the enclosing
  `blob.descriptor_recorded` event that carries this `BlobDescriptor`; the
  canonical payload must not embed a self-referential `EventRef` back to that
  same event

Blob reads return a tagged result instead of assuming content is always
available. Serialized wire values are camelCase:

- `inlineChunk`
  - descriptor, offset, byte count, base64 content, next offset, continuation
    token, digest verification, authorization event ref, and optional delivery
    event ref
- `unavailable`
  - descriptor, availability status, reason code, authorization event ref, and
    optional delivery event ref when the failure occurred after a read attempt
- `accessDenied`
  - descriptor, policy reason code, and authorization event ref
- `auditFailure`
  - audit-stream or audit-append failure; no content and no access event refs
- `selectorFailure`
  - selector resolution failed; authoritative status lives in the top-level
    `ReadBlobResponse.query_status`, and no exact custody fact was selected
- `invalidRequest`
  - resolved selector but invalid offset/continuation/session parameters; no
    content and no newly disclosed bytes

`provenanceBlob/read` opens one auditable blob-read session per logical
continuation chain. It appends `access.blob_read_authorized` before first
disclosure and appends `access.blob_read_delivered` as session-scoped
checkpoints rather than requiring one delivery event per chunk. Reads from
sealed historical epochs therefore remain auditable without modifying those
epochs. If the caller does not explicitly select an audit stream, the server
must deterministically resolve the current writable local audit stream and
expose the result through returned authorization and delivery event refs.

Writable audit ingress remains fail-closed: if the local `AccessAudit` stream
cannot be resolved or appended, the read must fail rather than disclose bytes
without audit. To control hot-store growth, implementations may export, cold
store, or compact intermediate delivery checkpoints for a finished blob-read
session after policy-defined durability/retention conditions are met, but they
must preserve the authorization event, the final retained delivery checkpoint,
and enough chain continuity to prove the disclosed byte ranges and result.

### Conversation content

Do not inline raw excerpts as part of the stable canonical export shape.

- raw or semi-raw excerpts should be blob-backed
- query surfaces may return a sanitized summary string for convenience
- full raw content remains behind blob reads and future policy gates

Even locally, `provenanceBlob/read` should be explicit and auditable. It may be a local
policy check in mcodex v1, but the API shape must not bypass future raw-content
entitlements.

`access.blob_read_authorized` is emitted once per logical blob-read session.
`opening_request_id` identifies the RPC that opened that session; later resumed
RPCs are linked by `blob_read_session_id`, not by reusing `opening_request_id`.
`opening_requested_byte_range` captures only the opening RPC's requested range.
The session's total disclosure budget is instead defined by
`authorized_session_scope`; resumed reads that would exceed that scope must open
a new blob-read session rather than silently extend the original one.

`access.blob_read_delivered` is cumulative at the checkpoint level:

- `delivered_byte_range` covers the cumulative byte range disclosed from the
  start of the session through this checkpoint
- `delivered_byte_count_cumulative` is the cumulative disclosed byte count
  through this checkpoint
- `delivered_content_digest_cumulative` binds the cumulative disclosed content
  through this checkpoint, not only the most recent chunk

## App-Server API

Provenance must have its own v2 surface.

### Required RPCs

- `provenanceWorkspaceStream/list`
- `provenanceLedgerStream/list`
- `provenanceStreamEpoch/list`
- `provenanceLedgerStream/read`
- `provenanceStreamHead/read`
- `provenanceRevisionAlias/list`
- `provenanceRevisionAlias/read`
- `provenanceSchema/read`
- `provenanceTurn/read`
- `provenanceRange/read`
- `provenanceHunk/read`
- `provenanceActivity/read`
- `provenanceMutationInterval/read`
- `provenanceWorkspaceTransition/read`
- `provenanceMutationObservation/read`
- `provenanceEvent/exportRawReplay`
- `provenanceEvent/exportCommitted`
- `provenanceBlobManifest/read`
- `provenanceBlob/read`

### API shape rules

- list or export endpoints use `cursor` / `limit` and `data` / `next_cursor`
- export and blob reads are keyed by stable stream identity, not only
  a local workspace root path
- `thread/read` remains thread history; it is not the provenance read model
- v2 payloads must be concrete `*Params` and `*Response` DTOs
- v2 provenance DTOs must derive `JsonSchema` and `TS`
- every v2 provenance DTO must set `#[ts(export_to = "v2/")]`
- optional client-to-server request fields must use
  `#[ts(optional = nullable)]`
- server-to-client `*Response` fields are not optional on the wire; `Option<T>`
  response fields serialize as explicit JSON `null` when empty
- shared DTOs should reuse the same `ExecutionContext`, `CausalityRefs`, and
  status shapes used in the journal/export envelope
- Rust DTO fields use snake_case with `#[serde(rename_all = "camelCase")]` for
  wire/export JSON
- the named DTOs below are the normative contract for schema generation; do not
  replace them with anonymous inline response blocks in implementation docs
- app-server DTO tagged unions use `#[serde(tag = "type", rename_all = "camelCase")]`;
  TypeScript bindings must use the same `type` discriminator and camelCase
  variant values
- `TraceKernelEvent.canonical_payload` is exempt from the generic `type`
  discriminator rule because it is externally discriminated by envelope
  `event_type`
- prose may use Rust-style enum variant names for readability, but wire/export
  values and canonical hash input are always the serialized lower-camel JSON
  values
- boolean include flags use default-false `bool`, not nullable optional booleans

List cursors must be stable and replayable. A cursor encodes the fixed snapshot
watermark selected by the first page plus the last emitted canonical sort key.
The server must not reorder rows between pages for the same cursor.
The cursor must also bind the canonicalized effective filters and include flags
for that list surface. Replaying a list cursor with different filters is a
request validation error; the server must not silently rebind it to a different
list context.

Canonical list/export ordering:

- `provenanceWorkspaceStream/list`: `workspace_stream_id ASC`
- `provenanceLedgerStream/list`: `ledger_scope ASC, stream_id ASC`
- `provenanceStreamEpoch/list`: `ledger_scope ASC, stream_id ASC,
  stream_epoch ASC`
- `provenanceRevisionAlias/list`: `workspace_instance_id ASC, git_commit_oid ASC,
  git_tree_oid ASC, alias_id ASC`
- `provenanceEvent/exportRawReplay`: `sequence ASC` within the selected stream
  epoch
- `provenanceEvent/exportCommitted`: `sequence ASC` within the selected stream
  epoch
- `provenanceBlobManifest/read`: `blob_ref_id ASC`
- candidate lineage pagination: `depth ASC, child_hunk_id ASC,
  parent_hunk_id ASC, edge_id ASC`

### Shared DTOs

`EventRef`

- `ledger_scope: LedgerScope`
  - `globalExecution`
  - `workspaceCustody`
  - `accessAudit`
- `stream_id: String`
- `stream_epoch: u64`
- `sequence: u64`
- `event_id: String`
- `event_hash: String`

`LedgerScope`

- `globalExecution`
- `workspaceCustody`
- `accessAudit`

`ByteRange`

- `offset: u64`
- `length: u64`

`QueryLineRange`

- `start_line: u32`
- `end_line: u32`

Query ranges are 1-based, inclusive, and non-empty. `start_line` must be at
least `1`, and `end_line` must be greater than or equal to `start_line`.

`PatchLineSpan`

- `start_line: u32`
- `line_count: u32`

Patch spans are 1-based anchors plus a line count. `line_count = 0` is allowed
only for insertion points or delete outputs where a patch operation has a real
zero-width side. Range queries must not use `PatchLineSpan`.

`ProvenanceStatus`

- `recorder_status: RecorderStatus`
- `query_status: QueryResolutionStatus`

`RecorderStatus`

- `certainty`
- `coverage`
- `indexing_status`
- `reason_codes`
- `status_anchor: StatusAnchor`

`QueryResolutionStatus`

- `selector_status`
- `freshness`
  - `current`
  - `stale`
  - `notApplicable`
- `reason_codes`
- `status_anchor: StatusAnchor`

`AuditFailureStatus`

- `audit_result`
  - `streamUnavailable`
  - `appendFailed`
  - `writeRejected`
- `reason_codes`
- `status_anchor: StatusAnchor`

`StatusAnchor`

- `workspaceState`
  - `workspace_state_id: String`
- `workspaceStream`
  - `workspace_stream_id: String`
- `liveWorkspaceSelector`
  - `selector: LiveWorkspaceSelector`
- `ledgerStream`
  - `ledger_scope: LedgerScope`
  - `stream_id: String`
- `streamEpoch`
  - `ledger_scope: LedgerScope`
  - `stream_id: String`
  - `stream_epoch: u64`
- `blobRef`
  - `selector: WorkspaceCustodySelector`
  - `blob_ref_id: String`
- `blobReadAttempt`
  - `selector: WorkspaceCustodySelector`
  - `blob_ref_id: String`
  - `expected_descriptor_event_ref: Option<EventRef>`
  - `continuation_token_fingerprint: Option<String>`
- `blobReadSession`
  - `selector: WorkspaceCustodySelector`
  - `blob_ref_id: String`
  - `descriptor_event_ref: EventRef`
  - `availability_event_ref: EventRef`
  - `blob_read_session_id: String`
- `turn`
  - `session_id: String`
  - `thread_id: String`
  - `turn_id: String`
- `hunk`
  - `hunk_id: String`
- `activity`
  - `activity_id: String`
  - `event_ref: Option<EventRef>`
- `mutationInterval`
  - `mutation_interval_id: String`
  - `event_ref: Option<EventRef>`
- `workspaceTransition`
  - `transition_id: String`
  - `event_ref: Option<EventRef>`
- `mutationObservation`
  - `observation_id: String`
  - `event_ref: Option<EventRef>`
- `codeRevision`
  - `selector: CodeRangeSelector`
- `codeRangeRequest`
  - `selector: CodeRangeSelector`
  - `path: String`
  - `range: QueryLineRange`
  - `expected_range_fingerprint: Option<String>`
- `schemaBundle`
  - `schema_bundle_id: Option<String>`
  - `schema_bundle_digest: Option<String>`
  - `schema_version: Option<String>`
  - `export_contract_version: Option<String>`
- `event`
  - `event_ref: EventRef`
- `none`

`AnchoredCodeRange`

- `path`
- `range: QueryLineRange`
- `selector: CodeRangeSelector`
- `range_fingerprint: Option<String>`

`range_fingerprint` is deterministic:

- algorithm is `sha256`
- input bytes are the exact UTF-8 byte slice for the resolved line range in the
  selected recorded file version
- original line terminators are preserved; there is no newline normalization,
  BOM stripping, or whitespace folding
- files that are not exact UTF-8 text are not valid line-range provenance
  targets in v1

`LiveWorkspaceSelector`

- `workspace_stream_id: Option<String>`
- `workspace_instance_id: Option<String>`
- `repo_scope_id: Option<String>`
- `expected_workspace_state_id: Option<String>`

`CodeRangeSelector`

- `type = liveWorkspace`
  - fields from `LiveWorkspaceSelector`
- `type = recordedState`
  - `workspace_state_id`
- `type = recordedAlias`
  - `alias_id`
- `type = gitCommit`
  - `commit_oid`
  - `repo_scope_id: Option<String>`
  - `workspace_instance_id: Option<String>`
  - `expected_tree_oid: Option<String>`
- `type = gitTree`
  - `tree_oid`
  - `repo_scope_id: Option<String>`
- `type = movingRef`
  - `full_ref_name`
  - `repo_scope_id: Option<String>`
  - `workspace_instance_id: Option<String>`
  - `expected_commit_oid: Option<String>`

Rules:

- `CodeRangeSelector` is a tagged one-of union; malformed multi-mode selectors
  are request validation errors
- wire shape is internally tagged with
  `#[serde(tag = "type", rename_all = "camelCase")]` and
  `#[ts(tag = "type", rename_all = "camelCase")]`
- multiple matching workspaces or recorded states are normal provenance results
  with `query_status.selector_status = Ambiguous`, not transport-level request
  failures
- `GitCommit.expected_tree_oid` is a secondary constraint; mismatch returns
  `query_status.selector_status = ConstraintMismatch`
- `GitCommit` must include `repo_scope_id` or `workspace_instance_id` when the
  local installation tracks more than one repo/workspace that could resolve the
  same commit; if the omitted constraints leave multiple exact candidates, the
  response is `query_status.selector_status = Ambiguous`
- `MovingRef.expected_commit_oid` compares against the server-resolved commit and
  mismatch returns `query_status.selector_status = ConstraintMismatch` with
  `selector_moved`
- `MovingRef.full_ref_name` must be a fully qualified Git ref under
  `refs/heads/` or `refs/tags/`; short names such as `main` are invalid request
  payloads
- `MovingRef` must include `repo_scope_id` or `workspace_instance_id` when the
  local installation tracks more than one repo/workspace that could contain the
  same fully-qualified ref; if the omitted constraints leave multiple exact
  candidates, the response is `query_status.selector_status = Ambiguous`
- `GitTree` is a primary selector for exact tree-state lookup when commit
  identity is unavailable or irrelevant
- `liveWorkspace.expected_workspace_state_id` is a drift guard; mismatch returns
  `query_status.selector_status = ConstraintMismatch`, not a stale result
- `liveWorkspace` must include at least one of `workspace_stream_id`,
  `workspace_instance_id`, or `repo_scope_id`; when more than one is provided,
  the additional fields are constraints


`ExecutionRef`

- `session_id: Option<String>`
- `thread_id: Option<String>`
- `turn_id: Option<String>`
- `item_id: Option<String>`
- `tool_call_id: Option<String>`
- `process_activity_id: Option<String>`
- `client_process_id: Option<String>`
- `client_connection_id: Option<String>`
- `execution_context: Option<ExecutionContext>`

`ActivityRef`

- `activity_id: String`
- `activity_kind: String`
- `execution_context: Option<ExecutionContext>`
- `causality_refs`

`TraceActivity`

- `activity_id: String`
- `activity_kind: String`
- `execution_context: Option<ExecutionContext>`
- `causality_refs: CausalityRefs`
- `started_event_ref: Option<EventRef>`
- `finished_event_ref: Option<EventRef>`
- `status`
  - `open`
  - `finished`
  - `abandoned`
- `workspace_stream_ids: Vec<String>`

`WorkspaceTransitionRef`

- `transition_id: String`
- `workspace_stream_id: String`
- `workspace_instance_id: String`
- `primary_cause: CauseRef`
- `supporting_evidence_event_refs: Vec<EventRef>`
- `pre_workspace_state_id: Option<String>`
- `post_workspace_state_id: String`
- `establishing_event_ref: EventRef`
- `recorder_status: RecorderStatus`

`MutationObservationRef`

- `observation_id: String`
- `recorded_event_ref: EventRef`
- `mutation_interval_id: String`
- `workspace_stream_id: String`
- `observation_kind: String`
- `touched_paths: Vec<String>`
- `source_file_version_ids: Vec<String>`
- `target_file_version_ids: Vec<String>`
- `reason_codes: Vec<String>`

`MutationIntervalRef`

- `mutation_interval_id: String`
- `workspace_stream_id: String`
- `workspace_instance_id: String`
- `pre_state_id: String`
- `post_state_id: String`
- `opened_event_ref: Option<EventRef>`
- `closed_event_ref: Option<EventRef>`
- `recorder_status: RecorderStatus`
- `causality_refs`

`CodeFactRef`

- `code_event_id: String`
- `event_type: String`
- `workspace_state_id: String`
- `cause_ref: CauseRef`
- `blob_refs: Vec<BlobDescriptorRef>`

`BlobDescriptorRef`

- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`

`ProvenanceQueryEnvelope`

- `execution_refs: Vec<ExecutionRef>`
- `activity_refs: Vec<ActivityRef>`
- `mutation_interval_refs: Vec<MutationIntervalRef>`
- `code_refs: Vec<CodeFactRef>`
- `blob_refs: Vec<BlobDescriptorRef>`
- `status: ProvenanceStatus`
- `sanitized_summary: Option<String>`

`BlobDescriptorRef` is the exact immutable descriptor pin that query clients may
feed back into `provenanceBlob/read.expected_descriptor_event_ref` without
requiring a fresh descriptor re-resolution step. Current availability remains a
separate query-time concern handled by blob manifest/read surfaces.

`RangeResolutionCandidate`

- `candidate_id: String`
- `lineage_revision_id: String`
- `resolved_workspace_stream_id: String`
- `resolved_workspace_instance_id: String`
- `resolved_workspace_state_id: String`
- `resolved_git_commit_oid: Option<String>`
- `resolved_git_tree_oid: Option<String>`
- `resolved_range: AnchoredCodeRange`
- `terminal_segments: Vec<ProjectedSegment>`
- `lineage_page: Option<LineagePage>`
- `envelope: ProvenanceQueryEnvelope`

`ReadRangeResponse.query_status` is the authoritative request-level resolution
status for the whole range query. A candidate envelope must describe the
resolved candidate only; its `status.query_status.selector_status` must
therefore be `Matched`.

`LineagePageEntry`

- `depth: u32`
- `child_hunk_id: String`
- `edge: HunkParentEdge`

`LineagePage`

- `entries: Vec<LineagePageEntry>`
- `next_cursor: Option<String>`

`LineagePage` is present only when `ReadRangeParams.include_lineage = true`.
When lineage is omitted, the response may still include terminal segments needed
to identify the immediate owning hunk, but it must not include parent traversal
edges.

`WorkspaceCustodySelector`

- `workspace_stream_id: String`
- `stream_epoch: u64`

### Query inputs

`provenanceWorkspaceStream/list`

- `ListStreamsParams`
  - `repo_scope_id: Option<String>`
  - `workspace_instance_id: Option<String>`
  - `include_closed: bool`
  - `cursor: Option<String>`
  - `limit: Option<u32>`
- `ListStreamsResponse`
  - `data: Vec<WorkspaceStreamDescriptor>`
  - `next_cursor: Option<String>`

`provenanceLedgerStream/list`

- `ListLedgerStreamsParams`
  - `ledger_scope: Option<LedgerScope>`
  - `include_closed: bool`
  - `cursor: Option<String>`
  - `limit: Option<u32>`
- `ListLedgerStreamsResponse`
  - `data: Vec<LedgerStreamDescriptor>`
  - `next_cursor: Option<String>`

`provenanceStreamEpoch/list`

- `ListStreamEpochsParams`
  - `ledger_scope: LedgerScope`
  - `stream_id: String`
  - `cursor: Option<String>`
  - `limit: Option<u32>`
- `ListStreamEpochsResponse`
  - `data: Vec<StreamEpochDescriptor>`
  - `next_cursor: Option<String>`

`provenanceLedgerStream/read`

- `ReadStreamDescriptorParams`
  - `ledger_scope: LedgerScope`
  - `stream_id: String`
- `ReadStreamDescriptorResponse`
  - `query_status: QueryResolutionStatus`
  - `ledger_stream_descriptor: Option<LedgerStreamDescriptor>`

If the requested stream id does not resolve, the response must set
`query_status.selector_status = Unavailable`,
`query_status.freshness = NotApplicable`, anchor the failure to
`StatusAnchor::ledgerStream`, and return `ledger_stream_descriptor = None`.
If it resolves, `selector_status = Matched` and the descriptor must be present.

`provenanceStreamHead/read`

- `ReadStreamHeadParams`
  - `selector: LiveWorkspaceSelector`
  - `stream_epoch: Option<u64>`

Rules:

- `provenanceStreamHead/read` is a workspace-custody convenience API for live editor and
  TUI resolution; generic head inspection for any ledger scope should use
  `provenanceLedgerStream/read` and `LedgerStreamDescriptor.head_event_ref`
- if `stream_epoch` is omitted, the server resolves the current open epoch for
  each matched workspace stream candidate
- callers that need the fact which established the current workspace state must
  use `workspace_state_event_ref`, not `head_event_ref`
- ambiguous or unavailable selector resolution for this RPC must anchor
  `query_status.status_anchor = liveWorkspaceSelector`

`StreamHeadCandidate`

- `workspace_stream_id: String`
- `workspace_instance_id: String`
- `stream_epoch: u64`
- `workspace_state_id: Option<String>`
- `head_event_ref: Option<EventRef>`
- `workspace_state_event_ref: Option<EventRef>`

`ReadStreamHeadResponse`

- `query_status: QueryResolutionStatus`
- `candidates: Vec<StreamHeadCandidate>`

`provenanceRevisionAlias/list`

- `ListRevisionAliasesParams`
  - `workspace_instance_id: Option<String>`
  - `git_commit_oid: Option<String>`
  - `git_tree_oid: Option<String>`
  - `include_superseded: bool`
  - `cursor: Option<String>`
  - `limit: Option<u32>`
- `ListRevisionAliasesResponse`
  - `data: Vec<RevisionAlias>`
  - `next_cursor: Option<String>`

`provenanceRevisionAlias/read`

- `ReadRevisionAliasParams`
  - `alias_id: String`
- `ReadRevisionAliasResponse`
  - `query_status: QueryResolutionStatus`
  - `revision_alias: Option<RevisionAlias>`

Direct-id semantics apply: if `alias_id` is unknown, return
`query_status.selector_status = Unavailable`,
`query_status.freshness = NotApplicable`, and `revision_alias = None`.

`provenanceSchema/read`

- `ReadSchemaBundleParams`
  - `schema_bundle_id: Option<String>`
  - `schema_bundle_digest: Option<String>`
  - `schema_version: Option<String>`
  - `export_contract_version: Option<String>`
- `SchemaBundleReadStatus`
  - `availability`
    - `Available`
    - `Unavailable`
    - `ConstraintMismatch`
  - `reason_codes: Vec<String>`
  - `status_anchor: StatusAnchor`
  - `selected_schema_versions: Vec<String>`
  - `selected_export_contract_version: Option<String>`
- `ReadSchemaBundleResponse`
  - `status: SchemaBundleReadStatus`
  - `schema_bundle: Option<TraceSchemaBundle>`

`schema_bundle = None` is allowed only when `status.availability` is
`Unavailable` or `ConstraintMismatch`.

`schema_bundle_id` and `schema_bundle_digest` are exact retrieval selectors for
an already-known bundle. If either is provided, the server must resolve that
exact bundle or return `ConstraintMismatch` / `Unavailable`; it must not
silently choose a different bundle by version.
If exact bundle selectors are provided, `selected_schema_versions` and
`selected_export_contract_version` must echo the resolved bundle rather than a
fresh version-based default selection.

If `export_contract_version` is omitted, the server deterministically selects
the latest locally supported export contract. If `schema_version` is omitted,
the returned bundle includes every schema version supported by the selected
export contract. `selected_schema_versions` and
`selected_export_contract_version` must echo the effective selection so the
response is self-describing.

`provenanceRange/read`

- `ReadRangeParams`
  - `selector: CodeRangeSelector`
  - `path: String`
  - `range: QueryLineRange`
  - `expected_range_fingerprint: Option<String>`
  - `include_lineage: bool`
  - `max_depth: Option<u32>`
  - `segment_limit: Option<u32>`
  - `lineage_candidate_id: Option<String>`
  - `cursor: Option<String>`

Rules:

- `range` is a `QueryLineRange`; line numbers are 1-based, inclusive, and
  non-empty
- `expected_range_fingerprint` is the digest of the exact requested range bytes
  in the resolved state
- mismatch returns `query_status.selector_status = ConstraintMismatch` with a
  specific reason code
- alias or historical selector resolution failure returns
  `query_status.selector_status = Unavailable`
- multi-candidate responses set `query_status.selector_status = Ambiguous`
- zero-candidate responses are valid when `query_status.selector_status` is
  `ConstraintMismatch`, `Unavailable`, or `Matched`
- `query_status.selector_status = Matched` with `candidates = []` means the
  selector resolved exactly, but no matching path/range exists in the resolved
  state; servers must use a specific reason code such as `pathNotFound` or
  `rangeOutOfBounds`
- range-query failures and constraint mismatches must anchor to
  `StatusAnchor.codeRangeRequest`, not only to the bare selector
- `cursor` paginates lineage edges for the candidate identified by
  `lineage_candidate_id`; it does not change selector resolution
- `lineage_candidate_id` is required whenever `cursor` is provided
- lineage pagination cursors must bind to the first-page candidate resolution:
  at minimum `candidate_id`, `resolved_workspace_state_id`, `requested_path`,
  `requested_range`, the selected authoritative `lineage_revision_id`,
  `max_depth`, and `segment_limit`
- if a cursor is replayed against a different candidate binding, the server must
  reject it with `query_status.selector_status = ConstraintMismatch` instead of
  silently re-resolving a fresh candidate
- `segment_limit` bounds immediate terminal segments per candidate, and
  `max_depth` bounds parent-edge traversal when `include_lineage = true`

`ReadRangeResponse`

- `query_status: QueryResolutionStatus`
- `requested_selector: CodeRangeSelector`
- `requested_path: String`
- `requested_range: QueryLineRange`
- `expected_range_fingerprint: Option<String>`
- `candidates: Vec<RangeResolutionCandidate>`

Top-level `ReadRangeResponse` does not carry a shared cursor. Lineage pagination
is candidate-scoped through `RangeResolutionCandidate.lineage_page.next_cursor`.
`ReadRangeResponse.query_status` is the authoritative request-level resolution
status. Candidate-level `ProvenanceQueryEnvelope` values describe only resolved
candidates and therefore must have
`status.query_status.selector_status = Matched`.
Only candidate records carry `resolved_range: AnchoredCodeRange`; the top-level
requested fields echo the caller's selector, path, range, and optional
fingerprint constraint even when selector resolution fails.

`provenanceTurn/read`

- `ReadTurnParams`
  - `session_id: String`
  - `thread_id: String`
  - `turn_id: String`
  - `include_intervals: bool`
  - `include_code_facts: bool`
- `TurnRef`
  - `session_id: String`
  - `thread_id: String`
  - `turn_id: String`
  - `execution_context: Option<ExecutionContext>`
  - `started_event_ref: Option<EventRef>`
  - `finished_event_ref: Option<EventRef>`
- `ReadTurnResponse`
  - `envelope: ProvenanceQueryEnvelope`
  - `turn: Option<TurnRef>`

Direct-id reads treat the requested id as an exact selector. If the primary
object is missing, the response must set
`envelope.status.query_status.selector_status = Unavailable`,
`envelope.status.query_status.freshness = NotApplicable`, anchor the failure to
the requested id, return the primary object as `None`, and return empty related
collections. If the primary object exists, `selector_status = Matched` and the
primary object must be present.

`provenanceHunk/read`

- `ReadHunkParams`
  - `hunk_id: String`
  - `include_parents: bool`
  - `include_evidence: bool`
- `ReadHunkResponse`
  - `envelope: ProvenanceQueryEnvelope`
  - `hunk: Option<HunkRecord>`
  - `parent_hunks: Vec<HunkRecord>`
  - `derived_terminal_segments: Vec<ProjectedSegment>`

`provenanceActivity/read`

- `ReadActivityParams`
  - `activity_id: String`
  - `include_children: bool`
  - `include_intervals: bool`
- `ReadActivityResponse`
  - `envelope: ProvenanceQueryEnvelope`
  - `activity: Option<TraceActivity>`
  - `child_activities: Vec<TraceActivity>`
  - `mutation_intervals: Vec<MutationIntervalRef>`

`provenanceMutationInterval/read`

- `ReadMutationIntervalParams`
  - `mutation_interval_id: String`
  - `include_evidence: bool`
  - `include_code_facts: bool`
- `ReadMutationIntervalResponse`
  - `envelope: ProvenanceQueryEnvelope`
  - `mutation_interval: Option<MutationIntervalRef>`
  - `workspace_transition: Option<WorkspaceTransitionRef>`
  - `mutation_observations: Vec<MutationObservationRef>`
  - `file_change_evidence: Vec<FileChangeEvidence>`

`provenanceWorkspaceTransition/read`

- `ReadWorkspaceTransitionParams`
  - `transition_id: String`
- `ReadWorkspaceTransitionResponse`
  - `envelope: ProvenanceQueryEnvelope`
  - `workspace_transition: Option<WorkspaceTransitionRef>`
  - `mutation_observations: Vec<MutationObservationRef>`

`provenanceMutationObservation/read`

- `ReadMutationObservationParams`
  - `observation_id: String`
- `ReadMutationObservationResponse`
  - `envelope: ProvenanceQueryEnvelope`
  - `mutation_observation: Option<MutationObservationRef>`

The direct-id read rule applies to `provenanceHunk/read`,
`provenanceActivity/read`, `provenanceMutationInterval/read`,
`provenanceWorkspaceTransition/read`, and
`provenanceMutationObservation/read` as well as `provenanceTurn/read`.

`ExportContinuityStatus`

- `expected_next_sequence: u64`
- `last_exported_sequence: Option<u64>`
- `last_exported_event_hash: Option<String>`
- `gap_detected: bool`
- `repair_required: bool`

Shared resume rules for both export surfaces:

- `cursor` is mutually exclusive with `after_sequence` and `after_event_hash`
- `after_sequence` and `after_event_hash` must either both be provided or both
  be omitted
- `after_sequence` is exclusive; export resumes at `after_sequence + 1`
- before resuming, the server must verify that the event at `after_sequence`
  has `after_event_hash`; mismatch returns
  `query_status.selector_status = ConstraintMismatch` with
  `event_hash_mismatch`, `data = []`, `next_cursor = None`, and no snapshot head
  fields
- if `snapshot_head_sequence` / `snapshot_head_event_hash` are provided, both
  must be provided and must match the fixed snapshot head established by the
  first page
- stateless resume with `after_sequence` / `after_event_hash` and no
  `snapshot_head_*` is a moving-tail high-watermark resume
- when no resume fields are provided, export starts from the first event in the
  requested epoch
- the first page establishes a snapshot head; subsequent pages returned via
  `cursor` must paginate against that fixed snapshot, not a moving live head
- every export cursor must bind the effective `export_contract_version` chosen
  on the first page; replaying a cursor under a different effective export
  mapping is a request validation error
- `snapshot_head_sequence` and `snapshot_head_event_hash` are populated only
  when `query_status.selector_status = Matched`. Selector mismatch or
  unavailable responses serialize both fields as `null` and return `data = []`
- `stream_epoch_descriptor` and `continuity_status` are populated only when the
  requested stream epoch resolved exactly
- when `query_status.selector_status = Matched`, `schema_bundle_ref` is
  mandatory and must identify the exact schema bundle needed to validate every
  row in `data`. It may be `null` only for selector mismatch or unavailable
  responses

`provenanceEvent/exportRawReplay`

- `ExportRawReplayEventsParams`
  - `ledger_scope: LedgerScope`
  - `stream_id: String`
  - `stream_epoch: u64`
  - `export_contract_version: String`
  - `cursor: Option<String>`
  - `limit: Option<u32>`
  - `after_sequence: Option<u64>`
  - `after_event_hash: Option<String>`
  - `snapshot_head_sequence: Option<u64>`
  - `snapshot_head_event_hash: Option<String>`

`RawReplayExportedTraceEvent`

- `kernel_event: TraceKernelEvent`
- `external_event_name: String`

`ExportRawReplayEventsResponse`

- `requested_stream_ref: LedgerStreamRef`
- `stream_epoch_descriptor: Option<StreamEpochDescriptor>`
- `query_status: QueryResolutionStatus`
- `export_contract_version: String`
- `required_schema_versions: Vec<String>`
- `schema_bundle_ref: Option<SchemaBundleRef>`
- `data: Vec<RawReplayExportedTraceEvent>`
- `next_cursor: Option<String>`
- `snapshot_head_sequence: Option<u64>`
- `snapshot_head_event_hash: Option<String>`
- `continuity_status: Option<ExportContinuityStatus>`
- `missing_blob_refs: Vec<String>`

`provenanceEvent/exportRawReplay` is the lossless append-only export surface.
It must return every durable event in the selected stream epoch that is visible
at or before the chosen raw replay snapshot head, including standalone events,
replayable batch participants, outcome markers, aborts, repairs, and
handoff/seal events. `export_contract_version` is mandatory on the request;
servers must reject omitted or unknown raw replay contract versions rather than
silently defaulting to the local newest mapping.

`ExportRawReplayEventsResponse` is the canonical lossless export artifact. Any
persisted raw export bundle must retain `requested_stream_ref`,
`export_contract_version`, and the paired `TraceSchemaBundle` or a durable
reference to it. `required_schema_versions` lists every
`kernel_event.schema_version` present in `data`; the paired bundle must cover
all of them. `schema_bundle_ref` may identify a separately fetched bundle, but
it must include the exact `schema_bundle_digest` that validators need to bind
exported events to schema content. Storing only the bare `data` vector is not a
valid lossless export format.

`provenanceEvent/exportCommitted`

- `ExportCommittedEventsParams`
  - `ledger_scope: LedgerScope`
  - `stream_id: String`
  - `stream_epoch: u64`
  - `export_contract_version: Option<String>`
  - `cursor: Option<String>`
  - `limit: Option<u32>`
  - `after_sequence: Option<u64>`
  - `after_event_hash: Option<String>`
  - `snapshot_head_sequence: Option<u64>`
  - `snapshot_head_event_hash: Option<String>`

`CommittedExportedTraceEvent`

- `kernel_event: TraceKernelEvent`
- `external_event_name: String`
- `batch_export_state`
  - `standalone`
  - `committed`
- `batch_outcome_terminus_event_ref: Option<EventRef>`

`ExportCommittedEventsResponse`

- `requested_stream_ref: LedgerStreamRef`
- `stream_epoch_descriptor: Option<StreamEpochDescriptor>`
- `query_status: QueryResolutionStatus`
- `export_contract_version: String`
- `required_schema_versions: Vec<String>`
- `schema_bundle_ref: Option<SchemaBundleRef>`
- `data: Vec<CommittedExportedTraceEvent>`
- `next_cursor: Option<String>`
- `snapshot_head_sequence: Option<u64>`
- `snapshot_head_event_hash: Option<String>`
- `continuity_status: Option<ExportContinuityStatus>`
- `missing_blob_refs: Vec<String>`

`provenanceEvent/exportCommitted` is the checkpointable committed projection.
It may default `export_contract_version` to the local latest supported mapping,
but it is not the lossless replay artifact. It exposes only finalized committed
recorder truth.

If the first committed-export page omitted `export_contract_version`, the
response's echoed effective version becomes part of the fixed snapshot context.
Cursor-based resume must reuse that bound version implicitly through the cursor.
Stateless fixed-snapshot resume (`after_sequence` / `after_event_hash` plus
`snapshot_head_*`) must supply that same effective `export_contract_version`
explicitly; otherwise the server must reject the resume rather than silently
remapping rows.

Replayable batch participants are committed-exportable only after their
stream-local outcome terminus is durable and visible to the exporter. Committed
export includes only participants from batches whose finalized outcome kind is
`committed`; aborted and repaired participants remain raw/forensic history and
must not be exposed as committed recorder truth. The committed exporter
computes the first page's snapshot head as the latest checkpointable event at
or before the live head, where no earlier replayable batch participant lacks a
visible stream-local outcome terminus. The committed exporter must not advance
`next_cursor`, `snapshot_head_sequence`, or `snapshot_head_event_hash` past
that checkpointable frontier. Exported participant rows include
`batch_export_state` and `batch_outcome_terminus_event_ref` so collectors can
verify that they checkpoint only finalized committed facts. In a writable
handoff batch, the old stream's terminus ref may therefore point to
`system.stream_sealed`. Compliant writers must keep any such stall at the
stream head by withholding later same-epoch visibility until the local outcome
terminus is durable.

For both export surfaces, `snapshot_head_sequence` and
`snapshot_head_event_hash` are resume assertions for continuing a previously
established export snapshot. They must be omitted on an initial page request
and may be supplied only together when resuming from a prior page's fixed
snapshot head.

`provenanceBlobManifest/read`

- `ReadBlobManifestParams`
  - `selector: WorkspaceCustodySelector`
  - `blob_ref_ids: Option<Vec<String>>`
  - `cursor: Option<String>`
  - `limit: Option<u32>`

Rules:

- `provenanceBlobManifest/read` is a workspace-custody blob API and does not accept other
  ledger scopes
- page 1 of a cursor-based manifest read must bind a stable snapshot of the
  requested epoch's visible descriptor set; later pages must read against that
  same snapshot rather than the live newest-visible set
- each returned `BlobManifestEntry` represents the unique visible
  `blob.descriptor_recorded` fact for that `blob_ref_id` in the requested
  workspace custody epoch, together with its exact descriptor `EventRef`
- `blob_ref_ids` mode is for fetching explicit descriptors by id
- `cursor` / `limit` mode is for paginating the stream-scoped manifest
- `blob_ref_ids` is mutually exclusive with `cursor` and `limit`
- every opaque `next_cursor` must bind the original selector, filters, sort
  order, and manifest snapshot watermark; reusing a cursor with changed request
  inputs is a request validation error
- each returned `BlobManifestEntry` must carry both the immutable
  `descriptor_event_ref` and the resolved `availability_event_ref` that made the
  entry's current availability authoritative in that snapshot

`ReadBlobManifestResponse`

- `selector: WorkspaceCustodySelector`
- `query_status: QueryResolutionStatus`
- `data: Vec<BlobManifestEntry>`
- `missing_blob_refs: Vec<String>`
- `snapshot_head_event_ref: Option<EventRef>`
- `next_cursor: Option<String>`

When `query_status.selector_status = Matched`, `snapshot_head_event_ref` is
mandatory and names the manifest snapshot frontier that every page in the same
cursor chain must use. It may be `null` only for selector mismatch or
unavailable responses.

`provenanceBlob/read`

- `ReadBlobParams`
  - `selector: WorkspaceCustodySelector`
  - `blob_ref_id: String`
  - `expected_descriptor_event_ref: Option<EventRef>`
  - `continuation_token: Option<String>`
  - `access_purpose: String`
  - `offset: u64`
  - `limit: u32`

Rules:

- `provenanceBlob/read` is a workspace-custody blob API and does not accept other ledger
  scopes
- blob read has two protocol phases:
  - initial resolution request: `continuation_token = None`
  - resumed session request: `continuation_token` is present and becomes the
    authoritative binding for the resolved descriptor, blob-read session, next
    offset, access purpose, and any echoed selector/blob identity fields
- the server derives `AccessRequester` from the actual caller and execution
  context; request payloads must not supply audit identity fields directly
- the server deterministically resolves the current writable `AccessAudit`
  stream for the local installation; callers do not choose audit-stream
  placement through the read RPC
- the exact authorization anchor for a resolved blob read is the selected
  `blob.descriptor_recorded` event for `(workspace_stream_id, stream_epoch,
  blob_ref_id)`; implementations must not choose between descriptor,
  availability, or workspace-state facts ad hoc
- the authorization event must also pin the resolved effective availability via
  `target_availability` and `target_availability_event_ref`; later
  availability changes must not retroactively change what availability fact
  justified the original authorization decision
- the first request in a logical blob-read session may omit
  `continuation_token`; if more bytes remain, the response must return a new
  opaque continuation token that binds selector, `blob_ref_id`,
  `descriptor_event_ref`, `availability_event_ref`, the audit session identity,
  next offset, access purpose, a visibility watermark for the resolved
  descriptor, and resumable digest-checkpoint state sufficient to continue
  cumulative delivery hashing without re-reading previously disclosed bytes
- when `continuation_token` is supplied, the server must resume that exact
  descriptor-pinned session; request fields that disagree with the token are
  request validation errors
- malformed or undecodable continuation tokens are request validation errors and
  must anchor `query_status.status_anchor = blobReadAttempt`
- before issuing a continuation token, the server must either encode the
  resumable digest state into that token or persist a digest checkpoint that
  the token can deterministically recover; resumed reads must not require
  re-reading already disclosed bytes merely to continue the cumulative digest
- a client that resumes from a previously returned `next_offset` must present
  the paired `continuation_token`; replaying bare selector + blob id + offset
  is an invalid request for continued disclosure
- when `continuation_token` is supplied, `offset` must equal the token-bound
  next offset
- `expected_descriptor_event_ref` is valid only for an initial resolution
  request and must be absent when `continuation_token` is supplied
- in v1, the first request with no continuation token selects the unique visible
  `blob.descriptor_recorded` event for that `blob_ref_id` in the requested
  workspace custody epoch
- after selecting that immutable descriptor, the server must resolve the
  effective current availability from the same snapshot and bind it with an
  exact `availability_event_ref`
- on an initial resolution request, if `expected_descriptor_event_ref` is
  provided, it must resolve to that exact selected descriptor event or the
  server returns `query_status.selector_status = ConstraintMismatch` with no
  content
- `limit` must be greater than zero and no larger than the server-advertised
  maximum chunk size
- `offset` is zero-based
- `offset` greater than the blob length is an invalid request, not a blob
  availability result
- `inlineChunk.byte_count` may be zero only when `offset` equals the blob
  length
- `inlineChunk.next_offset = None` means the returned chunk reached EOF
- `inlineChunk.continuation_token` is required whenever `next_offset` is
  present, and it must be absent at EOF
- `inlineChunk.blob_digest_verified = true` only when the server has validated
  the complete blob content against the descriptor digest
- the first resolved request in a logical blob-read session must append exactly
  one authorization event before any bytes are disclosed and return its
  `authorization_event_ref`
- `InlineBlobChunk`, `BlobUnavailableResult`, and `BlobAccessDeniedResult` must
  echo that session's `authorization_event_ref`
- delivery events are session-scoped checkpoints, not per-chunk requirements.
  The server must flush a delivery checkpoint before EOF and whenever a
  configured uncheckpointed-byte threshold would otherwise be exceeded; it may
  flush more often
- `delivery_event_ref` identifies the latest committed delivery checkpoint for
  the session and is optional on responses that disclose bytes before the next
  checkpoint boundary is reached
- no writable `AccessAudit` stream or authorization append failure returns
  `BlobAuditFailureResult` with no content and no access event refs
- successful disclosure must eventually record the actual delivered byte range,
  `delivered_byte_count_cumulative`, and
  `delivered_content_digest_cumulative` in one or more session delivery
  checkpoints; EOF must flush the final checkpoint before the terminal response
  is returned
- failed, truncated, or unavailable delivery attempts must commit a delivery
  checkpoint with the actual result before the terminal response is returned
- selector-level failures return `BlobSelectorFailureResult`; they do not append
  `access.blob_read_authorized` because no exact custody fact has been selected
- invalid offsets, cursor/token mismatches, or other blob-read parameter
  conflicts return `BlobRequestInvalidResult`
- initial-resolution descriptor mismatches anchor
  `query_status.status_anchor = blobReadAttempt` with
  `expected_descriptor_event_ref` populated and
  `continuation_token_fingerprint = None`
- request-validation failures before the server can recover one exact
  descriptor-pinned session anchor `query_status.status_anchor = blobReadAttempt`
- descriptor-pinned blob-read session failures must anchor
  `query_status.status_anchor = blobReadSession`

`InlineBlobChunk`

- `type = inlineChunk`
- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`
- `offset: u64`
- `byte_count: u32`
- `base64_content: String`
- `next_offset: Option<u64>`
- `continuation_token: Option<String>`
- `chunk_digest_verified: bool`
- `blob_digest_verified: bool`
- `authorization_event_ref: EventRef`
- `delivery_event_ref: Option<EventRef>`

`BlobUnavailableResult`

- `type = unavailable`
- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`
- `reason_code: String`
- `authorization_event_ref: EventRef`
- `delivery_event_ref: Option<EventRef>`

`BlobAccessDeniedResult`

- `type = accessDenied`
- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`
- `policy_reason_code: String`
- `authorization_event_ref: EventRef`

`BlobAuditFailureResult`

- `type = auditFailure`
- `audit_status: AuditFailureStatus`

`BlobSelectorFailureResult`

- `type = selectorFailure`

`BlobRequestInvalidResult`

- `type = invalidRequest`
- `reason_codes: Vec<String>`

`BlobReadResult`

- tagged union of `InlineBlobChunk`, `BlobUnavailableResult`,
  `BlobAccessDeniedResult`, `BlobAuditFailureResult`, and
  `BlobSelectorFailureResult`, and `BlobRequestInvalidResult`

`ReadBlobResponse`

- `query_status: QueryResolutionStatus`
- `result: BlobReadResult`

`ReadBlobResponse.query_status` is the single authoritative selector-resolution
status for blob reads. `BlobSelectorFailureResult` must not carry a second
`QueryResolutionStatus`. `BlobAuditFailureResult` must not duplicate
`reason_codes`; callers read audit failure reasons from
`audit_status.reason_codes`.

If `query_status.selector_status != Matched`, `result` must be
`BlobSelectorFailureResult`. Resolved-target variants are valid only when the
selector matched exactly one blob custody fact. If the selector matched but the
audit stream cannot be resolved or written, `result` must be
`BlobAuditFailureResult`, `query_status.selector_status` remains `Matched`, and
no bytes may be disclosed.
If the selector matched but the request parameters contradict the resolved blob
session or range contract, `result` must be `BlobRequestInvalidResult`,
`query_status.selector_status` remains `Matched`, and no bytes may be
disclosed.

Blob v1 uses chunked inline transfer only. It does, however, require opaque
continuation tokens for descriptor-pinned multi-chunk reads. Local file handles
or out-of-band streaming transports remain out of scope until a dedicated
transport defines redemption, expiry, range, and audit semantics.

### Query responses

Entity-resolving provenance query responses embed `ProvenanceQueryEnvelope` at
the resolved entity level. Range reads use a top-level `QueryResolutionStatus`
plus candidate-level envelopes; hunk, activity, turn, mutation interval,
workspace transition, and mutation observation reads embed the envelope
directly in the response. Discovery and transport-style APIs that do not
resolve recorder-backed entities may return `QueryResolutionStatus` plus their
typed payload instead:

- stream and epoch discovery
- event export
- blob manifest reads
- blob reads

Schema bundle reads return `SchemaBundleReadStatus` plus their typed payload.

Those APIs must still use `StatusAnchor` to bind failures to the exact selector,
stream, epoch, or blob reference they evaluated.

## External Mutations and Drift

Not every workspace mutation comes from Codex.

Examples:

- user edits outside Codex
- branch switch or checkout
- formatter or another process rewrites files between tool calls

Rules:

1. Reconcile workspace state before opening each mutation interval.
2. If drift is already present, record:
   - `code.unsupervised_workspace_mutation_observed` when the kernel can prove a
     Codex-owned write path bypassed exact ingress
   - `code.external_workspace_mutation_observed` for true outside drift
   before attributing new Codex writes.
   Those observations must mint a new recorded `workspace_state_id` and advance
   the workspace head before the next exact Codex write is attributed. Their
   resulting `WorkspaceTransition` uses `primary_cause = observationSet`, not a
   synthetic Codex write interval.
3. Revalidate after the interval closes.
4. If writes cannot be separated safely, surface `certainty = Ambiguous`.
5. If the workspace moves after capture but before exact query resolution,
   surface `freshness = Stale`.

## Testing Strategy

### Capture plane tests

- every mutating tool class opens a mutation interval
- `apply_patch`, shell, `js_repl`, unified exec, write-capable MCP/dynamic tools,
  and agent jobs all pass through the supervisor or are recorded as explicit
  unsupervised-Codex or external observations
- `ExecutorFileSystem` and supervised `CommandExecManager` paths participate in
  the exact custody boundary; app-server mutators that bypass them degrade to
  `unsupervisedCodexObservation`
- long-running process activities survive across `exec_command` and
  `write_stdin`
- idle long-running process drift defaults to ambiguous without an exact
  attribution source
- `write_stdin` creates exact child intervals only when `StructuredWriteTap` or
  `RuntimeWriteFence` attribution exists; otherwise it records process-level
  ambiguous drift
- `RuntimeWriteFence` only upgrades to exact attribution when the fence window
  also carries an external-writer exclusion guarantee
- child intervals are recorded for process-exit reconcile
- bulk mutation intervals for checkout and formatter rewrites
- `RevisionTransition`, `FormatterRewrite`, `GeneratorOutput`, and
  `OpaqueBulkRewrite` semantics are distinguished
- one multi-workspace tool/process write splits into one workspace-local
  `MutationInterval` / `WorkspaceTransition` per affected workspace
- single-writer gate behavior for Codex-controlled workspace mutations
- supervised exact-ingress paths stream immutable replay artifacts into
  `ExactArtifactStore` during the interval rather than deferring all exact
  capture work to close time

### Journal and store tests

- scoped stream and epoch ordering rules
- stream descriptor, epoch descriptor, and head discovery APIs
- hash-chain conflict handling
- `AppendBatch` commit, abort, repair, and idempotent retry semantics across
  multiple scopes
- `ReplayableAppendBatch` stream-local outcome termini in every participating
  stream, including the `system.stream_sealed` terminal-event exception for the
  old stream in writable handoff batches
- later same-epoch events in a stream do not become externally visible ahead of
  that stream's replayable-batch outcome terminus
- batch payloads carry explicit `StreamLocalBatchOutcome` entries rather than
  relying on positional `EventRef` arrays
- `system.append_batch_repaired` records raw repaired participants separately
  from later authoritative replacement facts
- closed `MutationInterval` facts and their establishing workspace-custody
  transition facts commit atomically in one `ReplayableAppendBatch`
- writable predecessor-stream seal and successor handoff facts commit in one
  `ReplayableAppendBatch`
- aborted append reservations use `AppendReservationRef` rather than fake
  `EventRef`s
- explicit broken-chain epoch descriptors without fabricated predecessor hashes
- `system.epoch_rotated` is the authoritative broken-chain fact and
  `system.chain_broken_recorded` is only supplemental diagnostic evidence
- `system.epoch_started` as the first event in genesis epochs and
  `system.epoch_rotated` as the first event in every non-genesis epoch
- successor streams opened with `UnknownPrevious(reason = streamHandoff)` and a
  following `system.stream_handoff_recorded` before custody facts
- `system.stream_claim_recorded` and `system.stream_registration_recorded`
  produce replayable export identity facts
- canonical event serialization
- `TraceSchemaBundle` validation keyed by `(schema_version, event_type)`
- versioned export-name mapping by `export_contract_version`
- export rows expose `external_event_name` and echo the effective
  `export_contract_version`
- default checkpointable export excludes aborted/repaired batch participants as
  committed truth
- atomic sequence allocation
- epoch rollover metadata
- old-stream `system.stream_sealed` and new-stream handoff verification
- strict `sealedPredecessor` versus `unsealedPredecessorRecovery` handoff-state
  validation, including rejection of unsealed recovery when the predecessor was
  still writable
- `Abandoned` and `UnknownClosure` handoff cases when an old stream cannot be
  sealed
- blob manifest and locator behavior
- idempotency retry behavior
- dedicated provenance DB migrations

### Projector tests

- exact hunk normalization
- edit-only island splitting
- immutable `FileChangeEvidence` readback for async projection
- delta/checkpoint workspace-state replay without full synchronous path-map
  capture
- exact `git_tree_oid`-backed path replay for large `RevisionTransition`s
  without synchronous full-path-map inlining
- budget-driven large rewrites still close with an exact immutable truth source,
  using delta externalization, manifest checkpoints, or
  `filesystemDeltaSnapshotState` rather than deferred truth recovery
- truth-first slow-path close records telemetry/metrics distinct from bounded
  close and can be alerted on without weakening exactness semantics
- `ExactArtifactStore` deduplicates Git promotion, filesystem snapshots, path
  deltas, and working-tree evidence under one retention model
- `checkpointCompaction` promotes already-captured exact truth into faster query
  structures without changing recorder truth or becoming a second truth source
- exact Git-backed states require durable `git_tree_oid` inputs, not `hashOnly`
  references
- promoted Git objects are deduplicated, retained while any surviving retention
  root still depends on them, including local-window exact states/file
  versions, projector inputs, persisted exports, active blob-read sessions, and
  explicit archival retention, and can be reclaimed only after those roots are
  gone or migrated to replay-preserving cold archive storage
- multi-parent logical states require `path_replay_parent_workspace_state_id` or
  checkpoint promotion before path replay is valid
- file/entity-scoped projection DAG dependency blocking and pending/partial
  propagation without blocking unrelated files
- file entity preserved, renamed, recreated, ambiguous boundaries
- directed file entity edge records
- hunk parent/input/output graph traversal
- `HunkParentEdge` stable ordering, acyclicity, partition, and ambiguous
  downgrade rules
- `weightedOverlap` edges with explicit overlap weight
- range projection validity ranges
- replay projection rebuild from `TraceKernelEvent`, `BlobDescriptor`, and
  available blob/manifest content, then compare query results against the live
  index
- repair replay chooses authoritative lineage from append-only supersession, not
  from mutable projection rows alone
- revision-level lineage authority comes from `code.projection_job_repaired`,
  while `system.repair_applied` remains diagnostic/system context
- authoritative transitions serialize one canonical cause shared by
  `WorkspaceTransition.primary_cause` and `causality_refs.caused_by`
- hunk lineage never reuses `hunk_id` across revisions
- started compaction/repair jobs may terminate as `superseded` at safe
  interruption boundaries
- replay with missing or redacted blobs returning `coverage = Unavailable`
- bootstrap prehistory

### Query tests

- live range queries
- live workspace selector resolution from stream, instance, or repo scope without
  requiring a caller-provided `workspace_state_id`
- historical Git range queries
- ambiguous workspace selector cases
- selector constraint mismatch and stale live-drift cases
- stale live reads bind one concrete workspace state before revalidation and do
  not silently retry against a newer state
- unavailable historical revision cases
- activity and mutation interval inspection
- workspace transition and mutation observation direct-id inspection
- `provenanceTurn/read` returns a primary `TurnRef`, not only an envelope
- `CodeRangeSelector` one-of grammar and fully qualified moving-ref resolution
- list cursors reject filter rebind
- stream, epoch, and head discovery reads
- revision alias discovery and direct reads by `alias_id`
- `current_workspace_state_event_ref` remains distinct from `head_event_ref`
- ambiguous `provenanceRange/read` multi-candidate responses
- `provenanceRange/read` exact selector with `pathNotFound` /
  `rangeOutOfBounds` returning `selector_status = Matched` and `candidates = []`
- candidate-scoped lineage pagination without a shared top-level cursor
- lineage cursors reject rebind to a different candidate/state/revision
- `provenanceEvent/exportRawReplay` and `provenanceEvent/exportCommitted`
  resume hash mismatches returning
  `query_status.selector_status = ConstraintMismatch`
- chunked blob `inlineChunk`, `unavailable`, and `accessDenied` responses
- blob `auditFailure` responses for audit-stream resolution or append failures
- blob `selectorFailure` responses for selector-level failures
- blob `invalidRequest` responses for offset/token/request mismatches
- initial blob reads may use `expected_descriptor_event_ref`, while resumed
  reads must reject it and treat token/request disagreement as `invalidRequest`
- blob manifest entries carry exact descriptor event refs
- blob manifest cursor pages stay bound to one snapshot head
- chunk boundary rules for zero-length EOF reads, out-of-range offsets, and
  final-chunk digest verification
- authorization refs returned for every resolved-target blob read result, and
  delivery refs reflect the latest committed session checkpoint
- multi-chunk blob reads pin to one exact descriptor event ref through opaque
  continuation tokens
- `ReadBlobResponse.query_status` is the only authoritative selector status for
  blob reads
- access-audit payloads resolve to exact authorizing custody `EventRef`s
- blob reads derive `AccessRequester` from the real caller rather than request
  payload fields
- blob delivery checkpoints are cumulative over the logical read session
- descriptor-pinned blob-read session failures anchor to `StatusAnchor.blobReadSession`
- blob audit sessions amortize delivery checkpoints without disclosing bytes
  past the configured uncheckpointed threshold
- finished blob-audit sessions may compact intermediate checkpoints only after
  policy-defined durability conditions while retaining session authorization and
  final retained delivery proof
- committed export blocks only at the stream head, and metrics expose the age of
  any head-blocking replayable batch until finalize/repair completes
- raw-content access-audit payloads record byte range and delivered content
  digest when disclosure succeeds
- deterministic server-selected audit-stream resolution
- deterministic range-fingerprint hashing for exact text ranges
- exact schema bundle retrieval by `schema_bundle_id` / digest
- `StatusAnchor` coverage for stream, epoch, and blob selector failures

### Drift and repair tests

- external edit before a mutation interval starts
- external edit during an active interval
- branch switch while indexing is pending
- failed bulk index repair
- hash-chain conflict causing epoch rotation
- dependent projection repair after predecessor failure

## Incremental Delivery

### Phase 0: Formal Contracts

- define identity model and epoch rules
- define stream descriptor, epoch descriptor, and stream head DTOs
- define `EventRef`, `LedgerScope`, and scoped ledger DTOs
- define execution and code event taxonomy
- define `TraceActivity`, `MutationInterval`, `WorkspaceTransition`,
  `MutationObservation`, `CauseRef`, and `FileChangeEvidence`
- define `CauseRef` as a strict tagged union plus event-family/cause
  compatibility rules
- define `WorkspaceTransitionRef`, `MutationObservationRef`, and
  `BlobManifestEntry`
- define canonical blob contract
- define `ExactArtifactStore` and exact-artifact retention rules
- define orthogonal status model
- define hash-chain canonical serialization and idempotency semantics
- define exact app-server DTOs
- define tagged union wire discriminators and schema generation requirements
- define `TraceSchemaBundle`, payload registry entries, and
  `export_contract_version` mapping tables
- define `SchemaBundleReadStatus`, `TurnRef`, `RevisionAlias`, and
  candidate-scoped lineage pagination DTOs

Any phase that changes app-server v2 DTOs must update generated schemas and
fixtures, including TypeScript bindings, and run the protocol crate tests.

### Phase 1: Durable Journal

- define dedicated provenance store schema
- implement provenance DB migrations
- implement stream creation and epoch rollover
- implement global execution, workspace custody, and access-audit streams
- implement `system.epoch_started` and `system.epoch_rotated` opening events for
  every ledger scope
- implement `ReplayableAppendBatch` durable per-stream outcome-terminus
  semantics for cross-scope appends and same-batch references
- enforce head-only visibility blocking for replayable batches until each
  participating stream's local outcome terminus is durable
- make each closed `MutationInterval` + establishing `WorkspaceTransition`
  commit atomically when they span execution and workspace custody scopes
- implement stream head updates and epoch descriptor reads
- implement atomic sequence allocation and event append
- implement old-stream seal and new-stream handoff events
- make writable predecessor seal + successor handoff one replayable batch
- enforce the `sealedPredecessor` / `unsealedPredecessorRecovery` handoff state
  machine
- implement broken-chain records for repair/conflict rollover
- implement blob descriptor and local locator records

### Phase 2: Capture Foundation

- add `MutationSupervisor`
- classify all write-capable tool paths
- split multi-workspace writes into one workspace-local transition per workspace
- move exact provenance coverage to shared write ingress such as
  `ExecutorFileSystem` and supervised `CommandExecManager`
- treat `CommandExecManager` as supervised only when writes route through
  enforceable write fences or a structured execution shim; ordinary unfenced
  process execution records process-level custody and may still produce
  ambiguous workspace drift
- mint kernel-owned `process_activity_id` values for command execution and keep
  client process ids as metadata
- degrade unsupervised app-server file/command mutators to
  `unsupervisedCodexObservation` until they route through the shared ingress
- add process-backed activities and child mutation intervals
- add interval-scoped workspace single-writer gate for Codex-controlled mutators
- add provenance baseline service
- record immutable file-change evidence
- implement write-through exact artifact capture for supervised exact-ingress
  mutators
- persist execution and coarse code facts synchronously
- implement `provenanceTurn/read`, `provenanceActivity/read`, and
  `provenanceMutationInterval/read` for captured execution and interval facts
- implement `provenanceWorkspaceTransition/read` and
  `provenanceMutationObservation/read`

Do not promise hunk or range queries before this phase is complete.

### Phase 3: Git-First Code Projection

- add Git-backed workspace states
- add delta/checkpoint workspace-state path-map replay
- define `ExactGitState(git_tree_oid)` as an authoritative exact manifest source
- implement `ExactArtifactStore` retention, dedupe, and GC before exact
  Git-backed or filesystem-backed states depend on unreachable/local artifacts
- make local retention window, archive migration, and replay-preserving
  cold-tier policy explicit before exact artifact growth is relied on in
  production
- add bootstrap prehistory
- add file version and file entity edge model
- add directed file entity edge records
- add hunk normalization
- add hunk parent/input/output graph
- add append-only lineage revision supersession for repair
- disallow `hunk_id` reuse across lineage revisions and add optional logical
  equivalence identity only if later needed
- add minimal projected segment and file/entity-scoped predecessor projection
  index needed by the projection DAG
- add revision alias creation
- add projection DAG jobs for bulk mutations and dependent intervals
- add `filesystemDeltaSnapshotState` sparse exact fallback capture for
  budget-limited non-Git rewrites
- reserve `filesystemSnapshotState` for bootstrap/import/explicit full
  checkpoints only
- add `checkpointCompaction` as checkpoint-promotion/query-acceleration work
  over already-captured exact truth
- add `superseded` projector-job terminal status for overtaken started jobs

### Phase 4: Range Queries

- extend projected segment index for query performance
- implement `provenanceRange/read` and `provenanceHunk/read`
- implement `provenanceRevisionAlias/list` and `provenanceRevisionAlias/read`
- implement exact one-of selector semantics
- implement live workspace selector resolution and multi-candidate responses
- implement `QueryLineRange`, lineage pagination, and stale-live-state binding

### Phase 5: Export and Repair

- add `provenanceSchema/read`, `provenanceEvent/exportRawReplay`,
  `provenanceEvent/exportCommitted`, blob manifest, and chunked blob reads
- add exact access-audit authorization refs and blob selector-failure responses
- add snapshot-bound blob manifest pagination
- add initial-resolution versus resumed-session blob protocol with token-owned
  descriptor/session pinning across chunked reads
- add session-scoped blob audit checkpoints
- make blob delivery checkpoints cumulative and descriptor-session anchored
- add export rows with explicit external event names and effective contract
  version
- add registered stream handoff and local claim export
- add repair workflows for stale workspaces and failed projector jobs
- add typed execution-context passthrough for Forgeloop launchers

## Key Decisions

- primary recording primitives: trace activity and mutation interval
- primary truth source: scoped hash-chained trace ledger
- primary ledger scopes: global execution, workspace custody, and access audit
- primary historical coordinate system: Git
- primary v1 line-level scope: Git-backed workspaces only
- primary query model: live workspace selectors, recorded state selectors, and
  exact Git selectors with multi-candidate ambiguity
- primary status model: selector status, freshness, certainty, coverage,
  indexing
- primary export shape: stable stream-based event journal
- thread and turn summaries are projections, not the truth source

## Recommendation

Proceed with a Git-first trace kernel, not a diff-centered provenance feature.

The best architecture is:

- a mandatory capture plane in `codex-core`
- a dedicated provenance journal and projector store
- scoped ledger streams for execution, workspace custody, and access audit
- a dual-track execution and code event model
- a stable mcodex kernel event namespace with export-layer mapping when needed
- explicit causality over activities, intervals, baselines, external
  observations, projector jobs, alias resolution, and repairs
- a separate provenance app-server surface
- an export contract designed for future Forgeloop ingestion from day one

This is a larger architectural move than extending `TurnDiffTracker`, ghost
snapshots, or `thread/read`, but it is the path that best preserves
chain-of-custody and avoids future rework.
