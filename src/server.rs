//! Employer dashboard: a dependency-free HTTP server on std::net. Read-only.
//!
//!   GET  /            embedded UI (ui/index.html)
//!   GET  /api/state   env check, roster, runs
//!   GET  /api/audit   accountant ledger + UFVK (view-only)
//!   POST /api/run     410 Gone: payouts run only from the CLI (`payroll pay --two-step`)
//!   GET  /api/proof   410 Gone: signed proofs come only from the CLI (`payroll export-proof --employee`)
//!
//! The dashboard cannot move funds or hand out viewing keys. A direct treasury payout
//! here would not be covered by employee proofs, and an unauthenticated proof endpoint
//! would give every employee's viewing key to anyone who can reach the port.

use crate::{audit, config, employee::Employee, CapPolicy, Engine, RunStatus};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const INDEX: &str = include_str!("../ui/index.html");

pub const RUN_GONE: &str = "{\"error\":\"The dashboard no longer pays: a direct treasury payout cannot be proven per employee. Run `payroll pay <roster.csv> --two-step --label <period>` from the CLI.\"}";
pub const PROOF_GONE: &str = "{\"error\":\"Proof download is disabled (it had no login). Export one employee's signed proof from the CLI: `payroll export-proof <roster.csv> --employee <id> --label <period>`.\"}";

pub struct State {
    pub engine: Engine,
    pub roster: Vec<Employee>,
    pub roster_path: String,
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
                "{{\"id\":{},\"name\":{},\"address\":{},\"amount\":{},\"memo\":{},\"active\":{}}}",
                json_str(&e.id),
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
        "{{\"network\":{},\"lwd\":{},\"config_error\":{},\"missing\":[{}],\"roster_path\":{},\"roster\":[{}],\"roster_total\":{},\"runs\":[{}]}}",
        json_str(&env("ZCASH_NETWORK").unwrap_or_default()),
        json_str(&env("ZCASH_LWD_URL").unwrap_or_default()),
        cfg_err.map_or("null".into(), |e| json_str(&e)),
        missing.join(","),
        json_str(&s.roster_path),
        roster.join(","),
        json_str(&crate::money::Zat(total).to_string()),
        runs.join(",")
    )
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
    let path = target.split_once('?').map_or(target, |(p, _)| p);
    let json = "application/json";
    match (method, path) {
        ("GET", "/") => respond(stream, "200 OK", "text/html; charset=utf-8", INDEX),
        ("GET", "/api/state") => {
            let body = state_json(&st.lock().unwrap());
            respond(stream, "200 OK", json, &body)
        }
        ("POST", "/api/run") => respond(stream, "410 Gone", json, RUN_GONE),
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
        ("GET", "/api/proof") => respond(stream, "410 Gone", json, PROOF_GONE),
        _ => respond(stream, "404 Not Found", json, "{\"error\":\"not found\"}"),
    }
}

pub fn serve(addr: &str, roster_path: String, roster: Vec<Employee>, caps: CapPolicy) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let st: Shared = Arc::new(Mutex::new(State {
        engine: Engine::new(caps),
        roster,
        roster_path,
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

    /// One real HTTP request through `handle` on a loopback socket; returns the raw response.
    fn request(req: &str) -> String {
        use std::io::Read;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let st: Shared = Arc::new(Mutex::new(State {
            engine: Engine::new(CapPolicy::default()),
            roster: vec![crate::employee::tests::emp("emp-a", "1")],
            roster_path: "roster.csv".into(),
        }));
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            handle(s, st)
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        server.join().unwrap();
        out
    }

    #[test]
    fn proof_endpoint_is_gone_and_leaks_no_key() {
        let r = request("GET /api/proof?employee=emp-a HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 410 Gone"), "{r}");
        assert!(r.contains("payroll export-proof") && !r.contains("uviewtest"), "{r}");
        let r = request("GET /api/proof HTTP/1.1\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 410 Gone"), "{r}");
    }

    #[test]
    fn run_endpoint_is_gone_and_points_to_the_cli() {
        let r = request("POST /api/run HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 410 Gone"), "{r}");
        assert!(r.contains("pay <roster.csv> --two-step"), "{r}");
        assert!(request("GET /api/state HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 200 OK"));
    }
}
