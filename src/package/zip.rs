// zip writer for ipa files, no zip64 because an installer would not read it either

const DOS_TIME: u16 = 0;
// 2020-01-01, so the same payload gives the same bytes
const DOS_DATE: u16 = (40 << 9) | (1 << 5) | 1;
const MAX_ENTRIES: usize = 65_535;

#[derive(Debug, Clone)]
pub struct Entry {
    // a directory name ends with /
    pub name: String,
    pub data: Vec<u8>,
    pub mode: u32,
    pub link: bool,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = !0u32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

pub fn deflate(data: &[u8]) -> Result<Vec<u8>, String> {
    crate::deflate::compress(data)
}

fn u16le(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn u32le(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn to_u32(x: usize, what: &str) -> Result<u32, String> {
    u32::try_from(x).map_err(|_| format!("{} is over 4 GiB, zip64 is not supported", what))
}

pub fn write(entries: &[Entry]) -> Result<Vec<u8>, String> {
    if entries.len() > MAX_ENTRIES {
        return Err(format!("{} entries, a zip holds at most {} without zip64", entries.len(), MAX_ENTRIES));
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let name = e.name.as_bytes();
        if name.is_empty() || name.len() > u16::MAX as usize {
            return Err(format!("bad entry name {:?}", e.name));
        }
        let crc = crc32(&e.data);
        let (method, body) = if e.is_dir() || e.link || e.data.is_empty() {
            (0u16, e.data.clone())
        } else {
            let d = deflate(&e.data)?;
            // a stream that did not shrink is stored as it is
            if d.len() < e.data.len() { (8u16, d) } else { (0u16, e.data.clone()) }
        };
        let flags: u16 = if e.name.is_ascii() { 0 } else { 0x0800 };
        let offset = to_u32(out.len(), "the archive")?;
        let usize_ = to_u32(e.data.len(), &e.name)?;
        let csize = to_u32(body.len(), &e.name)?;
        let needed: u16 = if method == 8 { 20 } else { 10 };

        u32le(&mut out, 0x0403_4b50);
        u16le(&mut out, needed);
        u16le(&mut out, flags);
        u16le(&mut out, method);
        u16le(&mut out, DOS_TIME);
        u16le(&mut out, DOS_DATE);
        u32le(&mut out, crc);
        u32le(&mut out, csize);
        u32le(&mut out, usize_);
        u16le(&mut out, name.len() as u16);
        u16le(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(&body);

        let kind: u32 = if e.link { 0o120000 } else if e.is_dir() { 0o040000 } else { 0o100000 };
        let attrs = ((kind | (e.mode & 0o7777)) << 16) | if e.is_dir() { 0x10 } else { 0 };
        u32le(&mut central, 0x0201_4b50);
        u16le(&mut central, (3 << 8) | 30);
        u16le(&mut central, needed);
        u16le(&mut central, flags);
        u16le(&mut central, method);
        u16le(&mut central, DOS_TIME);
        u16le(&mut central, DOS_DATE);
        u32le(&mut central, crc);
        u32le(&mut central, csize);
        u32le(&mut central, usize_);
        u16le(&mut central, name.len() as u16);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u16le(&mut central, 0);
        u32le(&mut central, attrs);
        u32le(&mut central, offset);
        central.extend_from_slice(name);
    }
    let cd_offset = to_u32(out.len(), "the archive")?;
    let cd_size = to_u32(central.len(), "the central directory")?;
    out.extend_from_slice(&central);
    u32le(&mut out, 0x0605_4b50);
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, entries.len() as u16);
    u16le(&mut out, entries.len() as u16);
    u32le(&mut out, cd_size);
    u32le(&mut out, cd_offset);
    u16le(&mut out, 0);
    Ok(out)
}

#[derive(Debug)]
pub struct Listed {
    pub name: String,
    pub method: u16,
    pub crc: u32,
    pub usize_: u32,
    pub mode: u32,
    pub body: std::ops::Range<usize>,
}

fn rd16(b: &[u8], at: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(b.get(at..at + 2).ok_or("truncated zip")?.try_into().unwrap()))
}

fn rd32(b: &[u8], at: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(b.get(at..at + 4).ok_or("truncated zip")?.try_into().unwrap()))
}

// reads the central directory and checks every local header against it
pub fn list(zip: &[u8]) -> Result<Vec<Listed>, String> {
    if zip.len() < 22 {
        return Err("too short for a zip".to_string());
    }
    let eocd = zip.len() - 22;
    if rd32(zip, eocd)? != 0x0605_4b50 {
        return Err("no end of central directory record".to_string());
    }
    let count = rd16(zip, eocd + 10)? as usize;
    let cd_size = rd32(zip, eocd + 12)? as usize;
    let cd_off = rd32(zip, eocd + 16)? as usize;
    if cd_off.checked_add(cd_size) != Some(eocd) {
        return Err("central directory does not end at the end record".to_string());
    }
    let mut at = cd_off;
    let mut out = Vec::new();
    for _ in 0..count {
        if rd32(zip, at)? != 0x0201_4b50 {
            return Err("bad central directory entry".to_string());
        }
        let method = rd16(zip, at + 10)?;
        let crc = rd32(zip, at + 16)?;
        let csize = rd32(zip, at + 20)?;
        let usize_ = rd32(zip, at + 24)?;
        let nlen = rd16(zip, at + 28)? as usize;
        let xlen = rd16(zip, at + 30)? as usize;
        let clen = rd16(zip, at + 32)? as usize;
        let attrs = rd32(zip, at + 38)?;
        let local = rd32(zip, at + 42)? as usize;
        let name = String::from_utf8(zip.get(at + 46..at + 46 + nlen).ok_or("truncated zip")?.to_vec()).map_err(|_| "entry name is not utf-8")?;
        if rd32(zip, local)? != 0x0403_4b50 {
            return Err(format!("{}: bad local header", name));
        }
        let lname = rd16(zip, local + 26)? as usize;
        let lextra = rd16(zip, local + 28)? as usize;
        let start = local + 30 + lname + lextra;
        let end = start.checked_add(csize as usize).filter(|&e| e <= cd_off).ok_or_else(|| format!("{}: data runs past the central directory", name))?;
        out.push(Listed { name, method, crc, usize_, mode: attrs >> 16, body: start..end });
        at += 46 + nlen + xlen + clen;
    }
    Ok(out)
}

// a damaged stream or a wrong crc would only show up on the device otherwise
pub fn check_entry(zip: &[u8], e: &Listed) -> Result<(), String> {
    let body = &zip[e.body.clone()];
    let data = match e.method {
        0 => body.to_vec(),
        8 => crate::png::inflate(body, e.usize_ as usize).map_err(|x| format!("{}: the deflate stream is damaged: {}", e.name, x))?,
        m => return Err(format!("{}: compression method {} is not deflate or store", e.name, m)),
    };
    if data.len() != e.usize_ as usize || crc32(&data) != e.crc {
        return Err(format!("{}: contents do not match the recorded size and crc", e.name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, data: &[u8], mode: u32) -> Entry {
        Entry { name: name.to_string(), data: data.to_vec(), mode, link: false }
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn written_zip_lists_and_checks() {
        let big: Vec<u8> = b"payload ".repeat(5000);
        let entries = vec![
            entry("Payload/", b"", 0o755),
            entry("Payload/A.app/", b"", 0o755),
            entry("Payload/A.app/A", &big, 0o755),
            entry("Payload/A.app/tiny", b"x", 0o644),
            entry("Payload/A.app/empty", b"", 0o644),
            Entry { name: "Payload/A.app/link".into(), data: b"tiny".to_vec(), mode: 0o777, link: true },
        ];
        let zip = write(&entries).unwrap();
        let listed = list(&zip).unwrap();
        assert_eq!(listed.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>());
        let a = &listed[2];
        assert_eq!(a.method, 8);
        assert_eq!(a.mode, 0o100755);
        assert!(a.body.len() < a.usize_ as usize);
        // one byte does not shrink, so it is stored
        assert_eq!(listed[3].method, 0);
        assert_eq!(listed[5].mode & 0o170000, 0o120000);
        for l in &listed {
            check_entry(&zip, l).unwrap();
        }
        // the same input twice gives the same bytes
        assert_eq!(zip, write(&entries).unwrap());
    }

    #[test]
    fn a_flipped_byte_is_caught() {
        let big: Vec<u8> = b"abcdefgh".repeat(4000);
        let mut zip = write(&[entry("f", &big, 0o644)]).unwrap();
        let l = list(&zip).unwrap().remove(0);
        let mid = l.body.start + (l.body.end - l.body.start) / 2;
        zip[mid] ^= 0xff;
        let l = list(&zip).unwrap().remove(0);
        assert!(check_entry(&zip, &l).is_err());
    }

    #[test]
    fn damaged_archives_are_errors() {
        assert!(list(b"").is_err());
        assert!(list(&[0u8; 40]).is_err());
        let zip = write(&[entry("f", b"hello hello hello", 0o644)]).unwrap();
        assert!(list(&zip[..zip.len() - 3]).is_err());
        let mut bad = zip.clone();
        bad[0] = 0;
        assert!(list(&bad).is_err());
    }
}
