# Codex Provenance Spec Set

This directory is the authoritative split version of the Codex provenance kernel
design. The previous monolithic document is preserved only as an archive:

- legacy entrypoint: [../2026-04-21-codex-provenance-kernel-design.md](../2026-04-21-codex-provenance-kernel-design.md)
- archived monolith: [archive/2026-04-21-codex-provenance-kernel-design.md](archive/2026-04-21-codex-provenance-kernel-design.md)

## Reading Order

The file split is editorial. The kernel still has the six contracts introduced
in [00-architecture.md](00-architecture.md); the original `blob and export`
contract is split across [06-export-ingest.md](06-export-ingest.md) and
[07-blob-access-audit.md](07-blob-access-audit.md) so export/replay and local
blob access can evolve independently.

1. [00-architecture.md](00-architecture.md)
2. [01-execution-workspace-capture.md](01-execution-workspace-capture.md)
3. [02-ledger-kernel.md](02-ledger-kernel.md)
4. [03-exact-artifact-store.md](03-exact-artifact-store.md)
5. [04-code-projection.md](04-code-projection.md)
6. [05-query-apis.md](05-query-apis.md)
7. [06-export-ingest.md](06-export-ingest.md)
8. [07-blob-access-audit.md](07-blob-access-audit.md)
9. [08-operability.md](08-operability.md)
10. [09-implementation-plan.md](09-implementation-plan.md)

## Ownership Map

| File | Owns |
| --- | --- |
| [00-architecture.md](00-architecture.md) | Scope, goals, non-goals, Forgeloop boundary, v1 shape, and architectural summary. |
| [01-execution-workspace-capture.md](01-execution-workspace-capture.md) | `MutationSupervisor`, tool classes, exact custody boundary, activity/interval rules, long-running processes, bulk mutation policy, and external drift observation. |
| [02-ledger-kernel.md](02-ledger-kernel.md) | Ledger streams, identity, ordering, hash chains, execution/code/blob/system fact payload families. |
| [03-exact-artifact-store.md](03-exact-artifact-store.md) | Workspace state records, exact artifact store, baselines, file identity records, and revision/file anchoring details. |
| [04-code-projection.md](04-code-projection.md) | Projection contract, hunk lineage, revision alias records, and projector status semantics. |
| [05-query-apis.md](05-query-apis.md) | Query contract, status model, app-server v2 query/read RPCs, shared DTOs, query inputs, and query responses. |
| [06-export-ingest.md](06-export-ingest.md) | Stream registration, stream handoff, canonical event envelope, event payload registry, export RPCs, and Forgeloop-facing export semantics. |
| [07-blob-access-audit.md](07-blob-access-audit.md) | Blob descriptor/read contract, manifest reads, access audit behavior, blob RPCs, and query-time blob access semantics. |
| [08-operability.md](08-operability.md) | Capture, journal/store, projector, query, drift, and repair test strategy. |
| [09-implementation-plan.md](09-implementation-plan.md) | Incremental delivery phases, key decisions, and final recommendation. |

## Shared Definition Map

| Concept | Normative location |
| --- | --- |
| `EventRef`, `LedgerScope`, ordering, hash-chain, append-batch semantics | [02-ledger-kernel.md](02-ledger-kernel.md) for ledger semantics; [05-query-apis.md](05-query-apis.md) for app-server DTO shape. |
| `TraceKernelEvent`, canonical payload registry, `TraceSchemaBundle`, `SchemaBundleRef` | [06-export-ingest.md](06-export-ingest.md). These are owned with export/replay because they are the canonical replay artifact shape. |
| `WorkspaceStreamDescriptor`, `StreamEpochDescriptor`, stream claims, registration, handoff | [06-export-ingest.md](06-export-ingest.md). Query APIs may return these descriptors but do not redefine them. |
| `BlobDescriptor`, `BlobManifestEntry`, `BlobReadResult`, blob read-session audit behavior | [07-blob-access-audit.md](07-blob-access-audit.md). |
| Workspace state, exact artifacts, baselines, file identity | [03-exact-artifact-store.md](03-exact-artifact-store.md). |
| Projection jobs, projected segments, hunk lineage, revision aliases | [04-code-projection.md](04-code-projection.md). |

## Change Rules

- Update the narrowest file that owns the concept being changed.
- Do not add new design content to the archived monolith.
- Do not redefine a DTO, event, status enum, or storage invariant in multiple
  files; link to the owning file instead.
- Keep API details in [05-query-apis.md](05-query-apis.md),
  [06-export-ingest.md](06-export-ingest.md), or
  [07-blob-access-audit.md](07-blob-access-audit.md) depending on the RPC surface.
- Keep implementation sequencing in [09-implementation-plan.md](09-implementation-plan.md),
  even when a phase references details from another file.
