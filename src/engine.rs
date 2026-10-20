//! Pay-run engine: plans runs from schedules (or a CSV batch), enforces
//! per-run caps, prevents double-paying a period, and tracks run status.
//!
//! Status machine:
//! `Draft -> Approved -> Broadcast{txid} -> Confirmed{height}`
//! `Approved | Broadcast -> Failed{reason}` (frees the period for a re-plan).

use crate::date::Date;
use crate::employee::{Employee, EmployeeError};
use crate::money::Zat;
use crate::schedule::Schedule;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapPolicy {
    /// Max sum of all outputs in one run.
    pub max_run_total: Option<Zat>,
    /// Max amount any single employee may receive in one run.
    pub max_per_employee: Option<Zat>,
    /// Max outputs in one run (one shielded tx). Policy choice, not a consensus rule.
    pub max_recipients: usize,
}

impl Default for CapPolicy {
    fn default() -> Self {
        CapPolicy { max_run_total: None, max_per_employee: None, max_recipients: 50 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayLine {
    pub employee_id: String,
    pub name: String,
    pub address: String,
    pub amount: Zat,
    pub memo: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Draft,
    Approved,
    Broadcast { txid: String },
    Confirmed { txid: String, height: u32 },
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayRun {
    pub id: u64,
    /// `schedule_id@YYYY-MM-DD` for scheduled runs, `adhoc:<label>` for CSV batches.
    pub period_key: String,
    pub lines: Vec<PayLine>,
    pub total: Zat,
    pub status: RunStatus,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EngineError {
    Employee(EmployeeError),
    DuplicateEmployee(String),
    DuplicateSchedule(String),
    UnknownEmployee(String),
    UnknownSchedule(String),
    UnknownRun(u64),
    NotAPayDate { schedule: String, date: Date },
    AlreadyPaid { period_key: String, run_id: u64 },
    EmptyRun,
    CapExceeded(Vec<String>),
    BadTransition { run_id: u64, from: String, to: &'static str },
    BadTxid(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use EngineError::*;
        match self {
            Employee(e) => write!(f, "{e}"),
            DuplicateEmployee(id) => write!(f, "employee `{id}` already exists"),
            DuplicateSchedule(id) => write!(f, "schedule `{id}` already exists"),
            UnknownEmployee(id) => write!(f, "unknown employee `{id}`"),
            UnknownSchedule(id) => write!(f, "unknown schedule `{id}`"),
            UnknownRun(id) => write!(f, "unknown run #{id}"),
            NotAPayDate { schedule, date } => write!(f, "{date} is not a pay date for `{schedule}`"),
            AlreadyPaid { period_key, run_id } => {
                write!(f, "period `{period_key}` already has run #{run_id}; refusing to double-pay")
            }
            EmptyRun => write!(f, "run has no active employees"),
            CapExceeded(v) => write!(f, "cap exceeded: {}", v.join("; ")),
            BadTransition { run_id, from, to } => write!(f, "run #{run_id}: cannot go from {from} to {to}"),
            BadTxid(t) => write!(f, "txid `{t}` is not 64 hex chars"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<EmployeeError> for EngineError {
    fn from(e: EmployeeError) -> Self {
        EngineError::Employee(e)
    }
}

fn status_name(s: &RunStatus) -> String {
    match s {
        RunStatus::Draft => "Draft",
        RunStatus::Approved => "Approved",
        RunStatus::Broadcast { .. } => "Broadcast",
        RunStatus::Confirmed { .. } => "Confirmed",
        RunStatus::Failed { .. } => "Failed",
    }
    .to_string()
}

#[derive(Default, Debug)]
pub struct Engine {
    pub caps: CapPolicy,
    employees: BTreeMap<String, Employee>,
    schedules: BTreeMap<String, Schedule>,
    runs: Vec<PayRun>,
}

impl Engine {
    pub fn new(caps: CapPolicy) -> Self {
        Engine { caps, ..Default::default() }
    }

    pub fn add_employee(&mut self, e: Employee) -> Result<(), EngineError> {
        e.validate()?;
        if self.employees.contains_key(&e.id) {
            return Err(EngineError::DuplicateEmployee(e.id));
        }
        self.employees.insert(e.id.clone(), e);
        Ok(())
    }

    pub fn set_active(&mut self, id: &str, active: bool) -> Result<(), EngineError> {
        let e = self.employees.get_mut(id).ok_or_else(|| EngineError::UnknownEmployee(id.into()))?;
        e.active = active;
        Ok(())
    }

    pub fn employee(&self, id: &str) -> Option<&Employee> {
        self.employees.get(id)
    }

    pub fn add_schedule(&mut self, s: Schedule) -> Result<(), EngineError> {
        if self.schedules.contains_key(&s.id) {
            return Err(EngineError::DuplicateSchedule(s.id));
        }
        if let Some(missing) = s.employee_ids.iter().find(|id| !self.employees.contains_key(*id)) {
            return Err(EngineError::UnknownEmployee(missing.clone()));
        }
        self.schedules.insert(s.id.clone(), s);
        Ok(())
    }

    pub fn runs(&self) -> &[PayRun] {
        &self.runs
    }

    pub fn run(&self, id: u64) -> Option<&PayRun> {
        self.runs.iter().find(|r| r.id == id)
    }

    /// Plan a Draft run for `schedule_id` on `date`.
    pub fn plan_scheduled(&mut self, schedule_id: &str, date: Date) -> Result<u64, EngineError> {
        let s = self
            .schedules
            .get(schedule_id)
            .ok_or_else(|| EngineError::UnknownSchedule(schedule_id.into()))?;
        if !s.is_pay_date(date) {
            return Err(EngineError::NotAPayDate { schedule: schedule_id.into(), date });
        }
        let lines = s
            .employee_ids
            .iter()
            .filter_map(|id| self.employees.get(id))
            .filter(|e| e.active)
            .map(line_for)
            .collect();
        self.push_run(format!("{schedule_id}@{date}"), lines)
    }

    /// Plan a Draft run from an ad-hoc batch (e.g. an uploaded CSV). Employees
    /// are validated but not added to the roster.
    pub fn plan_adhoc(&mut self, label: &str, batch: Vec<Employee>) -> Result<u64, EngineError> {
        for e in &batch {
            e.validate()?;
        }
        let lines = batch.iter().filter(|e| e.active).map(line_for).collect();
        self.push_run(format!("adhoc:{label}"), lines)
    }

    fn push_run(&mut self, period_key: String, lines: Vec<PayLine>) -> Result<u64, EngineError> {
        if let Some(prev) =
            self.runs.iter().find(|r| r.period_key == period_key && !matches!(r.status, RunStatus::Failed { .. }))
        {
            return Err(EngineError::AlreadyPaid { period_key, run_id: prev.id });
        }
        if lines.is_empty() {
            return Err(EngineError::EmptyRun);
        }
        let total = check_caps(&self.caps, &lines)?;
        let id = self.runs.last().map_or(1, |r| r.id + 1);
        self.runs.push(PayRun { id, period_key, lines, total, status: RunStatus::Draft });
        Ok(id)
    }

    fn transition(
        &mut self,
        id: u64,
        to: &'static str,
        f: impl FnOnce(&RunStatus) -> Option<RunStatus>,
    ) -> Result<(), EngineError> {
        let r = self.runs.iter_mut().find(|r| r.id == id).ok_or(EngineError::UnknownRun(id))?;
        match f(&r.status) {
            Some(next) => {
                r.status = next;
                Ok(())
            }
            None => Err(EngineError::BadTransition { run_id: id, from: status_name(&r.status), to }),
        }
    }

    pub fn approve(&mut self, id: u64) -> Result<(), EngineError> {
        // Re-check caps at approval in case policy was tightened after planning.
        let run = self.run(id).ok_or(EngineError::UnknownRun(id))?;
        check_caps(&self.caps, &run.lines)?;
        self.transition(id, "Approved", |s| matches!(s, RunStatus::Draft).then_some(RunStatus::Approved))
    }

    pub fn mark_broadcast(&mut self, id: u64, txid: &str) -> Result<(), EngineError> {
        if txid.len() != 64 || !txid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(EngineError::BadTxid(txid.into()));
        }
        let txid = txid.to_ascii_lowercase();
        self.transition(id, "Broadcast", |s| {
            matches!(s, RunStatus::Approved).then_some(RunStatus::Broadcast { txid })
        })
    }

    pub fn mark_confirmed(&mut self, id: u64, height: u32) -> Result<(), EngineError> {
        self.transition(id, "Confirmed", |s| match s {
            RunStatus::Broadcast { txid } => Some(RunStatus::Confirmed { txid: txid.clone(), height }),
            _ => None,
        })
    }

    pub fn mark_failed(&mut self, id: u64, reason: &str) -> Result<(), EngineError> {
        let reason = reason.to_string();
        self.transition(id, "Failed", |s| {
            matches!(s, RunStatus::Approved | RunStatus::Broadcast { .. })
                .then_some(RunStatus::Failed { reason })
        })
    }
}

fn line_for(e: &Employee) -> PayLine {
    PayLine {
        employee_id: e.id.clone(),
        name: e.name.clone(),
        address: e.address.clone(),
        amount: e.pay,
        memo: e.memo.clone(),
    }
}

/// Returns the run total, or every cap violation at once (so the employer
/// can fix the whole CSV in one pass).
pub fn check_caps(caps: &CapPolicy, lines: &[PayLine]) -> Result<Zat, EngineError> {
    let mut v = Vec::new();
    if lines.len() > caps.max_recipients {
        v.push(format!("{} recipients > max {}", lines.len(), caps.max_recipients));
    }
    let mut total = Zat(0);
    let mut overflow = false;
    for l in lines {
        if let Some(max) = caps.max_per_employee {
            if l.amount > max {
                v.push(format!("`{}` gets {} > per-employee max {}", l.employee_id, l.amount, max));
            }
        }
        match total.checked_add(l.amount) {
            Some(t) => total = t,
            None => overflow = true,
        }
    }
    if overflow {
        v.push("run total exceeds 21,000,000 ZEC".into());
    } else if let Some(max) = caps.max_run_total {
        if total > max {
            v.push(format!("run total {total} > max {max}"));
        }
    }
    if v.is_empty() {
        Ok(total)
    } else {
        Err(EngineError::CapExceeded(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::employee::tests::emp;
    use crate::schedule::Cadence;

    const TXID: &str = "aa00000000000000000000000000000000000000000000000000000000000001";

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    fn engine(caps: CapPolicy) -> Engine {
        let mut e = Engine::new(caps);
        e.add_employee(emp("alice", "1.5")).unwrap();
        e.add_employee(emp("bob", "2")).unwrap();
        e.add_employee(emp("carol", "0.25")).unwrap();
        e.add_schedule(Schedule {
            id: "biweekly".into(),
            cadence: Cadence::Biweekly,
            anchor: d("2026-10-02"),
            employee_ids: vec!["alice".into(), "bob".into(), "carol".into()],
        })
        .unwrap();
        e
    }

    #[test]
    fn plans_scheduled_run_with_exact_total() {
        let mut e = engine(CapPolicy::default());
        let id = e.plan_scheduled("biweekly", d("2026-10-16")).unwrap();
        let r = e.run(id).unwrap();
        assert_eq!(r.lines.len(), 3);
        assert_eq!(r.total, Zat::parse_zec("3.75").unwrap());
        assert_eq!(r.period_key, "biweekly@2026-10-16");
        assert_eq!(r.status, RunStatus::Draft);
    }

    #[test]
    fn rejects_off_schedule_and_unknowns() {
        let mut e = engine(CapPolicy::default());
        assert!(matches!(e.plan_scheduled("biweekly", d("2026-10-09")), Err(EngineError::NotAPayDate { .. })));
        assert!(matches!(e.plan_scheduled("nope", d("2026-10-16")), Err(EngineError::UnknownSchedule(_))));
        assert!(matches!(e.add_employee(emp("alice", "1")), Err(EngineError::DuplicateEmployee(_))));
        let bad = Schedule { id: "x".into(), cadence: Cadence::Weekly, anchor: d("2026-10-01"), employee_ids: vec!["ghost".into()] };
        assert!(matches!(e.add_schedule(bad), Err(EngineError::UnknownEmployee(_))));
    }

    #[test]
    fn inactive_employees_are_skipped() {
        let mut e = engine(CapPolicy::default());
        e.set_active("bob", false).unwrap();
        let id = e.plan_scheduled("biweekly", d("2026-10-16")).unwrap();
        assert_eq!(e.run(id).unwrap().total, Zat::parse_zec("1.75").unwrap());
        for n in ["alice", "carol"] {
            e.set_active(n, false).unwrap();
        }
        assert_eq!(e.plan_scheduled("biweekly", d("2026-10-30")), Err(EngineError::EmptyRun));
    }

    #[test]
    fn run_total_cap() {
        let caps = CapPolicy { max_run_total: Some(Zat::parse_zec("3").unwrap()), ..Default::default() };
        let mut e = engine(caps);
        match e.plan_scheduled("biweekly", d("2026-10-16")) {
            Err(EngineError::CapExceeded(v)) => assert!(v[0].contains("run total 3.75 ZEC > max 3 ZEC"), "{v:?}"),
            other => panic!("{other:?}"),
        }
        assert!(e.runs().is_empty(), "rejected run must not be stored");
    }

    #[test]
    fn per_employee_and_recipient_caps_report_all_violations() {
        let caps = CapPolicy { max_per_employee: Some(Zat::parse_zec("1").unwrap()), max_recipients: 2, ..Default::default() };
        let mut e = engine(caps);
        match e.plan_scheduled("biweekly", d("2026-10-16")) {
            Err(EngineError::CapExceeded(v)) => {
                assert_eq!(v.len(), 3, "{v:?}"); // recipients + alice + bob
                assert!(v.iter().any(|s| s.contains("3 recipients > max 2")));
                assert!(v.iter().any(|s| s.contains("`alice`")));
                assert!(v.iter().any(|s| s.contains("`bob`")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn cap_at_exact_limit_passes() {
        let caps = CapPolicy { max_run_total: Some(Zat::parse_zec("3.75").unwrap()), ..Default::default() };
        assert!(engine(caps).plan_scheduled("biweekly", d("2026-10-16")).is_ok());
    }

    #[test]
    fn approve_rechecks_tightened_caps() {
        let mut e = engine(CapPolicy::default());
        let id = e.plan_scheduled("biweekly", d("2026-10-16")).unwrap();
        e.caps.max_run_total = Some(Zat::parse_zec("1").unwrap());
        assert!(matches!(e.approve(id), Err(EngineError::CapExceeded(_))));
        assert_eq!(e.run(id).unwrap().status, RunStatus::Draft);
    }

    #[test]
    fn no_double_pay_unless_failed() {
        let mut e = engine(CapPolicy::default());
        let id = e.plan_scheduled("biweekly", d("2026-10-16")).unwrap();
        assert!(matches!(e.plan_scheduled("biweekly", d("2026-10-16")), Err(EngineError::AlreadyPaid { run_id, .. }) if run_id == id));
        e.approve(id).unwrap();
        e.mark_failed(id, "lightwalletd rejected tx").unwrap();
        let id2 = e.plan_scheduled("biweekly", d("2026-10-16")).unwrap();
        assert_eq!(id2, id + 1);
    }

    #[test]
    fn status_machine() {
        let mut e = engine(CapPolicy::default());
        let id = e.plan_scheduled("biweekly", d("2026-10-16")).unwrap();
        assert!(matches!(e.mark_broadcast(id, TXID), Err(EngineError::BadTransition { .. })));
        assert!(matches!(e.mark_failed(id, "x"), Err(EngineError::BadTransition { .. })));
        e.approve(id).unwrap();
        assert!(matches!(e.approve(id), Err(EngineError::BadTransition { .. })));
        assert!(matches!(e.mark_broadcast(id, "nothex"), Err(EngineError::BadTxid(_))));
        e.mark_broadcast(id, &TXID.to_uppercase()).unwrap();
        e.mark_confirmed(id, 3_100_000).unwrap();
        assert_eq!(e.run(id).unwrap().status, RunStatus::Confirmed { txid: TXID.into(), height: 3_100_000 });
        assert!(matches!(e.mark_failed(id, "late"), Err(EngineError::BadTransition { .. })));
        assert!(matches!(e.approve(99), Err(EngineError::UnknownRun(99))));
    }

    #[test]
    fn adhoc_batch_validates_and_dedupes_label() {
        let mut e = Engine::new(CapPolicy::default());
        let id = e.plan_adhoc("oct-bonus", vec![emp("x", "1"), emp("y", "2")]).unwrap();
        assert_eq!(e.run(id).unwrap().total, Zat::parse_zec("3").unwrap());
        assert!(matches!(e.plan_adhoc("oct-bonus", vec![emp("x", "1")]), Err(EngineError::AlreadyPaid { .. })));
        let mut bad = emp("z", "1");
        bad.address = crate::employee::tests::main_ua();
        assert!(matches!(e.plan_adhoc("other", vec![bad]), Err(EngineError::Employee(EmployeeError::MainnetAddress(_)))));
    }
}
