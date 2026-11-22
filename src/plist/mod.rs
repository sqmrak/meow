// property lists: read xml, binary and old style text plists, write xml, binary and json.
// `meow plist` converts between them, `meow migrate` reads filter plists with it

mod ascii;
mod binary;
mod cli;
mod json;
mod xml;

pub use cli::run;

// real plists nest a dozen levels, the limit keeps a damaged or self referring file from overflowing the stack
const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    // keys keep their order
    Dict(Vec<(String, Value)>),
    Array(Vec<Value>),
    String(String),
    Int(i64),
    Real(f64),
    Bool(bool),
    // seconds since 2001-01-01 00:00:00 utc
    Date(f64),
    Data(Vec<u8>),
    // an object reference of a keyed archive
    Uid(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Xml,
    Binary,
    Json,
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    // a dotted path, dict keys and array indexes: Filter.Bundles.0
    pub fn path(&self, path: &str) -> Option<&Value> {
        let mut cur = self;
        for part in path.split('.').filter(|p| !p.is_empty()) {
            cur = match cur {
                Value::Dict(_) => cur.get(part)?,
                Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(cur)
    }
}

pub fn parse(bytes: &[u8]) -> Result<Value, String> {
    if bytes.starts_with(b"bplist00") {
        return binary::parse(bytes);
    }
    let text = decode_text(bytes)?;
    let t = text.trim_start();
    if t.starts_with("<?xml") || t.starts_with("<plist") || t.starts_with("<!DOCTYPE") || t.starts_with("<!--") {
        xml::parse(&text)
    } else {
        ascii::parse(&text)
    }
}

pub fn write(v: &Value, f: Format) -> Result<Vec<u8>, String> {
    match f {
        Format::Xml => Ok(xml::write(v).into_bytes()),
        Format::Binary => binary::write(v),
        Format::Json => json::write(v).map(String::into_bytes),
    }
}

// utf-8, or utf-16 with a byte order mark
fn decode_text(bytes: &[u8]) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]).map(|r| (r, true)).or_else(|| bytes.strip_prefix(&[0xfe, 0xff]).map(|r| (r, false))) {
        let (body, le) = rest;
        if body.len() % 2 != 0 {
            return Err("utf-16 text with an odd number of bytes".to_string());
        }
        let units: Vec<u16> = body.chunks(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        return String::from_utf16(&units).map_err(|_| "damaged utf-16 text".to_string());
    }
    let b = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    String::from_utf8(b.to_vec()).map_err(|_| "the file is not utf-8 text and not a binary plist".to_string())
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub(crate) fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    let mut pad = 0;
    for c in text.bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            pad += 1;
            continue;
        }
        if pad > 0 {
            return Err("base64 data continues after the padding".to_string());
        }
        let v = B64.iter().position(|&x| x == c).ok_or_else(|| format!("bad character {:?} in base64 data", c as char))? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

// days since 1970-01-01 of a civil date, and the reverse
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

// 2001-01-01 is 11323 days after the unix epoch
const EPOCH_2001_DAYS: i64 = 11323;

pub(crate) fn date_to_text(secs: f64) -> String {
    let whole = secs.floor() as i64;
    let days = whole.div_euclid(86400) + EPOCH_2001_DAYS;
    let rem = whole.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

pub(crate) fn date_from_text(t: &str) -> Result<f64, String> {
    let bad = || format!("bad date {:?}", t);
    let b = t.trim().as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') || b[13] != b':' || b[16] != b':' {
        return Err(bad());
    }
    let n = |r: std::ops::Range<usize>| std::str::from_utf8(&b[r]).ok().and_then(|s| s.parse::<i64>().ok()).ok_or_else(bad);
    let (y, mo, d, h, mi, s) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return Err(bad());
    }
    Ok(((days_from_civil(y, mo, d) - EPOCH_2001_DAYS) * 86400 + h * 3600 + mi * 60 + s) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for data in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0, 255, 128, 7, 9]] {
            assert_eq!(base64_decode(&base64_encode(data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert!(base64_decode("a$b=").is_err());
    }

    #[test]
    fn dates() {
        assert_eq!(date_to_text(0.0), "2001-01-01T00:00:00Z");
        assert_eq!(date_from_text("2001-01-02T00:00:01Z").unwrap(), 86401.0);
        assert_eq!(date_to_text(date_from_text("2024-02-29T23:59:58Z").unwrap()), "2024-02-29T23:59:58Z");
        assert_eq!(date_to_text(-1.0), "2000-12-31T23:59:59Z");
        assert!(date_from_text("yesterday").is_err());
    }

    #[test]
    fn paths() {
        let v = Value::Dict(vec![("Filter".into(), Value::Dict(vec![("Bundles".into(), Value::Array(vec![Value::String("a".into())]))]))]);
        assert_eq!(v.path("Filter.Bundles.0").and_then(Value::as_str), Some("a"));
        assert!(v.path("Filter.Bundles.1").is_none());
        assert!(v.path("Nope").is_none());
    }

    fn rich() -> Value {
        Value::Dict(vec![
            ("s".into(), Value::String("h\u{e9}llo \u{20ac} <&>".into())),
            ("i".into(), Value::Array(vec![Value::Int(0), Value::Int(255), Value::Int(256), Value::Int(i64::MAX), Value::Int(i64::MIN), Value::Int(-1)])),
            ("r".into(), Value::Array(vec![Value::Real(1.5), Value::Real(-0.25), Value::Real(1e20)])),
            ("b".into(), Value::Array(vec![Value::Bool(true), Value::Bool(false)])),
            ("d".into(), Value::Date(date_from_text("2024-02-29T23:59:58Z").unwrap())),
            ("x".into(), Value::Data((0..=255u8).collect())),
            ("n".into(), Value::Dict(vec![("e".into(), Value::Dict(vec![])), ("a".into(), Value::Array(vec![]))])),
            ("long".into(), Value::String("y".repeat(300))),
        ])
    }

    #[test]
    fn every_format_round_trips() {
        for f in [Format::Xml, Format::Binary] {
            let bytes = write(&rich(), f).unwrap();
            assert_eq!(parse(&bytes).unwrap(), rich(), "{:?}", f);
        }
        assert!(write(&rich(), Format::Json).is_err());
    }

    #[test]
    fn reads_a_binary_plist_written_by_python() {
        let hex = "62706c6973743030d50102030405060a0b0c0d51615162516351645165a307080910016100e91300000100000000000942010213fffffffffffffffb23400400000000000008131517191b1d2123262f30333c0000000000000101000000000000000e00000000000000000000000000000045";
        let bytes: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        let v = parse(&bytes).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Array(vec![Value::Int(1), Value::String("\u{e9}".into()), Value::Int(1 << 40)])));
        assert_eq!(v.get("b"), Some(&Value::Bool(true)));
        assert_eq!(v.get("c"), Some(&Value::Data(vec![1, 2])));
        assert_eq!(v.get("d"), Some(&Value::Int(-5)));
        assert_eq!(v.get("e"), Some(&Value::Real(2.5)));
        // the bytes of our own writer are read back the same
        assert_eq!(parse(&write(&v, Format::Binary).unwrap()).unwrap(), v);
    }

    #[test]
    fn text_plists() {
        let v = parse(b"{ Filter = { Bundles = ( \"a.b\", c ); }; /* x */ K = <0a0B>; // y\n Q = \"a\\nb\\U00e9\"; }").unwrap();
        assert_eq!(v.path("Filter.Bundles.1").and_then(Value::as_str), Some("c"));
        assert_eq!(v.get("K"), Some(&Value::Data(vec![10, 11])));
        assert_eq!(v.get("Q").and_then(Value::as_str), Some("a\nb\u{e9}"));
        let strings = parse(b"\"a\" = \"1\";\n\"b\" = \"2\";\n").unwrap();
        assert_eq!(strings.get("b").and_then(Value::as_str), Some("2"));
        assert_eq!(parse(b"( a, b, )").unwrap().as_array().map(|a| a.len()), Some(2));
    }

    #[test]
    fn xml_details() {
        let x = "<?xml version=\"1.0\"?><!-- c --><plist version=\"1.0\"><dict><key>a</key><string>x&amp;y&#65;&#x42;<![CDATA[<z>]]></string><key>t</key><true/><key>e</key><string/><key>r</key><real>nan</real><key>h</key><integer>0x10</integer></dict></plist>";
        let v = parse(x.as_bytes()).unwrap();
        assert_eq!(v.get("a").and_then(Value::as_str), Some("x&yAB<z>"));
        assert_eq!(v.get("t"), Some(&Value::Bool(true)));
        assert_eq!(v.get("e").and_then(Value::as_str), Some(""));
        assert!(matches!(v.get("r"), Some(Value::Real(r)) if r.is_nan()));
        assert_eq!(v.get("h"), Some(&Value::Int(16)));
    }

    #[test]
    fn damaged_input_is_an_error_not_a_panic() {
        for bad in ["<plist><dict><key>a</key></dict></plist>", "<plist><dict>", "<plist><string>x</plist>", "<plist><integer>zz</integer></plist>", "<plist><foo/></plist>", "{ a = ", "( ", "\"abc", "<plist><data>a$</data></plist>", ""] {
            assert!(parse(bad.as_bytes()).is_err(), "{:?}", bad);
        }
        let good = write(&rich(), Format::Binary).unwrap();
        // every truncation and every flipped byte either parses or is an error
        for n in 0..good.len() {
            let _ = parse(&good[..n]);
        }
        for i in 0..good.len() {
            let mut b = good.clone();
            b[i] ^= 0xff;
            let _ = parse(&b);
        }
        // a dict that is its own value does not recurse forever
        let mut cyc = b"bplist00".to_vec();
        cyc.extend_from_slice(&[0xa1, 0x00]);
        let table = cyc.len();
        cyc.push(8);
        cyc.extend_from_slice(&[0; 6]);
        cyc.extend_from_slice(&[1, 1]);
        cyc.extend_from_slice(&1u64.to_be_bytes());
        cyc.extend_from_slice(&0u64.to_be_bytes());
        cyc.extend_from_slice(&(table as u64).to_be_bytes());
        assert!(parse(&cyc).unwrap_err().contains("nests too deep"));
    }
}
