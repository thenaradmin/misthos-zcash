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

/// The payout delays poll this (gRPC GetLatestBlock, no wallet, no zingo-cli).
#[test]
#[ignore = "hits real testnet lightwalletd; run with --ignored"]
fn lightwalletd_grpc_tip_height_for_delays() {
    let url = env("ZCASH_LWD_URL").filter(|v| !v.trim().is_empty());
    let url = url.unwrap_or_else(|| panic!("TESTNET TEST CANNOT RUN: ZCASH_LWD_URL not configured"));
    let tip = need(zec_payroll::chain::tip_height(&url));
    assert!(tip > 3_000_000, "implausible testnet tip {tip}");
    println!("testnet tip {tip} via {url}");
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

/// `PAYROLL_PAYOUT_A` / `PAYROLL_PAYOUT_B` = `<txid>:<height>:<raw-tx-hex-file>` of real step-2
/// payouts from employee A's and B's child wallets (keyed by PAYROLL_EMPLOYEE_A_UA / _B_UA). None exist yet (no funded wallet), so this panics naming the first missing variable.
#[test]
#[ignore = "needs two real funded testnet payouts; run with --ignored"]
fn testnet_employee_a_key_cannot_see_employee_b_payout() {
    use zec_payroll::disclosure::{self as d, DisclosureError, EmployerSeed};
    let seed = need(EmployerSeed::from_env(env));
    let load = |k: &str| {
        let v = env(k).unwrap_or_else(|| panic!("TESTNET TEST CANNOT RUN: {k} not configured (txid:height:rawhexfile)"));
        let mut it = v.splitn(3, ':');
        let (txid, h, f) = (it.next().unwrap().to_string(), it.next().expect("height"), it.next().expect("file"));
        let raw = hex::decode(std::fs::read_to_string(f).unwrap().trim()).unwrap();
        (txid, h.parse::<u32>().unwrap(), raw)
    };
    let (ta, ha, ra) = load("PAYROLL_PAYOUT_A");
    let (tb, hb, rb) = load("PAYROLL_PAYOUT_B");
    let addr = |k: &str| env(k).unwrap_or_else(|| panic!("TESTNET TEST CANNOT RUN: {k} not configured (employee payout UA)"));
    let ka = need(d::employee_ufvk(&seed, &addr("PAYROLL_EMPLOYEE_A_UA")));
    let kb = need(d::employee_ufvk(&seed, &addr("PAYROLL_EMPLOYEE_B_UA")));
    assert!(need(d::verify_tx(&ka, &ra, Some(&ta), ha)).paid_out_zat() > 0);
    assert!(need(d::verify_tx(&kb, &rb, Some(&tb), hb)).paid_out_zat() > 0);
    assert!(matches!(d::verify_tx(&ka, &rb, Some(&tb), hb), Err(DisclosureError::NothingVisible)));
    assert!(matches!(d::verify_tx(&kb, &ra, Some(&ta), ha), Err(DisclosureError::NothingVisible)));
}
