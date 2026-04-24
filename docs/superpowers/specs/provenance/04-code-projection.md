# Code Projection

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

## Projection Contract

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

## Revision Alias Records

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

