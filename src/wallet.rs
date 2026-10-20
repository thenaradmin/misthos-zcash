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
    let out = Command::new(bin)
        .args(["--chain", "testnet", "--server", &cfg.lwd_url, "--data-dir", &cfg.wallet_db, "--waitsync"])
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
}
