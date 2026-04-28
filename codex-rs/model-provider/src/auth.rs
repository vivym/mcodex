use std::sync::Arc;

use codex_agent_identity::AgentIdentityKey;
use codex_agent_identity::AgentTaskAuthorizationTarget;
use codex_agent_identity::authorization_header_for_agent_task;
use codex_api::AuthProvider;
use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use http::HeaderMap;
use http::HeaderValue;

use crate::bearer_auth_provider::AuthorizationHeaderAuthProvider;
use crate::bearer_auth_provider::BearerAuthProvider;

#[derive(Clone, Debug)]
struct AgentIdentityAuthProvider {
    auth: codex_login::auth::AgentIdentityAuth,
}

impl AuthProvider for AgentIdentityAuthProvider {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        let record = self.auth.record();
        let header_value = self
            .auth
            .process_task_id()
            .ok_or_else(|| std::io::Error::other("agent identity process task is not initialized"))
            .and_then(|task_id| {
                authorization_header_for_agent_task(
                    AgentIdentityKey {
                        agent_runtime_id: &record.agent_runtime_id,
                        private_key_pkcs8_base64: &record.agent_private_key,
                    },
                    AgentTaskAuthorizationTarget {
                        agent_runtime_id: &record.agent_runtime_id,
                        task_id,
                    },
                )
                .map_err(std::io::Error::other)
            });

        if let Ok(header_value) = header_value
            && let Ok(header) = HeaderValue::from_str(&header_value)
        {
            let _ = headers.insert(http::header::AUTHORIZATION, header);
        }

        if let Ok(header) = HeaderValue::from_str(self.auth.account_id()) {
            let _ = headers.insert("ChatGPT-Account-ID", header);
        }

        if self.auth.is_fedramp_account() {
            let _ = headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        }
    }
}

// Some providers are meant to send no auth headers. Examples include local OSS
// providers and custom test providers with `requires_openai_auth = false`.
#[derive(Clone, Debug)]
struct UnauthenticatedAuthProvider;

impl AuthProvider for UnauthenticatedAuthProvider {
    fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
}

pub fn unauthenticated_auth_provider() -> SharedAuthProvider {
    Arc::new(UnauthenticatedAuthProvider)
}

/// Returns the provider-scoped auth manager when this provider uses command-backed auth.
///
/// Providers without custom auth continue using the caller-supplied base manager, when present.
pub(crate) fn auth_manager_for_provider(
    auth_manager: Option<Arc<AuthManager>>,
    provider: &ModelProviderInfo,
) -> Option<Arc<AuthManager>> {
    match provider.auth.clone() {
        Some(config) => Some(AuthManager::external_bearer_only(config)),
        None => auth_manager,
    }
}

pub(crate) fn resolve_provider_auth(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<SharedAuthProvider> {
    if let Some(auth) = bearer_auth_for_provider(provider)? {
        return Ok(Arc::new(auth));
    }

    if let Some(auth_provider) = agent_identity_auth_provider_for_model_requests(auth)? {
        return Ok(Arc::new(auth_provider));
    }

    Ok(match auth {
        Some(auth) => auth_provider_from_auth(auth),
        None => unauthenticated_auth_provider(),
    })
}

fn bearer_auth_for_provider(
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<Option<BearerAuthProvider>> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(Some(BearerAuthProvider::new(api_key)));
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(Some(BearerAuthProvider::new(token)));
    }

    Ok(None)
}

fn agent_identity_auth_provider_for_model_requests(
    auth: Option<&CodexAuth>,
) -> codex_protocol::error::Result<Option<AuthorizationHeaderAuthProvider>> {
    let Some(auth) = auth else {
        return Ok(None);
    };
    let Some(authorization_header_value) = auth.agent_identity_authorization_header()? else {
        return Ok(None);
    };

    let mut auth_provider =
        AuthorizationHeaderAuthProvider::new(Some(authorization_header_value), None);
    if auth.is_fedramp_account() {
        auth_provider = auth_provider.with_fedramp_routing_header();
    }
    Ok(Some(auth_provider))
}

fn bearer_auth_for_provider_with_account_override(
    provider: &ModelProviderInfo,
    account_id: String,
) -> codex_protocol::error::Result<Option<BearerAuthProvider>> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(Some(BearerAuthProvider {
            token: Some(api_key),
            account_id: Some(account_id),
            is_fedramp_account: false,
        }));
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(Some(BearerAuthProvider {
            token: Some(token),
            account_id: Some(account_id),
            is_fedramp_account: false,
        }));
    }

    Ok(None)
}

/// Builds request-header auth for a first-party Codex auth snapshot.
pub fn auth_provider_from_auth(auth: &CodexAuth) -> SharedAuthProvider {
    match auth {
        CodexAuth::AgentIdentity(auth) => {
            Arc::new(AgentIdentityAuthProvider { auth: auth.clone() })
        }
        CodexAuth::ApiKey(_) | CodexAuth::Chatgpt(_) | CodexAuth::ChatgptAuthTokens(_) => {
            Arc::new(BearerAuthProvider {
                token: auth.get_token().ok(),
                account_id: auth.get_account_id(),
                is_fedramp_account: auth.is_fedramp_account(),
            })
        }
    }
}

pub fn resolve_provider_auth_with_account_override(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
    account_id: String,
) -> codex_protocol::error::Result<SharedAuthProvider> {
    if let Some(auth_provider) = agent_identity_auth_provider_for_model_requests(auth)? {
        return Ok(Arc::new(auth_provider));
    }

    if let Some(auth) =
        bearer_auth_for_provider_with_account_override(provider, account_id.clone())?
    {
        return Ok(Arc::new(auth));
    }

    Ok(match auth {
        Some(auth) => Arc::new(BearerAuthProvider {
            token: auth.get_token().ok(),
            account_id: Some(account_id),
            is_fedramp_account: auth.is_fedramp_account(),
        }),
        None => Arc::new(BearerAuthProvider {
            token: None,
            account_id: Some(account_id),
            is_fedramp_account: false,
        }),
    })
}

#[cfg(test)]
mod tests {
    use codex_model_provider_info::WireApi;
    use codex_model_provider_info::create_oss_provider_with_base_url;

    use super::*;

    #[test]
    fn unauthenticated_auth_provider_adds_no_headers() {
        let provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        let auth = resolve_provider_auth(/*auth*/ None, &provider).expect("auth should resolve");

        assert!(auth.to_auth_headers().is_empty());
    }
}
