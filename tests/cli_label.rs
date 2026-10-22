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

/// Two concurrent `pay --two-step` runs on one wallet DB: the second must stop on the lock
/// before it reads a ledger or calls ZCASH_WALLET_CLI.
#[test]
fn concurrent_two_step_run_stops_on_the_wallet_db_lock() {
    let d = std::env::temp_dir().join(format!("zpp-cli-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let ds = d.to_str().unwrap().to_string();
    let marker = d.join("wallet-cli-was-called");
    let cli = d.join("fake-wallet-cli.sh");
    std::fs::write(&cli, format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display())).unwrap();
    std::fs::set_permissions(&cli, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let held = zec_payroll::wallet::ledger::lock_wallet_db(&ds).unwrap(); // the "first run"
    let o = Command::new(env!("CARGO_BIN_EXE_payroll"))
        .args(["pay", "examples/bad.csv", "--two-step", "--label", "oct-2026"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("ZCASH_NETWORK", "testnet")
        .env("ZCASH_LWD_URL", "https://127.0.0.1:9")
        .env("ZCASH_WALLET_DB", &ds)
        .env("ZCASH_WALLET_CLI", &cli)
        .output()
        .unwrap();
    drop(held);
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("another pay --two-step is running for this wallet DB") && e.contains("nothing sent"), "{e}");
    assert!(!marker.exists(), "ZCASH_WALLET_CLI ran while another run held the lock");
    std::fs::remove_dir_all(&d).ok();
}
