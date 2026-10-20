//! Employer CSV import: header `name,ua,amountZEC,memo` (memo optional).
//! Supports RFC-4180 quoting (`"a, b"`, `""` escapes). Every bad row is
//! reported with its line number; nothing is partially imported.

use crate::employee::Employee;
use crate::money::Zat;

pub fn split_line(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if cur.trim().is_empty() => {
                cur.clear();
                quoted = true;
            }
            (',', false) => out.push(std::mem::take(&mut cur).trim().to_string()),
            (c, _) => cur.push(c),
        }
    }
    if quoted {
        return Err("unterminated quote".into());
    }
    out.push(cur.trim().to_string());
    Ok(out)
}

/// Parse a payroll CSV into validated employees. Ids are `row<N>` unless a
/// name repeats, which is rejected (likely a double-pay mistake).
pub fn parse_payroll_csv(text: &str) -> Result<Vec<Employee>, Vec<String>> {
    let mut errs = Vec::new();
    let mut rows = text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty());
    let Some((_, header)) = rows.next() else {
        return Err(vec!["CSV is empty".into()]);
    };
    let header: Vec<String> = match split_line(header) {
        Ok(h) => h.into_iter().map(|s| s.to_ascii_lowercase()).collect(),
        Err(e) => return Err(vec![format!("line 1: {e}")]),
    };
    let col = |n: &str| header.iter().position(|h| h == n);
    let (Some(ci_name), Some(ci_ua), Some(ci_amt)) = (col("name"), col("ua"), col("amountzec")) else {
        return Err(vec![format!("header must contain name,ua,amountZEC[,memo]; got `{}`", header.join(","))]);
    };
    let ci_memo = col("memo");

    let mut out: Vec<Employee> = Vec::new();
    for (idx, line) in rows {
        let ln = idx + 1;
        let f = match split_line(line) {
            Ok(f) => f,
            Err(e) => {
                errs.push(format!("line {ln}: {e}"));
                continue;
            }
        };
        let get = |i: usize| f.get(i).cloned().unwrap_or_default();
        let pay = match Zat::parse_zec(&get(ci_amt)) {
            Ok(p) => p,
            Err(e) => {
                errs.push(format!("line {ln}: {e}"));
                continue;
            }
        };
        let e = Employee {
            id: format!("row{ln}"),
            name: get(ci_name),
            address: get(ci_ua),
            pay,
            memo: ci_memo.map(get).unwrap_or_default(),
            active: true,
        };
        if let Err(err) = e.validate() {
            errs.push(format!("line {ln}: {err}"));
            continue;
        }
        if out.iter().any(|o| o.name.eq_ignore_ascii_case(&e.name)) {
            errs.push(format!("line {ln}: duplicate name `{}`", e.name));
            continue;
        }
        out.push(e);
    }
    if out.is_empty() && errs.is_empty() {
        errs.push("CSV has a header but no rows".into());
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::employee::tests::ua;

    #[test]
    fn quoting() {
        assert_eq!(split_line(r#"a, "b, c" ,"say ""hi""""#).unwrap(), vec!["a", "b, c", r#"say "hi""#]);
        assert!(split_line(r#""open"#).is_err());
    }

    #[test]
    fn parses_good_csv() {
        let csv = format!("name,ua,amountZEC,memo\nAlice,{},1.5,\"Oct, salary\"\n\nBob,{},0.2,\n", ua('p'), ua('z'));
        let v = parse_payroll_csv(&csv).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].memo, "Oct, salary");
        assert_eq!(v[1].pay, Zat(20_000_000));
        assert_eq!(v[1].id, "row4");
    }

    #[test]
    fn reports_every_bad_row() {
        let csv = format!(
            "name,ua,amountZEC\nA,u1mainnet,1\nB,{u},-2\nC,{u},1\nc,{u},1\n,{u},1\n",
            u = ua('q')
        );
        let errs = parse_payroll_csv(&csv).unwrap_err();
        assert_eq!(errs.len(), 4, "{errs:?}");
        assert!(errs[0].starts_with("line 2:") && errs[0].contains("MAINNET"));
        assert!(errs[1].starts_with("line 3:"));
        assert!(errs[2].contains("duplicate name"));
        assert!(errs[3].contains("name is empty"));
    }

    #[test]
    fn bad_header_and_empty() {
        assert!(parse_payroll_csv("").is_err());
        assert!(parse_payroll_csv("name,address,amount\n").unwrap_err()[0].contains("header"));
        assert!(parse_payroll_csv("name,ua,amountZEC\n").unwrap_err()[0].contains("no rows"));
    }
}
