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
//!   payroll serve [--addr 127.0.0.1:8787]
//!       Employer dashboard for the roster in PAYROLL_ROSTER_CSV.

use std::process::exit;
use zec_payroll::{audit, config, csv, money::Zat, wallet, zip321, CapPolicy, Engine};

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
        Some("serve") => serve(&args[1..]),
        _ => die("usage: payroll plan|pay <file.csv> [--label L] [--max-total ZEC] [--max-per ZEC] [--max-recipients N] | payroll audit-export <file.csv> [txid] | payroll serve [--addr HOST:PORT] | payroll check-env"),
    }
}

fn pay(args: &[String]) {
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
