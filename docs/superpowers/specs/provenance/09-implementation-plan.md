# Implementation Plan

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

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
- expose slow-path telemetry plus operator-configurable frequency/age threshold
  surfaces, and mark recurring fallback on structured first-party mutators as
  an ingress-coverage gap
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
  non-local `ExactArtifactStore` policy explicit before exact artifact growth is
  relied on in production
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
- expose committed-export head-blocking age plus operator-configurable
  finalize/repair SLA threshold surfaces
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
