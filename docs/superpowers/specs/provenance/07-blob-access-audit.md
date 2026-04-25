# Blob Access and Audit

_This file is part of the split provenance spec set. See [README](README.md) for the index and ownership map._

## Blob Contract

Do not make the canonical blob shape depend on a local file path.

`BlobAvailability`

- `available`
- `pending`
- `missing`
- `checksumMismatch`
- `redacted`
- `expired`

`BlobDescriptor`

- `blob_ref_id: String`
- `digest_algorithm: String`
- `expected_digest: String`
- `byte_size: u64`
- `content_kind: String`
- `content_type: Option<String>`
- `ref_kind: String`
- `scope: String`
- `sensitivity: String`
- `retention_hint: Option<String>`
- `legal_hold_hint: Option<String>`
- `required_for_reconstruction: bool`

`BlobDescriptor` is immutable content identity. Current availability is not part
of the descriptor. Availability changes are represented by replayable
availability facts and resolved separately at query time.

`blob_ref_id` is the immutable blob-identity key in v1. A given
`(workspace_stream_id, stream_epoch, blob_ref_id)` must resolve to exactly one
visible `blob.descriptor_recorded` event. v1 does not define blob-descriptor
supersession. If content identity changes, the recorder must mint a new
`blob_ref_id` rather than append a second competing descriptor for the same id.
Append validation must reject a second visible descriptor event for the same
epoch-local `blob_ref_id`.

`BlobManifestEntry`

- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`

`LocalBlobLocator`

- `blob_ref_id: String`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`
- `local_storage_ref: ExactArtifactRef`

Canonical export should depend on `BlobDescriptor`. The local locator is an
implementation detail for the local collector, but it must still bind to one
exact descriptor event rather than only a mutable `blob_ref_id`.

Descriptor selection and availability resolution are separate steps:

- `blob.descriptor_recorded` selects immutable blob identity
- the effective current availability for that descriptor is resolved from the
  latest visible availability fact in the same snapshot
- if no later `blob.availability_changed` exists, the descriptor event itself is
  the authoritative `availability_event_ref` and its
  `initial_availability` remains the current availability
- the authoritative descriptor event for a blob identity is the enclosing
  `blob.descriptor_recorded` event that carries this `BlobDescriptor`; the
  canonical payload must not embed a self-referential `EventRef` back to that
  same event

Blob reads return a tagged result instead of assuming content is always
available. Serialized wire values are camelCase:

- `inlineChunk`
  - descriptor, offset, byte count, base64 content, next offset, continuation
    token, digest verification, authorization event ref, and optional delivery
    event ref
- `unavailable`
  - descriptor, availability status, reason code, authorization event ref, and
    optional delivery event ref when the failure occurred after a read attempt
- `accessDenied`
  - descriptor, policy reason code, and authorization event ref
- `auditFailure`
  - audit-stream or audit-append failure; no content and no access event refs
- `selectorFailure`
  - selector resolution failed; authoritative status lives in the top-level
    `ReadBlobResponse.query_status`, and no exact custody fact was selected
- `invalidRequest`
  - resolved selector but invalid offset/continuation/session parameters; no
    content and no newly disclosed bytes

`provenanceBlob/read` opens one auditable blob-read session per logical
continuation chain. It appends `access.blob_read_authorized` before first
disclosure and appends `access.blob_read_delivered` as session-scoped
checkpoints rather than requiring one delivery event per chunk. Reads from
sealed historical epochs therefore remain auditable without modifying those
epochs. If the caller does not explicitly select an audit stream, the server
must deterministically resolve the current writable local audit stream and
expose the result through returned authorization and delivery event refs.

Writable audit ingress remains fail-closed: if the local `AccessAudit` stream
cannot be resolved or appended, the read must fail rather than disclose bytes
without audit. To control hot-store growth, implementations may export, cold
store, or compact intermediate delivery checkpoints for a finished blob-read
session after policy-defined durability/retention conditions are met, but they
must preserve the authorization event, the final retained delivery checkpoint,
and enough chain continuity to prove the disclosed byte ranges and result.

## Conversation Content

Do not inline raw excerpts as part of the stable canonical export shape.

- raw or semi-raw excerpts should be blob-backed
- query surfaces may return a sanitized summary string for convenience
- full raw content remains behind blob reads and future policy gates

Even locally, `provenanceBlob/read` should be explicit and auditable. It may be a local
policy check in mcodex v1, but the API shape must not bypass future raw-content
entitlements.

`access.blob_read_authorized` is emitted once per logical blob-read session.
`opening_request_id` identifies the RPC that opened that session; later resumed
RPCs are linked by `blob_read_session_id`, not by reusing `opening_request_id`.
`opening_requested_byte_range` captures only the opening RPC's requested range.
The session's total disclosure budget is instead defined by
`authorized_session_scope`; resumed reads that would exceed that scope must open
a new blob-read session rather than silently extend the original one.

`access.blob_read_delivered` is cumulative at the checkpoint level:

- `delivered_byte_range` covers the cumulative byte range disclosed from the
  start of the session through this checkpoint
- `delivered_byte_count_cumulative` is the cumulative disclosed byte count
  through this checkpoint
- `delivered_content_digest_cumulative` binds the cumulative disclosed content
  through this checkpoint, not only the most recent chunk


## Blob App-Server APIs

`provenanceBlobManifest/read`

- `ReadBlobManifestParams`
  - `selector: WorkspaceCustodySelector`
  - `blob_ref_ids: Option<Vec<String>>`
  - `cursor: Option<String>`
  - `limit: Option<u32>`

Rules:

- `provenanceBlobManifest/read` is a workspace-custody blob API and does not accept other
  ledger scopes
- page 1 of a cursor-based manifest read must bind a stable snapshot of the
  requested epoch's visible descriptor set; later pages must read against that
  same snapshot rather than the live newest-visible set
- each returned `BlobManifestEntry` represents the unique visible
  `blob.descriptor_recorded` fact for that `blob_ref_id` in the requested
  workspace custody epoch, together with its exact descriptor `EventRef`
- `blob_ref_ids` mode is for fetching explicit descriptors by id
- `cursor` / `limit` mode is for paginating the stream-scoped manifest
- `blob_ref_ids` is mutually exclusive with `cursor` and `limit`
- every opaque `next_cursor` must bind the original selector, filters, sort
  order, and manifest snapshot watermark; reusing a cursor with changed request
  inputs is a request validation error
- each returned `BlobManifestEntry` must carry both the immutable
  `descriptor_event_ref` and the resolved `availability_event_ref` that made the
  entry's current availability authoritative in that snapshot

`ReadBlobManifestResponse`

- `selector: WorkspaceCustodySelector`
- `query_status: QueryResolutionStatus`
- `data: Vec<BlobManifestEntry>`
- `missing_blob_refs: Vec<String>`
- `snapshot_head_event_ref: Option<EventRef>`
- `next_cursor: Option<String>`

When `query_status.selector_status = Matched`, `snapshot_head_event_ref` is
mandatory and names the manifest snapshot frontier that every page in the same
cursor chain must use. It may be `null` only for selector mismatch or
unavailable responses.

`provenanceBlob/read`

- `ReadBlobParams`
  - `selector: WorkspaceCustodySelector`
  - `blob_ref_id: String`
  - `expected_descriptor_event_ref: Option<EventRef>`
  - `continuation_token: Option<String>`
  - `access_purpose: String`
  - `offset: u64`
  - `limit: u32`

Rules:

- `provenanceBlob/read` is a workspace-custody blob API and does not accept other ledger
  scopes
- blob read has two protocol phases:
  - initial resolution request: `continuation_token = None`
  - resumed session request: `continuation_token` is present and becomes the
    authoritative binding for the resolved descriptor, blob-read session, next
    offset, access purpose, and any echoed selector/blob identity fields
- the server derives `AccessRequester` from the actual caller and execution
  context; request payloads must not supply audit identity fields directly
- the server deterministically resolves the current writable `AccessAudit`
  stream for the local installation; callers do not choose audit-stream
  placement through the read RPC
- the exact authorization anchor for a resolved blob read is the selected
  `blob.descriptor_recorded` event for `(workspace_stream_id, stream_epoch,
  blob_ref_id)`; implementations must not choose between descriptor,
  availability, or workspace-state facts ad hoc
- the authorization event must also pin the resolved effective availability via
  `target_availability` and `target_availability_event_ref`; later
  availability changes must not retroactively change what availability fact
  justified the original authorization decision
- the first request in a logical blob-read session may omit
  `continuation_token`; if more bytes remain, the response must return a new
  opaque continuation token that binds selector, `blob_ref_id`,
  `descriptor_event_ref`, `availability_event_ref`, the audit session identity,
  next offset, access purpose, a visibility watermark for the resolved
  descriptor, and resumable digest-checkpoint state sufficient to continue
  cumulative delivery hashing without re-reading previously disclosed bytes
- when `continuation_token` is supplied, the server must resume that exact
  descriptor-pinned session; request fields that disagree with the token are
  request validation errors
- malformed or undecodable continuation tokens are request validation errors and
  must anchor `query_status.status_anchor = blobReadAttempt`
- before issuing a continuation token, the server must either encode the
  resumable digest state into that token or persist a digest checkpoint that
  the token can deterministically recover; resumed reads must not require
  re-reading already disclosed bytes merely to continue the cumulative digest
- a client that resumes from a previously returned `next_offset` must present
  the paired `continuation_token`; replaying bare selector + blob id + offset
  is an invalid request for continued disclosure
- when `continuation_token` is supplied, `offset` must equal the token-bound
  next offset
- `expected_descriptor_event_ref` is valid only for an initial resolution
  request and must be absent when `continuation_token` is supplied
- in v1, the first request with no continuation token selects the unique visible
  `blob.descriptor_recorded` event for that `blob_ref_id` in the requested
  workspace custody epoch
- after selecting that immutable descriptor, the server must resolve the
  effective current availability from the same snapshot and bind it with an
  exact `availability_event_ref`
- on an initial resolution request, if `expected_descriptor_event_ref` is
  provided, it must resolve to that exact selected descriptor event or the
  server returns `query_status.selector_status = ConstraintMismatch` with no
  content
- `limit` must be greater than zero and no larger than the server-advertised
  maximum chunk size
- `offset` is zero-based
- `offset` greater than the blob length is an invalid request, not a blob
  availability result
- `inlineChunk.byte_count` may be zero only when `offset` equals the blob
  length
- `inlineChunk.next_offset = None` means the returned chunk reached EOF
- `inlineChunk.continuation_token` is required whenever `next_offset` is
  present, and it must be absent at EOF
- `inlineChunk.blob_digest_verified = true` only when the server has validated
  the complete blob content against the descriptor digest
- the first resolved request in a logical blob-read session must append exactly
  one authorization event before any bytes are disclosed and return its
  `authorization_event_ref`
- `InlineBlobChunk`, `BlobUnavailableResult`, and `BlobAccessDeniedResult` must
  echo that session's `authorization_event_ref`
- delivery events are session-scoped checkpoints, not per-chunk requirements.
  The server must flush a delivery checkpoint before EOF and whenever a
  configured uncheckpointed-byte threshold would otherwise be exceeded; it may
  flush more often
- `delivery_event_ref` identifies the latest committed delivery checkpoint for
  the session and is optional on responses that disclose bytes before the next
  checkpoint boundary is reached
- no writable `AccessAudit` stream or authorization append failure returns
  `BlobAuditFailureResult` with no content and no access event refs
- successful disclosure must eventually record the actual delivered byte range,
  `delivered_byte_count_cumulative`, and
  `delivered_content_digest_cumulative` in one or more session delivery
  checkpoints; EOF must flush the final checkpoint before the terminal response
  is returned
- failed, truncated, or unavailable delivery attempts must commit a delivery
  checkpoint with the actual result before the terminal response is returned
- selector-level failures return `BlobSelectorFailureResult`; they do not append
  `access.blob_read_authorized` because no exact custody fact has been selected
- invalid offsets, cursor/token mismatches, or other blob-read parameter
  conflicts return `BlobRequestInvalidResult`
- initial-resolution descriptor mismatches anchor
  `query_status.status_anchor = blobReadAttempt` with
  `expected_descriptor_event_ref` populated and
  `continuation_token_fingerprint = None`
- request-validation failures before the server can recover one exact
  descriptor-pinned session anchor `query_status.status_anchor = blobReadAttempt`
- descriptor-pinned blob-read session failures must anchor
  `query_status.status_anchor = blobReadSession`

`InlineBlobChunk`

- `type = inlineChunk`
- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`
- `offset: u64`
- `byte_count: u32`
- `base64_content: String`
- `next_offset: Option<u64>`
- `continuation_token: Option<String>`
- `chunk_digest_verified: bool`
- `blob_digest_verified: bool`
- `authorization_event_ref: EventRef`
- `delivery_event_ref: Option<EventRef>`

`BlobUnavailableResult`

- `type = unavailable`
- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`
- `reason_code: String`
- `authorization_event_ref: EventRef`
- `delivery_event_ref: Option<EventRef>`

`BlobAccessDeniedResult`

- `type = accessDenied`
- `blob_descriptor: BlobDescriptor`
- `descriptor_event_ref: EventRef`
- `availability: BlobAvailability`
- `availability_event_ref: EventRef`
- `policy_reason_code: String`
- `authorization_event_ref: EventRef`

`BlobAuditFailureResult`

- `type = auditFailure`
- `audit_status: AuditFailureStatus`

`BlobSelectorFailureResult`

- `type = selectorFailure`

`BlobRequestInvalidResult`

- `type = invalidRequest`
- `reason_codes: Vec<String>`

`BlobReadResult`

- tagged union of `InlineBlobChunk`, `BlobUnavailableResult`,
  `BlobAccessDeniedResult`, `BlobAuditFailureResult`, and
  `BlobSelectorFailureResult`, and `BlobRequestInvalidResult`

`ReadBlobResponse`

- `query_status: QueryResolutionStatus`
- `result: BlobReadResult`

`ReadBlobResponse.query_status` is the single authoritative selector-resolution
status for blob reads. `BlobSelectorFailureResult` must not carry a second
`QueryResolutionStatus`. `BlobAuditFailureResult` must not duplicate
`reason_codes`; callers read audit failure reasons from
`audit_status.reason_codes`.

If `query_status.selector_status != Matched`, `result` must be
`BlobSelectorFailureResult`. Resolved-target variants are valid only when the
selector matched exactly one blob custody fact. If the selector matched but the
audit stream cannot be resolved or written, `result` must be
`BlobAuditFailureResult`, `query_status.selector_status` remains `Matched`, and
no bytes may be disclosed.
If the selector matched but the request parameters contradict the resolved blob
session or range contract, `result` must be `BlobRequestInvalidResult`,
`query_status.selector_status` remains `Matched`, and no bytes may be
disclosed.

Blob v1 uses chunked inline transfer only. It does, however, require opaque
continuation tokens for descriptor-pinned multi-chunk reads. Local file handles
or out-of-band streaming transports remain out of scope until a dedicated
transport defines redemption, expiry, range, and audit semantics.
