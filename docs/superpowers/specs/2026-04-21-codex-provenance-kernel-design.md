# Codex Provenance Kernel Design

This document defines an API-first provenance system for tracing how code at a
given file and line range was introduced and evolved across Codex activity.

The intended primary consumers are machine systems, not end users inside the
TUI. Codex should therefore provide a language-agnostic provenance kernel keyed
by workspace, file path, and line range. A dedicated postmortem or incident
system can add symbol parsing, issue correlation, and higher-level analysis on
top of that kernel.

## Summary

Build a workspace-scoped provenance kernel with these properties:

1. Record structured mutation observations during a turn and group them into
   turn-level change sets rather than relying on ad hoc `git blame` or
   live-only turn diffs.
2. Use `file + line/range` as the primary query surface.
3. Treat hunks, not symbols or individual lines, as the primary stored unit of
   provenance.
4. Maintain a versioned live line-range projection for each tracked file so a
   current range can be resolved to the latest hunk and walked back through its
   lineage.
5. Prefer explicit ambiguity and stale-workspace states over silent
   misattribution when concurrent or external writers are involved.
6. Expose provenance through new app-server v2 APIs, with no dedicated TUI
   workflow in the first release.

The recommended implementation adds a new `codex-provenance` crate for lineage
models and algorithms, persists normalized provenance into the existing SQLite
state database, and integrates recording into the core runtime with a dedicated
baseline snapshot flow that is independent of undo.

## Goals

- Answer: "How did the logic currently at this file and range get here?"
- Make `file + line/range` the canonical provenance query surface.
- Cover all turn-driven file mutations, including `apply_patch`, shell commands,
  `js_repl`, and any other tool path that changes workspace files.
- Preserve enough conversation and tool context for external systems to build a
  full postmortem trail without reassembling raw rollout items by hand.
- Keep the Codex side language-agnostic so external systems can own symbol and
  incident semantics.
- Prefer explicit `ambiguous` or `stale` provenance states over writing lineage
  that might be wrong.
- Reuse existing app-server and state infrastructure where it improves
  operability and testability.

## Non-Goals

- Do not make `git blame` the primary provenance mechanism.
- Do not require or implement symbol parsing inside Codex in the first release.
- Do not build a dedicated TUI postmortem experience in the first release.
- Do not backfill historical rollouts or pre-existing repository history.
- Do not make incident, issue, or PR objects first-class provenance keys inside
  Codex.
- Do not attempt repository-wide semantic equivalence tracking across refactors.

## Why This Is Not a `git blame` Feature

`git blame` operates at commit granularity and answers a different question:
"Which commit last touched this line?" That is useful, but it loses the Codex
turn context that matters for postmortems:

- the user request that initiated the change
- the assistant response that decided the change shape
- the tool calls used to produce it
- intermediate edits inside the same turn
- subsequent Codex turns that reshaped the logic before any commit existed

The correct primitive here is not commit blame. It is turn-aware hunk lineage.

## Why Codex Should Not Parse Symbols in v1

Symbol parsing is valuable for ergonomics, but it is not the right truth source
for this system.

- The future primary consumers are dedicated postmortem systems, not direct TUI
  users.
- Those systems can resolve symbols to ranges using their own language-aware
  indexing stack.
- Codex already has strong file and diff primitives but does not have a general
  multi-language symbol infrastructure.
- Pushing symbol parsing into Codex would couple the kernel to parser and
  grammar maintenance without improving the core provenance truth model.

Codex should return high-fidelity range and hunk lineage. External systems can
layer symbol semantics on top.

## Current Context

The existing codebase already has several useful building blocks:

- turn-scoped diff tracking in `core/src/turn_diff_tracker.rs`
- live `TurnDiff` emission from `core/src/codex.rs`
- persisted thread history reconstruction in `app-server-protocol`
- existing `thread/read` surfaces for turn and item history
- existing SQLite-backed local state infrastructure in `codex-rs/state`
- ghost snapshot machinery that can capture a turn baseline in a Git repo

However, the current surfaces are not sufficient as the long-term provenance
kernel:

1. `TurnDiff` is currently emitted as a unified diff string and is not
   persisted as structured lineage.
2. `ThreadItem::FileChange` is a client-facing projection of file changes, not a
   normalized provenance model.
3. The current turn diff tracker is wired mainly through `apply_patch` events.
   Shell and `js_repl` execution paths do not hand the tracker through the same
   way, so a provenance system built on top of that tracker would miss mutating
   turns from other tool paths.
4. Existing history is thread-oriented, while the target query model must work
   across multiple Codex threads operating on the same workspace.
5. A single net `turn start -> turn end` diff cannot preserve tool-level
   intermediate changes and cannot safely disambiguate concurrent or external
   workspace mutations that land while a turn is in flight.

The design therefore needs a lower, workspace-oriented recording layer that is
separate from the current live diff presentation.

## Recommended Architecture

### 1. Scope Provenance by Workspace, Not by Thread

The primary query is not "what happened in this thread?" but "how did this code
in this workspace evolve?" That requires provenance to aggregate across many
threads.

Define a workspace scope as:

- the canonical Git worktree root when the current directory is inside a Git
  worktree
- otherwise, the canonical current working directory

Each recorded change set is attached to exactly one workspace scope and may
optionally reference a `thread_id` and `turn_id` when the source was a Codex
turn.

One Codex turn may emit more than one workspace-scoped change set if it mutates
files that belong to more than one canonical workspace root. No single change
set should span multiple workspaces.

Each workspace also owns a monotonic `projection_version`. All updates to the
workspace lineage graph and live segment projection must be applied through a
single serialized workspace transaction:

- read current projection state
- verify the expected base projection version
- apply new mutation observations
- advance the projection version

If the expected base version does not match, the recorder must not guess. It
must write an explicit ambiguous result and mark the workspace stale until it is
reconciled.

This gives the system the right aggregation model:

- many threads can contribute to one workspace lineage graph
- range queries do not need a thread id as the primary lookup key
- postmortem systems can ask about code in the workspace directly

### 2. Add a Dedicated Provenance Crate

Do not grow `codex-core` for this.

Add a new crate, recommended name `codex-provenance`, responsible for:

- normalized provenance model types
- diff and hunk normalization
- lineage graph construction
- live segment projection updates
- query result assembly helpers

Recommended ownership split:

- `codex-provenance`
  - pure models and lineage algorithms
  - no app-server protocol knowledge
- `codex-core`
  - turn lifecycle integration
  - baseline capture orchestration
  - invocation of provenance recording at turn completion
- `codex-state`
  - SQLite schema, migrations, and indexed persistence
- `codex-app-server-protocol`
  - new v2 request and response types
- `codex-app-server`
  - query handlers and request routing

This keeps the heavy logic out of `codex-core` and fits the existing workspace
shape.

### 3. Introduce a Provenance Baseline Snapshot Flow

The provenance kernel needs a stable "before" view for every tracked turn.
Current ghost snapshots are good prior art, but provenance should not depend on
undo being enabled.

Add a dedicated turn-start baseline capture flow:

- name: `ProvenanceBaselineTask` or equivalent
- trigger: every turn when provenance is enabled
- implementation: reuse the same Git snapshot machinery already used to produce
  ghost commits, but under provenance-specific control
- output:
  - workspace root
  - baseline snapshot identifier
  - base projection version
  - capture status

If the turn is inside a Git worktree, the baseline should be a snapshot commit
or equivalent stable snapshot reference produced from the current worktree
state.

If the turn is outside Git, v1 should degrade cleanly:

- mark provenance unavailable for that turn
- do not fail the user turn
- return an explicit availability reason from provenance APIs

This keeps the first release focused on the intended code-repo use case.

The first successful baseline in a workspace must also seed bootstrap
provenance for already-existing text files in that workspace. The system does
not backfill historical turns, but it still needs a synthetic prehistory layer
so that:

- unchanged legacy lines have a terminal segment
- the first recorded replacement of legacy code has a parent
- queries can distinguish "pre-provenance code" from "no data"

### 4. Extract Changes From Baseline to Turn End

The recorder must sit below the current live UI diff projection and must not
depend on `TurnDiffTracker` being complete. It must also avoid attributing
concurrent or external writes to the wrong Codex turn.

Recommended flow:

1. Capture turn baseline snapshot and `base_projection_version` at turn start.
2. Before each mutating tool observation, reconcile the actual workspace state
   against the projected workspace head for that workspace.
   - If drift exists before the tool starts, synthesize an
     `ExternalWorkspaceMutation` or mark the workspace stale before attributing
     any new Codex change.
3. Around each mutating tool boundary, capture a `MutationObservation` from the
   pre-observation state to the post-observation state.
4. Apply that observation inside a serialized workspace transaction.
   - If the workspace projection version advanced unexpectedly, or if the
     observed post-state cannot be reconciled with the projected head, record an
     ambiguous observation and stop applying further lineage for that workspace
     until repair occurs.
5. At turn completion, persist a turn envelope that references the ordered
   observations and a derived net summary for convenience.

This gives a single truth source that covers:

- `apply_patch`
- shell commands that edit files
- `js_repl` edits
- future mutating tools

without needing separate per-tool provenance logic, and it preserves
tool-level intermediate changes rather than only a turn-end net diff.

### 5. Store Hunks as the Primary Provenance Unit

The stored truth model should be hunk-based, not line-based and not
symbol-based.

Reasoning:

- line numbers drift too easily and encourage false precision
- symbols are not available in a language-agnostic way
- most meaningful logic changes happen in contiguous ranges, not isolated single
  lines

Recommended normalized model:

#### `WorkspaceProvenanceChangeSet`

- `workspace_id`
- `source`
  - `CodexTurn`
  - future: `ExternalWorkspaceMutation`
- `thread_id: Option<String>`
- `turn_id: Option<String>`
- `started_at`
- `completed_at`
- `workspace_root`
- `turn_baseline_ref`
- `base_projection_version`
- `final_projection_version: Option<i64>`
- `user_excerpt: Option<String>`
- `assistant_excerpt: Option<String>`
- `tool_refs: Vec<ToolRef>`
- `observations: Vec<MutationObservation>`
- `derived_net_summary: Vec<FileNetSummary>`

#### `MutationObservation`

- `observation_id`
- `workspace_id`
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
  - `Unavailable`
- `base_projection_version`
- `applied_projection_version: Option<i64>`
- `pre_state_ref`
- `post_state_ref`
- `files: Vec<FileChangeSet>`

#### `FileChangeSet`

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

#### `HunkRecord`

- `hunk_id`
- `change_id`
- `file_id`
- `before_start_line`
- `before_line_count`
- `after_start_line`
- `after_line_count`
- `context_before`
- `context_after`
- `content_fingerprint_before`
- `content_fingerprint_after`
- `parent_hunk_ids: Vec<HunkId>`
- `operation`
  - `Add`
  - `Replace`
  - `Delete`

#### `LiveSegment`

- `workspace_id`
- `file_id`
- `start_line`
- `end_line`
- `projection_version`
- `terminal_hunk_id`

`LiveSegment` is the crucial projection structure. It maps the current file
state to the latest hunk responsible for each contiguous line range.

### 6. Maintain a Live Segment Projection

Range queries need to answer questions about the current workspace state, not
just about a historical patch. That requires an index over the latest file
state.

Maintain a per-file live segment map:

- segments are contiguous
- segments do not overlap
- each segment points at the latest terminal hunk that wrote those lines

When a new normalized hunk is applied:

1. Resolve all affected parent segments that intersect the hunk's `before`
   range.
2. Create a child `HunkRecord` with `parent_hunk_ids` set to the terminal hunks
   found in that range.
3. Rewrite the affected segment map:
   - additions insert new segments
   - replacements remove affected segments and insert new child segments
   - deletions remove affected segments and create a tombstone hunk with parent
     references

This is the heart of the system. It allows:

- `file + line/range -> one or more terminal hunks`
- `terminal hunk -> parent hunks`
- repeated walking back to the turn that introduced the current logic

### 7. Give Files Stable Identity Across Renames

The same logic may survive path changes. Querying only by path is not enough.

Assign each tracked file a stable `file_id` when the recorder can prove
continuity. The change model must support both path movement and content edits
within the same observation:

- rename-only and rename-plus-edit observations can preserve `file_id`
- path changes update the current path alias history
- delete followed by later recreation at the same path creates a new `file_id`
  only when that boundary is observable
- when the available evidence cannot distinguish preserve-vs-recreate safely,
  set `identity_status = Ambiguous` and surface that ambiguity through the API

This keeps lineage coherent across renames without over-promising certainty for
delete/recreate or heavy rewrite edge cases.

### 8. Use SQLite in `codex-state`, Not Rollout JSONL, as the Store

The existing `state` crate already owns local SQLite-backed state. Provenance is
best persisted there rather than bloating rollout JSONL with analysis data.

Recommended new SQLite tables:

- `provenance_workspaces`
- `provenance_workspace_heads`
- `provenance_turns`
- `provenance_observations`
- `provenance_tool_refs`
- `provenance_files`
- `provenance_path_aliases`
- `provenance_file_heads`
- `provenance_text_blobs`
- `provenance_changes`
- `provenance_hunks`
- `provenance_hunk_parents`
- `provenance_live_segments`

Recommended indexes:

- by `workspace_id` and current path
- by `workspace_id` and `projection_version`
- by `hunk_id`
- by `thread_id` and `turn_id`
- by `file_id`, `projection_version`, `start_line`, and `end_line`
- by parent and child hunk edges

Why SQLite is the right default:

- there is already migration and runtime infrastructure in `codex-state`
- indexed range lookups are easier and safer than scanning JSONL side files
- app-server query handlers can stay simple and fast
- future external consumers can evolve without changing the storage contract

Rollouts remain the source of conversational history. SQLite becomes the
normalized provenance index. For queryable text files, the store must also keep
enough projected file state to make repair and overlap queries implementable:

- latest projected file content, directly or through a content-addressed text
  blob table
- workspace and file head refs keyed by projection version

### 9. Keep the Query Surface API-First

The first release should add app-server v2 APIs and skip dedicated TUI work.

Recommended methods and shapes:

#### `provenance/readRange`

Primary entry point.

`ProvenanceReadRangeParams`

- `workspace_root: Option<PathBuf>`
- `path: PathBuf`
- `start_line: u32`
- `end_line: Option<u32>`
- `expected_projection_version: Option<i64>`
- `expected_content_fingerprint: Option<String>`

`ProvenanceReadRangeResponse`

- resolved workspace and file identity
- actual projection version
- requested range
- `matched_segments: Vec<RangeSegmentProvenance>`
- optional collapsed summary when every segment shares the same lineage root
- availability or failure status

#### `provenance/readHunk`

Deep hunk inspection.

`ProvenanceReadHunkParams`

- `workspace_root: PathBuf`
- `hunk_id: String`

`ProvenanceReadHunkResponse`

- full hunk metadata
- parent hunk metadata
- child hunk metadata when needed
- attached turn and tool references

#### `provenance/readTurn`

Turn-centric inspection.

`ProvenanceReadTurnParams`

- `thread_id: String`
- `turn_id: String`

`ProvenanceReadTurnResponse`

- normalized turn change set
- file and hunk summaries
- excerpts and tool refs

These methods match the actual consumer model:

- a postmortem system identifies a suspect range
- it asks Codex for the terminal hunk and lineage
- it optionally drills into a specific hunk or turn

### 10. Return Machine-Friendly Evidence, Not Just Blame Labels

The API should not stop at "this turn last touched the line."

Each lineage response should include enough evidence for a dedicated postmortem
system to build a narrative:

- `hunk_id`
- file identity and current path
- current range and, when relevant, prior range
- origin and intermediate turn refs
- observation refs and attribution status
- user and assistant excerpts
- tool refs
- context snippets and content fingerprints

This lets the external system assemble rich review surfaces without parsing raw
rollout files on every lookup.

## External Mutations and Divergence

Long-term correctness requires acknowledging that not every workspace mutation
comes from a Codex turn.

Examples:

- the user edits files outside Codex
- Git checkout or branch switching changes the working tree
- another tool rewrites files between Codex turns

The architecture must support a second change source:

- `ExternalWorkspaceMutation`

Recommended end state:

1. Before a mutating Codex observation starts, compare the current workspace
   state with the projected workspace head for that workspace.
2. If they differ, synthesize and persist an `ExternalWorkspaceMutation`
   observation before attributing the Codex mutation.
3. After the Codex observation ends, revalidate the resulting post-state against
   the expected projected head before advancing the projection version.
4. If drift is detected inside an active Codex observation and cannot be
   separated safely, write an ambiguous observation and mark the workspace
   stale instead of attributing the mixed result to Codex.

This keeps the lineage graph honest even when the workspace changes outside
Codex.

If this full repair path is too much for the first increment, the fallback
behavior should be explicit:

- mark the workspace projection stale
- mark the affected observation or turn as ambiguous
- return a structured availability reason from query APIs
- do not silently answer with invalid lineage

## Operational Behavior

### Availability model

Provenance is best-effort and must not break normal turn execution.

If recording fails:

- the turn still completes normally
- the failure is surfaced as a warning or background event
- the workspace or turn is marked unavailable for provenance queries until the
  next successful baseline and projection update

### Ambiguity model

The system must distinguish unavailable provenance from ambiguous provenance.

- `Unavailable` means the recorder lacks enough data to answer.
- `Ambiguous` means the recorder observed conflicting or concurrent mutations
  and intentionally refused to guess.

Common ambiguity triggers:

- projection version mismatch during workspace apply
- drift detected inside an active Codex mutation window
- identity boundaries that cannot be proven safely
- range queries that are anchored to stale content expectations

### Text-only scope

The first release should only support text files.

For binary or oversized files:

- persist a file-level opaque change record if useful
- mark range queries unavailable for that file

### No TUI requirement

The first release does not need a TUI viewer. At most, later work may add a
thin TUI or IDE consumer that calls the same app-server APIs.

## Testing Strategy

### Unit tests in `codex-provenance`

- hunk normalization from baseline diffs
- parent hunk selection
- live segment rewrites for add, replace, delete
- rename-only handling
- delete and recreate at the same path

### Core integration tests

- turn with `apply_patch`
- turn with shell-driven file mutation
- turn with `js_repl` mutation
- turn with no file change
- provenance unavailable outside Git

### State and migration tests

- schema migration correctness
- indexed range lookup behavior
- projected file head reconstruction
- workspace and path alias updates

### App-server tests

- `provenance/readRange`
- `provenance/readHunk`
- `provenance/readTurn`
- multi-segment `readRange` responses
- availability and stale-projection responses

### Concurrency and drift tests

- overlapping turns in the same workspace
- external edit before a mutating tool starts
- external edit during an active mutating tool window
- bootstrap prehistory for existing repos
- rename-plus-edit and ambiguous delete/recreate cases
- writes that touch files outside the primary workspace root

## Incremental Delivery

### Phase 1: Foundation

- add `codex-provenance`
- add provenance baseline capture independent of undo
- seed bootstrap prehistory for existing queryable files
- add projection versioning and serialized workspace apply semantics
- persist mutation observations, file heads, and live segments into SQLite
- expose `provenance/readTurn`

### Phase 2: Range Queries

- implement `provenance/readRange`
- implement `provenance/readHunk`
- add stable `file_id`, path alias, and ambiguity semantics
- harden rename, delete, recreate, and multi-segment range behavior

### Phase 3: Divergence Handling

- add external mutation repair path and drift revalidation at observation
  boundaries
- support stale workspace detection and explicit repair semantics

### Phase 4: External Consumer Integration

- wire the dedicated postmortem system to the provenance APIs
- add higher-level semantic overlays outside Codex

## Key Decisions

- Primary truth model: `workspace -> file -> hunk lineage`
- Primary query input: `file + line/range`
- Primary storage: normalized SQLite state, not rollout JSONL
- Workspace projection updates are versioned and serialized
- Explicit ambiguity is better than silent misattribution
- Codex responsibility: provenance kernel
- External system responsibility: symbols, incidents, higher-level postmortem
  views
- No historical backfill in the first release

## Recommendation

Proceed with a workspace-scoped, hunk-based provenance kernel implemented as a
new `codex-provenance` crate plus SQLite-backed indexing in `codex-state`.

This is the best balance of correctness, extensibility, and operational
simplicity:

- more accurate than `git blame`
- more durable than per-turn unified diffs
- simpler and more stable than embedding symbol analysis into Codex
- directly usable by a dedicated postmortem system
