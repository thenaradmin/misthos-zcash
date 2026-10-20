//! Employee records and recipient-address checks.
//!
//! `check_testnet_address` fully decodes the address with `zcash_address`
//! (bech32/bech32m checksum, F4Jumble, receiver typecodes), then requires the
//! TESTNET network and a shielded (Sapling or Orchard) receiver.

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
    NotShieldedTestnet(String),
    Invalid(String, String),
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
            EmployeeError::NotShieldedTestnet(a) => write!(
                f,
                "address `{a}` is not a shielded testnet address (need a `utest1...` UA with a Sapling/Orchard receiver, or `ztestsapling1...`)"
            ),
            EmployeeError::Invalid(a, e) => write!(f, "address `{a}` is not a valid Zcash address: {e}"),
            EmployeeError::MemoTooLong { id, bytes } => {
                write!(f, "memo for `{id}` is {bytes} bytes (max {MAX_MEMO_BYTES})")
            }
        }
    }
}

impl std::error::Error for EmployeeError {}

use zcash_address::{unified, ConversionError, TryFromAddress, ZcashAddress};
use zcash_protocol::{consensus::NetworkType, PoolType};

/// Decoded network of an address; the payload itself is not needed here.
struct Net(NetworkType);

impl TryFromAddress for Net {
    type Error = std::convert::Infallible;
    fn try_from_sapling(net: NetworkType, _: [u8; 43]) -> Result<Self, ConversionError<Self::Error>> {
        Ok(Net(net))
    }
    fn try_from_unified(net: NetworkType, _: unified::Address) -> Result<Self, ConversionError<Self::Error>> {
        Ok(Net(net))
    }
    fn try_from_transparent_p2pkh(net: NetworkType, _: [u8; 20]) -> Result<Self, ConversionError<Self::Error>> {
        Ok(Net(net))
    }
    fn try_from_transparent_p2sh(net: NetworkType, _: [u8; 20]) -> Result<Self, ConversionError<Self::Error>> {
        Ok(Net(net))
    }
    fn try_from_tex(net: NetworkType, _: [u8; 20]) -> Result<Self, ConversionError<Self::Error>> {
        Ok(Net(net))
    }
}

pub fn check_testnet_address(addr: &str) -> Result<(), EmployeeError> {
    let a = addr.trim();
    let parsed = ZcashAddress::try_from_encoded(a).map_err(|e| EmployeeError::Invalid(a.to_string(), e.to_string()))?;
    let Net(net) = parsed
        .clone()
        .convert::<Net>()
        .map_err(|e| EmployeeError::Invalid(a.to_string(), e.to_string()))?;
    if net == NetworkType::Main {
        return Err(EmployeeError::MainnetAddress(a.to_string()));
    }
    if net != NetworkType::Test {
        return Err(EmployeeError::NotShieldedTestnet(a.to_string()));
    }
    if !(parsed.can_receive_as(PoolType::SAPLING) || parsed.can_receive_as(PoolType::ORCHARD)) {
        return Err(EmployeeError::NotShieldedTestnet(a.to_string()));
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

    use zcash_address::unified::{Encoding, Receiver, Revision, Uitem};
    use zcash_address::ToAddress;

    /// Correctly encoded testnet UA (Orchard receiver; bytes derived from `tag`).
    /// Used only by offline unit tests; never broadcast.
    pub fn ua(tag: char) -> String {
        let r = Receiver::Orchard([tag as u8; 43]);
        let ua = unified::Address::try_from_items(Revision::R0, vec![Uitem::Data(r)]).unwrap();
        ZcashAddress::from_unified(NetworkType::Test, ua).encode()
    }

    /// Correctly encoded MAINNET UA, to prove mainnet is refused.
    pub fn main_ua() -> String {
        let ua = unified::Address::try_from_items(Revision::R0, vec![Uitem::Data(Receiver::Orchard([1; 43]))]).unwrap();
        ZcashAddress::from_unified(NetworkType::Main, ua).encode()
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
        let good = ua('p');
        assert!(good.starts_with("utest1"));
        assert!(check_testnet_address(&good).is_ok());
        let sap = ZcashAddress::from_sapling(NetworkType::Test, [7; 43]).encode();
        assert!(check_testnet_address(&sap).is_ok());
        // Mainnet encodings of real receivers are rejected as mainnet.
        assert!(matches!(check_testnet_address(&main_ua()), Err(EmployeeError::MainnetAddress(_))));
        // Transparent testnet is not private payroll.
        let t = ZcashAddress::from_transparent_p2pkh(NetworkType::Test, [3; 20]).encode();
        assert!(matches!(check_testnet_address(&t), Err(EmployeeError::NotShieldedTestnet(_))));
        // The old structural check accepted these; they must now fail.
        let shape_only = format!("utest1{}", "q".repeat(60));
        assert!(matches!(check_testnet_address(&shape_only), Err(EmployeeError::Invalid(..))));
        let mut flipped: Vec<char> = good.chars().collect();
        let i = flipped.len() - 3;
        flipped[i] = if flipped[i] == 'q' { 'p' } else { 'q' };
        let flipped: String = flipped.into_iter().collect();
        assert!(matches!(check_testnet_address(&flipped), Err(EmployeeError::Invalid(..))));
        assert!(check_testnet_address("u1abc").is_err());
        assert!(check_testnet_address("ztestsapling1xyz").is_err());
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
