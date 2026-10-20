//! `payroll` CLI.
//!
//!   payroll plan <file.csv> [--label L] [--max-total ZEC] [--max-per ZEC] [--max-recipients N]
//!       Validate a CSV and print the capped pay run (offline; no chain I/O).
//!   payroll check-env
//!       Verify chain config; exits non-zero naming the first missing var.

use std::process::exit;
use zec_payroll::{config, csv, money::Zat, CapPolicy, Engine};

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
        _ => die("usage: payroll plan <file.csv> [--label L] [--max-total ZEC] [--max-per ZEC] [--max-recipients N] | payroll check-env"),
    }
}

fn plan(args: &[String]) {
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
    let run = engine.run(id).expect("just planned");
    println!("Run #{} [{}] status=Draft", run.id, run.period_key);
    for l in &run.lines {
        println!("  {:<20} {:>18}  {}…  memo={:?}", l.name, l.amount.to_string(), &l.address[..16], l.memo);
    }
    println!("  TOTAL {} across {} outputs (one shielded tx, built in M2)", run.total, run.lines.len());
}
