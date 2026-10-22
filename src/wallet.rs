//! Chain I/O: hands an Approved run's ZIP-321 URI to a real testnet wallet CLI
//! that talks to lightwalletd. No in-process fallback, no simulated txids.
//!
//! The wallet binary is `ZCASH_WALLET_CLI` (zingo-cli). Syntax checked against
//! zingolib `zingo-cli` source at 0a5cb69 (2026-10-19):
//! - session options (`--chain/--server/--data-dir/--waitsync`) go BEFORE the command;
//! - `send`/`quicksend` take `<addr> <zatoshis> [memo]` or ONE JSON array
//!   `[{"address":..,"amount":<zatoshis>,"memo":..}]` -- NOT a ZIP-321 URI;
//! - a `send` proposal lives only in that process, so `send` then `confirm` as
//!   two one-shot invocations can never broadcast. We use one `quicksend`;
//! - one-shot commands exit non-zero on error; success prints `{"txids":[..]}`.

use crate::config::{ChainConfig, ConfigError};
use crate::engine::PayLine;
use std::process::Command;

#[derive(Debug)]
pub enum WalletError {
    Config(ConfigError),
    Spawn { bin: String, err: String },
    Cli { code: Option<i32>, stderr: String },
    NoTxid(String),
    Empty,
}

impl std::fmt::Display for WalletError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalletError::Config(e) => write!(f, "{e}"),
            WalletError::Spawn { bin, err } => write!(f, "cannot run ZCASH_WALLET_CLI `{bin}`: {err}"),
            WalletError::Cli { code, stderr } => write!(f, "wallet CLI exited {code:?}: {stderr}"),
            WalletError::NoTxid(out) => write!(f, "wallet CLI printed no `txids` array; output: {out}"),
            WalletError::Empty => write!(f, "refusing to send a payout with no recipients"),
        }
    }
}

impl std::error::Error for WalletError {}

pub fn wallet_cli(get: impl Fn(&str) -> Option<String>) -> Result<String, WalletError> {
    get("ZCASH_WALLET_CLI")
        .filter(|v| !v.trim().is_empty())
        .ok_or(WalletError::Config(ConfigError::Missing("ZCASH_WALLET_CLI")))
}

fn run(bin: &str, cfg: &ChainConfig, cmd: &[&str]) -> Result<String, WalletError> {
    run_in(bin, cfg, &cfg.wallet_db, None, cmd)
}

/// Run zingo-cli against `data_dir`. A seed for a first-time restore goes through the
/// `ZINGO_SEED` env var (never argv, so it is not in the process list).
fn run_in(bin: &str, cfg: &ChainConfig, data_dir: &str, seed: Option<(&str, u32)>, cmd: &[&str]) -> Result<String, WalletError> {
    let mut c = Command::new(bin);
    c.args(["--chain", "testnet", "--server", &cfg.lwd_url, "--data-dir", data_dir, "--waitsync"]);
    c.env_remove("ZINGO_SEED");
    if let Some((phrase, birthday)) = seed {
        c.env("ZINGO_SEED", phrase).args(["--birthday", &birthday.to_string()]);
    }
    let out = c
        .args(cmd)
        .output()
        .map_err(|e| WalletError::Spawn { bin: bin.into(), err: e.to_string() })?;
    if !out.status.success() {
        let mut msg = String::from_utf8_lossy(&out.stderr).into_owned();
        msg.push_str(&String::from_utf8_lossy(&out.stdout));
        return Err(WalletError::Cli { code: out.status.code(), stderr: msg });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into())
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// zingo-cli JSON receiver list: amounts in zatoshis, memo only when non-empty.
pub fn receivers_json(lines: &[PayLine]) -> String {
    let items: Vec<String> = lines
        .iter()
        .map(|l| {
            let memo = if l.memo.is_empty() { String::new() } else { format!(",\"memo\":{}", json_str(&l.memo)) };
            format!("{{\"address\":{},\"amount\":{}{memo}}}", json_str(&l.address), l.amount.0)
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// Txids from zingo-cli's `{"txids":[...]}` output. Only that array counts, so a
/// stray 64-hex string (block hash, log line) is never mistaken for a txid.
pub fn extract_txids(out: &str) -> Option<Vec<String>> {
    let rest = &out[out.find("\"txids\"")? + 7..];
    let rest = &rest[rest.find('[')? + 1..];
    let body = &rest[..rest.find(']')?];
    let ids: Vec<String> = body
        .split(',')
        .map(|t| t.trim().trim_matches('"').to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if ids.is_empty() || ids.iter().any(|t| t.len() != 64 || !t.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    Some(ids)
}

/// Sync, then build + broadcast one proposal paying every line. Returns the txids
/// (normally one; zingolib may split a proposal into several steps).
pub fn send_payout(bin: &str, cfg: &ChainConfig, lines: &[PayLine]) -> Result<Vec<String>, WalletError> {
    send_payout_steps(bin, cfg, lines, |_| {})
}

/// As [`send_payout`], reporting each wallet step before it runs (for the UI feed).
pub fn send_payout_steps(
    bin: &str,
    cfg: &ChainConfig,
    lines: &[PayLine],
    mut step: impl FnMut(&str),
) -> Result<Vec<String>, WalletError> {
    if lines.is_empty() {
        return Err(WalletError::Empty);
    }
    step("wallet: sync to tip, then quicksend (propose + build + broadcast)");
    let out = run(bin, cfg, &["quicksend", &receivers_json(lines)])?;
    extract_txids(&out).ok_or(WalletError::NoTxid(out))
}

/// Chain tip height reported by the wallet via lightwalletd.
pub fn chain_height(bin: &str, cfg: &ChainConfig) -> Result<String, WalletError> {
    run(bin, cfg, &["height"])
}

/// Where an employee's child wallet lives: `<ZCASH_WALLET_DB>/children/<child-dir-name>`.
pub fn child_data_dir(cfg: &ChainConfig, payout_address: &str) -> String {
    format!("{}/children/{}", cfg.wallet_db.trim_end_matches('/'), crate::disclosure::child_dir_name(payout_address))
}

/// Pre-broadcast check for step 2: create (first time, restoring from the child
/// mnemonic at `birthday`) or open the child wallet and sync it to the tip. Any
/// failure here must stop the run BEFORE step 1 is broadcast.
pub fn init_child(
    bin: &str,
    cfg: &ChainConfig,
    child: &crate::disclosure::ChildWallet,
    payout_address: &str,
    birthday: u32,
) -> Result<String, WalletError> {
    let dir = child_data_dir(cfg, payout_address);
    let fresh = std::fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(true);
    std::fs::create_dir_all(&dir).map_err(|e| WalletError::Spawn { bin: bin.into(), err: format!("{dir}: {e}") })?;
    {
        use std::os::unix::fs::PermissionsExt;
        let p = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(&dir, p).map_err(|e| WalletError::Spawn { bin: bin.into(), err: format!("chmod {dir}: {e}") })?;
    }
    let seed = fresh.then(|| (child.phrase_for_wallet_env(), birthday));
    run_in(bin, cfg, &dir, seed, &["balance"])
}

/// Step 2: one ordinary `quicksend` from the child wallet to the employee.
pub fn send_from_child(bin: &str, cfg: &ChainConfig, line: &PayLine) -> Result<Vec<String>, WalletError> {
    let dir = child_data_dir(cfg, &line.address);
    let out = run_in(bin, cfg, &dir, None, &["quicksend", &receivers_json(std::slice::from_ref(line))])?;
    extract_txids(&out).ok_or(WalletError::NoTxid(out))
}

/// Durable per-child ledger (`<child dir>/payroll-ledger`) so a rerun or restart of
/// `pay --two-step` never re-funds or re-pays. Lines: `<state> <period> [txid]`, where
/// state is `fund-pending`, `funded`, `pay-pending` or `paid`. A `*-pending` line with no
/// matching completion means a broadcast may or may not have happened: the run refuses.
/// `pay-at <period> <height>` is the chosen payout height (delay); it never changes the
/// step, and the FIRST one per period wins, so a rerun reuses it instead of redrawing.
pub mod ledger {
    use std::io::Write;
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub enum Step { Fresh, FundPending, Funded, PayPending, Paid }
    fn path(dir: &str) -> String { format!("{dir}/payroll-ledger") }

    pub const LOCK_FILE: &str = "payroll.lock";

    /// Exclusive lock on `<wallet_db>/payroll.lock` for one `pay --two-step` run. Hold the
    /// returned file until the process exits: two runs reading the ledgers at once would both
    /// see `Fresh` and both fund, and clearing `fund-pending` (read, filter, rename) could drop
    /// the other run's appended lines.
    pub fn lock_wallet_db(wallet_db: &str) -> Result<std::fs::File, String> {
        std::fs::create_dir_all(wallet_db).map_err(|e| format!("ZCASH_WALLET_DB {wallet_db}: {e}; nothing sent"))?;
        let p = format!("{wallet_db}/{LOCK_FILE}");
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&p).map_err(|e| format!("{p}: {e}; nothing sent"))?;
        match f.try_lock() {
            Ok(()) => Ok(f),
            Err(std::fs::TryLockError::WouldBlock) => Err(format!("another pay --two-step is running for this wallet DB ({p} is locked); nothing sent")),
            Err(std::fs::TryLockError::Error(e)) => Err(format!("{p}: cannot lock: {e}; nothing sent")),
        }
    }
    /// Third field of every `<what> <period> ...` line, in file order.
    fn values(dir: &str, what: &str, period: &str) -> Vec<String> {
        let text = read(dir);
        text.lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                (it.next() == Some(what) && it.next() == Some(period)).then(|| it.next().unwrap_or("").to_string())
            })
            .collect()
    }
    /// The persisted payout height for `period`, if one was chosen.
    pub fn target(dir: &str, period: &str) -> Option<u32> {
        values(dir, "pay-at", period).iter().find_map(|v| v.parse().ok())
    }
    /// Persist the payout height before waiting on it. Keeps an existing one.
    pub fn set_target(dir: &str, period: &str, height: u32) -> std::io::Result<u32> {
        if let Some(h) = target(dir, period) {
            return Ok(h);
        }
        record(dir, "pay-at", period, &height.to_string())?;
        Ok(height)
    }
    fn txids(dir: &str, what: &str, period: &str) -> Vec<String> {
        let last = values(dir, what, period).pop().unwrap_or_default();
        last.split(',').filter(|t| t.len() == 64).map(str::to_string).collect()
    }
    /// Txids of the funding tx(s) recorded for `period` (step 1).
    pub fn funding_txids(dir: &str, period: &str) -> Vec<String> { txids(dir, "funded", period) }
    /// Txids of the payout tx(s) recorded for `period` (step 2).
    pub fn paid_txids(dir: &str, period: &str) -> Vec<String> { txids(dir, "paid", period) }
    /// Ledger text; a missing file is an empty ledger. Any other read error (permissions,
    /// I/O, bad UTF-8) panics: treating it as empty would make a paid child look Fresh.
    fn read(dir: &str) -> String {
        match std::fs::read_to_string(path(dir)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => panic!("cannot read {}: {e}; refusing to guess the payout state", path(dir)),
        }
    }
    pub fn state(dir: &str, period: &str) -> Step {
        let text = read(dir);
        let mut st = Step::Fresh;
        for l in text.lines() {
            let mut it = l.split_whitespace();
            let (Some(k), Some(p)) = (it.next(), it.next()) else { continue };
            if p != period { continue }
            st = match k { "fund-pending" => Step::FundPending, "funded" => Step::Funded, "pay-pending" => Step::PayPending, "paid" => Step::Paid, "pay-failed" => Step::Funded, _ => st };
        }
        st
    }
    pub fn record(dir: &str, what: &str, period: &str, txid: &str) -> std::io::Result<()> {
        // Lines are whitespace-split: a period with a space would never match itself on read,
        // so every rerun would look Fresh and fund again.
        if period.is_empty() || period.contains(char::is_whitespace) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("period {period:?} is empty or contains whitespace")));
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path(dir))?;
        writeln!(f, "{what} {period} {txid}")?;
        f.sync_all()
    }
}

/// zingo-cli errors that come from proposing, i.e. before anything is broadcast.
pub fn failed_before_broadcast(e: &WalletError) -> bool {
    let s = e.to_string();
    s.contains("Propose send error") || s.contains("Insufficient balance")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_missing_wallet_cli() {
        assert_eq!(wallet_cli(|_| None).unwrap_err().to_string(), "ZCASH_WALLET_CLI not configured");
    }

    #[test]
    fn finds_txids_only_in_txids_array() {
        let t = "a".repeat(64);
        assert_eq!(extract_txids(&format!("{{\n  \"txids\": [\n    \"{t}\"\n  ]\n}}")), Some(vec![t.clone()]));
        // A bare hex string (e.g. a block hash in a log line) is not a txid.
        assert_eq!(extract_txids(&format!("synced to block {t}")), None);
        assert_eq!(extract_txids("{\"txids\": []}"), None);
        assert_eq!(extract_txids("{\"txids\": [\"abc\"]}"), None);
    }

    #[test]
    fn receivers_json_shape() {
        let l = |a: &str, z: u64, m: &str| PayLine { employee_id: "e".into(), name: "n".into(), address: a.into(), amount: crate::money::Zat(z), memo: m.into() };
        let j = receivers_json(&[l("utest1x", 150_000_000, "Oct \"pay\""), l("utest1y", 1, "")]);
        assert_eq!(j, r#"[{"address":"utest1x","amount":150000000,"memo":"Oct \"pay\""},{"address":"utest1y","amount":1}]"#);
    }

    #[test]
    fn failing_cli_is_an_error_not_success() {
        let cfg = ChainConfig { network: "testnet".into(), lwd_url: "https://x".into(), wallet_db: "/tmp/none".into() };
        let l = PayLine { employee_id: "e".into(), name: "n".into(), address: "utest1x".into(), amount: crate::money::Zat(1), memo: String::new() };
        assert!(matches!(send_payout("false", &cfg, std::slice::from_ref(&l)), Err(WalletError::Cli { .. })));
        // exit 0 but no txids => NoTxid, never a fake success
        assert!(matches!(send_payout("true", &cfg, &[l]), Err(WalletError::NoTxid(_))));
        assert!(matches!(send_payout("true", &cfg, &[]), Err(WalletError::Empty)));
    }

    #[test]
    fn second_two_step_run_cannot_take_the_wallet_db_lock() {
        let d = std::env::temp_dir().join(format!("zpp-lock-{}", std::process::id()));
        let ds = d.to_str().unwrap().to_string();
        let held = ledger::lock_wallet_db(&ds).unwrap();
        // Without the lock both runs would read Fresh for the same child and both fund it.
        assert_eq!(ledger::state(&ds, "p1"), ledger::Step::Fresh);
        let e = ledger::lock_wallet_db(&ds).unwrap_err();
        assert!(e.contains("another pay --two-step is running") && e.contains("nothing sent"), "{e}");
        drop(held);
        assert!(ledger::lock_wallet_db(&ds).is_ok(), "lock is released when the first run exits");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unreadable_ledger_is_not_treated_as_fresh() {
        let d = std::env::temp_dir().join(format!("zpp-ledger-bad-{}", std::process::id()));
        std::fs::create_dir_all(d.join("payroll-ledger")).unwrap(); // a dir: read fails, not NotFound
        let ds = d.to_str().unwrap().to_string();
        let r = std::panic::catch_unwind(|| ledger::state(&ds, "p1"));
        assert!(r.is_err(), "an unreadable ledger must stop the run, not read as Fresh");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn ledger_blocks_refund_and_repay() {
        use ledger::{record, state, Step};
        let d = std::env::temp_dir().join(format!("zpp-ledger-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let d = d.to_str().unwrap();
        assert_eq!(state(d, "p1"), Step::Fresh);
        record(d, "fund-pending", "p1", "-").unwrap();
        assert_eq!(state(d, "p1"), Step::FundPending);
        record(d, "funded", "p1", "t1").unwrap();
        record(d, "pay-pending", "p1", "-").unwrap();
        assert_eq!(state(d, "p1"), Step::PayPending);
        record(d, "pay-failed", "p1", "-").unwrap();
        assert_eq!(state(d, "p1"), Step::Funded);
        record(d, "pay-pending", "p1", "-").unwrap();
        record(d, "paid", "p1", "t2").unwrap();
        assert_eq!(state(d, "p1"), Step::Paid);
        assert_eq!(state(d, "p2"), Step::Fresh);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn persisted_delay_target_is_reused_and_never_changes_the_step() {
        use ledger::{funding_txids, paid_txids, record, set_target, state, target, Step};
        let d = std::env::temp_dir().join(format!("zpp-ledger-target-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let d = d.to_str().unwrap();
        let (t1, t2) = ("1".repeat(64), "2".repeat(64));
        record(d, "fund-pending", "p1", "-").unwrap();
        record(d, "funded", "p1", &t1).unwrap();
        assert_eq!(target(d, "p1"), None);
        assert_eq!(set_target(d, "p1", 1005).unwrap(), 1005);
        // A rerun that drew a different delay keeps the first persisted height.
        assert_eq!(set_target(d, "p1", 1009).unwrap(), 1005);
        assert_eq!(target(d, "p1"), Some(1005));
        // Waiting on the delay is still `Funded`: never Fresh (no re-funding), never Paid.
        assert_eq!(state(d, "p1"), Step::Funded);
        assert_eq!(funding_txids(d, "p1"), vec![t1.clone()]);
        assert!(paid_txids(d, "p1").is_empty());
        record(d, "pay-pending", "p1", "-").unwrap();
        record(d, "paid", "p1", &t2).unwrap();
        assert_eq!(state(d, "p1"), Step::Paid);
        assert_eq!(paid_txids(d, "p1"), vec![t2]);
        // Another period has its own (absent) target.
        assert_eq!(target(d, "p2"), None);
        // A period with whitespace could never be read back (rerun = re-fund): refused.
        assert!(record(d, "fund-pending", "adhoc:/Volumes/Extreme SSD/pay.csv", "-").is_err());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn child_init_refuses_on_cli_failure_and_keeps_seed_out_of_argv() {
        let dir = std::env::temp_dir().join(format!("zpp-child-{}", std::process::id()));
        let cfg = ChainConfig { network: "testnet".into(), lwd_url: "https://x".into(), wallet_db: dir.display().to_string() };
        let seed = crate::disclosure::EmployerSeed::from_bytes(&[4u8; 32]);
        let child = crate::disclosure::child_wallet(&seed, "utest1x").unwrap();
        // An unusable wallet CLI is an error (the caller then refuses to send step 1).
        assert!(init_child("false", &cfg, &child, "utest1x", 1).is_err());
        // `sh -c` echoes argv and whether ZINGO_SEED is set; the phrase must not be in argv.
        std::fs::remove_dir_all(&dir).ok();
        let probe = dir.with_extension("sh");
        std::fs::write(&probe, "#!/bin/sh\necho \"$@\" seed_set=${ZINGO_SEED:+yes}\n").unwrap();
        std::process::Command::new("chmod").args(["+x"]).arg(&probe).status().unwrap();
        let out = init_child(probe.to_str().unwrap(), &cfg, &child, "utest1x", 7).unwrap();
        assert!(out.contains("seed_set=yes") && out.contains("--birthday 7"));
        assert!(!out.contains(child.phrase_for_wallet_env()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_file(&probe).ok();
    }
}
