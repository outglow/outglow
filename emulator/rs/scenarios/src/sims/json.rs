//! Just enough JSON for the scenarios that exchange data with Node
//! (emulator/ble_diag_sim.py): parsing as Python's json.loads does (integers
//! stay integers) and printing as json.dumps does with its defaults
//! (", " / ": " separators, ensure_ascii).

#[derive(Clone, Debug, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

static NULL: J = J::Null;

impl J {
    /// `d[k]` (Null when absent; the scripts only read keys that exist).
    pub fn get(&self, k: &str) -> &J {
        match self {
            J::Obj(v) => v.iter().find(|(x, _)| x == k).map(|(_, j)| j).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    pub fn idx(&self, i: usize) -> &J {
        match self {
            J::Arr(v) => v.get(i).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    pub fn arr(&self) -> &[J] {
        match self {
            J::Arr(v) => v,
            _ => &[],
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            J::Int(i) => Some(*i as f64),
            J::Float(f) => Some(*f),
            J::Bool(b) => Some(*b as i64 as f64),
            _ => None,
        }
    }

    /// `== n` in Python (True == 1 too).
    pub fn eq_num(&self, n: f64) -> bool {
        self.num() == Some(n)
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            J::Null => false,
            J::Bool(b) => *b,
            J::Int(i) => *i != 0,
            J::Float(f) => *f != 0.0,
            J::Str(s) => !s.is_empty(),
            J::Arr(v) => !v.is_empty(),
            J::Obj(v) => !v.is_empty(),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, J::Null)
    }

    /// `str(value)` of the decoded Python object (for f-strings).
    pub fn py_str(&self) -> String {
        match self {
            J::Null => "None".into(),
            J::Bool(b) => crate::py::boolean(*b).into(),
            J::Int(i) => i.to_string(),
            J::Float(f) => crate::py::float(*f),
            J::Str(s) => s.clone(),
            _ => self.py_repr(),
        }
    }

    /// `repr(value)`.
    pub fn py_repr(&self) -> String {
        match self {
            J::Str(s) => crate::py::str_repr(s),
            J::Arr(v) => format!("[{}]", v.iter().map(|x| x.py_repr()).collect::<Vec<_>>().join(", ")),
            J::Obj(v) => format!(
                "{{{}}}",
                v.iter().map(|(k, x)| format!("{}: {}", crate::py::str_repr(k), x.py_repr())).collect::<Vec<_>>().join(", ")
            ),
            _ => self.py_str(),
        }
    }

    /// `json.dumps(value)`.
    pub fn dumps(&self) -> String {
        match self {
            J::Null => "null".into(),
            J::Bool(b) => if *b { "true" } else { "false" }.into(),
            J::Int(i) => i.to_string(),
            J::Float(f) => {
                if f.is_nan() {
                    "NaN".into()
                } else if f.is_infinite() {
                    if *f > 0.0 { "Infinity" } else { "-Infinity" }.into()
                } else {
                    crate::py::float(*f)
                }
            }
            J::Str(s) => dump_str(s),
            J::Arr(v) => format!("[{}]", v.iter().map(|x| x.dumps()).collect::<Vec<_>>().join(", ")),
            J::Obj(v) => format!("{{{}}}", v.iter().map(|(k, x)| format!("{}: {}", dump_str(k), x.dumps())).collect::<Vec<_>>().join(", ")),
        }
    }
}

pub fn dump_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{08}' => o.push_str("\\b"),
            '\u{0c}' => o.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    o.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn parse(s: &str) -> Result<J, String> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let v = value(&b, &mut i)?;
    ws(&b, &mut i);
    if i != b.len() {
        return Err(format!("JSON: extra data at {i}"));
    }
    Ok(v)
}

fn ws(b: &[char], i: &mut usize) {
    while *i < b.len() && b[*i].is_whitespace() {
        *i += 1;
    }
}

fn value(b: &[char], i: &mut usize) -> Result<J, String> {
    ws(b, i);
    let Some(&c) = b.get(*i) else { return Err("JSON: unexpected end".into()) };
    match c {
        '{' => {
            *i += 1;
            let mut v = Vec::new();
            ws(b, i);
            if b.get(*i) == Some(&'}') {
                *i += 1;
                return Ok(J::Obj(v));
            }
            loop {
                ws(b, i);
                let J::Str(k) = value(b, i)? else { return Err("JSON: key".into()) };
                ws(b, i);
                if b.get(*i) != Some(&':') {
                    return Err("JSON: ':'".into());
                }
                *i += 1;
                let x = value(b, i)?;
                match v.iter_mut().find(|(kk, _): &&mut (String, J)| *kk == k) {
                    Some(e) => e.1 = x,
                    None => v.push((k, x)),
                }
                ws(b, i);
                match b.get(*i) {
                    Some(',') => *i += 1,
                    Some('}') => {
                        *i += 1;
                        return Ok(J::Obj(v));
                    }
                    _ => return Err("JSON: object".into()),
                }
            }
        }
        '[' => {
            *i += 1;
            let mut v = Vec::new();
            ws(b, i);
            if b.get(*i) == Some(&']') {
                *i += 1;
                return Ok(J::Arr(v));
            }
            loop {
                v.push(value(b, i)?);
                ws(b, i);
                match b.get(*i) {
                    Some(',') => *i += 1,
                    Some(']') => {
                        *i += 1;
                        return Ok(J::Arr(v));
                    }
                    _ => return Err("JSON: array".into()),
                }
            }
        }
        '"' => {
            *i += 1;
            let mut s = String::new();
            loop {
                let Some(&c) = b.get(*i) else { return Err("JSON: string".into()) };
                *i += 1;
                match c {
                    '"' => return Ok(J::Str(s)),
                    '\\' => {
                        let e = b[*i];
                        *i += 1;
                        match e {
                            'n' => s.push('\n'),
                            'r' => s.push('\r'),
                            't' => s.push('\t'),
                            'b' => s.push('\u{08}'),
                            'f' => s.push('\u{0c}'),
                            'u' => {
                                let h: String = b[*i..*i + 4].iter().collect();
                                *i += 4;
                                let mut u = u32::from_str_radix(&h, 16).map_err(|e| e.to_string())?;
                                if (0xD800..0xDC00).contains(&u) && b.get(*i) == Some(&'\\') && b.get(*i + 1) == Some(&'u') {
                                    let h2: String = b[*i + 2..*i + 6].iter().collect();
                                    let lo = u32::from_str_radix(&h2, 16).map_err(|e| e.to_string())?;
                                    *i += 6;
                                    u = 0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00);
                                }
                                s.push(char::from_u32(u).unwrap_or('\u{fffd}'));
                            }
                            x => s.push(x),
                        }
                    }
                    c => s.push(c),
                }
            }
        }
        't' => {
            *i += 4;
            Ok(J::Bool(true))
        }
        'f' => {
            *i += 5;
            Ok(J::Bool(false))
        }
        'n' => {
            *i += 4;
            Ok(J::Null)
        }
        _ => {
            let st = *i;
            while *i < b.len() && (b[*i].is_ascii_digit() || "+-.eE".contains(b[*i])) {
                *i += 1;
            }
            let t: String = b[st..*i].iter().collect();
            if t.contains(['.', 'e', 'E']) {
                t.parse().map(J::Float).map_err(|e| format!("JSON number {t}: {e}"))
            } else {
                t.parse().map(J::Int).map_err(|e| format!("JSON number {t}: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn like_python_json() {
        let j = parse(r#"{"a": 1, "b": [1.5, true, null], "c": "xé\"y"}"#).unwrap();
        assert_eq!(j.dumps(), r#"{"a": 1, "b": [1.5, true, null], "c": "x\u00e9\"y"}"#);
        assert_eq!(j.get("c").py_repr(), "'xé\"y'");
        assert!(j.get("a").eq_num(1.0));
    }
}
