//! Bridges Apps SDK-style `openai/fileParams` metadata into Codex's MCP flow.
//!
//! Strategy:
//! - Inspect `_meta["openai/fileParams"]` to discover which tool arguments are
//!   file inputs.
//! - At tool execution time, upload those local files to OpenAI file storage
//!   and rewrite only the declared arguments into the provided-file payload
//!   shape expected by the downstream Apps tool.
//!
//! Model-visible schema masking is owned by `codex-mcp` alongside MCP tool
//! inventory, so this module only handles the execution-time argument rewrite.

use crate::runtime_lease::RequestBoundaryKind;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use codex_api::AuthProvider;
use codex_api::OpenAiFileError;
use codex_api::OpenAiFileRequestKind;
use codex_api::PendingOpenAiFileUpload;
use codex_api::TransportError;
use codex_api::finalize_local_file_upload;
use codex_api::start_local_file_upload;
use codex_async_utils::OrCancelExt;
use codex_login::AuthRecovery;
use codex_login::CodexAuth;
use codex_model_provider::AuthorizationHeaderAuthProvider;
use codex_model_provider::BearerAuthProvider;
use http::StatusCode;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
enum OpenAiFileRewriteError {
    Message(String),
    Upload {
        field_name: String,
        index: Option<usize>,
        file_path: String,
        error: OpenAiFileError,
    },
}

impl OpenAiFileRewriteError {
    fn into_message(self) -> String {
        match self {
            Self::Message(message) => message,
            Self::Upload {
                field_name,
                index,
                file_path,
                error,
            } => format_openai_file_upload_error(&field_name, index, &file_path, error),
        }
    }

    fn unauthorized_transport(&self) -> Option<TransportError> {
        let Self::Upload { error, .. } = self else {
            return None;
        };
        let OpenAiFileError::UnexpectedStatus {
            request_kind: OpenAiFileRequestKind::Create | OpenAiFileRequestKind::Finalize,
            url,
            status,
            body,
        } = error
        else {
            return None;
        };
        if *status != StatusCode::UNAUTHORIZED {
            return None;
        }
        Some(TransportError::Http {
            status: *status,
            url: Some(url.clone()),
            headers: None,
            body: Some(body.clone()),
        })
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct OpenAiFileArgumentSlot {
    field_name: String,
    index: Option<usize>,
    file_path: String,
}

pub(crate) async fn rewrite_mcp_tool_arguments_for_openai_files(
    sess: &Session,
    turn_context: &TurnContext,
    arguments_value: Option<JsonValue>,
    openai_file_input_params: Option<&[String]>,
    cancellation_token: CancellationToken,
) -> Result<Option<JsonValue>, String> {
    let Some(openai_file_input_params) = openai_file_input_params else {
        return Ok(arguments_value);
    };

    let Some(arguments_value) = arguments_value else {
        return Ok(None);
    };
    let Some(arguments) = arguments_value.as_object() else {
        return Ok(Some(arguments_value));
    };
    let pooled_runtime_active = sess.services.model_client.pooled_runtime_authority_active();
    let collaboration_tree_id = sess.services.model_client.current_collaboration_tree_id();
    let collaboration_member_id = sess.services.model_client.current_collaboration_member_id();
    let mut rewritten_arguments = arguments.clone();
    let mut auth_recovery: Option<Box<dyn AuthRecovery>> = None;
    let mut upload_progress = HashMap::new();
    loop {
        let request_cancellation_token = cancellation_token.child_token();
        let (auth, reporter, next_auth_recovery, _guard) = if pooled_runtime_active {
            let admitted_setup = sess
                .services
                .model_client
                .admitted_client_setup(
                    RequestBoundaryKind::BackgroundModelCall,
                    crate::client::LeaseRequestPurpose::Standard,
                    crate::client::AdmittedClientSetupRequest {
                        collaboration_tree_id: &collaboration_tree_id,
                        collaboration_member_id: collaboration_member_id.clone(),
                        turn_id: None,
                        request_id: "openai-file-upload",
                        cancellation_token: request_cancellation_token.clone(),
                    },
                )
                .await
                .map_err(|error| {
                    format!("failed to acquire auth for OpenAI file upload: {error}")
                })?;
            let crate::client::AdmittedClientSetup {
                setup,
                reporter,
                auth_recovery: next_auth_recovery,
                guard,
            } = admitted_setup;
            (setup.auth, reporter, next_auth_recovery, guard)
        } else {
            // Non-pooled sessions retain the legacy auth source. When a pooled runtime
            // authority is published, upload auth must come from admission above.
            (
                sess.current_auth().await,
                None,
                sess.services.model_client.current_auth_recovery_legacy(),
                None,
            )
        };

        let mut request_auth_recovery =
            crate::client::merge_auth_recovery(next_auth_recovery, &mut auth_recovery);
        match rewrite_openai_file_arguments_once(
            sess,
            turn_context,
            &mut rewritten_arguments,
            openai_file_input_params,
            auth.as_ref(),
            &request_cancellation_token,
            &mut upload_progress,
        )
        .await
        {
            Ok(()) => {
                if rewritten_arguments == *arguments {
                    return Ok(Some(arguments_value));
                }
                return Ok(Some(JsonValue::Object(rewritten_arguments)));
            }
            Err(error) => {
                if let Some(unauthorized_transport) = error.unauthorized_transport() {
                    match crate::client::handle_unauthorized(
                        unauthorized_transport,
                        &mut request_auth_recovery,
                        &turn_context.session_telemetry,
                    )
                    .await
                    {
                        Ok(_) => {
                            auth_recovery = request_auth_recovery;
                            continue;
                        }
                        Err(recovery_error) => {
                            if let Some(reporter) = reporter.as_ref() {
                                reporter.report_terminal_unauthorized().await;
                            }
                            return Err(recovery_error.to_string());
                        }
                    }
                }
                if let Some(reporter) = reporter.as_ref()
                    && error.unauthorized_transport().is_some()
                {
                    reporter.report_terminal_unauthorized().await;
                }
                return Err(error.into_message());
            }
        }
    }
}

async fn rewrite_openai_file_arguments_once(
    sess: &Session,
    turn_context: &TurnContext,
    arguments: &mut serde_json::Map<String, JsonValue>,
    openai_file_input_params: &[String],
    auth: Option<&CodexAuth>,
    cancellation_token: &CancellationToken,
    upload_progress: &mut HashMap<OpenAiFileArgumentSlot, PendingOpenAiFileUpload>,
) -> Result<(), OpenAiFileRewriteError> {
    for field_name in openai_file_input_params {
        let Some(value) = arguments.get_mut(field_name) else {
            continue;
        };
        rewrite_argument_value_for_openai_files(
            sess,
            turn_context,
            auth,
            cancellation_token,
            field_name,
            value,
            upload_progress,
        )
        .await?;
    }
    Ok(())
}

async fn rewrite_argument_value_for_openai_files(
    sess: &Session,
    turn_context: &TurnContext,
    auth: Option<&CodexAuth>,
    cancellation_token: &CancellationToken,
    field_name: &str,
    value: &mut JsonValue,
    upload_progress: &mut HashMap<OpenAiFileArgumentSlot, PendingOpenAiFileUpload>,
) -> Result<(), OpenAiFileRewriteError> {
    match value {
        JsonValue::String(path_or_file_ref) => {
            let path_or_file_ref = path_or_file_ref.clone();
            let rewritten = build_uploaded_local_argument_value(
                sess,
                turn_context,
                auth,
                cancellation_token,
                field_name,
                /*index*/ None,
                &path_or_file_ref,
                upload_progress,
            )
            .await?;
            *value = rewritten;
        }
        JsonValue::Array(values) => {
            validate_openai_file_argument_array(field_name, values)?;
            for (index, item) in values.iter_mut().enumerate() {
                let Some(path_or_file_ref) = item.as_str().map(ToString::to_string) else {
                    continue;
                };
                let rewritten = build_uploaded_local_argument_value(
                    sess,
                    turn_context,
                    auth,
                    cancellation_token,
                    field_name,
                    Some(index),
                    &path_or_file_ref,
                    upload_progress,
                )
                .await?;
                *item = rewritten;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_openai_file_argument_array(
    field_name: &str,
    values: &[JsonValue],
) -> Result<(), OpenAiFileRewriteError> {
    for (index, item) in values.iter().enumerate() {
        if item.is_string() || is_existing_openai_file_argument_value(item) {
            continue;
        }
        return Err(OpenAiFileRewriteError::Message(format!(
            "OpenAI file argument `{field_name}[{index}]` must be a string file path or file object"
        )));
    }
    Ok(())
}

fn is_existing_openai_file_argument_value(value: &JsonValue) -> bool {
    let JsonValue::Object(map) = value else {
        return false;
    };
    map.get("uri").and_then(JsonValue::as_str).is_some()
        || map.get("file_id").and_then(JsonValue::as_str).is_some()
}

enum OpenAiFileUploadAuth {
    AgentAssertion(AuthorizationHeaderAuthProvider),
    Bearer(BearerAuthProvider),
}

impl AuthProvider for OpenAiFileUploadAuth {
    fn add_auth_headers(&self, headers: &mut http::HeaderMap) {
        match self {
            Self::AgentAssertion(auth) => auth.add_auth_headers(headers),
            Self::Bearer(auth) => auth.add_auth_headers(headers),
        }
    }
}

async fn build_uploaded_local_argument_value(
    _sess: &Session,
    turn_context: &TurnContext,
    auth: Option<&CodexAuth>,
    cancellation_token: &CancellationToken,
    field_name: &str,
    index: Option<usize>,
    file_path: &str,
    upload_progress: &mut HashMap<OpenAiFileArgumentSlot, PendingOpenAiFileUpload>,
) -> Result<JsonValue, OpenAiFileRewriteError> {
    let resolved_path = turn_context.resolve_path(Some(file_path.to_string()));
    let Some(auth) = auth else {
        return Err(OpenAiFileRewriteError::Message(
            "ChatGPT auth is required to upload local files for Codex Apps tools".to_string(),
        ));
    };
    let upload_auth = if let Some(authorization_header_value) = auth
        .agent_identity_authorization_header()
        .map_err(|error| {
            OpenAiFileRewriteError::Message(format!(
                "failed to build agent assertion authorization: {error}"
            ))
        })? {
        let mut auth_provider = AuthorizationHeaderAuthProvider::new(
            Some(authorization_header_value),
            /*account_id*/ None,
        );
        if auth.is_fedramp_account() {
            auth_provider = auth_provider.with_fedramp_routing_header();
        }
        OpenAiFileUploadAuth::AgentAssertion(auth_provider)
    } else {
        let token_data = auth.get_token_data().map_err(|error| {
            OpenAiFileRewriteError::Message(format!(
                "failed to read ChatGPT auth for file upload: {error}"
            ))
        })?;
        OpenAiFileUploadAuth::Bearer(BearerAuthProvider {
            token: Some(token_data.access_token),
            account_id: token_data.account_id,
            is_fedramp_account: auth.is_fedramp_account(),
        })
    };
    let slot = OpenAiFileArgumentSlot {
        field_name: field_name.to_string(),
        index,
        file_path: file_path.to_string(),
    };
    let pending_upload = if let Some(pending_upload) = upload_progress.get(&slot).cloned() {
        pending_upload
    } else {
        let pending_upload = match start_local_file_upload(
            turn_context.config.chatgpt_base_url.trim_end_matches('/'),
            &upload_auth,
            &resolved_path,
        )
        .or_cancel(cancellation_token)
        .await
        {
            Ok(Ok(pending_upload)) => pending_upload,
            Ok(Err(error)) => {
                return Err(OpenAiFileRewriteError::Upload {
                    field_name: field_name.to_string(),
                    index,
                    file_path: file_path.to_string(),
                    error,
                });
            }
            Err(_) => {
                return Err(OpenAiFileRewriteError::Message(format!(
                    "cancelled upload of `{file_path}` for `{field_name}`"
                )));
            }
        };
        upload_progress.insert(slot, pending_upload.clone());
        pending_upload
    };
    let uploaded = match finalize_local_file_upload(
        turn_context.config.chatgpt_base_url.trim_end_matches('/'),
        &upload_auth,
        &pending_upload,
    )
    .or_cancel(cancellation_token)
    .await
    {
        Ok(Ok(uploaded)) => uploaded,
        Ok(Err(error)) => {
            return Err(OpenAiFileRewriteError::Upload {
                field_name: field_name.to_string(),
                index,
                file_path: file_path.to_string(),
                error,
            });
        }
        Err(_) => {
            return Err(OpenAiFileRewriteError::Message(format!(
                "cancelled upload of `{file_path}` for `{field_name}`"
            )));
        }
    };
    Ok(serde_json::json!({
        "download_url": uploaded.download_url,
        "file_id": uploaded.file_id,
        "mime_type": uploaded.mime_type,
        "file_name": uploaded.file_name,
        "uri": uploaded.uri,
        "file_size_bytes": uploaded.file_size_bytes,
    }))
}

fn format_openai_file_upload_error(
    field_name: &str,
    index: Option<usize>,
    file_path: &str,
    error: OpenAiFileError,
) -> String {
    match index {
        Some(index) => {
            format!("failed to upload `{file_path}` for `{field_name}[{index}]`: {error}")
        }
        None => format!("failed to upload `{file_path}` for `{field_name}`: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease_auth::SessionLeaseAuth;
    use crate::session::session::Session;
    use crate::session::tests::make_session_and_context;
    use base64::Engine;
    use codex_login::LeasedTurnAuth;
    use codex_login::auth::AgentIdentityAuth;
    use codex_login::auth::AgentIdentityAuthRecord;
    use codex_login::auth::LeaseAuthBinding;
    use codex_login::auth::LeaseScopedAuthSession;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use tempfile::tempdir;

    async fn install_runtime_lease_authority(
        session: &mut Session,
        authority: crate::runtime_lease::RuntimeLeaseAuthority,
    ) {
        let runtime_lease_host =
            crate::runtime_lease::RuntimeLeaseHost::pooled_with_authority_for_test(
                crate::runtime_lease::RuntimeLeaseHostId::new(
                    "runtime-lease-mcp-openai-file-test".to_string(),
                ),
                authority,
            );
        let session_id = session.conversation_id.to_string();
        let provider = session.provider().await;
        session.services.runtime_lease_host = Some(runtime_lease_host.clone());
        session.services.model_client = crate::client::ModelClient::new_with_runtime_lease(
            /*auth_manager*/ None,
            /*lease_auth*/ None,
            Some(runtime_lease_host),
            Some(Arc::new(tokio::sync::Mutex::new(
                crate::runtime_lease::SessionLeaseView::new(),
            ))),
            session_id.clone(),
            Arc::new(crate::runtime_lease::CollaborationTreeBindingHandle::new(
                crate::runtime_lease::CollaborationTreeId::root_for_session(&session_id),
            )),
            session.conversation_id,
            "11111111-1111-4111-8111-111111111111".to_string(),
            provider,
            codex_protocol::protocol::SessionSource::Exec,
            /*model_verbosity*/ None,
            /*enable_request_compression*/ false,
            /*include_timing_metrics*/ false,
            /*beta_features_header*/ None,
        );
    }

    async fn install_legacy_lease_auth(session: &mut Session, lease_auth: Arc<SessionLeaseAuth>) {
        let provider = session.provider().await;
        session.services.lease_auth = lease_auth.clone();
        session.services.model_client = crate::client::ModelClient::new(
            /*auth_manager*/ None,
            Some(lease_auth),
            session.conversation_id,
            "11111111-1111-4111-8111-111111111111".to_string(),
            provider,
            codex_protocol::protocol::SessionSource::Exec,
            /*model_verbosity*/ None,
            /*enable_request_compression*/ false,
            /*include_timing_metrics*/ false,
            /*beta_features_header*/ None,
        );
    }

    fn fake_access_token(account_id: &str) -> String {
        let header = serde_json::json!({
            "alg": "none",
            "typ": "JWT",
        });
        let payload = serde_json::json!({
            "email": "user@example.com",
            "email_verified": true,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_user_id": "user-12345",
                "chatgpt_account_id": account_id,
            },
        });
        let b64 = |value: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&value).expect("serialize fake JWT part"))
        };
        format!("{}.{}.sig", b64(header), b64(payload))
    }

    struct RefreshingLeaseScopedAuthSession {
        binding: LeaseAuthBinding,
        refresh_calls: AtomicUsize,
    }

    impl RefreshingLeaseScopedAuthSession {
        fn new(account_id: &str) -> Self {
            Self {
                binding: LeaseAuthBinding {
                    account_id: account_id.to_string(),
                    backend_account_handle: format!("handle-{account_id}"),
                    lease_epoch: 1,
                },
                refresh_calls: AtomicUsize::new(0),
            }
        }
    }

    impl LeaseScopedAuthSession for RefreshingLeaseScopedAuthSession {
        fn leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> {
            Ok(LeasedTurnAuth::chatgpt(
                self.binding.account_id.clone(),
                fake_access_token(&self.binding.account_id),
            ))
        }

        fn refresh_leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> {
            self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            Ok(LeasedTurnAuth::chatgpt(
                self.binding.account_id.clone(),
                fake_access_token(&self.binding.account_id),
            ))
        }

        fn binding(&self) -> &LeaseAuthBinding {
            &self.binding
        }

        fn ensure_current(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    const TEST_AGENT_PRIVATE_KEY_PKCS8_BASE64: &str =
        "MC4CAQAwBQYDK2VwBCIEIHvmvq9bJ3HdN0riqn1H9V1FQFkKVMwhleIDul/h6thR";

    async fn make_agent_identity_auth(chatgpt_base_url: &str) -> CodexAuth {
        let auth = CodexAuth::AgentIdentity(AgentIdentityAuth::new(AgentIdentityAuthRecord {
            agent_runtime_id: "agent-123".to_string(),
            agent_private_key: TEST_AGENT_PRIVATE_KEY_PKCS8_BASE64.to_string(),
            account_id: "account_id".to_string(),
            chatgpt_user_id: "user-12345".to_string(),
            email: "user@example.com".to_string(),
            plan_type: codex_protocol::account::PlanType::Pro,
            chatgpt_account_is_fedramp: false,
        }));
        auth.initialize_runtime(Some(chatgpt_base_url.to_string()))
            .await
            .expect("initialize agent identity runtime");
        auth
    }
    #[tokio::test]
    async fn openai_file_argument_rewrite_requires_declared_file_params() {
        let (session, turn_context) = make_session_and_context().await;
        let arguments = Some(serde_json::json!({
            "file": "/tmp/codex-smoke-file.txt"
        }));

        let rewritten = rewrite_mcp_tool_arguments_for_openai_files(
            &session,
            &Arc::new(turn_context),
            arguments.clone(),
            /*openai_file_input_params*/ None,
            CancellationToken::new(),
        )
        .await
        .expect("rewrite should succeed");

        assert_eq!(rewritten, arguments);
    }

    #[tokio::test]
    async fn build_uploaded_local_argument_value_uploads_local_file_path() {
        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::body_json;
        use wiremock::matchers::header;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .and(header("chatgpt-account-id", "account_id"))
            .and(body_json(serde_json::json!({
                "file_name": "file_report.csv",
                "file_size": 5,
                "use_case": "codex",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_123",
                "upload_url": format!("{}/upload/file_123", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_123"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_123/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_123", server.uri()),
                "file_name": "file_report.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 5,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (session, mut turn_context) = make_session_and_context().await;
        let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);
        let mut upload_progress = HashMap::new();

        let rewritten = build_uploaded_local_argument_value(
            &session,
            &turn_context,
            Some(&auth),
            &CancellationToken::new(),
            "file",
            /*index*/ None,
            "file_report.csv",
            &mut upload_progress,
        )
        .await
        .expect("rewrite should upload the local file");

        assert_eq!(
            rewritten,
            serde_json::json!({
                "download_url": format!("{}/download/file_123", server.uri()),
                "file_id": "file_123",
                "mime_type": "text/csv",
                "file_name": "file_report.csv",
                "uri": "sediment://file_123",
                "file_size_bytes": 5,
            })
        );
    }

    #[tokio::test]
    async fn rewrite_argument_value_for_openai_files_rewrites_scalar_path() {
        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::body_json;
        use wiremock::matchers::header;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .and(header("chatgpt-account-id", "account_id"))
            .and(body_json(serde_json::json!({
                "file_name": "file_report.csv",
                "file_size": 5,
                "use_case": "codex",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_123",
                "upload_url": format!("{}/upload/file_123", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_123"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_123/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_123", server.uri()),
                "file_name": "file_report.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 5,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (session, mut turn_context) = make_session_and_context().await;
        let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);
        let mut rewritten = serde_json::json!("file_report.csv");
        let mut upload_progress = HashMap::new();
        rewrite_argument_value_for_openai_files(
            &session,
            &turn_context,
            Some(&auth),
            &CancellationToken::new(),
            "file",
            &mut rewritten,
            &mut upload_progress,
        )
        .await
        .expect("rewrite should succeed");

        assert_eq!(
            rewritten,
            serde_json::json!({
                "download_url": format!("{}/download/file_123", server.uri()),
                "file_id": "file_123",
                "mime_type": "text/csv",
                "file_name": "file_report.csv",
                "uri": "sediment://file_123",
                "file_size_bytes": 5,
            })
        );
    }

    #[tokio::test]
    async fn rewrite_argument_value_for_openai_files_rewrites_array_paths() {
        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::body_json;
        use wiremock::matchers::header;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .and(header("chatgpt-account-id", "account_id"))
            .and(body_json(serde_json::json!({
                "file_name": "one.csv",
                "file_size": 3,
                "use_case": "codex",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_1",
                "upload_url": format!("{}/upload/file_1", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .and(header("chatgpt-account-id", "account_id"))
            .and(body_json(serde_json::json!({
                "file_name": "two.csv",
                "file_size": 3,
                "use_case": "codex",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_2",
                "upload_url": format!("{}/upload/file_2", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_1"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_2"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_1/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_1", server.uri()),
                "file_name": "one.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 3,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_2/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_2", server.uri()),
                "file_name": "two.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 3,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (session, mut turn_context) = make_session_and_context().await;
        let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
        let dir = tempdir().expect("temp dir");
        tokio::fs::write(dir.path().join("one.csv"), b"one")
            .await
            .expect("write first local file");
        tokio::fs::write(dir.path().join("two.csv"), b"two")
            .await
            .expect("write second local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);
        let mut rewritten = serde_json::json!(["one.csv", "two.csv"]);
        let mut upload_progress = HashMap::new();
        rewrite_argument_value_for_openai_files(
            &session,
            &turn_context,
            Some(&auth),
            &CancellationToken::new(),
            "files",
            &mut rewritten,
            &mut upload_progress,
        )
        .await
        .expect("rewrite should succeed");

        assert_eq!(
            rewritten,
            serde_json::json!([
                {
                    "download_url": format!("{}/download/file_1", server.uri()),
                    "file_id": "file_1",
                    "mime_type": "text/csv",
                    "file_name": "one.csv",
                    "uri": "sediment://file_1",
                    "file_size_bytes": 3,
                },
                {
                    "download_url": format!("{}/download/file_2", server.uri()),
                    "file_id": "file_2",
                    "mime_type": "text/csv",
                    "file_name": "two.csv",
                    "uri": "sediment://file_2",
                    "file_size_bytes": 3,
                }
            ])
        );
    }

    #[tokio::test]
    async fn rewrite_argument_value_for_openai_files_rejects_mixed_array_without_uploads() {
        use wiremock::MockServer;

        let server = MockServer::start().await;
        let (session, mut turn_context) = make_session_and_context().await;
        let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
        let dir = tempdir().expect("temp dir");
        tokio::fs::write(dir.path().join("one.csv"), b"one")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);
        let mut rewritten = serde_json::json!(["one.csv", 7]);
        let mut upload_progress = HashMap::new();
        let error = rewrite_argument_value_for_openai_files(
            &session,
            &turn_context,
            Some(&auth),
            &CancellationToken::new(),
            "files",
            &mut rewritten,
            &mut upload_progress,
        )
        .await
        .expect_err("mixed arrays should fail before any upload starts");

        assert!(error.into_message().contains("files[1]"));
        let requests = server
            .received_requests()
            .await
            .expect("captured upload requests");
        assert_eq!(requests.len(), 0);
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_surfaces_upload_failures() {
        let (mut session, turn_context) = make_session_and_context().await;
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("account_id", 7);
        install_runtime_lease_authority(&mut session, authority).await;
        let error = rewrite_mcp_tool_arguments_for_openai_files(
            &session,
            &turn_context,
            Some(serde_json::json!({
                "file": "/definitely/missing/file.csv",
            })),
            Some(&["file".to_string()]),
            CancellationToken::new(),
        )
        .await
        .expect_err("missing file should fail");

        assert!(error.contains("failed to upload"));
        assert!(error.contains("file"));
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_honors_admission_cancellation() {
        let (mut session, turn_context) = make_session_and_context().await;
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_draining("account_id", 7);
        install_runtime_lease_authority(&mut session, authority).await;
        let cancellation_token = CancellationToken::new();
        cancellation_token.cancel();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            rewrite_mcp_tool_arguments_for_openai_files(
                &session,
                &turn_context,
                Some(serde_json::json!({
                    "file": "file.csv",
                })),
                Some(&["file".to_string()]),
                cancellation_token,
            ),
        )
        .await
        .expect("cancelled admission should not remain parked");
        let error = result.expect_err("cancelled admission should fail");

        assert!(error.contains("lease admission cancelled"));
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_retries_upload_unauthorized_with_fresh_admission()
     {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;

        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::Respond;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        struct SequenceResponder {
            call_count: AtomicUsize,
            responses: Vec<ResponseTemplate>,
        }

        impl Respond for SequenceResponder {
            fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
                let call_num = self.call_count.fetch_add(1, Ordering::SeqCst);
                self.responses
                    .get(call_num)
                    .unwrap_or_else(|| panic!("no response configured for call {call_num}"))
                    .clone()
            }
        }

        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .respond_with(SequenceResponder {
                call_count: AtomicUsize::new(0),
                responses: vec![
                    ResponseTemplate::new(401).set_body_string("unauthorized"),
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "file_id": "file_lease",
                        "upload_url": format!("{}/upload/file_lease", server.uri()),
                    })),
                ],
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_lease"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_lease/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_lease", server.uri()),
                "file_name": "file_report.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 5,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (mut session, mut turn_context) = make_session_and_context().await;
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority.clone()).await;
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);

        let rewritten = rewrite_mcp_tool_arguments_for_openai_files(
            &session,
            &turn_context,
            Some(serde_json::json!({
                "file": "file_report.csv",
            })),
            Some(&["file".to_string()]),
            CancellationToken::new(),
        )
        .await
        .expect("upload 401 should recover")
        .expect("rewritten arguments");

        assert_eq!(
            rewritten,
            serde_json::json!({
                "file": {
                    "download_url": format!("{}/download/file_lease", server.uri()),
                    "file_id": "file_lease",
                    "mime_type": "text/csv",
                    "file_name": "file_report.csv",
                    "uri": "sediment://file_lease",
                    "file_size_bytes": 5,
                }
            })
        );
        let requests = server
            .received_requests()
            .await
            .expect("captured upload requests");
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests
                .into_iter()
                .map(|request| request.url.path().to_string())
                .collect::<Vec<_>>(),
            vec![
                "/backend-api/files".to_string(),
                "/backend-api/files".to_string(),
                "/upload/file_lease".to_string(),
                "/backend-api/files/file_lease/uploaded".to_string(),
            ]
        );
        assert_eq!(
            authority.recorded_boundaries_for_test(),
            vec![
                crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall,
                crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall,
            ]
        );
        assert!(authority.runtime_snapshot().await.active);
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_retries_upload_unauthorized_with_legacy_lease_auth_recovery()
     {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;

        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::Respond;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        struct SequenceResponder {
            call_count: AtomicUsize,
            responses: Vec<ResponseTemplate>,
        }

        impl Respond for SequenceResponder {
            fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
                let call_num = self.call_count.fetch_add(1, Ordering::SeqCst);
                self.responses
                    .get(call_num)
                    .unwrap_or_else(|| panic!("no response configured for call {call_num}"))
                    .clone()
            }
        }

        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .respond_with(SequenceResponder {
                call_count: AtomicUsize::new(0),
                responses: vec![
                    ResponseTemplate::new(401).set_body_string("unauthorized"),
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "file_id": "file_lease",
                        "upload_url": format!("{}/upload/file_lease", server.uri()),
                    })),
                ],
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_lease"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_lease/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_lease", server.uri()),
                "file_name": "file_report.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 5,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (mut session, mut turn_context) = make_session_and_context().await;
        let lease_auth = Arc::new(SessionLeaseAuth::default());
        let lease_session = Arc::new(RefreshingLeaseScopedAuthSession::new("leased-account"));
        lease_auth.replace_current(Some(lease_session.clone()));
        install_legacy_lease_auth(&mut session, lease_auth).await;
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);

        let rewritten = rewrite_mcp_tool_arguments_for_openai_files(
            &session,
            &turn_context,
            Some(serde_json::json!({
                "file": "file_report.csv",
            })),
            Some(&["file".to_string()]),
            CancellationToken::new(),
        )
        .await
        .expect("legacy lease auth upload 401 should recover")
        .expect("rewritten arguments");

        assert_eq!(
            rewritten,
            serde_json::json!({
                "file": {
                    "download_url": format!("{}/download/file_lease", server.uri()),
                    "file_id": "file_lease",
                    "mime_type": "text/csv",
                    "file_name": "file_report.csv",
                    "uri": "sediment://file_lease",
                    "file_size_bytes": 5,
                }
            })
        );
        assert!(lease_session.refresh_calls.load(Ordering::SeqCst) > 0);
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_does_not_retry_storage_upload_unauthorized()
     {
        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_lease",
                "upload_url": format!("{}/upload/file_lease", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_lease"))
            .respond_with(ResponseTemplate::new(401).set_body_string("storage unauthorized"))
            .expect(1)
            .mount(&server)
            .await;

        let (mut session, mut turn_context) = make_session_and_context().await;
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority.clone()).await;
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);

        let error = rewrite_mcp_tool_arguments_for_openai_files(
            &session,
            &turn_context,
            Some(serde_json::json!({
                "file": "file_report.csv",
            })),
            Some(&["file".to_string()]),
            CancellationToken::new(),
        )
        .await
        .expect_err("storage upload 401 should not retry auth recovery");

        assert!(error.contains("storage unauthorized"));
        let requests = server
            .received_requests()
            .await
            .expect("captured upload requests");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .into_iter()
                .map(|request| request.url.path().to_string())
                .collect::<Vec<_>>(),
            vec![
                "/backend-api/files".to_string(),
                "/upload/file_lease".to_string(),
            ]
        );
        assert_eq!(
            authority.recorded_boundaries_for_test(),
            vec![crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall]
        );
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_retries_finalize_unauthorized_without_reuploading_file()
     {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;

        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::Respond;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        struct SequenceResponder {
            call_count: AtomicUsize,
            responses: Vec<ResponseTemplate>,
        }

        impl Respond for SequenceResponder {
            fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
                let call_num = self.call_count.fetch_add(1, Ordering::SeqCst);
                self.responses
                    .get(call_num)
                    .unwrap_or_else(|| panic!("no response configured for call {call_num}"))
                    .clone()
            }
        }

        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_lease",
                "upload_url": format!("{}/upload/file_lease", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_lease"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_lease/uploaded"))
            .respond_with(SequenceResponder {
                call_count: AtomicUsize::new(0),
                responses: vec![
                    ResponseTemplate::new(401).set_body_string("unauthorized"),
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "status": "success",
                        "download_url": format!("{}/download/file_lease", server.uri()),
                        "file_name": "file_report.csv",
                        "mime_type": "text/csv",
                        "file_size_bytes": 5,
                    })),
                ],
            })
            .expect(2)
            .mount(&server)
            .await;

        let (mut session, mut turn_context) = make_session_and_context().await;
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority.clone()).await;
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);

        let rewritten = rewrite_mcp_tool_arguments_for_openai_files(
            &session,
            &turn_context,
            Some(serde_json::json!({
                "file": "file_report.csv",
            })),
            Some(&["file".to_string()]),
            CancellationToken::new(),
        )
        .await
        .expect("finalize 401 should recover without re-uploading")
        .expect("rewritten arguments");

        assert_eq!(
            rewritten,
            serde_json::json!({
                "file": {
                    "download_url": format!("{}/download/file_lease", server.uri()),
                    "file_id": "file_lease",
                    "mime_type": "text/csv",
                    "file_name": "file_report.csv",
                    "uri": "sediment://file_lease",
                    "file_size_bytes": 5,
                }
            })
        );
        let requests = server
            .received_requests()
            .await
            .expect("captured upload requests");
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests
                .into_iter()
                .map(|request| request.url.path().to_string())
                .collect::<Vec<_>>(),
            vec![
                "/backend-api/files".to_string(),
                "/upload/file_lease".to_string(),
                "/backend-api/files/file_lease/uploaded".to_string(),
                "/backend-api/files/file_lease/uploaded".to_string(),
            ]
        );
        assert_eq!(
            authority.recorded_boundaries_for_test(),
            vec![
                crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall,
                crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall,
            ]
        );
        assert!(authority.runtime_snapshot().await.active);
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_cancels_inflight_finalize_when_tree_reports_terminal_unauthorized()
     {
        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_lease",
                "upload_url": format!("{}/upload/file_lease", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_lease"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_lease/uploaded"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(2))
                    .set_body_json(serde_json::json!({
                        "status": "success",
                        "download_url": format!("{}/download/file_lease", server.uri()),
                        "file_name": "file_report.csv",
                        "mime_type": "text/csv",
                        "file_size_bytes": 5,
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let (mut session, mut turn_context) = make_session_and_context().await;
        let runtime_lease_host =
            crate::runtime_lease::RuntimeLeaseHost::pooled_with_authority_for_test(
                crate::runtime_lease::RuntimeLeaseHostId::new(
                    "runtime-lease-mcp-openai-file-cancel-test".to_string(),
                ),
                crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting(
                    "pooled-account",
                    7,
                ),
            );
        install_runtime_lease_authority(
            &mut session,
            runtime_lease_host
                .pooled_authority()
                .expect("runtime authority should exist"),
        )
        .await;
        let authority = session
            .services
            .runtime_lease_host
            .as_ref()
            .and_then(crate::runtime_lease::RuntimeLeaseHost::pooled_authority)
            .expect("runtime authority");
        let runtime_lease_host = session
            .services
            .runtime_lease_host
            .as_ref()
            .cloned()
            .expect("runtime host");
        let tree_id = crate::runtime_lease::CollaborationTreeId::for_test("mcp-openai-file-tree");
        let member_cancel = CancellationToken::new();
        let membership = runtime_lease_host.register_collaboration_member(
            tree_id.clone(),
            "mcp-member".to_string(),
            member_cancel.clone(),
        );
        let _binding = session
            .services
            .model_client
            .bind_collaboration_tree(membership);
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);

        let session = Arc::new(session);
        let turn_context = Arc::new(turn_context);
        let rewrite_task = tokio::spawn({
            let session = Arc::clone(&session);
            let turn_context = Arc::clone(&turn_context);
            async move {
                rewrite_mcp_tool_arguments_for_openai_files(
                    &session,
                    &turn_context,
                    Some(serde_json::json!({
                        "file": "file_report.csv",
                    })),
                    Some(&["file".to_string()]),
                    CancellationToken::new(),
                )
                .await
            }
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let requests = server
                    .received_requests()
                    .await
                    .expect("captured upload requests");
                if requests
                    .iter()
                    .any(|request| request.url.path() == "/backend-api/files/file_lease/uploaded")
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("finalize request should start");

        let sibling_admission = authority
            .acquire_request_lease_for_test(crate::runtime_lease::LeaseRequestContext::new(
                crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall,
                "mcp-sibling".to_string(),
                tree_id,
                Some("sibling".to_string()),
                CancellationToken::new(),
            ))
            .await
            .expect("sibling admission should succeed");
        authority
            .report_terminal_unauthorized(&sibling_admission.snapshot)
            .await
            .expect("terminal unauthorized should report");
        drop(sibling_admission.guard);

        let join_result = tokio::time::timeout(std::time::Duration::from_secs(1), rewrite_task)
            .await
            .expect("in-flight finalize should stop after sibling 401");
        let rewrite_result = join_result.expect("rewrite task should join");
        let error = rewrite_result.expect_err("tree cancellation should fail rewrite");
        assert!(error.to_lowercase().contains("cancel"));
        assert!(member_cancel.is_cancelled());
        assert_eq!(authority.admitted_count_for_test(), 0);
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_preserves_completed_array_uploads_across_retry()
     {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;

        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::Respond;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        struct SequenceResponder {
            call_count: AtomicUsize,
            responses: Vec<ResponseTemplate>,
        }

        impl Respond for SequenceResponder {
            fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
                let call_num = self.call_count.fetch_add(1, Ordering::SeqCst);
                self.responses
                    .get(call_num)
                    .unwrap_or_else(|| panic!("no response configured for call {call_num}"))
                    .clone()
            }
        }

        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .respond_with(SequenceResponder {
                call_count: AtomicUsize::new(0),
                responses: vec![
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "file_id": "file_a",
                        "upload_url": format!("{}/upload/file_a", server.uri()),
                    })),
                    ResponseTemplate::new(401).set_body_string("unauthorized"),
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "file_id": "file_b",
                        "upload_url": format!("{}/upload/file_b", server.uri()),
                    })),
                ],
            })
            .expect(3)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_a"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_a/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_a", server.uri()),
                "file_name": "first.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 5,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_b"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_b/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_b", server.uri()),
                "file_name": "second.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 6,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (mut session, mut turn_context) = make_session_and_context().await;
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority.clone()).await;
        let dir = tempdir().expect("temp dir");
        tokio::fs::write(dir.path().join("first.csv"), b"hello")
            .await
            .expect("write first file");
        tokio::fs::write(dir.path().join("second.csv"), b"world!")
            .await
            .expect("write second file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);

        let rewritten = rewrite_mcp_tool_arguments_for_openai_files(
            &session,
            &turn_context,
            Some(serde_json::json!({
                "files": ["first.csv", "second.csv"],
            })),
            Some(&["files".to_string()]),
            CancellationToken::new(),
        )
        .await
        .expect("array upload retry should recover")
        .expect("rewritten arguments");

        assert_eq!(
            rewritten,
            serde_json::json!({
                "files": [
                    {
                        "download_url": format!("{}/download/file_a", server.uri()),
                        "file_id": "file_a",
                        "mime_type": "text/csv",
                        "file_name": "first.csv",
                        "uri": "sediment://file_a",
                        "file_size_bytes": 5,
                    },
                    {
                        "download_url": format!("{}/download/file_b", server.uri()),
                        "file_id": "file_b",
                        "mime_type": "text/csv",
                        "file_name": "second.csv",
                        "uri": "sediment://file_b",
                        "file_size_bytes": 6,
                    }
                ]
            })
        );
        let requests = server
            .received_requests()
            .await
            .expect("captured upload requests");
        assert_eq!(requests.len(), 7);
        assert_eq!(
            requests
                .into_iter()
                .map(|request| request.url.path().to_string())
                .collect::<Vec<_>>(),
            vec![
                "/backend-api/files".to_string(),
                "/upload/file_a".to_string(),
                "/backend-api/files/file_a/uploaded".to_string(),
                "/backend-api/files".to_string(),
                "/backend-api/files".to_string(),
                "/upload/file_b".to_string(),
                "/backend-api/files/file_b/uploaded".to_string(),
            ]
        );
        assert_eq!(
            authority.recorded_boundaries_for_test(),
            vec![
                crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall,
                crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall,
            ]
        );
        assert!(authority.runtime_snapshot().await.active);
    }

    #[tokio::test]
    async fn rewrite_mcp_tool_arguments_for_openai_files_uses_runtime_admission_for_upload_auth() {
        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::header;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .and(header("chatgpt-account-id", "pooled-account"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_lease",
                "upload_url": format!("{}/upload/file_lease", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_lease"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_lease/uploaded"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_lease", server.uri()),
                "file_name": "file_report.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 5,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (mut session, mut turn_context) = make_session_and_context().await;
        let authority =
            crate::runtime_lease::RuntimeLeaseAuthority::for_test_accepting("pooled-account", 7);
        install_runtime_lease_authority(&mut session, authority.clone()).await;
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);

        let rewritten = rewrite_mcp_tool_arguments_for_openai_files(
            &session,
            &turn_context,
            Some(serde_json::json!({
                "file": "file_report.csv",
            })),
            Some(&["file".to_string()]),
            CancellationToken::new(),
        )
        .await
        .expect("rewrite should upload with leased auth")
        .expect("rewritten arguments");

        assert_eq!(
            rewritten,
            serde_json::json!({
                "file": {
                    "download_url": format!("{}/download/file_lease", server.uri()),
                    "file_id": "file_lease",
                    "mime_type": "text/csv",
                    "file_name": "file_report.csv",
                    "uri": "sediment://file_lease",
                    "file_size_bytes": 5,
                }
            })
        );
        assert_eq!(
            authority.recorded_boundaries_for_test(),
            vec![crate::runtime_lease::RequestBoundaryKind::BackgroundModelCall]
        );
    }

    #[tokio::test]
    async fn build_uploaded_local_argument_value_uses_agent_assertion_for_agent_identity_auth() {
        use wiremock::Mock;
        use wiremock::MockServer;
        use wiremock::ResponseTemplate;
        use wiremock::matchers::body_json;
        use wiremock::matchers::header_regex;
        use wiremock::matchers::method;
        use wiremock::matchers::path;

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/backend-api/v1/agent/agent-123/task/register"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "task_id": "task-123",
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files"))
            .and(header_regex("authorization", r"^AgentAssertion .+"))
            .and(body_json(serde_json::json!({
                "file_name": "file_report.csv",
                "file_size": 5,
                "use_case": "codex",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "file_id": "file_123",
                "upload_url": format!("{}/upload/file_123", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/upload/file_123"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/backend-api/files/file_123/uploaded"))
            .and(header_regex("authorization", r"^AgentAssertion .+"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "download_url": format!("{}/download/file_123", server.uri()),
                "file_name": "file_report.csv",
                "mime_type": "text/csv",
                "file_size_bytes": 5,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let (session, mut turn_context) = make_session_and_context().await;
        let dir = tempdir().expect("temp dir");
        let local_path = dir.path().join("file_report.csv");
        tokio::fs::write(&local_path, b"hello")
            .await
            .expect("write local file");
        turn_context.cwd = AbsolutePathBuf::try_from(dir.path()).expect("absolute path");

        let mut config = (*turn_context.config).clone();
        config.chatgpt_base_url = format!("{}/backend-api", server.uri());
        turn_context.config = Arc::new(config);
        let auth = make_agent_identity_auth(&turn_context.config.chatgpt_base_url).await;

        let rewritten = build_uploaded_local_argument_value(
            &session,
            &turn_context,
            Some(&auth),
            &CancellationToken::new(),
            "file",
            /*index*/ None,
            "file_report.csv",
            &mut HashMap::new(),
        )
        .await
        .expect("rewrite should upload the local file");

        assert_eq!(
            rewritten,
            serde_json::json!({
                "download_url": format!("{}/download/file_123", server.uri()),
                "file_id": "file_123",
                "mime_type": "text/csv",
                "file_name": "file_report.csv",
                "uri": "sediment://file_123",
                "file_size_bytes": 5,
            })
        );
    }
}
