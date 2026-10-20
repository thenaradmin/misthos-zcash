//! `pay --two-step` and `export-proof` must not default the period to the CSV path.
use std::process::Command;

fn payroll(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_payroll")).args(args).env_clear().env("PATH", std::env::var("PATH").unwrap()).output().unwrap()
}

#[test]
fn two_step_pay_requires_label() {
    let o = payroll(&["pay", "examples/bad.csv", "--two-step"]);
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("needs an explicit --label"), "{e}");
}

#[test]
fn export_proof_requires_label() {
    let o = payroll(&["export-proof", "examples/bad.csv", "--employee", "emp-a"]);
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("needs --label"), "{e}");
}
