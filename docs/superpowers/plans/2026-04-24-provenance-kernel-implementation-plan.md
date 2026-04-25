# Provenance Kernel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the mcodex provenance kernel that records, stores, queries, and exports durable execution and code custody facts without losing chain-of-custody.

**Architecture:** Introduce two focused crates: `codex-provenance` for typed contracts, canonicalization, selectors, projection logic, and capture abstractions; `codex-provenance-store` for the local SQLite journal, exact artifact store, hash-chain validation, and query indexes. Keep `codex-core` integration thin through `MutationSupervisor` ingress wrappers, and expose all read/export surfaces through app-server v2 DTOs and handlers.

**Tech Stack:** Rust 2024, serde, schemars, ts-rs, sqlx SQLite, tokio, app-server v2 JSON-RPC, existing `just` commands, existing Bazel `codex_rust_crate` rules.

---

## Required References

- Spec index: `docs/superpowers/specs/provenance/README.md`
- Architecture: `docs/superpowers/specs/provenance/00-architecture.md`
- Capture contract: `docs/superpowers/specs/provenance/01-execution-workspace-capture.md`
- Ledger contract: `docs/superpowers/specs/provenance/02-ledger-kernel.md`
- Artifact contract: `docs/superpowers/specs/provenance/03-exact-artifact-store.md`
- Projection contract: `docs/superpowers/specs/provenance/04-code-projection.md`
- Query API contract: `docs/superpowers/specs/provenance/05-query-apis.md`
- Export contract: `docs/superpowers/specs/provenance/06-export-ingest.md`
- Blob/audit contract: `docs/superpowers/specs/provenance/07-blob-access-audit.md`
- Test strategy: `docs/superpowers/specs/provenance/08-operability.md`
- Phase guidance: `docs/superpowers/specs/provenance/09-implementation-plan.md`
- Rust repo instructions: `AGENTS.md`

## Scope Check

This plan intentionally remains one master implementation plan. The spec covers several subsystems, but they are not independent products: app-server DTOs, journal storage, capture, projection, blob audit, and export all share the same identity model and status vocabulary. Splitting the plan would increase the risk of incompatible contracts. Instead, each task below is a PR-sized vertical slice with its own tests and commit.

## File Structure

Create:

- `codex-rs/provenance/Cargo.toml` - new `codex-provenance` crate manifest.
- `codex-rs/provenance/BUILD.bazel` - Bazel target for the crate.
- `codex-rs/provenance/src/lib.rs` - public exports only.
- `codex-rs/provenance/src/canonical.rs` - RFC 8785 canonical JSON hash input helpers.
- `codex-rs/provenance/src/ids.rs` - `EventRef`, `LedgerScope`, stream refs, id newtypes.
- `codex-rs/provenance/src/status.rs` - recorder, query, audit, and schema status types.
- `codex-rs/provenance/src/selectors.rs` - code, workspace custody, blob, and live workspace selectors.
- `codex-rs/provenance/src/event.rs` - canonical `TraceKernelEvent`, payload envelope metadata, append batches.
- `codex-rs/provenance/src/schema.rs` - schema bundle and payload registry types.
- `codex-rs/provenance/src/capture.rs` - `TraceRecorder`, activity, interval, transition, and observation contracts.
- `codex-rs/provenance/src/artifact.rs` - exact artifact refs and workspace-state identity types.
- `codex-rs/provenance/src/projection.rs` - hunk lineage, projection job, and projected segment types.
- `codex-rs/provenance/src/query.rs` - query envelopes, range candidates, and query result types.
- `codex-rs/provenance/src/blob.rs` - blob descriptors, manifest entries, read results, audit DTOs.
- `codex-rs/provenance/src/export.rs` - stream descriptors, export event rows, export response DTOs.
- `codex-rs/provenance/src/tests.rs` - crate-local model/canonicalization tests.
- `codex-rs/provenance-store/Cargo.toml` - new `codex-provenance-store` crate manifest.
- `codex-rs/provenance-store/BUILD.bazel` - Bazel target with migration compile data.
- `codex-rs/provenance-store/migrations/0001_provenance_journal.sql` - ledger tables.
- `codex-rs/provenance-store/migrations/0002_exact_artifacts.sql` - artifact and workspace-state tables.
- `codex-rs/provenance-store/migrations/0003_projection_index.sql` - projection/query index tables.
- `codex-rs/provenance-store/src/lib.rs` - public store exports.
- `codex-rs/provenance-store/src/error.rs` - typed store errors.
- `codex-rs/provenance-store/src/db.rs` - SQLite pool/open/migration helpers.
- `codex-rs/provenance-store/src/journal.rs` - append-only event writes, hash-chain validation, raw reads.
- `codex-rs/provenance-store/src/streams.rs` - stream descriptors, epoch descriptors, stream handoff, stream head reads.
- `codex-rs/provenance-store/src/batches.rs` - replayable append batch reservation, visibility, outcome, and repair semantics.
- `codex-rs/provenance-store/src/artifacts.rs` - exact artifact and workspace-state persistence.
- `codex-rs/provenance-store/src/file_identity.rs` - file versions, file entity edges, path entries, and path replay persistence.
- `codex-rs/provenance-store/src/path_replay.rs` - path replay query helpers over workspace path entries.
- `codex-rs/provenance-store/src/revision_alias.rs` - exact Git revision alias persistence and supersession.
- `codex-rs/provenance-store/src/baseline.rs` - baseline creation and checkpoint/compaction helpers.
- `codex-rs/provenance-store/src/retention.rs` - retention roots and exact artifact garbage-collection eligibility.
- `codex-rs/provenance-store/src/projection.rs` - projection job and segment persistence.
- `codex-rs/provenance-store/src/recorder.rs` - production `TraceRecorder` implementation that appends hash-chained events.
- `codex-rs/provenance-store/src/query.rs` - query repository methods for app-server handlers.
- `codex-rs/provenance-store/src/export.rs` - raw replay and committed export repository methods.
- `codex-rs/provenance-store/src/blob.rs` - blob manifest/read audit repository methods.
- `codex-rs/app-server-protocol/src/protocol/v2/provenance.rs` - app-server v2 provenance DTOs.
- `codex-rs/app-server/src/provenance_api.rs` - app-server provenance request handlers.
- `codex-rs/app-server/src/provenance_mapping.rs` - app-server protocol to store model conversions.
- `codex-rs/app-server/src/provenance_store_provider.rs` - lazy local store construction for app-server.
- `codex-rs/core/src/provenance/mod.rs` - thin core integration module with explicit public exports.
- `codex-rs/core/src/provenance/supervisor.rs` - core-facing supervisor adapter.
- `codex-rs/core/src/provenance/supervised_fs.rs` - `ExecutorFileSystem` wrapper exported for app-server integration.
- `codex-rs/core/src/provenance/supervised_command.rs` - command/unified-exec supervision helpers exported for app-server integration.

Modify:

- `codex-rs/Cargo.toml` - add workspace members and workspace dependency entries for new crates.
- `codex-rs/app-server-protocol/src/protocol/v2.rs` - add `pub mod provenance; pub use provenance::*;`.
- `codex-rs/app-server-protocol/src/protocol/common.rs` - register provenance v2 client request methods.
- `codex-rs/app-server/src/lib.rs` - include provenance modules.
- `codex-rs/app-server/src/codex_message_processor.rs` - route provenance client requests.
- `codex-rs/app-server/src/fs_api.rs` - use supervised filesystem wrapper for mutating fs APIs.
- `codex-rs/app-server/src/command_exec.rs` - open/close supervised command activities and intervals.
- `codex-rs/core/src/tools/context.rs` - carry optional provenance recorder/supervisor handles through tool invocation.
- `codex-rs/core/src/tools/handlers/apply_patch.rs` - record exact mutator observations.
- `codex-rs/core/src/tools/handlers/shell.rs` - record opaque mutator observations.
- `codex-rs/core/src/tools/handlers/unified_exec.rs` - record long-running mutator observations.
- `codex-rs/core/src/tools/handlers/js_repl.rs` - record JS REPL mutator activities.
- `codex-rs/core/src/tools/handlers/mcp.rs` - record write-capable MCP calls or unsupervised observations.
- `codex-rs/core/src/tools/handlers/dynamic.rs` - record mutating dynamic tool calls.
- `codex-rs/core/src/tools/handlers/agent_jobs.rs` - record job and worker activity for platform-started AI execution.
- `codex-rs/core/src/tools/registry.rs` - keep mutating classification aligned with provenance capture.
- `codex-rs/codex-mcp/src/mcp_connection_manager.rs` - surface MCP capability metadata near MCP tool metadata handling.
- `codex-rs/core/src/tools/events.rs` - keep existing user-visible events separate from provenance facts.
- `docs/superpowers/specs/provenance/*.md` - update only when implementation reveals a contract issue.

Do not modify:

- Any code related to `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` or `CODEX_SANDBOX_ENV_VAR`.
- `thread/read` semantics. Provenance uses separate app-server methods.
- TUI UI. Review and postmortem presentation stays outside this implementation.

## Dependency Graph

1. Task 1 must land before all other code tasks.
2. Task 2 can run after Task 1 and can be parallel with Task 3 after shared DTO names are stable.
3. Task 3 must land before real app-server handlers, capture persistence, projection, export, and blob audit.
4. Task 4 can expose read surfaces backed by an empty store after Tasks 2 and 3.
5. Task 5 must land before exact range queries and blob reconstruction guarantees.
6. Task 6 must land before `provenanceRange/read` can return real lineage.
7. Task 7 wires the core supervisor skeleton after the journal contract exists; Task 8 wires production capture after the journal and exact artifact store exist.
8. Tasks 9 and 10 complete query/export/blob semantics.
9. Task 11 hardens end-to-end behavior and should be the last implementation PR.

## Required RPC Implementation Matrix

Every required app-server method must appear in four places before its task is complete: DTO, protocol registration, store/API implementation, and tests.

| Method | DTO task | Implementation task | Required tests |
| --- | --- | --- | --- |
| `provenanceWorkspaceStream/list` | Task 2 | Task 4 | list filters, cursor stability, include closed |
| `provenanceLedgerStream/list` | Task 2 | Task 4 | scope filter, cursor stability, closed streams |
| `provenanceStreamEpoch/list` | Task 2 | Task 4 | epoch ordering and unknown stream |
| `provenanceLedgerStream/read` | Task 2 | Task 4 | matched descriptor and unavailable descriptor |
| `provenanceStreamHead/read` | Task 2 | Task 4 | live selector match, ambiguity, unavailable |
| `provenanceRevisionAlias/list` | Task 2 | Task 5 | commit/tree filters and superseded aliases |
| `provenanceRevisionAlias/read` | Task 2 | Task 5 | matched alias and unavailable direct id |
| `provenanceSchema/read` | Task 2 | Task 4 | exact bundle selectors, default latest contract, constraint mismatch |
| `provenanceRange/read` | Task 2 | Tasks 6 and 9 | selector mismatch, matched empty range, candidate lineage pagination |
| `provenanceTurn/read` | Task 2 | Task 9 | matched direct id and unavailable direct id |
| `provenanceHunk/read` | Task 2 | Task 9 | include parents/evidence and unavailable direct id |
| `provenanceActivity/read` | Task 2 | Task 9 | include children/intervals and unavailable direct id |
| `provenanceMutationInterval/read` | Task 2 | Task 9 | evidence/code-fact expansion and unavailable direct id |
| `provenanceWorkspaceTransition/read` | Task 2 | Task 9 | observations expansion and unavailable direct id |
| `provenanceMutationObservation/read` | Task 2 | Task 9 | matched observation and unavailable direct id |
| `provenanceEvent/exportRawReplay` | Task 2 | Task 10 | lossless row inclusion, cursor resume, schema bundle binding |
| `provenanceEvent/exportCommitted` | Task 2 | Task 10 | tentative/aborted/repaired batch exclusion |
| `provenanceBlobManifest/read` | Task 2 | Task 10 | descriptor uniqueness, availability refs, pagination |
| `provenanceBlob/read` | Task 2 | Task 10 | fail-closed audit, selector failure, inline chunk continuation |

## Execution Rules

- Use @superpowers:subagent-driven-development for execution unless there is a reason to keep a task inline.
- Use @superpowers:test-driven-development for each implementation task.
- Use @superpowers:verification-before-completion before claiming a task is done.
- Each task should end with one commit.
- Run `just fmt` from `codex-rs` after Rust edits.
- Run the task's concrete scoped lint command from `codex-rs` before finalizing Rust edits, for example `just fix -p codex-provenance`.
- If `Cargo.toml` or `Cargo.lock` dependencies change, run `just bazel-lock-update` and `just bazel-lock-check` from repo root.
- If app-server v2 schema changes, run `just write-app-server-schema` from repo root and include generated fixture changes.
- Do not run the full workspace test suite without user approval. Run crate-specific tests by default.

## Task 0: Workspace and Baseline Verification

**Files:**
- Read: `docs/superpowers/specs/provenance/README.md`
- Read: `AGENTS.md`
- No code changes.

- [ ] **Step 1: Verify clean starting state**

Run:

```bash
git status --short
```

Expected: no output.

- [ ] **Step 2: Create a feature branch**

Run:

```bash
git switch -c provenance-kernel-phase-0
```

Expected: branch is created from current `main`.

- [ ] **Step 3: Read the spec index**

Run:

```bash
sed -n '1,120p' docs/superpowers/specs/provenance/README.md
```

Expected: the reading order and shared definition map are visible.

- [ ] **Step 4: Run current protocol tests as baseline**

Run:

```bash
cd codex-rs && cargo test -p codex-app-server-protocol
```

Expected: PASS before provenance changes.

- [ ] **Step 5: Commit is not required**

No commit for baseline-only work.

## Task 1: Add `codex-provenance` Contract Crate

**Files:**
- Modify: `codex-rs/Cargo.toml`
- Create: `codex-rs/provenance/Cargo.toml`
- Create: `codex-rs/provenance/BUILD.bazel`
- Create: `codex-rs/provenance/src/lib.rs`
- Create: `codex-rs/provenance/src/canonical.rs`
- Create: `codex-rs/provenance/src/ids.rs`
- Create: `codex-rs/provenance/src/status.rs`
- Create: `codex-rs/provenance/src/selectors.rs`
- Create: `codex-rs/provenance/src/event.rs`
- Create: `codex-rs/provenance/src/schema.rs`
- Create: `codex-rs/provenance/src/artifact.rs`
- Create: `codex-rs/provenance/src/projection.rs`
- Create: `codex-rs/provenance/src/query.rs`
- Create: `codex-rs/provenance/src/blob.rs`
- Create: `codex-rs/provenance/src/export.rs`
- Create: `codex-rs/provenance/src/tests.rs`

- [ ] **Step 1: Write failing serialization tests**

Add this to `codex-rs/provenance/src/tests.rs`:

```rust
use crate::ids::{EventRef, LedgerScope};
use crate::canonical::canonical_json_bytes;
use crate::status::{Freshness, QueryResolutionStatus, SelectorStatus, StatusAnchor};
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn event_ref_serializes_camel_case_with_lower_camel_scope() {
    let event_ref = EventRef {
        ledger_scope: LedgerScope::WorkspaceCustody,
        stream_id: "stream-1".to_string(),
        stream_epoch: 7,
        sequence: 42,
        event_id: "event-1".to_string(),
        event_hash: "abc123".to_string(),
    };

    let actual = serde_json::to_value(event_ref).unwrap();

    assert_eq!(
        actual,
        json!({
            "ledgerScope": "workspaceCustody",
            "streamId": "stream-1",
            "streamEpoch": 7,
            "sequence": 42,
            "eventId": "event-1",
            "eventHash": "abc123"
        })
    );
}

#[test]
fn query_status_serializes_all_dimensions() {
    let status = QueryResolutionStatus {
        selector_status: SelectorStatus::Matched,
        freshness: Freshness::NotApplicable,
        reason_codes: vec![],
        status_anchor: StatusAnchor::None,
    };

    let actual = serde_json::to_value(status).unwrap();

    assert_eq!(
        actual,
        json!({
            "selectorStatus": "matched",
            "freshness": "notApplicable",
            "reasonCodes": [],
            "statusAnchor": { "type": "none" }
        })
    );
}

#[test]
fn canonical_json_uses_rfc_8785_stable_key_order_and_wire_names() {
    let value = serde_json::json!({
        "streamId": "stream-1",
        "ledgerScope": "workspaceCustody",
        "sequence": 42
    });

    let actual = String::from_utf8(canonical_json_bytes(&value).unwrap()).unwrap();

    assert_eq!(
        actual,
        r#"{"ledgerScope":"workspaceCustody","sequence":42,"streamId":"stream-1"}"#
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance
```

Expected: FAIL because the crate and types do not exist.

- [ ] **Step 3: Create crate manifest and Bazel target**

Create `codex-rs/provenance/Cargo.toml`:

```toml
[package]
name = "codex-provenance"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
anyhow = { workspace = true }
async-trait = { workspace = true }
schemars = { workspace = true }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
strum = { workspace = true, features = ["derive"] }
thiserror = { workspace = true }
ts-rs = { workspace = true, features = ["serde-json-impl", "uuid-impl", "no-serde-warnings"] }
uuid = { workspace = true }

[dev-dependencies]
pretty_assertions = { workspace = true }

[lints]
workspace = true
```

Create `codex-rs/provenance/BUILD.bazel`:

```starlark
load("//:defs.bzl", "codex_rust_crate")

codex_rust_crate(
    name = "provenance",
    crate_name = "codex_provenance",
)
```

Modify `codex-rs/Cargo.toml`:

```toml
# In [workspace].members, add:
"provenance",

# In [workspace.dependencies], add:
codex-provenance = { path = "provenance" }
```

- [ ] **Step 4: Implement the minimal public model surface**

Use Rust snake_case field names and serde lower-camel wire names.

Create `codex-rs/provenance/src/lib.rs`:

```rust
pub mod canonical;
pub mod artifact;
pub mod blob;
pub mod capture;
pub mod event;
pub mod export;
pub mod ids;
pub mod projection;
pub mod query;
pub mod schema;
pub mod selectors;
pub mod status;

#[cfg(test)]
mod tests;
```

Create `codex-rs/provenance/src/canonical.rs` with a small, tested wrapper around the selected RFC 8785 implementation. If no existing workspace dependency supports RFC 8785 JSON canonicalization, add one deliberately and run the Bazel lock update/check steps. Do not use `serde_json::to_string` as the hash input implementation.

Create `codex-rs/provenance/src/capture.rs` before moving to core integration. It must define the public capture contracts that `codex-core` will consume later:

```rust
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::artifact::WorkspaceStateRecord;
use crate::ids::EventRef;
use crate::query::ActivityRef;
use crate::query::ActivityStatus;
use crate::query::CausalityRefs;
use crate::query::CauseRef;
use crate::query::ClosedMutationInterval;
use crate::query::EvidenceCollectionMode;
use crate::query::ExecutionContext;
use crate::query::MutationIntervalRef;
use crate::query::MutationObservationRef;
use crate::status::RecorderStatus;

#[async_trait]
pub trait TraceRecorder: Send + Sync + 'static {
    async fn record_activity_started(&self, input: ActivityStartedInput) -> anyhow::Result<ActivityRef>;
    async fn record_activity_finished(&self, input: ActivityFinishedInput) -> anyhow::Result<ActivityRef>;
    async fn record_mutation_interval_opened(&self, input: MutationIntervalOpenedInput) -> anyhow::Result<MutationIntervalRef>;
    async fn record_mutation_observation(&self, input: MutationObservationInput) -> anyhow::Result<MutationObservationRef>;
    async fn record_mutation_interval_closed(&self, input: MutationIntervalClosedInput) -> anyhow::Result<ClosedMutationInterval>;
    async fn record_unsupervised_workspace_mutation(&self, input: WorkspaceObservationInput) -> anyhow::Result<EventRef>;
    async fn record_external_workspace_mutation(&self, input: WorkspaceObservationInput) -> anyhow::Result<EventRef>;
}

#[derive(Clone)]
pub struct SharedTraceRecorder(std::sync::Arc<dyn TraceRecorder>);

impl SharedTraceRecorder {
    pub fn new(recorder: impl TraceRecorder) -> Self {
        Self(std::sync::Arc::new(recorder))
    }

    pub fn from_arc(recorder: std::sync::Arc<dyn TraceRecorder>) -> Self {
        Self(recorder)
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ActivityStartedInput {
    pub activity_id: String,
    pub activity_kind: String,
    pub execution_context: Option<ExecutionContext>,
    pub causality_refs: CausalityRefs,
    pub workspace_stream_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ActivityFinishedInput {
    pub activity_id: String,
    pub status: ActivityStatus,
    pub recorder_status: RecorderStatus,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MutationIntervalOpenedInput {
    pub mutation_interval_id: String,
    pub activity_id: String,
    pub workspace_stream_id: String,
    pub workspace_instance_id: String,
    pub pre_workspace_state_id: Option<String>,
    pub evidence_collection_mode: EvidenceCollectionMode,
    pub execution_context: Option<ExecutionContext>,
    pub causality_refs: CausalityRefs,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MutationObservationInput {
    pub observation_id: String,
    pub mutation_interval_id: String,
    pub workspace_stream_id: String,
    pub observation_kind: MutationObservationKind,
    pub touched_paths: Vec<String>,
    pub source_file_version_ids: Vec<String>,
    pub target_file_version_ids: Vec<String>,
    pub evidence_refs: Vec<EventRef>,
    pub reason_codes: Vec<String>,
    pub recorder_status: RecorderStatus,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MutationIntervalClosedInput {
    pub mutation_interval_id: String,
    pub workspace_stream_id: String,
    pub workspace_instance_id: String,
    pub pre_workspace_state_id: Option<String>,
    pub post_workspace_state_id: String,
    pub post_workspace_state: WorkspaceStateRecord,
    pub workspace_transition_id: String,
    pub primary_cause: CauseRef,
    pub supporting_evidence_event_refs: Vec<EventRef>,
    pub touched_paths: Vec<String>,
    pub recorder_status: RecorderStatus,
    pub causality_refs: CausalityRefs,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceObservationInput {
    pub observation_id: String,
    pub workspace_stream_id: String,
    pub workspace_instance_id: String,
    pub touched_paths: Vec<String>,
    pub evidence_refs: Vec<EventRef>,
    pub reason_codes: Vec<String>,
    pub recorder_status: RecorderStatus,
    pub execution_context: Option<ExecutionContext>,
    pub causality_refs: CausalityRefs,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum MutationObservationKind {
    AuthoredEdit,
    RevisionTransition,
    FormatterRewrite,
    GeneratorOutput,
    OpaqueBulkRewrite,
}
```

Also define `ActivityRef`, `MutationIntervalRef`, `MutationObservationRef`, `WorkspaceTransitionRef`, `ClosedMutationInterval`, `ExecutionContext`, `CausalityRefs`, `CauseRef`, `ActivityStatus`, and `EvidenceCollectionMode` in the appropriate model modules and re-export them through `capture.rs` or `query.rs`. The exact fields must match `docs/superpowers/specs/provenance/01-execution-workspace-capture.md`, `02-ledger-kernel.md`, and `05-query-apis.md`. Do not add ID-only recorder methods; convenience constructors used by tests must still produce these full typed inputs.

Keep `TraceRecorder` object-safe. `MutationSupervisor` and `ToolInvocation` should carry a cloneable `SharedTraceRecorder`/`Arc<dyn TraceRecorder>` wrapper rather than making tool context generic over a recorder type.

Add explicit `for_test(...)` constructors for the capture input structs used by later core tests. These constructors are allowed to fill deterministic test defaults, but they must not be used by production code paths that have real execution context, workspace state, or evidence refs available.

Create `codex-rs/provenance/src/ids.rs`:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct EventRef {
    pub ledger_scope: LedgerScope,
    pub stream_id: String,
    pub stream_epoch: u64,
    pub sequence: u64,
    pub event_id: String,
    pub event_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum LedgerScope {
    GlobalExecution,
    WorkspaceCustody,
    AccessAudit,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LedgerStreamRef {
    pub ledger_scope: LedgerScope,
    pub stream_id: String,
    pub stream_epoch: u64,
}
```

Create `codex-rs/provenance/src/status.rs`:

```rust
use crate::ids::EventRef;
use crate::ids::LedgerScope;
use crate::selectors::CodeRangeSelector;
use crate::selectors::LiveWorkspaceSelector;
use crate::selectors::WorkspaceCustodySelector;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum SelectorStatus {
    Matched,
    ConstraintMismatch,
    Ambiguous,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum Freshness {
    Current,
    Stale,
    NotApplicable,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct QueryResolutionStatus {
    pub selector_status: SelectorStatus,
    pub freshness: Freshness,
    pub reason_codes: Vec<String>,
    pub status_anchor: StatusAnchor,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type", rename_all = "camelCase", export_to = "v2/")]
pub enum StatusAnchor {
    WorkspaceState { workspace_state_id: String },
    WorkspaceStream { workspace_stream_id: String },
    LedgerStream { ledger_scope: LedgerScope, stream_id: String },
    StreamEpoch { ledger_scope: LedgerScope, stream_id: String, stream_epoch: u64 },
    LiveWorkspaceSelector { selector: LiveWorkspaceSelector },
    BlobRef { selector: WorkspaceCustodySelector, blob_ref_id: String },
    BlobReadAttempt {
        selector: WorkspaceCustodySelector,
        blob_ref_id: String,
        expected_descriptor_event_ref: Option<EventRef>,
        continuation_token_fingerprint: Option<String>,
    },
    BlobReadSession {
        selector: WorkspaceCustodySelector,
        blob_ref_id: String,
        descriptor_event_ref: EventRef,
        availability_event_ref: EventRef,
        blob_read_session_id: String,
    },
    Turn { session_id: String, thread_id: String, turn_id: String },
    Hunk { hunk_id: String },
    Activity { activity_id: String, event_ref: Option<EventRef> },
    MutationInterval { mutation_interval_id: String, event_ref: Option<EventRef> },
    WorkspaceTransition { transition_id: String, event_ref: Option<EventRef> },
    MutationObservation { observation_id: String, event_ref: Option<EventRef> },
    CodeRevision { selector: CodeRangeSelector },
    CodeRangeRequest {
        selector: CodeRangeSelector,
        path: String,
        range: crate::query::QueryLineRange,
        expected_range_fingerprint: Option<String>,
    },
    SchemaBundle {
        schema_bundle_id: Option<String>,
        schema_bundle_digest: Option<String>,
        schema_version: Option<String>,
        export_contract_version: Option<String>,
    },
    Event { event_ref: EventRef },
    None,
}
```

For `selectors.rs`, `event.rs`, `schema.rs`, `artifact.rs`, `projection.rs`, `query.rs`, `blob.rs`, and `export.rs`, transcribe the normative DTOs from the shared definition map in `docs/superpowers/specs/provenance/README.md`. Keep repeated DTOs in exactly one Rust module and re-export rather than redefining them.

`schema.rs` must build a real local `TraceSchemaBundle` and `EventPayloadRegistryEntry` list for every v1 event type named in `docs/superpowers/specs/provenance/06-export-ingest.md`. `provenanceSchema/read` must not be able to pass with an empty bundle.

- [ ] **Step 5: Run tests**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance
```

Expected: PASS.

- [ ] **Step 6: Format and lint**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-provenance
```

Expected: formatting succeeds; clippy fix exits successfully.

If an RFC 8785 crate or any other external dependency was added, also run from the repo root:

```bash
just bazel-lock-update
just bazel-lock-check
```

- [ ] **Step 7: Commit**

Run:

```bash
git add codex-rs/Cargo.toml codex-rs/Cargo.lock MODULE.bazel.lock codex-rs/provenance
git commit -m "Add provenance contract crate"
```

Expected: one commit containing `codex-provenance` setup, model tests, and any lockfile updates required by the selected RFC 8785 dependency. If no new external dependency was added and lockfiles did not change, `git add` leaves them unchanged.

## Task 2: Add App-Server Provenance DTOs and Method Registration

**Files:**
- Modify: `codex-rs/app-server-protocol/Cargo.toml`
- Modify: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Create: `codex-rs/app-server-protocol/src/protocol/v2/provenance.rs`
- Modify: `codex-rs/app-server-protocol/src/protocol/common.rs`
- Test: `codex-rs/app-server-protocol/src/protocol/v2.rs`
- Generated: app-server schema fixtures produced by `just write-app-server-schema`

- [ ] **Step 1: Write failing request serialization tests**

Add tests near the existing app-server protocol tests in `codex-rs/app-server-protocol/src/protocol/v2.rs`:

```rust
#[test]
fn provenance_range_read_request_serializes_camel_case() -> anyhow::Result<()> {
    use crate::ClientRequest;
    use crate::protocol::v2::provenance::*;
    use serde_json::json;

    let request = ClientRequest::ProvenanceRangeRead {
        request_id: "req-1".into(),
        params: ReadRangeParams {
            selector: CodeRangeSelector::RecordedState {
                workspace_state_id: "state-1".to_string(),
            },
            path: "src/lib.rs".to_string(),
            range: QueryLineRange {
                start_line: 3,
                end_line: 8,
            },
            expected_range_fingerprint: None,
            include_lineage: true,
            max_depth: None,
            segment_limit: None,
            lineage_candidate_id: None,
            cursor: None,
        },
    };

    let actual = serde_json::to_value(request)?;

    assert_eq!(
        actual,
        json!({
            "id": "req-1",
            "method": "provenanceRange/read",
            "params": {
                "selector": {
                    "type": "recordedState",
                    "workspaceStateId": "state-1"
                },
                "path": "src/lib.rs",
                "range": {
                    "startLine": 3,
                    "endLine": 8
                },
                "expectedRangeFingerprint": null,
                "includeLineage": true,
                "maxDepth": null,
                "segmentLimit": null,
                "lineageCandidateId": null,
                "cursor": null
            }
        })
    );

    Ok(())
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-app-server-protocol provenance_range_read_request_serializes_camel_case
```

Expected: FAIL because the request variant and DTOs do not exist.

- [ ] **Step 3: Add protocol dependency and v2 submodule**

Modify `codex-rs/app-server-protocol/Cargo.toml`:

```toml
codex-provenance = { workspace = true }
```

Modify `codex-rs/app-server-protocol/src/protocol/v2.rs` near the top-level module area:

```rust
pub mod provenance;
pub use provenance::*;
```

Create `codex-rs/app-server-protocol/src/protocol/v2/provenance.rs` and define request/response DTOs by wrapping or re-exporting `codex_provenance` DTOs. Use concrete `*Params` and `*Response` types, `#[serde(rename_all = "camelCase")]`, and `#[ts(export_to = "v2/")]`. Every `Option<T>` field in a client-to-server `*Params` type must also have `#[ts(optional = nullable)]`.

Minimal starting point:

```rust
pub use codex_provenance::blob::*;
pub use codex_provenance::export::*;
pub use codex_provenance::ids::*;
pub use codex_provenance::query::*;
pub use codex_provenance::selectors::*;
pub use codex_provenance::status::*;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ReadRangeParams {
    pub selector: CodeRangeSelector,
    pub path: String,
    pub range: QueryLineRange,
    #[ts(optional = nullable)]
    pub expected_range_fingerprint: Option<String>,
    pub include_lineage: bool,
    #[ts(optional = nullable)]
    pub max_depth: Option<u32>,
    #[ts(optional = nullable)]
    pub segment_limit: Option<u32>,
    #[ts(optional = nullable)]
    pub lineage_candidate_id: Option<String>,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ReadRangeResponse {
    pub query_status: QueryResolutionStatus,
    pub requested_selector: CodeRangeSelector,
    pub requested_path: String,
    pub requested_range: QueryLineRange,
    pub expected_range_fingerprint: Option<String>,
    pub candidates: Vec<RangeResolutionCandidate>,
}
```

Continue through all required RPCs listed in `docs/superpowers/specs/provenance/05-query-apis.md`, `06-export-ingest.md`, and `07-blob-access-audit.md`.

- [ ] **Step 4: Register methods in common protocol**

Modify the `client_requests!` block in `codex-rs/app-server-protocol/src/protocol/common.rs`:

```rust
#[experimental("provenance")]
ProvenanceRangeRead => "provenanceRange/read" {
    params: v2::ReadRangeParams,
    response: v2::ReadRangeResponse,
},
```

Add all provenance methods listed under "Required RPCs" in `docs/superpowers/specs/provenance/05-query-apis.md`.

- [ ] **Step 5: Regenerate app-server schema fixtures**

Run:

```bash
just write-app-server-schema --experimental
```

Expected: schema fixture files update and include provenance methods/types under `v2/`.

- [ ] **Step 6: Run protocol tests**

Run:

```bash
cd codex-rs && cargo test -p codex-app-server-protocol
```

Expected: PASS.

- [ ] **Step 7: Format, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-app-server-protocol
git add codex-rs/app-server-protocol codex-rs/Cargo.toml
git commit -m "Add provenance app-server protocol contracts"
```

Expected: one commit with protocol DTOs, method registration, and schema fixtures.

## Task 3: Add Ledger Store, Epochs, and Replayable Append Batches

**Files:**
- Modify: `codex-rs/Cargo.toml`
- Create: `codex-rs/provenance-store/Cargo.toml`
- Create: `codex-rs/provenance-store/BUILD.bazel`
- Create: `codex-rs/provenance-store/migrations/0001_provenance_journal.sql`
- Create: `codex-rs/provenance-store/src/lib.rs`
- Create: `codex-rs/provenance-store/src/error.rs`
- Create: `codex-rs/provenance-store/src/db.rs`
- Create: `codex-rs/provenance-store/src/journal.rs`
- Create: `codex-rs/provenance-store/src/streams.rs`
- Create: `codex-rs/provenance-store/src/batches.rs`
- Create: `codex-rs/provenance-store/src/query.rs`
- Create: `codex-rs/provenance-store/src/tests.rs`

- [ ] **Step 1: Write failing ledger tests**

Create `codex-rs/provenance-store/src/tests.rs`:

```rust
use codex_provenance::ids::LedgerScope;
use pretty_assertions::assert_eq;

use crate::db::ProvenanceDatabase;
use crate::journal::AppendEvent;
use crate::journal::JournalStore;
use crate::streams::OpenEpoch;

#[tokio::test]
async fn epoch_must_start_with_epoch_started_event() -> anyhow::Result<()> {
    let db = ProvenanceDatabase::open_in_memory().await?;
    let journal = JournalStore::new(db.pool().clone());

    journal
        .open_epoch(OpenEpoch {
            ledger_scope: LedgerScope::WorkspaceCustody,
            stream_id: "workspace-stream-1".to_string(),
            stream_epoch: 1,
            idempotency_key: "epoch-start-1".to_string(),
        })
        .await?;

    let head = journal
        .read_stream_head(LedgerScope::WorkspaceCustody, "workspace-stream-1", 1)
        .await?
        .expect("head exists");

    assert_eq!(head.sequence, 1);
    assert_eq!(head.event_type, "system.epoch_started");
    assert_eq!(head.previous_event_hash, JournalStore::GENESIS_PREVIOUS_HASH);
    Ok(())
}

#[tokio::test]
async fn append_rejects_wrong_previous_hash() -> anyhow::Result<()> {
    let db = ProvenanceDatabase::open_in_memory().await?;
    let journal = JournalStore::new(db.pool().clone());
    journal.open_test_epoch(LedgerScope::WorkspaceCustody, "stream-1", 1).await?;

    let first = AppendEvent::test_event(LedgerScope::WorkspaceCustody, "stream-1", 1, "event-1");
    let first_ref = journal.append_event(first).await?;
    assert_eq!(first_ref.sequence, 2);

    let mut second = AppendEvent::test_event(LedgerScope::WorkspaceCustody, "stream-1", 1, "event-2");
    second.previous_event_hash = Some("not-the-real-hash".to_string());

    let err = journal.append_event(second).await.unwrap_err();
    assert!(err.to_string().contains("previous hash mismatch"));
    Ok(())
}

#[tokio::test]
async fn replayable_batch_participants_stay_tentative_until_stream_terminus() -> anyhow::Result<()> {
    let db = ProvenanceDatabase::open_in_memory().await?;
    let journal = JournalStore::new(db.pool().clone());
    journal.open_test_epoch(LedgerScope::GlobalExecution, "exec-stream", 1).await?;
    journal.open_test_epoch(LedgerScope::WorkspaceCustody, "workspace-stream", 1).await?;

    let batch = journal
        .append_replayable_batch_for_test(vec![
            AppendEvent::test_event(LedgerScope::GlobalExecution, "exec-stream", 1, "exec-close"),
            AppendEvent::test_event(LedgerScope::WorkspaceCustody, "workspace-stream", 1, "state-close"),
        ])
        .await?;

    assert!(journal.read_committed_export_rows(batch.participant_streams[0].clone()).await?.is_empty());

    journal.commit_replayable_batch(batch.batch_id.clone(), "test-complete").await?;

    assert_eq!(
        journal
            .read_committed_export_rows(batch.participant_streams[0].clone())
            .await?
            .len(),
        1
    );
    Ok(())
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store
```

Expected: FAIL because the store crate does not exist.

- [ ] **Step 3: Create store crate**

Modify `codex-rs/Cargo.toml`:

```toml
members = [
    "provenance-store",
]

[workspace.dependencies]
codex-provenance-store = { path = "provenance-store" }
```

Merge these entries into the existing workspace member and dependency sections rather than replacing the full file.

Create `codex-rs/provenance-store/Cargo.toml`:

```toml
[package]
name = "codex-provenance-store"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
anyhow = { workspace = true }
codex-provenance = { workspace = true }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
sha2 = { workspace = true }
sqlx = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "sync", "time"] }

[dev-dependencies]
pretty_assertions = { workspace = true }
tempfile = { workspace = true }

[lints]
workspace = true
```

`sha2` already exists as a workspace dependency. If implementation chooses another RFC 8785 helper crate, add it deliberately and run the Bazel lock commands in Step 8.

Create `codex-rs/provenance-store/BUILD.bazel`:

```starlark
load("//:defs.bzl", "codex_rust_crate")

codex_rust_crate(
    name = "provenance-store",
    crate_name = "codex_provenance_store",
    compile_data = glob(["migrations/**"]),
)
```

- [ ] **Step 4: Add ledger migration**

Create `codex-rs/provenance-store/migrations/0001_provenance_journal.sql` with tables for streams, epochs, hash-chained events, replayable append batches, participants, per-stream outcome termini, stream claims, stream registrations, and handoffs:

```sql
CREATE TABLE ledger_streams (
  ledger_scope TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  current_stream_epoch INTEGER NOT NULL,
  stream_status TEXT NOT NULL,
  head_event_scope TEXT,
  head_event_stream_id TEXT,
  head_event_epoch INTEGER,
  head_event_sequence INTEGER,
  registered_stream_ref TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (ledger_scope, stream_id)
);

CREATE TABLE stream_epochs (
  ledger_scope TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_epoch INTEGER NOT NULL,
  epoch_status TEXT NOT NULL,
  first_sequence INTEGER NOT NULL,
  last_sequence INTEGER,
  last_event_hash TEXT,
  continuity_state_json TEXT NOT NULL,
  continuity_event_id TEXT NOT NULL,
  claim_event_id TEXT,
  registration_event_id TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (ledger_scope, stream_id, stream_epoch)
);

CREATE TABLE provenance_events (
  ledger_scope TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_epoch INTEGER NOT NULL,
  sequence INTEGER NOT NULL,
  event_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  category TEXT NOT NULL,
  schema_version TEXT NOT NULL,
  event_hash TEXT NOT NULL,
  previous_event_hash TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  append_batch_id TEXT,
  append_batch_index INTEGER,
  append_batch_size INTEGER,
  canonical_json TEXT NOT NULL,
  visibility TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (ledger_scope, stream_id, stream_epoch, sequence),
  UNIQUE (event_id),
  UNIQUE (ledger_scope, stream_id, stream_epoch, idempotency_key)
);

CREATE TABLE append_batches (
  append_batch_id TEXT PRIMARY KEY,
  outcome_kind TEXT NOT NULL,
  outcome_reason TEXT,
  created_at INTEGER NOT NULL,
  finalized_at INTEGER
);

CREATE TABLE append_batch_participants (
  append_batch_id TEXT NOT NULL,
  ledger_scope TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_epoch INTEGER NOT NULL,
  sequence INTEGER NOT NULL,
  participant_index INTEGER NOT NULL,
  event_hash TEXT NOT NULL,
  PRIMARY KEY (append_batch_id, participant_index)
);

CREATE TABLE append_batch_stream_termini (
  append_batch_id TEXT NOT NULL,
  ledger_scope TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_epoch INTEGER NOT NULL,
  terminus_sequence INTEGER NOT NULL,
  terminus_event_hash TEXT NOT NULL,
  PRIMARY KEY (append_batch_id, ledger_scope, stream_id, stream_epoch)
);

CREATE TABLE append_reservations (
  reservation_id TEXT PRIMARY KEY,
  ledger_scope TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_epoch INTEGER NOT NULL,
  idempotency_key TEXT NOT NULL,
  append_batch_id TEXT,
  reservation_status TEXT NOT NULL,
  materialized_sequence INTEGER,
  materialized_event_hash TEXT,
  created_at INTEGER NOT NULL,
  finalized_at INTEGER,
  UNIQUE (ledger_scope, stream_id, stream_epoch, idempotency_key)
);

CREATE TABLE stream_handoffs (
  handoff_event_id TEXT PRIMARY KEY,
  previous_ledger_scope TEXT NOT NULL,
  previous_stream_id TEXT NOT NULL,
  previous_stream_epoch INTEGER NOT NULL,
  previous_terminal_event_hash TEXT,
  previous_stream_closure_status TEXT NOT NULL,
  new_ledger_scope TEXT NOT NULL,
  new_stream_id TEXT NOT NULL,
  new_stream_epoch INTEGER NOT NULL
);

CREATE TABLE stream_claims (
  claim_event_id TEXT PRIMARY KEY,
  ledger_scope TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_epoch INTEGER NOT NULL,
  workspace_stream_id TEXT,
  workspace_instance_id TEXT,
  workspace_root_fingerprint TEXT,
  device_claim TEXT NOT NULL,
  repo_scope_claim TEXT,
  canonical_git_remote_claim TEXT
);

CREATE TABLE stream_registrations (
  registration_event_id TEXT PRIMARY KEY,
  ledger_scope TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_epoch INTEGER NOT NULL,
  registered_stream_ref TEXT NOT NULL,
  registered_at INTEGER NOT NULL
);

CREATE INDEX provenance_events_stream_head
ON provenance_events (ledger_scope, stream_id, stream_epoch, sequence DESC);

CREATE INDEX provenance_events_visible_export
ON provenance_events (ledger_scope, stream_id, stream_epoch, visibility, sequence ASC);
```

- [ ] **Step 5: Implement ledger, epoch, and batch semantics**

Implement:

- `ProvenanceDatabase::open_in_memory` and migration execution.
- `JournalStore::open_epoch` that appends `system.epoch_started` for genesis epochs and `system.epoch_rotated` for non-genesis epochs.
- `JournalStore::append_event` with gapless sequence allocation, previous-hash validation, category/scope validation, idempotency conflict detection, and RFC 8785 canonical hash input.
- `JournalStore::validate_event_refs` for every referenced `EventRef`: `(ledger_scope, stream_id, stream_epoch, sequence)`, `event_id`, and `event_hash` must resolve to the same durable event. Coordinate/id/hash mismatches are invalid.
- same-batch reference validation: a participant may reference only same-batch events with lower `append_batch_index`; same-batch cycles, forward refs, and missing materialized participants must abort or repair the whole batch before visibility.
- `JournalStore::reserve_append_slot` and `JournalStore::finalize_append_reservation` for durable append reservations that never materialize as events.
- `JournalStore::append_replayable_batch` that materializes participants with batch metadata and keeps them tentative until every participating stream has a durable stream-local outcome terminus.
- `JournalStore::commit_replayable_batch`, `abort_replayable_batch`, and `repair_replayable_batch`.
- `JournalStore::seal_stream_and_handoff` for the writable handoff path where predecessor seal, successor epoch opening, and successor handoff commit in one replayable batch.
- `JournalStore::read_stream_descriptor`, `list_ledger_streams`, `list_stream_epochs`, `read_stream_head`, `read_raw_replay_rows`, and `read_committed_export_rows`.
- `ProvenanceQueryStore` in `query.rs` with store-native request/response structs for discovery and schema reads:
  - `ListWorkspaceStreamsRequest` / `ListWorkspaceStreamsResult`
  - `ListLedgerStreamsRequest` / `ListLedgerStreamsResult`
  - `ListStreamEpochsRequest` / `ListStreamEpochsResult`
  - `ReadStreamDescriptorRequest` / `ReadStreamDescriptorResult`
  - `ReadStreamHeadRequest` / `ReadStreamHeadResult`
  - `ReadSchemaBundleRequest` / `ReadSchemaBundleResult`

These store-native structs should use model types from `codex-provenance` and plain Rust primitives only. Do not depend on `codex-app-server-protocol` from `codex-provenance-store`.

Do not allow committed export to read tentative, aborted, or repaired participant events as committed recorder truth.
Abort and repair payloads must use `AppendReservationRef` for reserved-but-unmaterialized append slots; they must not fake missing participants as `EventRef`s.

- [ ] **Step 6: Run ledger tests**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store journal
cd codex-rs && cargo test -p codex-provenance-store streams
cd codex-rs && cargo test -p codex-provenance-store batches
```

Expected: PASS.

- [ ] **Step 7: Add focused tests for stream rotation**

Add tests proving:

- non-genesis epoch starts with `system.epoch_rotated`
- broken-chain epoch opening records `BrokenChain`
- stream handoff uses `UnknownPrevious(reason = streamHandoff)` in the successor epoch
- predecessor seal and successor handoff are committed in the same replayable batch when predecessor is writable
- aborted append reservations use `AppendReservationRef`
- repaired batches include both materialized participant event refs and unmaterialized reservation refs
- referenced `EventRef`s reject coordinate/id/hash mismatches
- same-batch references to lower `append_batch_index` pass, while forward refs and cycles fail

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store stream_handoff
cd codex-rs && cargo test -p codex-provenance-store epoch_rotated
cd codex-rs && cargo test -p codex-provenance-store append_reservation
cd codex-rs && cargo test -p codex-provenance-store event_ref_validation
cd codex-rs && cargo test -p codex-provenance-store same_batch_refs
```

Expected: PASS.

- [ ] **Step 8: Format, lock, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-provenance-store
```

If new external dependencies were added:

```bash
just bazel-lock-update
just bazel-lock-check
```

Commit:

```bash
git add codex-rs/Cargo.toml codex-rs/Cargo.lock codex-rs/provenance-store MODULE.bazel.lock
git commit -m "Add provenance journal store"
```

Expected: one commit containing the store crate, migrations, and lockfile updates only if needed.

## Task 4: Wire App-Server Provenance Discovery and Schema Handlers

**Files:**
- Modify: `codex-rs/app-server/Cargo.toml`
- Modify: `codex-rs/app-server/src/lib.rs`
- Create: `codex-rs/app-server/src/provenance_api.rs`
- Create: `codex-rs/app-server/src/provenance_mapping.rs`
- Create: `codex-rs/app-server/src/provenance_store_provider.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Test: `codex-rs/app-server/src/provenance_api.rs`

- [ ] **Step 1: Write failing discovery and schema handler tests**

Add this test to `codex-rs/app-server/src/provenance_api.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::LedgerScope;
    use codex_app_server_protocol::ListStreamsParams;
    use codex_app_server_protocol::ReadSchemaBundleParams;
    use codex_app_server_protocol::ReadStreamDescriptorParams;
    use codex_app_server_protocol::SchemaBundleAvailability;
    use codex_app_server_protocol::SelectorStatus;

    #[tokio::test]
    async fn workspace_stream_list_returns_empty_page_for_empty_store() -> anyhow::Result<()> {
        let api = ProvenanceApi::new_for_test().await?;
        let response = api
            .list_workspace_streams(ListStreamsParams {
                repo_scope_id: None,
                workspace_instance_id: None,
                include_closed: false,
                cursor: None,
                limit: None,
            })
            .await?;

        assert!(response.data.is_empty());
        assert_eq!(response.next_cursor, None);
        Ok(())
    }

    #[tokio::test]
    async fn ledger_stream_read_returns_unavailable_for_missing_stream() -> anyhow::Result<()> {
        let api = ProvenanceApi::new_for_test().await?;
        let response = api
            .read_ledger_stream(ReadStreamDescriptorParams {
                ledger_scope: LedgerScope::WorkspaceCustody,
                stream_id: "missing-stream".to_string(),
            })
            .await?;

        assert_eq!(response.query_status.selector_status, SelectorStatus::Unavailable);
        assert!(response.ledger_stream_descriptor.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn schema_read_returns_local_bundle_for_latest_export_contract() -> anyhow::Result<()> {
        let api = ProvenanceApi::new_for_test().await?;
        let response = api
            .read_schema_bundle(ReadSchemaBundleParams {
                schema_bundle_id: None,
                schema_bundle_digest: None,
                schema_version: None,
                export_contract_version: None,
            })
            .await?;

        assert_eq!(response.status.availability, SchemaBundleAvailability::Available);
        assert!(response.schema_bundle.is_some());
        Ok(())
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-app-server provenance_api
```

Expected: FAIL because `ProvenanceApi` does not exist.

- [ ] **Step 3: Add app-server dependencies**

Modify `codex-rs/app-server/Cargo.toml`:

```toml
codex-provenance = { workspace = true }
codex-provenance-store = { workspace = true }
```

- [ ] **Step 4: Implement `ProvenanceApi`**

Create `codex-rs/app-server/src/provenance_api.rs`. This task implements the required discovery/schema RPCs:

- `provenanceWorkspaceStream/list`
- `provenanceLedgerStream/list`
- `provenanceStreamEpoch/list`
- `provenanceLedgerStream/read`
- `provenanceStreamHead/read`
- `provenanceSchema/read`

Range, entity, export, and blob APIs are routed later in Tasks 9 and 10.

Create `codex-rs/app-server/src/provenance_mapping.rs` in the same task. This file is the only place that converts between app-server protocol DTOs and `codex_provenance_store::query` request/response models. Do not add `codex-app-server-protocol` as a dependency of `codex-provenance-store`; the store crate must remain reusable by offline repair, export, and future service ingestion jobs without depending on app-server wire types.

Mapping file shape:

```rust
use codex_app_server_protocol as wire;
use codex_provenance_store::query as store_query;

pub(crate) fn map_list_workspace_streams_params(
    params: wire::ListStreamsParams,
) -> Result<store_query::ListWorkspaceStreamsRequest, wire::JSONRPCErrorError> {
    Ok(store_query::ListWorkspaceStreamsRequest {
        repo_scope_id: params.repo_scope_id,
        workspace_instance_id: params.workspace_instance_id,
        include_closed: params.include_closed,
        cursor: params.cursor,
        limit: params.limit,
    })
}

pub(crate) fn map_read_stream_descriptor_params(
    params: wire::ReadStreamDescriptorParams,
) -> Result<store_query::ReadStreamDescriptorRequest, wire::JSONRPCErrorError> {
    Ok(store_query::ReadStreamDescriptorRequest {
        ledger_scope: params.ledger_scope.into(),
        stream_id: params.stream_id,
    })
}

pub(crate) fn map_list_workspace_streams_response(
    response: store_query::ListWorkspaceStreamsResult,
) -> wire::ListStreamsResponse {
    wire::ListStreamsResponse {
        data: response.data.into_iter().map(Into::into).collect(),
        next_cursor: response.next_cursor,
    }
}
```

Implement the same explicit parameter and response conversions for the remaining five methods in this task: ledger stream list, stream epoch list, stream descriptor read, stream head read, and schema bundle read. Mapping failures, such as invalid enum conversion if store and wire types diverge, must return JSON-RPC invalid-params errors; store execution failures still go through `map_store_error`.

Initial shape:

```rust
use crate::provenance_mapping::map_list_ledger_streams_params;
use crate::provenance_mapping::map_list_ledger_streams_response;
use crate::provenance_mapping::map_list_stream_epochs_params;
use crate::provenance_mapping::map_list_stream_epochs_response;
use crate::provenance_mapping::map_list_workspace_streams_params;
use crate::provenance_mapping::map_list_workspace_streams_response;
use crate::provenance_mapping::map_read_schema_bundle_params;
use crate::provenance_mapping::map_read_schema_bundle_response;
use crate::provenance_mapping::map_read_stream_descriptor_params;
use crate::provenance_mapping::map_read_stream_descriptor_response;
use crate::provenance_mapping::map_read_stream_head_params;
use crate::provenance_mapping::map_read_stream_head_response;
use codex_app_server_protocol::ListLedgerStreamsParams;
use codex_app_server_protocol::ListLedgerStreamsResponse;
use codex_app_server_protocol::ListStreamEpochsParams;
use codex_app_server_protocol::ListStreamEpochsResponse;
use codex_app_server_protocol::ListStreamsParams;
use codex_app_server_protocol::ListStreamsResponse;
use codex_app_server_protocol::ReadSchemaBundleParams;
use codex_app_server_protocol::ReadSchemaBundleResponse;
use codex_app_server_protocol::ReadStreamDescriptorParams;
use codex_app_server_protocol::ReadStreamDescriptorResponse;
use codex_app_server_protocol::ReadStreamHeadParams;
use codex_app_server_protocol::ReadStreamHeadResponse;
use codex_provenance_store::query::ProvenanceQueryStore;

#[derive(Clone)]
pub(crate) struct ProvenanceApi {
    query_store: ProvenanceQueryStore,
}

impl ProvenanceApi {
    pub(crate) fn new(query_store: ProvenanceQueryStore) -> Self {
        Self { query_store }
    }

    pub(crate) async fn list_workspace_streams(&self, params: ListStreamsParams) -> Result<ListStreamsResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let request = map_list_workspace_streams_params(params)?;
        let response = self.query_store.list_workspace_streams(request).await.map_err(map_store_error)?;
        Ok(map_list_workspace_streams_response(response))
    }

    pub(crate) async fn list_ledger_streams(&self, params: ListLedgerStreamsParams) -> Result<ListLedgerStreamsResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let request = map_list_ledger_streams_params(params)?;
        let response = self.query_store.list_ledger_streams(request).await.map_err(map_store_error)?;
        Ok(map_list_ledger_streams_response(response))
    }

    pub(crate) async fn list_stream_epochs(&self, params: ListStreamEpochsParams) -> Result<ListStreamEpochsResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let request = map_list_stream_epochs_params(params)?;
        let response = self.query_store.list_stream_epochs(request).await.map_err(map_store_error)?;
        Ok(map_list_stream_epochs_response(response))
    }

    pub(crate) async fn read_ledger_stream(&self, params: ReadStreamDescriptorParams) -> Result<ReadStreamDescriptorResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let request = map_read_stream_descriptor_params(params)?;
        let response = self.query_store.read_ledger_stream(request).await.map_err(map_store_error)?;
        Ok(map_read_stream_descriptor_response(response))
    }

    pub(crate) async fn read_stream_head(&self, params: ReadStreamHeadParams) -> Result<ReadStreamHeadResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let request = map_read_stream_head_params(params)?;
        let response = self.query_store.read_stream_head(request).await.map_err(map_store_error)?;
        Ok(map_read_stream_head_response(response))
    }

    pub(crate) async fn read_schema_bundle(&self, params: ReadSchemaBundleParams) -> Result<ReadSchemaBundleResponse, codex_app_server_protocol::JSONRPCErrorError> {
        let request = map_read_schema_bundle_params(params)?;
        let response = self.query_store.read_schema_bundle(request).await.map_err(map_store_error)?;
        Ok(map_read_schema_bundle_response(response))
    }

    #[cfg(test)]
    pub(crate) async fn new_for_test() -> anyhow::Result<Self> {
        let db = codex_provenance_store::db::ProvenanceDatabase::open_in_memory().await?;
        Ok(Self::new(ProvenanceQueryStore::new(db.pool().clone())))
    }
}

fn map_store_error(err: codex_provenance_store::Error) -> codex_app_server_protocol::JSONRPCErrorError {
    codex_app_server_protocol::JSONRPCErrorError {
        code: crate::error_code::INTERNAL_ERROR_CODE,
        message: format!("provenance store error: {err}"),
        data: None,
    }
}
```

The store should own the actual unavailable response construction so app-server does not duplicate query semantics.

- [ ] **Step 5: Implement local store provider for read surfaces**

Create `codex-rs/app-server/src/provenance_store_provider.rs` with:

- `ProvenanceStoreProvider::disabled()` for default runtime behavior while the experimental feature is off.
- `ProvenanceStoreProvider::new_for_test()` backed by an in-memory or temp-file SQLite database.
- lazy construction of `ProvenanceDatabase` and `ProvenanceQueryStore`.
- explicit error reporting when the feature is enabled but the database cannot be opened.

This task only needs query-store construction. Task 8 extends the same provider to construct `StoreTraceRecorder` and `MutationSupervisor` for mutating capture.

- [ ] **Step 6: Route methods**

Add `mod provenance_api;`, `mod provenance_mapping;`, and `mod provenance_store_provider;` in `codex-rs/app-server/src/lib.rs`.

Add a `provenance_api: ProvenanceApi` field to `CodexMessageProcessor` if that struct is the request owner. In `process_request`, route the six discovery/schema `ClientRequest::Provenance*` variants to the matching method. Keep route bodies short; put logic in `provenance_api.rs`.

- [ ] **Step 7: Run app-server tests**

Run:

```bash
cd codex-rs && cargo test -p codex-app-server provenance_api
```

Expected: PASS.

- [ ] **Step 8: Format, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-app-server
git add codex-rs/app-server
git commit -m "Wire provenance app-server handlers"
```

Expected: one commit with app-server routing and empty-store behavior.

## Task 5: Add Exact Artifact, File Identity, Path Replay, and Revision Alias Store

**Files:**
- Create: `codex-rs/provenance-store/migrations/0002_exact_artifacts.sql`
- Modify: `codex-rs/provenance-store/src/artifacts.rs`
- Create: `codex-rs/provenance-store/src/file_identity.rs`
- Create: `codex-rs/provenance-store/src/path_replay.rs`
- Create: `codex-rs/provenance-store/src/revision_alias.rs`
- Create: `codex-rs/provenance-store/src/baseline.rs`
- Create: `codex-rs/provenance-store/src/retention.rs`
- Modify: `codex-rs/provenance-store/src/lib.rs`
- Modify: `codex-rs/app-server/src/provenance_api.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/provenance/src/artifact.rs`
- Modify: `codex-rs/provenance/src/projection.rs`
- Test: `codex-rs/provenance-store/src/artifacts.rs`
- Test: `codex-rs/provenance-store/src/path_replay.rs`
- Test: `codex-rs/provenance-store/src/revision_alias.rs`

- [ ] **Step 1: Write failing artifact tests**

Add tests in `codex-rs/provenance-store/src/artifacts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use codex_provenance::artifact::{ExactArtifactKind, ExactArtifactRef, StorageClass};
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn artifact_ref_is_content_addressed_and_idempotent() -> anyhow::Result<()> {
        let db = crate::db::ProvenanceDatabase::open_in_memory().await?;
        let store = ArtifactStore::new(db.pool().clone());
        let artifact = ExactArtifactRef::new_for_test(
            ExactArtifactKind::WorkingTreeBlob,
            "sha256:abc",
            StorageClass::LocalDurable,
        );

        let first = store.record_artifact(artifact.clone()).await?;
        let second = store.record_artifact(artifact).await?;

        assert_eq!(first, second);
        Ok(())
    }
}
```

- [ ] **Step 2: Write failing path replay and alias tests**

Add tests in `codex-rs/provenance-store/src/path_replay.rs` and `revision_alias.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn path_replay_resolves_file_version_from_changed_path_entry() -> anyhow::Result<()> {
        let db = crate::db::ProvenanceDatabase::open_in_memory().await?;
        let store = PathReplayStore::new(db.pool().clone());
        crate::tests::seed_workspace_state_with_file_version(
            db.pool(),
            "state-1",
            "src/lib.rs",
            "file-version-1",
        )
        .await?;

        let resolved = store
            .resolve_path("state-1", "src/lib.rs")
            .await?
            .expect("path should resolve");

        assert_eq!(resolved.file_version_id.as_deref(), Some("file-version-1"));
        Ok(())
    }

    #[tokio::test]
    async fn revision_alias_read_returns_unavailable_for_unknown_alias() -> anyhow::Result<()> {
        let db = crate::db::ProvenanceDatabase::open_in_memory().await?;
        let store = RevisionAliasStore::new(db.pool().clone());

        let response = store.read_revision_alias("missing-alias").await?;

        assert!(response.revision_alias.is_none());
        assert_eq!(response.query_status.selector_status, codex_provenance::status::SelectorStatus::Unavailable);
        Ok(())
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store artifact_ref_is_content_addressed_and_idempotent
cd codex-rs && cargo test -p codex-provenance-store path_replay_resolves_file_version_from_changed_path_entry
cd codex-rs && cargo test -p codex-provenance-store revision_alias_read_returns_unavailable_for_unknown_alias
```

Expected: FAIL because artifact, path replay, and revision alias persistence are not implemented.

- [ ] **Step 4: Add migration**

Create `codex-rs/provenance-store/migrations/0002_exact_artifacts.sql` with tables for exact artifacts, retention roots, workspace states, path entries, file versions, file entity edges, and revision aliases:

```sql
CREATE TABLE exact_artifacts (
  artifact_id TEXT PRIMARY KEY,
  artifact_kind TEXT NOT NULL,
  content_digest TEXT NOT NULL,
  storage_class TEXT NOT NULL,
  retention_class TEXT NOT NULL,
  byte_size INTEGER,
  created_at INTEGER NOT NULL,
  UNIQUE (artifact_kind, content_digest)
);

CREATE TABLE retention_roots (
  retention_root_id TEXT PRIMARY KEY,
  root_kind TEXT NOT NULL,
  target_artifact_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  expires_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE TABLE workspace_states (
  workspace_state_id TEXT PRIMARY KEY,
  workspace_stream_id TEXT NOT NULL,
  workspace_instance_id TEXT NOT NULL,
  repo_scope_id TEXT,
  state_kind TEXT NOT NULL,
  git_commit_oid TEXT,
  git_tree_oid TEXT,
  git_tree_durability TEXT NOT NULL,
  manifest_artifact_id TEXT,
  filesystem_delta_snapshot_artifact_id TEXT,
  filesystem_snapshot_artifact_id TEXT,
  path_delta_artifact_id TEXT,
  parent_workspace_state_ids_json TEXT NOT NULL,
  path_replay_parent_workspace_state_id TEXT,
  path_map_digest TEXT,
  path_replay_status TEXT NOT NULL,
  checkpoint_kind TEXT NOT NULL,
  created_event_id TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE workspace_path_entries (
  workspace_state_id TEXT NOT NULL,
  path TEXT NOT NULL,
  file_version_id TEXT,
  entry_kind TEXT NOT NULL,
  file_mode TEXT,
  line_count INTEGER,
  line_index_digest TEXT,
  PRIMARY KEY (workspace_state_id, path)
);

CREATE TABLE file_versions (
  file_version_id TEXT PRIMARY KEY,
  path TEXT NOT NULL,
  file_kind TEXT NOT NULL,
  file_mode TEXT,
  git_blob_oid TEXT,
  git_object_durability TEXT NOT NULL,
  blob_artifact_id TEXT,
  byte_size INTEGER,
  line_count INTEGER,
  line_index_digest TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE file_entity_edges (
  edge_id TEXT PRIMARY KEY,
  edge_kind TEXT NOT NULL,
  from_file_version_id TEXT,
  to_file_version_id TEXT NOT NULL,
  mutation_interval_id TEXT,
  cause_json TEXT NOT NULL,
  path_before TEXT,
  path_after TEXT,
  evidence_refs_json TEXT NOT NULL
);

CREATE TABLE revision_aliases (
  alias_id TEXT PRIMARY KEY,
  workspace_instance_id TEXT NOT NULL,
  workspace_state_id TEXT NOT NULL,
  git_commit_oid TEXT NOT NULL,
  git_tree_oid TEXT NOT NULL,
  created_event_ref_json TEXT NOT NULL,
  superseded_by_event_ref_json TEXT,
  alias_source TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE INDEX workspace_states_stream_head
ON workspace_states (workspace_stream_id, created_at DESC);

CREATE INDEX revision_aliases_commit_tree
ON revision_aliases (workspace_instance_id, git_commit_oid, git_tree_oid, alias_id);
```

- [ ] **Step 5: Implement store APIs**

Implement:

- `ArtifactStore::record_artifact`
- `ArtifactStore::read_artifact_ref`
- `ArtifactStore::record_workspace_state`
- `ArtifactStore::read_workspace_state`
- `ArtifactStore::read_workspace_head`
- `ArtifactStore::record_retention_root`
- `FileIdentityStore::record_file_version`
- `FileIdentityStore::record_file_entity_edge`
- `PathReplayStore::record_workspace_path_entries`
- `PathReplayStore::resolve_path`
- `PathReplayStore::resolve_exact_git_state_path`
- `PathReplayStore::resolve_delta_snapshot_path`
- `RevisionAliasStore::record_revision_alias`
- `RevisionAliasStore::list_revision_aliases`
- `RevisionAliasStore::read_revision_alias`
- `RevisionAliasStore::supersede_revision_alias`
- `ProvenanceApi::list_revision_aliases`
- `ProvenanceApi::read_revision_alias`
- `BaselineStore::record_exact_git_baseline`
- `BaselineStore::record_checkpoint_manifest`

Rules:

- Do not store mutable availability in `ExactArtifactRef`; availability belongs to blob facts.
- `file_version_id` is mandatory for every observed file state.
- `file_entity_id` is optional; ambiguous copy/split/merge continuity must use `FileEntityEdge::Ambiguous`.
- Path replay for `(workspace_state_id, path)` must use immutable recorded state input, never the live filesystem.
- If `ExactGitState.git_tree_durability` is not `reachable` or `storedUnreachable`, exact path replay must return unavailable.
- Revision aliases are append-only facts. Supersession records a new event ref; it must not mutate historical alias identity.

- [ ] **Step 6: Run artifact tests**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store artifacts
cd codex-rs && cargo test -p codex-provenance-store path_replay
cd codex-rs && cargo test -p codex-provenance-store revision_alias
cd codex-rs && cargo test -p codex-provenance-store file_identity
cd codex-rs && cargo test -p codex-provenance-store baseline
cd codex-rs && cargo test -p codex-provenance-store retention
cd codex-rs && cargo test -p codex-app-server provenance_api
```

Expected: PASS.

- [ ] **Step 7: Format, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-provenance-store && just fix -p codex-app-server
git add codex-rs/provenance codex-rs/provenance-store codex-rs/app-server
git commit -m "Add exact artifact and replay store"
```

Expected: one commit with artifact, workspace-state, file-version, path replay, revision alias, baseline, and retention persistence.

## Task 6: Add Projection Skeleton and Range Query Index

**Files:**
- Modify: `codex-rs/provenance/src/projection.rs`
- Create: `codex-rs/provenance-store/migrations/0003_projection_index.sql`
- Modify: `codex-rs/provenance-store/src/projection.rs`
- Modify: `codex-rs/provenance-store/src/query.rs`
- Test: `codex-rs/provenance-store/src/projection.rs`
- Test: `codex-rs/provenance-store/src/query.rs`

- [ ] **Step 1: Write failing projection query test**

Add this test in `codex-rs/provenance-store/src/query.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use codex_provenance::selectors::CodeRangeSelector;
    use codex_provenance::query::QueryLineRange;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn range_query_returns_ready_segment_for_recorded_state() -> anyhow::Result<()> {
        let db = crate::db::ProvenanceDatabase::open_in_memory().await?;
        crate::tests::seed_projected_segment(
            db.pool(),
            "state-1",
            "src/lib.rs",
            3,
            5,
            "hunk-1",
        )
        .await?;

        let store = ProvenanceQueryStore::new(db.pool().clone());
        let response = store
            .read_range_for_test(
                CodeRangeSelector::RecordedState {
                    workspace_state_id: "state-1".to_string(),
                },
                "src/lib.rs",
                QueryLineRange {
                    start_line: 4,
                    end_line: 4,
                },
            )
            .await?;

        assert_eq!(response.candidates.len(), 1);
        assert_eq!(response.candidates[0].terminal_segments[0].terminal_hunk_id, "hunk-1");
        Ok(())
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store range_query_returns_ready_segment_for_recorded_state
```

Expected: FAIL because projection tables and query methods do not exist.

- [ ] **Step 3: Add projection migration**

Create `codex-rs/provenance-store/migrations/0003_projection_index.sql` with hunk lineage, projection jobs, lineage revisions, and projected segments:

```sql
CREATE TABLE lineage_revisions (
  lineage_revision_id TEXT PRIMARY KEY,
  workspace_state_id TEXT NOT NULL,
  file_version_id TEXT NOT NULL,
  status TEXT NOT NULL,
  superseded_by_event_ref_json TEXT,
  created_event_ref_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE projection_jobs (
  projection_job_id TEXT PRIMARY KEY,
  lineage_revision_id TEXT NOT NULL,
  job_kind TEXT NOT NULL,
  input_workspace_state_id TEXT NOT NULL,
  output_workspace_state_id TEXT NOT NULL,
  touched_file_version_ids_json TEXT NOT NULL,
  source_file_version_ids_json TEXT NOT NULL,
  target_file_version_ids_json TEXT NOT NULL,
  status TEXT NOT NULL,
  reason_codes_json TEXT NOT NULL,
  dependent_projection_job_refs_json TEXT NOT NULL,
  superseded_lineage_revision_ids_json TEXT NOT NULL,
  started_event_ref_json TEXT NOT NULL,
  finished_event_ref_json TEXT,
  repaired_event_ref_json TEXT,
  updated_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE hunk_records (
  hunk_id TEXT PRIMARY KEY,
  lineage_revision_id TEXT NOT NULL,
  mutation_interval_id TEXT,
  cause_ref_json TEXT NOT NULL,
  input_file_version_id TEXT,
  output_file_version_id TEXT,
  input_range_json TEXT,
  output_range_json TEXT,
  operation TEXT NOT NULL,
  evidence_refs_json TEXT NOT NULL,
  certainty TEXT NOT NULL,
  coverage TEXT NOT NULL,
  recorder_status_json TEXT NOT NULL,
  observed_event_ref_json TEXT NOT NULL
);

CREATE TABLE hunk_parent_edges (
  edge_id TEXT PRIMARY KEY,
  child_hunk_id TEXT NOT NULL,
  edge_order INTEGER NOT NULL,
  parent_hunk_id TEXT,
  parent_file_version_id TEXT,
  parent_range_json TEXT,
  child_range_json TEXT,
  mapping_kind TEXT NOT NULL,
  overlap_weight REAL,
  merge_group_id TEXT,
  contribution_kind TEXT NOT NULL,
  certainty TEXT NOT NULL,
  coverage TEXT NOT NULL,
  evidence_refs_json TEXT NOT NULL
);

CREATE TABLE projected_segments (
  segment_id TEXT PRIMARY KEY,
  lineage_revision_id TEXT NOT NULL,
  workspace_state_id TEXT NOT NULL,
  file_version_id TEXT NOT NULL,
  path TEXT NOT NULL,
  line_start INTEGER NOT NULL,
  line_end INTEGER NOT NULL,
  terminal_hunk_id TEXT NOT NULL,
  range_fingerprint TEXT,
  indexing_status TEXT NOT NULL,
  coverage TEXT NOT NULL,
  applied_event_ref_json TEXT NOT NULL
);

CREATE INDEX projected_segments_lookup
ON projected_segments (workspace_state_id, path, line_start, line_end);

CREATE INDEX hunk_parent_edges_child
ON hunk_parent_edges (child_hunk_id, edge_order, edge_id);
```

- [ ] **Step 4: Implement projection code facts and repository**

Projection persistence is an index over replayable code facts, not the source of truth. Implement repository methods so every authoritative projection write first appends hash-chained workspace-custody events through `JournalStore`, then persists index rows that reference those event refs.

Required event writes:

- `code.projection_job_started` with full `ProjectionJobRecord`.
- `code.hunk_observed` for every persisted `HunkRecord`.
- `code.segment_projection_applied` for every persisted `ProjectedSegment`.
- `code.projection_job_finished` when a job reaches ready authoritative output.
- `code.projection_job_repaired` when a repair changes authoritative lineage or supersedes a prior lineage revision.

`ProjectionJobRecord` must preserve `projection_job_id`, `lineage_revision_id`, `job_kind`, input/output workspace states, touched/source/target file versions, dependent projection job refs, superseded lineage revision ids, status, and reason codes. Local `projection_jobs` rows may accelerate scheduling and queries, but replay from `TraceKernelEvent` rows must be sufficient to reconstruct the latest authoritative lineage revision.

Implement insert/read methods for lineage revisions, projection jobs, hunk records, parent edges, and projected segments. Hunk records must preserve the normative fields from `docs/superpowers/specs/provenance/04-code-projection.md`: cause ref, optional input/output file versions, optional input/output ranges, operation, evidence refs, certainty, coverage, and observed event ref. Parent edges must preserve edge order, parent/child ranges, mapping kind, optional overlap weight, optional merge group, contribution kind, certainty, coverage, and evidence refs.

Repository validation rules:

- `edge_order` must be stable and unique per `child_hunk_id`.
- exact mapping edges must not create cycles.
- exact one-to-one mappings must not overlap within the same child range.
- weighted-overlap mappings must include `overlap_weight`.
- merge groups must be internally consistent for all edges that share a `merge_group_id`.

If `include_lineage = true` and parent edges are not ready, return terminal segments with `indexing_status = Pending` and a reason code rather than guessing. Traversal must walk from `ProjectedSegment` to `HunkRecord` and then through `hunk_parent_edges`; it must not use `ProjectedSegment.segment_id` as a durable lineage edge.

- [ ] **Step 5: Run projection and query tests**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store projection
cd codex-rs && cargo test -p codex-provenance-store query
```

Expected: PASS.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-provenance-store
git add codex-rs/provenance codex-rs/provenance-store
git commit -m "Add provenance projection index"
```

Expected: one commit with projection skeleton and range query index.

## Task 7: Add Core Mutation Supervisor Skeleton

**Files:**
- Modify: `codex-rs/core/Cargo.toml`
- Modify: `codex-rs/core/src/lib.rs`
- Create: `codex-rs/core/src/provenance/mod.rs`
- Create: `codex-rs/core/src/provenance/supervisor.rs`
- Modify: `codex-rs/core/src/tools/context.rs`
- Test: `codex-rs/core/src/provenance/supervisor.rs`

- [ ] **Step 1: Write failing supervisor lifecycle test**

Create `codex-rs/core/src/provenance/supervisor.rs` with a test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use codex_provenance::capture::ActivityFinishedInput;
    use codex_provenance::capture::ActivityStartedInput;
    use codex_provenance::capture::MutationIntervalClosedInput;
    use codex_provenance::capture::MutationIntervalOpenedInput;
    use codex_provenance::capture::MutationObservationInput;
    use codex_provenance::capture::MutationObservationKind;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn supervisor_records_activity_interval_and_observation_order() -> anyhow::Result<()> {
        let recorder = InMemoryTraceRecorder::default();
        let supervisor = MutationSupervisor::new(recorder.clone());

        let activity = supervisor
            .record_activity_started(ActivityStartedInput::for_test("activity-1", "exactMutator"))
            .await?;
        let interval = supervisor
            .record_mutation_interval_opened(MutationIntervalOpenedInput::for_test(
                activity.activity_id.clone(),
                "interval-1",
                "workspace-stream-1",
                "workspace-instance-1",
                Some("state-before"),
            ))
            .await?;
        supervisor
            .record_mutation_observation(MutationObservationInput::for_test(
                interval.mutation_interval_id.clone(),
                "observation-1",
                MutationObservationKind::AuthoredEdit,
                vec!["src/lib.rs".to_string()],
            ))
            .await?;
        supervisor
            .record_mutation_interval_closed(MutationIntervalClosedInput::for_test(
                interval.mutation_interval_id,
                "workspace-stream-1",
                "workspace-instance-1",
                Some("state-before"),
                "state-after",
            ))
            .await?;
        supervisor
            .record_activity_finished(ActivityFinishedInput::for_test(activity.activity_id))
            .await?;

        assert_eq!(
            recorder.event_kinds().await,
            vec![
                "execution.activity_started",
                "execution.mutation_interval_opened",
                "code.mutation_observation_recorded",
                "execution.mutation_interval_closed",
                "execution.activity_finished",
            ]
        );
        Ok(())
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-core supervisor_records_activity_interval_and_observation_order
```

Expected: FAIL because core provenance module does not exist.

- [ ] **Step 3: Add core dependency and module**

Modify `codex-rs/core/Cargo.toml`:

```toml
codex-provenance = { workspace = true }
```

Modify `codex-rs/core/src/lib.rs`:

```rust
mod provenance;
pub use provenance::MutationSupervisor;
```

Implement `MutationSupervisor` as a cloneable thin adapter around `codex_provenance::capture::SharedTraceRecorder`. Keep it independent of SQLite so core tests can use `InMemoryTraceRecorder`. Do not make `MutationSupervisor` generic over recorder type; `ToolInvocation` must store one concrete supervisor type. Do not export supervised filesystem or command wrappers in this task; those files are created in Task 8, and each task should build as a standalone commit.

- [ ] **Step 4: Thread optional supervisor through tool context**

Modify `codex-rs/core/src/tools/context.rs`:

```rust
pub struct ToolInvocation {
    pub session: Arc<Session>,
    pub turn: Arc<TurnContext>,
    pub tracker: SharedTurnDiffTracker,
    pub provenance: Option<crate::MutationSupervisor>,
    pub call_id: String,
    pub tool_name: ToolName,
    pub payload: ToolPayload,
}
```

Update all constructors to pass `None` until real integration is added. This preserves behavior.

- [ ] **Step 5: Run core supervisor tests**

Run:

```bash
cd codex-rs && cargo test -p codex-core provenance::supervisor
```

Expected: PASS.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-core
git add codex-rs/core codex-rs/Cargo.toml
git commit -m "Add provenance mutation supervisor skeleton"
```

Expected: one commit with no behavior change except optional supervisor plumbing.

## Task 8: Supervise First-Party Mutating Ingress

**Files:**
- Modify: `codex-rs/core/src/lib.rs`
- Create: `codex-rs/core/src/provenance/supervised_fs.rs`
- Create: `codex-rs/core/src/provenance/supervised_command.rs`
- Create: `codex-rs/provenance-store/src/recorder.rs`
- Modify: `codex-rs/provenance-store/src/lib.rs`
- Modify: `codex-rs/app-server/src/fs_api.rs`
- Modify: `codex-rs/app-server/src/command_exec.rs`
- Modify: `codex-rs/app-server/src/provenance_store_provider.rs`
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/core/src/tools/handlers/apply_patch.rs`
- Modify: `codex-rs/core/src/tools/handlers/shell.rs`
- Modify: `codex-rs/core/src/tools/handlers/unified_exec.rs`
- Modify: `codex-rs/core/src/tools/handlers/js_repl.rs`
- Modify: `codex-rs/core/src/tools/handlers/mcp.rs`
- Modify: `codex-rs/core/src/tools/handlers/dynamic.rs`
- Modify: `codex-rs/core/src/tools/handlers/agent_jobs.rs`
- Modify: `codex-rs/core/src/tools/registry.rs`
- Modify: `codex-rs/codex-mcp/src/mcp_connection_manager.rs`
- Test: `codex-rs/core/src/provenance/supervised_fs.rs`
- Test: `codex-rs/provenance-store/src/recorder.rs`
- Test: `codex-rs/core/src/mcp_tool_call_tests.rs`
- Test: `codex-rs/core/src/tools/handlers/dynamic.rs`
- Test: `codex-rs/core/src/tools/handlers/agent_jobs_tests.rs`
- Test: existing handler tests near changed files.

- [ ] **Step 1: Write failing supervised filesystem test**

Add this test to `codex-rs/core/src/provenance/supervised_fs.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn write_file_opens_and_closes_exact_interval() -> anyhow::Result<()> {
        let recorder = InMemoryTraceRecorder::default();
        let supervisor = MutationSupervisor::new(recorder.clone());
        let fs = SupervisedExecutorFileSystem::new(MockExecutorFileSystem::default(), supervisor);

        fs.write_file("/tmp/work/src/lib.rs".as_ref(), b"hello".to_vec(), None).await?;

        assert_eq!(
            recorder.event_kinds().await,
            vec![
                "execution.activity_started",
                "execution.mutation_interval_opened",
                "code.mutation_observation_recorded",
                "execution.mutation_interval_closed",
                "execution.activity_finished",
            ]
        );
        Ok(())
    }
}
```

Add companion failing tests near the MCP, dynamic tool, and agent job handlers proving:

- a `StoreTraceRecorder`-backed supervisor appends queryable hash-chained lifecycle events to SQLite when an exact supervised write closes.
- session/thread/turn/item/tool-call/process lifecycle facts are emitted and can be used by `provenanceTurn/read`.
- overlapping Codex mutation intervals for the same workspace stream are rejected or queued by the writer lease.
- a tool call that touches two workspace streams produces two workspace-local mutation intervals and two workspace transitions.
- write-capable MCP tool calls open supervised activity/interval facts or emit explicit unsupervised-Codex observations when capability metadata is missing.
- dynamic tools are treated as mutating unless explicitly classified read-only and therefore pass through `MutationSupervisor`.
- agent job workers that can write the workspace create attributed activity records that include job id, item id, worker thread id, session id, and turn id.

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-core write_file_opens_and_closes_exact_interval
```

Expected: FAIL because the supervised wrapper is not implemented.

- [ ] **Step 3: Implement supervised wrappers**

Implement:

- `SupervisedExecutorFileSystem`
- `SupervisedCommandExec`
- `StoreTraceRecorder` in `codex-rs/provenance-store/src/recorder.rs`
- `WorkspaceWriterLease` keyed by `workspace_stream_id`
- multi-workspace write splitting that creates one interval/transition per affected workspace stream
- classification for `ExactMutator`, `OpaqueMutator`, `LongRunningMutator`, and `BulkMutator`
- `StructuredWriteTap` and `RuntimeWriteFence` attribution sources for long-running child intervals
- explicit fallback events for supervised paths that cannot provide exact custody

`StoreTraceRecorder` must implement `codex_provenance::capture::TraceRecorder` by converting the full typed capture inputs into hash-chained `TraceKernelEvent` rows through `JournalStore`. Session, thread, turn, item, tool-call, and process lifecycle callbacks must emit the corresponding `execution.*` event families before mutation intervals depend on them. Opening an activity emits `execution.activity_started`; closing it emits `execution.activity_finished`; opening an interval emits `execution.mutation_interval_opened`; recording an observation emits `code.mutation_observation_recorded`; closing an interval appends a replayable batch containing `execution.mutation_interval_closed` and the establishing `code.workspace_state_captured` event. The close path must validate that `MutationIntervalClosedInput.post_workspace_state_id` matches `post_workspace_state.workspace_state_id`, persist the exact state/artifact refs before appending the establishing event, and fail degraded rather than fabricating a state id. The store-backed recorder is the production path; `InMemoryTraceRecorder` remains test-only.

Do not compute hunk lineage here. Record only activity, interval, evidence refs, and observation facts. Exact child intervals for long-running processes may be emitted only when a supervised write callback provides `StructuredWriteTap` attribution or an explicit runtime begin/end fence provides `RuntimeWriteFence` attribution with an external-writer exclusion guarantee. File-watch-only, dirty-marker-only, or `write_stdin`-adjacent writes must degrade to process-level ambiguous drift facts and must not claim `certainty = Exact`.

Modify `codex-rs/core/src/lib.rs` after both wrapper modules exist:

```rust
pub use provenance::SupervisedCommandExec;
pub use provenance::SupervisedExecutorFileSystem;
```

- [ ] **Step 4: Wire mutating app-server fs calls**

In `codex-rs/app-server/src/provenance_store_provider.rs`, define the production lifecycle:

- disabled by default unless the experimental provenance API/config flag is enabled.
- local SQLite path is derived from the app-server state directory and repo/user scope, with `new_for_test(tempdir)` using an isolated file or in-memory database.
- lazy-open one `ProvenanceDatabase` per process/repo scope and expose `query_store()`, `journal_store()`, and `store_trace_recorder()`.
- construct `MutationSupervisor::new(StoreTraceRecorder::new(...))` for app-server sessions and thread it into `ToolInvocation`.
- surface provider initialization failure as an explicit unavailable provenance status for reads and as recorder degraded/unsupervised observations for mutating paths; do not silently disable capture after enablement.

In `codex-rs/app-server/src/fs_api.rs`, wrap `Environment::default().get_filesystem()` with `codex_core::SupervisedExecutorFileSystem` only when provenance is enabled. Read-only methods must not open mutation intervals. If constructor dependencies make direct wrapping awkward, add a public `codex_core::build_supervised_executor_file_system(...)` factory rather than reaching into private `core::provenance` modules.

Also modify `codex-rs/app-server/src/message_processor.rs`, where `FsApi::default()` is currently constructed and `Fs*` requests are handled before delegation to `CodexMessageProcessor`. Replace direct default construction with a provider-aware constructor, for example `FsApi::new(provenance_provider.supervisor_for_fs())`, so app-server filesystem mutators participate in the same capture path as core tools. Keep `codex-rs/app-server/src/codex_message_processor.rs` routing for provenance API requests and tool-session provenance handles.

- [ ] **Step 5: Wire command, MCP, dynamic, and agent-job handlers**

Update the changed handlers so:

- `apply_patch` records `ExactMutator`.
- shell records `OpaqueMutator` unless intercepted as `apply_patch`.
- unified exec `exec_command` opens long-running activity for streamable sessions.
- `write_stdin` links to the open process activity and creates a child mutation interval only when `StructuredWriteTap` or `RuntimeWriteFence` attribution is available; otherwise it records ambiguous process-level drift evidence.
- `js_repl` records `OpaqueMutator`.
- `mcp.rs` classifies write-capable MCP calls through metadata surfaced by `codex-rs/codex-mcp/src/mcp_connection_manager.rs`; unknown write capability records an explicit unsupervised-Codex observation instead of silently passing.
- `dynamic.rs` routes mutating dynamic tools through the supervisor, defaulting to mutating unless the dynamic tool spec explicitly declares read-only behavior.
- `agent_jobs.rs` records job-level activity and per-worker child activity; if a worker can mutate a workspace but cannot provide exact custody, emit unsupervised-Codex observations with job/item/thread/session/turn anchors.
- `tools/registry.rs` must keep `is_mutating` classification consistent with provenance classification so approval/sandbox behavior and provenance capture do not diverge.

Do not add generic MCP plumbing through multiple unrelated call layers. Keep capability extraction close to `codex-rs/codex-mcp/src/mcp_connection_manager.rs`, and keep provenance activity/interval emission in the core tool handlers.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cd codex-rs && cargo test -p codex-core provenance
cd codex-rs && cargo test -p codex-core tools::handlers::apply_patch_tests
cd codex-rs && cargo test -p codex-core tools::handlers::shell_tests
cd codex-rs && cargo test -p codex-core tools::handlers::unified_exec_tests
cd codex-rs && cargo test -p codex-core tools::handlers::js_repl_tests
cd codex-rs && cargo test -p codex-provenance-store recorder
cd codex-rs && cargo test -p codex-core mcp_tool_call_tests
cd codex-rs && cargo test -p codex-core tools::handlers::agent_jobs_tests
```

Expected: PASS.

- [ ] **Step 7: Format, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-core && just fix -p codex-provenance-store && just fix -p codex-mcp
git add codex-rs/core codex-rs/app-server codex-rs/provenance-store codex-rs/codex-mcp
git commit -m "Supervise mutating execution ingress"
```

Expected: one commit with capture ingress coverage and no projection logic.

## Task 9: Implement Range, Activity, and Mutation Read APIs

**Files:**
- Modify: `codex-rs/provenance-store/src/query.rs`
- Modify: `codex-rs/app-server/src/provenance_api.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Test: `codex-rs/app-server/src/provenance_api.rs`
- Test: `codex-rs/provenance-store/src/query.rs`

- [ ] **Step 1: Write failing app-server API tests**

Add tests for:

- `provenanceTurn/read`
- `provenanceActivity/read`
- `provenanceMutationInterval/read`
- `provenanceWorkspaceTransition/read`
- `provenanceMutationObservation/read`
- `provenanceHunk/read`
- `provenanceRange/read`

Use seeded store rows and assert full response objects with `pretty_assertions::assert_eq`.

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cd codex-rs && cargo test -p codex-app-server provenance_api
```

Expected: FAIL for unimplemented read surfaces.

- [ ] **Step 3: Implement query store methods**

Implement all read methods in `codex-rs/provenance-store/src/query.rs`. Rules:

- Selector ambiguity is returned in `QueryResolutionStatus`, not as JSON-RPC transport error.
- Live selector resolution binds one concrete workspace state before reading.
- Stale live revalidation returns `freshness = Stale` and evidence for the bound state.
- `RecordedState` and exact Git selectors use `freshness = NotApplicable`.
- Missing projections return `indexing_status = Pending`, not invented lineage.

- [ ] **Step 4: Implement app-server handler methods**

Add one method per RPC in `codex-rs/app-server/src/provenance_api.rs`. Keep request validation close to DTO semantics, and keep store/query semantics in `codex-provenance-store`.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store query
cd codex-rs && cargo test -p codex-app-server provenance_api
```

Expected: PASS.

- [ ] **Step 6: Format, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-provenance-store && just fix -p codex-app-server
git add codex-rs/provenance-store codex-rs/app-server
git commit -m "Implement provenance read APIs"
```

Expected: one commit with query/read behavior.

## Task 10: Implement Export and Blob Access Audit APIs

**Files:**
- Modify: `codex-rs/provenance-store/src/export.rs`
- Modify: `codex-rs/provenance-store/src/blob.rs`
- Modify: `codex-rs/app-server/src/provenance_api.rs`
- Test: `codex-rs/provenance-store/src/export.rs`
- Test: `codex-rs/provenance-store/src/blob.rs`
- Test: `codex-rs/app-server/src/provenance_api.rs`

- [ ] **Step 1: Write failing export tests**

Add tests that seed journal rows and assert:

- `provenanceEvent/exportRawReplay` includes all visible durable rows in stream order.
- `provenanceEvent/exportCommitted` excludes aborted/repaired batch participants.
- Cursor resume rejects changed filters or mismatched snapshot head assertions.
- `schema_bundle_ref` is present when rows are returned.
- committed export withholds head-blocked tentative participants and exposes oldest head-blocking batch age.
- configured finalize/repair SLA thresholds are surfaced alongside head-blocking status so operators can distinguish healthy waiting from overdue repair.

- [ ] **Step 2: Write failing blob audit tests**

Add tests that seed a blob descriptor and assert:

- `provenanceBlobManifest/read` returns immutable descriptor event refs and availability refs.
- `provenanceBlob/read` fails closed when `AccessAudit` append fails.
- `BlobSelectorFailureResult` is returned when selector status is not matched.
- `InlineBlobChunk` includes authorization event ref before content disclosure.
- initial reads may set `expected_descriptor_event_ref`; if it does not match the selected descriptor event, return `ConstraintMismatch` with no content.
- resumed reads with `continuation_token` must reject any non-null `expected_descriptor_event_ref` as `BlobRequestInvalidResult`.
- continuation tokens reject changed token-bound selector, descriptor, range, requested offset, or session identity. If the implementation chooses to encode chunk size into the opaque token, changed chunk size must also reject; otherwise chunk size may vary within the authorized session scope.
- EOF delivery creates the final cumulative delivery checkpoint before the API returns the last chunk.

- [ ] **Step 3: Run tests to verify failure**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store export
cd codex-rs && cargo test -p codex-provenance-store blob
```

Expected: FAIL for unimplemented export/blob behavior.

- [ ] **Step 4: Implement export repository**

Implement:

- fixed snapshot watermark selection
- stable stream ordering
- raw replay rows
- committed export rows
- export continuity status
- schema bundle binding
- head-blocking telemetry: oldest blocking replayable batch age, blocking batch refs, and configured finalize/repair SLA thresholds
- missing blob ref reporting

- [ ] **Step 5: Implement blob repository and audit writes**

Implement:

- blob manifest pagination
- descriptor and availability resolution
- logical blob-read sessions
- initial-only `expected_descriptor_event_ref` handling
- fail-closed `AccessAudit` authorization writes
- cumulative delivery checkpoints
- inline chunk continuation tokens

Do not disclose bytes before the audit authorization event is durable.

- [ ] **Step 6: Wire app-server methods**

Wire:

- `provenanceEvent/exportRawReplay`
- `provenanceEvent/exportCommitted`
- `provenanceBlobManifest/read`
- `provenanceBlob/read`

- [ ] **Step 7: Run focused tests**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance-store export
cd codex-rs && cargo test -p codex-provenance-store blob
cd codex-rs && cargo test -p codex-app-server provenance_api
```

Expected: PASS.

- [ ] **Step 8: Format, lint, and commit**

Run:

```bash
cd codex-rs && just fmt && just fix -p codex-provenance-store && just fix -p codex-app-server
git add codex-rs/provenance-store codex-rs/app-server
git commit -m "Implement provenance export and blob audit APIs"
```

Expected: one commit with export and blob read behavior.

## Task 11: End-to-End Capture and Replay Hardening

**Files:**
- Modify: `codex-rs/core/src/provenance/*`
- Modify: `codex-rs/provenance-store/src/*`
- Modify: `codex-rs/app-server/src/provenance_api.rs`
- Create: `codex-rs/core/tests/suite/provenance.rs`
- Modify: `codex-rs/core/tests/suite/mod.rs`
- Test: `codex-rs/provenance-store/src/tests.rs`
- Test: `codex-rs/app-server/src/provenance_api.rs`
- Docs: `docs/superpowers/specs/provenance/08-operability.md` only if test strategy changes.

- [ ] **Step 1: Write failing end-to-end test**

Create a test that:

1. Starts a temporary Git-backed workspace.
2. Records an exact baseline.
3. Applies a supervised patch.
4. Appends execution and code custody events.
5. Builds projection rows.
6. Queries a recorded line range.
7. Exports raw replay rows.
8. Verifies event refs connect the range back to the activity and mutation interval.

Use structured helpers instead of string-searching JSON.

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd codex-rs && cargo test -p codex-core provenance_end_to_end
```

Expected: FAIL until the pipeline is fully wired.

- [ ] **Step 3: Implement repair and drift cases**

Add coverage for:

- external edit before interval starts
- external edit during active interval
- branch switch while indexing pending
- hash-chain conflict causing epoch rotation
- failed projection job and dependent repair

Return explicit `reason_codes`; do not hide uncertainty.

- [ ] **Step 4: Run required focused tests**

Run:

```bash
cd codex-rs && cargo test -p codex-provenance
cd codex-rs && cargo test -p codex-provenance-store
cd codex-rs && cargo test -p codex-app-server-protocol
cd codex-rs && cargo test -p codex-app-server provenance_api
cd codex-rs && cargo test -p codex-core provenance
```

Expected: PASS.

- [ ] **Step 5: Regenerate schema fixtures if any DTO changed**

Run only if protocol DTOs changed in this task:

```bash
just write-app-server-schema --experimental
```

Expected: schema fixture updates are intentional and reviewed.

- [ ] **Step 6: Format, lint, and ask before full workspace tests**

Run:

```bash
cd codex-rs && just fmt
cd codex-rs && just fix -p codex-provenance
cd codex-rs && just fix -p codex-provenance-store
cd codex-rs && just fix -p codex-app-server
cd codex-rs && just fix -p codex-core
```

Ask the user before running:

```bash
cd codex-rs && cargo test
```

Expected: user explicitly approves before full workspace test.

- [ ] **Step 7: Commit**

Run:

```bash
git add codex-rs docs/superpowers/specs/provenance
git commit -m "Harden provenance capture and replay"
```

Expected: one commit with end-to-end hardening and any necessary spec clarifications.

## Parallelization Plan

After Task 1:

- Task 2 can be done by one worker.
- Task 3 can be done by another worker.

After Tasks 2 and 3:

- Task 4 can proceed independently of capture.
- Task 5 can proceed independently of app-server routing.

After Tasks 3 and 5:

- Task 6 can proceed.

After Tasks 4, 6, and 7:

- Task 9 can proceed.
- Task 10 can proceed mostly in parallel with Task 9 if both workers avoid editing the same sections of `provenance_api.rs`; split ownership by method groups.

Task 11 should not start until all prior tasks have landed.

## Definition of Done

- `codex-provenance` owns all model contracts and canonical serialization tests.
- `codex-provenance-store` can append, verify, replay, and query hash-chained scoped ledger events.
- App-server v2 exposes provenance read/export/blob APIs separately from `thread/read`.
- First-party mutating ingress routes through `MutationSupervisor` or records an explicit unsupervised/external observation.
- Exact Git-backed recorded states can answer line-range provenance queries.
- Export raw replay can reconstruct recorder facts from `TraceKernelEvent` rows.
- Blob reads are explicit, audited, and fail closed when audit append fails.
- All changed app-server schema fixtures are regenerated.
- Focused tests for changed crates pass.
- Full workspace tests are either run with user approval or explicitly deferred.

## Open Risks

- `codex-core` integration points may shift during implementation. Keep the supervisor adapter small and avoid moving existing tool logic unless required.
- Exact large bulk mutation close-path performance must be measured before expanding beyond skeleton support.
- Schema DTO volume is high. Prefer one `v2/provenance.rs` module plus re-exports over growing `v2.rs`.
- Blob transport v1 is inline chunk only. Do not introduce file-handle redemption or out-of-band streaming without a separate transport spec.
- Symbol/function-level lookup remains out of scope for mcodex; callers should use line ranges.
