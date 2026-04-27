# Execution and Workspace Capture

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

## Capture and Mutation Observation Contract

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

### Tool classes

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

### Exact custody boundary

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

### Activity and interval rules

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

### Long-running process rules

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

### Bulk mutation rules

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
