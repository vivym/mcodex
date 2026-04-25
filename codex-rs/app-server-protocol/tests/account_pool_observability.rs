use anyhow::Context;
use anyhow::Result;
use codex_app_server_protocol::AccountPoolAccountResponse;
use codex_app_server_protocol::AccountPoolBackendKind;
use codex_app_server_protocol::AccountPoolPolicyResponse;
use codex_app_server_protocol::AccountPoolQuotaFamilyResponse;
use codex_app_server_protocol::AccountPoolQuotaResponse;
use codex_app_server_protocol::AccountPoolQuotaWindowResponse;
use codex_app_server_protocol::AccountPoolReadParams;
use codex_app_server_protocol::AccountPoolReadResponse;
use codex_app_server_protocol::AccountPoolSelectionResponse;
use codex_app_server_protocol::AccountPoolSummaryResponse;
use codex_app_server_protocol::generate_json_with_experimental;
use codex_app_server_protocol::generate_ts;
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
fn account_pool_account_response_serializes_quota_families() {
    let response = AccountPoolAccountResponse {
        account_id: "acct-a".into(),
        quota: Some(AccountPoolQuotaResponse {
            remaining_percent: Some(18.0),
            resets_at: Some(1_710_000_000),
            observed_at: 1_709_999_000,
        }),
        quotas: vec![quota_family("chatgpt", 72.0), quota_family("codex", 82.0)],
        ..account_row_fixture()
    };

    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["quotas"][0]["limitId"], "chatgpt");
    assert!(value["quotas"][0]["primary"].is_object());
    assert!(value["quotas"][0]["secondary"].is_object());
    assert!(value["quotas"][0].get("exhaustedWindows").is_some());
    assert!(value["quotas"][0].get("predictedBlockedUntil").is_some());
    assert!(value["quotas"][0].get("nextProbeAfter").is_some());
    assert!(value["quotas"][0].get("observedAt").is_some());
    assert_eq!(value["quota"]["remainingPercent"], 18.0);
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
    assert_required_non_null_array_property(
        &accounts_list_response,
        &["definitions", "AccountPoolAccountResponse"],
        "quotas",
    );

    let read_response = read_json(temp_dir.path().join("v2/AccountPoolReadResponse.json"))?;
    assert_required_nullable_property(
        &read_response,
        &["definitions", "AccountPoolSummaryResponse"],
        "pausedAccounts",
    );

    Ok(())
}

#[test]
fn generated_account_pool_accounts_list_params_account_id_is_optional_nullable() -> Result<()> {
    let temp_dir = tempfile::tempdir().context("create temp dir")?;
    generate_ts(temp_dir.path(), None)?;

    let contents = fs::read_to_string(temp_dir.path().join("v2/AccountPoolAccountsListParams.ts"))
        .context("read generated AccountPoolAccountsListParams.ts")?;
    assert!(
        contents.contains("accountId?: string | null"),
        "expected accountId optional nullable field, got:\n{contents}"
    );

    Ok(())
}

fn read_json(path: impl AsRef<Path>) -> Result<Value> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn assert_required_non_null_array_property(schema: &Value, path: &[&str], property_name: &str) {
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
        !schema_allows_null(property_schema),
        "expected {property_name} not to allow null at path {path:?}, got {property_schema:?}"
    );
    assert!(
        schema_is_array(property_schema),
        "expected {property_name} to be an array at path {path:?}, got {property_schema:?}"
    );
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

fn schema_is_array(schema: &Value) -> bool {
    match schema {
        Value::Object(map) => {
            if let Some(Value::String(kind)) = map.get("type")
                && kind == "array"
            {
                return true;
            }
            for union_key in ["anyOf", "oneOf"] {
                if let Some(Value::Array(variants)) = map.get(union_key)
                    && variants.iter().any(schema_is_array)
                {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
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

fn account_row_fixture() -> AccountPoolAccountResponse {
    AccountPoolAccountResponse {
        account_id: "acct-fixture".to_string(),
        backend_account_ref: Some("backend-fixture".to_string()),
        account_kind: "chatgpt".to_string(),
        enabled: true,
        health_state: Some("healthy".to_string()),
        operational_state: None,
        allocatable: None,
        status_reason_code: None,
        status_message: None,
        current_lease: None,
        quota: None,
        quotas: Vec::new(),
        selection: Some(AccountPoolSelectionResponse {
            eligible: true,
            next_eligible_at: None,
            preferred: false,
            suppressed: false,
        }),
        updated_at: 1_709_999_000,
    }
}

fn quota_family(limit_id: &str, used_percent: f64) -> AccountPoolQuotaFamilyResponse {
    AccountPoolQuotaFamilyResponse {
        limit_id: limit_id.to_string(),
        primary: AccountPoolQuotaWindowResponse {
            used_percent: Some(used_percent),
            resets_at: Some(1_710_000_000),
        },
        secondary: AccountPoolQuotaWindowResponse {
            used_percent: None,
            resets_at: None,
        },
        exhausted_windows: "primary".to_string(),
        predicted_blocked_until: Some(1_710_000_000),
        next_probe_after: Some(1_709_999_500),
        observed_at: 1_709_999_000,
    }
}
