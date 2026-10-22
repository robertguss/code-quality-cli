//! Calendar dates for expiry checks (KTD13). "Today" comes from the system
//! clock once per run and is passed in, so tests always fix it.

use std::fmt;

/// A day in the proleptic Gregorian calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    /// Days since 1970-01-01.
    days: i64,
}

impl Date {
    pub fn from_ymd(year: i64, month: u32, day: u32) -> Option<Date> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Date {
            days: days_from_civil(year, month, day),
        })
    }

    /// `YYYY-MM-DD`, nothing else.
    pub fn parse(s: &str) -> Option<Date> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let num = |r: std::ops::Range<usize>| -> Option<u32> {
            let part = &s[r];
            part.bytes()
                .all(|c| c.is_ascii_digit())
                .then(|| part.parse().ok())?
        };
        Date::from_ymd(num(0..4)? as i64, num(5..7)?, num(8..10)?)
    }

    /// The local date now.
    pub fn today() -> Date {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        Date {
            days: (secs + local_offset_secs(secs)).div_euclid(86_400),
        }
    }

    pub fn add_days(self, n: i64) -> Date {
        Date {
            days: self.days + n,
        }
    }

    /// Days from `self` to `later`; negative when `later` is earlier.
    pub fn days_until(self, later: Date) -> i64 {
        later.days - self.days
    }

    fn ymd(self) -> (i64, u32, u32) {
        civil_from_days(self.days)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = self.ymd();
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's civil-date algorithms.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The local timezone's offset from UTC at `secs`, in seconds.
fn local_offset_secs(secs: i64) -> i64 {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` writes only into `tm`, which outlives the call.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok { tm.tm_gmtoff as i64 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        let d = Date::parse("2026-10-22").unwrap();
        assert_eq!(d.to_string(), "2026-10-22");
        assert_eq!(Date::parse("1970-01-01").unwrap().days, 0);
        assert_eq!(Date::parse("2024-02-29").unwrap().to_string(), "2024-02-29");
        for bad in [
            "2026-02-29",
            "2026-13-01",
            "2026-1-01",
            "26-10-22",
            "2026-10-22x",
            "",
        ] {
            assert!(Date::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn day_arithmetic() {
        let d = Date::parse("2026-10-22").unwrap();
        assert_eq!(d.add_days(90).to_string(), "2027-01-20");
        assert_eq!(d.add_days(-22).to_string(), "2026-09-30");
        assert_eq!(d.days_until(Date::parse("2026-12-31").unwrap()), 70);
        assert!(Date::parse("2026-10-01").unwrap() < d);
    }
}
