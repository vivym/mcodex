use std::sync::Arc;

use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;

use crate::bearer_auth_provider::AuthorizationHeaderAuthProvider;
use crate::bearer_auth_provider::BearerAuthProvider;

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

fn bearer_auth_provider_from_auth(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<BearerAuthProvider> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(BearerAuthProvider {
            token: Some(api_key),
            account_id: None,
            is_fedramp_account: false,
        });
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(BearerAuthProvider {
            token: Some(token),
            account_id: None,
            is_fedramp_account: false,
        });
    }

    if let Some(auth) = auth {
        let token = auth.get_token()?;
        Ok(BearerAuthProvider {
            token: Some(token),
            account_id: auth.get_account_id(),
            is_fedramp_account: auth.is_fedramp_account(),
        })
    } else {
        Ok(BearerAuthProvider {
            token: None,
            account_id: None,
            is_fedramp_account: false,
        })
    }
}

fn bearer_auth_provider_from_auth_with_account_override(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
    account_id: String,
) -> codex_protocol::error::Result<BearerAuthProvider> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(BearerAuthProvider {
            token: Some(api_key),
            account_id: Some(account_id),
            is_fedramp_account: false,
        });
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(BearerAuthProvider {
            token: Some(token),
            account_id: Some(account_id),
            is_fedramp_account: false,
        });
    }

    if let Some(auth) = auth {
        let token = auth.get_token()?;
        Ok(BearerAuthProvider {
            token: Some(token),
            account_id: Some(account_id),
            is_fedramp_account: auth.is_fedramp_account(),
        })
    } else {
        Ok(BearerAuthProvider {
            token: None,
            account_id: Some(account_id),
            is_fedramp_account: false,
        })
    }
}

fn agent_identity_auth_provider_from_auth(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<Option<AuthorizationHeaderAuthProvider>> {
    if provider.api_key()?.is_some() || provider.experimental_bearer_token.is_some() {
        return Ok(None);
    }

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

pub fn resolve_provider_auth(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<SharedAuthProvider> {
    if let Some(auth_provider) = agent_identity_auth_provider_from_auth(auth, provider)? {
        return Ok(Arc::new(auth_provider));
    }

    Ok(Arc::new(bearer_auth_provider_from_auth(auth, provider)?))
}

pub fn resolve_provider_auth_with_account_override(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
    account_id: String,
) -> codex_protocol::error::Result<SharedAuthProvider> {
    if let Some(auth_provider) = agent_identity_auth_provider_from_auth(auth, provider)? {
        return Ok(Arc::new(auth_provider));
    }

    Ok(Arc::new(
        bearer_auth_provider_from_auth_with_account_override(auth, provider, account_id)?,
    ))
}
