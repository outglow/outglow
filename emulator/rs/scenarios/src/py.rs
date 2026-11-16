pub fn float(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let a = x.abs();
    if a != 0.0 && !(1e-4..1e16).contains(&a) {
        let s = format!("{x:e}");
        let (m, e) = s.split_once('e').unwrap();
        let (sign, digits) = match e.strip_prefix('-') {
            Some(d) => ('-', d),
            None => ('+', e),
        };
        return format!("{m}e{sign}{digits:0>2}");
    }
    format!("{x:?}")
}

pub fn round(x: f64, n: i32) -> f64 {
    let s = format!("{:.*}", n.max(0) as usize, x);
    s.parse().unwrap()
}

pub fn list<I: IntoIterator<Item = String>>(items: I) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(", "))
}

pub fn tuple<I: IntoIterator<Item = String>>(items: I) -> String {
    let v: Vec<String> = items.into_iter().collect();
    if v.len() == 1 {
        format!("({},)", v[0])
    } else {
        format!("({})", v.join(", "))
    }
}

pub fn ints<I: IntoIterator<Item = i64>>(items: I) -> String {
    list(items.into_iter().map(|x| x.to_string()))
}

pub fn floats<I: IntoIterator<Item = f64>>(items: I) -> String {
    list(items.into_iter().map(float))
}

pub fn boolean(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

pub fn hex_sep(b: &[u8], sep: &str) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn like_python() {
        assert_eq!(float(3.0), "3.0");
        assert_eq!(float(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(float(1e16), "1e+16");
        assert_eq!(float(1.5e-5), "1.5e-05");
        assert_eq!(float(-0.0), "-0.0");
        assert_eq!(round(2.675, 2), 2.67);
        assert_eq!(tuple(["37".to_string()]), "(37,)");
        assert_eq!(floats([1.25, 2.0]), "[1.25, 2.0]");
    }
}

pub fn str_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut o = String::new();
    o.push(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c == q => {
                o.push('\\');
                o.push(q);
            }
            c if (c as u32) < 0x20 || (0x7f..=0xa0).contains(&(c as u32)) || c as u32 == 0xad => {
                o.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

pub fn bytes_repr(b: &[u8]) -> String {
    crate::frames::py_bytes(b)
}

pub fn utf8(b: &[u8]) -> Result<String, String> {
    match std::str::from_utf8(b) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => {
            let i = e.valid_up_to();
            let why = match e.error_len() {
                None => "unexpected end of data",
                Some(_) if (0x80..0xc2).contains(&b[i]) || b[i] >= 0xf5 => "invalid start byte",
                Some(_) => "invalid continuation byte",
            };
            Err(format!("'utf-8' codec can't decode byte {:#04x} in position {i}: {why}", b[i]))
        }
    }
}

pub fn hexq(x: u64) -> String {
    format!("'{x:#x}'")
}
