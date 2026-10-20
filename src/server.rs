//! Employer dashboard: a dependency-free HTTP server on std::net.
//!
//!   GET  /            embedded UI (ui/index.html)
//!   GET  /api/state   env check, roster, runs
//!   POST /api/run     start one shielded testnet payout in a background thread
//!   GET  /api/feed?since=N   payout feed events after index N
//!   GET  /api/audit   accountant ledger + UFVK (view-only)
//!
//! Every chain step goes through the real wallet CLI; a missing var is reported
//! in the feed and the state banner by name, never papered over.

use crate::{audit, config, employee::Employee, wallet, zip321, CapPolicy, Engine, RunStatus};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const INDEX: &str = include_str!("../ui/index.html");

pub struct Event {
    pub kind: &'static str, // info | ok | err | slip
    pub text: String,
}

pub struct State {
    pub engine: Engine,
    pub roster: Vec<Employee>,
    pub roster_path: String,
    pub feed: Vec<Event>,
    pub running: bool,
    pub runs_started: u32,
}

type Shared = Arc<Mutex<State>>;

pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Names of every unset chain var, in the order the payout needs them.
pub fn missing_vars(get: impl Fn(&str) -> Option<String>) -> Vec<&'static str> {
    ["ZCASH_NETWORK", "ZCASH_LWD_URL", "ZCASH_WALLET_DB", "ZCASH_WALLET_CLI", "ZCASH_AUDITOR_UFVK"]
        .into_iter()
        .filter(|k| get(k).is_none_or(|v| v.trim().is_empty()))
        .collect()
}

fn state_json(s: &State) -> String {
    let env = |k: &str| std::env::var(k).ok();
    let missing: Vec<String> = missing_vars(env).iter().map(|v| json_str(v)).collect();
    let cfg_err = config::load_from_env().err().map(|e| e.to_string());
    let roster: Vec<String> = s
        .roster
        .iter()
        .map(|e| {
            format!(
                "{{\"name\":{},\"address\":{},\"amount\":{},\"memo\":{},\"active\":{}}}",
                json_str(&e.name),
                json_str(&e.address),
                json_str(&e.pay.to_string()),
                json_str(&e.memo),
                e.active
            )
        })
        .collect();
    let runs: Vec<String> = s
        .engine
        .runs()
        .iter()
        .map(|r| {
            let (st, txid) = match &r.status {
                RunStatus::Draft => ("draft".to_string(), String::new()),
                RunStatus::Approved => ("approved".into(), String::new()),
                RunStatus::Broadcast { txid } => ("broadcast".into(), txid.clone()),
                RunStatus::Confirmed { txid, height } => (format!("confirmed@{height}"), txid.clone()),
                RunStatus::Failed { reason } => (format!("failed: {reason}"), String::new()),
            };
            format!(
                "{{\"id\":{},\"period\":{},\"status\":{},\"txid\":{},\"total\":{},\"outputs\":{}}}",
                r.id,
                json_str(&r.period_key),
                json_str(&st),
                json_str(&txid),
                json_str(&r.total.to_string()),
                r.lines.len()
            )
        })
        .collect();
    let total = s.roster.iter().filter(|e| e.active).map(|e| e.pay.0).sum::<u64>();
    format!(
        "{{\"network\":{},\"lwd\":{},\"config_error\":{},\"missing\":[{}],\"roster_path\":{},\"roster\":[{}],\"roster_total\":{},\"runs\":[{}],\"running\":{},\"feed_len\":{}}}",
        json_str(&env("ZCASH_NETWORK").unwrap_or_default()),
        json_str(&env("ZCASH_LWD_URL").unwrap_or_default()),
        cfg_err.map_or("null".into(), |e| json_str(&e)),
        missing.join(","),
        json_str(&s.roster_path),
        roster.join(","),
        json_str(&crate::money::Zat(total).to_string()),
        runs.join(","),
        s.running,
        s.feed.len()
    )
}

fn feed_json(s: &State, since: usize) -> String {
    let ev: Vec<String> = s
        .feed
        .iter()
        .skip(since)
        .map(|e| format!("{{\"kind\":\"{}\",\"text\":{}}}", e.kind, json_str(&e.text)))
        .collect();
    format!("{{\"next\":{},\"running\":{},\"events\":[{}]}}", s.feed.len(), s.running, ev.join(","))
}

fn push(st: &Shared, kind: &'static str, text: impl Into<String>) {
    st.lock().unwrap().feed.push(Event { kind, text: text.into() });
}

/// The real payout, step by step. Stops at the first failure and says why.
fn payout(st: Shared) {
    let fail = |st: &Shared, msg: String| {
        push(st, "err", msg);
        st.lock().unwrap().running = false;
    };
    let (batch, label) = {
        let mut s = st.lock().unwrap();
        s.runs_started += 1;
        (s.roster.clone(), format!("dashboard-{}", s.runs_started))
    };
    push(&st, "info", format!("validating {} roster lines (testnet UAs, amounts, memos)", batch.len()));

    let cfg = match config::load_from_env() {
        Ok(c) => c,
        Err(e) => return fail(&st, e.to_string()),
    };
    let bin = match wallet::wallet_cli(|k| std::env::var(k).ok()) {
        Ok(b) => b,
        Err(e) => return fail(&st, e.to_string()),
    };

    let (id, uri, lines) = {
        let mut s = st.lock().unwrap();
        let id = match s.engine.plan_adhoc(&label, batch) {
            Ok(id) => id,
            Err(e) => {
                drop(s);
                return fail(&st, format!("plan: {e}"));
            }
        };
        if let Err(e) = s.engine.approve(id) {
            drop(s);
            return fail(&st, format!("approve: {e}"));
        }
        let run = s.engine.run(id).expect("planned");
        let lines = run.lines.clone();
        match zip321::payout_uri(run) {
            Ok(u) => (id, u, lines),
            Err(e) => {
                drop(s);
                return fail(&st, format!("zip321: {e}"));
            }
        }
    };
    push(&st, "ok", format!("run #{id} approved; ZIP-321 proposal with {} shielded outputs", lines.len()));

    let res = wallet::send_payout_steps(&bin, &cfg, &uri, |step| push(&st, "info", step));
    match res {
        Ok(txid) => {
            let mut s = st.lock().unwrap();
            let _ = s.engine.mark_broadcast(id, &txid);
            s.feed.push(Event { kind: "ok", text: format!("broadcast one shielded tx {txid}") });
            for (i, l) in lines.iter().enumerate() {
                s.feed.push(Event {
                    kind: "slip",
                    text: format!("{} · {} · output {i} · tx {}… · memo \"{}\"", l.name, l.amount, &txid[..12], l.memo),
                });
            }
            s.running = false;
        }
        Err(e) => {
            let _ = st.lock().unwrap().engine.mark_failed(id, &e.to_string());
            fail(&st, e.to_string())
        }
    }
}

fn respond(mut s: TcpStream, code: &str, ctype: &str, body: &str) {
    let _ = write!(
        s,
        "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn handle(stream: TcpStream, st: Shared) {
    let mut rd = BufReader::new(&stream);
    let mut first = String::new();
    if rd.read_line(&mut first).is_err() {
        return;
    }
    // Drain headers; no endpoint takes a body.
    let mut line = String::new();
    while rd.read_line(&mut line).map(|n| n > 2).unwrap_or(false) {
        line.clear();
    }
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let json = "application/json";
    match (method, path) {
        ("GET", "/") => respond(stream, "200 OK", "text/html; charset=utf-8", INDEX),
        ("GET", "/api/state") => {
            let body = state_json(&st.lock().unwrap());
            respond(stream, "200 OK", json, &body)
        }
        ("GET", "/api/feed") => {
            let since = query.strip_prefix("since=").and_then(|v| v.parse().ok()).unwrap_or(0);
            let body = feed_json(&st.lock().unwrap(), since);
            respond(stream, "200 OK", json, &body)
        }
        ("POST", "/api/run") => {
            let mut s = st.lock().unwrap();
            if s.running {
                drop(s);
                return respond(stream, "409 Conflict", json, "{\"error\":\"a payout is already running\"}");
            }
            if s.roster.is_empty() {
                drop(s);
                return respond(stream, "400 Bad Request", json, "{\"error\":\"PAYROLL_ROSTER_CSV not configured\"}");
            }
            s.running = true;
            let since = s.feed.len();
            drop(s);
            let st2 = st.clone();
            std::thread::spawn(move || payout(st2));
            respond(stream, "202 Accepted", json, &format!("{{\"since\":{since}}}"))
        }
        ("GET", "/api/audit") => {
            let ufvk = std::env::var("ZCASH_AUDITOR_UFVK").ok().filter(|v| !v.trim().is_empty());
            let Some(ufvk) = ufvk else {
                return respond(stream, "400 Bad Request", json, "{\"error\":\"ZCASH_AUDITOR_UFVK not configured\"}");
            };
            let res = audit::export_csv(st.lock().unwrap().engine.runs(), &ufvk);
            match res {
                Ok(csv) => respond(stream, "200 OK", "text/csv; charset=utf-8", &csv),
                Err(e) => respond(stream, "400 Bad Request", json, &format!("{{\"error\":{}}}", json_str(&e.to_string()))),
            }
        }
        _ => respond(stream, "404 Not Found", json, "{\"error\":\"not found\"}"),
    }
}

pub fn serve(addr: &str, roster_path: String, roster: Vec<Employee>, caps: CapPolicy) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let st: Shared = Arc::new(Mutex::new(State {
        engine: Engine::new(caps),
        roster,
        roster_path,
        feed: Vec::new(),
        running: false,
        runs_started: 0,
    }));
    println!("dashboard on http://{addr}");
    for stream in listener.incoming().flatten() {
        let st = st.clone();
        std::thread::spawn(move || handle(stream, st));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_json() {
        assert_eq!(json_str("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
    }

    #[test]
    fn lists_every_missing_var() {
        let got = missing_vars(|k| (k == "ZCASH_NETWORK").then(|| "testnet".to_string()));
        assert_eq!(got, ["ZCASH_LWD_URL", "ZCASH_WALLET_DB", "ZCASH_WALLET_CLI", "ZCASH_AUDITOR_UFVK"]);
    }
}
