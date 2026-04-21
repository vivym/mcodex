# Codex Provenance Kernel Design

This document defines the mcodex-side provenance system for tracing how code at
an anchored file and line range was introduced and evolved across Codex
activity.

The primary job of mcodex is to be a local provenance kernel:

- capture code mutation facts during Codex execution
- build hunk-level lineage for workspace and Git revision queries
- expose app-server APIs for local and external consumers
- persist enough structured evidence to support future export into Forgeloop's
  trace/evidence plane

mcodex should not become the online postmortem platform. Forgeloop owns actor
timelines, incident objects, repo-level SaaS aggregation, cross-user
authorization, raw-content approval, retention policy, and postmortem
snapshots.

## Summary

Build a workspace-scoped, hunk-based provenance kernel with these properties:

1. Record structured mutation observations during a turn and group them into
   turn-level change sets.
2. Use anchored code coordinates as the primary query surface:
   - `workspace anchor + file + line/range` for live workspace queries
   - `git code_ref + file + line/range` for historical revision queries
3. Treat hunks, not symbols or individual lines, as the primary stored unit of
   provenance.
4. Maintain a versioned live segment projection for each tracked file so a
   current range can resolve to terminal hunks and walk backward through
   lineage.
5. Prefer explicit `ambiguous`, `partial`, `stale`, or `unavailable` states
   over silent misattribution.
6. Persist canonical, exportable provenance observations so a future Forgeloop
   collector can upload the same facts without reinterpreting local SQLite
   internals.
7. Expose provenance through app-server v2 APIs, with no dedicated TUI workflow
   in the first release.

Recommended implementation:

- add a `codex-provenance` crate for models and lineage algorithms
- persist normalized local state in the existing SQLite state database
- add a local append-only provenance event stream for export compatibility
- integrate recording into core via a dedicated baseline and observation flow
- expose query APIs through app-server v2

## Goals

- Answer: "How did the logic at this anchored code coordinate get here?"
- Cover all Codex-driven file mutations, including `apply_patch`, shell
  commands, `js_repl`, and future mutating tools.
- Preserve enough conversation and tool context for external systems to build a
  postmortem trail without reassembling raw rollout items by hand.
- Keep the Codex side language-agnostic; external systems own symbol, issue,
  incident, and actor semantics.
- Support historical lookups for Git-backed workspaces when the selected
  revision can be mapped exactly to a recorded provenance state.
- Provide exportable canonical observations that can map cleanly into
  Forgeloop's future `TraceEvent`, `TraceSpan`, `TraceBlobRef`,
  `WorkspaceState`, and `CodeRangeProvenanceIndex` concepts.
- Reuse existing app-server and state infrastructure where it improves
  operability and testability.

## Non-Goals

- Do not make `git blame` the primary provenance mechanism.
- Do not require or implement symbol parsing inside Codex in the first release.
- Do not build a dedicated TUI postmortem experience in the first release.
- Do not backfill historical rollouts or pre-existing repository history.
- Do not make incident, issue, PR, employee, or performance objects first-class
  provenance keys inside Codex.
- Do not implement SaaS upload, tenant authorization, raw-content approvals, or
  cloud retention policy inside mcodex v1.
- Do not attempt repository-wide semantic equivalence tracking across refactors.

## Relationship to Forgeloop

Forgeloop's trace/evidence plane is the eventual cloud consumer. mcodex should
produce high-fidelity local facts that Forgeloop can ingest later.

### mcodex owns

- local workspace stream identity
- turn and tool source context
- workspace baseline and post-state anchors
- mutation observations
- hunk lineage
- live segment projection
- revision alias mapping for exact historical lookups
- local app-server query APIs
- local export journal entries and blob/content digests

### Forgeloop owns

- SaaS tenant and repo enrollment
- actor and actor identity mapping
- ExecutionPackage / RunSession / Review / Release / Incident objects
- cross-user repo aggregation
- raw-content entitlement and audit
- retention, legal hold, redaction, and cloud blob lifecycle
- incident/postmortem snapshots
- organization learning workflows

### Compatibility Contract

mcodex should not know Forgeloop object IDs in the core model. It should expose
source context and stable content/revision anchors so a connector can later
link facts to Forgeloop objects.

Every exported provenance event must include:

- stable event id
- local workspace stream id
- monotonically increasing local sequence
- schema version
- event hash and previous event hash
- source context: thread, turn, item, tool call when available
- workspace state anchors
- content digests and blob references
- hunk ids and lineage edges for code provenance facts

This is an export compatibility contract, not a requirement to build SaaS sync
inside mcodex v1.

## Why This Is Not a `git blame` Feature

`git blame` answers: "Which commit last touched this line?"

Postmortem-grade Codex provenance needs to answer:

- which user request initiated the logic
- which assistant response and tool calls produced it
- which intermediate tool-level edits happened before any commit existed
- which later Codex turns reshaped the logic
- whether a concurrent or external writer made the attribution ambiguous

The correct primitive is turn-aware hunk lineage, not commit blame.

## Why Git Still Matters

Git is still required, but for a different job.

- Git is the historical code coordinate system.
- Provenance is the causal record of how those bytes were produced.

The valid coordinate forms are:

- live workspace query:
  - `workspace_root + projection anchor + path + line/range`
- historical query:
  - `git code_ref + path + line/range`

Historical range queries cannot be keyed by bare `file + line/range`. The same
path and line numbers can refer to different logic on different branches or
commits.

## Why Codex Should Not Parse Symbols in v1

Symbol parsing is valuable for ergonomics, but it is not the right truth source
for mcodex v1.

- Dedicated systems can resolve symbols to ranges with language-aware indexes.
- Codex already has strong file and diff primitives but no general
  multi-language symbol infrastructure.
- Pushing symbol parsing into Codex couples the kernel to parser and grammar
  maintenance without improving the provenance truth model.

Codex should return high-fidelity range and hunk lineage. External systems can
layer symbol semantics on top.

## Current Context

Useful existing building blocks:

- turn-scoped diff tracking in `core/src/turn_diff_tracker.rs`
- live `TurnDiff` emission from `core/src/codex.rs`
- persisted thread history reconstruction in `app-server-protocol`
- existing `thread/read` surfaces for turn and item history
- existing SQLite-backed local state infrastructure in `codex-rs/state`
- ghost snapshot machinery that can capture a turn baseline in a Git repo
- existing git metadata and diff utilities in `codex-rs/git-utils`

Current gaps:

1. `TurnDiff` is a unified diff string and is not persisted as structured
   lineage.
2. `ThreadItem::FileChange` is a client-facing projection, not a normalized
   provenance model.
3. The current turn diff tracker is wired mainly through `apply_patch`; shell
   and `js_repl` paths can mutate files without passing through the same
   tracker.
4. Existing history is thread-oriented, while provenance must aggregate across
   multiple Codex threads operating on the same workspace.
5. A single net `turn start -> turn end` diff loses tool-level intermediate
   changes and cannot safely disambiguate external mutations while a turn is in
   flight.
6. Bare `file + line/range` is not a complete historical coordinate.

## Architecture

### 1. Scope Provenance by Workspace

The primary query is not "what happened in this thread?" but "how did this code
in this workspace evolve?"

Define a workspace scope as:

- the canonical Git worktree root when inside a Git worktree
- otherwise, the canonical current working directory

Each change set is attached to exactly one workspace scope and may reference a
`thread_id` and `turn_id` when the source was a Codex turn.

One Codex turn may emit multiple workspace-scoped change sets if it mutates
files under multiple canonical workspace roots. A single change set must not
span multiple workspaces.

Each workspace owns:

- a stable `workspace_id`
- a stable local `workspace_stream_id`
- a monotonic `projection_version`
- a local append-only provenance sequence

All lineage and live segment updates for a workspace must happen through a
serialized workspace transaction:

1. read current projection state
2. verify expected base projection version
3. apply mutation observation
4. write normalized records and export event envelope
5. advance projection version

If the expected base version does not match, the recorder must not guess. It
must write an explicit ambiguous result and mark the workspace stale until
reconciled.

### 2. Add `codex-provenance`

Do not grow `codex-core` for the provenance algorithms.

Add `codex-provenance` with:

- normalized provenance model types
- canonical event payload types
- diff and hunk normalization
- lineage graph construction
- live segment projection updates
- query result assembly helpers
- export event envelope helpers

Recommended ownership split:

- `codex-provenance`
  - pure models and lineage algorithms
  - canonical event payload schemas
  - no app-server protocol dependency
- `codex-core`
  - turn lifecycle integration
  - baseline capture orchestration
  - mutation observation boundaries
- `codex-state`
  - SQLite schema, migrations, persistence, export journal
- `codex-app-server-protocol`
  - v2 request and response types
- `codex-app-server`
  - query handlers and request routing

### 3. Capture Provenance Baselines

The kernel needs a stable "before" view for every tracked turn. Ghost snapshots
are good prior art, but provenance should not depend on undo being enabled.

Add a provenance-specific baseline flow:

- name: `ProvenanceBaselineTask` or equivalent
- trigger: every turn when provenance is enabled
- implementation: reuse Git snapshot machinery where possible
- output:
  - workspace root
  - baseline snapshot identifier
  - baseline Git tree identity when available
  - base projection version
  - baseline content digest manifest
  - capture status

If inside a Git worktree, the baseline should be a snapshot commit or stable
snapshot ref produced from the current worktree state.

If outside Git, v1 should degrade cleanly:

- mark provenance unavailable for that turn
- do not fail the user turn
- return an explicit availability reason from provenance APIs

For Git-backed workspaces, each baseline and post-observation state should be a
resolvable `WorkspaceStateRef`:

- `workspace_state_id`
- `projection_version`
- `snapshot_ref`
- `git_tree_oid`
- `git_commit_oid` when the state corresponds exactly to a real commit
- state content digest

### 4. Seed Bootstrap Prehistory

The first successful baseline in a workspace must seed bootstrap provenance for
already-existing text files.

This does not backfill old turns. It creates a synthetic prehistory layer so:

- unchanged legacy lines have a terminal segment
- the first recorded replacement of legacy code has a parent
- queries can distinguish `BootstrapPrehistory` from `NoData`

Bootstrap applies only to the workspace state first seen after provenance is
enabled. It does not create full historical lineage for arbitrary old Git
revisions.

### 5. Observe Mutations at Tool Boundaries

The recorder must sit below the current live UI diff projection and must not
depend on `TurnDiffTracker` being complete.

Recommended flow:

1. Capture turn baseline and `base_projection_version` at turn start.
2. Before each mutating tool observation, reconcile actual workspace state
   against the projected workspace head.
3. If drift exists before the tool starts, synthesize
   `ExternalWorkspaceMutation` or mark stale before attributing any new Codex
   change.
4. Around each mutating tool boundary, capture a `MutationObservation` from
   pre-observation state to post-observation state.
5. Apply that observation inside a serialized workspace transaction.
6. If projection version advanced unexpectedly, or the post-state cannot be
   reconciled, write an ambiguous observation and stop applying lineage for
   that workspace until repair occurs.
7. At turn completion, persist a turn envelope that references ordered
   observations and a derived net summary.
8. If a post-state exactly matches a real commit or later resolves to one,
   persist a revision alias so historical `code_ref` lookups can attach to the
   recorded state without heuristic matching.

This covers:

- `apply_patch`
- shell commands that edit files
- `js_repl` edits
- future mutating tools

while preserving tool-level intermediate changes.

## Canonical Local Event Model

SQLite tables are the local query store. An append-only local event stream is
the source of export compatibility and replayable projection updates.

### LocalProvenanceEvent

```text
LocalProvenanceEvent
- event_id
- workspace_id
- workspace_stream_id
- sequence
- schema_version
- event_type
- occurred_at
- recorded_at
- event_hash
- previous_event_hash
- source_context
- payload
- blob_refs
```

Sequence rules:

- sequence is gapless per `workspace_stream_id`
- same sequence + same event hash is idempotent
- same sequence + different event hash marks the stream conflicted
- `previous_event_hash` detects local stream forks

This is local correctness machinery. SaaS enrollment, stream token, tenant
validation, raw access approval, and cloud retention remain Forgeloop concerns.

### SourceContext

```text
SourceContext
- source_system: codex
- thread_id
- turn_id
- item_id
- tool_call_id
- tool_name
- command_id
- workspace_root
- workspace_state_id
- projection_version
- git_commit_oid
- git_tree_oid
```

### BlobRef

Large content should be referenced by digest rather than inlined:

```text
BlobRef
- blob_ref_id
- digest_algorithm: sha256
- expected_digest
- byte_size
- content_kind
- local_storage_ref
- availability: available | pending | missing | redacted | expired
- required_for_reconstruction
```

In mcodex v1, blob lifecycle is local:

- `available` means local state can read it
- `pending` means event is recorded but content has not been persisted yet
- `missing` means reconstruction/query must return `Partial` or `Unavailable`
- `redacted` and `expired` are reserved for future export/cloud consumers

### Canonical Event Types

Required event types:

- `workspace.bootstrap_seeded`
- `workspace.state_created`
- `mutation.observed`
- `file.delta_observed`
- `hunk.observed`
- `revision.alias_created`
- `projection.applied`
- `projection.marked_stale`
- `provenance.marked_ambiguous`
- `external_mutation.observed`

These event types must be stable enough to export. Internal SQLite table names
may evolve, but event payload semantics should not churn casually.

## Normalized Model

### WorkspaceProvenanceChangeSet

- `workspace_id`
- `workspace_stream_id`
- `source`
  - `CodexTurn`
  - `ExternalWorkspaceMutation`
  - `BootstrapPrehistory`
- `thread_id: Option<String>`
- `turn_id: Option<String>`
- `started_at`
- `completed_at`
- `workspace_root`
- `turn_baseline_ref: WorkspaceStateRef`
- `base_projection_version`
- `final_projection_version: Option<i64>`
- `user_excerpt: Option<String>`
- `assistant_excerpt: Option<String>`
- `tool_refs: Vec<ToolRef>`
- `observations: Vec<MutationObservation>`
- `derived_net_summary: Vec<FileNetSummary>`

### MutationObservation

- `observation_id`
- `workspace_id`
- `workspace_stream_id`
- `thread_id: Option<String>`
- `turn_id: Option<String>`
- `tool_ref: Option<ToolRef>`
- `source_kind`
  - `CodexTool`
  - `ExternalWorkspaceMutation`
  - `BootstrapPrehistory`
- `attribution_status`
  - `Attributed`
  - `Ambiguous`
  - `Partial`
  - `Unavailable`
- `base_projection_version`
- `applied_projection_version: Option<i64>`
- `pre_state_ref: WorkspaceStateRef`
- `post_state_ref: WorkspaceStateRef`
- `files: Vec<FileChangeSet>`
- `event_id`

### WorkspaceStateRef

- `workspace_state_id`
- `projection_version: Option<i64>`
- `snapshot_ref: Option<String>`
- `git_tree_oid: Option<String>`
- `git_commit_oid: Option<String>`
- `content_digest: Option<String>`
- `required_blob_refs: Vec<BlobRefId>`

### WorkspaceStateDelta

- `workspace_state_id`
- `parent_workspace_state_id: Option<String>`
- `path_before: Option<PathBuf>`
- `path_after: Option<PathBuf>`
- `change_kind`
  - `Add`
  - `Update`
  - `Delete`
  - `Rename`
  - `ModeChange`
  - `Ambiguous`
- `before_content_digest: Option<String>`
- `after_content_digest: Option<String>`
- `before_blob_ref: Option<BlobRefId>`
- `after_blob_ref: Option<BlobRefId>`
- `patch_blob_ref: Option<BlobRefId>`
- `hunk_ids: Vec<HunkId>`

State reconstruction rules:

- every state must resolve to either a Git tree, a checkpoint blob, or a parent
  state plus deltas
- required missing blobs make the query `Unavailable` or `Partial`
- ambiguous delete/recreate or rename boundaries must be surfaced as
  `Ambiguous`, never guessed
- checkpoints should be generated periodically so parent chains remain bounded

### FileChangeSet

- `observation_id`
- `change_id`
- `workspace_id`
- `file_id`
- `path_before: Option<PathBuf>`
- `path_after: Option<PathBuf>`
- `existed_before`
- `existed_after`
- `content_changed`
- `identity_status`
  - `Preserved`
  - `Created`
  - `Deleted`
  - `Ambiguous`
- `is_text`
- `is_queryable`
- `hunks: Vec<HunkRecord>`

### HunkRecord

- `hunk_id`
- `change_id`
- `file_id`
- `path_before: Option<PathBuf>`
- `path_after: Option<PathBuf>`
- `before_start_line`
- `before_line_count`
- `after_start_line`
- `after_line_count`
- `context_before_digest`
- `context_after_digest`
- `content_fingerprint_before`
- `content_fingerprint_after`
- `parent_hunk_ids: Vec<HunkId>`
- `origin_hunk_id: Option<HunkId>`
- `operation`
  - `Add`
  - `Replace`
  - `Delete`
- `observation_confidence`
  - `Exact`
  - `Partial`
  - `Ambiguous`

### LiveSegment

- `workspace_id`
- `file_id`
- `start_line`
- `end_line`
- `projection_version`
- `terminal_hunk_id`

`LiveSegment` maps the projected file state to terminal hunks.

### RevisionAlias

- `workspace_id`
- `git_commit_oid`
- `git_tree_oid`
- `projection_version`
- `workspace_state_id`
- `exact`
- `source`
  - `RecordedPostState`
  - `GitResolution`
  - `CommitMatch`

Historical queries must use exact aliases only. No fuzzy matching.

## Hunk and Segment Projection

The stored truth model is hunk-based.

When a normalized hunk is applied:

1. Resolve all affected parent segments intersecting the hunk's before range.
2. Create a child `HunkRecord` with parent hunk ids from those terminal
   segments.
3. Rewrite the affected segment map:
   - additions insert new segments
   - replacements remove affected segments and insert child segments
   - deletions remove affected segments and create a tombstone hunk with parent
     references
4. Write the canonical `hunk.observed` event with the same hunk ids and parent
   edges.

This allows:

- `file + line/range -> one or more terminal hunks`
- `terminal hunk -> parent hunks`
- lineage walk back to origin turn or bootstrap prehistory

## File Identity

Assign each tracked file a stable `file_id` only when continuity can be
proven.

Rules:

- rename-only and rename-plus-edit observations can preserve `file_id`
- path changes update path alias history
- delete followed by later recreation at the same path creates a new `file_id`
  only when that boundary is observable
- when evidence cannot distinguish preserve-vs-recreate safely, set
  `identity_status = Ambiguous`

## Local Persistence in `codex-state`

Use SQLite in `codex-state` rather than rollout JSONL for normalized
provenance.

Recommended tables:

- `provenance_workspaces`
- `provenance_workspace_streams`
- `provenance_local_events`
- `provenance_workspace_heads`
- `provenance_workspace_states`
- `provenance_workspace_state_deltas`
- `provenance_revision_aliases`
- `provenance_turns`
- `provenance_observations`
- `provenance_tool_refs`
- `provenance_files`
- `provenance_path_aliases`
- `provenance_file_heads`
- `provenance_blobs`
- `provenance_blob_refs`
- `provenance_changes`
- `provenance_hunks`
- `provenance_hunk_parents`
- `provenance_live_segments`

Recommended indexes:

- by `workspace_id` and current path
- by `workspace_stream_id` and sequence
- by `workspace_id` and projection version
- by `workspace_id` and Git tree OID
- by Git commit OID
- by hunk id
- by thread id and turn id
- by file id, projection version, start line, and end line
- by parent and child hunk edges

Rollouts remain the source of conversational history. SQLite becomes the
normalized provenance index and local export journal.

For queryable text files, the store must keep enough projected file state to
make repair and overlap queries implementable:

- latest projected file content through content-addressed blobs or equivalent
- workspace and file head refs keyed by projection version
- exact revision aliases keyed by commit OID and tree OID

## App-Server API

The first release should add app-server v2 APIs and skip dedicated TUI work.

### `provenance/readRange`

Primary entry point for anchored code coordinates.

`ProvenanceReadRangeParams`

- `selector: ProvenanceRangeSelector`
- `path: PathBuf`
- `start_line: u32`
- `end_line: Option<u32>`
- `expected_content_fingerprint: Option<String>`

`ProvenanceRangeSelector`

- `WorkspaceAnchor`
  - `workspace_root: PathBuf`
  - `expected_projection_version: Option<i64>`
- `GitRevision`
  - `workspace_root: Option<PathBuf>`
  - `code_ref: String`
  - `expected_commit_oid: Option<String>`

`ProvenanceReadRangeResponse`

- resolved workspace and file identity
- resolved selector info
  - live query: actual projection version
  - revision query: resolved commit OID and tree OID
- requested range
- `matched_segments: Vec<RangeSegmentProvenance>`
- optional collapsed summary when every segment shares the same lineage root
- completeness:
  - `Complete`
  - `Partial`
  - `Unavailable`
  - `Ambiguous`
- availability or failure reason

Live and historical queries share the same response model:

- `WorkspaceAnchor` resolves against the current projected workspace head
- `GitRevision` resolves `code_ref` to commit/tree, then maps that tree to an
  exact recorded `WorkspaceStateRef`

The server must not use fuzzy matching for historical Git queries. If the
selected revision cannot be mapped exactly to a recorded provenance state, it
must return `Unavailable`.

### `provenance/readHunk`

Deep hunk inspection.

`ProvenanceReadHunkParams`

- `workspace_root: PathBuf`
- `hunk_id: String`

`ProvenanceReadHunkResponse`

- full hunk metadata
- parent hunk metadata
- child hunk metadata when needed
- attached turn and tool references
- content/blob availability state

### `provenance/readTurn`

Turn-centric inspection.

`ProvenanceReadTurnParams`

- `thread_id: String`
- `turn_id: String`

`ProvenanceReadTurnResponse`

- normalized turn change set
- file and hunk summaries
- excerpts and tool refs
- observation attribution and ambiguity status

### `provenance/exportEvents`

Local export compatibility API.

This API is optional for v1 but the storage model must support it.

`ProvenanceExportEventsParams`

- `workspace_root: PathBuf`
- `after_sequence: Option<i64>`
- `limit: Option<u32>`

`ProvenanceExportEventsResponse`

- `workspace_stream_id`
- `events`
- `next_sequence`
- missing blob refs, if any

This is for future collectors. It is not a SaaS upload implementation.

## Evidence Returned by Queries

Responses should include machine-friendly evidence, not only "last touched by."

Each lineage response should include:

- hunk id
- file identity and current path
- current range and prior range when relevant
- origin and intermediate turn refs
- observation refs and attribution status
- user and assistant excerpts
- tool refs
- context digests and content fingerprints
- blob refs and availability
- resolved Git revision identity for historical queries

External systems can use this to assemble review and postmortem surfaces
without parsing raw rollout files for every lookup.

## External Mutations and Divergence

Not every workspace mutation comes from a Codex turn.

Examples:

- the user edits files outside Codex
- Git checkout or branch switching changes the working tree
- another tool rewrites files between Codex turns

Support `ExternalWorkspaceMutation`.

Recommended behavior:

1. Before a mutating Codex observation starts, compare current workspace state
   with projected workspace head.
2. If they differ, synthesize and persist `ExternalWorkspaceMutation` before
   attributing the Codex mutation.
3. After the Codex observation ends, revalidate post-state against expected
   projected head.
4. If drift is detected inside an active Codex observation and cannot be
   separated safely, write an ambiguous observation and mark the workspace
   stale.

Fallback if full repair is not ready:

- mark projection stale
- mark affected observation or turn ambiguous
- return structured availability reason from query APIs
- never silently answer with invalid lineage

## Operational Behavior

### Availability model

Provenance is best-effort and must not break normal turn execution.

If recording fails:

- the turn still completes normally
- the failure is surfaced as a warning or background event
- the workspace or turn is marked unavailable for provenance queries until the
  next successful baseline and projection update

### Ambiguity model

The system must distinguish unavailable from ambiguous provenance.

- `Unavailable` means the recorder lacks enough data to answer.
- `Partial` means some evidence is usable but required content or lineage is
  missing.
- `Ambiguous` means the recorder observed conflicting or concurrent mutations
  and intentionally refused to guess.

Common ambiguity triggers:

- projection version mismatch during workspace apply
- drift detected inside an active Codex mutation window
- identity boundaries that cannot be proven safely
- stale content expectations in range queries
- missing required blob refs for state reconstruction

### Text-only scope

The first release should only support text files.

For binary or oversized files:

- persist a file-level opaque change record if useful
- mark range queries unavailable for that file

### No TUI requirement

The first release does not need a TUI viewer. Later work may add a thin TUI or
IDE consumer calling the same app-server APIs.

## Testing Strategy

### Unit tests in `codex-provenance`

- hunk normalization from baseline diffs
- canonical event payload generation
- event hash and sequence validation
- parent hunk selection
- live segment rewrites for add, replace, delete
- rename-only handling
- delete and recreate at the same path
- blob availability and partial query behavior

### Core integration tests

- turn with `apply_patch`
- turn with shell-driven file mutation
- turn with `js_repl` mutation
- turn with no file change
- provenance unavailable outside Git
- provenance baseline independent of undo

### State and migration tests

- schema migration correctness
- local event stream gap/hash behavior
- indexed range lookup behavior
- projected file head reconstruction
- workspace and path alias updates
- exact revision alias lookup

### App-server tests

- `provenance/readRange`
- `provenance/readHunk`
- `provenance/readTurn`
- multi-segment `readRange` responses
- exact historical Git revision resolution
- unavailable historical Git revision responses
- availability and stale-projection responses
- optional export event pagination

### Concurrency and drift tests

- overlapping turns in the same workspace
- external edit before a mutating tool starts
- external edit during an active mutating tool window
- bootstrap prehistory for existing repos
- rename-plus-edit and ambiguous delete/recreate cases
- writes that touch files outside the primary workspace root
- branch or commit selectors that point at different code for the same path and
  range

## Incremental Delivery

### Phase 0: Model and Store Contracts

- add `codex-provenance` models
- define canonical local event envelope and payloads
- add SQLite migrations for workspace, event stream, blobs, observations, and
  hunks
- add event hash and gapless sequence checks

### Phase 1: Turn Recording Foundation

- add provenance baseline capture independent of undo
- seed bootstrap prehistory for existing queryable files
- add projection versioning and serialized workspace apply semantics
- persist mutation observations, file heads, and live segments
- expose `provenance/readTurn`

### Phase 2: Range Queries

- implement `provenance/readRange`
- implement `provenance/readHunk`
- implement stable `file_id`, path aliases, and ambiguity semantics
- implement multi-segment range responses

### Phase 3: Git Revision Queries

- implement exact Git revision resolution for recorded workspace states
- persist revision aliases for observed commit/tree states
- return `Unavailable` for unobserved historical revisions

### Phase 4: Divergence Handling

- add external mutation observation and repair path
- add drift revalidation at observation boundaries
- support stale workspace detection and explicit repair semantics

### Phase 5: Export Compatibility

- expose local `provenance/exportEvents`
- document mapping from mcodex local events to Forgeloop TraceEvent concepts
- keep cloud upload, enrollment, raw access approval, and retention out of
  mcodex

## Key Decisions

- Primary truth model: `workspace -> file -> hunk lineage`
- Primary query input: anchored code coordinates
- Primary storage: normalized SQLite state plus append-only local event stream
- Workspace projection updates are versioned and serialized
- Explicit ambiguity is better than silent misattribution
- Git is the historical code selector, not the provenance truth source
- Codex responsibility: local provenance kernel and exportable facts
- Forgeloop responsibility: cloud trace/evidence plane, incidents, actors,
  permissions, retention, and organization learning
- No historical backfill in the first release

## Recommendation

Proceed with a workspace-scoped, hunk-based provenance kernel implemented as a
new `codex-provenance` crate plus SQLite-backed indexing in `codex-state`.

Revise implementation planning around this sharper boundary:

- mcodex builds the local kernel, app-server APIs, and export-compatible
  canonical observations
- Forgeloop consumes those observations later and owns the online product layer

This avoids building cloud product concerns into mcodex while still preventing
future rework when Forgeloop needs to ingest provenance at scale.
