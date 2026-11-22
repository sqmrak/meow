use super::{Value, MAX_DEPTH};
use std::collections::HashMap;

// bplist00: objects, an offset table and a 32 byte trailer
const MAX_NODES: usize = 5_000_000;

fn be(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |a, &x| a << 8 | x as u64)
}

struct R<'a> {
    b: &'a [u8],
    offsets: Vec<usize>,
    ref_size: usize,
    table: usize,
    nodes: usize,
}

impl<'a> R<'a> {
    fn bytes(&self, at: usize, n: usize) -> Result<&'a [u8], String> {
        let end = at.checked_add(n).ok_or("an object runs past the end")?;
        if end > self.table {
            return Err("an object runs into the offset table".to_string());
        }
        Ok(&self.b[at..end])
    }

    // the count of an object: the low nibble, or a following integer when it is 15
    fn count(&self, low: u8, at: usize) -> Result<(usize, usize), String> {
        if low != 0xf {
            return Ok((low as usize, at));
        }
        let m = *self.bytes(at, 1)?.first().ok_or("a count is cut off")?;
        if m >> 4 != 1 {
            return Err("a count is not an integer".to_string());
        }
        let size = 1usize << (m & 0xf);
        if size > 8 {
            return Err("a count is too large".to_string());
        }
        let n = be(self.bytes(at + 1, size)?);
        Ok((usize::try_from(n).map_err(|_| "a count is too large")?, at + 1 + size))
    }

    fn object(&mut self, idx: usize, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("the plist nests too deep, or refers to itself".to_string());
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err("the plist has too many objects".to_string());
        }
        let at = *self.offsets.get(idx).ok_or("an object reference is out of range")?;
        let m = *self.bytes(at, 1)?.first().ok_or("an object is cut off")?;
        let (hi, lo) = (m >> 4, m & 0xf);
        match hi {
            0x0 => match lo {
                0x8 => Ok(Value::Bool(false)),
                0x9 => Ok(Value::Bool(true)),
                _ => Err(format!("unsupported object marker {:#04x}", m)),
            },
            0x1 => {
                let size = 1usize << lo;
                if size > 16 {
                    return Err("an integer is too large".to_string());
                }
                let raw = self.bytes(at + 1, size)?;
                if size == 16 {
                    if raw[..8].iter().any(|&x| x != 0) {
                        return Err("an integer does not fit in 64 bits".to_string());
                    }
                    return i64::try_from(be(&raw[8..])).map(Value::Int).map_err(|_| "an integer does not fit in 64 bits".to_string());
                }
                let n = be(raw);
                Ok(Value::Int(n as i64))
            }
            0x2 => match lo {
                2 => Ok(Value::Real(f32::from_bits(be(self.bytes(at + 1, 4)?) as u32) as f64)),
                3 => Ok(Value::Real(f64::from_bits(be(self.bytes(at + 1, 8)?)))),
                _ => Err("an unsupported real size".to_string()),
            },
            0x3 if lo == 3 => Ok(Value::Date(f64::from_bits(be(self.bytes(at + 1, 8)?)))),
            0x4 => {
                let (n, p) = self.count(lo, at + 1)?;
                Ok(Value::Data(self.bytes(p, n)?.to_vec()))
            }
            0x5 => {
                let (n, p) = self.count(lo, at + 1)?;
                let raw = self.bytes(p, n)?;
                if !raw.is_ascii() {
                    return Err("an ascii string holds other bytes".to_string());
                }
                Ok(Value::String(String::from_utf8_lossy(raw).into_owned()))
            }
            0x6 => {
                let (n, p) = self.count(lo, at + 1)?;
                let raw = self.bytes(p, n.checked_mul(2).ok_or("a string is too long")?)?;
                let units: Vec<u16> = raw.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                String::from_utf16(&units).map(Value::String).map_err(|_| "a damaged utf-16 string".to_string())
            }
            0x7 => {
                let (n, p) = self.count(lo, at + 1)?;
                String::from_utf8(self.bytes(p, n)?.to_vec()).map(Value::String).map_err(|_| "a damaged utf-8 string".to_string())
            }
            0x8 => Ok(Value::Uid(be(self.bytes(at + 1, lo as usize + 1)?))),
            0xa | 0xc => {
                let (n, p) = self.count(lo, at + 1)?;
                let refs = self.refs(p, n)?;
                let mut a = Vec::with_capacity(n.min(1024));
                for r in refs {
                    a.push(self.object(r, depth + 1)?);
                }
                Ok(Value::Array(a))
            }
            0xd => {
                let (n, p) = self.count(lo, at + 1)?;
                let refs = self.refs(p, n.checked_mul(2).ok_or("a dict is too large")?)?;
                let mut d = Vec::with_capacity(n.min(1024));
                for i in 0..n {
                    let Value::String(k) = self.object(refs[i], depth + 1)? else {
                        return Err("a dict key is not a string".to_string());
                    };
                    d.push((k, self.object(refs[n + i], depth + 1)?));
                }
                Ok(Value::Dict(d))
            }
            _ => Err(format!("unsupported object marker {:#04x}", m)),
        }
    }

    fn refs(&self, at: usize, n: usize) -> Result<Vec<usize>, String> {
        let raw = self.bytes(at, n.checked_mul(self.ref_size).ok_or("too many references")?)?;
        Ok(raw.chunks(self.ref_size).map(|c| be(c) as usize).collect())
    }
}

pub fn parse(b: &[u8]) -> Result<Value, String> {
    if b.len() < 8 + 1 + 32 {
        return Err("a binary plist that is too short".to_string());
    }
    let t = &b[b.len() - 32..];
    let (off_size, ref_size) = (t[6] as usize, t[7] as usize);
    let (num, top, table) = (be(&t[8..16]), be(&t[16..24]), be(&t[24..32]));
    if !(1..=8).contains(&off_size) || !(1..=8).contains(&ref_size) {
        return Err("a bad size in the trailer".to_string());
    }
    let limit = b.len() - 32;
    let num = usize::try_from(num).map_err(|_| "too many objects")?;
    let table = usize::try_from(table).map_err(|_| "a bad offset table")?;
    if num == 0 || num > limit || table < 8 || table.checked_add(num.checked_mul(off_size).ok_or("too many objects")?).map_or(true, |e| e > limit) {
        return Err("a bad offset table".to_string());
    }
    if top >= num as u64 {
        return Err("the top object is out of range".to_string());
    }
    let mut offsets = Vec::with_capacity(num);
    for i in 0..num {
        let o = be(&b[table + i * off_size..table + (i + 1) * off_size]) as usize;
        if o < 8 || o >= table {
            return Err("an object offset is out of range".to_string());
        }
        offsets.push(o);
    }
    R { b, offsets, ref_size, table, nodes: 0 }.object(top as usize, 0)
}

enum Obj {
    Bytes(Vec<u8>),
    Array(Vec<usize>),
    Dict(Vec<usize>, Vec<usize>),
}

struct W {
    objs: Vec<Obj>,
    strings: HashMap<String, usize>,
}

fn size_class(n: u64) -> usize {
    if n <= 0xff { 1 } else if n <= 0xffff { 2 } else if n <= 0xffff_ffff { 4 } else { 8 }
}

fn int_obj(n: u64) -> Vec<u8> {
    let s = size_class(n);
    let mut o = vec![0x10 | s.trailing_zeros() as u8];
    o.extend_from_slice(&n.to_be_bytes()[8 - s..]);
    o
}

fn head(marker: u8, n: usize) -> Vec<u8> {
    if n < 15 {
        vec![marker | n as u8]
    } else {
        let mut o = vec![marker | 0xf];
        o.extend(int_obj(n as u64));
        o
    }
}

impl W {
    fn add(&mut self, o: Obj) -> usize {
        self.objs.push(o);
        self.objs.len() - 1
    }

    fn string(&mut self, s: &str) -> usize {
        if let Some(&i) = self.strings.get(s) {
            return i;
        }
        let mut o;
        if s.is_ascii() {
            o = head(0x50, s.len());
            o.extend_from_slice(s.as_bytes());
        } else {
            let u: Vec<u16> = s.encode_utf16().collect();
            o = head(0x60, u.len());
            for x in u {
                o.extend_from_slice(&x.to_be_bytes());
            }
        }
        let i = self.add(Obj::Bytes(o));
        self.strings.insert(s.to_string(), i);
        i
    }

    fn value(&mut self, v: &Value) -> usize {
        match v {
            Value::Dict(d) => {
                let me = self.add(Obj::Dict(Vec::new(), Vec::new()));
                let keys: Vec<usize> = d.iter().map(|(k, _)| self.string(k)).collect();
                let vals: Vec<usize> = d.iter().map(|(_, x)| self.value(x)).collect();
                self.objs[me] = Obj::Dict(keys, vals);
                me
            }
            Value::Array(a) => {
                let me = self.add(Obj::Array(Vec::new()));
                let items: Vec<usize> = a.iter().map(|x| self.value(x)).collect();
                self.objs[me] = Obj::Array(items);
                me
            }
            Value::String(s) => self.string(s),
            Value::Int(n) => {
                if *n >= 0 {
                    self.add(Obj::Bytes(int_obj(*n as u64)))
                } else {
                    let mut o = vec![0x13];
                    o.extend_from_slice(&n.to_be_bytes());
                    self.add(Obj::Bytes(o))
                }
            }
            Value::Real(r) => {
                let mut o = vec![0x23];
                o.extend_from_slice(&r.to_bits().to_be_bytes());
                self.add(Obj::Bytes(o))
            }
            Value::Bool(b) => self.add(Obj::Bytes(vec![if *b { 0x09 } else { 0x08 }])),
            Value::Date(d) => {
                let mut o = vec![0x33];
                o.extend_from_slice(&d.to_bits().to_be_bytes());
                self.add(Obj::Bytes(o))
            }
            Value::Data(d) => {
                let mut o = head(0x40, d.len());
                o.extend_from_slice(d);
                self.add(Obj::Bytes(o))
            }
            Value::Uid(u) => {
                let s = size_class(*u);
                let mut o = vec![0x80 | (s as u8 - 1)];
                o.extend_from_slice(&u.to_be_bytes()[8 - s..]);
                self.add(Obj::Bytes(o))
            }
        }
    }
}

pub fn write(v: &Value) -> Result<Vec<u8>, String> {
    let mut w = W { objs: Vec::new(), strings: HashMap::new() };
    w.value(v);
    let ref_size = size_class(w.objs.len() as u64 - 1);
    let mut out = b"bplist00".to_vec();
    let mut offsets = Vec::with_capacity(w.objs.len());
    let put_refs = |out: &mut Vec<u8>, refs: &[usize]| {
        for &r in refs {
            out.extend_from_slice(&(r as u64).to_be_bytes()[8 - ref_size..]);
        }
    };
    for o in &w.objs {
        offsets.push(out.len());
        match o {
            Obj::Bytes(b) => out.extend_from_slice(b),
            Obj::Array(a) => {
                out.extend(head(0xa0, a.len()));
                put_refs(&mut out, a);
            }
            Obj::Dict(k, v) => {
                out.extend(head(0xd0, k.len()));
                put_refs(&mut out, k);
                put_refs(&mut out, v);
            }
        }
    }
    let table = out.len();
    let off_size = size_class(table as u64);
    for o in &offsets {
        out.extend_from_slice(&(*o as u64).to_be_bytes()[8 - off_size..]);
    }
    out.extend_from_slice(&[0; 6]);
    out.push(off_size as u8);
    out.push(ref_size as u8);
    out.extend_from_slice(&(w.objs.len() as u64).to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes());
    out.extend_from_slice(&(table as u64).to_be_bytes());
    Ok(out)
}
