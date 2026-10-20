//! Environment config. Missing or invalid values fail loudly, naming the var.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainConfig {
    pub network: String,
    pub lwd_url: String,
    pub wallet_db: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing(&'static str),
    Invalid { var: &'static str, why: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Missing(v) => write!(f, "{v} not configured"),
            ConfigError::Invalid { var, why } => write!(f, "{var} invalid: {why}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Reads via `get` so tests don't mutate the process environment.
pub fn load_with(get: impl Fn(&str) -> Option<String>) -> Result<ChainConfig, ConfigError> {
    let req = |k: &'static str| get(k).filter(|v| !v.trim().is_empty()).ok_or(ConfigError::Missing(k));
    let network = req("ZCASH_NETWORK")?;
    if network != "testnet" {
        return Err(ConfigError::Invalid {
            var: "ZCASH_NETWORK",
            why: format!("`{network}`; only `testnet` is allowed (no mainnet without explicit OK)"),
        });
    }
    let lwd_url = req("ZCASH_LWD_URL")?;
    if !lwd_url.starts_with("https://") {
        return Err(ConfigError::Invalid { var: "ZCASH_LWD_URL", why: "must start with https://".into() });
    }
    let wallet_db = req("ZCASH_WALLET_DB")?;
    Ok(ChainConfig { network, lwd_url, wallet_db })
}

pub fn load_from_env() -> Result<ChainConfig, ConfigError> {
    load_with(|k| std::env::var(k).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn names_the_missing_var() {
        let e = load_with(env(&[("ZCASH_NETWORK", "testnet")])).unwrap_err();
        assert_eq!(e.to_string(), "ZCASH_LWD_URL not configured");
        let e = load_with(env(&[])).unwrap_err();
        assert_eq!(e.to_string(), "ZCASH_NETWORK not configured");
    }

    #[test]
    fn refuses_mainnet_and_plain_http() {
        assert!(matches!(load_with(env(&[("ZCASH_NETWORK", "mainnet")])), Err(ConfigError::Invalid { var: "ZCASH_NETWORK", .. })));
        let e = load_with(env(&[("ZCASH_NETWORK", "testnet"), ("ZCASH_LWD_URL", "http://x:9067")])).unwrap_err();
        assert!(matches!(e, ConfigError::Invalid { var: "ZCASH_LWD_URL", .. }));
    }

    #[test]
    fn ok() {
        let c = load_with(env(&[
            ("ZCASH_NETWORK", "testnet"),
            ("ZCASH_LWD_URL", "https://lwd.example:443"),
            ("ZCASH_WALLET_DB", "./data/wallet.sqlite"),
        ]))
        .unwrap();
        assert_eq!(c.wallet_db, "./data/wallet.sqlite");
    }
}
