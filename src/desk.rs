//! Payout mode for the desk (`payroll serve --payouts`): the employer runs payroll from the
//! browser. Nothing here builds, signs or sends a transaction. Every chain action runs the
//! SAME `payroll` binary's existing commands as a child process:
//!
//!   run      -> `payroll pay <roster snapshot> --two-step --label L [caps]`
//!   proof    -> `payroll export-proof <roster snapshot> --employee <id> --label L`
//!   verify   -> `payroll verify-proof <proof> <txid> --lwd $ZCASH_LWD_URL`
//!
//! so the per-child ledger, the wallet-DB lock, the pre-broadcast checks, the block delays,
//! the rerun refusal and the employer signature are exactly the CLI's. This module adds the
//! parts a browser needs: a roster snapshot per period (the roster `export-proof` must sign
//! from), a job that streams the run's output lines, a parser that turns those lines into
//! per-employee progress, a read of the per-child ledgers, and the on-chain check that lets
//! the employer clear an unknown payout outcome (`pay-pending`) only when the child wallet
//! still holds the money.

use crate::{csv, employee::Employee, wallet};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Explorer used for every tx link the desk shows.
pub const EXPLORER_TX: &str = "https://testnet.zecblock.com/tx/";

/// One line of a job's output, as the CLI printed it.
#[derive(Clone, Debug)]
pub struct JobLine {
    pub ms: u64,
    pub err: bool,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: u64,
    pub label: String,
    pub started: Instant,
    pub lines: Vec<JobLine>,
    /// `None` while running; the exit code (or -1 if killed by a signal) once done.
    pub exit: Option<i32>,
}

pub struct Desk {
    /// The `payroll` binary itself: every chain action runs one of its subcommands.
    pub exe: std::path::PathBuf,
    pub job: Arc<Mutex<Option<Job>>>,
    /// Mined heights of payout txids already looked up (a mined height never changes here).
    pub heights: Mutex<BTreeMap<String, u32>>,
    /// When an unmined txid was last looked up (rate limit for the desk's polling).
    pub tried: Mutex<BTreeMap<String, Instant>>,
}

impl Desk {
    pub fn new(exe: std::path::PathBuf) -> Self {
        Desk { exe, job: Arc::new(Mutex::new(None)), heights: Mutex::new(BTreeMap::new()), tried: Mutex::new(BTreeMap::new()) }
    }
}

/// A pay-period label the per-child ledger can read back: 1-64 chars of `[A-Za-z0-9._-]`.
pub fn check_label(label: &str) -> Result<(), String> {
    let ok = !label.is_empty() && label.len() <= 64 && label.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(format!("pay period `{label}` must be 1-64 characters of letters, digits, `.`, `_` or `-` (no spaces)"))
    }
}

/// Where the desk keeps the roster each period was run with: `<ZCASH_WALLET_DB>/desk/<label>.csv`.
/// `export-proof` signs the amount from this file, so it must be the roster of that period.
pub fn snapshot_path(wallet_db: &str, label: &str) -> String {
    format!("{}/desk/{label}.csv", wallet_db.trim_end_matches('/'))
}

/// Writes the period's roster snapshot. A rerun of the same period must use the same roster:
/// a different one is refused (the ledger is per payout address, and proofs sign the snapshot).
pub fn write_snapshot(wallet_db: &str, label: &str, csv_text: &str) -> Result<String, String> {
    check_label(label)?;
    let p = snapshot_path(wallet_db, label);
    match std::fs::read_to_string(&p) {
        Ok(old) if old == csv_text => return Ok(p),
        Ok(_) => return Err(format!("pay period `{label}` was already run with a different roster; pick a new period label. Nothing sent.")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{p}: {e}; nothing sent")),
    }
    let dir = format!("{}/desk", wallet_db.trim_end_matches('/'));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{dir}: {e}; nothing sent"))?;
    let tmp = format!("{p}.tmp");
    std::fs::write(&tmp, csv_text).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("{p}: {e}; nothing sent"))?;
    Ok(p)
}

/// Periods this desk has run, newest first (by snapshot mtime).
pub fn periods(wallet_db: &str) -> Vec<String> {
    let dir = format!("{}/desk", wallet_db.trim_end_matches('/'));
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut v: Vec<(std::time::SystemTime, String)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let label = name.strip_suffix(".csv")?.to_string();
            check_label(&label).ok()?;
            Some((e.metadata().and_then(|m| m.modified()).ok()?, label))
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, l)| l).collect()
}

/// The roster a period was run with (parsed from its snapshot).
pub fn snapshot_roster(wallet_db: &str, label: &str) -> Result<Vec<Employee>, String> {
    check_label(label)?;
    let p = snapshot_path(wallet_db, label);
    let text = std::fs::read_to_string(&p).map_err(|e| format!("no roster recorded for period `{label}` ({p}: {e})"))?;
    csv::parse_payroll_csv(&text).map_err(|e| e.join("; "))
}

/// Never show a line that talks about wallet secrets (defence in depth: the CLI prints none).
pub fn scrub(line: &str) -> String {
    let l = line.to_ascii_lowercase();
    if l.contains("seed") || l.contains("mnemonic") || l.contains("spending key") {
        "[line hidden by the desk: it mentions wallet secrets]".to_string()
    } else {
        line.to_string()
    }
}

/// Starts `payroll <args>` in the background and streams its output into the job.
/// Refuses while another job is running.
pub fn start_job(desk: &Desk, label: &str, args: Vec<String>) -> Result<u64, String> {
    let mut slot = desk.job.lock().unwrap();
    if let Some(j) = slot.as_ref() {
        if j.exit.is_none() {
            return Err(format!("a pay run for `{}` is still running; wait for it to finish", j.label));
        }
    }
    let mut child = Command::new(&desk.exe)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start the pay run: {e}"))?;
    let id = slot.as_ref().map_or(1, |j| j.id + 1);
    let started = Instant::now();
    *slot = Some(Job { id, label: label.to_string(), started, lines: Vec::new(), exit: None });
    drop(slot);
    let pipe = |r: Box<dyn std::io::Read + Send>, err: bool| {
        let job = desk.job.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines() {
                let Ok(line) = line else { break };
                let mut g = job.lock().unwrap();
                if let Some(j) = g.as_mut().filter(|j| j.id == id) {
                    j.lines.push(JobLine { ms: started.elapsed().as_millis() as u64, err, text: scrub(&line) });
                }
            }
        })
    };
    let out = pipe(Box::new(child.stdout.take().expect("piped")), false);
    let err = pipe(Box::new(child.stderr.take().expect("piped")), true);
    let job = desk.job.clone();
    std::thread::spawn(move || {
        let code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
        let _ = (out.join(), err.join());
        if let Some(j) = job.lock().unwrap().as_mut().filter(|j| j.id == id) {
            j.exit = Some(code);
        }
    });
    Ok(id)
}

/// Runs `payroll <args>` to completion (proof export, proof check). Returns (exit, stdout, stderr).
pub fn run_cmd(exe: &std::path::Path, args: &[&str]) -> Result<(i32, String, String), String> {
    let o = Command::new(exe).args(args).stdin(Stdio::null()).output().map_err(|e| format!("cannot run payroll: {e}"))?;
    let scrub_all = |b: &[u8]| String::from_utf8_lossy(b).lines().map(scrub).collect::<Vec<_>>().join("\n");
    Ok((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), scrub_all(&o.stderr)))
}

/// Where one employee stands in the run, as read from the run's own output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmpProgress {
    pub ready: bool,
    /// `fund` (in the funding tx), `skip` (funded by an earlier run), or empty.
    pub funding: String,
    pub target: Option<u32>,
    pub blocks_left: Option<u32>,
    /// `waiting`, `sending`, `retrying` (note not yet spendable), `paid`, `paid-before`, `unknown`.
    pub payout: String,
    pub txid: String,
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub tip: Option<u32>,
    pub delays: String,
    pub funding_txids: Vec<String>,
    pub funding_height: Option<u32>,
    pub payouts_after: Option<u32>,
    pub waiting_funding: bool,
    pub employees: BTreeMap<String, EmpProgress>,
    pub error: Option<String>,
}

fn word_after<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    s.split_once(key).map(|(_, r)| r.split(|c: char| c.is_whitespace() || c == ';' || c == ')' || c == ',').next().unwrap_or(""))
}

/// Turns `pay --two-step` output into progress. It reads only the lines the CLI already prints
/// (READY, FUND, FUNDING BROADCAST/MINED, SKIP, PAYOUT SCHEDULED/BROADCAST, WAIT, DELAY OVER,
/// step-2 retries and ERROR); an unknown line is ignored, never guessed at.
pub fn parse_progress(lines: &[JobLine]) -> Progress {
    let mut p = Progress::default();
    let num = |s: Option<&str>| s.and_then(|v| v.parse::<u32>().ok());
    for l in lines {
        let t = l.text.trim();
        let mut w = t.split_whitespace();
        let (a, b) = (w.next().unwrap_or(""), w.next().unwrap_or(""));
        match (a, b) {
            ("READY", "child") => {
                // READY child wallet <id> -> <dir>
                if let Some(id) = t.split_whitespace().nth(3) {
                    p.employees.entry(id.into()).or_default().ready = true;
                }
            }
            ("READY", "lightwalletd") => {
                p.tip = num(word_after(t, "tip "));
                p.delays = t.split_once("payout delays ").map(|(_, r)| r.to_string()).unwrap_or_default();
            }
            ("SKIP", "FUNDING") => {
                if let Some(id) = t.split_whitespace().nth(2) {
                    p.employees.entry(id.into()).or_default().funding = "skip".into();
                }
            }
            ("FUND", id) if !id.is_empty() => p.employees.entry(id.into()).or_default().funding = "fund".into(),
            ("FUNDING", "BROADCAST") => {
                if let Some(tx) = word_after(t, "txid=") {
                    p.funding_txids.push(tx.to_string());
                }
            }
            ("FUNDING", "MINED") => {
                p.waiting_funding = false;
                p.funding_height = num(word_after(t, "at height "));
                p.payouts_after = num(word_after(t, "after height "));
            }
            ("WAIT", "funding") => p.waiting_funding = true,
            ("SKIP", "PAYOUT") => {
                if let Some(id) = t.split_whitespace().nth(2) {
                    let e = p.employees.entry(id.into()).or_default();
                    e.payout = "paid-before".into();
                }
            }
            ("PAYOUT", "SCHEDULED") => {
                if let Some(id) = t.split_whitespace().nth(2) {
                    let target = num(word_after(t, "at height "));
                    let e = p.employees.entry(id.into()).or_default();
                    e.target = target;
                    if e.payout.is_empty() {
                        e.payout = "waiting".into();
                    }
                }
            }
            ("WAIT", id) if !id.is_empty() && num(t.split_whitespace().nth(2)).is_some() => {
                // WAIT <id> <n> more block(s) (tip <tip>, payout at <t>)
                let left = num(t.split_whitespace().nth(2));
                let tip = num(word_after(t, "(tip "));
                if tip.is_some() {
                    p.tip = tip;
                }
                let e = p.employees.entry(id.into()).or_default();
                e.blocks_left = left;
                e.payout = "waiting".into();
            }
            ("DELAY", "OVER") => {
                if let Some(id) = t.split_whitespace().nth(2) {
                    p.tip = num(word_after(t, "at tip ")).or(p.tip);
                    let e = p.employees.entry(id.into()).or_default();
                    e.blocks_left = Some(0);
                    e.payout = "sending".into();
                }
            }
            ("PAYOUT", "BROADCAST") => {
                if let Some(id) = t.split_whitespace().nth(2) {
                    let tx = word_after(t, "txid=").unwrap_or("").to_string();
                    let e = p.employees.entry(id.into()).or_default();
                    e.payout = "paid".into();
                    e.txid = tx;
                    e.note.clear();
                }
            }
            ("step", "2") => {
                // `step 2 <id> not yet spendable: …` or `step 2 <id>: outcome unknown, not retrying: …`
                if let Some(raw) = t.split_whitespace().nth(2) {
                    let id = raw.trim_end_matches(':');
                    let e = p.employees.entry(id.into()).or_default();
                    if t.contains("not yet spendable") {
                        e.payout = "retrying".into();
                        e.note = "funding note not spendable yet; retrying every 30 s".into();
                    } else if t.contains("outcome unknown") {
                        e.payout = "unknown".into();
                        e.note = "the wallet errored after a possible broadcast; check it on chain before a rerun".into();
                    }
                }
            }
            ("ERROR:", _) => p.error = Some(t.trim_start_matches("ERROR:").trim().to_string()),
            _ => {}
        }
    }
    p
}

pub fn progress_json(p: &Progress, roster: &[Employee]) -> String {
    use crate::server::json_str;
    let opt = |v: Option<u32>| v.map_or("null".to_string(), |n| n.to_string());
    let emps: Vec<String> = p
        .employees
        .iter()
        .map(|(id, e)| {
            let name = roster.iter().find(|r| &r.id == id).map(|r| r.name.as_str()).unwrap_or("");
            format!(
                "{{\"id\":{},\"name\":{},\"ready\":{},\"funding\":{},\"target\":{},\"blocks_left\":{},\"payout\":{},\"txid\":{},\"note\":{}}}",
                json_str(id),
                json_str(name),
                e.ready,
                json_str(&e.funding),
                opt(e.target),
                opt(e.blocks_left),
                json_str(&e.payout),
                json_str(&e.txid),
                json_str(&e.note)
            )
        })
        .collect();
    format!(
        "{{\"tip\":{},\"delays\":{},\"funding_txids\":[{}],\"funding_height\":{},\"payouts_after\":{},\"waiting_funding\":{},\"employees\":[{}],\"error\":{}}}",
        opt(p.tip),
        json_str(&p.delays),
        p.funding_txids.iter().map(|t| json_str(t)).collect::<Vec<_>>().join(","),
        opt(p.funding_height),
        opt(p.payouts_after),
        p.waiting_funding,
        emps.join(","),
        p.error.as_deref().map_or("null".into(), json_str)
    )
}

pub fn step_name(s: wallet::ledger::Step) -> &'static str {
    use wallet::ledger::Step::*;
    match s {
        Fresh => "not-started",
        FundPending => "fund-pending",
        Funded => "funded",
        PayPending => "pay-pending",
        Paid => "paid",
    }
}

/// One entry of zingo-cli `value_transfers` for a child wallet.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub txid: String,
    pub status: String,
    pub kind: String,
    pub value: u64,
    pub height: Option<u32>,
}

/// Reads zingo-cli's `value_transfers` blocks (`txid: …`, `status: …`, `kind: …`, `value: …`,
/// `blockheight: …`, one `{ … }` block per transfer). Other fields are ignored.
pub fn parse_transfers(out: &str) -> Vec<Transfer> {
    let mut v = Vec::new();
    let mut cur: Option<Transfer> = None;
    for line in out.lines() {
        let l = line.trim();
        if l == "{" {
            cur = Some(Transfer::default());
            continue;
        }
        if l.starts_with('}') {
            if let Some(t) = cur.take().filter(|t| t.txid.len() == 64) {
                v.push(t);
            }
            continue;
        }
        let (Some(t), Some((k, val))) = (cur.as_mut(), l.split_once(':')) else { continue };
        let val = val.trim();
        match k.trim() {
            "txid" => t.txid = val.to_ascii_lowercase(),
            "status" => t.status = val.to_string(),
            "kind" => t.kind = val.to_string(),
            "value" => t.value = val.replace('_', "").parse().unwrap_or(0),
            "blockheight" => t.height = val.parse().ok(),
            _ => {}
        }
    }
    v
}

/// What the employer's on-chain check of one child wallet found.
#[derive(Debug, PartialEq, Eq)]
pub struct ChildCheck {
    /// Total zatoshi the child wallet holds (confirmed + unconfirmed, shielded), from `balance`.
    pub holds_zat: u64,
    /// Its transfers, from `value_transfers`.
    pub transfers: Vec<Transfer>,
}

impl ChildCheck {
    /// Safe to record `pay-failed` (so a rerun pays again) only if the wallet still holds the
    /// payout plus its fee AND no send of it is pending or confirmed: then nothing left it.
    pub fn nothing_left(&self, amount_zat: u64) -> bool {
        let fee = 10_000;
        let sent_ok = self.transfers.iter().any(|t| t.kind == "sent" && t.status != "failed");
        self.holds_zat >= amount_zat + fee && !sent_ok
    }
}

/// Sums the shielded `confirmed_*_balance` and `unconfirmed_*_balance` figures zingo-cli
/// prints (never `spendable_*` or `total_*`, so nothing is counted twice; a note waiting for
/// confirmations still counts as "still in the wallet"). Transparent is ignored.
pub fn parse_balance_total(out: &str) -> Option<u64> {
    let mut found = false;
    let mut sum = 0u64;
    for line in out.lines() {
        let l = line.trim().trim_end_matches(',');
        let Some((k, v)) = l.split_once(':') else { continue };
        let k = k.trim().trim_matches('"').to_ascii_lowercase();
        let v = v.trim().trim_matches('"').replace(['_', ','], "");
        let counts = (k.starts_with("confirmed_") || k.starts_with("unconfirmed_")) && k.ends_with("_balance") && !k.contains("transparent");
        if counts {
            if let Ok(n) = v.split_whitespace().next().unwrap_or("").parse::<u64>() {
                sum += n;
                found = true;
            }
        }
    }
    found.then_some(sum)
}

/// Asks the child wallet (through zingo-cli, synced) what it holds and what it sent.
pub fn check_child(bin: &str, cfg: &crate::config::ChainConfig, address: &str) -> Result<ChildCheck, String> {
    let bal = wallet::child_command(bin, cfg, address, &["balance"]).map_err(|e| e.to_string())?;
    let holds = parse_balance_total(&bal).ok_or_else(|| format!("could not read a balance from zingo-cli: {}", bal.chars().take(300).collect::<String>()))?;
    let vt = wallet::child_command(bin, cfg, address, &["value_transfers"]).map_err(|e| e.to_string())?;
    Ok(ChildCheck { holds_zat: holds, transfers: parse_transfers(&vt) })
}

/// Minimal parser for the flat JSON objects the desk's own page sends:
/// `{"key": "string" | number | true | false | null, ...}`. Nested values are refused.
pub fn parse_flat_json(s: &str) -> Result<BTreeMap<String, String>, String> {
    let b = s.trim().as_bytes();
    let mut i = 0usize;
    let err = |m: &str| Err(format!("bad JSON body: {m}"));
    let ws = |i: &mut usize| {
        while *i < b.len() && (b[*i] as char).is_ascii_whitespace() {
            *i += 1;
        }
    };
    fn string(b: &[u8], i: &mut usize) -> Result<String, String> {
        if b.get(*i) != Some(&b'"') {
            return Err("bad JSON body: expected a string".into());
        }
        *i += 1;
        let mut out: Vec<u8> = Vec::new();
        while *i < b.len() {
            match b[*i] {
                b'"' => {
                    *i += 1;
                    return String::from_utf8(out).map_err(|_| "bad JSON body: not UTF-8".to_string());
                }
                b'\\' => {
                    *i += 1;
                    let c = *b.get(*i).ok_or("bad JSON body: dangling escape")?;
                    match c {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = b.get(*i + 1..*i + 5).ok_or("bad JSON body: short \\u escape")?;
                            let cp = u32::from_str_radix(std::str::from_utf8(hex).map_err(|_| "bad \\u")?, 16).map_err(|_| "bad JSON body: bad \\u escape")?;
                            *i += 4;
                            let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err("bad JSON body: unknown escape".into()),
                    }
                    *i += 1;
                }
                c => {
                    out.push(c);
                    *i += 1;
                }
            }
        }
        Err("bad JSON body: unterminated string".into())
    }
    let mut map = BTreeMap::new();
    ws(&mut i);
    if b.get(i) != Some(&b'{') {
        return err("expected an object");
    }
    i += 1;
    ws(&mut i);
    if b.get(i) == Some(&b'}') {
        i += 1;
        ws(&mut i);
        return if i == b.len() { Ok(map) } else { err("trailing data") };
    }
    loop {
        ws(&mut i);
        let k = string(b, &mut i)?;
        ws(&mut i);
        if b.get(i) != Some(&b':') {
            return err("expected `:`");
        }
        i += 1;
        ws(&mut i);
        let v = match b.get(i) {
            Some(b'"') => string(b, &mut i)?,
            Some(b'{') | Some(b'[') => return err("nested values are not accepted"),
            Some(_) => {
                let start = i;
                while i < b.len() && !matches!(b[i], b',' | b'}') && !(b[i] as char).is_ascii_whitespace() {
                    i += 1;
                }
                let lit = std::str::from_utf8(&b[start..i]).map_err(|_| "bad literal")?;
                let ok = matches!(lit, "true" | "false" | "null") || lit.parse::<f64>().is_ok();
                if !ok {
                    return err("bad literal");
                }
                if lit == "null" {
                    String::new()
                } else {
                    lit.to_string()
                }
            }
            None => return err("unexpected end"),
        };
        if map.insert(k.clone(), v).is_some() {
            return Err(format!("bad JSON body: duplicate key `{k}`"));
        }
        ws(&mut i);
        match b.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => {
                i += 1;
                ws(&mut i);
                return if i == b.len() { Ok(map) } else { err("trailing data") };
            }
            _ => return err("expected `,` or `}`"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<JobLine> {
        v.iter().map(|t| JobLine { ms: 0, err: t.starts_with("step 2") || t.starts_with("ERROR"), text: t.to_string() }).collect()
    }

    const T1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const T2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const T3: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    #[test]
    fn progress_follows_a_full_two_step_run() {
        // The exact line shapes `payroll pay --two-step` prints (src/main.rs).
        let p = parse_progress(&lines(&[
            "READY child wallet row2 -> .demo/wallets/employer/children/child-003ff92442106387",
            "READY child wallet row3 -> .demo/wallets/employer/children/child-539c65255e1c6834",
            "READY lightwalletd tip 4548700; payout delays 1..=2 blocks",
            "FUND row2 0.0004 ZEC -> utest1abc",
            "FUND row3 0.0003 ZEC -> utest1def",
            &format!("FUNDING BROADCAST txid={T1}"),
            "WAIT funding tx(s) not mined yet",
            "FUNDING MINED at height 4548704; payouts begin after height 4548706",
            "PAYOUT SCHEDULED row2 at height 4548707",
            "PAYOUT SCHEDULED row3 at height 4548709",
            "WAIT row2 2 more block(s) (tip 4548705, payout at 4548707)",
            "WAIT row3 4 more block(s) (tip 4548705, payout at 4548709)",
            "DELAY OVER row2 at tip 4548707; 1800s step-2 timeout starts now",
            "step 2 row2 not yet spendable: wallet CLI exited Some(1): Insufficient balance (have 0, need 40000 including fee)",
            &format!("PAYOUT BROADCAST row2 txid={T2}"),
            "DELAY OVER row3 at tip 4548709; 1800s step-2 timeout starts now",
            &format!("PAYOUT BROADCAST row3 txid={T3}"),
        ]));
        assert_eq!(p.tip, Some(4548709));
        assert_eq!(p.delays, "1..=2 blocks");
        assert_eq!(p.funding_txids, vec![T1.to_string()]);
        assert_eq!((p.funding_height, p.payouts_after, p.waiting_funding), (Some(4548704), Some(4548706), false));
        let a = &p.employees["row2"];
        assert!(a.ready && a.funding == "fund" && a.payout == "paid" && a.txid == T2 && a.target == Some(4548707), "{a:?}");
        assert!(a.note.is_empty(), "a retry note is cleared once paid");
        assert_eq!(p.employees["row3"].txid, T3);
        assert_eq!(p.error, None);
    }

    #[test]
    fn progress_shows_retry_unknown_outcome_skips_and_the_refusal() {
        let p = parse_progress(&lines(&[
            "SKIP FUNDING row2 (ledger: Paid for adhoc:oct)",
            "SKIP FUNDING row3 (ledger: Funded for adhoc:oct)",
            "SKIP PAYOUT row2 (already paid for adhoc:oct)",
            "PAYOUT SCHEDULED row3 at height 9",
            "DELAY OVER row3 at tip 9; 1800s step-2 timeout starts now",
            "step 2 row3: outcome unknown, not retrying: wallet CLI exited Some(1): no indexer confirmed delivery",
            "ERROR: step 2 outcome unknown for [\"row3\"]; their ledgers stay at pay-pending so a rerun refuses until checked",
        ]));
        assert_eq!(p.employees["row2"].funding, "skip");
        assert_eq!(p.employees["row2"].payout, "paid-before");
        assert_eq!(p.employees["row3"].payout, "unknown");
        assert!(p.error.as_deref().unwrap().contains("rerun refuses until checked"));
        // A pay-pending refusal from a rerun is an ERROR line before anything is sent.
        let r = parse_progress(&lines(&["ERROR: row3: an earlier `adhoc:oct` broadcast for this child has no recorded outcome (PayPending); check its wallet on chain and fix x/payroll-ledger by hand. Nothing sent."]));
        assert!(r.error.unwrap().contains("Nothing sent"));
        assert!(r.employees.is_empty());
    }

    #[test]
    fn unknown_lines_are_ignored_not_guessed() {
        let p = parse_progress(&lines(&["synced 1234 blocks", "PAYOUT", "FUND", "WAIT", ""]));
        assert_eq!(p, Progress::default());
    }

    #[test]
    fn labels_are_ledger_safe() {
        assert!(check_label("payroll-2026-10.v3_a").is_ok());
        for bad in ["", "has space", "a/b", "../x", "x\n", &"a".repeat(65)] {
            assert!(check_label(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn snapshot_is_written_once_and_a_different_roster_is_refused() {
        let d = std::env::temp_dir().join(format!("zpp-desk-snap-{}", std::process::id()));
        let ds = d.to_str().unwrap();
        let p = write_snapshot(ds, "oct", "name,ua,amountZEC\n").unwrap();
        assert!(p.ends_with("/desk/oct.csv"));
        assert_eq!(write_snapshot(ds, "oct", "name,ua,amountZEC\n").unwrap(), p, "same roster: a rerun reuses it");
        let e = write_snapshot(ds, "oct", "name,ua,amountZEC\nother\n").unwrap_err();
        assert!(e.contains("different roster") && e.contains("Nothing sent"), "{e}");
        assert!(write_snapshot(ds, "../escape", "x").is_err());
        assert_eq!(periods(ds), vec!["oct".to_string()]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn secrets_are_scrubbed_from_job_output() {
        assert!(scrub("restoring from SEED phrase abandon abandon").starts_with("[line hidden"));
        assert!(scrub("your mnemonic is").starts_with("[line hidden"));
        assert_eq!(scrub("PAYOUT BROADCAST row2 txid=ab"), "PAYOUT BROADCAST row2 txid=ab");
    }

    #[test]
    fn balance_total_reads_confirmed_and_unconfirmed_shielded_only() {
        // zingo-cli `balance` as printed on testnet (Oct 28): totals repeat the confirmed figures.
        let out = "[\n    confirmed_ironwood_balance: 15_000\n    unconfirmed_ironwood_balance: 0\n    total_ironwood_balance: 15_000\n\n    confirmed_orchard_balance: 25_000\n    confirmed_transparent_balance: 999\n]";
        assert_eq!(parse_balance_total(out), Some(40_000));
        assert_eq!(parse_balance_total("confirmed_sapling_balance: 5\nunconfirmed_ironwood_balance: 7\n"), Some(12));
        assert_eq!(parse_balance_total("Error: wallet not synced"), None);
    }

    #[test]
    fn transfers_and_the_nothing_left_rule() {
        // Real zingo-cli `value_transfers` shape (Bob's child wallet, film run of Oct 28).
        let out = "{\n    txid: 54a2e6d33cdef872929f17f76d1af2078a4274c04f31611e8b30d02712d28e19\n    status: failed\n    blockheight: 4548719\n    kind: sent\n    value: 20000\n    memos: \n\tmemo 1: Oct 2026 testnet pay\n}\n{\n    txid: df5fd0e85ec636d54cbff1221af512cc3625375eedf6cbace41195416e5c6bf7\n    status: confirmed\n    blockheight: 4548704\n    kind: received\n    value: 30_000\n}\n";
        let t = parse_transfers(out);
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].status.as_str(), t[0].kind.as_str(), t[0].value, t[0].height), ("failed", "sent", 20000, Some(4548719)));
        assert_eq!(t[1].value, 30000);
        let c = ChildCheck { holds_zat: 30_000, transfers: t.clone() };
        assert!(c.nothing_left(20_000), "failed send, money still there");
        assert!(!ChildCheck { holds_zat: 0, transfers: t.clone() }.nothing_left(20_000), "money gone: never clear");
        let mut sent = t;
        sent[0].status = "pending".into();
        assert!(!ChildCheck { holds_zat: 30_000, transfers: sent }.nothing_left(20_000), "a pending send may still land");
    }

    #[test]
    fn flat_json_parses_what_the_page_sends_and_refuses_the_rest() {
        let m = parse_flat_json(r#"{"label":"oct","max_total":"0.001","n":3,"ok":true,"none":null,"csv":"name,ua\n\"A\",xé"}"#).unwrap();
        assert_eq!(m["label"], "oct");
        assert_eq!(m["n"], "3");
        assert_eq!(m["ok"], "true");
        assert_eq!(m["none"], "");
        assert_eq!(m["csv"], "name,ua\n\"A\",x\u{e9}");
        assert!(parse_flat_json("{}").unwrap().is_empty());
        for bad in ["", "[]", "{\"a\":{}}", "{\"a\":[1]}", "{\"a\":1,\"a\":2}", "{\"a\":nope}", "{\"a\":\"x\"} trailing", "{\"a\":\"unterminated}"] {
            assert!(parse_flat_json(bad).is_err(), "{bad}");
        }
    }
}
