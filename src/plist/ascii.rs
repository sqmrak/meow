use super::{Value, MAX_DEPTH};

// old style text plists, `{ Filter = { Bundles = ( "a" ); }; }`. every value is a string
// or data, the format has no numbers or booleans

struct A<'a> {
    s: &'a str,
    i: usize,
}

fn delimiter(c: u8) -> bool {
    c.is_ascii_whitespace() || matches!(c, b';' | b',' | b'=' | b'{' | b'}' | b'(' | b')' | b'<' | b'>' | b'"')
}

impl<'a> A<'a> {
    fn skip(&mut self) -> Result<(), String> {
        loop {
            let rest = &self.s[self.i..];
            let t = rest.trim_start();
            self.i += rest.len() - t.len();
            if t.starts_with("//") {
                self.i += t.find('\n').unwrap_or(t.len());
            } else if t.starts_with("/*") {
                let e = t.find("*/").ok_or("an unterminated comment")?;
                self.i += e + 2;
            } else {
                return Ok(());
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn quoted(&mut self) -> Result<String, String> {
        let q = self.peek().unwrap_or(b'"');
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = self.s[self.i..].chars().next().ok_or("an unterminated string")?;
            self.i += c.len_utf8();
            if c as u32 == q as u32 {
                return Ok(out);
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            let e = self.s[self.i..].chars().next().ok_or("an unterminated string")?;
            self.i += e.len_utf8();
            match e {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'a' => out.push('\x07'),
                'b' => out.push('\x08'),
                'f' => out.push('\x0c'),
                'v' => out.push('\x0b'),
                'U' | 'u' => {
                    let h = self.s.get(self.i..self.i + 4).ok_or("a short \\U escape")?;
                    let code = u32::from_str_radix(h, 16).map_err(|_| "a bad \\U escape".to_string())?;
                    out.push(char::from_u32(code).ok_or("a bad \\U escape")?);
                    self.i += 4;
                }
                '0'..='7' => {
                    let mut n = e.to_digit(8).unwrap_or(0);
                    for _ in 0..2 {
                        match self.peek() {
                            Some(d @ b'0'..=b'7') => {
                                n = n * 8 + (d - b'0') as u32;
                                self.i += 1;
                            }
                            _ => break,
                        }
                    }
                    out.push(char::from_u32(n).ok_or("a bad octal escape")?);
                }
                other => out.push(other),
            }
        }
    }

    fn word(&mut self) -> Result<String, String> {
        let start = self.i;
        while self.peek().is_some_and(|c| !delimiter(c)) {
            self.i += 1;
        }
        if self.i == start {
            return Err(format!("unexpected character at byte {}", start));
        }
        Ok(self.s[start..self.i].to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        self.skip()?;
        match self.peek() {
            Some(b'"') | Some(b'\'') => self.quoted(),
            _ => self.word(),
        }
    }

    fn entries(&mut self, first: Option<String>, close: Option<u8>, depth: usize) -> Result<Value, String> {
        let mut d = Vec::new();
        let mut pending = first;
        loop {
            self.skip()?;
            let key = match pending.take() {
                Some(k) => k,
                None => {
                    if close.is_some() && self.peek() == close {
                        self.i += 1;
                        return Ok(Value::Dict(d));
                    }
                    if close.is_none() && self.peek().is_none() {
                        return Ok(Value::Dict(d));
                    }
                    self.string()?
                }
            };
            self.skip()?;
            let v = match self.peek() {
                Some(b'=') => {
                    self.i += 1;
                    self.value(depth + 1)?
                }
                // `key;` means key = key
                _ => Value::String(key.clone()),
            };
            self.skip()?;
            if self.peek() == Some(b';') {
                self.i += 1;
            } else if !(close.is_some() && self.peek() == close) && !(close.is_none() && self.peek().is_none()) {
                return Err(format!("expected ; at byte {}", self.i));
            }
            d.push((key, v));
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("the plist nests too deep".to_string());
        }
        self.skip()?;
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                self.entries(None, Some(b'}'), depth)
            }
            Some(b'(') => {
                self.i += 1;
                let mut a = Vec::new();
                loop {
                    self.skip()?;
                    match self.peek() {
                        Some(b')') => {
                            self.i += 1;
                            return Ok(Value::Array(a));
                        }
                        Some(b',') => self.i += 1,
                        None => return Err("an unterminated array".to_string()),
                        _ => a.push(self.value(depth + 1)?),
                    }
                }
            }
            Some(b'<') => {
                let e = self.s[self.i..].find('>').ok_or("unterminated data")? + self.i;
                let hex: String = self.s[self.i + 1..e].chars().filter(|c| !c.is_whitespace()).collect();
                if hex.len() % 2 != 0 || !hex.is_ascii() {
                    return Err("bad hex data".to_string());
                }
                let mut data = Vec::new();
                for k in (0..hex.len()).step_by(2) {
                    data.push(u8::from_str_radix(&hex[k..k + 2], 16).map_err(|_| "bad hex data".to_string())?);
                }
                self.i = e + 1;
                Ok(Value::Data(data))
            }
            _ => self.string().map(Value::String),
        }
    }
}

pub fn parse(text: &str) -> Result<Value, String> {
    let mut a = A { s: text.strip_prefix('\u{feff}').unwrap_or(text), i: 0 };
    a.skip()?;
    if a.peek().is_none() {
        return Err("an empty file".to_string());
    }
    let v = a.value(0)?;
    a.skip()?;
    // a strings file: `"key" = "value";` without braces
    if let (Value::String(k), Some(b'=')) = (&v, a.peek()) {
        return a.entries(Some(k.clone()), None, 0);
    }
    if a.peek().is_some() {
        return Err(format!("text after the value at byte {}", a.i));
    }
    Ok(v)
}
