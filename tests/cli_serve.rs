//! `payroll serve` must not silently expose the unauthenticated desk on a non-loopback address.
use std::process::Command;

#[test]
fn non_loopback_addr_is_refused_without_allow_remote() {
    let roster = concat!(env!("CARGO_MANIFEST_DIR"), "/e2e/fixtures/roster.csv");
    let o = Command::new(env!("CARGO_BIN_EXE_payroll"))
        .args(["serve", "--addr", "0.0.0.0:0"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("PAYROLL_ROSTER_CSV", roster)
        .output()
        .unwrap();
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("refusing to serve on non-loopback 0.0.0.0:0") && e.contains("--allow-remote"), "{e}");
}
