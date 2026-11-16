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

pub fn atoi(s: &str) -> i64 {
    parse_int_lossy(s, 10)
}

pub fn hex_i64(s: &str) -> i64 {
    parse_int_lossy(s.strip_prefix("0x").unwrap_or(s), 16)
}

pub fn parse_int_base0_lossy(s: &str) -> i64 {
    if let Some(v) = tc32emu::parse_go_int(s) {
        return v;
    }
    0
}

pub fn parse_uint_radix(s: &str, base: u32, bits: u32) -> Option<u64> {
    let u = digits(s, base).ok()?;
    if bits < 64 && u >> bits != 0 {
        return None;
    }
    Some(u)
}

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

pub fn parse_float(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

pub fn float0(s: &str) -> f64 {
    parse_float(s).unwrap_or(0.0)
}

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
