use anyhow::Context;
use anyhow::Result;
use codex_app_server_protocol::AccountPoolBackendKind;
use codex_app_server_protocol::AccountPoolPolicyResponse;
use codex_app_server_protocol::AccountPoolReadParams;
use codex_app_server_protocol::AccountPoolReadResponse;
use codex_app_server_protocol::AccountPoolSummaryResponse;
use codex_app_server_protocol::generate_json_with_experimental;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::fs;
use std::path::Path;

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

#[test]
fn generated_account_pool_response_schema_requires_nullable_fields() -> Result<()> {
    let temp_dir = tempfile::tempdir().context("create temp dir")?;
    generate_json_with_experimental(temp_dir.path(), /*experimental_api*/ false)?;

    let accounts_list_response = read_json(
        temp_dir
            .path()
            .join("v2/AccountPoolAccountsListResponse.json"),
    )?;
    assert_required_nullable_property(&accounts_list_response, &[], "nextCursor");
    assert_required_nullable_property(
        &accounts_list_response,
        &["definitions", "AccountPoolAccountResponse"],
        "backendAccountRef",
    );

    let read_response = read_json(temp_dir.path().join("v2/AccountPoolReadResponse.json"))?;
    assert_required_nullable_property(
        &read_response,
        &["definitions", "AccountPoolSummaryResponse"],
        "pausedAccounts",
    );

    Ok(())
}

fn read_json(path: impl AsRef<Path>) -> Result<Value> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn assert_required_nullable_property(schema: &Value, path: &[&str], property_name: &str) {
    let object_schema = path.iter().fold(schema, |value, segment| &value[*segment]);
    let Some(required) = object_schema["required"].as_array() else {
        panic!("expected required array at path {path:?}, got {object_schema:?}");
    };
    assert!(
        required.iter().any(|value| value == property_name),
        "expected {property_name} to be required at path {path:?}, got {required:?}"
    );

    let property_schema = &object_schema["properties"][property_name];
    assert!(
        schema_allows_null(property_schema),
        "expected {property_name} to allow null at path {path:?}, got {property_schema:?}"
    );
}

fn schema_allows_null(schema: &Value) -> bool {
    match schema {
        Value::Object(map) => {
            if let Some(Value::String(kind)) = map.get("type") {
                return kind == "null";
            }
            if let Some(Value::Array(types)) = map.get("type")
                && types.iter().any(|value| value == "null")
            {
                return true;
            }
            for union_key in ["anyOf", "oneOf"] {
                if let Some(Value::Array(variants)) = map.get(union_key)
                    && variants.iter().any(schema_allows_null)
                {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}
