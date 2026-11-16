use serde_json::Value;

pub fn str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        _ => repr(v),
    }
}

pub fn get(m: &serde_json::Map<String, Value>, k: &str) -> String {
    m.get(k).map(str).unwrap_or_else(|| "None".into())
}

pub fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (_, Some(u)) => u.to_string(),
            _ => float(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter().map(|(k, v)| format!("{}: {}", repr(&Value::String(k.clone())), repr(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

pub fn float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e16 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

pub fn num(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

pub fn eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let a = a.unwrap_or(&Value::Null);
    let b = b.unwrap_or(&Value::Null);
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

pub fn csv_cell(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => csv_quote(&str(v)),
    }
}

pub fn csv_quote(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn csv_row(cells: &[String]) -> String {
    format!("{}\r\n", cells.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn like_python() {
        assert_eq!(str(&json!(null)), "None");
        assert_eq!(str(&json!(true)), "True");
        assert_eq!(str(&json!(2.0)), "2.0");
        assert_eq!(str(&json!(0.7)), "0.7");
        assert_eq!(str(&json!(-57)), "-57");
        assert_eq!(str(&json!("good")), "good");
        assert_eq!(str(&json!({"a": 1, "b": [true, "x"]})), "{'a': 1, 'b': [True, 'x']}");
        assert!(!truthy(Some(&json!(0))));
        assert!(truthy(Some(&json!("none"))));
        assert!(eq(Some(&json!(8)), Some(&json!(8.0))));
        assert!(eq(None, Some(&json!(null))));
        assert_eq!(csv_cell(Some(&json!("a,b"))), "\"a,b\"");
    }
}
