//! Integration smoke test for host spec collection.

use clawz_setup::HostSpecChecker;

#[test]
fn collect_returns_report_with_os_set() {
    let report = HostSpecChecker::collect();
    assert!(!report.os.is_empty(), "os should be populated");
    assert!(!report.arch.is_empty(), "arch should be populated");
    assert!(!report.summary.is_empty(), "summary should be populated");
}
