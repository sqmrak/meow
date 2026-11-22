use super::Value;

fn string(s: &str, out: &mut String) {
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

fn node(v: &Value, out: &mut String) -> Result<(), String> {
    match v {
        Value::Dict(d) => {
            out.push('{');
            for (i, (k, x)) in d.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(k, out);
                out.push(':');
                node(x, out)?;
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                node(x, out)?;
            }
            out.push(']');
        }
        Value::String(s) => string(s, out),
        Value::Int(n) => out.push_str(&n.to_string()),
        Value::Real(r) if r.is_finite() => out.push_str(&format!("{}", r)),
        Value::Real(_) => return Err("nan and infinity cannot be written as json".to_string()),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Date(_) => return Err("a date cannot be written as json".to_string()),
        Value::Data(_) => return Err("data cannot be written as json".to_string()),
        Value::Uid(_) => return Err("a keyed archive reference cannot be written as json".to_string()),
    }
    Ok(())
}

pub fn write(v: &Value) -> Result<String, String> {
    let mut out = String::new();
    node(v, &mut out)?;
    out.push('\n');
    Ok(out)
}
