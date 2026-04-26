use crate::bottom_pane::FeedbackAudience;
#[cfg(test)]
use crate::legacy_core::append_message_history_entry;
use crate::legacy_core::config::Config;
use crate::legacy_core::message_history_metadata;
use crate::status::StatusAccountDisplay;
use crate::status::StatusAccountLeaseDisplay;
use crate::status::StatusAccountQuotaFamilyDisplay;
use crate::status::StatusAccountQuotaWindowDisplay;
use crate::status::format_reset_timestamp;
use crate::status::plan_type_display_name;
use chrono::Local;
use chrono::TimeZone;
use codex_app_server_client::AppServerClient;
use codex_app_server_client::AppServerEvent;
use codex_app_server_client::AppServerRequestHandle;
use codex_app_server_client::TypedRequestError;
use codex_app_server_protocol::Account;
use codex_app_server_protocol::AccountLeaseReadResponse;
use codex_app_server_protocol::AccountLeaseResumeResponse;
use codex_app_server_protocol::AccountPoolAccountResponse;
use codex_app_server_protocol::AccountPoolAccountsListParams;
use codex_app_server_protocol::AccountPoolAccountsListResponse;
use codex_app_server_protocol::AccountPoolQuotaFamilyResponse;
use codex_app_server_protocol::AccountPoolQuotaWindowResponse;
use codex_app_server_protocol::AuthMode;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ConfigBatchWriteParams;
use codex_app_server_protocol::ConfigEdit;
use codex_app_server_protocol::ConfigWriteResponse;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::GetAccountRateLimitsResponse;
use codex_app_server_protocol::GetAccountResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::MergeStrategy;
use codex_app_server_protocol::Model as ApiModel;
use codex_app_server_protocol::ModelListParams;
use codex_app_server_protocol::ModelListResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ReviewDelivery;
use codex_app_server_protocol::ReviewStartParams;
use codex_app_server_protocol::ReviewStartResponse;
use codex_app_server_protocol::SkillsListParams;
use codex_app_server_protocol::SkillsListResponse;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanParams;
use codex_app_server_protocol::ThreadBackgroundTerminalsCleanResponse;
use codex_app_server_protocol::ThreadCompactStartParams;
use codex_app_server_protocol::ThreadCompactStartResponse;
use codex_app_server_protocol::ThreadForkParams;
use codex_app_server_protocol::ThreadForkResponse;
use codex_app_server_protocol::ThreadListParams;
use codex_app_server_protocol::ThreadListResponse;
use codex_app_server_protocol::ThreadLoadedListParams;
use codex_app_server_protocol::ThreadLoadedListResponse;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::ThreadReadResponse;
use codex_app_server_protocol::ThreadRealtimeAppendAudioParams;
use codex_app_server_protocol::ThreadRealtimeAppendAudioResponse;
use codex_app_server_protocol::ThreadRealtimeAppendTextParams;
use codex_app_server_protocol::ThreadRealtimeAppendTextResponse;
use codex_app_server_protocol::ThreadRealtimeStartParams;
use codex_app_server_protocol::ThreadRealtimeStartResponse;
use codex_app_server_protocol::ThreadRealtimeStartTransport;
use codex_app_server_protocol::ThreadRealtimeStopParams;
use codex_app_server_protocol::ThreadRealtimeStopResponse;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadRollbackParams;
use codex_app_server_protocol::ThreadRollbackResponse;
use codex_app_server_protocol::ThreadSetNameParams;
use codex_app_server_protocol::ThreadSetNameResponse;
use codex_app_server_protocol::ThreadShellCommandParams;
use codex_app_server_protocol::ThreadShellCommandResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadStartSource;
use codex_app_server_protocol::ThreadUnsubscribeParams;
use codex_app_server_protocol::ThreadUnsubscribeResponse;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::TurnInterruptParams;
use codex_app_server_protocol::TurnInterruptResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnSteerParams;
use codex_app_server_protocol::TurnSteerResponse;
use codex_otel::TelemetryAuthMode;
use codex_protocol::ThreadId;
use codex_protocol::openai_models::ModelAvailabilityNux;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelUpgrade;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ConversationAudioParams;
use codex_protocol::protocol::ConversationStartParams;
use codex_protocol::protocol::ConversationStartTransport;
use codex_protocol::protocol::ConversationTextParams;
use codex_protocol::protocol::CreditsSnapshot;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::RateLimitWindow;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::protocol::ReviewTarget as CoreReviewTarget;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionNetworkProxyRuntime;
use color_eyre::eyre::ContextCompat;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

/// Data collected during the TUI bootstrap phase that the main event loop
/// needs to configure the UI, telemetry, and initial rate-limit prefetch.
///
/// Rate-limit snapshots are intentionally **not** included here; they are
/// fetched asynchronously after bootstrap returns so that the TUI can render
/// its first frame without waiting for the rate-limit round-trip.
pub(crate) struct AppServerBootstrap {
    pub(crate) account_email: Option<String>,
    pub(crate) auth_mode: Option<TelemetryAuthMode>,
    pub(crate) status_account_display: Option<StatusAccountDisplay>,
    pub(crate) account_lease_display: Option<StatusAccountLeaseDisplay>,
    pub(crate) plan_type: Option<codex_protocol::account::PlanType>,
    /// Whether the configured model provider needs OpenAI-style auth. Combined
    /// with `has_chatgpt_account` to decide if a startup rate-limit prefetch
    /// should be fired.
    pub(crate) requires_openai_auth: bool,
    pub(crate) default_model: String,
    pub(crate) feedback_audience: FeedbackAudience,
    pub(crate) has_chatgpt_account: bool,
    pub(crate) available_models: Vec<ModelPreset>,
}

pub(crate) struct AppServerSession {
    client: AppServerClient,
    next_request_id: AtomicI64,
    remote_cwd_override: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThreadSessionState {
    pub(crate) thread_id: ThreadId,
    pub(crate) forked_from_id: Option<ThreadId>,
    pub(crate) thread_name: Option<String>,
    pub(crate) model: String,
    pub(crate) model_provider_id: String,
    pub(crate) service_tier: Option<codex_protocol::config_types::ServiceTier>,
    pub(crate) approval_policy: AskForApproval,
    pub(crate) approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
    pub(crate) sandbox_policy: SandboxPolicy,
    pub(crate) cwd: PathBuf,
    pub(crate) instruction_source_paths: Vec<PathBuf>,
    pub(crate) reasoning_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    pub(crate) history_log_id: u64,
    pub(crate) history_entry_count: u64,
    pub(crate) network_proxy: Option<SessionNetworkProxyRuntime>,
    pub(crate) rollout_path: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum ThreadParamsMode {
    Embedded,
    Remote,
}

impl ThreadParamsMode {
    fn model_provider_from_config(self, config: &Config) -> Option<String> {
        match self {
            Self::Embedded => Some(config.model_provider_id.clone()),
            Self::Remote => None,
        }
    }
}

pub(crate) struct AppServerStartedThread {
    pub(crate) session: ThreadSessionState,
    pub(crate) turns: Vec<Turn>,
}

impl AppServerSession {
    pub(crate) fn new(client: AppServerClient) -> Self {
        Self {
            client,
            next_request_id: AtomicI64::new(1),
            remote_cwd_override: None,
        }
    }

    pub(crate) fn with_remote_cwd_override(mut self, remote_cwd_override: Option<PathBuf>) -> Self {
        self.remote_cwd_override = remote_cwd_override;
        self
    }

    pub(crate) fn remote_cwd_override(&self) -> Option<&std::path::Path> {
        self.remote_cwd_override.as_deref()
    }

    pub(crate) fn is_remote(&self) -> bool {
        matches!(self.client, AppServerClient::Remote(_))
    }

    pub(crate) async fn bootstrap(&mut self, config: &Config) -> Result<AppServerBootstrap> {
        let account = self.read_account().await?;
        let account_lease_display = match self.read_account_lease_display().await {
            Ok(display) => display,
            Err(err) => {
                tracing::debug!(error = %err, "accountLease/read unavailable during TUI bootstrap");
                None
            }
        };
        let model_request_id = self.next_request_id();
        let models: ModelListResponse = self
            .client
            .request_typed(ClientRequest::ModelList {
                request_id: model_request_id,
                params: ModelListParams {
                    cursor: None,
                    limit: None,
                    include_hidden: Some(true),
                },
            })
            .await
            .wrap_err("model/list failed during TUI bootstrap")?;
        let available_models = models
            .data
            .into_iter()
            .map(model_preset_from_api_model)
            .collect::<Vec<_>>();
        let default_model = config
            .model
            .clone()
            .or_else(|| {
                available_models
                    .iter()
                    .find(|model| model.is_default)
                    .map(|model| model.model.clone())
            })
            .or_else(|| available_models.first().map(|model| model.model.clone()))
            .wrap_err("model/list returned no models for TUI bootstrap")?;

        let (
            account_email,
            auth_mode,
            status_account_display,
            plan_type,
            feedback_audience,
            has_chatgpt_account,
        ) = match account.account {
            Some(Account::ApiKey {}) => (
                None,
                Some(TelemetryAuthMode::ApiKey),
                Some(StatusAccountDisplay::ApiKey),
                None,
                FeedbackAudience::External,
                false,
            ),
            Some(Account::Chatgpt { email, plan_type }) => {
                let feedback_audience = if email.ends_with("@openai.com") {
                    FeedbackAudience::OpenAiEmployee
                } else {
                    FeedbackAudience::External
                };
                (
                    Some(email.clone()),
                    Some(TelemetryAuthMode::Chatgpt),
                    Some(StatusAccountDisplay::ChatGpt {
                        email: Some(email),
                        plan: Some(plan_type_display_name(plan_type)),
                    }),
                    Some(plan_type),
                    feedback_audience,
                    true,
                )
            }
            None => (None, None, None, None, FeedbackAudience::External, false),
        };
        Ok(AppServerBootstrap {
            account_email,
            auth_mode,
            status_account_display,
            account_lease_display,
            plan_type,
            requires_openai_auth: account.requires_openai_auth,
            default_model,
            feedback_audience,
            has_chatgpt_account,
            available_models,
        })
    }

    /// Fetches the current account info without refreshing the auth token.
    ///
    /// Used by both `bootstrap` (to populate the initial UI) and `get_login_status`
    /// (to check auth mode without the overhead of a full bootstrap).
    pub(crate) async fn read_account(&mut self) -> Result<GetAccountResponse> {
        let account_request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::GetAccount {
                request_id: account_request_id,
                params: GetAccountParams {
                    refresh_token: false,
                },
            })
            .await
            .wrap_err("account/read failed during TUI bootstrap")
    }

    pub(crate) async fn read_account_lease_display(
        &self,
    ) -> Result<Option<StatusAccountLeaseDisplay>> {
        let request_id = self.next_request_id();
        let response: AccountLeaseReadResponse = self
            .client
            .request_typed(ClientRequest::AccountLeaseRead {
                request_id,
                params: None,
            })
            .await
            .wrap_err("accountLease/read failed during TUI bootstrap")?;
        let account = self
            .read_current_account_pool_account(
                response.pool_id.as_deref(),
                response.account_id.as_deref(),
            )
            .await;
        Ok(
            status_account_lease_display_from_response_and_hydration_result(
                response,
                account,
                Local::now(),
            ),
        )
    }

    async fn read_current_account_pool_account(
        &self,
        pool_id: Option<&str>,
        account_id: Option<&str>,
    ) -> Result<Option<AccountPoolAccountResponse>> {
        let (Some(pool_id), Some(account_id)) = (pool_id, account_id) else {
            return Ok(None);
        };
        let request_id = self.next_request_id();
        let response: AccountPoolAccountsListResponse = self
            .client
            .request_typed(ClientRequest::AccountPoolAccountsList {
                request_id,
                params: AccountPoolAccountsListParams {
                    pool_id: pool_id.to_string(),
                    account_id: Some(account_id.to_string()),
                    cursor: None,
                    limit: None,
                    states: None,
                    account_kinds: None,
                },
            })
            .await
            .wrap_err("accountPool/accounts/list failed during TUI bootstrap")?;
        Ok(response.data.into_iter().next())
    }

    #[allow(dead_code)]
    pub(crate) async fn read_account_lease_startup_probe(
        &self,
    ) -> Result<Option<AccountLeaseReadResponse>> {
        const METHOD: &str = "accountLease/read";
        const METHOD_NOT_FOUND: i64 = -32601;

        let request_id = self.next_request_id();
        let response = self
            .client
            .request(ClientRequest::AccountLeaseRead {
                request_id,
                params: None,
            })
            .await
            .wrap_err("accountLease/read failed during TUI startup probe")?;

        match response {
            Ok(payload) => serde_json::from_value(payload)
                .map(Some)
                .map_err(|source| TypedRequestError::Deserialize {
                    method: METHOD.to_string(),
                    source,
                })
                .wrap_err("accountLease/read failed during TUI startup probe"),
            Err(source) if source.code == METHOD_NOT_FOUND => Ok(None),
            Err(source) => Err(TypedRequestError::Server {
                method: METHOD.to_string(),
                source,
            })
            .wrap_err("accountLease/read failed during TUI startup probe"),
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn resume_pooled_startup(&self) -> Result<AccountLeaseResumeResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::AccountLeaseResume {
                request_id,
                params: None,
            })
            .await
            .wrap_err("accountLease/resume failed in TUI")
    }

    #[allow(dead_code)]
    pub(crate) async fn write_hide_pooled_only_startup_notice(&self, hide: bool) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ConfigWriteResponse = self
            .client
            .request_typed(ClientRequest::ConfigBatchWrite {
                request_id,
                params: ConfigBatchWriteParams {
                    edits: vec![ConfigEdit {
                        key_path: "notice.hide_pooled_only_startup_notice".to_string(),
                        value: serde_json::json!(hide),
                        merge_strategy: MergeStrategy::Replace,
                    }],
                    file_path: None,
                    expected_version: None,
                    reload_user_config: false,
                },
            })
            .await
            .wrap_err("config/batchWrite failed while writing pooled startup notice preference")?;
        Ok(())
    }

    pub(crate) async fn next_event(&mut self) -> Option<AppServerEvent> {
        self.client.next_event().await
    }

    pub(crate) async fn start_thread(&mut self, config: &Config) -> Result<AppServerStartedThread> {
        self.start_thread_with_session_start_source(config, /*session_start_source*/ None)
            .await
    }

    pub(crate) async fn start_thread_with_session_start_source(
        &mut self,
        config: &Config,
        session_start_source: Option<ThreadStartSource>,
    ) -> Result<AppServerStartedThread> {
        let request_id = self.next_request_id();
        let response: ThreadStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadStart {
                request_id,
                params: thread_start_params_from_config(
                    config,
                    self.thread_params_mode(),
                    self.remote_cwd_override.as_deref(),
                    session_start_source,
                ),
            })
            .await
            .wrap_err("thread/start failed during TUI bootstrap")?;
        started_thread_from_start_response(response, config).await
    }

    pub(crate) async fn resume_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        let request_id = self.next_request_id();
        let response: ThreadResumeResponse = self
            .client
            .request_typed(ClientRequest::ThreadResume {
                request_id,
                params: thread_resume_params_from_config(
                    config.clone(),
                    thread_id,
                    self.thread_params_mode(),
                    self.remote_cwd_override.as_deref(),
                ),
            })
            .await
            .wrap_err("thread/resume failed during TUI bootstrap")?;
        started_thread_from_resume_response(response, &config).await
    }

    pub(crate) async fn fork_thread(
        &mut self,
        config: Config,
        thread_id: ThreadId,
    ) -> Result<AppServerStartedThread> {
        let request_id = self.next_request_id();
        let response: ThreadForkResponse = self
            .client
            .request_typed(ClientRequest::ThreadFork {
                request_id,
                params: thread_fork_params_from_config(
                    config.clone(),
                    thread_id,
                    self.thread_params_mode(),
                    self.remote_cwd_override.as_deref(),
                ),
            })
            .await
            .wrap_err("thread/fork failed during TUI bootstrap")?;
        started_thread_from_fork_response(response, &config).await
    }

    fn thread_params_mode(&self) -> ThreadParamsMode {
        match &self.client {
            AppServerClient::InProcess(_) => ThreadParamsMode::Embedded,
            AppServerClient::Remote(_) => ThreadParamsMode::Remote,
        }
    }

    pub(crate) async fn thread_list(
        &mut self,
        params: ThreadListParams,
    ) -> Result<ThreadListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadList { request_id, params })
            .await
            .wrap_err("thread/list failed during TUI session lookup")
    }

    /// Lists thread ids that the app server currently holds in memory.
    ///
    /// Used by `App::backfill_loaded_subagent_threads` to discover subagent threads that were
    /// spawned before the TUI connected. The caller then fetches full metadata per thread via
    /// `thread_read` and walks the spawn tree.
    pub(crate) async fn thread_loaded_list(
        &mut self,
        params: ThreadLoadedListParams,
    ) -> Result<ThreadLoadedListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadLoadedList { request_id, params })
            .await
            .wrap_err("failed to list loaded threads from app server")
    }

    pub(crate) async fn thread_read(
        &mut self,
        thread_id: ThreadId,
        include_turns: bool,
    ) -> Result<Thread> {
        let request_id = self.next_request_id();
        let response: ThreadReadResponse = self
            .client
            .request_typed(ClientRequest::ThreadRead {
                request_id,
                params: ThreadReadParams {
                    thread_id: thread_id.to_string(),
                    include_turns,
                },
            })
            .await
            .wrap_err("thread/read failed during TUI session lookup")?;
        Ok(response.thread)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn turn_start(
        &mut self,
        thread_id: ThreadId,
        items: Vec<codex_protocol::user_input::UserInput>,
        cwd: PathBuf,
        approval_policy: AskForApproval,
        approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
        sandbox_policy: SandboxPolicy,
        model: String,
        effort: Option<codex_protocol::openai_models::ReasoningEffort>,
        summary: Option<codex_protocol::config_types::ReasoningSummary>,
        service_tier: Option<Option<codex_protocol::config_types::ServiceTier>>,
        collaboration_mode: Option<codex_protocol::config_types::CollaborationMode>,
        personality: Option<codex_protocol::config_types::Personality>,
        output_schema: Option<serde_json::Value>,
    ) -> Result<TurnStartResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::TurnStart {
                request_id,
                params: TurnStartParams {
                    thread_id: thread_id.to_string(),
                    input: items.into_iter().map(Into::into).collect(),
                    responsesapi_client_metadata: None,
                    cwd: Some(cwd),
                    approval_policy: Some(approval_policy.into()),
                    approvals_reviewer: Some(approvals_reviewer.into()),
                    sandbox_policy: Some(sandbox_policy.into()),
                    model: Some(model),
                    service_tier,
                    effort,
                    summary,
                    personality,
                    output_schema,
                    collaboration_mode,
                },
            })
            .await
            .wrap_err("turn/start failed in TUI")
    }

    pub(crate) async fn turn_interrupt(
        &mut self,
        thread_id: ThreadId,
        turn_id: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: TurnInterruptResponse = self
            .client
            .request_typed(ClientRequest::TurnInterrupt {
                request_id,
                params: TurnInterruptParams {
                    thread_id: thread_id.to_string(),
                    turn_id,
                },
            })
            .await
            .wrap_err("turn/interrupt failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn turn_steer(
        &mut self,
        thread_id: ThreadId,
        turn_id: String,
        items: Vec<codex_protocol::user_input::UserInput>,
    ) -> std::result::Result<TurnSteerResponse, TypedRequestError> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::TurnSteer {
                request_id,
                params: TurnSteerParams {
                    thread_id: thread_id.to_string(),
                    input: items.into_iter().map(Into::into).collect(),
                    responsesapi_client_metadata: None,
                    expected_turn_id: turn_id,
                },
            })
            .await
    }

    pub(crate) async fn thread_set_name(
        &mut self,
        thread_id: ThreadId,
        name: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadSetNameResponse = self
            .client
            .request_typed(ClientRequest::ThreadSetName {
                request_id,
                params: ThreadSetNameParams {
                    thread_id: thread_id.to_string(),
                    name,
                },
            })
            .await
            .wrap_err("thread/name/set failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_unsubscribe(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadUnsubscribeResponse = self
            .client
            .request_typed(ClientRequest::ThreadUnsubscribe {
                request_id,
                params: ThreadUnsubscribeParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/unsubscribe failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_compact_start(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadCompactStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadCompactStart {
                request_id,
                params: ThreadCompactStartParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/compact/start failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_shell_command(
        &mut self,
        thread_id: ThreadId,
        command: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadShellCommandResponse = self
            .client
            .request_typed(ClientRequest::ThreadShellCommand {
                request_id,
                params: ThreadShellCommandParams {
                    thread_id: thread_id.to_string(),
                    command,
                },
            })
            .await
            .wrap_err("thread/shellCommand failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_background_terminals_clean(
        &mut self,
        thread_id: ThreadId,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadBackgroundTerminalsCleanResponse = self
            .client
            .request_typed(ClientRequest::ThreadBackgroundTerminalsClean {
                request_id,
                params: ThreadBackgroundTerminalsCleanParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/backgroundTerminals/clean failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_rollback(
        &mut self,
        thread_id: ThreadId,
        num_turns: u32,
    ) -> Result<ThreadRollbackResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadRollback {
                request_id,
                params: ThreadRollbackParams {
                    thread_id: thread_id.to_string(),
                    num_turns,
                },
            })
            .await
            .wrap_err("thread/rollback failed in TUI")
    }

    pub(crate) async fn review_start(
        &mut self,
        thread_id: ThreadId,
        review_request: ReviewRequest,
    ) -> Result<ReviewStartResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ReviewStart {
                request_id,
                params: ReviewStartParams {
                    thread_id: thread_id.to_string(),
                    target: review_target_to_app_server(review_request.target),
                    delivery: Some(ReviewDelivery::Inline),
                },
            })
            .await
            .wrap_err("review/start failed in TUI")
    }

    pub(crate) async fn skills_list(
        &mut self,
        params: SkillsListParams,
    ) -> Result<SkillsListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::SkillsList { request_id, params })
            .await
            .wrap_err("skills/list failed in TUI")
    }

    pub(crate) async fn reload_user_config(&mut self) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ConfigWriteResponse = self
            .client
            .request_typed(ClientRequest::ConfigBatchWrite {
                request_id,
                params: ConfigBatchWriteParams {
                    edits: Vec::new(),
                    file_path: None,
                    expected_version: None,
                    reload_user_config: true,
                },
            })
            .await
            .wrap_err("config/batchWrite failed while reloading user config in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_start(
        &mut self,
        thread_id: ThreadId,
        params: ConversationStartParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStart {
                request_id,
                params: ThreadRealtimeStartParams {
                    thread_id: thread_id.to_string(),
                    output_modality: params.output_modality,
                    prompt: params.prompt,
                    session_id: params.session_id,
                    voice: params.voice,
                    transport: params.transport.map(|transport| match transport {
                        ConversationStartTransport::Websocket => {
                            ThreadRealtimeStartTransport::Websocket
                        }
                        ConversationStartTransport::Webrtc { sdp } => {
                            ThreadRealtimeStartTransport::Webrtc { sdp }
                        }
                    }),
                },
            })
            .await
            .wrap_err("thread/realtime/start failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_audio(
        &mut self,
        thread_id: ThreadId,
        params: ConversationAudioParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeAppendAudioResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeAppendAudio {
                request_id,
                params: ThreadRealtimeAppendAudioParams {
                    thread_id: thread_id.to_string(),
                    audio: params.frame.into(),
                },
            })
            .await
            .wrap_err("thread/realtime/appendAudio failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_text(
        &mut self,
        thread_id: ThreadId,
        params: ConversationTextParams,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeAppendTextResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeAppendText {
                request_id,
                params: ThreadRealtimeAppendTextParams {
                    thread_id: thread_id.to_string(),
                    text: params.text,
                },
            })
            .await
            .wrap_err("thread/realtime/appendText failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_stop(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStopResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStop {
                request_id,
                params: ThreadRealtimeStopParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/realtime/stop failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn reject_server_request(
        &self,
        request_id: RequestId,
        error: JSONRPCErrorError,
    ) -> std::io::Result<()> {
        self.client.reject_server_request(request_id, error).await
    }

    pub(crate) async fn resolve_server_request(
        &self,
        request_id: RequestId,
        result: serde_json::Value,
    ) -> std::io::Result<()> {
        self.client.resolve_server_request(request_id, result).await
    }

    pub(crate) async fn shutdown(self) -> std::io::Result<()> {
        self.client.shutdown().await
    }

    pub(crate) fn request_handle(&self) -> AppServerRequestHandle {
        self.client.request_handle()
    }

    fn next_request_id(&self) -> RequestId {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        RequestId::Integer(request_id)
    }
}

fn status_account_lease_display_from_response(
    response: AccountLeaseReadResponse,
    account: Option<&AccountPoolAccountResponse>,
    captured_at: chrono::DateTime<Local>,
) -> Option<StatusAccountLeaseDisplay> {
    let quota_families = account
        .map(|account| status_account_quota_families_from_response(&account.quotas, captured_at))
        .unwrap_or_default();
    let effective_quota_family = account.and_then(|account| {
        let selection_family = if account.selection_family.is_empty() {
            "codex"
        } else {
            account.selection_family.as_str()
        };
        account
            .quotas
            .iter()
            .find(|quota| quota.limit_id == selection_family)
            .or_else(|| {
                if selection_family == "codex" {
                    None
                } else {
                    account
                        .quotas
                        .iter()
                        .find(|quota| quota.limit_id == "codex")
                }
            })
    });
    let next_probe_after = effective_quota_family
        .and_then(|quota| quota.next_probe_after)
        .filter(|timestamp| *timestamp > captured_at.timestamp())
        .and_then(|timestamp| format_local_timestamp(timestamp, captured_at));
    let has_proactive_switch_note = response.proactive_switch_pending == Some(true)
        && response.proactive_switch_suppressed == Some(true);
    let has_visible_state = response.active
        || response.suppressed
        || response.pool_id.is_some()
        || response.account_id.is_some()
        || response.health_state.is_some()
        || response.switch_reason.is_some()
        || response.suppression_reason.is_some()
        || response.transport_reset_generation.is_some()
        || response.last_remote_context_reset_turn_id.is_some()
        || has_proactive_switch_note
        || response.proactive_switch_allowed_at.is_some()
        || response.next_eligible_at.is_some()
        || next_probe_after.is_some()
        || !quota_families.is_empty();
    if !has_visible_state {
        return None;
    }

    let proactive_switch_allowed_at = response
        .proactive_switch_allowed_at
        .and_then(|timestamp| format_local_timestamp(timestamp, captured_at));
    let next_eligible_at = response
        .next_eligible_at
        .and_then(|timestamp| format_local_timestamp(timestamp, captured_at));
    let health = response
        .health_state
        .as_deref()
        .and_then(account_lease_health_text);
    let status_primary = if response.active {
        "Active"
    } else if response.suppressed {
        "Suppressed"
    } else if next_eligible_at.is_some() || next_probe_after.is_some() {
        "Cooling down"
    } else {
        "Waiting"
    };
    let status = match health {
        Some(health) if !response.suppressed => format!("{status_primary} · {health}"),
        Some(_) | None => status_primary.to_string(),
    };
    let proactive_switch_note = has_proactive_switch_note
        .then(|| "Automatic switch held by minimum switch interval".to_string());
    let quota_note =
        effective_quota_family.and_then(|quota| account_quota_note(quota, captured_at));

    Some(StatusAccountLeaseDisplay {
        pool_id: response.pool_id,
        account_id: response.account_id,
        status,
        note: response
            .suppression_reason
            .as_deref()
            .map(account_lease_reason_text)
            .or(proactive_switch_note)
            .or_else(|| {
                response
                    .switch_reason
                    .as_deref()
                    .map(account_lease_reason_text)
            })
            .or(quota_note),
        proactive_switch_allowed_at,
        next_eligible_at,
        next_probe_after,
        remote_reset: account_lease_remote_reset_text(
            response.transport_reset_generation,
            response.last_remote_context_reset_turn_id.as_deref(),
        ),
        quota_families,
    })
}

fn status_account_lease_display_from_response_and_hydration_result(
    response: AccountLeaseReadResponse,
    account: Result<Option<AccountPoolAccountResponse>>,
    captured_at: chrono::DateTime<Local>,
) -> Option<StatusAccountLeaseDisplay> {
    let account = match account {
        Ok(account) => account,
        Err(err) => {
            tracing::debug!(error = %err, "account pool account hydration unavailable");
            None
        }
    };
    status_account_lease_display_from_response(response, account.as_ref(), captured_at)
}

fn account_quota_note(
    quota: &AccountPoolQuotaFamilyResponse,
    captured_at: chrono::DateTime<Local>,
) -> Option<String> {
    if quota
        .next_probe_after
        .is_some_and(|timestamp| timestamp > captured_at.timestamp())
    {
        return Some("Quota probe throttle active".to_string());
    }
    if matches!(quota.exhausted_windows.as_str(), "secondary" | "both") {
        return Some("Blocked by secondary quota window".to_string());
    }
    if quota.exhausted_windows.as_str() == "primary" {
        return Some("Blocked by primary quota window".to_string());
    }
    if quota.exhausted_windows.as_str() == "unknown" {
        return Some("Blocked by quota window".to_string());
    }
    None
}

fn status_account_quota_families_from_response(
    quotas: &[AccountPoolQuotaFamilyResponse],
    captured_at: chrono::DateTime<Local>,
) -> Vec<StatusAccountQuotaFamilyDisplay> {
    let mut quotas = quotas.to_vec();
    quotas.sort_by(|left, right| left.limit_id.cmp(&right.limit_id));
    quotas
        .into_iter()
        .map(|quota| StatusAccountQuotaFamilyDisplay {
            limit_id: quota.limit_id,
            primary: status_account_quota_window_from_response(quota.primary, captured_at),
            secondary: status_account_quota_window_from_response(quota.secondary, captured_at),
            exhausted_windows: quota.exhausted_windows,
            predicted_blocked_until: quota
                .predicted_blocked_until
                .and_then(|timestamp| format_local_timestamp(timestamp, captured_at)),
            next_probe_after: quota
                .next_probe_after
                .and_then(|timestamp| format_future_local_timestamp(timestamp, captured_at)),
        })
        .collect()
}

fn status_account_quota_window_from_response(
    window: AccountPoolQuotaWindowResponse,
    captured_at: chrono::DateTime<Local>,
) -> StatusAccountQuotaWindowDisplay {
    StatusAccountQuotaWindowDisplay {
        used_percent: window
            .used_percent
            .map(|used_percent| format!("{used_percent:.0}% used")),
        resets_at: window
            .resets_at
            .and_then(|timestamp| format_local_timestamp(timestamp, captured_at)),
    }
}

fn format_local_timestamp(timestamp: i64, captured_at: chrono::DateTime<Local>) -> Option<String> {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|dt| format_reset_timestamp(dt, captured_at))
}

fn format_future_local_timestamp(
    timestamp: i64,
    captured_at: chrono::DateTime<Local>,
) -> Option<String> {
    if timestamp <= captured_at.timestamp() {
        return None;
    }
    format_local_timestamp(timestamp, captured_at)
}

fn account_lease_health_text(health_state: &str) -> Option<&'static str> {
    match health_state {
        "healthy" => Some("Healthy"),
        "unhealthy" => Some("Unhealthy"),
        "busy" => Some("Busy"),
        "unavailable" => Some("Unavailable"),
        _ => None,
    }
}

fn account_lease_reason_text(reason: &str) -> String {
    match reason {
        "automaticAccountSelected" => "Automatic selection in use".to_string(),
        "preferredAccountSelected" => "Preferred account selected".to_string(),
        "missingPool" => "Configured pool is missing".to_string(),
        "preferredAccountMissing" => "Preferred account is missing".to_string(),
        "preferredAccountInOtherPool" => "Preferred account belongs to another pool".to_string(),
        "preferredAccountDisabled" => "Preferred account is disabled".to_string(),
        "preferredAccountUnhealthy" => "Preferred account is unhealthy".to_string(),
        "preferredAccountBusy" => "Preferred account is busy".to_string(),
        "noEligibleAccount" => "No eligible account is available".to_string(),
        "durablySuppressed" => "Pooled startup is suppressed until resumed".to_string(),
        "nonReplayableTurn" => {
            "Current turn was not replayed; future turns will use the next eligible account"
                .to_string()
        }
        _ => reason.to_string(),
    }
}

fn account_lease_remote_reset_text(
    generation: Option<u64>,
    turn_id: Option<&str>,
) -> Option<String> {
    match (generation, turn_id) {
        (Some(generation), Some(turn_id)) => Some(format!("gen {generation} after turn {turn_id}")),
        (Some(generation), None) => Some(format!("gen {generation}")),
        (None, Some(turn_id)) => Some(format!("after turn {turn_id}")),
        (None, None) => None,
    }
}

pub(crate) fn status_account_display_from_auth_mode(
    auth_mode: Option<AuthMode>,
    plan_type: Option<codex_protocol::account::PlanType>,
) -> Option<StatusAccountDisplay> {
    match auth_mode {
        Some(AuthMode::ApiKey) => Some(StatusAccountDisplay::ApiKey),
        Some(AuthMode::Chatgpt) | Some(AuthMode::ChatgptAuthTokens) => {
            Some(StatusAccountDisplay::ChatGpt {
                email: None,
                plan: plan_type.map(plan_type_display_name),
            })
        }
        None => None,
    }
}

fn model_preset_from_api_model(model: ApiModel) -> ModelPreset {
    let upgrade = model.upgrade.map(|upgrade_id| {
        let upgrade_info = model.upgrade_info.clone();
        ModelUpgrade {
            id: upgrade_id,
            reasoning_effort_mapping: None,
            migration_config_key: model.model.clone(),
            model_link: upgrade_info
                .as_ref()
                .and_then(|info| info.model_link.clone()),
            upgrade_copy: upgrade_info
                .as_ref()
                .and_then(|info| info.upgrade_copy.clone()),
            migration_markdown: upgrade_info.and_then(|info| info.migration_markdown),
        }
    });

    ModelPreset {
        id: model.id,
        model: model.model,
        display_name: model.display_name,
        description: model.description,
        default_reasoning_effort: model.default_reasoning_effort,
        supported_reasoning_efforts: model
            .supported_reasoning_efforts
            .into_iter()
            .map(|effort| ReasoningEffortPreset {
                effort: effort.reasoning_effort,
                description: effort.description,
            })
            .collect(),
        supports_personality: model.supports_personality,
        additional_speed_tiers: model.additional_speed_tiers,
        is_default: model.is_default,
        upgrade,
        show_in_picker: !model.hidden,
        availability_nux: model.availability_nux.map(|nux| ModelAvailabilityNux {
            message: nux.message,
        }),
        // `model/list` already returns models filtered for the active client/auth context.
        supported_in_api: true,
        input_modalities: model.input_modalities,
    }
}

fn approvals_reviewer_override_from_config(
    config: &Config,
) -> Option<codex_app_server_protocol::ApprovalsReviewer> {
    Some(config.approvals_reviewer.into())
}

fn config_request_overrides_from_config(
    config: &Config,
) -> Option<HashMap<String, serde_json::Value>> {
    config.active_profile.as_ref().map(|profile| {
        HashMap::from([(
            "profile".to_string(),
            serde_json::Value::String(profile.clone()),
        )])
    })
}

fn sandbox_mode_from_policy(
    policy: SandboxPolicy,
) -> Option<codex_app_server_protocol::SandboxMode> {
    match policy {
        SandboxPolicy::DangerFullAccess => {
            Some(codex_app_server_protocol::SandboxMode::DangerFullAccess)
        }
        SandboxPolicy::ReadOnly { .. } => Some(codex_app_server_protocol::SandboxMode::ReadOnly),
        SandboxPolicy::WorkspaceWrite { .. } => {
            Some(codex_app_server_protocol::SandboxMode::WorkspaceWrite)
        }
        SandboxPolicy::ExternalSandbox { .. } => None,
    }
}

fn thread_start_params_from_config(
    config: &Config,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&std::path::Path>,
    session_start_source: Option<ThreadStartSource>,
) -> ThreadStartParams {
    ThreadStartParams {
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(config),
        cwd: thread_cwd_from_config(config, thread_params_mode, remote_cwd_override),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(config),
        ephemeral: Some(config.ephemeral),
        session_start_source,
        persist_extended_history: true,
        ..ThreadStartParams::default()
    }
}

fn thread_resume_params_from_config(
    config: Config,
    thread_id: ThreadId,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&std::path::Path>,
) -> ThreadResumeParams {
    ThreadResumeParams {
        thread_id: thread_id.to_string(),
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(&config),
        cwd: thread_cwd_from_config(&config, thread_params_mode, remote_cwd_override),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(&config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(&config),
        persist_extended_history: true,
        ..ThreadResumeParams::default()
    }
}

fn thread_fork_params_from_config(
    config: Config,
    thread_id: ThreadId,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&std::path::Path>,
) -> ThreadForkParams {
    ThreadForkParams {
        thread_id: thread_id.to_string(),
        model: config.model.clone(),
        model_provider: thread_params_mode.model_provider_from_config(&config),
        cwd: thread_cwd_from_config(&config, thread_params_mode, remote_cwd_override),
        approval_policy: Some(config.permissions.approval_policy.value().into()),
        approvals_reviewer: approvals_reviewer_override_from_config(&config),
        sandbox: sandbox_mode_from_policy(config.permissions.sandbox_policy.get().clone()),
        config: config_request_overrides_from_config(&config),
        ephemeral: config.ephemeral,
        persist_extended_history: true,
        ..ThreadForkParams::default()
    }
}

fn thread_cwd_from_config(
    config: &Config,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<&std::path::Path>,
) -> Option<String> {
    match thread_params_mode {
        ThreadParamsMode::Embedded => Some(config.cwd.to_string_lossy().to_string()),
        ThreadParamsMode::Remote => {
            remote_cwd_override.map(|cwd| cwd.to_string_lossy().to_string())
        }
    }
}

async fn started_thread_from_start_response(
    response: ThreadStartResponse,
    config: &Config,
) -> Result<AppServerStartedThread> {
    let session = thread_session_state_from_thread_start_response(&response, config)
        .await
        .map_err(color_eyre::eyre::Report::msg)?;
    Ok(AppServerStartedThread {
        session,
        turns: response.thread.turns,
    })
}

async fn started_thread_from_resume_response(
    response: ThreadResumeResponse,
    config: &Config,
) -> Result<AppServerStartedThread> {
    let session = thread_session_state_from_thread_resume_response(&response, config)
        .await
        .map_err(color_eyre::eyre::Report::msg)?;
    Ok(AppServerStartedThread {
        session,
        turns: response.thread.turns,
    })
}

async fn started_thread_from_fork_response(
    response: ThreadForkResponse,
    config: &Config,
) -> Result<AppServerStartedThread> {
    let session = thread_session_state_from_thread_fork_response(&response, config)
        .await
        .map_err(color_eyre::eyre::Report::msg)?;
    Ok(AppServerStartedThread {
        session,
        turns: response.thread.turns,
    })
}

async fn thread_session_state_from_thread_start_response(
    response: &ThreadStartResponse,
    config: &Config,
) -> Result<ThreadSessionState, String> {
    thread_session_state_from_thread_response(
        &response.thread.id,
        response.thread.forked_from_id.clone(),
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.instruction_sources.clone(),
        response.reasoning_effort,
        config,
    )
    .await
}

async fn thread_session_state_from_thread_resume_response(
    response: &ThreadResumeResponse,
    config: &Config,
) -> Result<ThreadSessionState, String> {
    thread_session_state_from_thread_response(
        &response.thread.id,
        response.thread.forked_from_id.clone(),
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.instruction_sources.clone(),
        response.reasoning_effort,
        config,
    )
    .await
}

async fn thread_session_state_from_thread_fork_response(
    response: &ThreadForkResponse,
    config: &Config,
) -> Result<ThreadSessionState, String> {
    thread_session_state_from_thread_response(
        &response.thread.id,
        response.thread.forked_from_id.clone(),
        response.thread.name.clone(),
        response.thread.path.clone(),
        response.model.clone(),
        response.model_provider.clone(),
        response.service_tier,
        response.approval_policy.to_core(),
        response.approvals_reviewer.to_core(),
        response.sandbox.to_core(),
        response.cwd.clone(),
        response.instruction_sources.clone(),
        response.reasoning_effort,
        config,
    )
    .await
}

fn review_target_to_app_server(
    target: CoreReviewTarget,
) -> codex_app_server_protocol::ReviewTarget {
    match target {
        CoreReviewTarget::UncommittedChanges => {
            codex_app_server_protocol::ReviewTarget::UncommittedChanges
        }
        CoreReviewTarget::BaseBranch { branch } => {
            codex_app_server_protocol::ReviewTarget::BaseBranch { branch }
        }
        CoreReviewTarget::Commit { sha, title } => {
            codex_app_server_protocol::ReviewTarget::Commit { sha, title }
        }
        CoreReviewTarget::Custom { instructions } => {
            codex_app_server_protocol::ReviewTarget::Custom { instructions }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "session mapping keeps explicit fields"
)]
async fn thread_session_state_from_thread_response(
    thread_id: &str,
    forked_from_id: Option<String>,
    thread_name: Option<String>,
    rollout_path: Option<PathBuf>,
    model: String,
    model_provider_id: String,
    service_tier: Option<codex_protocol::config_types::ServiceTier>,
    approval_policy: AskForApproval,
    approvals_reviewer: codex_protocol::config_types::ApprovalsReviewer,
    sandbox_policy: SandboxPolicy,
    cwd: PathBuf,
    instruction_source_paths: Vec<PathBuf>,
    reasoning_effort: Option<codex_protocol::openai_models::ReasoningEffort>,
    config: &Config,
) -> Result<ThreadSessionState, String> {
    let thread_id = ThreadId::from_string(thread_id)
        .map_err(|err| format!("thread id `{thread_id}` is invalid: {err}"))?;
    let forked_from_id = forked_from_id
        .as_deref()
        .map(ThreadId::from_string)
        .transpose()
        .map_err(|err| format!("forked_from_id is invalid: {err}"))?;
    let (history_log_id, history_entry_count) = message_history_metadata(config).await;
    let history_entry_count = u64::try_from(history_entry_count).unwrap_or(u64::MAX);

    Ok(ThreadSessionState {
        thread_id,
        forked_from_id,
        thread_name,
        model,
        model_provider_id,
        service_tier,
        approval_policy,
        approvals_reviewer,
        sandbox_policy,
        cwd,
        instruction_source_paths,
        reasoning_effort,
        history_log_id,
        history_entry_count,
        network_proxy: None,
        rollout_path,
    })
}

pub(crate) fn app_server_rate_limit_snapshots_to_core(
    response: GetAccountRateLimitsResponse,
) -> Vec<RateLimitSnapshot> {
    let mut snapshots = Vec::new();
    snapshots.push(app_server_rate_limit_snapshot_to_core(response.rate_limits));
    if let Some(by_limit_id) = response.rate_limits_by_limit_id {
        snapshots.extend(
            by_limit_id
                .into_values()
                .map(app_server_rate_limit_snapshot_to_core),
        );
    }
    snapshots
}

pub(crate) fn app_server_rate_limit_snapshot_to_core(
    snapshot: codex_app_server_protocol::RateLimitSnapshot,
) -> RateLimitSnapshot {
    RateLimitSnapshot {
        limit_id: snapshot.limit_id,
        limit_name: snapshot.limit_name,
        primary: snapshot.primary.map(app_server_rate_limit_window_to_core),
        secondary: snapshot.secondary.map(app_server_rate_limit_window_to_core),
        credits: snapshot.credits.map(app_server_credits_snapshot_to_core),
        plan_type: snapshot.plan_type,
    }
}

fn app_server_rate_limit_window_to_core(
    window: codex_app_server_protocol::RateLimitWindow,
) -> RateLimitWindow {
    RateLimitWindow {
        used_percent: window.used_percent as f64,
        window_minutes: window.window_duration_mins,
        resets_at: window.resets_at,
    }
}

fn app_server_credits_snapshot_to_core(
    snapshot: codex_app_server_protocol::CreditsSnapshot,
) -> CreditsSnapshot {
    CreditsSnapshot {
        has_credits: snapshot.has_credits,
        unlimited: snapshot.unlimited,
        balance: snapshot.balance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_core::config::ConfigBuilder;
    use chrono::TimeZone;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnStatus;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    async fn build_config(temp_dir: &TempDir) -> Config {
        ConfigBuilder::default()
            .codex_home(temp_dir.path().to_path_buf())
            .build()
            .await
            .expect("config should build")
    }

    #[tokio::test]
    async fn thread_start_params_include_cwd_for_embedded_sessions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;

        let params = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Embedded,
            /*remote_cwd_override*/ None,
            /*session_start_source*/ None,
        );

        assert_eq!(params.cwd, Some(config.cwd.to_string_lossy().to_string()));
        assert_eq!(params.model_provider, Some(config.model_provider_id));
    }

    #[tokio::test]
    async fn thread_start_params_can_mark_clear_source() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;

        let params = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Embedded,
            /*remote_cwd_override*/ None,
            Some(ThreadStartSource::Clear),
        );

        assert_eq!(params.session_start_source, Some(ThreadStartSource::Clear));
    }

    #[tokio::test]
    async fn thread_lifecycle_params_omit_cwd_without_remote_override_for_remote_sessions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();

        let start = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Remote,
            /*remote_cwd_override*/ None,
            /*session_start_source*/ None,
        );
        let resume = thread_resume_params_from_config(
            config.clone(),
            thread_id,
            ThreadParamsMode::Remote,
            /*remote_cwd_override*/ None,
        );
        let fork = thread_fork_params_from_config(
            config,
            thread_id,
            ThreadParamsMode::Remote,
            /*remote_cwd_override*/ None,
        );

        assert_eq!(start.cwd, None);
        assert_eq!(resume.cwd, None);
        assert_eq!(fork.cwd, None);
        assert_eq!(start.model_provider, None);
        assert_eq!(resume.model_provider, None);
        assert_eq!(fork.model_provider, None);
    }

    #[tokio::test]
    async fn thread_lifecycle_params_forward_explicit_remote_cwd_override_for_remote_sessions() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();
        let remote_cwd = PathBuf::from("repo/on/server");

        let start = thread_start_params_from_config(
            &config,
            ThreadParamsMode::Remote,
            Some(remote_cwd.as_path()),
            /*session_start_source*/ None,
        );
        let resume = thread_resume_params_from_config(
            config.clone(),
            thread_id,
            ThreadParamsMode::Remote,
            Some(remote_cwd.as_path()),
        );
        let fork = thread_fork_params_from_config(
            config,
            thread_id,
            ThreadParamsMode::Remote,
            Some(remote_cwd.as_path()),
        );

        assert_eq!(start.cwd.as_deref(), Some("repo/on/server"));
        assert_eq!(resume.cwd.as_deref(), Some("repo/on/server"));
        assert_eq!(fork.cwd.as_deref(), Some("repo/on/server"));
        assert_eq!(start.model_provider, None);
        assert_eq!(resume.model_provider, None);
        assert_eq!(fork.model_provider, None);
    }

    #[tokio::test]
    async fn resume_response_restores_turns_from_thread_items() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();
        let forked_from_id = ThreadId::new();
        let response = ThreadResumeResponse {
            thread: codex_app_server_protocol::Thread {
                id: thread_id.to_string(),
                forked_from_id: Some(forked_from_id.to_string()),
                preview: "hello".to_string(),
                ephemeral: false,
                model_provider: "openai".to_string(),
                created_at: 1,
                updated_at: 2,
                status: ThreadStatus::Idle,
                path: None,
                cwd: PathBuf::from("/tmp/project"),
                cli_version: "0.0.0".to_string(),
                source: codex_protocol::protocol::SessionSource::Cli.into(),
                agent_nickname: None,
                agent_role: None,
                git_info: None,
                name: None,
                turns: vec![Turn {
                    id: "turn-1".to_string(),
                    items: vec![
                        codex_app_server_protocol::ThreadItem::UserMessage {
                            id: "user-1".to_string(),
                            content: vec![codex_app_server_protocol::UserInput::Text {
                                text: "hello from history".to_string(),
                                text_elements: Vec::new(),
                            }],
                        },
                        codex_app_server_protocol::ThreadItem::AgentMessage {
                            id: "assistant-1".to_string(),
                            text: "assistant reply".to_string(),
                            phase: None,
                            memory_citation: None,
                        },
                    ],
                    status: TurnStatus::Completed,
                    error: None,
                    started_at: None,
                    completed_at: None,
                    duration_ms: None,
                }],
            },
            model: "gpt-5.4".to_string(),
            model_provider: "openai".to_string(),
            service_tier: None,
            cwd: PathBuf::from("/tmp/project"),
            instruction_sources: vec![PathBuf::from("/tmp/project/AGENTS.md")],
            approval_policy: codex_protocol::protocol::AskForApproval::Never.into(),
            approvals_reviewer: codex_app_server_protocol::ApprovalsReviewer::User,
            sandbox: codex_protocol::protocol::SandboxPolicy::new_read_only_policy().into(),
            reasoning_effort: None,
        };

        let started = started_thread_from_resume_response(response.clone(), &config)
            .await
            .expect("resume response should map");
        assert_eq!(started.session.forked_from_id, Some(forked_from_id));
        assert_eq!(
            started.session.instruction_source_paths,
            response.instruction_sources
        );
        assert_eq!(started.turns.len(), 1);
        assert_eq!(started.turns[0], response.thread.turns[0]);
    }

    #[tokio::test]
    async fn session_configured_populates_history_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();

        append_message_history_entry("older", &thread_id, &config)
            .await
            .expect("history append should succeed");
        append_message_history_entry("newer", &thread_id, &config)
            .await
            .expect("history append should succeed");

        let session = thread_session_state_from_thread_response(
            &thread_id.to_string(),
            /*forked_from_id*/ None,
            Some("restore".to_string()),
            /*rollout_path*/ None,
            "gpt-5.4".to_string(),
            "openai".to_string(),
            /*service_tier*/ None,
            AskForApproval::Never,
            codex_protocol::config_types::ApprovalsReviewer::User,
            SandboxPolicy::new_read_only_policy(),
            PathBuf::from("/tmp/project"),
            Vec::new(),
            /*reasoning_effort*/ None,
            &config,
        )
        .await
        .expect("session should map");

        assert_ne!(session.history_log_id, 0);
        assert_eq!(session.history_entry_count, 2);
    }

    #[tokio::test]
    async fn session_configured_preserves_fork_source_thread_id() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config = build_config(&temp_dir).await;
        let thread_id = ThreadId::new();
        let forked_from_id = ThreadId::new();

        let session = thread_session_state_from_thread_response(
            &thread_id.to_string(),
            Some(forked_from_id.to_string()),
            Some("restore".to_string()),
            /*rollout_path*/ None,
            "gpt-5.4".to_string(),
            "openai".to_string(),
            /*service_tier*/ None,
            AskForApproval::Never,
            codex_protocol::config_types::ApprovalsReviewer::User,
            SandboxPolicy::new_read_only_policy(),
            PathBuf::from("/tmp/project"),
            Vec::new(),
            /*reasoning_effort*/ None,
            &config,
        )
        .await
        .expect("session should map");

        assert_eq!(session.forked_from_id, Some(forked_from_id));
    }

    #[test]
    fn status_account_display_from_auth_mode_uses_remapped_plan_labels() {
        let business = status_account_display_from_auth_mode(
            Some(AuthMode::Chatgpt),
            Some(codex_protocol::account::PlanType::EnterpriseCbpUsageBased),
        );
        assert!(matches!(
            business,
            Some(StatusAccountDisplay::ChatGpt {
                email: None,
                plan: Some(ref plan),
            }) if plan == "Enterprise"
        ));

        let team = status_account_display_from_auth_mode(
            Some(AuthMode::Chatgpt),
            Some(codex_protocol::account::PlanType::SelfServeBusinessUsageBased),
        );
        assert!(matches!(
            team,
            Some(StatusAccountDisplay::ChatGpt {
                email: None,
                plan: Some(ref plan),
            }) if plan == "Business"
        ));
    }

    #[test]
    fn status_account_lease_display_keeps_base_response_when_hydration_fails() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let response = AccountLeaseReadResponse {
            active: false,
            suppressed: false,
            account_id: Some("acct-1".to_string()),
            pool_id: Some("team-main".to_string()),
            lease_id: None,
            lease_epoch: None,
            lease_acquired_at: None,
            health_state: Some("healthy".to_string()),
            switch_reason: None,
            suppression_reason: None,
            transport_reset_generation: None,
            last_remote_context_reset_turn_id: None,
            min_switch_interval_secs: None,
            proactive_switch_pending: None,
            proactive_switch_suppressed: None,
            proactive_switch_allowed_at: None,
            next_eligible_at: None,
            effective_pool_resolution_source: None,
            configured_default_pool_id: None,
            persisted_default_pool_id: None,
        };

        let display = status_account_lease_display_from_response_and_hydration_result(
            response,
            Err(color_eyre::eyre::eyre!("account hydration unavailable")),
            captured_at,
        );

        assert_eq!(
            display,
            Some(StatusAccountLeaseDisplay {
                pool_id: Some("team-main".to_string()),
                account_id: Some("acct-1".to_string()),
                status: "Waiting · Healthy".to_string(),
                note: None,
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                next_probe_after: None,
                remote_reset: None,
                quota_families: Vec::new(),
            })
        );
    }

    #[test]
    fn status_account_lease_display_from_response_hides_empty_state() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: false,
                suppressed: false,
                account_id: None,
                pool_id: None,
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: None,
                health_state: None,
                switch_reason: None,
                suppression_reason: None,
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: None,
                proactive_switch_pending: None,
                proactive_switch_suppressed: None,
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                effective_pool_resolution_source: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
            },
            None,
            captured_at,
        );

        assert_eq!(display, None);
    }

    #[test]
    fn status_account_lease_display_from_response_formats_pool_details() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: false,
                suppressed: false,
                account_id: Some("acct-2".to_string()),
                pool_id: Some("legacy-default".to_string()),
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: None,
                health_state: Some("busy".to_string()),
                switch_reason: Some("automaticAccountSelected".to_string()),
                suppression_reason: None,
                transport_reset_generation: Some(2),
                last_remote_context_reset_turn_id: Some("turn-17".to_string()),
                min_switch_interval_secs: None,
                proactive_switch_pending: None,
                proactive_switch_suppressed: None,
                proactive_switch_allowed_at: None,
                next_eligible_at: Some(
                    (captured_at + chrono::Duration::days(1))
                        .with_timezone(&chrono::Utc)
                        .timestamp(),
                ),
                effective_pool_resolution_source: Some("persistedSelection".to_string()),
                configured_default_pool_id: None,
                persisted_default_pool_id: Some("legacy-default".to_string()),
            },
            None,
            captured_at,
        );

        assert_eq!(
            display,
            Some(StatusAccountLeaseDisplay {
                pool_id: Some("legacy-default".to_string()),
                account_id: Some("acct-2".to_string()),
                status: "Cooling down · Busy".to_string(),
                note: Some("Automatic selection in use".to_string()),
                proactive_switch_allowed_at: None,
                next_eligible_at: Some("03:04 on 11 Apr".to_string()),
                next_probe_after: None,
                remote_reset: Some("gen 2 after turn turn-17".to_string()),
                quota_families: Vec::new(),
            })
        );
    }

    #[test]
    fn status_account_lease_display_from_response_formats_damped_proactive_switch() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: true,
                suppressed: false,
                account_id: Some("acct-1".to_string()),
                pool_id: Some("team-main".to_string()),
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: Some(captured_at.with_timezone(&chrono::Utc).timestamp()),
                health_state: Some("healthy".to_string()),
                switch_reason: None,
                suppression_reason: None,
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: Some(600),
                proactive_switch_pending: Some(true),
                proactive_switch_suppressed: Some(true),
                proactive_switch_allowed_at: Some(
                    (captured_at + chrono::Duration::minutes(20))
                        .with_timezone(&chrono::Utc)
                        .timestamp(),
                ),
                next_eligible_at: None,
                effective_pool_resolution_source: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
            },
            None,
            captured_at,
        );

        assert_eq!(
            display,
            Some(StatusAccountLeaseDisplay {
                pool_id: Some("team-main".to_string()),
                account_id: Some("acct-1".to_string()),
                status: "Active · Healthy".to_string(),
                note: Some("Automatic switch held by minimum switch interval".to_string()),
                proactive_switch_allowed_at: Some("03:24".to_string()),
                next_eligible_at: None,
                next_probe_after: None,
                remote_reset: None,
                quota_families: Vec::new(),
            })
        );
    }

    #[test]
    fn status_account_lease_display_from_response_hides_inert_damping_metadata() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: false,
                suppressed: false,
                account_id: None,
                pool_id: None,
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: Some(captured_at.with_timezone(&chrono::Utc).timestamp()),
                health_state: None,
                switch_reason: None,
                suppression_reason: None,
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: Some(600),
                proactive_switch_pending: Some(false),
                proactive_switch_suppressed: Some(false),
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                effective_pool_resolution_source: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
            },
            None,
            captured_at,
        );

        assert_eq!(display, None);
    }

    #[test]
    fn status_account_lease_display_prefers_damping_note_over_switch_reason() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: true,
                suppressed: false,
                account_id: Some("acct-1".to_string()),
                pool_id: Some("team-main".to_string()),
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: Some(captured_at.with_timezone(&chrono::Utc).timestamp()),
                health_state: Some("healthy".to_string()),
                switch_reason: Some("automaticAccountSelected".to_string()),
                suppression_reason: None,
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: Some(600),
                proactive_switch_pending: Some(true),
                proactive_switch_suppressed: Some(true),
                proactive_switch_allowed_at: Some(
                    (captured_at + chrono::Duration::minutes(20))
                        .with_timezone(&chrono::Utc)
                        .timestamp(),
                ),
                next_eligible_at: None,
                effective_pool_resolution_source: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
            },
            None,
            captured_at,
        );

        assert_eq!(
            display,
            Some(StatusAccountLeaseDisplay {
                pool_id: Some("team-main".to_string()),
                account_id: Some("acct-1".to_string()),
                status: "Active · Healthy".to_string(),
                note: Some("Automatic switch held by minimum switch interval".to_string()),
                proactive_switch_allowed_at: Some("03:24".to_string()),
                next_eligible_at: None,
                next_probe_after: None,
                remote_reset: None,
                quota_families: Vec::new(),
            })
        );
    }

    #[test]
    fn status_account_lease_display_derives_probe_metadata_from_quota_families() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let next_probe_after = (captured_at + chrono::Duration::minutes(20))
            .with_timezone(&chrono::Utc)
            .timestamp();
        let blocked_until = (captured_at + chrono::Duration::hours(1))
            .with_timezone(&chrono::Utc)
            .timestamp();
        let account = AccountPoolAccountResponse {
            account_id: "acct-1".to_string(),
            backend_account_ref: None,
            account_kind: "chatgpt".to_string(),
            selection_family: "chatgpt".to_string(),
            enabled: true,
            health_state: Some("healthy".to_string()),
            operational_state: Some(
                codex_app_server_protocol::AccountOperationalState::CoolingDown,
            ),
            allocatable: Some(false),
            status_reason_code: None,
            status_message: None,
            current_lease: None,
            quota: None,
            quotas: vec![
                quota_family(
                    "codex",
                    "primary",
                    /*predicted_blocked_until*/ Some(blocked_until),
                    /*next_probe_after*/ None,
                ),
                quota_family(
                    "chatgpt",
                    "secondary",
                    /*predicted_blocked_until*/ Some(blocked_until),
                    Some(next_probe_after),
                ),
            ],
            selection: None,
            updated_at: captured_at.with_timezone(&chrono::Utc).timestamp(),
        };

        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: false,
                suppressed: false,
                account_id: Some("acct-1".to_string()),
                pool_id: Some("team-main".to_string()),
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: None,
                health_state: Some("healthy".to_string()),
                switch_reason: None,
                suppression_reason: None,
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: None,
                proactive_switch_pending: None,
                proactive_switch_suppressed: None,
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                effective_pool_resolution_source: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
            },
            Some(&account),
            captured_at,
        );

        assert_eq!(
            display,
            Some(StatusAccountLeaseDisplay {
                pool_id: Some("team-main".to_string()),
                account_id: Some("acct-1".to_string()),
                status: "Cooling down · Healthy".to_string(),
                note: Some("Quota probe throttle active".to_string()),
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                next_probe_after: Some("03:24".to_string()),
                remote_reset: None,
                quota_families: vec![
                    StatusAccountQuotaFamilyDisplay {
                        limit_id: "chatgpt".to_string(),
                        primary: StatusAccountQuotaWindowDisplay {
                            used_percent: Some("42% used".to_string()),
                            resets_at: None,
                        },
                        secondary: StatusAccountQuotaWindowDisplay {
                            used_percent: Some("100% used".to_string()),
                            resets_at: Some("04:04".to_string()),
                        },
                        exhausted_windows: "secondary".to_string(),
                        predicted_blocked_until: Some("04:04".to_string()),
                        next_probe_after: Some("03:24".to_string()),
                    },
                    StatusAccountQuotaFamilyDisplay {
                        limit_id: "codex".to_string(),
                        primary: StatusAccountQuotaWindowDisplay {
                            used_percent: Some("42% used".to_string()),
                            resets_at: None,
                        },
                        secondary: StatusAccountQuotaWindowDisplay {
                            used_percent: Some("100% used".to_string()),
                            resets_at: Some("04:04".to_string()),
                        },
                        exhausted_windows: "primary".to_string(),
                        predicted_blocked_until: Some("04:04".to_string()),
                        next_probe_after: None,
                    },
                ],
            })
        );
    }

    #[test]
    fn status_account_lease_display_ignores_past_probe_timestamp_for_quota_note() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let stale_probe_after = (captured_at - chrono::Duration::minutes(5))
            .with_timezone(&chrono::Utc)
            .timestamp();
        let blocked_until = (captured_at + chrono::Duration::hours(1))
            .with_timezone(&chrono::Utc)
            .timestamp();
        let account = AccountPoolAccountResponse {
            account_id: "acct-1".to_string(),
            backend_account_ref: None,
            account_kind: "chatgpt".to_string(),
            selection_family: "chatgpt".to_string(),
            enabled: true,
            health_state: Some("healthy".to_string()),
            operational_state: Some(
                codex_app_server_protocol::AccountOperationalState::CoolingDown,
            ),
            allocatable: Some(false),
            status_reason_code: None,
            status_message: None,
            current_lease: None,
            quota: None,
            quotas: vec![quota_family(
                "chatgpt",
                "secondary",
                /*predicted_blocked_until*/ Some(blocked_until),
                Some(stale_probe_after),
            )],
            selection: None,
            updated_at: captured_at.with_timezone(&chrono::Utc).timestamp(),
        };

        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: false,
                suppressed: false,
                account_id: Some("acct-1".to_string()),
                pool_id: Some("team-main".to_string()),
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: None,
                health_state: Some("healthy".to_string()),
                switch_reason: None,
                suppression_reason: None,
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: None,
                proactive_switch_pending: None,
                proactive_switch_suppressed: None,
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                effective_pool_resolution_source: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
            },
            Some(&account),
            captured_at,
        );

        assert_eq!(
            display,
            Some(StatusAccountLeaseDisplay {
                pool_id: Some("team-main".to_string()),
                account_id: Some("acct-1".to_string()),
                status: "Waiting · Healthy".to_string(),
                note: Some("Blocked by secondary quota window".to_string()),
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                next_probe_after: None,
                remote_reset: None,
                quota_families: vec![StatusAccountQuotaFamilyDisplay {
                    limit_id: "chatgpt".to_string(),
                    primary: StatusAccountQuotaWindowDisplay {
                        used_percent: Some("42% used".to_string()),
                        resets_at: None,
                    },
                    secondary: StatusAccountQuotaWindowDisplay {
                        used_percent: Some("100% used".to_string()),
                        resets_at: Some("04:04".to_string()),
                    },
                    exhausted_windows: "secondary".to_string(),
                    predicted_blocked_until: Some("04:04".to_string()),
                    next_probe_after: None,
                }],
            })
        );
    }

    #[test]
    fn status_account_lease_display_uses_selection_family_for_quota_note() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let next_probe_after = (captured_at + chrono::Duration::minutes(20))
            .with_timezone(&chrono::Utc)
            .timestamp();
        let blocked_until = (captured_at + chrono::Duration::hours(1))
            .with_timezone(&chrono::Utc)
            .timestamp();
        let account = AccountPoolAccountResponse {
            account_id: "acct-1".to_string(),
            backend_account_ref: None,
            account_kind: "chatgpt".to_string(),
            selection_family: "local".to_string(),
            enabled: true,
            health_state: Some("healthy".to_string()),
            operational_state: Some(
                codex_app_server_protocol::AccountOperationalState::CoolingDown,
            ),
            allocatable: Some(false),
            status_reason_code: None,
            status_message: None,
            current_lease: None,
            quota: None,
            quotas: vec![
                quota_family(
                    "codex",
                    "secondary",
                    /*predicted_blocked_until*/ Some(blocked_until),
                    Some(next_probe_after),
                ),
                quota_family(
                    "local",
                    "primary",
                    /*predicted_blocked_until*/ Some(blocked_until),
                    /*next_probe_after*/ None,
                ),
            ],
            selection: None,
            updated_at: captured_at.with_timezone(&chrono::Utc).timestamp(),
        };

        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: false,
                suppressed: false,
                account_id: Some("acct-1".to_string()),
                pool_id: Some("team-main".to_string()),
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: None,
                health_state: Some("healthy".to_string()),
                switch_reason: None,
                suppression_reason: None,
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: None,
                proactive_switch_pending: None,
                proactive_switch_suppressed: None,
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                effective_pool_resolution_source: None,
                configured_default_pool_id: None,
                persisted_default_pool_id: None,
            },
            Some(&account),
            captured_at,
        );

        assert_eq!(
            display,
            Some(StatusAccountLeaseDisplay {
                pool_id: Some("team-main".to_string()),
                account_id: Some("acct-1".to_string()),
                status: "Waiting · Healthy".to_string(),
                note: Some("Blocked by primary quota window".to_string()),
                proactive_switch_allowed_at: None,
                next_eligible_at: None,
                next_probe_after: None,
                remote_reset: None,
                quota_families: vec![
                    StatusAccountQuotaFamilyDisplay {
                        limit_id: "codex".to_string(),
                        primary: StatusAccountQuotaWindowDisplay {
                            used_percent: Some("42% used".to_string()),
                            resets_at: None,
                        },
                        secondary: StatusAccountQuotaWindowDisplay {
                            used_percent: Some("100% used".to_string()),
                            resets_at: Some("04:04".to_string()),
                        },
                        exhausted_windows: "secondary".to_string(),
                        predicted_blocked_until: Some("04:04".to_string()),
                        next_probe_after: Some("03:24".to_string()),
                    },
                    StatusAccountQuotaFamilyDisplay {
                        limit_id: "local".to_string(),
                        primary: StatusAccountQuotaWindowDisplay {
                            used_percent: Some("42% used".to_string()),
                            resets_at: None,
                        },
                        secondary: StatusAccountQuotaWindowDisplay {
                            used_percent: Some("100% used".to_string()),
                            resets_at: Some("04:04".to_string()),
                        },
                        exhausted_windows: "primary".to_string(),
                        predicted_blocked_until: Some("04:04".to_string()),
                        next_probe_after: None,
                    },
                ],
            })
        );
    }

    #[test]
    fn status_account_lease_display_from_response_formats_non_replayable_turn_reason() {
        let captured_at = chrono::Local
            .with_ymd_and_hms(2024, 4, 10, 3, 4, 5)
            .single()
            .expect("timestamp");
        let display = status_account_lease_display_from_response(
            AccountLeaseReadResponse {
                active: true,
                suppressed: false,
                account_id: Some("acct-1".to_string()),
                pool_id: Some("legacy-default".to_string()),
                lease_id: None,
                lease_epoch: None,
                lease_acquired_at: None,
                health_state: Some("healthy".to_string()),
                switch_reason: Some("nonReplayableTurn".to_string()),
                suppression_reason: None,
                transport_reset_generation: None,
                last_remote_context_reset_turn_id: None,
                min_switch_interval_secs: None,
                proactive_switch_pending: None,
                proactive_switch_suppressed: None,
                proactive_switch_allowed_at: None,
                next_eligible_at: Some(
                    (captured_at + chrono::Duration::minutes(20))
                        .with_timezone(&chrono::Utc)
                        .timestamp(),
                ),
                effective_pool_resolution_source: Some("persistedSelection".to_string()),
                configured_default_pool_id: None,
                persisted_default_pool_id: Some("legacy-default".to_string()),
            },
            None,
            captured_at,
        );

        assert_eq!(
            display,
            Some(StatusAccountLeaseDisplay {
                pool_id: Some("legacy-default".to_string()),
                account_id: Some("acct-1".to_string()),
                status: "Active · Healthy".to_string(),
                note: Some(
                    "Current turn was not replayed; future turns will use the next eligible account"
                        .to_string(),
                ),
                proactive_switch_allowed_at: None,
                next_eligible_at: Some("03:24".to_string()),
                next_probe_after: None,
                remote_reset: None,
                quota_families: Vec::new(),
            })
        );
    }

    fn quota_family(
        limit_id: &str,
        exhausted_windows: &str,
        predicted_blocked_until: Option<i64>,
        next_probe_after: Option<i64>,
    ) -> AccountPoolQuotaFamilyResponse {
        AccountPoolQuotaFamilyResponse {
            limit_id: limit_id.to_string(),
            primary: AccountPoolQuotaWindowResponse {
                used_percent: Some(42.0),
                resets_at: None,
            },
            secondary: AccountPoolQuotaWindowResponse {
                used_percent: Some(100.0),
                resets_at: predicted_blocked_until,
            },
            exhausted_windows: exhausted_windows.to_string(),
            predicted_blocked_until,
            next_probe_after,
            observed_at: 1,
        }
    }
}
