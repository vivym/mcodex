use super::host::RuntimeLeaseHost;
use super::host::RuntimeLeaseHostId;
use super::host::RuntimeLeaseHostMode;
use pretty_assertions::assert_eq;

#[test]
fn pooled_host_id_is_stable_for_one_runtime() {
    let host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-a".to_string()));

    assert_eq!(host.id(), RuntimeLeaseHostId::new("runtime-a".to_string()));
    assert_eq!(host.mode(), RuntimeLeaseHostMode::Pooled);
}

#[test]
fn non_pooled_host_never_reports_pooled_authority() {
    let host =
        RuntimeLeaseHost::non_pooled_for_test(RuntimeLeaseHostId::new("runtime-a".to_string()));

    assert_eq!(host.mode(), RuntimeLeaseHostMode::NonPooled);
    assert!(host.authority_for_test().is_none());
}

#[test]
fn cloned_host_shares_one_runtime_handle() {
    let host = RuntimeLeaseHost::pooled_for_test(RuntimeLeaseHostId::new("runtime-a".to_string()));
    let cloned = host.clone();

    assert!(host.ptr_eq_for_test(&cloned));
}
