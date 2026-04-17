use codex_app_server_protocol::AccountPoolBackendKind;
use codex_app_server_protocol::AccountPoolPolicyResponse;
use codex_app_server_protocol::AccountPoolReadParams;
use codex_app_server_protocol::AccountPoolReadResponse;
use codex_app_server_protocol::AccountPoolSummaryResponse;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn account_pool_read_params_serialize_pool_id_in_camel_case() {
    let params = AccountPoolReadParams {
        pool_id: "team-main".to_string(),
    };

    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({"poolId": "team-main"})
    );
}

#[test]
fn account_pool_backend_kind_serializes_as_closed_enum() {
    assert_eq!(
        serde_json::to_value(AccountPoolBackendKind::Local).unwrap(),
        json!("local")
    );
}

#[test]
fn account_pool_read_response_preserves_nullable_summary_fields() {
    let response = AccountPoolReadResponse {
        pool_id: "team-main".to_string(),
        backend: AccountPoolBackendKind::Local,
        summary: AccountPoolSummaryResponse {
            total_accounts: 2,
            active_leases: 1,
            available_accounts: Some(1),
            leased_accounts: Some(1),
            paused_accounts: None,
            draining_accounts: None,
            near_exhausted_accounts: None,
            exhausted_accounts: None,
            error_accounts: None,
        },
        policy: AccountPoolPolicyResponse {
            allocation_mode: "exclusive".to_string(),
            allow_context_reuse: true,
            proactive_switch_threshold_percent: Some(85),
            min_switch_interval_secs: Some(300),
        },
        refreshed_at: 1_710_000_000,
    };

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["summary"]["pausedAccounts"], serde_json::Value::Null);
}
