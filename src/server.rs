//! Employer dashboard: a dependency-free HTTP server on std::net.
//!
//!   GET  /                       embedded UI (ui/index.html)
//!   GET  /api/state              env check, roster, runs, payout mode, recorded periods
//!   GET  /api/audit              accountant ledger + UFVK (view-only)
//!   POST /api/roster             replace the roster with an uploaded CSV (validated, not saved)
//!   POST /api/plan               plan the roster against caps (offline, like `payroll plan`)
//!   POST /api/payroll/verify     third-party proof check against the chain (`verify-proof --lwd`)
//!   POST /api/run                410 Gone: a one-shot treasury payout cannot be proven per employee
//!   GET  /api/proof              410 Gone: proofs are exported with POST /api/payroll/proof
//!
//! Only with `payroll serve --payouts` (loopback only), see `crate::desk`:
//!   POST /api/payroll/run        start `payroll pay <snapshot> --two-step --label L` in the background
//!   GET  /api/payroll/job        that run's output lines and per-employee progress
//!   GET  /api/payroll/ledger     the per-child ledger of a period (+ payout block heights)
//!   POST /api/payroll/check      on-chain check of a child wallet whose payout outcome is unknown
//!   POST /api/payroll/resolve    record `pay-failed` only if that check shows nothing left the wallet
//!   POST /api/payroll/proof      one employee's employer-signed proof (`payroll export-proof`)
//!   GET  /api/payroll/accountant the period's ledger for the accountant, with the employer UFVK
//!
//! Without `--payouts` the desk moves no funds and exports no proofs. Every POST must carry
//! `X-Misthos-Desk: 1` and `Content-Type: application/json`, and a browser `Origin` (if sent)
//! must be a loopback or bound host: a page on another site cannot send that header without
//! a CORS preflight this server never approves, so it cannot start a payout (CSRF).
//!
//! It is NOT authenticated: `/api/state` shows the full roster and `/api/audit` shows the
//! employer's viewing key (`ZCASH_AUDITOR_UFVK`) to anyone who can reach the port. So it
//! binds to loopback by default, refuses a non-loopback `--addr` without `--allow-remote`,
//! and answers 403 to any request whose `Host` is not a loopback name or the bound host
//! (DNS rebinding: a web page on another domain resolved to 127.0.0.1). Each connection
//! has 5 s read/write timeouts, a 16 KiB cap on the request line plus headers, at most 64
//! header lines, and at most 64 connections are handled at once (503 above that).

use crate::{audit, config, employee::Employee, CapPolicy, Engine, RunStatus};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Read/write timeout per connection; a client that stalls longer is dropped.
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap on the request line plus all headers. No endpoint takes a body.
pub const MAX_HEAD_BYTES: u64 = 16 * 1024;
/// Cap on the number of header lines.
pub const MAX_HEADER_LINES: usize = 64;
/// Connections handled at once; above this the server answers 503 and closes.
pub const MAX_CONNECTIONS: usize = 64;

const INDEX: &str = include_str!("../ui/index.html");

pub const RUN_GONE: &str = "{\"error\":\"A direct treasury payout cannot be proven per employee, so it is gone. Pay runs are two-step: Run payroll on a desk started with --payouts (POST /api/payroll/run), or `payroll pay <roster.csv> --two-step --label <period>`.\"}";
pub const PROOF_GONE: &str = "{\"error\":\"Proof download by GET is disabled (no login, no CSRF guard). On a desk started with --payouts, export one employee's signed proof with POST /api/payroll/proof; or `payroll export-proof <roster.csv> --employee <id> --label <period>`.\"}";
pub const PAYOUTS_OFF: &str = "{\"error\":\"Payouts are off: this desk is read-only. Start it with `payroll serve --payouts` (loopback only) to run payroll from the browser.\"}";
/// Cap on a request body (a roster CSV or a proof file).
pub const MAX_BODY_BYTES: usize = 256 * 1024;

pub struct State {
    pub engine: Engine,
    pub roster: Vec<Employee>,
    pub roster_path: String,
    /// Host part of the bound `--addr` (accepted in `Host` besides the loopback names).
    pub bound_host: String,
    /// The roster's CSV text, exactly as loaded or uploaded (a pay run snapshots this).
    pub roster_csv: String,
    /// Payout mode (`serve --payouts`); `None` = read-only desk.
    pub payouts: Option<Arc<crate::desk::Desk>>,
}

pub type Shared = Arc<Mutex<State>>;

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
    let missing_names = missing_vars(env);
    let missing: Vec<String> = missing_names.iter().map(|v| json_str(v)).collect();
    let mut cfg_errors: Vec<String> = config::load_from_env().err().map(|e| e.to_string()).into_iter().collect();
    for name in missing_names {
        let error = format!("{name} not configured");
        if !cfg_errors.contains(&error) {
            cfg_errors.push(error);
        }
    }
    let auditor_error = env("ZCASH_AUDITOR_UFVK")
        .filter(|key| !key.trim().is_empty())
        .and_then(|key| audit::check_viewing_key(&key).err())
        .map(|error| error.to_string());
    if let Some(error) = &auditor_error {
        cfg_errors.push(error.clone());
    }
    let cfg_err = (!cfg_errors.is_empty()).then(|| cfg_errors.join("; "));
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
    let delays = match crate::delay::range_from(env) {
        Ok(d) => format!("{{\"min\":{},\"max\":{},\"default\":{}}}", d.min, d.max, d == crate::delay::DelayRange::default()),
        Err(e) => format!("{{\"error\":{}}}", json_str(&e.to_string())),
    };
    let periods: Vec<String> = config::load_from_env().map(|c| crate::desk::periods(&c.wallet_db)).unwrap_or_default().iter().map(|p| json_str(p)).collect();
    let mut payout_missing: Vec<String> = Vec::new();
    if s.payouts.is_some() {
        for k in ["PAYROLL_EMPLOYER_SEED", "PAYROLL_CHILD_BIRTHDAY"] {
            if env(k).is_none_or(|v| v.trim().is_empty()) {
                payout_missing.push(json_str(k));
            }
        }
        if env("PAYROLL_EMPLOYER_SIGNING_KEY").is_none_or(|v| v.trim().is_empty()) && env("PAYROLL_EMPLOYER_SIGNING_KEY_FILE").is_none_or(|v| v.trim().is_empty()) {
            payout_missing.push(json_str("PAYROLL_EMPLOYER_SIGNING_KEY_FILE"));
        }
    }
    let extra = format!(
        ",\"payouts\":{},\"payout_missing\":[{}],\"delays\":{},\"employer_pubkey\":{},\"periods\":[{}],\"explorer_tx\":{}",
        s.payouts.is_some(),
        payout_missing.join(","),
        delays,
        json_str(&env("PAYROLL_EMPLOYER_PUBKEY").unwrap_or_default()),
        periods.join(","),
        json_str(crate::desk::EXPLORER_TX)
    );
    format!(
        "{{\"network\":{},\"lwd\":{},\"config_error\":{},\"auditor_error\":{},\"missing\":[{}],\"roster_path\":{},\"roster\":[{}],\"roster_total\":{},\"runs\":[{}]{extra}}}",
        json_str(&env("ZCASH_NETWORK").unwrap_or_default()),
        json_str(&env("ZCASH_LWD_URL").unwrap_or_default()),
        cfg_err.map_or("null".into(), |e| json_str(&e)),
        auditor_error.map_or("null".into(), |e| json_str(&e)),
        missing.join(","),
        json_str(&s.roster_path),
        roster.join(","),
        json_str(&crate::money::Zat(total).to_string()),
        runs.join(",")
    )
}

fn respond(mut s: TcpStream, code: &str, ctype: &str, body: &str) {
    // A rejected request can leave unread bytes in the socket. Build the entire
    // response first so closing that connection cannot cut a series of small
    // formatting writes off after only the HTTP prefix.
    let response = format!(
        "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(response.as_bytes());
}

/// Host part of `host[:port]`, `[v6]:port` or a bare name, lowercased.
pub fn host_name(h: &str) -> String {
    let h = h.trim();
    let name = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else if h.matches(':').count() == 1 {
        h.split(':').next().unwrap_or("")
    } else {
        h
    };
    name.to_ascii_lowercase()
}

/// DNS-rebinding guard: only loopback names, or the host the server was bound to.
pub fn host_allowed(host: Option<&str>, bound_host: &str) -> bool {
    let Some(h) = host else { return false };
    let n = host_name(h);
    if n.is_empty() {
        return false;
    }
    matches!(n.as_str(), "127.0.0.1" | "localhost" | "::1") || n == host_name(bound_host)
}

/// True if every address `addr` resolves to is loopback.
pub fn is_loopback_addr(addr: &str) -> Result<bool, String> {
    let addrs: Vec<_> = addr.to_socket_addrs().map_err(|e| format!("{addr}: {e}"))?.collect();
    if addrs.is_empty() {
        return Err(format!("{addr}: resolves to no address"));
    }
    Ok(addrs.iter().all(|a| a.ip().is_loopback()))
}

/// One of at most `max` concurrent handlers; released on drop.
pub struct Slot(Arc<AtomicUsize>);

impl Slot {
    pub fn acquire(count: &Arc<AtomicUsize>, max: usize) -> Option<Slot> {
        let prev = count.fetch_add(1, Ordering::SeqCst);
        if prev >= max {
            count.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(count.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Request line and the headers the desk uses, read within the size, line-count and time caps.
enum Head {
    Ok { first: String, host: Option<String>, hdr: Headers, rest: Vec<u8> },
    TooLarge,
    Bad,
    Gone,
}

#[derive(Default)]
struct Headers {
    content_length: Option<usize>,
    content_type: String,
    origin: Option<String>,
    desk: bool,
    fetch_site: Option<String>,
}

fn read_head(stream: &TcpStream) -> Head {
    let mut rd = BufReader::new(stream.take(MAX_HEAD_BYTES));
    let mut first = String::new();
    match rd.read_line(&mut first) {
        Ok(0) => return Head::Gone,
        Ok(_) if !first.ends_with('\n') => return if rd.get_ref().limit() == 0 { Head::TooLarge } else { Head::Gone },
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Head::Bad,
        Err(_) => return Head::Gone,
    }
    let mut host = None;
    let mut hdr = Headers::default();
    let mut lines = 0usize;
    loop {
        let mut line = String::new();
        match rd.read_line(&mut line) {
            Ok(0) => return Head::Gone,
            Ok(_) if !line.ends_with('\n') => return if rd.get_ref().limit() == 0 { Head::TooLarge } else { Head::Gone },
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Head::Bad,
            Err(_) => return Head::Gone,
        }
        if line.trim_end_matches(['\r', '\n']).is_empty() {
            // Bytes of the body the reader already buffered past the blank line.
            let rest = rd.buffer().to_vec();
            return Head::Ok { first, host, hdr, rest };
        }
        lines += 1;
        if lines > MAX_HEADER_LINES {
            return Head::TooLarge;
        }
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            match k.as_str() {
                "host" => {
                    if host.is_some() {
                        return Head::Bad;
                    }
                    host = Some(v);
                }
                "content-length" => {
                    let Ok(n) = v.parse::<usize>() else { return Head::Bad };
                    if hdr.content_length.is_some_and(|m| m != n) {
                        return Head::Bad;
                    }
                    hdr.content_length = Some(n);
                }
                "transfer-encoding" => return Head::Bad, // chunked bodies are not accepted
                "content-type" => hdr.content_type = v.to_ascii_lowercase(),
                "origin" => hdr.origin = Some(v),
                "x-misthos-desk" => hdr.desk = v == "1",
                "sec-fetch-site" => hdr.fetch_site = Some(v.to_ascii_lowercase()),
                _ => {}
            }
        }
    }
}

/// CSRF guard for every POST: our header, a JSON body, and (if the browser says where the
/// request comes from) the same site. Returns the reason it refuses.
fn post_guard(hdr: &Headers, bound: &str) -> Result<(), &'static str> {
    if !hdr.desk {
        return Err("missing X-Misthos-Desk: 1 (requests come from the desk page only)");
    }
    if !hdr.content_type.starts_with("application/json") {
        return Err("Content-Type must be application/json");
    }
    if let Some(o) = &hdr.origin {
        let h = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://"));
        if !h.is_some_and(|h| host_allowed(Some(h), bound)) {
            return Err("Origin not allowed: requests must come from the desk page itself");
        }
    }
    if let Some(site) = &hdr.fetch_site {
        if site != "same-origin" && site != "none" {
            return Err("cross-site request refused");
        }
    }
    Ok(())
}

/// Reads exactly Content-Length body bytes (capped), starting from what the head reader buffered.
fn read_body(stream: &TcpStream, hdr: &Headers, mut rest: Vec<u8>) -> Result<String, (&'static str, &'static str)> {
    let n = hdr.content_length.ok_or(("411 Length Required", "{\"error\":\"Content-Length required\"}"))?;
    if n > MAX_BODY_BYTES {
        return Err(("413 Payload Too Large", "{\"error\":\"body larger than 256 KiB\"}"));
    }
    rest.truncate(n);
    if rest.len() < n {
        let mut more = vec![0u8; n - rest.len()];
        let mut s = stream;
        s.read_exact(&mut more).map_err(|_| ("400 Bad Request", "{\"error\":\"body shorter than Content-Length\"}"))?;
        rest.extend_from_slice(&more);
    }
    String::from_utf8(rest).map_err(|_| ("400 Bad Request", "{\"error\":\"body is not UTF-8\"}"))
}

fn err_json(e: &str) -> String {
    format!("{{\"error\":{}}}", json_str(e))
}

fn query_param(target: &str, key: &str) -> Option<String> {
    let q = target.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

fn handle(stream: TcpStream, st: Shared) {
    handle_with(stream, st, IO_TIMEOUT)
}

fn handle_with(stream: TcpStream, st: Shared, timeout: Duration) {
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let json = "application/json";
    let (first, host, hdr, rest) = match read_head(&stream) {
        Head::Ok { first, host, hdr, rest } => (first, host, hdr, rest),
        Head::TooLarge => return respond(stream, "431 Request Header Fields Too Large", json, "{\"error\":\"request line or headers too large\"}"),
        Head::Bad => return respond(stream, "400 Bad Request", json, "{\"error\":\"bad request\"}"),
        Head::Gone => return,
    };
    let (first, host, hdr, rest) = (first, host, hdr, rest);
    let bound = st.lock().unwrap().bound_host.clone();
    if !host_allowed(host.as_deref(), &bound) {
        return respond(stream, "403 Forbidden", json, "{\"error\":\"Host not allowed: the desk answers only on 127.0.0.1, localhost or [::1] (or the --addr host it was bound to)\"}");
    }
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let path = target.split_once('?').map_or(target, |(p, _)| p);
    match (method, path) {
        ("GET", "/") => return respond(stream, "200 OK", "text/html; charset=utf-8", INDEX),
        ("GET", "/api/state") => {
            let body = state_json(&st.lock().unwrap());
            return respond(stream, "200 OK", json, &body);
        }
        ("POST", "/api/run") => return respond(stream, "410 Gone", json, RUN_GONE),
        ("GET", "/api/audit") => {
            let ufvk = std::env::var("ZCASH_AUDITOR_UFVK").ok().filter(|v| !v.trim().is_empty());
            let Some(ufvk) = ufvk else {
                return respond(stream, "400 Bad Request", json, "{\"error\":\"ZCASH_AUDITOR_UFVK not configured\"}");
            };
            let res = audit::export_csv(st.lock().unwrap().engine.runs(), &ufvk);
            return match res {
                Ok(csv) => respond(stream, "200 OK", "text/csv; charset=utf-8", &csv),
                Err(e) => respond(stream, "400 Bad Request", json, &format!("{{\"error\":{}}}", json_str(&e.to_string()))),
            };
        }
        ("GET", "/api/proof") => return respond(stream, "410 Gone", json, PROOF_GONE),
        _ => {}
    }
    let desk = st.lock().unwrap().payouts.clone();
    let known_post = matches!(path, "/api/roster" | "/api/plan" | "/api/payroll/run" | "/api/payroll/check" | "/api/payroll/resolve" | "/api/payroll/proof" | "/api/payroll/verify");
    let known_get = matches!(path, "/api/payroll/job" | "/api/payroll/ledger" | "/api/payroll/accountant");
    if !(method == "POST" && known_post || method == "GET" && known_get) {
        return respond(stream, "404 Not Found", json, "{\"error\":\"not found\"}");
    }
    let payout_only = !matches!(path, "/api/plan" | "/api/payroll/verify");
    if payout_only && desk.is_none() {
        return respond(stream, "410 Gone", json, PAYOUTS_OFF);
    }
    if method == "GET" {
        let label = query_param(target, "label").unwrap_or_default();
        let (code, body) = crate::desk_api::get(path, &label, &st, desk.as_deref().expect("payout-only checked"));
        return respond(stream, code, json, &body);
    }
    if let Err(why) = post_guard(&hdr, &bound) {
        return respond(stream, "403 Forbidden", json, &err_json(why));
    }
    let body = match read_body(&stream, &hdr, rest) {
        Ok(b) => b,
        Err((code, msg)) => return respond(stream, code, json, msg),
    };
    let req = match crate::desk::parse_flat_json(&body) {
        Ok(m) => m,
        Err(e) => return respond(stream, "400 Bad Request", json, &err_json(&e)),
    };
    let (code, body) = crate::desk_api::post(path, &req, &st, desk.as_deref());
    respond(stream, code, json, &body)
}

pub fn serve(addr: &str, roster_path: String, roster: Vec<Employee>, roster_csv: String, caps: CapPolicy, payouts: bool) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let bound_host = addr.rsplit_once(':').map_or(addr, |(h, _)| h).to_string();
    let desk = if payouts { Some(Arc::new(crate::desk::Desk::new(std::env::current_exe()?))) } else { None };
    let st: Shared = Arc::new(Mutex::new(State {
        engine: Engine::new(caps),
        roster,
        roster_path,
        bound_host,
        roster_csv,
        payouts: desk,
    }));
    let local = listener.local_addr().map(|a| a.to_string()).unwrap_or_else(|_| addr.to_string());
    println!("dashboard on http://{local}");
    if payouts {
        println!("payouts ON: Run payroll on this desk sends real testnet transactions (loopback only)");
    }
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming().flatten() {
        let Some(slot) = Slot::acquire(&active, MAX_CONNECTIONS) else {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
            respond(stream, "503 Service Unavailable", "application/json", "{\"error\":\"too many connections\"}");
            continue;
        };
        let st = st.clone();
        std::thread::spawn(move || {
            let _slot = slot;
            handle(stream, st)
        });
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

    fn state() -> Shared {
        Arc::new(Mutex::new(State {
            engine: Engine::new(CapPolicy::default()),
            roster: vec![crate::employee::tests::emp("emp-a", "1")],
            roster_path: "roster.csv".into(),
            bound_host: "127.0.0.1".into(),
            roster_csv: String::new(),
            payouts: None,
        }))
    }

    /// One real HTTP request through `handle` on a loopback socket; returns the raw response.
    fn request(req: &str) -> String {
        request_bytes(req.as_bytes())
    }

    fn request_bytes(req: &[u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let st = state();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            handle(s, st)
        });
        let mut c = TcpStream::connect(addr).unwrap();
        // The server may answer and close before reading everything (431): ignore write errors.
        let _ = c.write_all(req);
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out);
        server.join().unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn proof_endpoint_is_gone_and_leaks_no_key() {
        let r = request("GET /api/proof?employee=emp-a HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 410 Gone"), "{r}");
        assert!(r.contains("payroll export-proof") && !r.contains("uviewtest"), "{r}");
        let r = request("GET /api/proof HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 410 Gone"), "{r}");
    }

    #[test]
    fn run_endpoint_is_gone_and_points_to_the_cli() {
        let r = request("POST /api/run HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 410 Gone"), "{r}");
        assert!(r.contains("pay <roster.csv> --two-step"), "{r}");
        // Read-only desk: the payout endpoints answer 410 and point to --payouts.
        let r = request("POST /api/payroll/run HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Misthos-Desk: 1\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}");
        assert!(r.starts_with("HTTP/1.1 410 Gone") && r.contains("serve --payouts"), "{r}");
        assert!(request("GET /api/payroll/job HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").starts_with("HTTP/1.1 410 Gone"));
        assert!(request("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n\r\n").starts_with("HTTP/1.1 200 OK"));
    }

    /// DNS rebinding: a page on attacker.example resolved to 127.0.0.1 sends its own Host.
    #[test]
    fn foreign_host_gets_403_and_no_roster_or_key() {
        for path in ["/api/audit", "/api/state", "/"] {
            let r = request(&format!("GET {path} HTTP/1.1\r\nHost: attacker.example:8799\r\n\r\n"));
            assert!(r.starts_with("HTTP/1.1 403 Forbidden"), "{path}: {r}");
            assert!(!r.contains("uviewtest") && !r.contains("emp-a") && !r.contains("<html"), "{path}: {r}");
        }
        // No Host at all, or two of them, is refused too.
        assert!(request("GET /api/state HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403 Forbidden"));
        assert!(request("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nHost: evil\r\n\r\n").starts_with("HTTP/1.1 400"));
        for ok in ["127.0.0.1:8787", "localhost", "LOCALHOST:1", "[::1]:8787"] {
            let r = request(&format!("GET /api/state HTTP/1.1\r\nHost: {ok}\r\n\r\n"));
            assert!(r.starts_with("HTTP/1.1 200 OK"), "{ok}: {r}");
        }
    }

    /// CSRF: a cross-site form or fetch cannot plan, upload or (with --payouts) pay.
    #[test]
    fn posts_need_the_desk_header_json_and_a_same_site_origin() {
        let body = r#"{"label":"oct"}"#;
        let req = |extra: &str| request(&format!("POST /api/plan HTTP/1.1\r\nHost: 127.0.0.1:8787\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len()));
        // A plain HTML form post: no header, form content type.
        let r = req("Content-Type: application/x-www-form-urlencoded\r\n");
        assert!(r.starts_with("HTTP/1.1 403") && r.contains("X-Misthos-Desk"), "{r}");
        let r = req("X-Misthos-Desk: 1\r\nContent-Type: text/plain\r\n");
        assert!(r.starts_with("HTTP/1.1 403") && r.contains("application/json"), "{r}");
        let r = req("X-Misthos-Desk: 1\r\nContent-Type: application/json\r\nOrigin: https://attacker.example\r\n");
        assert!(r.starts_with("HTTP/1.1 403") && r.contains("Origin"), "{r}");
        let r = req("X-Misthos-Desk: 1\r\nContent-Type: application/json\r\nSec-Fetch-Site: cross-site\r\n");
        assert!(r.starts_with("HTTP/1.1 403"), "{r}");
        let r = req("X-Misthos-Desk: 1\r\nContent-Type: application/json\r\nOrigin: http://127.0.0.1:8787\r\nSec-Fetch-Site: same-origin\r\n");
        assert!(r.starts_with("HTTP/1.1 200 OK") && r.contains("\"total\""), "{r}");
        // Chunked bodies and oversized bodies are refused.
        let r = request("POST /api/plan HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Misthos-Desk: 1\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 400"), "{r}");
        let r = request(&format!("POST /api/plan HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Misthos-Desk: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1));
        assert!(r.starts_with("HTTP/1.1 413"), "{r}");
    }

    #[test]
    fn host_rules() {
        assert!(host_allowed(Some("192.168.1.5:8787"), "192.168.1.5"));
        assert!(!host_allowed(Some("192.168.1.5:8787"), "127.0.0.1"));
        assert!(!host_allowed(Some("127.0.0.1.attacker.example"), "127.0.0.1"));
        assert!(!host_allowed(Some(""), "127.0.0.1"));
        assert!(!host_allowed(None, "127.0.0.1"));
        assert_eq!(host_name("[::1]:80"), "::1");
        assert_eq!(host_name("::1"), "::1");
    }

    #[test]
    fn non_loopback_bind_is_detected() {
        assert_eq!(is_loopback_addr("127.0.0.1:8787"), Ok(true));
        assert_eq!(is_loopback_addr("[::1]:8787"), Ok(true));
        assert_eq!(is_loopback_addr("0.0.0.0:8787"), Ok(false));
        assert_eq!(is_loopback_addr("192.168.1.5:8787"), Ok(false));
        assert!(is_loopback_addr("no-port").is_err());
    }

    #[test]
    fn oversized_head_gets_431_not_unbounded_memory() {
        // A request line of exactly the cap with no newline: 431.
        let mut big = b"GET /".to_vec();
        big.resize(MAX_HEAD_BYTES as usize, b'A');
        assert!(request_bytes(&big).starts_with("HTTP/1.1 431"));
        // Unread trailing bytes must not truncate the rejection response.
        for prefix in ["GET /", "GET / HTTP/1.1\r\nHost: localhost\r\nX: "] {
            let oversized = format!("{prefix}{}\r\n\r\n", "x".repeat(MAX_HEAD_BYTES as usize));
            let response = request(&oversized);
            assert!(response.starts_with("HTTP/1.1 431 Request Header Fields Too Large\r\n"), "{response}");
            assert!(response.ends_with("\r\n\r\n{\"error\":\"request line or headers too large\"}"), "{response}");
        }
        // 64 MiB with no newline: the handler stops reading at 16 KiB and returns, instead of
        // buffering it all (the client then sees a reset, which is fine).
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let st = state();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            handle(s, st)
        });
        let client = std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            let chunk = vec![b'A'; 1 << 20];
            for _ in 0..64 {
                if c.write_all(&chunk).is_err() {
                    return true; // server closed early
                }
            }
            false
        });
        server.join().unwrap();
        assert!(client.join().unwrap(), "server read all 64 MiB instead of stopping at the cap");
        // Too many header lines.
        let mut many = String::from("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\n");
        for i in 0..MAX_HEADER_LINES {
            many.push_str(&format!("X-{i}: y\r\n"));
        }
        many.push_str("\r\n");
        assert!(request(&many).starts_with("HTTP/1.1 431"), "{}", request(&many));
    }

    #[test]
    fn stalled_client_is_dropped_after_the_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let st = state();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let t = std::time::Instant::now();
            handle_with(s, st, Duration::from_millis(300));
            t.elapsed()
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(b"GET / HTTP/1.1\r\n").unwrap(); // and then nothing (slowloris)
        let took = server.join().unwrap();
        assert!(took < Duration::from_secs(3), "handler stayed alive for {took:?}");
        drop(c);
    }

    #[test]
    fn connection_slots_are_capped_and_released() {
        let n = Arc::new(AtomicUsize::new(0));
        let held: Vec<_> = (0..MAX_CONNECTIONS).map(|_| Slot::acquire(&n, MAX_CONNECTIONS).unwrap()).collect();
        assert!(Slot::acquire(&n, MAX_CONNECTIONS).is_none());
        assert_eq!(n.load(Ordering::SeqCst), MAX_CONNECTIONS);
        drop(held);
        assert_eq!(n.load(Ordering::SeqCst), 0);
        assert!(Slot::acquire(&n, MAX_CONNECTIONS).is_some());
    }
}
