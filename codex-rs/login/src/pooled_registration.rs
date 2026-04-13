use crate::device_code_auth::run_pooled_device_code_registration as run_device_code_registration;
use crate::server::ServerOptions;
use crate::server::run_browser_login_for_registration;
use age::secrecy::SecretString;

/// Provider-level ChatGPT tokens captured for pooled registration.
#[derive(Clone, Debug)]
pub struct ChatgptManagedRegistrationTokens {
    pub id_token: String,
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub account_id: String,
}

pub async fn run_pooled_browser_registration(
    opts: ServerOptions,
) -> std::io::Result<ChatgptManagedRegistrationTokens> {
    run_browser_login_for_registration(opts).await
}

pub async fn run_pooled_device_code_registration(
    opts: ServerOptions,
) -> std::io::Result<ChatgptManagedRegistrationTokens> {
    run_device_code_registration(opts).await
}
