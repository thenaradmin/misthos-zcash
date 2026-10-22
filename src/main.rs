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
//!       waits for it to be mined plus random block delays, then step 2 (each child wallet ->
//!       employee, staggered). Needs PAYROLL_EMPLOYER_SEED, PAYROLL_CHILD_BIRTHDAY.
//!   payroll keygen-signing <key-file>
//!       New ed25519 employer signing key (file 0600); prints only the public key.
//!   payroll export-proof <file.csv> --employee <employee_id|name> [--label L]
//!       That ONE employee's signed proof: child-wallet UFVK + employer signature over the
//!       paid txid from the ledger. Needs PAYROLL_EMPLOYER_SIGNING_KEY(_FILE).
//!   payroll verify-proof <proof-file> <txid> --lwd <url> [--employer-pubkey HEX]
//!       Third-party check: employer signature, then what this employee's key sees in that tx.
//!   payroll serve [--addr 127.0.0.1:8787] [--allow-remote]
//!       Employer dashboard for the roster in PAYROLL_ROSTER_CSV. Unauthenticated, so a
//!       non-loopback --addr is refused unless --allow-remote is given.

use std::process::exit;
use zec_payroll::{attest, audit, chain, config, csv, delay, disclosure, money, money::Zat, wallet, zip321, CapPolicy, Engine};

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
        Some("keygen-signing") => keygen_signing(&args[1..]),
        Some("serve") => serve(&args[1..]),
        _ => die("usage: payroll plan|pay <file.csv> [--two-step] [--label L] [--max-total ZEC] [--max-per ZEC] [--max-recipients N] | payroll audit-export <file.csv> [txid] | payroll keygen-signing <key-file> | payroll export-proof <file.csv> --employee <employee_id|name> [--label L] | payroll verify-proof <proof> <txid> --lwd <url> [--employer-pubkey HEX] | payroll serve [--addr HOST:PORT] [--allow-remote] | payroll check-env"),
    }
}

fn keygen_signing(args: &[String]) {
    let [path] = args else { die("usage: payroll keygen-signing <key-file>   (created 0600; refuses to overwrite)") };
    let public = zec_payroll::attest::EmployerKey::generate_to_file(path).unwrap_or_else(|e| die(e));
    println!("wrote signing key to {path} (mode 0600, not shown). Set PAYROLL_EMPLOYER_SIGNING_KEY_FILE={path}");
    println!("employer_pubkey={public}");
    println!("Publish that public key (e.g. in {}) so verifiers can check proofs against it.", zec_payroll::attest::PUBKEY_FILE);
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
    let usage = "usage: payroll serve [--addr HOST:PORT] [--allow-remote]";
    let (mut addr, mut allow_remote) = ("127.0.0.1:8787".to_string(), false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--addr" => addr = it.next().unwrap_or_else(|| die(usage)).clone(),
            "--allow-remote" => allow_remote = true,
            _ => die(usage),
        }
    }
    // The desk has no login: the roster and the employer's viewing key go to whoever reaches the port.
    let loopback = zec_payroll::server::is_loopback_addr(&addr).unwrap_or_else(|e| die(e));
    if !loopback && !allow_remote {
        die(format!("refusing to serve on non-loopback {addr}: the desk is unauthenticated and shows the full roster and the employer's viewing key (ZCASH_AUDITOR_UFVK) to anyone who can reach it. Keep it on 127.0.0.1, or pass --allow-remote if you really mean it."));
    }
    if !loopback {
        eprintln!("WARNING: serving on non-loopback {addr} (--allow-remote). The roster and the employer viewing key are UNAUTHENTICATED: anyone who can reach this port and sends Host: {} can read them.", addr.rsplit_once(':').map_or(addr.as_str(), |(h, _)| h));
    }
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
    if !args.iter().skip(1).any(|a| a == "--label") {
        die("pay --two-step needs an explicit --label <period> (the CSV path is not a stable period: `./x.csv` and `x.csv` would be different periods and fund/pay again). Nothing sent.");
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    // Timing config first: a bad value must stop the run before anything else happens.
    let delays = delay::range_from(env).unwrap_or_else(|e| die(e));
    let secs = |k: &str, dflt: u64| -> u64 {
        env(k).map_or(dflt, |v| v.trim().parse().unwrap_or_else(|_| die(format!("{k}: `{v}` is not a whole number of seconds"))))
    };
    let wait_secs = secs("PAYROLL_STEP2_TIMEOUT_SECS", 1800);
    let confirm_secs = secs("PAYROLL_FUNDING_CONFIRM_TIMEOUT_SECS", 3600);
    let cfg = config::load_from_env().unwrap_or_else(|e| die(e));
    let bin = wallet::wallet_cli(env).unwrap_or_else(|e| die(e));
    let seed = disclosure::EmployerSeed::from_env(env).unwrap_or_else(|e| die(e));
    let birthday: u32 = env("PAYROLL_CHILD_BIRTHDAY")
        .unwrap_or_else(|| die("PAYROLL_CHILD_BIRTHDAY not configured (testnet height to restore child wallets from)"))
        .parse()
        .unwrap_or_else(|_| die("PAYROLL_CHILD_BIRTHDAY: not a number"));
    let (mut engine, id) = build(args);
    engine.approve(id).unwrap_or_else(|e| die(e));
    let run = engine.run(id).expect("planned");
    let (period, lines) = (run.period_key.clone(), run.lines.clone());
    if period.contains(char::is_whitespace) {
        die(format!("period `{period}` contains whitespace; pass --label <name-without-spaces> (the per-child ledger is whitespace-separated). Nothing sent."));
    }
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
    // The payout delays poll lightwalletd's tip directly: it must answer before step 1.
    let tip = chain::tip_height(&cfg.lwd_url).unwrap_or_else(|e| die(format!("{e}; payout delays need the chain tip; step 1 NOT sent")));
    println!("READY lightwalletd tip {tip}; payout delays {}..={} blocks", delays.min, delays.max);

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

    let to_pay: Vec<_> = lines.iter().filter(|l| ledger::state(&dir_of(l), &period) == Step::Funded).cloned().collect();
    for l in lines.iter().filter(|l| ledger::state(&dir_of(l), &period) == Step::Paid) {
        println!("SKIP PAYOUT {} (already paid for {period})", l.employee_id);
    }

    // Delays: payouts start a random d0 blocks after the funding tx is mined, then each
    // employee waits its own random d_i after the previous one. Each target height is written
    // to that child's ledger BEFORE any waiting, and a rerun reuses it (first `pay-at` wins).
    let unscheduled: Vec<_> = to_pay.iter().filter(|l| ledger::target(&dir_of(l), &period).is_none()).collect();
    if !unscheduled.is_empty() {
        let mut txids: Vec<String> = unscheduled.iter().flat_map(|l| ledger::funding_txids(&dir_of(l), &period)).collect();
        txids.sort();
        txids.dedup();
        if txids.is_empty() {
            die(format!("ledger has `funded` for {period} but no funding txid; check the child wallets by hand. Nothing sent."));
        }
        let funded_at = wait_mined(&cfg.lwd_url, &txids, confirm_secs);
        let after = to_pay.iter().filter_map(|l| ledger::target(&dir_of(l), &period)).max().unwrap_or(0);
        let start = (funded_at + delay::draw(delays, delay::os_u32)).max(after);
        let ds: Vec<u32> = unscheduled.iter().map(|_| delay::draw(delays, delay::os_u32)).collect();
        println!("FUNDING MINED at height {funded_at}; payouts begin after height {start}");
        for (l, t) in unscheduled.iter().zip(delay::stagger(start, &ds)) {
            ledger::set_target(&dir_of(l), &period, t).unwrap_or_else(|e| die(format!("ledger: {e}; no payout sent for {}", l.employee_id)));
        }
    }

    // Step 2: per employee, wait for its target height; only then does its
    // PAYROLL_STEP2_TIMEOUT_SECS countdown start, retrying quicksend until spendable.
    struct Pending {
        line: zec_payroll::PayLine,
        target: u32,
        ready_since: Option<u64>,
        shown_left: Option<u32>,
    }
    let mut pending: Vec<Pending> = to_pay
        .iter()
        .map(|l| {
            let target = ledger::target(&dir_of(l), &period).expect("target persisted above");
            println!("PAYOUT SCHEDULED {} at height {target}", l.employee_id);
            Pending { line: l.clone(), target, ready_since: None, shown_left: None }
        })
        .collect();
    let clock = std::time::Instant::now();
    let (mut timed_out, mut tip_failures) = (Vec::new(), 0u32);
    while !pending.is_empty() {
        let tip = match chain::tip_height(&cfg.lwd_url) {
            Ok(h) => {
                tip_failures = 0;
                h
            }
            Err(e) => {
                tip_failures += 1;
                if tip_failures >= 10 {
                    die(format!("{e} (10 polls in a row); unpaid employees' target heights are saved: rerun with the same --label (no re-funding)"));
                }
                eprintln!("tip poll failed ({tip_failures}/10): {e}");
                std::thread::sleep(std::time::Duration::from_secs(30));
                continue;
            }
        };
        let now = clock.elapsed().as_secs();
        let mut ambiguous = Vec::new();
        pending.retain_mut(|p| {
            let l = &p.line;
            match delay::phase(p.target, tip, p.ready_since, now, wait_secs) {
                delay::Phase::Waiting { blocks_left } => {
                    if p.shown_left != Some(blocks_left) {
                        println!("WAIT {} {blocks_left} more block(s) (tip {tip}, payout at {})", l.employee_id, p.target);
                        p.shown_left = Some(blocks_left);
                    }
                    return true;
                }
                delay::Phase::TimedOut => {
                    timed_out.push(l.employee_id.clone());
                    return false;
                }
                delay::Phase::Ready { .. } if p.ready_since.is_none() => {
                    p.ready_since = Some(now);
                    println!("DELAY OVER {} at tip {tip}; {wait_secs}s step-2 timeout starts now", l.employee_id);
                }
                delay::Phase::Ready { .. } => {}
            }
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
        if !pending.is_empty() {
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    }
    if !timed_out.is_empty() {
        die(format!("step 2 timed out {wait_secs}s after the delay ended for {timed_out:?}; funds are in their child wallets; rerun with the same --label to pay only these (no re-funding)"));
    }
}

/// Polls lightwalletd until every funding txid is mined; returns the highest mined height.
fn wait_mined(lwd: &str, txids: &[String], timeout_secs: u64) -> u32 {
    let start = std::time::Instant::now();
    loop {
        let mut heights = Vec::new();
        for t in txids {
            match zec_payroll::chain::fetch_tx(lwd, t) {
                Ok(o) => heights.push(o.height),
                Err(e) => {
                    eprintln!("funding {t}: {e}");
                    heights.push(None);
                }
            }
        }
        if let Some(hs) = heights.iter().copied().collect::<Option<Vec<u32>>>() {
            return hs.into_iter().max().expect("non-empty");
        }
        if start.elapsed().as_secs() > timeout_secs {
            die(format!("funding tx(s) {txids:?} not mined after {timeout_secs}s (PAYROLL_FUNDING_CONFIRM_TIMEOUT_SECS); no payout sent; rerun with the same --label (no re-funding)"));
        }
        println!("WAIT funding tx(s) not mined yet");
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
}

/// A funding send that failed at proposal never broadcast: drop the pending marker.
fn clear_fund_pending(lines: &[zec_payroll::PayLine], dir_of: &dyn Fn(&zec_payroll::PayLine) -> String, period: &str) {
    for l in lines {
        let p = format!("{}/payroll-ledger", dir_of(l));
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => die(format!("{p}: {e}; ledger left as is (fund-pending), a rerun refuses until checked")),
        };
        let kept: String = text
            .lines()
            .filter(|x| {
                let mut it = x.split_whitespace();
                let (k, per) = (it.next(), it.next());
                !(per == Some(period) && matches!(k, Some("fund-pending")))
            })
            .map(|x| format!("{x}\n"))
            .collect();
        // Atomic replace: a crash mid-write must never truncate the ledger (that would erase
        // `paid` history and make every period look Fresh, i.e. fund and pay again).
        let tmp = format!("{p}.tmp");
        let res = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(kept.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &p)
        })();
        if let Err(e) = res {
            die(format!("{p}: could not clear fund-pending ({e}); a rerun refuses until checked"));
        }
    }
}

fn export_proof(args: &[String]) {
    let usage = "usage: payroll export-proof <file.csv> --employee <employee_id|name> --label L   (same --label as `pay --two-step`)";
    let Some((path, flags)) = args.split_first() else { die(usage) };
    let (mut emp, mut label) = (None, None::<String>);
    let mut it = flags.iter();
    while let Some(f) = it.next() {
        let v = it.next().unwrap_or_else(|| die(usage)).clone();
        match f.as_str() {
            "--employee" => emp = Some(v),
            "--label" => label = Some(v),
            _ => die(usage),
        }
    }
    let emp = emp.unwrap_or_else(|| die(usage));
    let label = label.unwrap_or_else(|| die("export-proof needs --label <period>, the same one given to `pay --two-step`"));
    let env = |k: &str| std::env::var(k).ok();
    let cfg = config::load_from_env().unwrap_or_else(|e| die(e));
    let seed = disclosure::EmployerSeed::from_env(env).unwrap_or_else(|e| die(e));
    let signer = attest::EmployerKey::load(env).unwrap_or_else(|e| die(e));
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("{path}: {e}")));
    let roster = csv::parse_payroll_csv(&text).unwrap_or_else(|errs| die(errs.join("; ")));
    let e = roster.iter().find(|e| e.id == emp || e.name == emp).unwrap_or_else(|| die(format!("no employee `{emp}` in {path}")));
    // The signed txid comes from this child's durable ledger, never from the command line.
    let period = format!("adhoc:{label}");
    let dir = wallet::child_data_dir(&cfg, &e.address);
    let txid = match wallet::ledger::paid_txids(&dir, &period).as_slice() {
        [t] => t.clone(),
        [] => die(format!("{}: no `paid` payout for {period} in {dir}/payroll-ledger; run `pay --two-step --label {label}` first", e.id)),
        many => die(format!("{}: payout for {period} has {} txids {many:?}; refusing to sign an ambiguous payout", e.id, many.len())),
    };
    let payout = disclosure::Payout { txid: &txid, amount_zat: e.pay.0, pay_period: &period };
    print!("{}", disclosure::export(&seed, &signer, &e.address, &e.id, &e.name, &payout).unwrap_or_else(|e| die(e)));
}

fn verify_proof(args: &[String]) {
    let usage = "usage: payroll verify-proof <proof-file> <txid> --lwd <https://lightwalletd:443> [--employer-pubkey HEX]   (checks the chain)\n   or: payroll verify-proof <proof-file> <raw-tx-hex-file> <txid> <block-height> [--employer-pubkey HEX]   (OFFLINE, trusts the supplier)";
    let mut pubkey_flag = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--employer-pubkey" {
            pubkey_flag = Some(it.next().unwrap_or_else(|| die(usage)).clone());
        } else {
            rest.push(a.clone());
        }
    }
    enum Source<'a> {
        Lwd(&'a str),
        Offline { raw: &'a str, height: &'a str },
    }
    let (proof_path, txid, src) = match rest.as_slice() {
        [proof, txid, flag, url] if flag == "--lwd" => (proof, txid, Source::Lwd(url)),
        [proof, raw, txid, height] => (proof, txid, Source::Offline { raw, height }),
        _ => die(usage),
    };
    // Employer binding first: a bad signature stops here, before any network call.
    let text = std::fs::read_to_string(proof_path).unwrap_or_else(|e| die(format!("{proof_path}: {e}")));
    let signed = disclosure::parse_export(&text).unwrap_or_else(|e| die(e));
    let (expected, source) = attest::expected_pubkey(pubkey_flag.as_deref(), |k| std::env::var(k).ok()).unwrap_or_else(|e| die(e));
    disclosure::verify_signature(&signed, &expected).unwrap_or_else(|e| die(e));
    let att = &signed.attestation;
    if txid.trim().to_ascii_lowercase() != att.txid {
        die(format!("txid {txid} is not the payout the employer signed ({}): REJECTED", att.txid));
    }
    let (bytes, height, onchain) = match src {
        Source::Lwd(url) => {
            if !url.starts_with("https://") {
                die("--lwd must be https://");
            }
            let t = chain::fetch_tx(url, txid).unwrap_or_else(|e| die(e));
            let h = t.height.unwrap_or_else(|| die(format!("{txid} is not mined in the best chain according to {url}")));
            (t.raw, h, Some(url))
        }
        Source::Offline { raw, height } => {
            let hexs = std::fs::read_to_string(raw).unwrap_or_else(|e| die(format!("{raw}: {e}")));
            let bytes = hex::decode(hexs.trim()).unwrap_or_else(|e| die(format!("{raw}: not hex: {e}")));
            let h: u32 = height.parse().unwrap_or_else(|_| die("block-height: not a number"));
            (bytes, h, None)
        }
    };
    let emp = &signed.employee_id;
    let p = disclosure::verify_tx(&signed.ufvk, &bytes, Some(txid), height).unwrap_or_else(|e| die(e));
    disclosure::check_payout(att, &p).unwrap_or_else(|e| die(e));
    println!(
        "EMPLOYER SIGNATURE OK: key {} (expected key from {source}) signed {} zat, period {}, child {}",
        hex::encode(expected.as_bytes()),
        att.amount_zat,
        att.pay_period,
        att.child_address
    );
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
    println!("NOTE: the signature binds this payout to whoever holds the employer key above; it is only as good as your source for that key.");
}

