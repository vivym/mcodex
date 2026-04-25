# Operability and Testing

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

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
- slow-path telemetry includes operator-configurable threshold surfaces for
  frequency/age alerting, and recurring fallback on structured first-party
  mutators is detectable as an ingress-coverage gap
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
  gone or migrated to replay-preserving non-local `ExactArtifactStore` storage
  classes such as `exportedBundle` or `coldArchived`
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
- committed export blocking also exposes operator-configurable finalize/repair
  SLA threshold surfaces, not only raw blocking-age telemetry
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


