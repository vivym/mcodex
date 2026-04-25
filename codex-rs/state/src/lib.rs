//! SQLite-backed state for rollout metadata.
//!
//! This crate is intentionally small and focused: it extracts rollout metadata
//! from JSONL rollouts and mirrors it into a local SQLite database. Backfill
//! orchestration and rollout scanning live in `codex-core`.

mod extract;
pub mod log_db;
mod migrations;
mod model;
mod paths;
mod runtime;

pub use model::LogEntry;
pub use model::LogQuery;
pub use model::LogRow;
pub use model::Phase2InputSelection;
pub use model::Phase2JobClaimOutcome;
/// Preferred entrypoint: owns configuration and metrics.
pub use runtime::StateRuntime;

/// Low-level storage engine: useful for focused tests.
///
/// Most consumers should prefer [`StateRuntime`].
pub use extract::apply_rollout_item;
pub use extract::rollout_item_affects_thread_metadata;
pub use model::AccountCompatMigrationState;
pub use model::AccountHealthEvent;
pub use model::AccountHealthState;
pub use model::AccountLeaseError;
pub use model::AccountLeaseRecord;
pub use model::AccountPoolAccountDiagnostic;
pub use model::AccountPoolAccountRecord;
pub use model::AccountPoolAccountsListQuery;
pub use model::AccountPoolAccountsPage;
pub use model::AccountPoolDiagnostic;
pub use model::AccountPoolDiagnosticsRecord;
pub use model::AccountPoolEventRecord;
pub use model::AccountPoolEventsCursor;
pub use model::AccountPoolEventsListQuery;
pub use model::AccountPoolEventsPage;
pub use model::AccountPoolHealthState;
pub use model::AccountPoolIssueRecord;
pub use model::AccountPoolLeaseRecord;
pub use model::AccountPoolLeaseSnapshotRecord;
pub use model::AccountPoolMembership;
pub use model::AccountPoolQuotaRecord;
pub use model::AccountPoolSelectionRecord;
pub use model::AccountPoolSnapshotRecord;
pub use model::AccountPoolSummaryRecord;
pub use model::AccountQuotaProbeBackoff;
pub use model::AccountQuotaProbeObservation;
pub use model::AccountQuotaProbeStillBlocked;
pub use model::AccountQuotaStateRecord;
pub use model::AccountRegistryEntryUpdate;
pub use model::AccountSource;
pub use model::AccountStartupAvailability;
pub use model::AccountStartupCandidatePool;
pub use model::AccountStartupEligibility;
pub use model::AccountStartupResolutionIssue;
pub use model::AccountStartupResolutionIssueKind;
pub use model::AccountStartupResolutionIssueSource;
pub use model::AccountStartupSelectionPreview;
pub use model::AccountStartupSelectionState;
pub use model::AccountStartupSelectionUpdate;
pub use model::AccountStartupStatus;
pub use model::AgentJob;
pub use model::AgentJobCreateParams;
pub use model::AgentJobItem;
pub use model::AgentJobItemCreateParams;
pub use model::AgentJobItemStatus;
pub use model::AgentJobProgress;
pub use model::AgentJobStatus;
pub use model::Anchor;
pub use model::BackfillState;
pub use model::BackfillStats;
pub use model::BackfillStatus;
pub use model::DirectionalThreadSpawnEdgeStatus;
pub use model::EffectivePoolResolutionSource;
pub use model::ExtractionOutcome;
pub use model::LeaseKey;
pub use model::LeaseRenewal;
pub use model::LegacyAccountImport;
pub use model::NewPendingAccountRegistration;
pub use model::PendingAccountRegistration;
pub use model::QuotaExhaustedWindows;
pub use model::QuotaProbeResult;
pub use model::RegisteredAccountMembership;
pub use model::RegisteredAccountRecord;
pub use model::RegisteredAccountUpsert;
pub use model::SortKey;
pub use model::Stage1JobClaim;
pub use model::Stage1JobClaimOutcome;
pub use model::Stage1Output;
pub use model::Stage1OutputRef;
pub use model::Stage1StartupClaimParams;
pub use model::ThreadConfigBaselineSnapshot;
pub use model::ThreadMetadata;
pub use model::ThreadMetadataBuilder;
pub use model::ThreadsPage;
pub use runtime::RemoteControlEnrollmentRecord;
pub use runtime::logs_db_filename;
pub use runtime::logs_db_path;
pub use runtime::state_db_filename;
pub use runtime::state_db_path;

/// Environment variable for overriding the SQLite state database home directory.
pub const SQLITE_HOME_ENV: &str = "CODEX_SQLITE_HOME";

pub const LOGS_DB_FILENAME: &str = "logs";
pub const LOGS_DB_VERSION: u32 = 2;
pub const STATE_DB_FILENAME: &str = "state";
pub const STATE_DB_VERSION: u32 = 7;

/// Errors encountered during DB operations. Tags: [stage]
pub const DB_ERROR_METRIC: &str = "codex.db.error";
/// Metrics on backfill process. Tags: [status]
pub const DB_METRIC_BACKFILL: &str = "codex.db.backfill";
/// Metrics on backfill duration. Tags: [status]
pub const DB_METRIC_BACKFILL_DURATION_MS: &str = "codex.db.backfill.duration_ms";
