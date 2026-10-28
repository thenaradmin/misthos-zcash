//! The desk in payout mode (`payroll serve --payouts`), driven over real HTTP against the real
//! binary. Nothing reaches the chain: the wallet CLI is `false` (it always fails), so every run
//! stops at the CLI's own pre-broadcast check, which is exactly what these tests pin down.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ROSTER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/e2e/fixtures/roster.csv");
const UFVK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/e2e/fixtures/auditor-ufvk.txt");
// The public BIP 39 test vector (all-zero entropy). It owns no funds and nothing is sent.
const TEST_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";

struct Desk {
    child: Child,
    port: u16,
    db: std::path::PathBuf,
}

impl Drop for Desk {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.db);
    }
}

fn start(tag: &str, payouts: bool) -> Desk {
    let db = std::env::temp_dir().join(format!("zpp-desk-api-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&db);
    std::fs::create_dir_all(&db).unwrap();
    let mut args = vec!["serve", "--addr", "127.0.0.1:0"];
    if payouts {
        args.push("--payouts");
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_payroll"))
        .args(&args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("PAYROLL_ROSTER_CSV", ROSTER)
        .env("ZCASH_NETWORK", "testnet")
        .env("ZCASH_LWD_URL", "https://testnet.zec.rocks:443")
        .env("ZCASH_WALLET_DB", &db)
        .env("ZCASH_WALLET_CLI", "false")
        .env("ZCASH_AUDITOR_UFVK", std::fs::read_to_string(UFVK).unwrap().trim())
        .env("PAYROLL_EMPLOYER_SEED", TEST_MNEMONIC)
        .env("PAYROLL_CHILD_BIRTHDAY", "4500000")
        // A throwaway test signing key (not the published demo employer key).
        .env("PAYROLL_EMPLOYER_SIGNING_KEY", "07".repeat(32))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut first).unwrap();
    let port = first.trim().rsplit(':').next().unwrap().parse().unwrap_or_else(|_| panic!("no port in {first:?}"));
    Desk { child, port, db }
}

fn http(d: &Desk, method: &str, path: &str, body: Option<&str>, headers: &str) -> (u16, String) {
    let mut c = TcpStream::connect(("127.0.0.1", d.port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let b = body.unwrap_or("");
    let len = if body.is_some() { format!("Content-Length: {}\r\n", b.len()) } else { String::new() };
    write!(c, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{headers}{len}\r\n{b}", d.port).unwrap();
    let mut out = String::new();
    c.read_to_string(&mut out).unwrap();
    let code = out[9..12].parse().unwrap();
    (code, out.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}

const JSON: &str = "X-Misthos-Desk: 1\r\nContent-Type: application/json\r\n";

fn post(d: &Desk, path: &str, body: &str) -> (u16, String) {
    http(d, "POST", path, Some(body), JSON)
}

fn wait_job(d: &Desk) -> String {
    let t = Instant::now();
    loop {
        let (code, body) = http(d, "GET", "/api/payroll/job", None, "");
        assert_eq!(code, 200, "{body}");
        if body.contains("\"running\":false") {
            return body;
        }
        assert!(t.elapsed() < Duration::from_secs(60), "job never finished: {body}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn payouts_mode_is_loopback_only() {
    let o = Command::new(env!("CARGO_BIN_EXE_payroll"))
        .args(["serve", "--addr", "0.0.0.0:0", "--allow-remote", "--payouts"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("PAYROLL_ROSTER_CSV", ROSTER)
        .output()
        .unwrap();
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("refusing --payouts on non-loopback"), "{e}");
}

#[test]
fn read_only_desk_refuses_payout_endpoints() {
    let d = start("ro", false);
    let (code, body) = post(&d, "/api/payroll/run", r#"{"label":"oct"}"#);
    assert_eq!(code, 410, "{body}");
    assert!(body.contains("--payouts"));
    let (code, body) = http(&d, "GET", "/api/state", None, "");
    assert_eq!(code, 200);
    assert!(body.contains("\"payouts\":false"), "{body}");
    // Planning is offline and works on a read-only desk too.
    let (code, body) = post(&d, "/api/plan", r#"{"label":"oct"}"#);
    assert_eq!(code, 200, "{body}");
}

#[test]
fn plan_lists_every_cap_violation_and_the_funding_total() {
    let d = start("plan", true);
    let (code, body) = post(&d, "/api/plan", r#"{"label":"oct","max_total":"0.03","max_per":"0.01"}"#);
    assert_eq!(code, 422, "{body}");
    // Both employees over the per-employee cap, and the total over the run cap, all at once.
    assert_eq!(body.matches("per-employee max").count(), 2, "{body}");
    assert!(body.contains("run total"), "{body}");
    let (code, body) = post(&d, "/api/plan", r#"{"label":"oct","max_total":"1","max_per":"0.5"}"#);
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"total_zat\":4000000") && body.contains("\"funding_zat\":4020000"), "{body}");
    assert!(body.contains("\"ledger\":\"not-started\""), "{body}");
    let (code, body) = post(&d, "/api/plan", r#"{"label":"has space"}"#);
    assert_eq!(code, 422, "{body}");
}

#[test]
fn roster_upload_validates_and_replaces_the_roster() {
    let d = start("roster", true);
    let (code, body) = post(&d, "/api/roster", r#"{"csv":"name,ua,amountZEC\nEve,utest1nope,0.1\nMallory,u1mainnet,0\n"}"#);
    assert_eq!(code, 422, "{body}");
    assert!(body.contains("line 2") && body.contains("line 3") && body.contains("nothing was loaded"), "{body}");
    let csv = std::fs::read_to_string(ROSTER).unwrap();
    let one: String = csv.lines().take(2).map(|l| format!("{l}\n")).collect();
    let req = format!("{{\"name\":\"one.csv\",\"csv\":{}}}", zec_payroll::server::json_str(&one));
    let (code, body) = post(&d, "/api/roster", &req);
    assert_eq!(code, 200, "{body}");
    let (_, state) = http(&d, "GET", "/api/state", None, "");
    assert!(state.contains("one.csv (uploaded in the desk)") && state.contains("Alice") && !state.contains("\"Bob\""), "{state}");
}

#[test]
fn run_streams_the_cli_and_stops_before_broadcast_when_a_wallet_is_not_ready() {
    let d = start("run", true);
    // CSRF: the same request without the desk header is refused before anything runs.
    let (code, _) = http(&d, "POST", "/api/payroll/run", Some(r#"{"label":"oct"}"#), "Content-Type: application/json\r\n");
    assert_eq!(code, 403);
    let (code, body) = post(&d, "/api/payroll/run", r#"{"label":"oct","max_total":"1"}"#);
    assert_eq!(code, 202, "{body}");
    let job = wait_job(&d);
    // The CLI's own pre-broadcast check failed (wallet CLI is `false`): exit 1, step 1 NOT sent.
    assert!(job.contains("\"exit\":1") && job.contains("step 1 NOT sent"), "{job}");
    assert!(job.contains("\"error\":\"step-2 wallet for row2 not ready; step 1 NOT sent"), "{job}");
    // The period's roster snapshot exists, and nothing was funded or paid.
    assert!(d.db.join("desk/oct.csv").exists());
    let (code, ledger) = http(&d, "GET", "/api/payroll/ledger?label=oct", None, "");
    assert_eq!(code, 200, "{ledger}");
    assert_eq!(ledger.matches("\"state\":\"not-started\"").count(), 2, "{ledger}");
    let (_, state) = http(&d, "GET", "/api/state", None, "");
    assert!(state.contains("\"periods\":[\"oct\"]"), "{state}");
    // The same period with a different roster is refused; a rerun with the same one is allowed.
    let csv = std::fs::read_to_string(ROSTER).unwrap().replace("0.02,", "0.03,");
    let (code, _) = post(&d, "/api/roster", &format!("{{\"csv\":{}}}", zec_payroll::server::json_str(&csv)));
    assert_eq!(code, 200);
    let (code, body) = post(&d, "/api/payroll/run", r#"{"label":"oct"}"#);
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("different roster") && body.contains("Nothing sent"), "{body}");
    // No payout recorded: the proof export refuses (the CLI's own message).
    let (code, body) = post(&d, "/api/payroll/proof", r#"{"label":"oct","employee":"Alice"}"#);
    assert_eq!(code, 422, "{body}");
    assert!(body.contains("no `paid` payout"), "{body}");
}

#[test]
fn an_unknown_payout_outcome_is_only_cleared_by_a_successful_chain_check() {
    let d = start("resolve", true);
    let (code, _) = post(&d, "/api/payroll/run", r#"{"label":"nov"}"#);
    assert_eq!(code, 202);
    wait_job(&d);
    // Simulate the state a transport error leaves behind: funded, then a pay-pending with no outcome.
    let (_, ledger) = http(&d, "GET", "/api/payroll/ledger?label=nov", None, "");
    assert!(ledger.contains("not-started"), "{ledger}");
    let bob_addr = std::fs::read_to_string(ROSTER).unwrap().lines().find(|l| l.starts_with("Bob,")).unwrap().split(',').nth(1).unwrap().to_string();
    let bob_dir = d.db.join("children").join(zec_payroll::disclosure::child_dir_name(&bob_addr));
    std::fs::create_dir_all(&bob_dir).unwrap();
    let t = "ab".repeat(32);
    std::fs::write(bob_dir.join("payroll-ledger"), format!("fund-pending adhoc:nov -\nfunded adhoc:nov {t}\npay-pending adhoc:nov -\n")).unwrap();
    let (_, ledger) = http(&d, "GET", "/api/payroll/ledger?label=nov", None, "");
    assert!(ledger.contains("\"state\":\"pay-pending\""), "{ledger}");
    // The rerun refuses (the CLI's own rule), before anything is sent.
    let (code, _) = post(&d, "/api/payroll/run", r#"{"label":"nov"}"#);
    assert_eq!(code, 202);
    let job = wait_job(&d);
    assert!(job.contains("no recorded outcome") && job.contains("Nothing sent"), "{job}");
    // Clearing it needs the on-chain check; the wallet CLI fails, so nothing is recorded.
    let who = "Bob";
    let (code, body) = post(&d, "/api/payroll/resolve", &format!("{{\"label\":\"nov\",\"employee\":\"{who}\"}}"));
    assert_eq!(code, 502, "{body}");
    let text = std::fs::read_to_string(bob_dir.join("payroll-ledger")).unwrap();
    assert!(!text.contains("pay-failed"), "a failed check must never clear the outcome: {text}");
    // An employee with nothing pending has nothing to check.
    let (code, body) = post(&d, "/api/payroll/check", r#"{"label":"nov","employee":"Alice"}"#);
    assert_eq!(code, 409, "{body}");
    assert!(body.contains("nothing to check"), "{body}");
}

#[test]
fn accountant_view_carries_the_employer_viewing_key_and_the_period_rows() {
    let d = start("acct", true);
    let (code, body) = http(&d, "GET", "/api/payroll/accountant?label=dec", None, "");
    assert_eq!(code, 404, "no period yet: {body}");
    post(&d, "/api/payroll/run", r#"{"label":"dec"}"#);
    wait_job(&d);
    let (code, body) = http(&d, "GET", "/api/payroll/accountant?label=dec", None, "");
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"ufvk\":\"uviewtest1") && body.contains("Alice") && body.contains("Bob"), "{body}");
    assert!(body.contains("\"total_zat\":4000000"), "{body}");
    let (code, _) = http(&d, "GET", "/api/payroll/accountant?label=../x", None, "");
    assert_eq!(code, 422);
}

#[test]
fn verify_refuses_a_bad_proof_before_any_network_call() {
    let d = start("verify", true);
    let txid = "cd".repeat(32);
    let (code, body) = post(&d, "/api/payroll/verify", &format!("{{\"proof\":\"not a proof\",\"txid\":\"{txid}\"}}"));
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"ok\":false") && body.contains("\"exit\":1"), "{body}");
    let (code, _) = post(&d, "/api/payroll/verify", r#"{"proof":"x","txid":"short"}"#);
    assert_eq!(code, 422);
}
