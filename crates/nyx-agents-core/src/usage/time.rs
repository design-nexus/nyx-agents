//! Timestamps in agents' records, and local calendar days.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// RFC 3339 / ISO 8601 (`2026-11-10T07:33:17.038Z`, `…+02:00`, no offset = UTC) as
/// Unix seconds.
pub fn parse_iso(s: &str) -> Option<f64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut i = 19;
    let mut frac = 0.0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        frac = format!("0.{}", &s[start..i]).parse().unwrap_or(0.0);
    }
    let offset = match b.get(i) {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(b'+') | Some(b'-') => {
            let sign = if b[i] == b'-' { -1 } else { 1 };
            let oh = num(i + 1..i + 3)?;
            let om = if b.get(i + 3) == Some(&b':') { num(i + 4..i + 6)? } else { num(i + 3..i + 5).unwrap_or(0) };
            sign * (oh * 3600 + om * 60)
        }
        _ => return None,
    };
    Some((days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec - offset) as f64 + frac)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// A record's timestamp: Unix seconds or milliseconds, or an ISO string.
pub fn stamp(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => {
            let x = n.as_f64()?;
            Some(if x > 1e11 { x / 1000.0 } else { x })
        }
        serde_json::Value::String(s) => parse_iso(s),
        _ => None,
    }
}

fn local_tm(secs: i64) -> libc::tm {
    let t: libc::time_t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    tm
}

/// The local calendar day of a Unix time, as `YYYY-MM-DD`.
pub fn local_day(secs: f64) -> String {
    let tm = local_tm(secs as i64);
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

pub fn today() -> String {
    local_day(now())
}

/// The last `n` local days, oldest first, ending today. Counted from today's noon,
/// so a daylight-saving change never skips or repeats a day.
pub fn last_days(n: usize) -> Vec<String> {
    let mut tm = local_tm(now() as i64);
    tm.tm_hour = 12;
    tm.tm_min = 0;
    tm.tm_sec = 0;
    tm.tm_isdst = -1;
    let noon = unsafe { libc::mktime(&mut tm) } as f64;
    (0..n).rev().map(|i| local_day(noon - i as f64 * 86400.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso() {
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_iso("2026-11-10T07:33:17Z"), Some(1_794_295_997.0));
        assert_eq!(parse_iso("2026-11-10T08:33:17+01:00"), Some(1_794_295_997.0));
        assert_eq!(parse_iso("2026-11-10T07:33:17.5"), Some(1_794_295_997.5));
        assert_eq!(parse_iso("yesterday"), None);
        assert_eq!(stamp(&serde_json::json!(1_794_295_997_000u64)), Some(1_794_295_997.0));
    }

    #[test]
    fn seven_days_end_today() {
        let d = last_days(7);
        assert_eq!(d.len(), 7);
        assert_eq!(d[6], today());
        assert!(d.windows(2).all(|w| w[0] < w[1]));
    }
}
