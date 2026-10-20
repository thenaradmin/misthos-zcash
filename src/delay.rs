//! Block delays for `pay --two-step`, so a payout does not land in the block right
//! after its funding tx. Pure functions only; the CLI does the chain polling.
//!
//! After the funding tx is mined at height `F`, payouts begin at `F + d0`, and the
//! employees then go one after another, each `d_i` blocks after the previous one:
//! `target_i = F + d0 + d_1 + ... + d_i`. Every `d` is drawn uniformly from
//! `PAYROLL_DELAY_MIN_BLOCKS..=PAYROLL_DELAY_MAX_BLOCKS` (default 2..=6).
//! An employee's `PAYROLL_STEP2_TIMEOUT_SECS` only starts once its target is reached.

use crate::config::ConfigError;

pub const DEFAULT_MIN_BLOCKS: u32 = 2;
pub const DEFAULT_MAX_BLOCKS: u32 = 6;
/// Sanity bound: a typo like `600` would otherwise park funds for days.
pub const MAX_BLOCKS: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelayRange {
    pub min: u32,
    pub max: u32,
}

impl Default for DelayRange {
    fn default() -> Self {
        DelayRange { min: DEFAULT_MIN_BLOCKS, max: DEFAULT_MAX_BLOCKS }
    }
}

/// Reads `PAYROLL_DELAY_MIN_BLOCKS` / `PAYROLL_DELAY_MAX_BLOCKS`; unset means the default.
pub fn range_from(get: impl Fn(&str) -> Option<String>) -> Result<DelayRange, ConfigError> {
    let num = |k: &'static str, dflt: u32| -> Result<u32, ConfigError> {
        match get(k).filter(|v| !v.trim().is_empty()) {
            None => Ok(dflt),
            Some(v) => match v.trim().parse::<u32>() {
                Ok(n) if n <= MAX_BLOCKS => Ok(n),
                Ok(n) => Err(ConfigError::Invalid { var: k, why: format!("{n} blocks is more than the {MAX_BLOCKS}-block limit") }),
                Err(_) => Err(ConfigError::Invalid { var: k, why: format!("`{v}` is not a whole number of blocks") }),
            },
        }
    };
    let min = num("PAYROLL_DELAY_MIN_BLOCKS", DEFAULT_MIN_BLOCKS)?;
    let max = num("PAYROLL_DELAY_MAX_BLOCKS", DEFAULT_MAX_BLOCKS)?;
    if min > max {
        return Err(ConfigError::Invalid {
            var: "PAYROLL_DELAY_MIN_BLOCKS",
            why: format!("{min} is greater than PAYROLL_DELAY_MAX_BLOCKS={max}"),
        });
    }
    Ok(DelayRange { min, max })
}

/// Uniform draw in `r.min..=r.max` from a source of random `u32`s (rejection sampling, no modulo bias).
pub fn draw(r: DelayRange, mut rand_u32: impl FnMut() -> u32) -> u32 {
    let span = u64::from(r.max - r.min) + 1;
    let zone = (1u64 << 32) - ((1u64 << 32) % span);
    loop {
        let x = u64::from(rand_u32());
        if x < zone {
            return r.min + (x % span) as u32;
        }
    }
}

/// A random `u32` from the OS CSPRNG. Fails loudly instead of falling back to anything predictable.
pub fn os_u32() -> u32 {
    let mut b = [0u8; 4];
    getrandom::getrandom(&mut b).expect("OS random number generator unavailable");
    u32::from_le_bytes(b)
}

/// Target payout heights, in payout order: `start + d_1`, `start + d_1 + d_2`, ...
/// `start` is `funding height + d0` (or the last already-persisted target on a rerun).
pub fn stagger(start: u32, delays: &[u32]) -> Vec<u32> {
    let mut h = start;
    delays
        .iter()
        .map(|d| {
            h = h.saturating_add(*d);
            h
        })
        .collect()
}

/// Where one employee's step 2 stands. Times are seconds on any monotonic clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Delay not over: do not send, and the timeout has not started.
    Waiting { blocks_left: u32 },
    /// Delay over: try to send until `deadline`.
    Ready { deadline: u64 },
    /// Delay over and `timeout` elapsed since then without a successful send.
    TimedOut,
}

/// `ready_since` is when this employee was first seen `Ready` (None until then); the caller
/// stores `now` there on the first `Ready`. Once ready, a lower tip (reorg) does not re-arm the delay.
pub fn phase(target: u32, tip: u32, ready_since: Option<u64>, now: u64, timeout_secs: u64) -> Phase {
    let since = match ready_since {
        Some(t) => t,
        None if tip < target => return Phase::Waiting { blocks_left: target - tip },
        None => now,
    };
    let deadline = since.saturating_add(timeout_secs);
    if now > deadline {
        Phase::TimedOut
    } else {
        Phase::Ready { deadline }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn default_range_is_2_to_6() {
        assert_eq!(range_from(env(&[])).unwrap(), DelayRange { min: 2, max: 6 });
        let r = range_from(env(&[("PAYROLL_DELAY_MIN_BLOCKS", "3"), ("PAYROLL_DELAY_MAX_BLOCKS", "3")])).unwrap();
        assert_eq!(r, DelayRange { min: 3, max: 3 });
    }

    #[test]
    fn invalid_config_is_refused_naming_the_var() {
        let e = range_from(env(&[("PAYROLL_DELAY_MIN_BLOCKS", "7")])).unwrap_err().to_string();
        assert!(e.starts_with("PAYROLL_DELAY_MIN_BLOCKS invalid") && e.contains("PAYROLL_DELAY_MAX_BLOCKS=6"), "{e}");
        let e = range_from(env(&[("PAYROLL_DELAY_MIN_BLOCKS", "5"), ("PAYROLL_DELAY_MAX_BLOCKS", "4")])).unwrap_err();
        assert!(matches!(e, ConfigError::Invalid { var: "PAYROLL_DELAY_MIN_BLOCKS", .. }));
        for bad in ["-1", "two", "2.5", "1001"] {
            let e = range_from(env(&[("PAYROLL_DELAY_MAX_BLOCKS", bad)])).unwrap_err();
            assert!(matches!(e, ConfigError::Invalid { var: "PAYROLL_DELAY_MAX_BLOCKS", .. }), "{bad}");
        }
    }

    #[test]
    fn draws_stay_in_bounds_and_hit_both_ends() {
        let r = DelayRange::default();
        let mut seen = [false; 7];
        for _ in 0..2000 {
            let d = draw(r, os_u32);
            assert!((2..=6).contains(&d), "{d}");
            seen[d as usize] = true;
        }
        assert!(seen[2..=6].iter().all(|s| *s), "{seen:?}");
        // Extremes of the random source map inside the range.
        assert_eq!(draw(r, || 0), 2);
        assert_eq!(draw(DelayRange { min: 4, max: 4 }, || u32::MAX), 4);
        // Values in the biased tail are rejected, not folded in.
        let mut xs = [u32::MAX, 7].into_iter();
        assert_eq!(draw(r, || xs.next().unwrap()), 2 + 7 % 5);
        assert_eq!(draw(DelayRange { min: 0, max: MAX_BLOCKS }, || 1000), 1000);
    }

    #[test]
    fn stagger_is_cumulative() {
        assert_eq!(stagger(100, &[2, 6, 3]), vec![102, 108, 111]);
        assert!(stagger(100, &[]).is_empty());
    }

    #[test]
    fn timeout_starts_only_after_the_delay() {
        let timeout = 1800;
        // Hours pass while waiting for blocks: still Waiting, never TimedOut.
        assert_eq!(phase(110, 105, None, 50_000, timeout), Phase::Waiting { blocks_left: 5 });
        // Target reached at t=50_000: deadline counts from then, not from the run start.
        assert_eq!(phase(110, 110, None, 50_000, timeout), Phase::Ready { deadline: 51_800 });
        assert_eq!(phase(110, 111, Some(50_000), 51_800, timeout), Phase::Ready { deadline: 51_800 });
        assert_eq!(phase(110, 112, Some(50_000), 51_801, timeout), Phase::TimedOut);
        // A reorg below the target after becoming ready does not restart the delay or the clock.
        assert_eq!(phase(110, 108, Some(50_000), 50_100, timeout), Phase::Ready { deadline: 51_800 });
    }

    #[test]
    fn staggered_employees_get_staggered_deadlines() {
        let targets = stagger(200, &[2, 4]); // 202, 206
        let first_seen = |target: u32| -> u64 {
            // tip advances one block per 75 s from height 200 at t=0
            (0u64..).find(|t| 200 + (t / 75) as u32 >= target).unwrap()
        };
        let d: Vec<_> = targets
            .iter()
            .map(|t| match phase(*t, *t, None, first_seen(*t), 600) {
                Phase::Ready { deadline } => deadline,
                p => panic!("{p:?}"),
            })
            .collect();
        assert_eq!(d, vec![150 + 600, 450 + 600]);
    }
}
