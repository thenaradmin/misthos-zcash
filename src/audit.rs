//! Accountant export: a read-only ledger of pay runs plus the employer's UFVK.
//!
//! The UFVK lets the accountant import a view-only wallet and check every line
//! against the chain themselves. Spending keys are rejected outright.

use crate::{PayRun, RunStatus};

#[derive(Debug, PartialEq, Eq)]
pub enum AuditError {
    SpendingKey,
    NotTestnetUfvk,
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditError::SpendingKey => write!(f, "ZCASH_AUDITOR_UFVK looks like a spending key; refusing to export it"),
            AuditError::NotTestnetUfvk => write!(f, "ZCASH_AUDITOR_UFVK invalid: must be a testnet UFVK (`uviewtest1…`)"),
        }
    }
}

impl std::error::Error for AuditError {}

/// Testnet UFVKs use HRP `uviewtest` (ZIP-316). Anything with a spending-key
/// HRP (`secret-extended-key`, `usk`, raw seed words) is refused.
pub fn check_viewing_key(key: &str) -> Result<(), AuditError> {
    let k = key.trim();
    if k.contains("secret") || k.starts_with("usk") || k.split_whitespace().count() >= 12 {
        return Err(AuditError::SpendingKey);
    }
    if !k.starts_with("uviewtest1") || k.len() < 50 {
        return Err(AuditError::NotTestnetUfvk);
    }
    Ok(())
}

/// RFC 4180 field. A field a spreadsheet would run as a formula (leading `=`, `+`, `-`, `@`,
/// tab or CR) gets a leading `'` and is quoted, so it opens as text.
fn csv_field(s: &str) -> String {
    let formula = s.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let s = if formula { format!("'{s}") } else { s.to_string() };
    if formula || s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// CSV with one row per output. `txid`/`height` are blank until the run is broadcast/confirmed.
/// `line_index` is the row order in the run, NOT the on-chain output/action index
/// (wallets shuffle and pad shielded outputs).
pub fn export_csv(runs: &[PayRun], ufvk: &str) -> Result<String, AuditError> {
    check_viewing_key(ufvk)?;
    let mut out = format!("# network=testnet\n# ufvk={}\n", ufvk.trim());
    out.push_str("run_id,period,status,txid,height,line_index,employee_id,name,address,amount_zec,memo\n");
    for r in runs {
        let (status, txid, height) = match &r.status {
            RunStatus::Draft => ("draft", "", String::new()),
            RunStatus::Approved => ("approved", "", String::new()),
            RunStatus::Broadcast { txid } => ("broadcast", txid.as_str(), String::new()),
            RunStatus::Confirmed { txid, height } => ("confirmed", txid.as_str(), height.to_string()),
            RunStatus::Failed { .. } => ("failed", "", String::new()),
        };
        for (i, l) in r.lines.iter().enumerate() {
            let row = [
                r.id.to_string(),
                csv_field(&r.period_key),
                status.into(),
                txid.into(),
                height.clone(),
                i.to_string(),
                csv_field(&l.employee_id),
                csv_field(&l.name),
                l.address.clone(),
                l.amount.to_string(),
                csv_field(&l.memo),
            ];
            out.push_str(&row.join(","));
            out.push('\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_spending_material() {
        assert_eq!(check_viewing_key("secret-extended-key-test1qq"), Err(AuditError::SpendingKey));
        let seed = "abandon ".repeat(24);
        assert_eq!(check_viewing_key(&seed), Err(AuditError::SpendingKey));
        assert_eq!(check_viewing_key("uview1mainnetkeyxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"), Err(AuditError::NotTestnetUfvk));
    }

    #[test]
    fn quotes_fields_with_commas() {
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn neutralises_spreadsheet_formulas_and_quotes_cr() {
        assert_eq!(csv_field("=1+1"), "\"'=1+1\"");
        assert_eq!(csv_field("=HYPERLINK(\"http://x\",\"pay\")"), "\"'=HYPERLINK(\"\"http://x\"\",\"\"pay\"\")\"");
        for f in ["+1", "-1", "@SUM(A1)", "\tx", "\rx"] {
            let out = csv_field(f);
            assert!(out.starts_with("\"'"), "{f:?} -> {out:?}");
        }
        assert_eq!(csv_field("a\rb"), "\"a\rb\"");
        assert_eq!(csv_field("Alice"), "Alice");
    }
}
