use codex_account_pool::AccountKind;
use codex_account_pool::AccountPoolBackend;
use codex_account_pool::AccountRecord;
use codex_account_pool::ContextReuseDecision;
use codex_account_pool::ContextReuseRequest;
use codex_account_pool::QuotaFamilyView;
use codex_account_pool::SelectionRequest;
use codex_account_pool::evaluate_context_reuse;
use codex_account_pool::select_startup_account;
use pretty_assertions::assert_eq;

#[test]
fn policy_default_selection_prefers_healthy_account_and_rejects_mixed_kind_auto_rotation() {
    let pool = TestPool::homogeneous_chatgpt();
    let selection = select_startup_account(&pool, SelectionRequest::default()).unwrap();
    assert_eq!(selection.account_id, "acct-2");

    let mixed = TestPool::mixed_kind_manual_only();
    assert!(select_startup_account(&mixed, SelectionRequest::default()).is_err());
}

#[test]
fn policy_context_reuse_requires_consent_and_portable_transport() {
    let decision = evaluate_context_reuse(ContextReuseRequest {
        allow_context_reuse: true,
        explicit_context_reuse_consent: false,
        same_workspace: true,
        same_backend_family: true,
        transport_portable: true,
    });

    assert_eq!(decision, ContextReuseDecision::ResetRemoteContext);
}

struct TestPool {
    accounts: Vec<AccountRecord>,
}

impl TestPool {
    fn homogeneous_chatgpt() -> Self {
        Self {
            accounts: vec![
                AccountRecord {
                    account_id: "acct-1".to_string(),
                    healthy: false,
                    kind: AccountKind::ChatGpt,
                    enabled: true,
                    selector_auth_eligible: false,
                    pool_position: 0,
                    leased_to_other_holder: false,
                    quota: QuotaFamilyView::default(),
                },
                AccountRecord {
                    account_id: "acct-2".to_string(),
                    healthy: true,
                    kind: AccountKind::ChatGpt,
                    enabled: true,
                    selector_auth_eligible: true,
                    pool_position: 1,
                    leased_to_other_holder: false,
                    quota: QuotaFamilyView::default(),
                },
            ],
        }
    }

    fn mixed_kind_manual_only() -> Self {
        Self {
            accounts: vec![
                AccountRecord {
                    account_id: "acct-1".to_string(),
                    healthy: true,
                    kind: AccountKind::ChatGpt,
                    enabled: true,
                    selector_auth_eligible: true,
                    pool_position: 0,
                    leased_to_other_holder: false,
                    quota: QuotaFamilyView::default(),
                },
                AccountRecord {
                    account_id: "acct-2".to_string(),
                    healthy: true,
                    kind: AccountKind::ManualOnly,
                    enabled: true,
                    selector_auth_eligible: true,
                    pool_position: 1,
                    leased_to_other_holder: false,
                    quota: QuotaFamilyView::default(),
                },
            ],
        }
    }
}

impl AccountPoolBackend for TestPool {
    fn accounts(&self) -> &[AccountRecord] {
        &self.accounts
    }
}
