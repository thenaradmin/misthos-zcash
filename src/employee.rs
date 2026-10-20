//! Employee records and recipient-address checks.
//!
//! NOTE: `check_testnet_address` is a structural pre-check only (HRP + bech32
//! charset + length). Full Unified Address decoding (F4Jumble, receiver
//! typecodes, checksum) is milestone M1 and will use the `zcash_address`
//! crate once its version is verified on crates.io.

use crate::money::Zat;
use std::fmt;

/// ZIP-321 caps memos at 512 bytes.
pub const MAX_MEMO_BYTES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Employee {
    pub id: String,
    pub name: String,
    /// Testnet Unified Address (`utest1...`).
    pub address: String,
    /// Gross pay per scheduled period.
    pub pay: Zat,
    pub memo: String,
    pub active: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EmployeeError {
    EmptyId,
    EmptyName,
    MainnetAddress(String),
    NotUnifiedTestnet(String),
    BadCharset(String),
    MemoTooLong { id: String, bytes: usize },
}

impl fmt::Display for EmployeeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmployeeError::EmptyId => write!(f, "employee id is empty"),
            EmployeeError::EmptyName => write!(f, "employee name is empty"),
            EmployeeError::MainnetAddress(a) => {
                write!(f, "address `{a}` is a MAINNET address; this rail is testnet-only")
            }
            EmployeeError::NotUnifiedTestnet(a) => {
                write!(f, "address `{a}` is not a testnet Unified Address (expected `utest1...`)")
            }
            EmployeeError::BadCharset(a) => write!(f, "address `{a}` has non-bech32 characters"),
            EmployeeError::MemoTooLong { id, bytes } => {
                write!(f, "memo for `{id}` is {bytes} bytes (max {MAX_MEMO_BYTES})")
            }
        }
    }
}

impl std::error::Error for EmployeeError {}

const BECH32_CHARSET: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

pub fn check_testnet_address(addr: &str) -> Result<(), EmployeeError> {
    let a = addr.trim();
    if a.starts_with("u1") || a.starts_with("zs1") || a.starts_with("t1") || a.starts_with("t3") {
        return Err(EmployeeError::MainnetAddress(a.to_string()));
    }
    let Some(data) = a.strip_prefix("utest1") else {
        return Err(EmployeeError::NotUnifiedTestnet(a.to_string()));
    };
    // A UA carries at least one receiver + 6-char checksum; anything this
    // short cannot be a real UA.
    if data.len() < 40 {
        return Err(EmployeeError::NotUnifiedTestnet(a.to_string()));
    }
    if !data.chars().all(|c| BECH32_CHARSET.contains(c)) {
        return Err(EmployeeError::BadCharset(a.to_string()));
    }
    Ok(())
}

impl Employee {
    pub fn validate(&self) -> Result<(), EmployeeError> {
        if self.id.trim().is_empty() {
            return Err(EmployeeError::EmptyId);
        }
        if self.name.trim().is_empty() {
            return Err(EmployeeError::EmptyName);
        }
        check_testnet_address(&self.address)?;
        if self.memo.len() > MAX_MEMO_BYTES {
            return Err(EmployeeError::MemoTooLong { id: self.id.clone(), bytes: self.memo.len() });
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Shape-valid placeholder used only by offline unit tests of the engine;
    /// never broadcast. Real recipients come from real testnet wallets.
    pub fn ua(tag: char) -> String {
        format!("utest1{}", std::iter::repeat(tag).take(60).collect::<String>())
    }

    pub fn emp(id: &str, zec: &str) -> Employee {
        Employee {
            id: id.into(),
            name: format!("Name {id}"),
            address: ua('q'),
            pay: Zat::parse_zec(zec).unwrap(),
            memo: format!("salary {id}"),
            active: true,
        }
    }

    #[test]
    fn address_checks() {
        assert!(check_testnet_address(&ua('p')).is_ok());
        assert!(matches!(check_testnet_address("u1abc"), Err(EmployeeError::MainnetAddress(_))));
        assert!(matches!(check_testnet_address("zs1abc"), Err(EmployeeError::MainnetAddress(_))));
        assert!(matches!(check_testnet_address("ztestsapling1xyz"), Err(EmployeeError::NotUnifiedTestnet(_))));
        assert!(matches!(check_testnet_address("utest1short"), Err(EmployeeError::NotUnifiedTestnet(_))));
        let bad = format!("utest1{}", "b".repeat(60)); // 'b' is not in bech32 charset
        assert!(matches!(check_testnet_address(&bad), Err(EmployeeError::BadCharset(_))));
    }

    #[test]
    fn memo_limit() {
        let mut e = emp("a", "1");
        e.memo = "x".repeat(MAX_MEMO_BYTES + 1);
        assert!(matches!(e.validate(), Err(EmployeeError::MemoTooLong { .. })));
        e.memo = "x".repeat(MAX_MEMO_BYTES);
        assert!(e.validate().is_ok());
    }
}
