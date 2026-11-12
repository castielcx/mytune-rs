use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// `%ProgramData%\MyTune`. The env var `MYTUNE_DIR` overrides it (integration tests use this so they
/// never write to the real log or state).
pub fn default_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("MYTUNE_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into());
    PathBuf::from(base).join("MyTune")
}

/// Local time as (year, month, day, hour, minute, second).
pub fn local_now() -> (u16, u16, u16, u16, u16, u16) {
    let t = unsafe { GetLocalTime() };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// `yyyy-MM-dd HH:mm:ss` in local time (log line prefix).
pub fn local_stamp() -> String {
    let (y, mo, d, h, mi, s) = local_now();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// `yyyyMMddHHmmss` in local time (.corrupt file suffix).
pub fn local_compact() -> String {
    let (y, mo, d, h, mi, s) = local_now();
    format!("{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}")
}

/// Current time as ISO 8601 with 7 fractional digits and `+00:00`, readable by C# DateTimeOffset.
pub fn iso_now() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:07}+00:00",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_nanos() / 100
    )
}

/// Days since 1970-01-01 for a civil date (inverse of the algorithm in `iso_now`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Local time like C# `DateTime.Now` serialised by System.Text.Json: `2026-11-10T13:45:12.1234567+01:00`.
pub fn local_iso_now() -> String {
    let (y, mo, d, h, mi, s) = local_now();
    let utc = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let local_secs = days_from_civil(y as i64, mo as i64, d as i64) * 86400 + h as i64 * 3600 + mi as i64 * 60 + s as i64;
    let offset_min = ((local_secs - utc.as_secs() as i64) as f64 / 60.0).round() as i64;
    let sign = if offset_min < 0 { '-' } else { '+' };
    format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{:07}{sign}{:02}:{:02}",
        utc.subsec_nanos() / 100,
        offset_min.abs() / 60,
        offset_min.abs() % 60
    )
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in s.trim_end_matches('=').bytes() {
        let v = B64.iter().position(|&b| b == ch)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_and_known() {
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert_eq!(b64_encode(b"M"), "TQ==");
        assert_eq!(b64_encode(b""), "");
        for n in 0..40u8 {
            let v: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(b64_decode(&b64_encode(&v)).unwrap(), v);
        }
        assert!(b64_decode("!!").is_none());
    }

    #[test]
    fn days_from_civil_roundtrip_and_local_iso() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2026, 11, 10), 20767);
        let s = local_iso_now();
        assert_eq!(s.len(), "2026-11-10T13:45:12.1234567+01:00".len(), "{s}");
        assert!(s.as_bytes()[10] == b'T' && (s.contains('+') || s[19..].contains('-')));
    }

    #[test]
    fn iso_shape() {
        let s = iso_now();
        assert_eq!(s.len(), "2026-11-09T23:28:34.9960836+01:00".len());
        assert!(s.ends_with("+00:00") && s.as_bytes()[10] == b'T');
    }
}
