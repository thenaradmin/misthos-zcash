//! Real Zcash testnet tests. Run with `cargo test --test testnet -- --ignored`.
//! Every test panics naming the first missing env var; none are skipped silently.
//!
//! Needs: ZCASH_NETWORK=testnet, ZCASH_LWD_URL, ZCASH_WALLET_DB, ZCASH_WALLET_CLI,
//! and for the payout test also PAYROLL_TEST_CSV (3 testnet UAs, tiny amounts).

use zec_payroll::{config, csv, wallet, zip321, CapPolicy, Engine, RunStatus};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

fn need<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
    r.unwrap_or_else(|e| panic!("TESTNET TEST CANNOT RUN: {e}"))
}

#[test]
#[ignore = "hits real testnet lightwalletd; run with --ignored"]
fn lightwalletd_reports_chain_height() {
    let cfg = need(config::load_with(env));
    let bin = need(wallet::wallet_cli(env));
    let out = need(wallet::chain_height(&bin, &cfg));
    assert!(out.chars().any(|c| c.is_ascii_digit()), "no height in wallet output: {out}");
}

#[test]
#[ignore = "spends testnet ZEC; run with --ignored"]
fn sends_one_shielded_tx_to_all_csv_recipients() {
    let cfg = need(config::load_with(env));
    let bin = need(wallet::wallet_cli(env));
    let path = env("PAYROLL_TEST_CSV").unwrap_or_else(|| panic!("TESTNET TEST CANNOT RUN: PAYROLL_TEST_CSV not configured"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let batch = csv::parse_payroll_csv(&text).unwrap_or_else(|e| panic!("{path}: {e:?}"));

    let mut engine = Engine::new(CapPolicy::default());
    let id = engine.plan_adhoc("testnet-it", batch).expect("plan");
    engine.approve(id).expect("approve");
    zip321::payout_uri(engine.run(id).unwrap()).expect("uri");
    let lines = engine.run(id).unwrap().lines.clone();
    let txids = need(wallet::send_payout(&bin, &cfg, &lines));
    let txid = &txids[0];
    engine.mark_broadcast(id, txid).expect("mark");
    assert!(matches!(engine.run(id).unwrap().status, RunStatus::Broadcast { .. }));
    println!("broadcast txids={txids:?}");
}
