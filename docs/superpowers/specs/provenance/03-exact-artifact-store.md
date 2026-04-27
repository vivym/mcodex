# Exact Artifact Store

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

## Workspace State Records

The kernel does not reason directly over "the current filesystem." It reasons
over recorded workspace states.

## Workspace state classes

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

## Exact artifact store

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
  `ExactArtifactStore` under a replay-preserving non-local `storage_class`
  (`exportedBundle` or `coldArchived`). Archive migration may move bytes to
  colder backing storage or retained export media, but it must not introduce a
  second untyped locator model or silently break the replay contract when the
  local retention window expires
- hot indexes, tree-entry caches, and other derived acceleration structures are
  not retention roots and may be evicted without weakening exact replay
- supervised exact-ingress paths should publish replay artifacts into
  `ExactArtifactStore` incrementally during the mutation interval so the close
  path can seal refs and manifests instead of re-reading already captured bytes
- `filesystemDeltaSnapshotState` is the fallback when exact write-through
  capture is unavailable or when an opaque write path still needs an immutable
  sparse snapshot at close time; it is not the preferred capture mode for
  structured first-party mutators

## Baselines

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

## File Identity Records

The kernel must distinguish "a file state version exists" from "continuity of
the same file entity is proven."

## Required identity objects

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
  surviving retention root or replay-preserving non-local `ExactArtifactStore`
  tier still preserves exact replay
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
