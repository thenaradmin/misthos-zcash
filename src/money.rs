//! Exact ZEC amounts in zatoshis (1 ZEC = 100_000_000 zat). No floats anywhere.

use std::fmt;

pub const COIN: u64 = 100_000_000;
/// Consensus MAX_MONEY: 21,000,000 ZEC.
pub const MAX_MONEY: u64 = 21_000_000 * COIN;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Zat(pub u64);

#[derive(Debug, PartialEq, Eq)]
pub enum AmountError {
    Empty,
    Invalid(String),
    TooManyDecimals(String),
    Zero,
    ExceedsMaxMoney(String),
}

impl fmt::Display for AmountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AmountError::Empty => write!(f, "amount is empty"),
            AmountError::Invalid(s) => write!(f, "amount `{s}` is not a decimal ZEC value"),
            AmountError::TooManyDecimals(s) => write!(f, "amount `{s}` has more than 8 decimals"),
            AmountError::Zero => write!(f, "amount must be greater than zero"),
            AmountError::ExceedsMaxMoney(s) => write!(f, "amount `{s}` exceeds 21,000,000 ZEC"),
        }
    }
}

impl std::error::Error for AmountError {}

impl Zat {
    /// Parse a decimal ZEC string like `"1.25"` or `"0.00000001"`. Rejects
    /// negatives, exponents, >8 decimals, zero and anything above MAX_MONEY.
    pub fn parse_zec(s: &str) -> Result<Zat, AmountError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(AmountError::Empty);
        }
        let (whole, frac) = match s.split_once('.') {
            Some((w, f)) => (w, f),
            None => (s, ""),
        };
        let digits_ok = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if (whole.is_empty() && frac.is_empty()) || !digits_ok(whole) || !digits_ok(frac) {
            return Err(AmountError::Invalid(s.to_string()));
        }
        if frac.len() > 8 {
            return Err(AmountError::TooManyDecimals(s.to_string()));
        }
        let too_big = || AmountError::ExceedsMaxMoney(s.to_string());
        let whole: u64 = if whole.is_empty() {
            0
        } else {
            whole.parse().map_err(|_| too_big())?
        };
        let frac_zat: u64 = if frac.is_empty() {
            0
        } else {
            format!("{frac:0<8}").parse().expect("8 ascii digits")
        };
        let total = whole
            .checked_mul(COIN)
            .and_then(|w| w.checked_add(frac_zat))
            .ok_or_else(too_big)?;
        if total == 0 {
            return Err(AmountError::Zero);
        }
        if total > MAX_MONEY {
            return Err(too_big());
        }
        Ok(Zat(total))
    }

    pub fn checked_add(self, o: Zat) -> Option<Zat> {
        self.0.checked_add(o.0).filter(|v| *v <= MAX_MONEY).map(Zat)
    }
}

impl fmt::Display for Zat {
    /// Renders as ZEC with trailing zeros trimmed, e.g. `1.5 ZEC`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / COIN;
        let frac = self.0 % COIN;
        if frac == 0 {
            write!(f, "{whole} ZEC")
        } else {
            let fs = format!("{frac:08}");
            write!(f, "{whole}.{} ZEC", fs.trim_end_matches('0'))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_decimals() {
        assert_eq!(Zat::parse_zec("1").unwrap(), Zat(COIN));
        assert_eq!(Zat::parse_zec("0.5").unwrap(), Zat(50_000_000));
        assert_eq!(Zat::parse_zec(".00000001").unwrap(), Zat(1));
        assert_eq!(Zat::parse_zec(" 2.10 ").unwrap(), Zat(210_000_000));
    }

    #[test]
    fn rejects_bad_amounts() {
        assert_eq!(Zat::parse_zec(""), Err(AmountError::Empty));
        assert!(matches!(Zat::parse_zec("-1"), Err(AmountError::Invalid(_))));
        assert!(matches!(Zat::parse_zec("1e3"), Err(AmountError::Invalid(_))));
        assert!(matches!(Zat::parse_zec("."), Err(AmountError::Invalid(_))));
        assert!(matches!(Zat::parse_zec("0.000000001"), Err(AmountError::TooManyDecimals(_))));
        assert_eq!(Zat::parse_zec("0.0"), Err(AmountError::Zero));
        assert!(matches!(Zat::parse_zec("21000000.00000001"), Err(AmountError::ExceedsMaxMoney(_))));
        assert!(matches!(Zat::parse_zec("99999999999999999999"), Err(AmountError::ExceedsMaxMoney(_))));
    }

    #[test]
    fn display_trims() {
        assert_eq!(Zat(150_000_000).to_string(), "1.5 ZEC");
        assert_eq!(Zat(COIN).to_string(), "1 ZEC");
        assert_eq!(Zat(1).to_string(), "0.00000001 ZEC");
    }
}
