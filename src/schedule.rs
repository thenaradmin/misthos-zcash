//! Pay schedules: which dates a pay run is due on.

use crate::date::{days_in_month, Date};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cadence {
    Weekly,
    Biweekly,
    /// Pay on this day of month; clamped to the month's last day (e.g. 31 -> Feb 28).
    Monthly { day: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub id: String,
    pub cadence: Cadence,
    /// First pay date. Runs are never due before it.
    pub anchor: Date,
    /// Employee ids paid on this schedule.
    pub employee_ids: Vec<String>,
}

impl Schedule {
    /// Is `date` a pay date for this schedule?
    pub fn is_pay_date(&self, date: Date) -> bool {
        if date < self.anchor {
            return false;
        }
        let delta = date.to_days() - self.anchor.to_days();
        match self.cadence {
            Cadence::Weekly => delta % 7 == 0,
            Cadence::Biweekly => delta % 14 == 0,
            Cadence::Monthly { day } => date.d == day.min(days_in_month(date.y, date.m)),
        }
    }

    /// First pay date strictly after `after` (or the anchor if later).
    pub fn next_pay_date(&self, after: Date) -> Date {
        let mut d = if after < self.anchor { self.anchor } else { after.add_days(1) };
        // Bounded: monthly needs at most ~31 steps, biweekly 14.
        for _ in 0..62 {
            if self.is_pay_date(d) {
                return d;
            }
            d = d.add_days(1);
        }
        unreachable!("every cadence hits a pay date within 62 days")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(c: Cadence, anchor: &str) -> Schedule {
        Schedule { id: "s".into(), cadence: c, anchor: Date::parse(anchor).unwrap(), employee_ids: vec![] }
    }

    #[test]
    fn biweekly() {
        let sch = s(Cadence::Biweekly, "2026-10-02");
        assert!(sch.is_pay_date(Date::parse("2026-10-16").unwrap()));
        assert!(!sch.is_pay_date(Date::parse("2026-10-09").unwrap()));
        assert!(!sch.is_pay_date(Date::parse("2026-09-18").unwrap()), "before anchor");
        assert_eq!(sch.next_pay_date(Date::parse("2026-10-16").unwrap()).to_string(), "2026-10-30");
        assert_eq!(sch.next_pay_date(Date::parse("2026-01-01").unwrap()).to_string(), "2026-10-02");
    }

    #[test]
    fn monthly_clamps_to_month_end() {
        let sch = s(Cadence::Monthly { day: 31 }, "2026-01-31");
        assert!(sch.is_pay_date(Date::parse("2026-02-28").unwrap()));
        assert!(sch.is_pay_date(Date::parse("2026-04-30").unwrap()));
        assert_eq!(sch.next_pay_date(Date::parse("2026-01-31").unwrap()).to_string(), "2026-02-28");
        assert_eq!(sch.next_pay_date(Date::parse("2028-01-31").unwrap()).to_string(), "2028-02-29");
    }

    #[test]
    fn weekly() {
        let sch = s(Cadence::Weekly, "2026-10-05");
        assert_eq!(sch.next_pay_date(Date::parse("2026-10-05").unwrap()).to_string(), "2026-10-12");
    }
}
