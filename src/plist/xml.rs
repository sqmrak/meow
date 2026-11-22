use super::{base64_decode, base64_encode, date_from_text, date_to_text, Value, MAX_DEPTH};

struct P<'a> {
    s: &'a str,
    i: usize,
}

fn unescape(raw: &str) -> Result<String, String> {
    if !raw.contains('&') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let end = rest[at..].find(';').ok_or("an & without a ; in the text")? + at;
        let ent = &rest[at + 1..end];
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code = if let Some(h) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                    u32::from_str_radix(h, 16).ok()
                } else if let Some(d) = ent.strip_prefix('#') {
                    d.parse().ok()
                } else {
                    None
                };
                out.push(code.and_then(char::from_u32).ok_or_else(|| format!("unknown entity &{};", ent))?);
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

impl<'a> P<'a> {
    fn skip_ws(&mut self) {
        while self.s.as_bytes().get(self.i).is_some_and(|b| b.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    // whitespace, comments, the declaration and the doctype
    fn skip_misc(&mut self) -> Result<(), String> {
        loop {
            self.skip_ws();
            let rest = &self.s[self.i..];
            if rest.starts_with("<?") {
                let e = rest.find("?>").ok_or("an unterminated <? ?> declaration")?;
                self.i += e + 2;
            } else if rest.starts_with("<!--") {
                let e = rest.find("-->").ok_or("an unterminated comment")?;
                self.i += e + 3;
            } else if rest.starts_with("<!DOCTYPE") {
                let mut depth = 0;
                let mut n = 0;
                for (k, c) in rest.char_indices() {
                    match c {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        '>' if depth <= 0 => {
                            n = k + 1;
                            break;
                        }
                        _ => {}
                    }
                }
                if n == 0 {
                    return Err("an unterminated <!DOCTYPE>".to_string());
                }
                self.i += n;
            } else {
                return Ok(());
            }
        }
    }

    // `<name attrs>` or `<name/>`, returns the name and whether it is empty
    fn open(&mut self) -> Result<(String, bool), String> {
        self.skip_misc()?;
        let rest = &self.s[self.i..];
        if !rest.starts_with('<') || rest.starts_with("</") {
            return Err(format!("expected a tag at byte {}", self.i));
        }
        let end = rest.find('>').ok_or("an unterminated tag")?;
        let inner = &rest[1..end];
        self.i += end + 1;
        let empty = inner.ends_with('/');
        let inner = inner.trim_end_matches('/');
        let name = inner.split_whitespace().next().unwrap_or("").to_string();
        Ok((name, empty))
    }

    fn close(&mut self, name: &str) -> Result<(), String> {
        self.skip_misc()?;
        let want = format!("</{}>", name);
        if self.s[self.i..].starts_with(&want) {
            self.i += want.len();
            Ok(())
        } else {
            Err(format!("expected {} at byte {}", want, self.i))
        }
    }

    // character data up to the next tag, cdata sections included
    fn text(&mut self) -> Result<String, String> {
        let mut out = String::new();
        loop {
            let rest = &self.s[self.i..];
            if let Some(r) = rest.strip_prefix("<![CDATA[") {
                let e = r.find("]]>").ok_or("an unterminated CDATA section")?;
                out.push_str(&r[..e]);
                self.i += 9 + e + 3;
                continue;
            }
            if rest.starts_with("<!--") {
                let e = rest.find("-->").ok_or("an unterminated comment")?;
                self.i += e + 3;
                continue;
            }
            let e = rest.find('<').ok_or("the text does not end")?;
            out.push_str(&unescape(&rest[..e])?);
            self.i += e;
            if self.s[self.i..].starts_with("<![CDATA[") || self.s[self.i..].starts_with("<!--") {
                continue;
            }
            return Ok(out);
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("the plist nests too deep".to_string());
        }
        let (name, empty) = self.open()?;
        match name.as_str() {
            "dict" => {
                let mut d = Vec::new();
                if !empty {
                    loop {
                        self.skip_misc()?;
                        if self.s[self.i..].starts_with("</dict>") {
                            self.i += 7;
                            break;
                        }
                        let (k, kempty) = self.open()?;
                        if k != "key" {
                            return Err(format!("expected <key> in a dict, found <{}>", k));
                        }
                        let key = if kempty { String::new() } else { let t = self.text()?; self.close("key")?; t };
                        let v = self.value(depth + 1)?;
                        d.push((key, v));
                    }
                }
                Ok(Value::Dict(d))
            }
            "array" => {
                let mut a = Vec::new();
                if !empty {
                    loop {
                        self.skip_misc()?;
                        if self.s[self.i..].starts_with("</array>") {
                            self.i += 8;
                            break;
                        }
                        a.push(self.value(depth + 1)?);
                    }
                }
                Ok(Value::Array(a))
            }
            "true" | "false" => {
                if !empty {
                    self.close(&name)?;
                }
                Ok(Value::Bool(name == "true"))
            }
            "string" | "integer" | "real" | "date" | "data" => {
                let t = if empty { String::new() } else { let t = self.text()?; self.close(&name)?; t };
                match name.as_str() {
                    "string" => Ok(Value::String(t)),
                    "integer" => {
                        let t = t.trim();
                        let n = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { i64::from_str_radix(h, 16).ok() } else { t.parse::<i64>().ok() };
                        n.map(Value::Int).ok_or_else(|| format!("bad integer {:?}", t))
                    }
                    "real" => {
                        let t = t.trim();
                        let r = match t.to_ascii_lowercase().as_str() {
                            "nan" => Some(f64::NAN),
                            "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
                            "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
                            _ => t.parse::<f64>().ok(),
                        };
                        r.map(Value::Real).ok_or_else(|| format!("bad real {:?}", t))
                    }
                    "date" => date_from_text(&t).map(Value::Date),
                    _ => base64_decode(&t).map(Value::Data),
                }
            }
            other => Err(format!("unknown element <{}>", other)),
        }
    }
}

pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = P { s: text, i: 0 };
    let (name, empty) = p.open()?;
    if name != "plist" {
        return Err(format!("expected <plist>, found <{}>", name));
    }
    if empty {
        return Err("an empty <plist/>".to_string());
    }
    let v = p.value(0)?;
    p.close("plist")?;
    p.skip_misc()?;
    if p.i != text.len() {
        return Err("text after </plist>".to_string());
    }
    Ok(v)
}

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            // characters xml cannot carry are dropped, as plutil does
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => o.push(c),
        }
    }
    o
}

fn real_text(r: f64) -> String {
    if r.is_nan() {
        "nan".to_string()
    } else if r.is_infinite() {
        if r > 0.0 { "inf" } else { "-inf" }.to_string()
    } else if r == r.trunc() && r.abs() < 1e15 {
        format!("{:.1}", r)
    } else {
        format!("{}", r)
    }
}

fn node(v: &Value, tabs: usize, out: &mut String) {
    let pad = "\t".repeat(tabs);
    match v {
        Value::Dict(d) if d.is_empty() => out.push_str(&format!("{}<dict/>\n", pad)),
        Value::Dict(d) => {
            out.push_str(&format!("{}<dict>\n", pad));
            for (k, x) in d {
                out.push_str(&format!("{}\t<key>{}</key>\n", pad, escape(k)));
                node(x, tabs + 1, out);
            }
            out.push_str(&format!("{}</dict>\n", pad));
        }
        Value::Array(a) if a.is_empty() => out.push_str(&format!("{}<array/>\n", pad)),
        Value::Array(a) => {
            out.push_str(&format!("{}<array>\n", pad));
            for x in a {
                node(x, tabs + 1, out);
            }
            out.push_str(&format!("{}</array>\n", pad));
        }
        Value::String(s) => out.push_str(&format!("{}<string>{}</string>\n", pad, escape(s))),
        Value::Int(n) => out.push_str(&format!("{}<integer>{}</integer>\n", pad, n)),
        Value::Real(r) => out.push_str(&format!("{}<real>{}</real>\n", pad, real_text(*r))),
        Value::Bool(b) => out.push_str(&format!("{}<{}/>\n", pad, b)),
        Value::Date(d) => out.push_str(&format!("{}<date>{}</date>\n", pad, date_to_text(*d))),
        Value::Data(d) => {
            out.push_str(&format!("{}<data>\n", pad));
            let b = base64_encode(d);
            for line in b.as_bytes().chunks(68) {
                out.push_str(&format!("{}{}\n", pad, std::str::from_utf8(line).unwrap_or("")));
            }
            out.push_str(&format!("{}</data>\n", pad));
        }
        // a keyed archive reference, written the way plutil writes it
        Value::Uid(u) => {
            out.push_str(&format!("{}<dict>\n{}\t<key>CF$UID</key>\n{}\t<integer>{}</integer>\n{}</dict>\n", pad, pad, pad, u, pad));
        }
    }
}

pub fn write(v: &Value) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n");
    node(v, 0, &mut out);
    out.push_str("</plist>\n");
    out
}
