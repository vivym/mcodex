# Export and Ingest

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

## Export Contract

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

## Export App-Server APIs

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

