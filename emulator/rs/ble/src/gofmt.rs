// SPDX-License-Identifier: Apache-2.0
//! The bits of Go's fmt the models' output needs and Rust's formatting does
//! not give the same way.

/// Go's %v (and %g) of a float64: the shortest digits that read back
/// (strconv 'g', -1), in %e form when the exponent is < -4 or >= 6, the
/// exponent with at least 2 digits.
pub fn g(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let e = format!("{:e}", x); // shortest digits: "-1.2345e-7"
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let (neg, mant) = match mant.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mant),
    };
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if exp < -4 || exp >= 6 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let a = exp.unsigned_abs();
        if a < 10 {
            out.push('0');
        }
        out.push_str(&a.to_string());
        return out;
    }
    let dp = exp + 1; // digits before the point
    let n = digits.len() as i32;
    if dp <= 0 {
        out.push_str("0.");
        for _ in 0..(-dp) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if dp >= n {
        out.push_str(&digits);
        for _ in 0..(dp - n) {
            out.push('0');
        }
    } else {
        out.push_str(&digits[..dp as usize]);
        out.push('.');
        out.push_str(&digits[dp as usize..]);
    }
    out
}

/// Go's %x of a byte slice (and hex.EncodeToString).
pub fn hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(2 * b.len());
    for &x in b {
        s.push(D[(x >> 4) as usize] as char);
        s.push(D[(x & 15) as usize] as char);
    }
    s
}

/// Go's %.Nf: Rust's {:.N} except for the infinities.
pub fn f(x: f64, prec: usize) -> String {
    if x.is_infinite() {
        return if x > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    format!("{:.*}", prec, x)
}

/// hex.DecodeString: the bytes decoded before an error, and whether there was none.
pub fn unhex(s: &str) -> (Vec<u8>, bool) {
    let b = s.as_bytes();
    let val = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 1;
    while i < b.len() {
        match (val(b[i - 1]), val(b[i])) {
            (Some(a), Some(c)) => out.push(a << 4 | c),
            _ => return (out, false),
        }
        i += 2;
    }
    if b.len() % 2 == 1 {
        return (out, false);
    }
    (out, true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn go_v_float() {
        // go: fmt.Printf("%v|", v) for each
        let xs = [0.0, 1.0, 123456.0, 1234567.0, 999999.5, 1e21, 1e20, 0.0001, 0.00001, 12.345, 100000.0, 1000000.0, 2.5e-5, -3.25,
            123456789.123];
        let want = "0|1|123456|1.234567e+06|999999.5|1e+21|1e+20|0.0001|1e-05|12.345|100000|1e+06|2.5e-05|-3.25|1.23456789123e+08|";
        let got: String = xs.iter().map(|&x| super::g(x) + "|").collect();
        assert_eq!(got, want);
    }
}
