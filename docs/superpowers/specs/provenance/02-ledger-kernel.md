# Ledger Kernel

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

## Ledger Contract

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

## State and Continuity Contract

The kernel needs stable ordering and explicit rotation semantics.

### Identity types

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

### Ordering rules

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

### Hash-chain rules

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

### Execution Fact Payloads

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

### Code Fact Payloads

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

### Blob and System Payloads

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
