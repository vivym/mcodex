# Runtime Lease Authority For Subagents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore runtime-scoped pooled lease ownership so parent sessions, `spawn_agent` children, review/guardian delegates, and background model work share one dynamic runtime lease authority with request-scoped admission and tree-scoped terminal-`401` cancellation.

**Architecture:** Introduce a runtime-shared `RuntimeLeaseHost` above per-session services, then move pooled lease lifetime and request admission into that host while keeping remote-context reset in a session-local `SessionLeaseView`. Extend the existing collaboration tree model for cancellation scope, and replace the quota-aware selection plan's original `codex-core` Task 4 with a host-backed integration slice after quota-aware selection Task 3 is complete.

**Tech Stack:** Rust, Tokio, `codex-core`, `codex-account-pool`, `codex-state`, app-server stdio runtime gates, `core_test_support::responses`, `pretty_assertions`, existing account-pool SQLite state.

---

## Coordination With Quota-Aware Selection Work

This plan assumes `account-pool-quota-aware-selection` continues through Task 3 in `.worktrees/account-pool-quota-aware-selection/docs/superpowers/plans/2026-04-18-account-pool-quota-aware-selection-implementation.md`, then pauses before its Task 4.

Do not execute the quota-aware plan's original Task 4 while this plan is in progress. That original Task 4 modifies `codex-rs/core/src/state/service.rs`, `codex-rs/core/tests/suite/account_pool.rs`, and `codex-rs/account-pool/src/backend.rs`, which are exactly the runtime control-plane integration points this plan changes. In this plan, Task 7 replaces that original Task 4 by wiring the quota-aware selector into the new `RuntimeLeaseHost` boundary.

Post Task 6 anti-regression rule: pooled production code must not reintroduce a session-local bridge path around `RuntimeLeaseHost`. Root pooled sessions build exactly one owned `RuntimeLeaseAuthority` and publish it with `RuntimeLeaseHost::install_authority(...)`; children and delegates use request-boundary admission from the inherited host. Do not add APIs named or shaped like `legacy_manager_bridge`, `attach_legacy_manager_bridge`, `has_legacy_manager_bridge`, `account_pool_manager_for_turn`, or `install_manager_owner`.

Safe parallel work while this plan is active:

- quota-aware selection Tasks 1-3 in `codex-rs/state` and `codex-rs/account-pool`
- this plan's runtime host, request admission, model-client, and collaboration-tree slices

Unsafe parallel work:

- independent edits to `codex-rs/core/src/state/service.rs`
- independent edits to `codex-rs/core/tests/suite/account_pool.rs`
- adding a second runtime failover path outside `RuntimeLeaseHost`

## File Structure

Create a focused `runtime_lease` module under `codex-core` instead of growing `codex.rs`, `client.rs`, or `state/service.rs` further.

- Create: `codex-rs/core/src/runtime_lease/mod.rs`
  - Public crate-internal exports and module docs for the runtime lease boundary.
- Create: `codex-rs/core/src/runtime_lease/host.rs`
  - `RuntimeLeaseHost`, host construction, non-pooled fallback representation, host ids, and app-server host-scope key types.
- Create: `codex-rs/core/src/runtime_lease/authority.rs`
  - `RuntimeLeaseAuthority`, generation state, heartbeat ownership, draining, and wrappers around current account-pool manager behavior.
- Create: `codex-rs/core/src/runtime_lease/admission.rs`
  - `LeaseRequestContext`, `LeaseSnapshot`, `LeaseAuthHandle`, `LeaseAdmissionGuard`, `RequestBoundaryKind`, `LeaseAdmissionError`, and admitted-request counters.
- Create: `codex-rs/core/src/runtime_lease/reporting.rs`
  - `LeaseRequestReporter` methods that mutate runtime lease state using the admitted snapshot after provider responses or failures.
- Create: `codex-rs/core/src/runtime_lease/session_view.rs`
  - `SessionLeaseView`, per-session last account tracking, remote-context reset decisions, and auth materialization for a request snapshot.
- Create: `codex-rs/core/src/runtime_lease/collaboration_tree.rs`
  - `CollaborationTreeId`, membership registration guards, delegated/background tree ids, and tree cancellation hooks backed by existing spawn tree state. Task 3 creates the id type and a no-op root context; Task 8 extends it into the full registry.
- Create: `codex-rs/core/src/runtime_lease/tests.rs`
  - Unit tests for admission, draining, stale-generation reports, session reset decisions, and membership lifetime.
- Modify: `codex-rs/core/src/lib.rs`
  - Add `mod runtime_lease;` and expose only crate-internal APIs unless an existing public test requires a narrow re-export.
- Modify: `codex-rs/core/src/state/service.rs`
  - Replace session-local pooled ownership with host-backed services. Keep durable selection helpers here temporarily only when they are implementation details of the host.
- Modify: `codex-rs/core/src/lease_auth.rs`
  - Keep compatibility for existing auth providers while adding a request-scoped auth provider path for `LeaseSnapshot`.
- Modify: `codex-rs/core/src/client.rs`
  - Replace session-construction-time pooled auth snapshots with per-provider-request admission.
- Modify: `codex-rs/core/src/codex.rs`
  - Thread the host through `CodexSpawnArgs`, session construction, turn lifecycle, compact paths, fault reporting, and heartbeat removal.
- Modify: `codex-rs/core/src/thread_manager.rs`
  - Pass the runtime host handle to `ThreadSpawn` children and prevent child-local manager creation.
- Modify: `codex-rs/core/src/codex_delegate.rs`
  - Pass the runtime host and invocation-scoped collaboration membership to review/guardian delegates instead of static inherited lease auth.
- Modify: `codex-rs/core/src/tasks/review.rs`
  - Register one-shot review membership and remove static lease inheritance from the primary path.
- Modify: `codex-rs/core/src/guardian/review_session.rs`
  - Rebind reusable guardian sessions per review invocation and unregister on completion/cancellation.
- Modify: `codex-rs/core/src/memories/phase2.rs`
  - Register memory-summary model calls into parent-bound or synthetic background trees.
- Modify: `codex-rs/core/src/compact.rs`
  - Ensure remote compaction acquires request admission at the provider request boundary.
- Modify: `codex-rs/core/src/compact_remote.rs`
  - Convert compact fault reports to explicit admission/generation context.
- Modify: `codex-rs/core/src/tasks/compact.rs`
  - Remove compact-task turn-scoped pooled manager setup so remote compact uses `RuntimeLeaseHost` instead of its own `prepare_turn()` / heartbeat path.
- Modify: `codex-rs/core/src/tools/handlers/agent_jobs.rs`
  - Register agent-job model work with parent-bound or synthetic background trees.
- Modify: `codex-rs/core/src/session_startup_prewarm.rs`
  - Treat prewarm websocket setup as a provider request boundary with handshake-scoped admission.
- Modify: `codex-rs/core/tests/suite/account_pool.rs`
  - Add runtime-shared lease, drain, terminal-`401`, quota selector integration, and stale-generation tests.
- Modify: `codex-rs/core/src/client_tests.rs`
  - Add `ModelClient` request-boundary and transport-cache admission tests.
- Modify: `codex-rs/core/src/agent/control_tests.rs`
  - Add tree-scoped cancellation coverage for spawned agents.
- Modify: `codex-rs/core/src/codex_delegate_tests.rs`
  - Add review/guardian delegated membership cleanup coverage.
- Modify: `codex-rs/core/src/codex_tests_guardian.rs`
  - Add reusable guardian per-invocation rebinding regression coverage.
- Modify: `codex-rs/core/src/memories/tests.rs`
  - Add background synthetic tree coverage for memory summary calls.
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
  - Enforce stdio app-server pooled-mode host scope across start/resume/fork/load/unload.
- Modify: `codex-rs/app-server/src/message_processor.rs`
  - Preserve `AppServerTransport` through request dispatch so WebSocket app-server can reject pooled host creation before `Codex::spawn`.
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
  - Read live account-lease snapshots from `RuntimeLeaseHost` after per-session pooled managers are removed.
- Modify: `codex-rs/app-server/tests/suite/v2/account_pool.rs`
  - Add app-server pooled host boundary tests.
- Modify after quota Task 3 lands: `codex-rs/account-pool/src/backend.rs`
  - Add any selector request/outcome types required by `RuntimeLeaseHost` if Task 3 did not already expose them.

## Task 0: Preflight And Branch Safety

**Files:**
- Read: `docs/superpowers/specs/2026-04-18-runtime-lease-authority-for-subagents-design.md`
- Read: `.worktrees/account-pool-quota-aware-selection/docs/superpowers/plans/2026-04-18-account-pool-quota-aware-selection-implementation.md`
- Read: `docs/superpowers/specs/2026-04-18-account-pool-quota-aware-selection-design.md`

- [ ] **Step 1: Confirm quota-aware Task 3 is the pause point**

Run:

```bash
git -C .worktrees/account-pool-quota-aware-selection status --short
rg -n "### Task 3|### Task 4|core/src/state/service.rs" .worktrees/account-pool-quota-aware-selection/docs/superpowers/plans/2026-04-18-account-pool-quota-aware-selection-implementation.md
```

Expected: Task 3 exists before Task 4, and Task 4 is the first quota plan task that enters `codex-rs/core/src/state/service.rs`.

- [ ] **Step 2: Confirm this worktree has no conflicting in-progress runtime lease edits**

Run:

```bash
git status --short
rg -n "RuntimeLeaseHost|LeaseAdmissionGuard|SessionLeaseView|CollaborationTreeRegistry" codex-rs/core/src codex-rs/app-server/src
```

Expected: only unrelated user changes may appear in `git status`; the `rg` command should not show existing implementations unless another worker already started this plan.

- [ ] **Step 3: Commit or explicitly leave unrelated docs untouched**

If `git status --short` shows unrelated files, do not stage them. Record the clean baseline for this plan in the task notes.

## Task 1: Add Runtime Lease Module Skeleton And Host Seam

**Files:**
- Create: `codex-rs/core/src/runtime_lease/mod.rs`
- Create: `codex-rs/core/src/runtime_lease/host.rs`
- Create: `codex-rs/core/src/runtime_lease/tests.rs`
- Modify: `codex-rs/core/src/lib.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Test: `codex-rs/core/src/runtime_lease/tests.rs`

- [ ] **Step 1: Write failing host-seam tests**

Create `codex-rs/core/src/runtime_lease/tests.rs` with focused unit tests that do not require real provider I/O:

```rust
use super::host::{RuntimeLeaseHost, RuntimeLeaseHostId, RuntimeLeaseHostMode};
use pretty_assertions::assert_eq;

#[test]
fn pooled_host_id_is_stable_for_one_runtime() {
    let host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::for_test("runtime-a"));

    assert_eq!(host.id(), RuntimeLeaseHostId::for_test("runtime-a"));
    assert_eq!(host.mode(), RuntimeLeaseHostMode::Pooled);
}

#[test]
fn non_pooled_host_never_reports_pooled_authority() {
    let host = RuntimeLeaseHost::non_pooled_for_test(RuntimeLeaseHostId::for_test("runtime-a"));

    assert_eq!(host.mode(), RuntimeLeaseHostMode::NonPooled);
    assert!(host.authority_for_test().is_none());
}
```

- [ ] **Step 2: Run the focused tests and verify they fail to compile**

Run:

```bash
cd codex-rs
cargo test -p codex-core runtime_lease -- --nocapture
```

Expected: FAIL because `runtime_lease` and `RuntimeLeaseHost` do not exist.

- [ ] **Step 3: Add the module skeleton**

Create `codex-rs/core/src/runtime_lease/mod.rs`:

```rust
//! Runtime-scoped account-pool lease ownership.
//!
//! Pooled lease choice is runtime-owned. Sessions consume request-scoped
//! admissions from this module and keep only session-local transport continuity.

mod host;

#[cfg(test)]
mod tests;

pub(crate) use host::RuntimeLeaseHost;
pub(crate) use host::RuntimeLeaseHostId;
pub(crate) use host::RuntimeLeaseHostMode;
```

Create the minimal `codex-rs/core/src/runtime_lease/host.rs`:

```rust
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeLeaseHostId(String);

impl RuntimeLeaseHostId {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Display for RuntimeLeaseHostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeLeaseHostMode {
    Pooled,
    NonPooled,
}

#[derive(Debug)]
pub(crate) struct RuntimeLeaseAuthorityMarker;

#[derive(Clone, Debug)]
pub(crate) struct RuntimeLeaseHost {
    id: RuntimeLeaseHostId,
    authority: Option<Arc<RuntimeLeaseAuthorityMarker>>,
}

impl RuntimeLeaseHost {
    pub(crate) fn pooled(id: RuntimeLeaseHostId) -> Self {
        Self {
            id,
            authority: Some(Arc::new(RuntimeLeaseAuthorityMarker)),
        }
    }

    pub(crate) fn non_pooled(id: RuntimeLeaseHostId) -> Self {
        Self {
            id,
            authority: None,
        }
    }

    pub(crate) fn id(&self) -> RuntimeLeaseHostId {
        self.id.clone()
    }

    pub(crate) fn mode(&self) -> RuntimeLeaseHostMode {
        if self.authority.is_some() {
            RuntimeLeaseHostMode::Pooled
        } else {
            RuntimeLeaseHostMode::NonPooled
        }
    }

    #[cfg(test)]
    pub(crate) fn pooled_for_test(id: RuntimeLeaseHostId) -> Self {
        Self::pooled(id)
    }

    #[cfg(test)]
    pub(crate) fn non_pooled_for_test(id: RuntimeLeaseHostId) -> Self {
        Self::non_pooled(id)
    }

    #[cfg(test)]
    pub(crate) fn authority_for_test(&self) -> Option<Arc<RuntimeLeaseAuthorityMarker>> {
        self.authority.clone()
    }
}
```

Modify `codex-rs/core/src/lib.rs`:

```rust
mod runtime_lease;
```

- [ ] **Step 4: Thread the host handle into spawn/session structs without changing behavior**

Modify `CodexSpawnArgs` in `codex-rs/core/src/codex.rs` to include:

```rust
pub(crate) runtime_lease_host: Option<crate::runtime_lease::RuntimeLeaseHost>,
```

Modify `SessionServices` in `codex-rs/core/src/state/service.rs` to include:

```rust
pub(crate) runtime_lease_host: Option<crate::runtime_lease::RuntimeLeaseHost>,
```

At all `Codex::spawn(CodexSpawnArgs { ... })` call sites, pass `runtime_lease_host: None` for this task. This is a seam-only commit; no child behavior should change yet.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core runtime_lease -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Format and commit**

Run:

```bash
cd codex-rs
just fmt
git add core/src/lib.rs core/src/runtime_lease core/src/codex.rs core/src/state/service.rs
git commit -m "feat(core): add runtime lease host seam"
```

## Task 2: Enforce Single Pooled Control Plane At Session Construction

**Files:**
- Modify: `codex-rs/core/src/runtime_lease/host.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/src/thread_manager.rs`
- Modify: `codex-rs/core/src/codex_delegate.rs`
- Test: `codex-rs/core/src/runtime_lease/tests.rs`
- Test: `codex-rs/core/src/codex_delegate_tests.rs`

- [ ] **Step 1: Write failing tests for the inherited-child no-dual-manager invariant**

Add tests that verify an inherited child session with a runtime host does not build a second session-local `AccountPoolManager` while the root session can still keep the temporary owned authority until Task 6:

```rust
#[tokio::test]
async fn child_session_with_inherited_runtime_host_skips_session_local_account_pool_manager() -> anyhow::Result<()> {
    let fixture = crate::test_support::account_pool::session_fixture_with_pool().await?;
    let host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::for_test("runtime-a"));

    let root_services = fixture.spawn_root_services_with_runtime_host(Some(host.clone())).await?;
    let child_services = fixture
        .spawn_child_services_with_inherited_runtime_host(Some(host))
        .await?;

    assert!(root_services.runtime_lease_host.is_some());
    assert!(root_services.account_pool_manager.is_some());
    assert!(child_services.runtime_lease_host.is_some());
    assert!(child_services.account_pool_manager.is_none());
    Ok(())
}
```

If the existing test support cannot build this fixture directly, create a small helper in `codex-rs/core/src/runtime_lease/tests.rs` that calls `SessionServices::build_account_pool_manager_for_runtime_host(...)` once that method exists.

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cd codex-rs
cargo test -p codex-core child_session_with_inherited_runtime_host_skips_session_local_account_pool_manager -- --nocapture
```

Expected: FAIL because sessions still decide account-pool ownership only from `inherited_lease_auth_session`.

- [ ] **Step 3: Replace the construction decision**

In `Codex::spawn`, derive a host before account-pool manager construction. The host may be `Pooled` or `NonPooled`; host presence alone must not mean provider requests require pooled admission. Only `host.pooled_authority()` / `host.is_pooled()` drives pooled request admission.

During Tasks 2-5, root pooled sessions keep the existing `account_pool_manager` as a temporary owned authority so startup selection, compact gating, live snapshots, and shutdown behavior keep working until Task 6 migrates ownership into `RuntimeLeaseAuthority`. Inherited child sessions sharing that host must not create their own manager.

```rust
let runtime_lease_host = runtime_lease_host.or_else(|| {
    Some(crate::runtime_lease::RuntimeLeaseHost::new_from_config(
        &config,
        account_pool_holder_instance_id.clone(),
    ))
});
let account_pool_manager = if runtime_lease_host
    .as_ref()
    .is_some_and(RuntimeLeaseHost::is_inherited_from_parent)
{
    None
} else if inherited_lease_auth_session.is_some() {
    None
} else {
    SessionServices::build_account_pool_manager(
        state_db_ctx.clone(),
        config.accounts.clone(),
        config.codex_home.clone().to_path_buf(),
        account_pool_holder_instance_id,
    )
    .await?
};
```

Do not keep this exact `new_from_config` shape if a clearer constructor emerges, but preserve the invariant: inherited child sessions must not create a second pooled manager. Root pooled sessions may temporarily keep exactly one owned authority until Task 6 replaces it with the host-owned authority.

For non-pooled hosts, `account_pool_manager` can remain available as the existing non-pooled compatibility path until Task 6 finishes separating pooled and non-pooled host behavior. Do not remove working root-session behavior before the host-backed control plane exists.

At the same time, register the root authority with the host so later tasks have a real production authority target:

```rust
if let (Some(host), Some(authority)) = (
    runtime_lease_host.as_ref(),
    authority_owned_runtime_lease.as_ref(),
) {
    host.install_authority(authority.clone())?;
}
```

- [ ] **Step 4: Pass the same host to child spawn paths**

Modify `thread_manager.rs` so `spawn_thread_with_source` passes the parent's host when spawning `ThreadSpawn` children. Modify `codex_delegate.rs` so review/guardian delegates receive the parent session's host.

The callsite should read like:

```rust
runtime_lease_host: parent_session.services.runtime_lease_host.clone(),
```

For root sessions, keep host creation in `Codex::spawn`.
For pooled review/guardian child paths, this step is also where static inherited lease auth stops being the primary path: once a pooled runtime host is passed down, do not keep passing pooled `inherited_lease_auth_session` alongside it.

- [ ] **Step 5: Keep static lease inheritance as compatibility only**

Leave `inherited_lease_auth_session` in `CodexSpawnArgs` for now, but add a debug assertion and comment:

```rust
debug_assert!(
    !(runtime_lease_host
        .as_ref()
        .is_some_and(RuntimeLeaseHost::is_inherited_from_parent)
        && inherited_lease_auth_session.is_some()),
    "inherited pooled runtime host must not be combined with static inherited lease auth"
);
```

- [ ] **Step 6: Run focused construction and delegate tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core runtime_lease -- --nocapture
cargo test -p codex-core codex_delegate -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
cd codex-rs
just fmt
git add core/src/runtime_lease/host.rs core/src/codex.rs core/src/state/service.rs core/src/thread_manager.rs core/src/codex_delegate.rs core/src/runtime_lease/tests.rs core/src/codex_delegate_tests.rs
git commit -m "feat(core): enforce runtime pooled lease ownership"
```

## Task 3: Implement Request Admission And Generation Draining

**Guard cleanup design:** `LeaseAdmissionGuard` must release admissions synchronously in `Drop`. Do not put the admitted-request set behind `tokio::sync::Mutex` if the guard's drop path needs to mutate it. Use a small synchronous interior state for admission accounting, for example `std::sync::Mutex<AdmissionTrackerState>` plus `tokio::sync::Notify`, and keep all database/lease-acquisition work outside the guard drop path. The drop path may remove an `admission_id` and call `Notify::notify_waiters()` synchronously; it must not spawn async cleanup.

**Temporary production manager-owner design:** Tasks 3-5 must be buildable before Task 6 moves full lease ownership into `RuntimeLeaseAuthority`. To do that, `RuntimeLeaseAuthority` needs an explicit temporary production mode that wraps the root session's existing `AccountPoolManager` and exposes the same `acquire_request_lease` / `report_*` interface. Use an internal mode like:

```rust
enum RuntimeLeaseAuthorityMode {
    ManagerOwner(Arc<tokio::sync::Mutex<AccountPoolManager>>),
    HostOwned(HostOwnedLeaseState),
}
```

Tasks 3-5 implement the interface against `ManagerOwner` for real pooled sessions while unit tests can still use the in-memory authority constructors. Task 6 keeps this as the runtime authority owner surface and removes all session-level bridge accessors.

**Files:**
- Create: `codex-rs/core/src/runtime_lease/admission.rs`
- Create: `codex-rs/core/src/runtime_lease/authority.rs`
- Create: `codex-rs/core/src/runtime_lease/collaboration_tree.rs`
- Modify: `codex-rs/core/src/runtime_lease/mod.rs`
- Modify: `codex-rs/core/src/runtime_lease/host.rs`
- Test: `codex-rs/core/src/runtime_lease/tests.rs`

- [ ] **Step 1: Write failing admission lifecycle tests**

Add tests:

```rust
use super::admission::{LeaseAdmissionError, LeaseRequestContext, RequestBoundaryKind};
use super::collaboration_tree::CollaborationTreeId;

#[tokio::test]
async fn admission_guard_releases_exactly_once() {
    let authority = RuntimeLeaseAuthority::for_test_accepting("acct-a", 11);
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
    );

    let admission = authority
        .acquire_request_lease_for_test(request_context)
        .await
        .unwrap();
    assert_eq!(authority.admitted_count_for_test(), 1);

    drop(admission.guard);
    assert_eq!(authority.admitted_count_for_test(), 0);
}

#[tokio::test]
async fn draining_acquire_waits_until_replacement_generation() {
    let authority = RuntimeLeaseAuthority::for_test_accepting("acct-a", 11);
    let request_context = LeaseRequestContext::for_test(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
    );
    let first = authority
        .acquire_request_lease_for_test(request_context.clone())
        .await
        .unwrap();

    authority.close_current_generation_for_test().await;
    let waiter = tokio::spawn({
        let authority = authority.clone();
        async move {
            authority
                .acquire_request_lease_for_test(request_context)
                .await
                .unwrap()
        }
    });

    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());

    drop(first.guard);
    authority.install_replacement_for_test("acct-b", 12).await;
    let second = waiter.await.unwrap();

    assert_eq!(second.snapshot.account_id(), "acct-b");
    assert_eq!(second.snapshot.generation(), 12);
}

#[tokio::test]
async fn cancelled_draining_acquire_returns_typed_cancellation() {
    let authority = RuntimeLeaseAuthority::for_test_draining("acct-a", 11);
    let token = tokio_util::sync::CancellationToken::new();
    let request_context = LeaseRequestContext::for_test_with_cancel(
        RequestBoundaryKind::ResponsesHttp,
        "session-a",
        CollaborationTreeId::for_test("tree-a"),
        token.clone(),
    );
    token.cancel();

    let err = authority
        .acquire_request_lease_for_test(request_context)
        .await
        .unwrap_err();

    assert_eq!(err, LeaseAdmissionError::Cancelled);
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cd codex-rs
cargo test -p codex-core runtime_lease -- --nocapture
```

Expected: FAIL because admission and authority types do not exist.

- [ ] **Step 3: Implement request-boundary and admission types**

Create `codex-rs/core/src/runtime_lease/admission.rs`:

```rust
use std::sync::Arc;

use codex_login::auth::LeaseScopedAuthSession;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::collaboration_tree::CollaborationTreeId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestBoundaryKind {
    ResponsesHttp,
    ResponsesWebSocket,
    ResponsesWebSocketPrewarm,
    ResponsesCompact,
    Realtime,
    MemorySummary,
    BackgroundModelCall,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LeaseAdmissionError {
    Cancelled,
    NoEligibleAccount,
    NonPooled,
    RuntimeShutdown,
    UnsupportedPooledPath,
}

#[derive(Clone, Debug)]
pub(crate) struct LeaseRequestContext {
    pub(crate) boundary: RequestBoundaryKind,
    pub(crate) session_id: String,
    pub(crate) collaboration_tree_id: CollaborationTreeId,
    pub(crate) cancel: CancellationToken,
}

impl LeaseRequestContext {
    pub(crate) fn new(
        boundary: RequestBoundaryKind,
        session_id: String,
        collaboration_tree_id: CollaborationTreeId,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            boundary,
            session_id,
            collaboration_tree_id,
            cancel,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        boundary: RequestBoundaryKind,
        session_id: &str,
        collaboration_tree_id: CollaborationTreeId,
    ) -> Self {
        Self::new(
            boundary,
            session_id.to_string(),
            collaboration_tree_id,
            CancellationToken::new(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test_with_cancel(
        boundary: RequestBoundaryKind,
        session_id: &str,
        collaboration_tree_id: CollaborationTreeId,
        cancel: CancellationToken,
    ) -> Self {
        Self::new(
            boundary,
            session_id.to_string(),
            collaboration_tree_id,
            cancel,
        )
    }
}

#[derive(Clone)]
pub(crate) struct LeaseAuthHandle {
    auth_session: Arc<dyn LeaseScopedAuthSession>,
}

impl std::fmt::Debug for LeaseAuthHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseAuthHandle").finish_non_exhaustive()
    }
}

impl LeaseAuthHandle {
    pub(crate) fn new(auth_session: Arc<dyn LeaseScopedAuthSession>) -> Self {
        Self { auth_session }
    }

    pub(crate) fn auth_session(&self) -> Arc<dyn LeaseScopedAuthSession> {
        Arc::clone(&self.auth_session)
    }

    pub(crate) fn auth_recovery(&self) -> crate::lease_auth::LeaseSessionAuthRecovery {
        crate::lease_auth::LeaseSessionAuthRecovery::new(self.auth_session())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LeaseSnapshot {
    pub(crate) admission_id: Uuid,
    pub(crate) pool_id: String,
    pub(crate) account_id: String,
    pub(crate) selection_family: String,
    pub(crate) generation: u64,
    pub(crate) boundary: RequestBoundaryKind,
    pub(crate) session_id: String,
    pub(crate) collaboration_tree_id: CollaborationTreeId,
    pub(crate) allow_context_reuse: bool,
    pub(crate) auth_handle: LeaseAuthHandle,
}

impl LeaseSnapshot {
    pub(crate) fn account_id(&self) -> &str {
        &self.account_id
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

pub(crate) struct LeaseAdmission {
    pub(crate) snapshot: LeaseSnapshot,
    pub(crate) guard: LeaseAdmissionGuard,
}

pub(crate) struct LeaseAdmissionGuard {
    admission_id: Uuid,
    release: Option<Arc<dyn Fn(Uuid) + Send + Sync>>,
}

impl LeaseAdmissionGuard {
    pub(crate) fn new(admission_id: Uuid, release: Arc<dyn Fn(Uuid) + Send + Sync>) -> Self {
        Self {
            admission_id,
            release: Some(release),
        }
    }
}

impl Drop for LeaseAdmissionGuard {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release(self.admission_id);
        }
    }
}

pub(crate) type AdmissionCancelToken = CancellationToken;
```

Create the initial `codex-rs/core/src/runtime_lease/collaboration_tree.rs` with the id type only; Task 8 extends this module into the full registry:

```rust
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CollaborationTreeId(String);

impl CollaborationTreeId {
    pub(crate) fn root_for_session(session_id: &str) -> Self {
        Self(format!("session:{session_id}"))
    }

    #[cfg(test)]
    pub(crate) fn for_test(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl fmt::Display for CollaborationTreeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
```

- [ ] **Step 4: Implement in-memory authority state**

Create `codex-rs/core/src/runtime_lease/authority.rs` with a testable in-memory generation state first. Split state into two parts:

- async authority state for lease acquisition/replacement work
- synchronous admission tracker state used by `LeaseAdmissionGuard::drop`

Use `tokio::sync::Notify` or `watch` so draining acquirers can wait cancellably after the synchronous tracker observes zero admissions.

Minimum implementation shape:

```rust
use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use super::admission::{
    AdmissionCancelToken, LeaseAdmission, LeaseAdmissionError, LeaseAdmissionGuard, LeaseSnapshot,
    RequestBoundaryKind,
};

#[derive(Clone)]
pub(crate) struct RuntimeLeaseAuthority {
    inner: Arc<AuthorityInner>,
}

struct AuthorityInner {
    state: Mutex<AuthorityState>,
    admissions: std::sync::Mutex<AdmissionTrackerState>,
    changed: Notify,
}

struct AuthorityState {
    mode: RuntimeLeaseAuthorityMode,
}

enum RuntimeLeaseAuthorityMode {
    ManagerOwner(Arc<tokio::sync::Mutex<AccountPoolManager>>),
    HostOwned(HostOwnedLeaseState),
}

struct GenerationState {
    pool_id: String,
    account_id: String,
    selection_family: String,
    generation: u64,
    auth_session: Arc<dyn LeaseScopedAuthSession>,
    allow_context_reuse: bool,
    accepting: bool,
}

struct AdmissionTrackerState {
    active_generation: Option<u64>,
    admissions: HashSet<Uuid>,
}

struct HostOwnedLeaseState {
    generation: Option<GenerationState>,
}
```

Expose production methods:

```rust
pub(crate) async fn acquire_request_lease(
    &self,
    context: LeaseRequestContext,
) -> Result<LeaseAdmission, LeaseAdmissionError>;

pub(crate) async fn close_current_generation(&self);

pub(crate) async fn invalidate_current_generation(&self);
```

Use test constructors only behind `#[cfg(test)]`.

- [ ] **Step 5: Export the new APIs**

Modify `runtime_lease/mod.rs`:

```rust
mod admission;
mod authority;
mod collaboration_tree;
mod host;

pub(crate) use admission::LeaseAdmission;
pub(crate) use admission::LeaseAdmissionError;
pub(crate) use admission::LeaseAdmissionGuard;
pub(crate) use admission::LeaseAuthHandle;
pub(crate) use admission::LeaseRequestContext;
pub(crate) use admission::LeaseSnapshot;
pub(crate) use admission::RequestBoundaryKind;
pub(crate) use authority::RuntimeLeaseAuthority;
pub(crate) use collaboration_tree::CollaborationTreeId;
```

- [ ] **Step 6: Run admission tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core runtime_lease -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

Run:

```bash
cd codex-rs
just fmt
git add core/src/runtime_lease
git commit -m "feat(core): add pooled lease request admission"
```

## Task 4: Add SessionLeaseView And Preserve Transport Reset Semantics

**Files:**
- Create: `codex-rs/core/src/runtime_lease/session_view.rs`
- Modify: `codex-rs/core/src/runtime_lease/mod.rs`
- Modify: `codex-rs/core/src/lease_auth.rs`
- Modify: `codex-rs/core/src/client.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Test: `codex-rs/core/src/runtime_lease/tests.rs`
- Test: `codex-rs/core/src/client_tests.rs`

- [ ] **Step 1: Write failing session-view reset tests**

Add tests:

```rust
use super::session_view::{SessionLeaseView, SessionLeaseViewDecision};

#[test]
fn session_view_resets_only_when_account_changes_and_pool_disallows_reuse() {
    let mut view = SessionLeaseView::for_test();
    let first = LeaseSnapshot::for_test("pool-main", "acct-a", "codex", 1, /*allow_context_reuse*/ false);
    let second = LeaseSnapshot::for_test("pool-main", "acct-a", "codex", 1, /*allow_context_reuse*/ false);
    let third = LeaseSnapshot::for_test("pool-main", "acct-b", "codex", 2, /*allow_context_reuse*/ false);

    assert_eq!(
        view.before_request_for_test(&first),
        SessionLeaseViewDecision::Continue
    );
    assert_eq!(
        view.before_request_for_test(&second),
        SessionLeaseViewDecision::Continue
    );
    assert_eq!(
        view.before_request_for_test(&third),
        SessionLeaseViewDecision::ResetRemoteContext
    );
}
```

In `client_tests.rs`, add a regression that reset still advances `window_generation`, clears cached websocket state, and mints a fresh remote session id:

```rust
#[tokio::test]
async fn lease_view_reset_uses_existing_model_client_reset_boundary() {
    let client = test_model_client_with_runtime_lease_view(/*allow_context_reuse*/ false);
    let before = client.remote_session_id();

    client.apply_test_lease_snapshot("acct-a", 1).await;
    client.apply_test_lease_snapshot("acct-b", 2).await;

    assert_ne!(client.remote_session_id(), before);
    assert_eq!(client.cached_websocket_session_for_test().connection, None);
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-core session_view -- --nocapture
cargo test -p codex-core lease_view_reset_uses_existing_model_client_reset_boundary -- --nocapture
```

Expected: FAIL because `SessionLeaseView` does not exist and `ModelClient` still snapshots pooled auth at session construction.

- [ ] **Step 3: Implement `SessionLeaseView`**

Create `codex-rs/core/src/runtime_lease/session_view.rs`:

```rust
use super::LeaseSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionLeaseViewDecision {
    Continue,
    ResetRemoteContext,
}

#[derive(Debug)]
pub(crate) struct SessionLeaseView {
    last_account_id: Option<String>,
}

impl SessionLeaseView {
    pub(crate) fn new() -> Self {
        Self {
            last_account_id: None,
        }
    }

    pub(crate) fn before_request(&mut self, snapshot: &LeaseSnapshot) -> SessionLeaseViewDecision {
        let reset = self
            .last_account_id
            .as_deref()
            .is_some_and(|previous| previous != snapshot.account_id())
            && !snapshot.allow_context_reuse;
        self.last_account_id = Some(snapshot.account_id().to_string());
        if reset {
            SessionLeaseViewDecision::ResetRemoteContext
        } else {
            SessionLeaseViewDecision::Continue
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::new()
    }

    #[cfg(test)]
    pub(crate) fn before_request_for_test(
        &mut self,
        snapshot: &LeaseSnapshot,
    ) -> SessionLeaseViewDecision {
        self.before_request(snapshot)
    }
}
```

- [ ] **Step 4: Attach the view to `ModelClientState`**

Modify `ModelClientState` in `client.rs` to hold:

```rust
runtime_lease_host: Option<RuntimeLeaseHost>,
session_lease_view: Option<Arc<tokio::sync::Mutex<SessionLeaseView>>>,
session_id: String,
collaboration_tree_binding: Arc<CollaborationTreeBindingHandle>,
```

Do not remove `lease_auth` in this task. Keep it as compatibility fallback until all provider request boundaries are converted.

Modify `ModelClient::new(...)` so production callers must pass these values. Then update `Codex::spawn` in `codex.rs` to pass:

```rust
runtime_lease_host.clone(),
Arc::new(tokio::sync::Mutex::new(SessionLeaseView::new())),
conversation_id.to_string(),
root_collaboration_tree_binding,
```

`root_collaboration_tree_binding` should be created in `Codex::spawn` from a new helper in `runtime_lease/collaboration_tree.rs`:

```rust
pub(crate) struct CollaborationTreeBindingHandle {
    tx: tokio::sync::watch::Sender<CollaborationTreeId>,
    rx: tokio::sync::watch::Receiver<CollaborationTreeId>,
}
```

It initially points at `CollaborationTreeId::root_for_session(&conversation_id.to_string())`; Task 8 uses the sender half to rebind delegate/background invocations dynamically.

Add this lightweight record type to `runtime_lease/host.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteContextResetRecord {
    pub(crate) session_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) request_id: String,
    pub(crate) lease_generation: u64,
    pub(crate) transport_reset_generation: u64,
}
```

`RuntimeLeaseHost::record_remote_context_reset(...)` stores only the latest record needed for live snapshot/read-notification output.

- [ ] **Step 5: Add reset helpers that consume a lease snapshot**

Add a `ModelClient` helper whose only responsibility is shared session-local reset and that returns whether active turn state must also be cleared:

```rust
async fn apply_lease_snapshot_before_request(
    &self,
    snapshot: &LeaseSnapshot,
    turn_id: Option<&str>,
    request_id: &str,
) -> Result<SessionLeaseViewDecision, CodexErr> {
    if let Some(view) = self.state.session_lease_view.as_ref() {
        let mut view = view.lock().await;
        let decision = view.before_request(snapshot);
        if decision == SessionLeaseViewDecision::ResetRemoteContext {
            self.reset_remote_session_identity();
            if let Some(host) = self.state.runtime_lease_host.as_ref() {
                host.record_remote_context_reset(RemoteContextResetRecord {
                    session_id: self.state.session_id.clone(),
                    turn_id: turn_id.map(ToString::to_string),
                    request_id: request_id.to_string(),
                    lease_generation: snapshot.generation(),
                    transport_reset_generation: self.current_window_generation(),
                });
            }
        }
        return Ok(decision);
    }
    Ok(SessionLeaseViewDecision::Continue)
}
```

Add a `ModelClientSession` helper for streaming/turn-scoped state:

```rust
async fn apply_lease_snapshot_before_request(
    &mut self,
    snapshot: &LeaseSnapshot,
    turn_id: Option<&str>,
    request_id: &str,
) -> Result<(), CodexErr> {
    let decision = self
        .client
        .apply_lease_snapshot_before_request(snapshot, turn_id, request_id)
        .await?;
    if decision == SessionLeaseViewDecision::ResetRemoteContext {
        self.reset_websocket_session();
        self.turn_state = Arc::new(OnceLock::new());
        self.account_id_override = None;
    }
    Ok(())
}
```

The `ModelClient` helper clears shared cached websocket state through the existing `reset_remote_session_identity()` path. The `ModelClientSession` helper additionally clears active turn-scoped websocket state, previous-response/incremental state held inside `websocket_session`, sticky turn state, and account overrides so cross-account requests cannot reuse an abandoned transport generation.

`RemoteContextResetRecord` is observability state only. It mirrors the existing `transport_reset_generation` / `last_remote_context_reset_turn_id` fields for `accountLease/read` and notifications, but the reset decision remains session-local in `SessionLeaseView`. For regular turn requests, pass the turn sub-id as both `turn_id: Some(...)` and `request_id`. For non-turn requests like prewarm or background utilities, pass `turn_id: None` and a synthetic request id; `accountLease/read` should only update `last_remote_context_reset_turn_id` when the latest record has `Some(turn_id)`.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core session_view -- --nocapture
cargo test -p codex-core client_tests -- --nocapture
```

Expected: PASS for the new reset tests and no regression in existing client tests.

- [ ] **Step 7: Commit**

Run:

```bash
cd codex-rs
just fmt
git add core/src/runtime_lease/session_view.rs core/src/runtime_lease/mod.rs core/src/lease_auth.rs core/src/client.rs core/src/codex.rs core/src/state/service.rs core/src/runtime_lease/tests.rs core/src/client_tests.rs
git commit -m "feat(core): add session lease view"
```

## Task 5: Route ModelClient Provider Boundaries Through Admission

**Files:**
- Modify: `codex-rs/core/src/runtime_lease/admission.rs`
- Create: `codex-rs/core/src/runtime_lease/reporting.rs`
- Modify: `codex-rs/core/src/runtime_lease/authority.rs`
- Modify: `codex-rs/core/src/client.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/session_startup_prewarm.rs`
- Modify: `codex-rs/core/src/compact.rs`
- Modify: `codex-rs/core/src/compact_remote.rs`
- Modify: `codex-rs/core/src/tasks/compact.rs`
- Modify: `codex-rs/core/src/memories/phase2.rs`
- Modify: `codex-rs/core/src/realtime_conversation.rs`
- Modify: `codex-rs/core/src/mcp_openai_file.rs`
- Test: `codex-rs/core/src/client_tests.rs`
- Test: `codex-rs/core/src/realtime_conversation_tests.rs`
- Test: `codex-rs/core/src/compact_tests.rs`
- Test: `codex-rs/core/src/memories/tests.rs`

- [ ] **Step 1: Write failing request-boundary tests**

Add coverage in `client_tests.rs`:

```rust
#[tokio::test]
async fn responses_http_stream_acquires_admission_per_provider_round_trip() {
    let harness = model_client_with_recording_runtime_host();

    let _ = harness.stream_one_http_response().await.unwrap();
    let _ = harness.stream_one_http_response().await.unwrap();

    assert_eq!(
        harness.recorded_boundaries(),
        vec![RequestBoundaryKind::ResponsesHttp, RequestBoundaryKind::ResponsesHttp]
    );
}

#[tokio::test]
async fn websocket_prewarm_releases_handshake_admission_when_idle_connection_is_cached() {
    let harness = model_client_with_recording_runtime_host();

    harness.prewarm_websocket().await.unwrap();

    assert_eq!(harness.active_admissions(), 0);
    assert_eq!(
        harness.recorded_boundaries(),
        vec![RequestBoundaryKind::ResponsesWebSocketPrewarm]
    );
}

#[tokio::test]
async fn cached_websocket_is_discarded_when_admitted_generation_changes() {
    let harness = model_client_with_recording_runtime_host();

    harness.prewarm_websocket_for_generation("acct-a", 1).await.unwrap();
    harness.rotate_to_generation("acct-b", 2).await;
    harness.stream_one_websocket_response().await.unwrap();

    assert!(harness.cached_websocket_was_discarded());
}

#[tokio::test]
async fn websocket_preconnect_releases_handshake_admission_when_idle_connection_is_cached() {
    let harness = model_client_with_recording_runtime_host();

    harness.preconnect_websocket().await.unwrap();

    assert_eq!(harness.active_admissions(), 0);
    assert_eq!(
        harness.recorded_boundaries(),
        vec![RequestBoundaryKind::ResponsesWebSocketPrewarm]
    );
}

#[tokio::test]
async fn streaming_admission_is_held_until_stream_completion_then_released_once() {
    let harness = model_client_with_recording_runtime_host();
    let stream = harness.start_streaming_response().await.unwrap();

    assert_eq!(harness.active_admissions(), 1);
    stream.collect_to_completion().await.unwrap();

    assert_eq!(harness.active_admissions(), 0);
    assert_eq!(harness.release_count_for_last_admission(), 1);
}

#[tokio::test]
async fn websocket_streaming_admission_releases_once_on_drop_cancel_or_transport_failure() {
    let harness = model_client_with_recording_runtime_host();
    let stream = harness.start_websocket_streaming_response().await.unwrap();

    assert_eq!(harness.active_admissions(), 1);
    stream.simulate_transport_failure_or_drop().await;

    assert_eq!(harness.active_admissions(), 0);
    assert_eq!(harness.release_count_for_last_admission(), 1);
}
```

Add focused tests for `compact_conversation_history`, `summarize_memories`, and realtime call creation to assert their exact `RequestBoundaryKind`.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-core responses_http_stream_acquires_admission_per_provider_round_trip -- --nocapture
cargo test -p codex-core websocket_prewarm_releases_handshake_admission_when_idle_connection_is_cached -- --nocapture
cargo test -p codex-core cached_websocket_is_discarded_when_admitted_generation_changes -- --nocapture
```

Expected: FAIL because `current_client_setup()` still materializes auth without request admission and cached websocket tags do not include lease generation.

- [ ] **Step 3: Introduce an admitted setup helper**

In `client.rs`, replace direct pooled auth materialization with a helper:

```rust
struct AdmittedClientSetup {
    setup: CurrentClientSetup,
    reporter: Option<LeaseRequestReporter>,
    auth_recovery: Option<Box<dyn AuthRecovery>>,
    guard: Option<LeaseAdmissionGuard>,
}

async fn admitted_client_setup(
    &self,
    boundary: RequestBoundaryKind,
    turn_id: Option<&str>,
    request_id: &str,
    cancellation_token: CancellationToken,
) -> Result<AdmittedClientSetup> {
    let Some(host) = self.state.runtime_lease_host.as_ref() else {
        return Ok(AdmittedClientSetup {
            setup: self.current_client_setup_legacy().await?,
            reporter: None,
            auth_recovery: self.current_auth_recovery_legacy(),
            guard: None,
        });
    };
    let Some(authority) = host.pooled_authority() else {
        return Ok(AdmittedClientSetup {
            setup: self.current_client_setup_legacy().await?,
            reporter: None,
            auth_recovery: self.current_auth_recovery_legacy(),
            guard: None,
        });
    };

    let request_context = self.build_lease_request_context(boundary, cancellation_token)?;
    let admission = authority
        .acquire_request_lease(request_context)
        .await
        .map_err(map_lease_admission_error)?;
    self.apply_lease_snapshot_before_request(&admission.snapshot, turn_id, request_id)
        .await?;
    let setup = self.current_client_setup_from_admission(&admission).await?;
    let auth_recovery: Box<dyn AuthRecovery> =
        Box::new(admission.snapshot.auth_handle.auth_recovery());
    let reporter = LeaseRequestReporter::new(authority, admission.snapshot.clone());
    Ok(AdmittedClientSetup {
        setup,
        reporter: Some(reporter),
        auth_recovery: Some(auth_recovery),
        guard: Some(admission.guard),
    })
}
```

Use explicit names like `current_client_setup_legacy` only while migrating. Do not leave direct pooled callers on the legacy path. A `NonPooled` host must take the legacy branch above; host presence alone is not enough to require admission.
Rename the existing shared-auth recovery helper to `current_auth_recovery_legacy()` and use it only for non-pooled or no-host branches.

Implement `current_client_setup_from_admission(&LeaseAdmission)` using the admission's `LeaseAuthHandle`, not `SessionLeaseAuth::current_session()`:

```rust
async fn current_client_setup_from_admission(
    &self,
    admission: &LeaseAdmission,
) -> Result<CurrentClientSetup> {
    let auth_session = admission.snapshot.auth_handle.auth_session();
    let auth = Some(
        auth_session
            .leased_turn_auth()
            .map_err(|err| CodexErr::Io(std::io::Error::other(err.to_string())))?
            .auth()
            .clone(),
    );
    let api_provider = self
        .state
        .provider
        .to_api_provider(auth.as_ref().map(CodexAuth::auth_mode))?;
    let api_auth = auth_provider_from_auth(auth.clone(), &self.state.provider)?;
    Ok(CurrentClientSetup {
        auth,
        api_provider,
        api_auth,
    })
}
```

Create `codex-rs/core/src/runtime_lease/reporting.rs`:

```rust
#[derive(Clone)]
pub(crate) struct LeaseRequestReporter {
    authority: RuntimeLeaseAuthority,
    snapshot: LeaseSnapshot,
}

impl LeaseRequestReporter {
    pub(crate) fn new(authority: RuntimeLeaseAuthority, snapshot: LeaseSnapshot) -> Self {
        Self { authority, snapshot }
    }

    pub(crate) fn snapshot(&self) -> &LeaseSnapshot {
        &self.snapshot
    }

    pub(crate) async fn report_rate_limits(&self, rate_limits: &RateLimitSnapshot) {
        let _ = self.authority.report_rate_limits(&self.snapshot, rate_limits).await;
    }

    pub(crate) async fn report_usage_limit_reached(&self) {
        let _ = self
            .authority
            .report_usage_limit_reached(&self.snapshot)
            .await;
    }

    pub(crate) async fn report_terminal_unauthorized(&self) {
        let _ = self
            .authority
            .report_terminal_unauthorized(&self.snapshot)
            .await;
    }
}
```

Export `LeaseRequestReporter` from `runtime_lease/mod.rs`. Keeping it out of `admission.rs` avoids an authority/admission module cycle.

Add compile-ready report method stubs to `RuntimeLeaseAuthority` in this task so `LeaseRequestReporter` compiles immediately:

```rust
pub(crate) async fn report_rate_limits(
    &self,
    snapshot: &LeaseSnapshot,
    rate_limits: &RateLimitSnapshot,
) -> anyhow::Result<()> {
    let _ = (snapshot, rate_limits);
    Ok(())
}

pub(crate) async fn report_usage_limit_reached(
    &self,
    snapshot: &LeaseSnapshot,
) -> anyhow::Result<()> {
    let _ = snapshot;
    Ok(())
}

pub(crate) async fn report_terminal_unauthorized(
    &self,
    snapshot: &LeaseSnapshot,
) -> anyhow::Result<()> {
    let _ = snapshot;
    Ok(())
}
```

Task 6 replaces these stubs with durable health/quota writes, draining, heartbeat, and stale-generation behavior. Do not leave reporter methods undefined until Task 6.

- [ ] **Step 4: Build request context with production session and tree identity**

Use the `ModelClientState` fields wired from `Codex::spawn` in Task 4:

```rust
session_id: String,
collaboration_tree_binding: Arc<CollaborationTreeBindingHandle>,
```

Then implement:

```rust
fn build_lease_request_context(
    &self,
    boundary: RequestBoundaryKind,
    cancellation_token: CancellationToken,
) -> Result<LeaseRequestContext> {
    Ok(LeaseRequestContext::new(
        boundary,
        self.state.session_id.clone(),
        self.state.collaboration_tree_binding.current(),
        cancellation_token,
    ))
}
```

Root sessions can initialize the binding to `CollaborationTreeId::root_for_session(&session_id)`. Task 8 uses `CollaborationTreeBindingHandle::set_current(...)` to switch the active tree for delegated and background memberships before admission.

Add one production-path integration test in `core/tests/suite/account_pool.rs` or the nearest existing pooled runtime harness:

```rust
#[tokio::test]
async fn normal_pooled_codex_session_routes_provider_request_through_runtime_host() -> anyhow::Result<()> {
    let harness = pooled_codex_session_with_recording_runtime_host().await?;

    harness.submit_one_user_turn("hello").await?;

    assert_eq!(harness.recorded_request_admissions(), 1);
    assert!(harness.runtime_host_has_owned_authority());
    Ok(())
}
```

This test must construct a normal `Codex` session rather than a standalone `ModelClient`, so it proves `Codex::spawn` and `ModelClient::new` are wired in production.

- [ ] **Step 5: Update all provider entry points**

Before changing callsites, audit for pooled-auth bypasses:

```bash
cd codex-rs
rg -n 'current_auth\\(\\)\\.await|current_session\\(' core/src
```

Expected hits must either be migrated in this task or explicitly documented as non-pooled-only paths. At minimum, migrate or justify:

- `core/src/client.rs`
- `core/src/realtime_conversation.rs`
- `core/src/mcp_openai_file.rs`
- `core/src/tasks/compact.rs`
- any other pooled request path found by the audit

Convert these call sites to pass a `RequestBoundaryKind`:

- `ModelClient::compact_conversation_history` -> `ResponsesCompact`
- `ModelClient::create_realtime_call_with_headers` -> `Realtime`
- `ModelClient::summarize_memories` -> `MemorySummary`
- `ModelClientSession::stream` HTTP path -> `ResponsesHttp`
- `ModelClientSession::prewarm_websocket` -> `ResponsesWebSocketPrewarm`
- `ModelClientSession::preconnect_websocket` -> `ResponsesWebSocketPrewarm`
- websocket reconnect or first streaming websocket use -> `ResponsesWebSocket`
- auth-recovery retry that performs another provider round-trip -> acquire a new admission for that retry
- `tasks/compact.rs` remote compact path -> stop calling `prepare_turn()`, `lease_auth.replace_current(...)`, or `start_account_pool_lease_heartbeat()`; it must rely on the same request-boundary admission path as the regular session client
- `realtime_conversation.rs` provider requests -> route through the same admission helper or explicitly prove the path is non-pooled-only
- `mcp_openai_file.rs` OpenAI file request path -> route through the same admission helper or explicitly prove the path is non-pooled-only

The admission guard for streaming response work must be owned by the returned stream or connection wrapper, not dropped before streaming finishes.

For `ModelClientSession` request paths, call the `ModelClientSession::apply_lease_snapshot_before_request(...)` helper from Task 4, not only the `ModelClient` helper, so active websocket/previous-response/turn-state caches are cleared on account reset.

Add a focused regression test for auth recovery:

```rust
#[tokio::test]
async fn auth_recovery_retry_reacquires_fresh_admission_and_reporter() {
    let harness = model_client_with_recording_runtime_host();

    harness.trigger_one_unauthorized_then_recovery_retry().await.unwrap();

    assert_eq!(harness.recorded_request_admissions().len(), 2);
    assert!(harness.second_admission_id_differs_from_first());
}
```

- [ ] **Step 6: Thread reporter ownership through streaming and errors**

Fault reporting happens inside `ModelClient` and stream wrappers while the snapshot is still available. Do not rely on `Codex` to infer which lease failed after it only sees a plain `CodexErr`.

Required ownership rules:

- unary calls own `LeaseRequestReporter` and `LeaseAdmissionGuard` until the provider future completes
- streaming calls move both into the returned `ResponseStream`-like object
- stream EOF, explicit cancel, drop, and transport failure drop the guard and release admission
- provider responses with rate-limit snapshots call `reporter.report_rate_limits(...)`
- `usage_limit_reached` provider errors call `reporter.report_usage_limit_reached()` before returning `CodexErr::UsageLimitReached`
- terminal unauthorized is reported only after `AdmittedClientSetup.auth_recovery` has run the existing leased-auth recovery or backend revalidation path and that recovery fails
- auth-recovery retry attempts acquire a fresh admission and therefore receive a fresh reporter and a fresh request-scoped recovery handle

Codex-level error handling may still update user-visible rate-limit state, but it must not be the authoritative pooled fault reporter.

- [ ] **Step 7: Tag cached websocket sessions**

Extend `WebsocketSession` in `client.rs`:

```rust
lease_account_id: Option<String>,
lease_generation: Option<u64>,
transport_reset_generation: u64,
```

Before reusing a cached websocket, require:

```rust
cached.lease_account_id.as_deref() == Some(snapshot.account_id())
    && cached.lease_generation == Some(snapshot.generation())
    && cached.transport_reset_generation == self.current_transport_reset_generation()
```

If tags do not match, discard the cached websocket session.

- [ ] **Step 8: Run request-boundary tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core client_tests -- --nocapture
cargo test -p codex-core compact_tests -- --nocapture
cargo test -p codex-core memories -- --nocapture
cargo test -p codex-core realtime_conversation -- --nocapture
```

Expected: PASS.

- [ ] **Step 9: Commit**

Run:

```bash
cd codex-rs
just fmt
git add core/src/runtime_lease/admission.rs core/src/runtime_lease/reporting.rs core/src/runtime_lease/authority.rs core/src/runtime_lease/mod.rs core/src/client.rs core/src/codex.rs core/src/session_startup_prewarm.rs core/src/compact.rs core/src/compact_remote.rs core/src/tasks/compact.rs core/src/memories/phase2.rs core/src/realtime_conversation.rs core/src/mcp_openai_file.rs core/src/client_tests.rs core/src/realtime_conversation_tests.rs core/src/compact_tests.rs core/src/memories/tests.rs core/tests/suite/account_pool.rs
git commit -m "feat(core): acquire pooled lease admission per provider request"
```

## Task 6: Move Lease Lifetime, Heartbeat, And Fault Reporting Into RuntimeLeaseAuthority

**Files:**
- Modify: `codex-rs/core/src/runtime_lease/authority.rs`
- Modify: `codex-rs/core/src/runtime_lease/host.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/compact_remote.rs`
- Modify: `codex-rs/app-server/src/account_lease_api.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_pool.rs`
- Test: `codex-rs/core/src/runtime_lease/tests.rs`
- Test: `codex-rs/core/tests/suite/account_pool.rs`

- [ ] **Step 1: Write failing runtime lease lifecycle tests**

In `core/tests/suite/account_pool.rs`, add integration tests:

```rust
#[tokio::test]
async fn parent_and_spawned_child_share_same_runtime_lease_generation() -> anyhow::Result<()> {
    let harness = pooled_runtime_with_two_available_accounts().await?;
    let parent = harness.start_parent_thread().await?;
    let child = parent.spawn_agent_child().await?;

    let parent_snapshot = parent.capture_next_request_lease().await?;
    let child_snapshot = child.capture_next_request_lease().await?;

    assert_eq!(parent_snapshot.account_id, child_snapshot.account_id);
    assert_eq!(parent_snapshot.generation, child_snapshot.generation);
    Ok(())
}

#[tokio::test]
async fn usage_limit_closes_generation_and_blocks_new_requests_until_drain() -> anyhow::Result<()> {
    let harness = pooled_runtime_with_two_available_accounts().await?;
    let first = harness.acquire_request("parent").await?;
    harness.report_usage_limit_reached(&first.snapshot).await?;
    let waiting_child = harness.spawn_waiting_request("child").await;
    assert!(waiting_child.is_waiting_for_replacement().await);

    drop(first.guard);
    let child_snapshot = waiting_child.await_snapshot().await?;
    assert_ne!(child_snapshot.generation, first.snapshot.generation);
    Ok(())
}

#[tokio::test]
async fn stale_generation_fault_does_not_poison_reacquired_same_account() -> anyhow::Result<()> {
    let harness = pooled_runtime_with_reacquirable_account().await?;
    let old = harness.acquire_request("parent").await?;
    harness.report_usage_limit_reached(&old.snapshot).await?;
    drop(old.guard);
    let new_snapshot = harness.acquire_after_replacement().await?;

    harness.report_terminal_unauthorized(&old.snapshot).await?;

    assert_eq!(harness.current_generation().await?, new_snapshot.generation);
    assert!(harness.current_generation_accepts_new_work().await?);
    Ok(())
}

#[tokio::test]
async fn heartbeat_renewal_failure_and_missing_lease_during_drain_release_runtime_cleanly() -> anyhow::Result<()> {
    let harness = pooled_runtime_with_faulty_renewal().await?;
    let first = harness.acquire_request("parent").await?;
    harness.force_renewal_missing_for_active_generation().await?;
    harness.close_current_generation_for_test().await?;
    drop(first.guard);

    harness.wait_for_runtime_release().await?;

    assert!(harness.no_active_generation().await?);
    Ok(())
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-core account_pool -- --nocapture
```

Expected: FAIL because lease lifetime still belongs to `AccountPoolManager::prepare_turn()` and fault reports lack admission context.

- [ ] **Step 3: Wrap current account-pool manager logic inside authority**

Move ownership of active lease, heartbeat interval, `pending_rotation`, proactive state, and health reporting behind `RuntimeLeaseAuthority`. Prefer moving code from `state/service.rs` into `runtime_lease/authority.rs` over adding more code to `service.rs`.

Keep these methods as the production interface:

```rust
pub(crate) async fn acquire_request_lease(
    &self,
    context: LeaseRequestContext,
) -> Result<LeaseAdmission, LeaseAdmissionError>;

pub(crate) async fn report_rate_limits(
    &self,
    snapshot: &LeaseSnapshot,
    rate_limits: &RateLimitSnapshot,
) -> anyhow::Result<()>;

pub(crate) async fn report_usage_limit_reached(
    &self,
    snapshot: &LeaseSnapshot,
) -> anyhow::Result<()>;

pub(crate) async fn report_terminal_unauthorized(
    &self,
    snapshot: &LeaseSnapshot,
) -> anyhow::Result<()>;

pub(crate) async fn release_for_shutdown(&self) -> anyhow::Result<()>;
```

Reports must ignore stale generations and must not mutate a replacement generation.

- [ ] **Step 4: Replace turn-level prepare with request-level admission**

In `codex.rs`, remove the pre-turn `prepare_turn()` call from `run_turn()` once `ModelClient` requests acquire admission directly. The turn may still perform a lightweight availability check for user-facing error messages, but it must not select a turn-scoped static lease auth session.

Remove `start_account_pool_lease_heartbeat()` or turn it into a compatibility shim that is no longer used in pooled runtime-host mode. Heartbeat belongs to the authority and continues while a draining generation has admitted work.

At the end of Task 6, remove all temporary root-session bridge accessors introduced in Task 2 for pooled runtimes. From this point onward, pooled root sessions and child sessions both use the host-owned authority; only non-pooled compatibility sessions may still carry `SessionServices.account_pool_manager`.

- [ ] **Step 5: Migrate live lease snapshot reads and notifications**

Current live account-lease reads depend on `SessionServices.account_pool_manager`. When pooled sessions stop creating a per-session manager, move that read model to `RuntimeLeaseAuthority` before removing manager-backed reads.

Implement:

```rust
impl RuntimeLeaseAuthority {
    pub(crate) async fn runtime_snapshot_seed(&self) -> AccountPoolManagerSnapshotSeedLike;
}
```

The concrete type can reuse or rename `AccountPoolManagerSnapshotSeed`, but it must preserve fields used by `Codex::account_lease_snapshot()` and app-server `accountLease/read`, including active lease, pool id, account id, lease epoch, switch/suppression reasons, proactive switch metadata, transport reset generation, and last remote-context reset turn id.
Populate `transport_reset_generation` and `last_remote_context_reset_turn_id` from the latest `RemoteContextResetRecord` recorded by `ModelClient` when `SessionLeaseView` requested a reset. Do not make the runtime authority decide resets; it only mirrors the session-local reset record for live observability.

Update:

- `Codex::account_lease_snapshot()` to prefer `services.runtime_lease_host.pooled_authority().runtime_snapshot_seed()` when present
- app-server `accountLease/read` in `codex-rs/app-server/src/account_lease_api.rs` to keep returning live runtime host state
- account-lease updated notifications to fire when authority generation state, draining state, switch reasons, or transport reset generation changes

Add a regression test that starts a pooled runtime host without a session-local `AccountPoolManager`, acquires a lease through request admission, and verifies `accountLease/read` reports the active account and lease epoch.

- [ ] **Step 5A: Add explicit shutdown release API and callsites**

Define the host/authority teardown path explicitly:

```rust
impl RuntimeLeaseHost {
    pub(crate) async fn release_for_shutdown(&self) -> anyhow::Result<()> {
        if let Some(authority) = self.pooled_authority() {
            authority.release_for_shutdown().await?;
        }
        Ok(())
    }
}
```

Wire it into:

- CLI/TUI session teardown where `SessionServices.account_pool_manager.release_for_shutdown()` is currently used
- app-server unload/close paths before a top-level pooled runtime host is discarded
- any existing `Codex` shutdown guard that currently assumes the session-local manager owns lease release

Add a regression test that a pooled runtime releases its active lease immediately on shutdown or app-server unload rather than waiting for TTL expiry.

- [ ] **Step 6: Convert fault reporting to explicit snapshots**

Replace calls like:

```rust
account_pool_manager.report_usage_limit_reached().await
account_pool_manager.report_unauthorized().await
```

with snapshot-based reports from the admitted request reporter introduced in Task 5. The owning `ModelClient` provider future or returned stream wrapper must report faults while it still owns the `LeaseRequestReporter`; `Codex` should no longer be responsible for deciding which pooled generation a plain `CodexErr` belongs to.

Use this ownership model:

- `ModelClient` reports `usage_limit_reached` before returning `CodexErr::UsageLimitReached`
- `ModelClient` reports terminal unauthorized only after its existing recovery/revalidation path fails
- streaming wrappers report errors from the stream polling path before surfacing the error to `Codex`
- `Codex` may still update visible rate-limit telemetry from the error payload, but it must not call pooled authority fault methods without a snapshot

Do not treat a raw wire-level 401 before recovery as terminal.

- [ ] **Step 7: Run lifecycle tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core runtime_lease -- --nocapture
cargo test -p codex-core account_pool -- --nocapture
cargo test -p codex-app-server account_pool -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
cd codex-rs
just fmt
git add core/src/runtime_lease core/src/state/service.rs core/src/codex.rs core/src/compact_remote.rs core/tests/suite/account_pool.rs app-server/src/account_lease_api.rs app-server/tests/suite/v2/account_pool.rs
git commit -m "feat(core): move pooled lease lifetime to runtime authority"
```

## Task 7: Rebase Quota-Aware Core Integration Onto RuntimeLeaseHost

**Prerequisite:** `account-pool-quota-aware-selection` Task 3 is merged or otherwise available in this worktree. Do not start this task before Task 3's selector/backend APIs are stable.

**Files:**
- Modify: `codex-rs/core/src/runtime_lease/authority.rs`
- Modify: `codex-rs/core/src/runtime_lease/host.rs`
- Modify: `codex-rs/core/src/state/service.rs`
- Modify: `codex-rs/account-pool/src/backend.rs`
- Modify: `codex-rs/core/tests/suite/account_pool.rs`

- [ ] **Step 1: Write failing quota-aware host integration tests**

Port the intent from the quota-aware selection plan's original Task 4 into host-backed tests:

```rust
#[tokio::test]
async fn hard_failover_uses_active_limit_family_through_runtime_authority() -> anyhow::Result<()> {
    let harness = runtime_authority_fixture_with_limit_family("chatgpt").await?;
    harness.seed_quota("acct-a", "chatgpt", exhausted_primary()).await;
    harness.seed_quota("acct-b", "chatgpt", healthy_primary(12.0)).await;

    let first = harness.acquire_request_on("acct-a").await?;
    harness.report_usage_limit_reached(&first.snapshot).await?;
    drop(first.guard);

    let next = harness.acquire_request("parent").await?;
    assert_eq!(next.snapshot.account_id(), "acct-b");
    Ok(())
}

#[tokio::test]
async fn successful_probe_retries_original_intent_after_runtime_drain() -> anyhow::Result<()> {
    let harness = runtime_authority_fixture_with_early_reset_probe().await?;

    let outcome = harness
        .trigger_soft_rotation_without_ordinary_candidates()
        .await?;

    assert_eq!(outcome.selection_reason, "probeRecoveredThenReselected");
    assert_eq!(outcome.account_id, "acct-recovered");
    Ok(())
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-core account_pool -- --nocapture
```

Expected: FAIL until `RuntimeLeaseAuthority` calls the quota-aware selector APIs from quota Task 3.

- [ ] **Step 3: Move selector intent resolution into authority**

Implement selection intent resolution in `RuntimeLeaseAuthority`, not in session-local service code:

- startup or first acquisition -> `Startup` / `codex`
- soft rotation -> active family when known, otherwise `codex`
- hard failover from `usage_limit_reached` -> active family when known, otherwise `codex`
- ambiguous usage limit without known family -> write `codex` row with unknown exhausted windows
- successful probe -> release verification lease, then retry original non-probe intent

Keep switch damping runtime-local inside authority. Do not reintroduce per-session damping state.

- [ ] **Step 4: Wire live quota observations from admitted requests**

When successful provider responses carry rate-limit snapshots, call:

```rust
runtime_lease_host
    .report_rate_limits(&lease_snapshot, &rate_limits)
    .await?;
```

This report must update `account_quota_state` using the active selection family from the admitted snapshot. It must also clear stale blocked quota rows when fresh non-exhausted observations arrive.
The admitted snapshot must already contain `selection_family`; do not recompute it from the current generation at report time because reports may arrive after rotation.

- [ ] **Step 5: Add structured events from the authority**

Append account-pool events from the runtime authority for:

- live quota observation
- exhausted-window transitions
- probe reservation outcomes
- probe recovery details
- hard failover and soft proactive rotation

Keep `details_json` populated according to the quota-aware selection spec.

- [ ] **Step 6: Run account-pool integration tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core account_pool -- --nocapture
cargo test -p codex-account-pool lease_lifecycle -- --nocapture
cargo test -p codex-state account_pool -- --nocapture
```

Expected: PASS for runtime authority integration and existing quota-aware backend/state tests.

- [ ] **Step 7: Commit**

Run:

```bash
cd codex-rs
just fmt
just fix -p codex-account-pool
git add core/src/runtime_lease/authority.rs core/src/runtime_lease/host.rs core/src/state/service.rs account-pool/src/backend.rs core/tests/suite/account_pool.rs
git commit -m "feat(core): integrate quota-aware selection with runtime leases"
```

## Task 8: Add CollaborationTreeRegistry And Delegate Membership

**Files:**
- Modify: `codex-rs/core/src/runtime_lease/collaboration_tree.rs`
- Modify: `codex-rs/core/src/runtime_lease/admission.rs`
- Modify: `codex-rs/core/src/runtime_lease/authority.rs`
- Modify: `codex-rs/core/src/runtime_lease/host.rs`
- Modify: `codex-rs/core/src/runtime_lease/mod.rs`
- Modify: `codex-rs/core/src/client.rs`
- Modify: `codex-rs/core/src/agent/control.rs`
- Modify: `codex-rs/core/src/thread_manager.rs`
- Modify: `codex-rs/core/src/codex_delegate.rs`
- Modify: `codex-rs/core/src/tasks/review.rs`
- Modify: `codex-rs/core/src/guardian/review_session.rs`
- Modify: `codex-rs/core/src/memories/phase2.rs`
- Modify: `codex-rs/core/src/compact.rs`
- Modify: `codex-rs/core/src/compact_remote.rs`
- Modify: `codex-rs/core/src/tools/handlers/agent_jobs.rs`
- Test: `codex-rs/core/src/agent/control_tests.rs`
- Test: `codex-rs/core/src/codex_delegate_tests.rs`
- Test: `codex-rs/core/src/codex_tests_guardian.rs`
- Test: `codex-rs/core/src/memories/tests.rs`

- [ ] **Step 1: Write failing tree-membership tests**

Add tests:

```rust
#[tokio::test]
async fn terminal_401_cancels_reporting_spawn_tree_but_not_unrelated_tree() -> anyhow::Result<()> {
    let harness = pooled_runtime_with_two_collaboration_trees().await?;
    let tree_a_child = harness.spawn_child_in_tree("tree-a").await?;
    let tree_b_child = harness.spawn_child_in_tree("tree-b").await?;

    tree_a_child.report_terminal_401().await?;

    assert!(tree_a_child.was_cancelled().await);
    assert!(!tree_b_child.was_cancelled().await);
    assert!(tree_b_child.is_drain_only_if_already_admitted().await);
    Ok(())
}

#[tokio::test]
async fn guardian_reusable_session_rebinds_membership_per_invocation() -> anyhow::Result<()> {
    let harness = guardian_review_harness().await?;

    let first = harness.run_review_invocation("parent-turn-1").await?;
    let second = harness.run_review_invocation("parent-turn-2").await?;

    assert_ne!(first.collaboration_tree_id, second.collaboration_tree_id);
    assert!(first.membership_released);
    assert!(second.membership_released);
    Ok(())
}

#[tokio::test]
async fn background_work_without_parent_uses_per_invocation_synthetic_tree() -> anyhow::Result<()> {
    let harness = pooled_runtime_background_harness().await?;

    let first = harness.run_memory_summary_without_parent().await?;
    let second = harness.run_memory_summary_without_parent().await?;

    assert_ne!(first.collaboration_tree_id, second.collaboration_tree_id);
    assert!(first.collaboration_tree_id.starts_with("background:"));
    Ok(())
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-core terminal_401_cancels_reporting_spawn_tree_but_not_unrelated_tree -- --nocapture
cargo test -p codex-core guardian_reusable_session_rebinds_membership_per_invocation -- --nocapture
cargo test -p codex-core background_work_without_parent_uses_per_invocation_synthetic_tree -- --nocapture
```

Expected: FAIL because delegated sessions and background work do not carry explicit collaboration-tree membership.

- [ ] **Step 3: Extend collaboration tree types into a registry**

Extend the `CollaborationTreeId` module created in Task 3:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) struct CollaborationTreeMembership {
    registry: Arc<CollaborationTreeRegistry>,
    tree_id: CollaborationTreeId,
    member_id: String,
}

#[derive(Default)]
pub(crate) struct CollaborationTreeRegistry {
    inner: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    members: HashMap<CollaborationTreeId, HashMap<String, CancellationToken>>,
}
```

Expose:

```rust
pub(crate) async fn register_member(
    &self,
    tree_id: CollaborationTreeId,
    member_id: String,
    cancellation_token: CancellationToken,
) -> CollaborationTreeMembership;

pub(crate) async fn cancel_tree(&self, tree_id: &CollaborationTreeId);

pub(crate) fn synthetic_background_tree_id(
    runtime_host_id: &RuntimeLeaseHostId,
    invocation_id: Uuid,
) -> CollaborationTreeId;
```

The membership guard unregisters on drop.
Because membership cleanup happens in `Drop`, the registry must use a synchronous short-held mutex for its membership map, just like the admission tracker. `cancel_tree(...)` can synchronously clone the target cancellation tokens under the lock, release the lock, and then call `CancellationToken::cancel()` without awaiting.

- [ ] **Step 4: Attach tree ids to lease admissions and reports**

`LeaseSnapshot` already carries tree and session identity from Task 3:

```rust
pub(crate) collaboration_tree_id: CollaborationTreeId,
pub(crate) session_id: String,
```

Keep those fields required in `LeaseRequestContext`; do not add an overload that acquires by boundary alone.

Own the registry in `RuntimeLeaseHost` and inject it into `RuntimeLeaseAuthority`:

```rust
pub(crate) struct RuntimeLeaseHost {
    id: RuntimeLeaseHostId,
    authority: Option<RuntimeLeaseAuthority>,
    collaboration_registry: Arc<CollaborationTreeRegistry>,
}
```

Update `RuntimeLeaseAuthority::report_terminal_unauthorized` so it invalidates the runtime generation and calls `CollaborationTreeRegistry::cancel_tree` for the reporting snapshot's tree id. This is the authoritative terminal-`401` path; do not leave cancellation as a caller-side best effort outside the authority.

- [ ] **Step 5: Register all child and background sources**

Wire these sources:

- `ThreadSpawn` -> existing spawn tree id from `agent/control.rs` or `thread_manager.rs`
- one-shot review -> parent turn tree id, invocation-scoped guard
- reusable guardian -> rebind per review invocation, unregister at invocation completion
- memory consolidation -> parent tree when invoked from a parent context, otherwise synthetic background tree
- compact -> parent tree
- agent-job -> parent tree if available, otherwise synthetic background tree

Do not infer tree identity only from `SubAgentSource`; every admission must carry the active id.

- [ ] **Step 6: Rebind the `ModelClient` tree context dynamically**

Update the `ModelClientState` tree id watch sender/receiver introduced in Task 5 so delegated and background invocations can temporarily bind the active tree:

```rust
pub(crate) struct CollaborationTreeBinding {
    _membership: CollaborationTreeMembership,
    previous_tree_id: CollaborationTreeId,
    handle: Arc<CollaborationTreeBindingHandle>,
}
```

When a review, guardian, memory, compact, or agent-job invocation starts, call `handle.set_current(new_tree_id.clone())` before any provider request boundary can occur. Restore `previous_tree_id` through the same handle and drop membership on completion, cancellation, or error.

This step is what makes `build_lease_request_context(...)` in `client.rs` produce the correct tree id without inferring it from `SubAgentSource`.

- [ ] **Step 7: Run tree tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core agent::control_tests -- --nocapture
cargo test -p codex-core codex_delegate -- --nocapture
cargo test -p codex-core guardian -- --nocapture
cargo test -p codex-core memories -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
cd codex-rs
just fmt
git add core/src/runtime_lease/collaboration_tree.rs core/src/runtime_lease/admission.rs core/src/runtime_lease/authority.rs core/src/runtime_lease/host.rs core/src/runtime_lease/mod.rs core/src/client.rs core/src/agent/control.rs core/src/thread_manager.rs core/src/codex_delegate.rs core/src/tasks/review.rs core/src/guardian/review_session.rs core/src/memories/phase2.rs core/src/compact.rs core/src/compact_remote.rs core/src/tools/handlers/agent_jobs.rs core/src/agent/control_tests.rs core/src/codex_delegate_tests.rs core/src/codex_tests_guardian.rs core/src/memories/tests.rs
git commit -m "feat(core): scope pooled lease faults to collaboration trees"
```

## Task 9: Enforce App-Server Pooled Host Scope

**Files:**
- Modify: `codex-rs/app-server/src/message_processor.rs`
- Modify: `codex-rs/app-server/src/codex_message_processor.rs`
- Modify: `codex-rs/app-server/tests/suite/v2/account_pool.rs`
- Modify: `codex-rs/app-server/README.md`

- [x] **Step 1: Write failing app-server boundary tests**

Add tests:

```rust
#[tokio::test]
async fn stdio_pooled_mode_rejects_second_loaded_top_level_thread() -> anyhow::Result<()> {
    let server = pooled_stdio_app_server().await?;
    let first = server.thread_start_with_account_pool().await?;

    let err = server.thread_start_with_account_pool().await.unwrap_err();

    assert_eq!(err.code(), "pooledRuntimeAlreadyLoaded");
    assert!(first.thread_id.is_some());
    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_releases_host_when_loaded_thread_unloads() -> anyhow::Result<()> {
    let server = pooled_stdio_app_server().await?;
    let first = server.thread_start_with_account_pool().await?;
    server.thread_unload(&first.thread_id).await?;

    let second = server.thread_start_with_account_pool().await?;

    assert_ne!(first.thread_id, second.thread_id);
    Ok(())
}

#[tokio::test]
async fn stdio_pooled_mode_blocks_resume_and_fork_that_would_create_second_top_level_context() -> anyhow::Result<()> {
    let server = pooled_stdio_app_server().await?;
    let loaded = server.thread_start_with_account_pool().await?;

    assert_eq!(
        server.thread_resume_other_top_level().await.unwrap_err().code(),
        "pooledRuntimeAlreadyLoaded"
    );
    assert_eq!(
        server.thread_fork_top_level(&loaded.thread_id).await.unwrap_err().code(),
        "pooledRuntimeAlreadyLoaded"
    );
    Ok(())
}

#[tokio::test]
async fn websocket_app_server_rejects_pooled_runtime_host_creation() -> anyhow::Result<()> {
    let server = websocket_app_server_with_account_pool_config().await?;

    let err = server.thread_start_with_account_pool().await.unwrap_err();

    assert_eq!(err.code(), "pooledRuntimeUnsupportedTransport");
    Ok(())
}
```

- [x] **Step 2: Run app-server tests and verify failure**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool -- --nocapture
```

Expected: FAIL because the app-server guard does not yet cover all start/resume/fork/load/unload host boundaries.

- [x] **Step 3: Implement top-level pooled host scope**

In `message_processor.rs`, preserve and pass `AppServerTransport` into `CodexMessageProcessor` for every `thread/start`, `thread/resume`, and `thread/fork` path that can create a runtime host. In `codex_message_processor.rs`, keep the runtime lease host scoped to the loaded top-level thread or pooled-selection context, not the process-global `ThreadManagerState`.

If the transport is `AppServerTransport::WebSocket { .. }`, pooled runtime host creation must fail clearly before `Codex::spawn`; this plan does not broaden pooled mode to multi-client WebSocket app-server.

Apply the gate to:

- `thread/start`
- `thread/resume`
- `thread/fork`
- thread load paths
- unload/close cleanup paths

Subagent `ThreadSpawn` inside the loaded top-level context is allowed and shares the same runtime host.

- [x] **Step 4: Update app-server README**

Document that pooled mode in stdio app-server supports only one loaded/running top-level thread context at a time; child subagents under that context share its runtime lease host.

- [x] **Step 5: Run app-server tests**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool -- --nocapture
```

Expected: PASS.

- [x] **Step 6: Commit**

Run:

```bash
cd codex-rs
just fmt
git add app-server/src/message_processor.rs app-server/src/codex_message_processor.rs app-server/tests/suite/v2/account_pool.rs app-server/README.md
git commit -m "feat(app-server): scope pooled lease host to top-level thread"
```

## Task 10: Remove Static Inherited Lease Auth From Primary Pooled Path

**Files:**
- Modify: `codex-rs/core/src/codex.rs`
- Modify: `codex-rs/core/src/codex_delegate.rs`
- Modify: `codex-rs/core/src/thread_manager.rs`
- Modify: `codex-rs/core/src/lease_auth.rs`
- Modify: `codex-rs/core/src/client.rs`
- Modify: `codex-rs/core/src/tasks/review.rs`
- Modify: `codex-rs/core/src/guardian/review_session.rs`
- Test: `codex-rs/core/src/client_tests.rs`
- Test: `codex-rs/core/src/codex_delegate_tests.rs`
- Test: `codex-rs/core/tests/suite/account_pool.rs`

- [x] **Step 1: Write failing no-static-inheritance regression tests**

Add tests:

```rust
#[tokio::test]
async fn thread_spawn_does_not_receive_static_inherited_lease_auth() -> anyhow::Result<()> {
    let harness = pooled_runtime_spawn_harness().await?;
    let child = harness.spawn_agent_child().await?;

    assert!(child.runtime_lease_host().is_some());
    assert!(child.static_inherited_lease_auth_for_test().is_none());
    Ok(())
}

#[tokio::test]
async fn child_session_follows_rotation_after_creation() -> anyhow::Result<()> {
    let harness = pooled_runtime_with_two_available_accounts().await?;
    let child = harness.spawn_agent_child().await?;
    let first = child.capture_next_request_lease().await?;

    harness.close_generation_and_rotate().await?;
    let second = child.capture_next_request_lease().await?;

    assert_ne!(first.generation, second.generation);
    assert_ne!(first.account_id, second.account_id);
    Ok(())
}
```

- [x] **Step 2: Run tests and verify failure if compatibility path still wins**

Run:

```bash
cd codex-rs
cargo test -p codex-core thread_spawn_does_not_receive_static_inherited_lease_auth -- --nocapture
cargo test -p codex-core child_session_follows_rotation_after_creation -- --nocapture
```

Expected: FAIL until all primary child paths use runtime host admission instead of static inherited lease auth.

- [x] **Step 3: Narrow `inherited_lease_auth_session`**

Remove `inherited_lease_auth_session` from normal `ThreadSpawn`, review, and guardian call paths. If it is still required for a non-pooled compatibility case, rename the field to make that narrowness explicit:

```rust
compat_inherited_lease_auth_session: Option<Arc<dyn LeaseScopedAuthSession>>,
```

Do not leave ambiguous primary-path callsites passing inherited lease auth.

- [x] **Step 4: Remove `ModelClientSession` creation-time lease snapshots**

Remove this pattern from `new_session()`:

```rust
lease_auth_session: self
    .state
    .lease_auth
    .as_ref()
    .and_then(|lease_auth| lease_auth.current_session()),
```

`ModelClientSession` must not cache pooled auth as a turn-scoped capability. It can keep non-pooled shared auth behavior and request-scoped `LeaseSnapshot` data only for in-flight provider work.

- [x] **Step 5: Run pooled child behavior tests**

Run:

```bash
cd codex-rs
cargo test -p codex-core account_pool -- --nocapture
cargo test -p codex-core codex_delegate -- --nocapture
cargo test -p codex-core client_tests -- --nocapture
```

Expected: PASS.

- [x] **Step 6: Commit**

Run:

```bash
cd codex-rs
just fmt
git add core/src/codex.rs core/src/codex_delegate.rs core/src/thread_manager.rs core/src/lease_auth.rs core/src/client.rs core/src/tasks/review.rs core/src/guardian/review_session.rs core/src/client_tests.rs core/src/codex_delegate_tests.rs core/tests/suite/account_pool.rs
git commit -m "refactor(core): retire static pooled lease inheritance"
```

## Task 11: End-To-End Verification And Lint

**Files:**
- Verify: all files touched by this plan
- Modify only if needed: docs affected by API/behavior changes

- [ ] **Step 1: Run focused core suites**

Run:

```bash
cd codex-rs
cargo test -p codex-core runtime_lease -- --nocapture
cargo test -p codex-core account_pool -- --nocapture
cargo test -p codex-core client_tests -- --nocapture
cargo test -p codex-core codex_delegate -- --nocapture
cargo test -p codex-core guardian -- --nocapture
cargo test -p codex-core memories -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Run app-server focused suite**

Run:

```bash
cd codex-rs
cargo test -p codex-app-server account_pool -- --nocapture
cargo test -p codex-app-server account_lease -- --nocapture
cargo test -p codex-app-server thread_archive -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run CLI focused suite**

Run:

```bash
cd codex-rs
cargo test -p codex-cli mcodex -- --nocapture
cargo test -p codex-cli --test accounts -- --nocapture
cargo test -p codex-cli --test accounts_observability -- --nocapture
```

Expected: PASS.

- [x] **Step 4: Run formatting**

Run:

```bash
cd codex-rs
just fmt
```

Expected: no remaining rustfmt changes, or only expected formatting changes from this plan.

Latest verification ledger, 2026-04-25:

- RED: `cargo test -p codex-core list_agent_subtree_thread_ids_uses_live_descendants_after_root_removed -- --nocapture` failed with `ThreadNotFound(root)` before the live-descendant fallback fix.
- GREEN: `cargo test -p codex-core list_agent_subtree_thread_ids_uses_live_descendants_after_root_removed -- --nocapture` passed after the fix.
- RED: `cargo test -p codex-core pooled_host_snapshot_ignores_remote_reset_from_previous_generation -- --nocapture` failed because generation 11 reset metadata was exposed on generation 12.
- GREEN: `cargo test -p codex-core pooled_host_snapshot_ignores_remote_reset_from_previous_generation -- --nocapture` passed after the generation match fix.
- PASS: `cargo test -p codex-core list_agent_subtree_thread_ids -- --nocapture` passed 2 targeted tests.
- PASS: `cargo test -p codex-core pooled_host_snapshot -- --nocapture` passed 3 targeted tests.
- PASS: `cargo test -p codex-app-server account_pool -- --nocapture` passed 19 targeted tests.
- PASS: `just fmt`.
- PASS: `just fix -p codex-core`; it still reports the existing `core/src/client.rs:1194` `expect_used` warning.
- Not run in this pass: the full Task 11 core matrix, app-server `account_lease` and `thread_archive`, CLI focused suites, and full workspace `cargo test`.

- [ ] **Step 5: Run scoped lints**

Run:

```bash
cd codex-rs
just fix -p codex-core
just fix -p codex-app-server
just fix -p codex-account-pool
just fix -p codex-cli
```

Expected: no unresolved clippy diagnostics.

- [ ] **Step 6: Ask before full workspace tests**

Because this plan changes `codex-core`, ask the user before running the complete suite:

```bash
cd codex-rs
cargo test
```

Expected if approved: PASS.

- [ ] **Step 7: Commit verification fixes**

If `just fix` or docs updates changed files:

```bash
git add codex-rs docs
git commit -m "test: cover runtime lease authority integration"
```

If no files changed, do not create an empty commit.

## Acceptance Criteria

- A pooled runtime has one `RuntimeLeaseHost` and no session with that host creates a separate `AccountPoolManager`.
- Every outbound provider request boundary in pooled mode acquires a fresh `LeaseSnapshot` and owns a `LeaseAdmissionGuard` until provider work finishes.
- `ModelClientSession` no longer snapshots pooled auth at construction time.
- `usage_limit_reached` closes the generation to future work, lets already admitted work drain, and only acquires a replacement after release.
- Terminal `401` is reported only after existing recovery/revalidation fails, invalidates the runtime generation, and cancels only the reporting collaboration tree.
- Late reports from old generations, including the same account reacquired under a new generation, cannot mutate the current generation.
- Session remote-context resets continue to use the existing transport reset generation, cached websocket clearing, previous-response-id clearing, and remote identity reset mechanics.
- `Codex::account_lease_snapshot()`, app-server `accountLease/read`, and account-lease update notifications continue to report live runtime host state after per-session pooled managers are removed.
- `ThreadSpawn`, review, guardian, memory summary, compact, and agent-job model work all use the same runtime authority path and carry explicit collaboration-tree ids.
- stdio app-server pooled mode remains limited to one loaded/running top-level thread context while allowing child subagents under that context.
- Quota-aware selector integration from the paused quota plan Task 4 is implemented through `RuntimeLeaseAuthority`, not through a parallel per-session runtime failover path.
