//! A small JSON value with a writer and a parser (the tool takes no crates
//! beyond windows-sys).

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn f64(&self, key: &str) -> f64 {
        match self.get(key) {
            Some(Value::Num(n)) => *n,
            _ => 0.0,
        }
    }

    pub fn str(&self, key: &str) -> &str {
        match self.get(key) {
            Some(Value::Str(s)) => s,
            _ => "",
        }
    }

    pub fn arr(&self, key: &str) -> &[Value] {
        match self.get(key) {
            Some(Value::Arr(a)) => a,
            _ => &[],
        }
    }

    pub fn obj(&self, key: &str) -> &[(String, Value)] {
        match self.get(key) {
            Some(Value::Obj(o)) => o,
            _ => &[],
        }
    }

    pub fn num(&self) -> f64 {
        if let Value::Num(n) = self { *n } else { 0.0 }
    }
}

pub fn escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Writes `v`; objects and arrays nested at most `depth` deep get a line per
/// entry, deeper ones stay on one line.
pub fn write(v: &Value, out: &mut String, indent: usize, depth: usize) {
    let pad = |out: &mut String, n: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', n));
    };
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Num(n) => {
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                out.push_str(&format!("{}", *n as i64));
            } else if n.is_finite() {
                out.push_str(&format!("{n:.4}"));
            } else {
                out.push_str("null");
            }
        }
        Value::Str(s) => escape(s, out),
        Value::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if depth > 0 {
                    pad(out, indent + 2);
                }
                write(x, out, indent + 2, depth.saturating_sub(1));
            }
            if depth > 0 && !a.is_empty() {
                pad(out, indent);
            }
            out.push(']');
        }
        Value::Obj(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if depth > 0 {
                    pad(out, indent + 2);
                }
                escape(k, out);
                out.push(':');
                write(x, out, indent + 2, depth.saturating_sub(1));
            }
            if depth > 0 && !o.is_empty() {
                pad(out, indent);
            }
            out.push('}');
        }
    }
}

pub fn parse(s: &str) -> Result<Value, String> {
    let mut p = Parser { b: s.as_bytes(), i: 0 };
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(format!("trailing data at byte {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.i))
    }

    fn lit(&mut self, word: &str, v: Value) -> Result<Value, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            self.err("bad literal")
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.ws();
        match self.b.get(self.i) {
            None => self.err("unexpected end"),
            Some(b'{') => {
                self.i += 1;
                let mut o = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Value::Obj(o));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return self.err("expected ':'");
                    }
                    self.i += 1;
                    o.push((k, self.value()?));
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Obj(o));
                        }
                        _ => return self.err("expected ',' or '}'"),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Value::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::Arr(a));
                        }
                        _ => return self.err("expected ',' or ']'"),
                    }
                }
            }
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') => self.lit("true", Value::Bool(true)),
            Some(b'f') => self.lit("false", Value::Bool(false)),
            Some(b'n') => self.lit("null", Value::Null),
            Some(_) => {
                let start = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[start..self.i])
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .map(Value::Num)
                    .map_or_else(|| self.err("bad number"), Ok)
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.b.get(self.i) != Some(&b'"') {
            return self.err("expected string");
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(&c) = self.b.get(self.i) else { return self.err("unterminated string") };
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.b.get(self.i) else { return self.err("bad escape") };
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = |p: &mut Self| -> Result<u32, String> {
                                let h = p.b.get(p.i..p.i + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u32::from_str_radix(h, 16).ok());
                                p.i += 4;
                                h.map_or_else(|| p.err("bad \\u escape"), Ok)
                            };
                            let mut u = hex(self)?;
                            if (0xd800..0xdc00).contains(&u) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = hex(self)?;
                                u = 0x10000 + ((u - 0xd800) << 10) + (lo.wrapping_sub(0xdc00) & 0x3ff);
                            }
                            let ch = char::from_u32(u).unwrap_or('\u{fffd}');
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        _ => return self.err("bad escape"),
                    }
                }
                c => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| "bad UTF-8".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = Value::Obj(vec![
            ("a".into(), Value::Num(3.0)),
            ("b".into(), Value::Arr(vec![Value::Str("x\"y\\z\n\u{1}é".into()), Value::Num(0.25), Value::Null, Value::Bool(true)])),
            ("c".into(), Value::Obj(vec![])),
            ("d".into(), Value::Arr(vec![])),
        ]);
        for depth in [0, 1, 5] {
            let mut s = String::new();
            write(&v, &mut s, 0, depth);
            assert_eq!(parse(&s).unwrap(), v, "{s}");
        }
        assert_eq!(parse(r#" {"k": [1, -2.5e1, "é😀"]} "#).unwrap().arr("k")[2], Value::Str("é😀".into()));
        assert!(parse("{\"a\":1,}").is_err());
        assert!(parse("[1 2]").is_err());
    }
}
