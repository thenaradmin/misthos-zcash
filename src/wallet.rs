//! Chain I/O: hands an Approved run's ZIP-321 URI to a real testnet wallet CLI
//! that talks to lightwalletd. No in-process fallback, no simulated txids.
//!
//! The wallet binary is `ZCASH_WALLET_CLI` (zingo-cli). Its exact `send`
//! syntax is NOT yet verified against current zingolib; a wrong invocation
//! surfaces as a non-zero exit with the CLI's stderr, never as a fake success.

use crate::config::{ChainConfig, ConfigError};
use std::process::Command;

#[derive(Debug)]
pub enum WalletError {
    Config(ConfigError),
    Spawn { bin: String, err: String },
    Cli { code: Option<i32>, stderr: String },
    NoTxid(String),
}

impl std::fmt::Display for WalletError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalletError::Config(e) => write!(f, "{e}"),
            WalletError::Spawn { bin, err } => write!(f, "cannot run ZCASH_WALLET_CLI `{bin}`: {err}"),
            WalletError::Cli { code, stderr } => write!(f, "wallet CLI exited {code:?}: {stderr}"),
            WalletError::NoTxid(out) => write!(f, "wallet CLI returned no 64-hex txid; output: {out}"),
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
        .args(["--chain", "testnet", "--server", &cfg.lwd_url, "--data-dir", &cfg.wallet_db])
        .args(cmd)
        .output()
        .map_err(|e| WalletError::Spawn { bin: bin.into(), err: e.to_string() })?;
    if !out.status.success() {
        return Err(WalletError::Cli { code: out.status.code(), stderr: String::from_utf8_lossy(&out.stderr).into() });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into())
}

/// First 64-char lowercase-hex token in wallet output.
pub fn extract_txid(out: &str) -> Option<String> {
    out.split(|c: char| !c.is_ascii_hexdigit())
        .find(|t| t.len() == 64)
        .map(str::to_ascii_lowercase)
}

/// Sync, then send one tx paying every output of the ZIP-321 URI. Returns the txid.
pub fn send_payout(bin: &str, cfg: &ChainConfig, zip321_uri: &str) -> Result<String, WalletError> {
    send_payout_steps(bin, cfg, zip321_uri, |_| {})
}

/// As [`send_payout`], reporting each wallet step before it runs (for the UI feed).
pub fn send_payout_steps(
    bin: &str,
    cfg: &ChainConfig,
    zip321_uri: &str,
    mut step: impl FnMut(&str),
) -> Result<String, WalletError> {
    step("wallet: sync run (lightwalletd)");
    run(bin, cfg, &["sync", "run"])?;
    step("wallet: send ZIP-321 proposal");
    run(bin, cfg, &["send", zip321_uri])?;
    step("wallet: confirm + broadcast");
    let out = run(bin, cfg, &["confirm"])?;
    extract_txid(&out).ok_or(WalletError::NoTxid(out))
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
    fn finds_txid() {
        let t = "a".repeat(64);
        assert_eq!(extract_txid(&format!("{{\"txids\":[\"{t}\"]}}")), Some(t));
        assert_eq!(extract_txid("no tx here 1234"), None);
    }
}
