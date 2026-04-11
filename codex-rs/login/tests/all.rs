// Single integration test binary that aggregates all test modules.
// The submodules live in `tests/suite/`.
mod suite;

#[tokio::test]
async fn legacy_auth_view_reads_auth_manager_snapshot() {
    suite::auth_seams::legacy_auth_view_reads_auth_manager_snapshot().await;
}

#[test]
fn leased_turn_auth_does_not_read_shared_auth_manager() {
    suite::auth_seams::leased_turn_auth_does_not_read_shared_auth_manager();
}
