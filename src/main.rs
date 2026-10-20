//! `payroll` CLI.
//!
//!   payroll plan <file.csv> [--label L] [--max-total ZEC] [--max-per ZEC] [--max-recipients N]
//!       Validate a CSV and print the capped pay run (offline; no chain I/O).
//!   payroll check-env
//!       Verify chain config; exits non-zero naming the first missing var.
//!   payroll pay <file.csv> [same cap flags]
//!       Plan, approve and send ONE shielded testnet tx to every line via ZCASH_WALLET_CLI.
//!   payroll audit-export <file.csv> [txid]
//!       Accountant CSV ledger + ZCASH_AUDITOR_UFVK (view-only; spending keys refused).
//!   payroll pay <file.csv> --two-step [cap flags]
//!       Checks every employee's child wallet syncs, then step 1 (treasury -> child wallets),
//!       then step 2 (each child wallet -> employee). Needs PAYROLL_EMPLOYER_SEED, PAYROLL_CHILD_BIRTHDAY.
//!   payroll export-proof <file.csv> <employee_id|name>
//!       Proof file with that employee's payroll-account UFVK (never the treasury key).
//!   payroll verify-proof <proof-file> <raw-tx-hex-file> <txid> <block-height>
//!       Third-party check: what this employee's key sees in that tx (amount, recipient, memo).
//!   payroll serve [--addr 127.0.0.1:8787]
//!       Employer dashboard for the roster in PAYROLL_ROSTER_CSV.

use std::process::exit;
use zec_payroll::{audit, config, csv, disclosure, money, money::Zat, wallet, zip321, CapPolicy, Engine};

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("ERROR: {msg}");
    exit(1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check-env") => match config::load_from_env() {
            Ok(c) => println!("OK network={} lwd={} wallet_db={}", c.network, c.lwd_url, c.wallet_db),
            Err(e) => die(e),
        },
        Some("plan") => plan(&args[1..]),
        Some("pay") => pay(&args[1..]),
        Some("audit-export") => audit_export(&args[1..]),
        Some("export-proof") => export_proof(&args[1..]),
        Some("verify-proof") => verify_proof(&args[1..]),
        Some("serve") => serve(&args[1..]),
        _ => die("usage: payroll plan|pay <file.csv> [--label L] [--max-total ZEC] [--max-per ZEC] [--max-recipients N] | payroll audit-export <file.csv> [txid] | payroll export-proof <file.csv> <employee_id|name> | payroll verify-proof <proof> <rawtx-hex> <txid> <height> | payroll serve [--addr HOST:PORT] | payroll check-env"),
    }
}

fn pay(args: &[String]) {
    if args.iter().any(|a| a == "--two-step") {
        let rest: Vec<String> = args.iter().filter(|a| *a != "--two-step").cloned().collect();
        return pay_two_step(&rest);
    }
    let cfg = config::load_from_env().unwrap_or_else(|e| die(e));
    let bin = wallet::wallet_cli(|k| std::env::var(k).ok()).unwrap_or_else(|e| die(e));
    let (mut engine, id) = build(args);
    engine.approve(id).unwrap_or_else(|e| die(e));
    let uri = zip321::payout_uri(engine.run(id).expect("planned")).unwrap_or_else(|e| die(e));
    println!("ZIP-321: {uri}");
    let lines = engine.run(id).expect("planned").lines.clone();
    let txids = wallet::send_payout(&bin, &cfg, &lines).unwrap_or_else(|e| die(e));
    engine.mark_broadcast(id, &txids[0]).unwrap_or_else(|e| die(e));
    for t in &txids {
        println!("BROADCAST txid={t}");
    }
}

fn audit_export(args: &[String]) {
    let ufvk = std::env::var("ZCASH_AUDITOR_UFVK").ok().filter(|v| !v.trim().is_empty());
    let ufvk = ufvk.unwrap_or_else(|| die("ZCASH_AUDITOR_UFVK not configured"));
    let txid = args.get(1).cloned();
    let (mut engine, id) = build(&args[..1.min(args.len())]);
    if let Some(t) = txid {
        engine.approve(id).unwrap_or_else(|e| die(e));
        engine.mark_broadcast(id, &t).unwrap_or_else(|e| die(e));
    }
    print!("{}", audit::export_csv(engine.runs(), &ufvk).unwrap_or_else(|e| die(e)));
}

fn serve(args: &[String]) {
    let addr = match args {
        [] => "127.0.0.1:8787".to_string(),
        [f, v] if f == "--addr" => v.clone(),
        _ => die("usage: payroll serve [--addr HOST:PORT]"),
    };
    let path = std::env::var("PAYROLL_ROSTER_CSV").ok().filter(|v| !v.trim().is_empty());
    let path = path.unwrap_or_else(|| die("PAYROLL_ROSTER_CSV not configured"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| die(format!("PAYROLL_ROSTER_CSV {path}: {e}")));
    let roster = csv::parse_payroll_csv(&text).unwrap_or_else(|errs| {
        for e in &errs {
            eprintln!("ERROR: {e}");
        }
        exit(1)
    });
    zec_payroll::server::serve(&addr, path, roster, CapPolicy::default()).unwrap_or_else(|e| die(format!("{addr}: {e}")));
}

fn plan(args: &[String]) {
    let (engine, id) = build(args);
    print_plan(&engine, id);
}

fn build(args: &[String]) -> (Engine, u64) {
    let Some(path) = args.first() else { die("plan: missing <file.csv>") };
    let mut caps = CapPolicy::default();
    let mut label = path.clone();
    let mut it = args[1..].iter();
    while let Some(flag) = it.next() {
        let val = it.next().unwrap_or_else(|| die(format!("{flag} needs a value")));
        let zec = |v: &str| Zat::parse_zec(v).unwrap_or_else(|e| die(format!("{flag}: {e}")));
        match flag.as_str() {
            "--label" => label = val.clone(),
            "--max-total" => caps.max_run_total = Some(zec(val)),
            "--max-per" => caps.max_per_employee = Some(zec(val)),
            "--max-recipients" => {
                caps.max_recipients = val.parse().unwrap_or_else(|_| die(format!("{flag}: not a number")))
            }
            other => die(format!("unknown flag {other}")),
        }
    }
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let batch = csv::parse_payroll_csv(&text).unwrap_or_else(|errs| {
        for e in &errs {
            eprintln!("ERROR: {e}");
        }
        exit(1)
    });
    let mut engine = Engine::new(caps);
    let id = engine.plan_adhoc(&label, batch).unwrap_or_else(|e| die(e));
    (engine, id)
}

fn print_plan(engine: &Engine, id: u64) {
    let run = engine.run(id).expect("just planned");
    println!("Run #{} [{}] status=Draft", run.id, run.period_key);
    for l in &run.lines {
        println!("  {:<20} {:>18}  {}…  memo={:?}", l.name, l.amount.to_string(), &l.address[..16], l.memo);
    }
    println!("  TOTAL {} across {} outputs (one shielded tx, built in M2)", run.total, run.lines.len());
}

fn pay_two_step(args: &[String]) {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let cfg = config::load_from_env().unwrap_or_else(|e| die(e));
    let bin = wallet::wallet_cli(env).unwrap_or_else(|e| die(e));
    let seed = disclosure::EmployerSeed::from_env(env).unwrap_or_else(|e| die(e));
    let birthday: u32 = env("PAYROLL_CHILD_BIRTHDAY")
        .unwrap_or_else(|| die("PAYROLL_CHILD_BIRTHDAY not configured (testnet height to restore child wallets from)"))
        .parse()
        .unwrap_or_else(|_| die("PAYROLL_CHILD_BIRTHDAY: not a number"));
    let wait_secs: u64 = env("PAYROLL_STEP2_TIMEOUT_SECS").and_then(|v| v.parse().ok()).unwrap_or(1800);
    let (mut engine, id) = build(args);
    engine.approve(id).unwrap_or_else(|e| die(e));
    let run = engine.run(id).expect("planned");
    let (period, lines) = (run.period_key.clone(), run.lines.clone());
    disclosure::check_accounts(lines.iter().map(|l| l.address.as_str())).unwrap_or_else(|e| die(e));
    use wallet::ledger::{self, Step};
    let dir_of = |l: &zec_payroll::PayLine| wallet::child_data_dir(&cfg, &l.address);

    // Durable ledger first: a rerun must not re-fund or re-pay, and an ambiguous
    // earlier broadcast (pending with no outcome) stops the whole run.
    let mut states = Vec::new();
    for l in &lines {
        let st = ledger::state(&dir_of(l), &period);
        if matches!(st, Step::FundPending | Step::PayPending) {
            die(format!("{}: an earlier `{period}` broadcast for this child has no recorded outcome ({st:?}); check its wallet on chain and fix {}/payroll-ledger by hand. Nothing sent.", l.employee_id, dir_of(l)));
        }
        states.push(st);
    }

    // Pre-broadcast check: every step-2 wallet must open and sync, or nothing is sent.
    for l in &lines {
        let child = disclosure::child_wallet(&seed, &l.address).unwrap_or_else(|e| die(e));
        wallet::init_child(&bin, &cfg, &child, &l.address, birthday)
            .unwrap_or_else(|e| die(format!("step-2 wallet for {} not ready; step 1 NOT sent: {e}", l.employee_id)));
        println!("READY child wallet {} -> {}", l.employee_id, dir_of(l));
    }

    let to_fund: Vec<_> = lines.iter().zip(&states).filter(|(_, s)| **s == Step::Fresh).map(|(l, _)| l.clone()).collect();
    for (l, s) in lines.iter().zip(&states) {
        if *s != Step::Fresh {
            println!("SKIP FUNDING {} (ledger: {s:?} for {period})", l.employee_id);
        }
    }
    if !to_fund.is_empty() {
        let funding = disclosure::funding_lines(&seed, &to_fund).unwrap_or_else(|e| die(e));
        for f in &funding {
            println!("FUND {} {} -> {}", f.employee_id, f.amount, f.address);
        }
        for l in &to_fund {
            ledger::record(&dir_of(l), "fund-pending", &period, "-").unwrap_or_else(|e| die(format!("ledger: {e}; nothing sent")));
        }
        match wallet::send_payout(&bin, &cfg, &funding) {
            Ok(txids) => {
                for l in &to_fund {
                    ledger::record(&dir_of(l), "funded", &period, &txids.join(",")).unwrap_or_else(|e| die(format!("ledger after broadcast: {e}")));
                }
                for t in &txids {
                    println!("FUNDING BROADCAST txid={t}");
                }
            }
            Err(e) if wallet::failed_before_broadcast(&e) => {
                // Proposal failed: funding never left the treasury, so reset to Fresh.
                clear_fund_pending(&to_fund, &dir_of, &period);
                die(e)
            }
            Err(e) => die(format!("{e}; funding outcome unknown, ledger left at fund-pending so a rerun refuses")),
        }
    }

    // Step 2: retry each funded child's quicksend until spendable or timeout. Paid children are skipped.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
    let mut pending: Vec<_> = lines.iter().filter(|l| ledger::state(&dir_of(l), &period) == Step::Funded).cloned().collect();
    for l in lines.iter().filter(|l| ledger::state(&dir_of(l), &period) == Step::Paid) {
        println!("SKIP PAYOUT {} (already paid for {period})", l.employee_id);
    }
    while !pending.is_empty() {
        let mut ambiguous = Vec::new();
        pending.retain(|l| {
            let d = dir_of(l);
            ledger::record(&d, "pay-pending", &period, "-").unwrap_or_else(|e| die(format!("ledger: {e}")));
            match wallet::send_from_child(&bin, &cfg, l) {
                Ok(t) => {
                    ledger::record(&d, "paid", &period, &t.join(",")).unwrap_or_else(|e| die(format!("ledger after broadcast: {e}")));
                    println!("PAYOUT BROADCAST {} txid={}", l.employee_id, t.join(","));
                    false
                }
                Err(e) if wallet::failed_before_broadcast(&e) => {
                    ledger::record(&d, "pay-failed", &period, "-").ok();
                    eprintln!("step 2 {} not yet spendable: {e}", l.employee_id);
                    true
                }
                Err(e) => {
                    eprintln!("step 2 {}: outcome unknown, not retrying: {e}", l.employee_id);
                    ambiguous.push(l.employee_id.clone());
                    false
                }
            }
        });
        if !ambiguous.is_empty() {
            die(format!("step 2 outcome unknown for {ambiguous:?}; their ledgers stay at pay-pending so a rerun refuses until checked"));
        }
        if pending.is_empty() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let ids: Vec<_> = pending.iter().map(|l| l.employee_id.as_str()).collect();
            die(format!("step 2 timed out after {wait_secs}s for {ids:?}; funds are in their child wallets; rerun with the same --label to pay only these (no re-funding)"));
        }
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

/// A funding send that failed at proposal never broadcast: drop the pending marker.
fn clear_fund_pending(lines: &[zec_payroll::PayLine], dir_of: &dyn Fn(&zec_payroll::PayLine) -> String, period: &str) {
    for l in lines {
        let p = format!("{}/payroll-ledger", dir_of(l));
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let kept: String = text
            .lines()
            .filter(|x| {
                let mut it = x.split_whitespace();
                let (k, per) = (it.next(), it.next());
                !(per == Some(period) && matches!(k, Some("fund-pending")))
            })
            .map(|x| format!("{x}\n"))
            .collect();
        std::fs::write(&p, kept).ok();
    }
}

fn export_proof(args: &[String]) {
    let [path, emp] = args else { die("usage: payroll export-proof <file.csv> <employee_id|name>") };
    let seed = disclosure::EmployerSeed::from_env(|k| std::env::var(k).ok()).unwrap_or_else(|e| die(e));
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let roster = csv::parse_payroll_csv(&text).unwrap_or_else(|errs| die(errs.join("; ")));
    let e = roster.iter().find(|e| &e.id == emp || &e.name == emp).unwrap_or_else(|| die(format!("no employee `{emp}` in {path}")));
    print!("{}", disclosure::export(&seed, &e.address, &e.id, &e.name).unwrap_or_else(|e| die(e)));
}

fn verify_proof(args: &[String]) {
    let usage = "usage: payroll verify-proof <proof-file> <txid> --lwd <https://lightwalletd:443>   (checks the chain)\n   or: payroll verify-proof <proof-file> <raw-tx-hex-file> <txid> <block-height>   (OFFLINE, trusts the supplier)";
    let (proof, bytes, txid, height, onchain) = match args {
        [proof, txid, flag, url] if flag == "--lwd" => {
            if !url.starts_with("https://") {
                die("--lwd must be https://");
            }
            let t = zec_payroll::chain::fetch_tx(url, txid).unwrap_or_else(|e| die(e));
            let h = t.height.unwrap_or_else(|| die(format!("{txid} is not mined in the best chain according to {url}")));
            (proof, t.raw, txid, h, Some(url.clone()))
        }
        [proof, raw, txid, height] => {
            let hexs = std::fs::read_to_string(raw).unwrap_or_else(|e| die(format!("{raw}: {e}")));
            let bytes = hex::decode(hexs.trim()).unwrap_or_else(|e| die(format!("{raw}: not hex: {e}")));
            let h: u32 = height.parse().unwrap_or_else(|_| die("block-height: not a number"));
            (proof, bytes, txid, h, None)
        }
        _ => die(usage),
    };
    let text = std::fs::read_to_string(proof).unwrap_or_else(|e| die(format!("{proof}: {e}")));
    let (emp, ufvk) = disclosure::parse_export(&text).unwrap_or_else(|e| die(e));
    let p = disclosure::verify_tx(&ufvk, &bytes, Some(txid), height).unwrap_or_else(|e| die(e));
    match &onchain {
        Some(url) => println!("VERIFIED ON CHAIN via {url}: employee={emp} txid={} mined at height {}", p.txid, p.height),
        None => println!(
            "DECRYPTED OFFLINE (NOT checked on chain: the tx bytes and height {} are whatever the supplier gave you; use --lwd): employee={emp} txid={}",
            p.height, p.txid
        ),
    }
    for o in &p.outputs {
        println!("  {:?} {:?} {} recipient={} memo={:?}", o.direction, o.pool, money::Zat(o.amount_zat), o.recipient_hex, o.memo);
    }
    println!("PAID OUT {}", money::Zat(p.paid_out_zat()));
    println!("NOTE: this shows what the key in the proof file sees. It does not prove that key belongs to this employer; get the employer's published child-wallet UA or signature for that.");
}

