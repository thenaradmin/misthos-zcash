//! Employer dashboard: a dependency-free HTTP server on std::net. Read-only.
//!
//!   GET  /            embedded UI (ui/index.html)
//!   GET  /api/state   env check, roster, runs
//!   GET  /api/audit   accountant ledger + UFVK (view-only)
//!   POST /api/run     410 Gone: payouts run only from the CLI (`payroll pay --two-step`)
//!   GET  /api/proof   410 Gone: signed proofs come only from the CLI (`payroll export-proof --employee`)
//!
//! The dashboard cannot move funds or export employee proofs. A direct treasury payout
//! here would not be covered by employee proofs, and an unauthenticated proof endpoint
//! would give every employee's viewing key to anyone who can reach the port.
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

pub const RUN_GONE: &str = "{\"error\":\"The dashboard no longer pays: a direct treasury payout cannot be proven per employee. Run `payroll pay <roster.csv> --two-step --label <period>` from the CLI.\"}";
pub const PROOF_GONE: &str = "{\"error\":\"Proof download is disabled (it had no login). Export one employee's signed proof from the CLI: `payroll export-proof <roster.csv> --employee <id> --label <period>`.\"}";

pub struct State {
    pub engine: Engine,
    pub roster: Vec<Employee>,
    pub roster_path: String,
    /// Host part of the bound `--addr` (accepted in `Host` besides the loopback names).
    pub bound_host: String,
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
    let missing_names = missing_vars(env);
    let missing: Vec<String> = missing_names.iter().map(|v| json_str(v)).collect();
    let mut cfg_errors: Vec<String> = config::load_from_env().err().map(|e| e.to_string()).into_iter().collect();
    for name in missing_names {
        let error = format!("{name} not configured");
        if !cfg_errors.contains(&error) {
            cfg_errors.push(error);
        }
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

/// Request line and `Host`, read within the size, line-count and time caps.
enum Head {
    Ok { first: String, host: Option<String> },
    TooLarge,
    Bad,
    Gone,
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
            return Head::Ok { first, host };
        }
        lines += 1;
        if lines > MAX_HEADER_LINES {
            return Head::TooLarge;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("host") {
                if host.is_some() {
                    return Head::Bad;
                }
                host = Some(v.trim().to_string());
            }
        }
    }
}

fn handle(stream: TcpStream, st: Shared) {
    handle_with(stream, st, IO_TIMEOUT)
}

fn handle_with(stream: TcpStream, st: Shared, timeout: Duration) {
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let json = "application/json";
    let (first, host) = match read_head(&stream) {
        Head::Ok { first, host } => (first, host),
        Head::TooLarge => return respond(stream, "431 Request Header Fields Too Large", json, "{\"error\":\"request line or headers too large\"}"),
        Head::Bad => return respond(stream, "400 Bad Request", json, "{\"error\":\"bad request\"}"),
        Head::Gone => return,
    };
    let bound = st.lock().unwrap().bound_host.clone();
    if !host_allowed(host.as_deref(), &bound) {
        return respond(stream, "403 Forbidden", json, "{\"error\":\"Host not allowed: the desk answers only on 127.0.0.1, localhost or [::1] (or the --addr host it was bound to)\"}");
    }
    let mut parts = first.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let path = target.split_once('?').map_or(target, |(p, _)| p);
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
    let bound_host = addr.rsplit_once(':').map_or(addr, |(h, _)| h).to_string();
    let st: Shared = Arc::new(Mutex::new(State {
        engine: Engine::new(caps),
        roster,
        roster_path,
        bound_host,
    }));
    println!("dashboard on http://{addr}");
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
