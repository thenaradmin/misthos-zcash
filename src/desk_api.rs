//! The desk's JSON endpoints for roster upload, planning and (with `--payouts`) the pay run.
//! Routing, the Host/CSRF guards and body limits are in `crate::server`; the chain work is
//! the `payroll` binary's own commands, started by `crate::desk`.

use crate::desk::{self, Desk};
use crate::server::{json_str, Shared};
use crate::wallet::ledger;
use crate::{config, csv, employee::Employee, money::Zat, CapPolicy, Engine, EngineError};
use std::collections::BTreeMap;

type Resp = (&'static str, String);

fn bad(code: &'static str, msg: impl AsRef<str>) -> Resp {
    (code, format!("{{\"error\":{}}}", json_str(msg.as_ref())))
}

fn field<'a>(req: &'a BTreeMap<String, String>, k: &str) -> &'a str {
    req.get(k).map(|s| s.trim()).unwrap_or("")
}

/// Cap flags exactly as the CLI takes them (`--max-total`, `--max-per`, `--max-recipients`).
fn caps_from(req: &BTreeMap<String, String>) -> Result<(CapPolicy, Vec<String>), String> {
    let mut caps = CapPolicy::default();
    let mut flags = Vec::new();
    for (k, flag) in [("max_total", "--max-total"), ("max_per", "--max-per")] {
        let v = field(req, k);
        if v.is_empty() {
            continue;
        }
        let z = Zat::parse_zec(v).map_err(|e| format!("{flag}: {e}"))?;
        if k == "max_total" {
            caps.max_run_total = Some(z);
        } else {
            caps.max_per_employee = Some(z);
        }
        flags.extend([flag.to_string(), v.to_string()]);
    }
    let n = field(req, "max_recipients");
    if !n.is_empty() {
        caps.max_recipients = n.parse().map_err(|_| "--max-recipients: not a whole number".to_string())?;
        flags.extend(["--max-recipients".to_string(), n.to_string()]);
    }
    Ok((caps, flags))
}

fn emp_json(e: &Employee) -> String {
    format!(
        "{{\"id\":{},\"name\":{},\"address\":{},\"amount\":{},\"zat\":{},\"memo\":{}}}",
        json_str(&e.id),
        json_str(&e.name),
        json_str(&e.address),
        json_str(&e.pay.to_string()),
        e.pay.0,
        json_str(&e.memo)
    )
}

/// The ledger step of each roster line for a period, if the chain config is loaded.
fn steps(roster: &[Employee], label: &str) -> Option<Vec<ledger::Step>> {
    let cfg = config::load_from_env().ok()?;
    let period = format!("adhoc:{label}");
    std::panic::catch_unwind(|| roster.iter().map(|e| ledger::state(&crate::wallet::child_data_dir(&cfg, &e.address), &period)).collect()).ok()
}

pub fn post(path: &str, req: &BTreeMap<String, String>, st: &Shared, desk: Option<&Desk>) -> Resp {
    match path {
        "/api/roster" => roster(req, st),
        "/api/plan" => plan(req, st),
        "/api/payroll/verify" => verify(req, desk),
        "/api/payroll/run" => run(req, st, desk.expect("payout mode")),
        "/api/payroll/check" | "/api/payroll/resolve" => check(req, path.ends_with("resolve")),
        "/api/payroll/proof" => proof(req, desk.expect("payout mode")),
        _ => bad("404 Not Found", "not found"),
    }
}

pub fn get(path: &str, label: &str, st: &Shared, desk: &Desk) -> Resp {
    match path {
        "/api/payroll/job" => job(st, desk),
        "/api/payroll/ledger" => ledger_view(label, desk, false),
        "/api/payroll/accountant" => ledger_view(label, desk, true),
        _ => bad("404 Not Found", "not found"),
    }
}

fn roster(req: &BTreeMap<String, String>, st: &Shared) -> Resp {
    let text = field(req, "csv");
    let name = field(req, "name");
    match csv::parse_payroll_csv(text) {
        Ok(r) => {
            let mut s = st.lock().unwrap();
            s.roster = r;
            s.roster_csv = req.get("csv").cloned().unwrap_or_default();
            s.roster_path = if name.is_empty() { "uploaded in the desk".into() } else { format!("{name} (uploaded in the desk)") };
            let rows: Vec<String> = s.roster.iter().map(emp_json).collect();
            ("200 OK", format!("{{\"ok\":true,\"roster\":[{}]}}", rows.join(",")))
        }
        Err(errs) => (
            "422 Unprocessable Entity",
            format!("{{\"error\":\"the roster has errors; nothing was loaded\",\"errors\":[{}]}}", errs.iter().map(|e| json_str(e)).collect::<Vec<_>>().join(",")),
        ),
    }
}

/// `payroll plan`, in the browser: the roster against the caps, every violation at once.
fn plan(req: &BTreeMap<String, String>, st: &Shared) -> Resp {
    let label = field(req, "label");
    let label = if label.is_empty() { "draft" } else { label };
    if let Err(e) = desk::check_label(label) {
        return bad("422 Unprocessable Entity", e);
    }
    let (caps, _) = match caps_from(req) {
        Ok(c) => c,
        Err(e) => return bad("422 Unprocessable Entity", e),
    };
    let roster = st.lock().unwrap().roster.clone();
    let mut engine = Engine::new(caps);
    match engine.plan_adhoc(label, roster.clone()) {
        Ok(id) => {
            let run = engine.run(id).expect("planned");
            let st_names = steps(&roster, label);
            let lines: Vec<String> = run
                .lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let step = st_names.as_ref().map_or("", |v| desk::step_name(v[i]));
                    format!(
                        "{{\"id\":{},\"name\":{},\"address\":{},\"amount\":{},\"zat\":{},\"memo\":{},\"ledger\":{}}}",
                        json_str(&l.employee_id),
                        json_str(&l.name),
                        json_str(&l.address),
                        json_str(&l.amount.to_string()),
                        l.amount.0,
                        json_str(&l.memo),
                        json_str(step)
                    )
                })
                .collect();
            let hop_fee = 10_000u64 * run.lines.len() as u64;
            (
                "200 OK",
                format!(
                    "{{\"ok\":true,\"period\":{},\"lines\":[{}],\"total\":{},\"total_zat\":{},\"payout_fees_zat\":{},\"funding_zat\":{}}}",
                    json_str(&run.period_key),
                    lines.join(","),
                    json_str(&run.total.to_string()),
                    run.total.0,
                    hop_fee,
                    run.total.0 + hop_fee
                ),
            )
        }
        Err(EngineError::CapExceeded(v)) => (
            "422 Unprocessable Entity",
            format!("{{\"error\":\"over the caps; nothing can be paid until this is fixed\",\"violations\":[{}]}}", v.iter().map(|e| json_str(e)).collect::<Vec<_>>().join(",")),
        ),
        Err(e) => bad("422 Unprocessable Entity", e.to_string()),
    }
}

fn run(req: &BTreeMap<String, String>, st: &Shared, desk: &Desk) -> Resp {
    let label = field(req, "label");
    if let Err(e) = desk::check_label(label) {
        return bad("422 Unprocessable Entity", e);
    }
    let cfg = match config::load_from_env() {
        Ok(c) => c,
        Err(e) => return bad("400 Bad Request", format!("{e}; nothing sent")),
    };
    let (caps, cap_flags) = match caps_from(req) {
        Ok(c) => c,
        Err(e) => return bad("422 Unprocessable Entity", e),
    };
    let (roster, csv_text) = {
        let s = st.lock().unwrap();
        (s.roster.clone(), s.roster_csv.clone())
    };
    // The same plan check the CLI does first: caps fail here before anything is written.
    if let Err(e) = Engine::new(caps).plan_adhoc(label, roster) {
        return bad("422 Unprocessable Entity", format!("{e}; nothing sent"));
    }
    let snap = match desk::write_snapshot(&cfg.wallet_db, label, &csv_text) {
        Ok(p) => p,
        Err(e) => return bad("409 Conflict", e),
    };
    let mut args = vec!["pay".to_string(), snap, "--two-step".into(), "--label".into(), label.to_string()];
    args.extend(cap_flags);
    match desk::start_job(desk, label, args) {
        Ok(id) => ("202 Accepted", format!("{{\"ok\":true,\"job\":{id},\"label\":{}}}", json_str(label))),
        Err(e) => bad("409 Conflict", e),
    }
}

fn job(_st: &Shared, desk: &Desk) -> Resp {
    let Some(j) = desk.job.lock().unwrap().clone() else {
        return ("200 OK", "{\"job\":null}".into());
    };
    let roster = config::load_from_env().ok().and_then(|c| desk::snapshot_roster(&c.wallet_db, &j.label).ok()).unwrap_or_default();
    let p = desk::parse_progress(&j.lines);
    let lines: Vec<String> = j
        .lines
        .iter()
        .map(|l| format!("{{\"ms\":{},\"err\":{},\"text\":{}}}", l.ms, l.err, json_str(&l.text)))
        .collect();
    (
        "200 OK",
        format!(
            "{{\"job\":{{\"id\":{},\"label\":{},\"running\":{},\"exit\":{},\"elapsed_ms\":{},\"lines\":[{}],\"progress\":{}}}}}",
            j.id,
            json_str(&j.label),
            j.exit.is_none(),
            j.exit.map_or("null".into(), |c| c.to_string()),
            j.started.elapsed().as_millis(),
            lines.join(","),
            desk::progress_json(&p, &roster)
        ),
    )
}

/// Mined height of a payout txid from lightwalletd, cached once mined; an unmined txid is
/// looked up again at most every 20 s.
fn height_of(desk: &Desk, lwd: &str, txid: &str) -> Option<u32> {
    if let Some(h) = desk.heights.lock().unwrap().get(txid) {
        return Some(*h);
    }
    {
        let mut tried = desk.tried.lock().unwrap();
        if tried.get(txid).is_some_and(|t| t.elapsed().as_secs() < 20) {
            return None;
        }
        tried.insert(txid.to_string(), std::time::Instant::now());
    }
    let h = crate::chain::fetch_tx(lwd, txid).ok()?.height?;
    desk.heights.lock().unwrap().insert(txid.to_string(), h);
    Some(h)
}

fn ledger_view(label: &str, desk: &Desk, accountant: bool) -> Resp {
    if let Err(e) = desk::check_label(label) {
        return bad("422 Unprocessable Entity", e);
    }
    let cfg = match config::load_from_env() {
        Ok(c) => c,
        Err(e) => return bad("400 Bad Request", e.to_string()),
    };
    let ufvk = if accountant {
        match std::env::var("ZCASH_AUDITOR_UFVK").ok().filter(|v| !v.trim().is_empty()) {
            None => return bad("400 Bad Request", "ZCASH_AUDITOR_UFVK not configured"),
            Some(k) => match crate::audit::check_viewing_key(&k) {
                Ok(()) => Some(k.trim().to_string()),
                Err(e) => return bad("400 Bad Request", e.to_string()),
            },
        }
    } else {
        None
    };
    let roster = match desk::snapshot_roster(&cfg.wallet_db, label) {
        Ok(r) => r,
        Err(e) => return bad("404 Not Found", e),
    };
    let period = format!("adhoc:{label}");
    let rows = std::panic::catch_unwind(|| {
        roster
            .iter()
            .map(|e| {
                let dir = crate::wallet::child_data_dir(&cfg, &e.address);
                let step = ledger::state(&dir, &period);
                let fund = ledger::funding_txids(&dir, &period);
                let paid = ledger::paid_txids(&dir, &period);
                let target = ledger::target(&dir, &period);
                (e.clone(), step, fund, paid, target)
            })
            .collect::<Vec<_>>()
    });
    let Ok(rows) = rows else { return bad("500 Internal Server Error", "a child ledger could not be read; refusing to guess the payout state") };
    let mut fund_heights = BTreeMap::new();
    let out: Vec<String> = rows
        .iter()
        .map(|(e, step, fund, paid, target)| {
            let paid_h = match paid.as_slice() {
                [t] => height_of(desk, &cfg.lwd_url, t),
                _ => None,
            };
            for f in fund {
                if !fund_heights.contains_key(f) {
                    fund_heights.insert(f.clone(), height_of(desk, &cfg.lwd_url, f));
                }
            }
            let list = |v: &[String]| v.iter().map(|t| json_str(t)).collect::<Vec<_>>().join(",");
            format!(
                "{{\"employee\":{},\"state\":{},\"funding_txids\":[{}],\"paid_txids\":[{}],\"target\":{},\"paid_height\":{}}}",
                emp_json(e),
                json_str(desk::step_name(*step)),
                list(fund),
                list(paid),
                target.map_or("null".into(), |t| t.to_string()),
                paid_h.map_or("null".into(), |t| t.to_string())
            )
        })
        .collect();
    let fh: Vec<String> = fund_heights.iter().map(|(t, h)| format!("{}:{}", json_str(t), h.map_or("null".into(), |h| h.to_string()))).collect();
    let total: u64 = roster.iter().map(|e| e.pay.0).sum();
    (
        "200 OK",
        format!(
            "{{\"label\":{},\"period\":{},\"rows\":[{}],\"funding_heights\":{{{}}},\"total\":{},\"total_zat\":{},\"ufvk\":{}}}",
            json_str(label),
            json_str(&period),
            out.join(","),
            fh.join(","),
            json_str(&Zat(total).to_string()),
            total,
            ufvk.map_or("null".into(), |k| json_str(&k))
        ),
    )
}

fn find_emp(label: &str, emp: &str) -> Result<(config::ChainConfig, Employee), Resp> {
    desk::check_label(label).map_err(|e| bad("422 Unprocessable Entity", e))?;
    let cfg = config::load_from_env().map_err(|e| bad("400 Bad Request", e.to_string()))?;
    let roster = desk::snapshot_roster(&cfg.wallet_db, label).map_err(|e| bad("404 Not Found", e))?;
    let e = csv::select_employee(&roster, emp).map_err(|e| bad("404 Not Found", e))?.clone();
    Ok((cfg, e))
}

/// On-chain check of one child wallet whose last broadcast has no recorded outcome; with
/// `resolve`, records `pay-failed` only if the money is provably still in that wallet.
fn check(req: &BTreeMap<String, String>, resolve: bool) -> Resp {
    let label = field(req, "label");
    let (cfg, e) = match find_emp(label, field(req, "employee")) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let period = format!("adhoc:{label}");
    let dir = crate::wallet::child_data_dir(&cfg, &e.address);
    let step = match std::panic::catch_unwind(|| ledger::state(&dir, &period)) {
        Ok(s) => s,
        Err(_) => return bad("500 Internal Server Error", "this child's ledger could not be read; refusing to guess"),
    };
    if step != ledger::Step::PayPending && !(step == ledger::Step::FundPending && !resolve) {
        return bad("409 Conflict", format!("nothing to check: {}'s ledger for {period} says {}", e.name, desk::step_name(step)));
    }
    let bin = match crate::wallet::wallet_cli(|k| std::env::var(k).ok()) {
        Ok(b) => b,
        Err(err) => return bad("400 Bad Request", err.to_string()),
    };
    let c = match desk::check_child(&bin, &cfg, &e.address) {
        Ok(c) => c,
        Err(err) => return bad("502 Bad Gateway", format!("the on-chain check of {}'s wallet failed: {err}", e.name)),
    };
    let nothing_left = c.nothing_left(e.pay.0);
    let transfers: Vec<String> = c
        .transfers
        .iter()
        .map(|t| {
            format!(
                "{{\"txid\":{},\"status\":{},\"kind\":{},\"value\":{},\"height\":{}}}",
                json_str(&t.txid),
                json_str(&t.status),
                json_str(&t.kind),
                t.value,
                t.height.map_or("null".into(), |h| h.to_string())
            )
        })
        .collect();
    let summary = format!(
        "\"employee\":{},\"state\":{},\"holds_zat\":{},\"needs_zat\":{},\"nothing_left\":{},\"transfers\":[{}]",
        json_str(&e.name),
        json_str(desk::step_name(step)),
        c.holds_zat,
        e.pay.0 + 10_000,
        nothing_left,
        transfers.join(",")
    );
    if !resolve {
        return ("200 OK", format!("{{{summary}}}"));
    }
    if !nothing_left {
        return (
            "409 Conflict",
            format!("{{\"error\":{},{summary}}}", json_str(&format!("not cleared: {}'s wallet does not hold the payout plus its fee, or shows a send that did not fail; it may have been paid. Leave it for a manual check.", e.name))),
        );
    }
    match ledger::record(&dir, "pay-failed", &period, "-") {
        Ok(()) => ("200 OK", format!("{{\"ok\":true,\"recorded\":\"pay-failed\",{summary}}}")),
        Err(err) => bad("500 Internal Server Error", format!("ledger: {err}")),
    }
}

fn proof(req: &BTreeMap<String, String>, desk: &Desk) -> Resp {
    let label = field(req, "label");
    let (cfg, e) = match find_emp(label, field(req, "employee")) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let snap = desk::snapshot_path(&cfg.wallet_db, label);
    match desk::run_cmd(&desk.exe, &["export-proof", &snap, "--employee", &e.id, "--label", label]) {
        Ok((0, out, _)) => {
            let txid = out.lines().find_map(|l| l.strip_prefix("txid=")).unwrap_or("");
            ("200 OK", format!("{{\"ok\":true,\"employee\":{},\"txid\":{},\"proof\":{}}}", json_str(&e.name), json_str(txid), json_str(&out)))
        }
        Ok((_, _, err)) => bad("422 Unprocessable Entity", last_error(&err)),
        Err(err) => bad("500 Internal Server Error", err),
    }
}

fn last_error(stderr: &str) -> String {
    stderr.lines().rev().find_map(|l| l.strip_prefix("ERROR: ")).unwrap_or(stderr.trim()).to_string()
}

/// `payroll verify-proof <proof> <txid> --lwd $ZCASH_LWD_URL`: the signature against the
/// employer key the desk was started with, then the tx fetched from lightwalletd and decrypted.
fn verify(req: &BTreeMap<String, String>, desk: Option<&Desk>) -> Resp {
    let proof = req.get("proof").map(String::as_str).unwrap_or("");
    let txid = field(req, "txid").to_ascii_lowercase();
    if proof.trim().is_empty() {
        return bad("422 Unprocessable Entity", "no proof file given");
    }
    if txid.len() != 64 || !txid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return bad("422 Unprocessable Entity", "txid must be 64 hex characters");
    }
    let lwd = match std::env::var("ZCASH_LWD_URL").ok().filter(|v| !v.trim().is_empty()) {
        Some(l) => l,
        None => return bad("400 Bad Request", "ZCASH_LWD_URL not configured"),
    };
    let exe = match desk {
        Some(d) => d.exe.clone(),
        None => match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => return bad("500 Internal Server Error", e.to_string()),
        },
    };
    // The proof goes to a private temp file for the CLI, then is removed.
    let path = std::env::temp_dir().join(format!("misthos-verify-{}-{:08x}.txt", std::process::id(), crate::delay::os_u32()));
    let wrote = (|| -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        f.write_all(proof.as_bytes())
    })();
    if let Err(e) = wrote {
        return bad("500 Internal Server Error", format!("temp file: {e}"));
    }
    let p = path.to_string_lossy().into_owned();
    let res = desk::run_cmd(&exe, &["verify-proof", &p, &txid, "--lwd", &lwd]);
    let _ = std::fs::remove_file(&path);
    match res {
        Ok((code, out, err)) => {
            let ok = code == 0 && out.contains("VERIFIED ON CHAIN");
            let error = if ok { "null".to_string() } else { json_str(&last_error(&err)) };
            ("200 OK", format!("{{\"ok\":{ok},\"exit\":{code},\"stdout\":{},\"error\":{error}}}", json_str(&out)))
        }
        Err(e) => bad("500 Internal Server Error", e),
    }
}
