# Query APIs

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

## Query Contract

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

Query and discovery RPCs are specified in this file. Export RPC payloads are
specified in [06-export-ingest.md](06-export-ingest.md), and blob manifest/read
RPC payloads are specified in [07-blob-access-audit.md](07-blob-access-audit.md).

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
- the named DTOs in this file and the API-specific files linked above are the
  normative contract for schema generation; do not replace them with anonymous
  inline response blocks in implementation docs
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
