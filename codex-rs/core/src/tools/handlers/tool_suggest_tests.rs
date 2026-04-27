use super::*;
use crate::function_tool::FunctionCallError;
use crate::plugins::PluginInstallRequest;
use crate::plugins::PluginsManager;
use crate::plugins::test_support::load_plugins_config;
use crate::plugins::test_support::write_curated_plugin_sha;
use crate::plugins::test_support::write_openai_curated_marketplace;
use crate::plugins::test_support::write_plugins_feature_config;
use crate::session::session::Session;
use crate::session::tests::make_session_and_context;
use crate::session::turn_context::TurnContext;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::turn_diff_tracker::TurnDiffTracker;
use base64::Engine;
use codex_config::types::ToolSuggestDiscoverable;
use codex_config::types::ToolSuggestDiscoverableType;
use codex_features::Feature;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::LeasedTurnAuth;
use codex_login::auth::LeaseAuthBinding;
use codex_login::auth::LeaseScopedAuthSession;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Serialize;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::Mutex;

const DISCOVERABLE_GMAIL_ID: &str = "connector_68df038e0ba48191908c8434991bbac2";

struct TestLeaseScopedAuthSession {
    binding: LeaseAuthBinding,
}

impl TestLeaseScopedAuthSession {
    fn new(account_id: &str) -> Self {
        Self {
            binding: LeaseAuthBinding {
                account_id: account_id.to_string(),
                backend_account_handle: format!("handle-{account_id}"),
                lease_epoch: 1,
            },
        }
    }
}

impl LeaseScopedAuthSession for TestLeaseScopedAuthSession {
    fn leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> {
        self.refresh_leased_turn_auth()
    }

    fn refresh_leased_turn_auth(&self) -> anyhow::Result<LeasedTurnAuth> {
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

fn fake_access_token(chatgpt_account_id: &str) -> String {
    #[derive(Serialize)]
    struct Header {
        alg: &'static str,
        typ: &'static str,
    }

    let header = Header {
        alg: "none",
        typ: "JWT",
    };
    let payload = serde_json::json!({
        "email": "user@example.com",
        "email_verified": true,
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro",
            "chatgpt_user_id": "user-12345",
            "chatgpt_account_id": chatgpt_account_id,
        },
    });
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 = b64(&serde_json::to_vec(&header).unwrap_or_else(|err| {
        panic!("serialize header: {err}");
    }));
    let payload_b64 = b64(&serde_json::to_vec(&payload).unwrap_or_else(|err| {
        panic!("serialize payload: {err}");
    }));
    let signature_b64 = b64(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}

fn function_invocation(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    arguments: serde_json::Value,
) -> ToolInvocation {
    ToolInvocation {
        session,
        turn,
        tracker: Arc::new(Mutex::new(TurnDiffTracker::default())),
        call_id: "call-1".to_string(),
        tool_name: TOOL_SUGGEST_TOOL_NAME.into(),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}

#[tokio::test]
async fn verified_plugin_suggestion_completed_requires_installed_plugin() {
    let codex_home = tempdir().expect("tempdir should succeed");
    let curated_root = crate::plugins::curated_plugins_repo_path(codex_home.path());
    write_openai_curated_marketplace(&curated_root, &["sample"]);
    write_curated_plugin_sha(codex_home.path());
    write_plugins_feature_config(codex_home.path());

    let config = load_plugins_config(codex_home.path()).await;
    let plugins_manager = PluginsManager::new(codex_home.path().to_path_buf());

    assert!(!verified_plugin_suggestion_completed(
        "sample@openai-curated",
        &config,
        &plugins_manager,
    ));

    plugins_manager
        .install_plugin(PluginInstallRequest {
            plugin_name: "sample".to_string(),
            marketplace_path: AbsolutePathBuf::try_from(
                curated_root.join(".agents/plugins/marketplace.json"),
            )
            .expect("marketplace path"),
        })
        .await
        .expect("plugin should install");

    let refreshed_config = load_plugins_config(codex_home.path()).await;
    assert!(verified_plugin_suggestion_completed(
        "sample@openai-curated",
        &refreshed_config,
        &plugins_manager,
    ));
}

#[tokio::test]
async fn tool_suggest_handler_uses_leased_auth_for_discoverable_connector_lookup() {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/connectors/directory/list"))
        .and(header("chatgpt-account-id", "pooled-account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "apps": [{
                "id": DISCOVERABLE_GMAIL_ID,
                "name": "Gmail",
                "description": "Find and summarize email threads.",
            }],
            "nextToken": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let (mut session, mut turn_context) = make_session_and_context().await;
    session.services.auth_manager =
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    session
        .services
        .lease_auth
        .replace_current(Some(Arc::new(TestLeaseScopedAuthSession::new(
            "pooled-account",
        ))));

    let mut config = (*turn_context.config).clone();
    config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow Apps");
    config
        .features
        .enable(Feature::Plugins)
        .expect("test config should allow Plugins");
    config
        .features
        .enable(Feature::ToolSuggest)
        .expect("test config should allow ToolSuggest");
    config.chatgpt_base_url = server.uri();
    config.tool_suggest.discoverables = vec![ToolSuggestDiscoverable {
        kind: ToolSuggestDiscoverableType::Connector,
        id: DISCOVERABLE_GMAIL_ID.to_string(),
    }];
    turn_context.config = Arc::new(config);

    let error = match ToolSuggestHandler
        .handle(function_invocation(
            Arc::new(session),
            Arc::new(turn_context),
            serde_json::json!({
                "tool_type": "connector",
                "action_type": "install",
                "tool_id": "connector_missing",
                "suggest_reason": "need gmail",
            }),
        ))
        .await
    {
        Ok(_) => panic!("missing tool should be rejected after discoverable lookup"),
        Err(error) => error,
    };

    let FunctionCallError::RespondToModel(message) = error else {
        panic!("expected model-visible validation error");
    };
    assert!(message.contains("tool_id must match"));
}
