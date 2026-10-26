//! The few Python formatting rules the port must reproduce exactly: `repr(float)` (and so `json.dumps`), `f"{x:g}"`
//! (row names), `round(x, 6)`, round-half-away-from-zero (`rhaz`), and `json.dumps(obj, indent=2)` with insertion-ordered
//! keys, so `build\rows.json` comes out byte-identical to build_regulation.py's (except the volatile fields).

/// Python `repr(x)` of a float (shortest round-trip digits; scientific when the exponent is < -4 or >= 16).
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let s = format!("{:e}", x.abs());
    let (mant, exp) = s.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let sign = if x < 0.0 { "-" } else { "" };
    let body = if !(-4..16).contains(&exp) {
        let rest = &digits[1..];
        let m = if rest.is_empty() { digits[..1].to_string() } else { format!("{}.{rest}", &digits[..1]) };
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    } else if exp >= 0 {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            format!("{digits}{}.0", "0".repeat(int_len - digits.len()))
        } else {
            format!("{}.{}", &digits[..int_len], &digits[int_len..])
        }
    } else {
        format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
    };
    format!("{sign}{body}")
}

/// Python `format(x, "g")` (6 significant digits, trailing zeros stripped).
pub fn fmt_g(x: f64) -> String {
    if !x.is_finite() {
        return if x.is_nan() { "nan".into() } else if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    const P: i32 = 6;
    let e = format!("{:.*e}", (P - 1) as usize, x);
    let exp: i32 = e.split_once('e').unwrap().1.parse().unwrap();
    let strip = |s: String| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
    };
    if (-4..P).contains(&exp) {
        strip(format!("{:.*}", (P - 1 - exp) as usize, x))
    } else {
        let (m, _) = e.split_once('e').unwrap();
        format!("{}e{}{:02}", strip(m.to_string()), if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// Python `round(x, 6)` (correctly rounded through the decimal string, as CPython does).
pub fn round6(x: f64) -> f64 {
    format!("{x:.6}").parse().unwrap()
}

/// build_regulation.rhaz: round half away from zero via `floor(x + 0.5)` (not Rust's `round`, which differs at
/// 0.49999999999999994).
pub fn rhaz(x: f64) -> i64 {
    if x >= 0.0 { (x + 0.5).floor() as i64 } else { -((-x + 0.5).floor() as i64) }
}

/// A JSON value with insertion-ordered objects and Python's int / float distinction.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    /// Sets `k` (replacing an existing key in place, as a Python dict does).
    pub fn set(&mut self, k: &str, v: impl Into<Json>) -> &mut Self {
        if let Json::Obj(m) = self {
            let v = v.into();
            match m.iter_mut().find(|(key, _)| key == k) {
                Some(slot) => slot.1 = v,
                None => m.push((k.to_string(), v)),
            }
        }
        self
    }

    pub fn with(mut self, k: &str, v: impl Into<Json>) -> Self {
        self.set(k, v);
        self
    }

    pub fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            _ => None,
        }
    }

    /// `json.dumps(self, indent=2)`.
    pub fn dumps(&self) -> String {
        let mut s = String::new();
        dump(self, 0, &mut s);
        s
    }

    /// From a parsed serde_json value (key order of the source when serde_json keeps it; numbers keep int vs float).
    pub fn from_serde(v: &serde_json::Value) -> Json {
        match v {
            serde_json::Value::Null => Json::Null,
            serde_json::Value::Bool(b) => Json::Bool(*b),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => Json::Int(i),
                None => Json::Float(n.as_f64().unwrap_or(f64::NAN)),
            },
            serde_json::Value::String(s) => Json::Str(s.clone()),
            serde_json::Value::Array(a) => Json::Arr(a.iter().map(Json::from_serde).collect()),
            serde_json::Value::Object(m) => Json::Obj(m.iter().map(|(k, v)| (k.clone(), Json::from_serde(v))).collect()),
        }
    }

    /// Canonical form for comparisons: objects sorted by key (as coop_manifest's `sort_keys=True`).
    pub fn sorted(&self) -> Json {
        match self {
            Json::Arr(a) => Json::Arr(a.iter().map(Json::sorted).collect()),
            Json::Obj(m) => {
                let mut v: Vec<(String, Json)> = m.iter().map(|(k, v)| (k.clone(), v.sorted())).collect();
                v.sort_by(|a, b| a.0.cmp(&b.0));
                Json::Obj(v)
            }
            x => x.clone(),
        }
    }
}

impl From<i64> for Json {
    fn from(v: i64) -> Self {
        Json::Int(v)
    }
}
impl From<i32> for Json {
    fn from(v: i32) -> Self {
        Json::Int(v as i64)
    }
}
impl From<u32> for Json {
    fn from(v: u32) -> Self {
        Json::Int(v as i64)
    }
}
impl From<usize> for Json {
    fn from(v: usize) -> Self {
        Json::Int(v as i64)
    }
}
impl From<f64> for Json {
    fn from(v: f64) -> Self {
        Json::Float(v)
    }
}
impl From<&str> for Json {
    fn from(v: &str) -> Self {
        Json::Str(v.into())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Self {
        Json::Str(v)
    }
}
impl From<Vec<Json>> for Json {
    fn from(v: Vec<Json>) -> Self {
        Json::Arr(v)
    }
}

fn dump_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7E => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn dump(v: &Json, depth: usize, out: &mut String) {
    let pad = |d: usize, out: &mut String| {
        out.push('\n');
        out.push_str(&" ".repeat(2 * d));
    };
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(i) => out.push_str(&i.to_string()),
        Json::Float(f) => out.push_str(&float_repr(*f)),
        Json::Str(s) => dump_str(s, out),
        Json::Arr(a) if a.is_empty() => out.push_str("[]"),
        Json::Obj(m) if m.is_empty() => out.push_str("{}"),
        Json::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(depth + 1, out);
                dump(x, depth + 1, out);
            }
            pad(depth, out);
            out.push(']');
        }
        Json::Obj(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                pad(depth + 1, out);
                dump_str(k, out);
                out.push_str(": ");
                dump(x, depth + 1, out);
            }
            pad(depth, out);
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_float_repr() {
        let cases = [
            (4.0, "4.0"),
            (0.15, "0.15"),
            (999.0, "999.0"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (-2.5, "-2.5"),
            (1.5e-7, "1.5e-07"),
            (0.381, "0.381"),
            (123456789.125, "123456789.125"),
        ];
        for (x, want) in cases {
            assert_eq!(float_repr(x), want, "{x:e}");
        }
    }

    #[test]
    fn python_format_g() {
        let cases = [
            (712.5, "712.5"),
            (1200.0, "1200"),
            (1e6, "1e+06"),
            (123456.7, "123457"),
            (0.0001, "0.0001"),
            (0.00001234, "1.234e-05"),
            (337.5, "337.5"),
            (10.0 / 3.0, "3.33333"),
            (-50.25, "-50.25"),
            (0.0, "0"),
        ];
        for (x, want) in cases {
            assert_eq!(fmt_g(x), want, "{x}");
        }
    }

    #[test]
    fn rounding_rules() {
        assert_eq!(rhaz(2.5), 3);
        assert_eq!(rhaz(-2.5), -3);
        assert_eq!(rhaz(0.49999999999999994), 1, "floor(x + 0.5), as the Python");
        assert_eq!(round6(15.24 / 2.0), 7.62);
        assert_eq!(round6(0.0254 * 155.0), 3.937);
        assert_eq!(round6(1.23456789), 1.234568);
    }

    #[test]
    fn json_dumps_matches_python_layout() {
        let j = Json::obj()
            .with("a", 1)
            .with("b", Json::Arr(vec![Json::Float(1.0), Json::Str("x\"é".into())]))
            .with("c", Json::obj())
            .with("d", Json::Arr(vec![]))
            .with("e", Json::obj().with("f", Json::Null));
        let want = "{\n  \"a\": 1,\n  \"b\": [\n    1.0,\n    \"x\\\"\\u00e9\"\n  ],\n  \"c\": {},\n  \"d\": [],\n  \"e\": {\n    \"f\": null\n  }\n}";
        assert_eq!(j.dumps(), want);
        let mut j = Json::obj().with("k", 1).with("z", 2);
        j.set("k", 5);
        assert_eq!(j.dumps(), "{\n  \"k\": 5,\n  \"z\": 2\n}", "replace in place keeps the order");
    }
}
