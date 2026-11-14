// SPDX-License-Identifier: Apache-2.0
//! Go's strconv as zbnode uses it: the value it returns along with an error
//! (0 on a syntax error, the clamped value out of range), which zbnode keeps.

/// The digits of s in base (2..=36), as a magnitude; Err(true) out of range of u64, Err(false) bad syntax.
fn digits(s: &str, base: u32) -> Result<u64, bool> {
    if s.is_empty() {
        return Err(false);
    }
    let mut n: u64 = 0;
    let mut over = false;
    for c in s.chars() {
        let d = c.to_digit(base).ok_or(false)? as u64;
        match n.checked_mul(base as u64).and_then(|x| x.checked_add(d)) {
            Some(x) => n = x,
            None => over = true,
        }
    }
    if over {
        Err(true)
    } else {
        Ok(n)
    }
}

/// strconv.ParseInt(s, base, 64) keeping the value Go returns with an error.
fn parse_int_lossy(s: &str, base: u32) -> i64 {
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    match digits(rest, base) {
        Err(false) => 0,
        Err(true) => {
            if neg {
                i64::MIN
            } else {
                i64::MAX
            }
        }
        Ok(u) => {
            if !neg && u > i64::MAX as u64 {
                i64::MAX
            } else if neg && u > 1u64 << 63 {
                i64::MIN
            } else if neg {
                (u as i64).wrapping_neg()
            } else {
                u as i64
            }
        }
    }
}

/// strconv.Atoi.
pub fn atoi(s: &str) -> i64 {
    parse_int_lossy(s, 10)
}

/// strconv.ParseInt(strings.TrimPrefix(s, "0x"), 16, 64).
pub fn hex_i64(s: &str) -> i64 {
    parse_int_lossy(s.strip_prefix("0x").unwrap_or(s), 16)
}

/// strconv.ParseInt(s, 0, 64) keeping the value returned with an error.
pub fn parse_int_base0_lossy(s: &str) -> i64 {
    if let Some(v) = tc32emu::parse_go_int(s) {
        return v;
    }
    0
}

/// strconv.ParseUint(s, base, bits): None on any error.
pub fn parse_uint_radix(s: &str, base: u32, bits: u32) -> Option<u64> {
    let u = digits(s, base).ok()?;
    if bits < 64 && u >> bits != 0 {
        return None;
    }
    Some(u)
}

/// strconv.ParseUint(s, base, bits) keeping the value returned with an error (max out of range).
pub fn parse_uint_lossy(s: &str, base: u32, bits: u32) -> u64 {
    let max = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
    match digits(s, base) {
        Err(false) => 0,
        Err(true) => max,
        Ok(u) => {
            if u > max {
                max
            } else {
                u
            }
        }
    }
}

/// strconv.ParseFloat(s, 64); None on a syntax error.
pub fn parse_float(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

/// strconv.ParseFloat ignoring the error (0 on bad syntax).
pub fn float0(s: &str) -> f64 {
    parse_float(s).unwrap_or(0.0)
}

/// Go's %q of a string (the printable ASCII cases).
pub fn go_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
