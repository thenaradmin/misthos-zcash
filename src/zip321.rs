//! ZIP-321 multi-payment URI for one pay run: one shielded tx, N outputs.
//! Spec: https://zips.z.cash/zip-0321 (indexed params `address.N`, `amount.N`,
//! `memo.N`; memo is base64url without padding; first payment has no index).

use crate::{PayRun, RunStatus};

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64url_nopad(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(B64URL[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// ZIP-321 amounts are decimal ZEC with at most 8 fractional digits, no trailing zeros needed.
fn zec_decimal(zat: u64) -> String {
    let (whole, frac) = (zat / 100_000_000, zat % 100_000_000);
    if frac == 0 {
        return whole.to_string();
    }
    format!("{whole}.{frac:08}").trim_end_matches('0').to_string()
}

#[derive(Debug, PartialEq, Eq)]
pub enum UriError {
    NotApproved(RunStatus),
    Empty,
    MemoTooLong { name: String, bytes: usize },
}

impl std::fmt::Display for UriError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UriError::NotApproved(s) => write!(f, "run must be Approved before payout (is {s:?})"),
            UriError::Empty => write!(f, "run has no pay lines"),
            UriError::MemoTooLong { name, bytes } => write!(f, "memo for {name} is {bytes} bytes; max 512"),
        }
    }
}

impl std::error::Error for UriError {}

pub fn payout_uri(run: &PayRun) -> Result<String, UriError> {
    if run.status != RunStatus::Approved {
        return Err(UriError::NotApproved(run.status.clone()));
    }
    if run.lines.is_empty() {
        return Err(UriError::Empty);
    }
    let mut params = Vec::new();
    for (i, l) in run.lines.iter().enumerate() {
        let sfx = if i == 0 { String::new() } else { format!(".{i}") };
        params.push(format!("address{sfx}={}", l.address));
        params.push(format!("amount{sfx}={}", zec_decimal(l.amount.0)));
        if !l.memo.is_empty() {
            if l.memo.len() > 512 {
                return Err(UriError::MemoTooLong { name: l.name.clone(), bytes: l.memo.len() });
            }
            params.push(format!("memo{sfx}={}", base64url_nopad(l.memo.as_bytes())));
        }
    }
    Ok(format!("zcash:?{}", params.join("&")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_rfc4648_vectors() {
        assert_eq!(base64url_nopad(b""), "");
        assert_eq!(base64url_nopad(b"f"), "Zg");
        assert_eq!(base64url_nopad(b"fo"), "Zm8");
        assert_eq!(base64url_nopad(b"foo"), "Zm9v");
        assert_eq!(base64url_nopad(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url_nopad(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn three_outputs_one_uri_and_draft_refused() {
        use crate::employee::tests::emp;
        use crate::{CapPolicy, Engine};
        let mut e = Engine::new(CapPolicy::default());
        let mut a = emp("a", "1.5");
        a.memo = "Oct salary".into();
        let (mut b, mut c) = (emp("b", "2"), emp("c", "0.00000001"));
        (b.memo, c.memo) = (String::new(), String::new());
        let id = e.plan_adhoc("oct", vec![a, b, c]).unwrap();
        assert_eq!(payout_uri(e.run(id).unwrap()), Err(UriError::NotApproved(RunStatus::Draft)));
        e.approve(id).unwrap();
        let uri = payout_uri(e.run(id).unwrap()).unwrap();
        let r = e.run(id).unwrap();
        let want = format!(
            "zcash:?address={}&amount=1.5&memo={}&address.1={}&amount.1=2&address.2={}&amount.2=0.00000001",
            r.lines[0].address, base64url_nopad(b"Oct salary"), r.lines[1].address, r.lines[2].address
        );
        assert_eq!(uri, want);
    }

    #[test]
    fn zec_decimals() {
        assert_eq!(zec_decimal(100_000_000), "1");
        assert_eq!(zec_decimal(150_000_000), "1.5");
        assert_eq!(zec_decimal(1), "0.00000001");
    }
}
