// Single integration test binary that aggregates all test modules.
// The submodules live in `tests/suite/`.
mod suite;

#[tokio::test]
async fn auth_seams_legacy_auth_view_reads_auth_manager_snapshot() {
    suite::auth_seams::legacy_auth_view_reads_auth_manager_snapshot().await;
}

#[tokio::test]
async fn auth_seams_leased_turn_auth_does_not_read_shared_auth_manager() {
    suite::auth_seams::leased_turn_auth_does_not_read_shared_auth_manager().await;
}

#[tokio::test]
async fn auth_seams_pooled_registration_returns_tokens_without_writing_shared_auth() {
    suite::auth_seams::pooled_registration_browser_returns_tokens_without_writing_shared_auth()
        .await;
}

#[tokio::test]
async fn auth_seams_local_lease_scoped_session_refresh_fails_closed_on_account_rebind() {
    suite::auth_seams::local_lease_scoped_session_refresh_fails_closed_on_account_rebind()
        .await
        .expect("expected rebind failure test to pass");
}

#[tokio::test]
async fn pooled_registration_browser_returns_tokens_without_writing_shared_auth() {
    suite::pooled_registration::pooled_browser_registration_returns_tokens_without_writing_shared_auth()
        .await;
}
