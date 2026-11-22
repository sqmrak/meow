use crate::image::Image;

// png reading and writing without a library. reading takes what editors and rsvg
// write: every color type and bit depth, palette and tRNS, no interlacing. writing
// picks a filter per row and compresses with the in-tree deflate

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
const MAX_SIDE: u32 = 16_384;
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

// ---- inflate, after puff.c from zlib

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    cnt: u32,
}

impl<'a> Bits<'a> {
    fn need(&mut self, n: u32) -> Result<u32, String> {
        let mut v = self.buf;
        while self.cnt < n {
            let b = *self.data.get(self.pos).ok_or("deflate stream ends early")? as u32;
            self.pos += 1;
            v |= b << self.cnt;
            self.cnt += 8;
        }
        self.buf = v >> n;
        self.cnt -= n;
        Ok(v & ((1u32 << n) - 1))
    }
}

struct Huffman {
    count: [u16; 16],
    symbol: Vec<u16>,
}

fn construct(lengths: &[u8]) -> Result<Huffman, String> {
    let mut count = [0u16; 16];
    for &l in lengths {
        count[l as usize] += 1;
    }
    if count[0] as usize == lengths.len() {
        return Ok(Huffman { count, symbol: Vec::new() });
    }
    let mut left: i32 = 1;
    for len in 1..16 {
        left <<= 1;
        left -= count[len] as i32;
        if left < 0 {
            return Err("over-subscribed huffman code".to_string());
        }
    }
    let mut offs = [0u16; 16];
    for len in 1..15 {
        offs[len + 1] = offs[len] + count[len];
    }
    let mut symbol = vec![0u16; lengths.len()];
    for (sym, &l) in lengths.iter().enumerate() {
        if l != 0 {
            symbol[offs[l as usize] as usize] = sym as u16;
            offs[l as usize] += 1;
        }
    }
    // an incomplete code is only valid for a single distance code
    let used: u32 = count[1..].iter().map(|&c| c as u32).sum();
    if left > 0 && used != 1 {
        return Err("incomplete huffman code".to_string());
    }
    Ok(Huffman { count, symbol })
}

fn decode(b: &mut Bits, h: &Huffman) -> Result<u16, String> {
    let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
    for len in 1..16 {
        code |= b.need(1)? as i32;
        let count = h.count[len] as i32;
        if code - count < first {
            return h.symbol.get((index + (code - first)) as usize).copied().ok_or_else(|| "bad huffman symbol".to_string());
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    Err("invalid huffman code".to_string())
}

const LBASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEXT: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DBASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DEXT: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

fn codes(b: &mut Bits, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman, limit: usize) -> Result<(), String> {
    loop {
        let sym = decode(b, lit)? as usize;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let s = sym - 257;
            if s >= 29 {
                return Err("bad length symbol".to_string());
            }
            let len = LBASE[s] as usize + b.need(LEXT[s] as u32)? as usize;
            let ds = decode(b, dist)? as usize;
            if ds >= 30 {
                return Err("bad distance symbol".to_string());
            }
            let d = DBASE[ds] as usize + b.need(DEXT[ds] as u32)? as usize;
            if d > out.len() {
                return Err("distance reaches before the start".to_string());
            }
            for _ in 0..len {
                out.push(out[out.len() - d]);
            }
        }
        if out.len() > limit {
            return Err("deflate output over the size limit".to_string());
        }
    }
}

pub fn inflate(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut b = Bits { data, pos: 0, buf: 0, cnt: 0 };
    let mut out = Vec::new();
    loop {
        let last = b.need(1)?;
        match b.need(2)? {
            0 => {
                b.buf = 0;
                b.cnt = 0;
                let h = data.get(b.pos..b.pos + 4).ok_or("truncated stored block")?;
                let len = u16::from_le_bytes([h[0], h[1]]) as usize;
                if u16::from_le_bytes([h[2], h[3]]) != !(len as u16) {
                    return Err("stored block length check failed".to_string());
                }
                b.pos += 4;
                out.extend_from_slice(data.get(b.pos..b.pos + len).ok_or("truncated stored block")?);
                b.pos += len;
                if out.len() > limit {
                    return Err("deflate output over the size limit".to_string());
                }
            }
            1 => {
                let mut l = [0u8; 288];
                for (i, v) in l.iter_mut().enumerate() {
                    *v = if i < 144 { 8 } else if i < 256 { 9 } else if i < 280 { 7 } else { 8 };
                }
                codes(&mut b, &mut out, &construct(&l)?, &construct(&[5u8; 32])?, limit)?;
            }
            2 => {
                let nlen = b.need(5)? as usize + 257;
                let ndist = b.need(5)? as usize + 1;
                let ncode = b.need(4)? as usize + 4;
                if nlen > 286 || ndist > 30 {
                    return Err("too many length or distance codes".to_string());
                }
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut lengths = [0u8; 320];
                for &o in ORDER.iter().take(ncode) {
                    lengths[o] = b.need(3)? as u8;
                }
                let lencode = construct(&lengths[..19])?;
                let mut idx = 0;
                let mut all = [0u8; 320];
                while idx < nlen + ndist {
                    let sym = decode(&mut b, &lencode)?;
                    if sym < 16 {
                        all[idx] = sym as u8;
                        idx += 1;
                    } else {
                        let (prev, rep) = match sym {
                            16 => {
                                if idx == 0 {
                                    return Err("repeat with nothing before".to_string());
                                }
                                (all[idx - 1], 3 + b.need(2)? as usize)
                            }
                            17 => (0, 3 + b.need(3)? as usize),
                            _ => (0, 11 + b.need(7)? as usize),
                        };
                        if idx + rep > nlen + ndist {
                            return Err("too many code lengths".to_string());
                        }
                        for _ in 0..rep {
                            all[idx] = prev;
                            idx += 1;
                        }
                    }
                }
                if all[256] == 0 {
                    return Err("no end of block code".to_string());
                }
                codes(&mut b, &mut out, &construct(&all[..nlen])?, &construct(&all[nlen..nlen + ndist])?, limit)?;
            }
            _ => return Err("bad deflate block type".to_string()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn zlib_decode(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if data.len() < 6 || data[0] & 0x0f != 8 || ((data[0] as u32) << 8 | data[1] as u32) % 31 != 0 || data[1] & 0x20 != 0 {
        return Err("bad zlib header".to_string());
    }
    let out = inflate(&data[2..data.len() - 4], limit)?;
    let want = u32::from_be_bytes(data[data.len() - 4..].try_into().unwrap());
    if adler32(&out) != want {
        return Err("zlib checksum mismatch".to_string());
    }
    Ok(out)
}

// ---- decode

fn be32(b: &[u8], at: usize) -> Result<u32, String> {
    Ok(u32::from_be_bytes(b.get(at..at + 4).ok_or("truncated png")?.try_into().unwrap()))
}

pub fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    if bytes.get(..8) != Some(&SIGNATURE) {
        return Err("not a png file, convert the source to png".to_string());
    }
    let mut at = 8;
    let (mut w, mut h, mut depth, mut color, mut interlace) = (0u32, 0u32, 0u8, 0u8, 0u8);
    let mut idat = Vec::new();
    let mut plte: Vec<u8> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut seen_ihdr = false;
    loop {
        let len = be32(bytes, at)? as usize;
        let kind = bytes.get(at + 4..at + 8).ok_or("truncated png")?;
        let body = bytes.get(at + 8..at + 8 + len).ok_or("truncated png chunk")?;
        let crc = be32(bytes, at + 8 + len)?;
        if crate::zip::crc32(&bytes[at + 4..at + 8 + len]) != crc {
            return Err(format!("crc mismatch in chunk {}", String::from_utf8_lossy(kind)));
        }
        match kind {
            b"CgBI" => return Err("apple optimized png (CgBI) is not supported, use the original png".to_string()),
            b"IHDR" => {
                if len != 13 {
                    return Err("bad IHDR".to_string());
                }
                w = be32(body, 0)?;
                h = be32(body, 4)?;
                depth = body[8];
                color = body[9];
                interlace = body[12];
                if body[10] != 0 || body[11] != 0 {
                    return Err("unknown png compression or filter method".to_string());
                }
                seen_ihdr = true;
            }
            b"PLTE" => plte = body.to_vec(),
            b"tRNS" => trns = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    if !seen_ihdr || w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || (w as u64) * (h as u64) > MAX_PIXELS {
        return Err("png dimensions are missing or too large".to_string());
    }
    if interlace != 0 {
        return Err("interlaced png is not supported".to_string());
    }
    let channels = match (color, depth) {
        (0, 1 | 2 | 4 | 8 | 16) => 1,
        (2, 8 | 16) => 3,
        (3, 1 | 2 | 4 | 8) => 1,
        (4, 8 | 16) => 2,
        (6, 8 | 16) => 4,
        _ => return Err(format!("png color type {} with depth {} is not valid", color, depth)),
    };
    let bits = channels * depth as usize;
    let stride = (w as usize * bits).div_ceil(8);
    let bpp = bits.div_ceil(8);
    let raw = zlib_decode(&idat, (stride + 1) * h as usize + 16)?;
    if raw.len() != (stride + 1) * h as usize {
        return Err("png data has the wrong length".to_string());
    }
    // unfilter in place into rows
    let mut rows = vec![0u8; stride * h as usize];
    for y in 0..h as usize {
        let ft = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..stride {
            let a = if x >= bpp { rows[y * stride + x - bpp] } else { 0 } as i32;
            let b = if y > 0 { rows[(y - 1) * stride + x] } else { 0 } as i32;
            let c = if y > 0 && x >= bpp { rows[(y - 1) * stride + x - bpp] } else { 0 } as i32;
            let pred = match ft {
                0 => 0,
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                }
                _ => return Err("bad png filter type".to_string()),
            };
            rows[y * stride + x] = line[x].wrapping_add(pred as u8);
        }
    }
    let sample = |row: &[u8], i: usize| -> u16 {
        // i-th sample of a row, any depth
        match depth {
            16 => u16::from_be_bytes([row[2 * i], row[2 * i + 1]]),
            8 => row[i] as u16,
            d => {
                let per = 8 / d as usize;
                let byte = row[i / per];
                let shift = 8 - d as usize * (i % per + 1);
                ((byte >> shift) & ((1u8 << d) - 1)) as u16
            }
        }
    };
    let to8 = |v: u16| -> u8 {
        match depth {
            16 => (v >> 8) as u8,
            8 => v as u8,
            d => (v as u32 * 255 / ((1u32 << d) - 1)) as u8,
        }
    };
    let mut img = Image::new(w as usize, h as usize);
    let key_gray = (trns.len() >= 2 && color == 0).then(|| u16::from_be_bytes([trns[0], trns[1]]));
    let key_rgb = (trns.len() >= 6 && color == 2).then(|| [u16::from_be_bytes([trns[0], trns[1]]), u16::from_be_bytes([trns[2], trns[3]]), u16::from_be_bytes([trns[4], trns[5]])]);
    for y in 0..h as usize {
        let row = &rows[y * stride..(y + 1) * stride];
        for x in 0..w as usize {
            let px = match color {
                0 => {
                    let v = sample(row, x);
                    let g = to8(v);
                    [g, g, g, if key_gray == Some(v) { 0 } else { 255 }]
                }
                2 => {
                    let v = [sample(row, 3 * x), sample(row, 3 * x + 1), sample(row, 3 * x + 2)];
                    [to8(v[0]), to8(v[1]), to8(v[2]), if key_rgb == Some(v) { 0 } else { 255 }]
                }
                3 => {
                    let i = sample(row, x) as usize;
                    let c = plte.get(3 * i..3 * i + 3).ok_or("palette index out of range")?;
                    [c[0], c[1], c[2], trns.get(i).copied().unwrap_or(255)]
                }
                4 => {
                    let g = to8(sample(row, 2 * x));
                    [g, g, g, to8(sample(row, 2 * x + 1))]
                }
                _ => [to8(sample(row, 4 * x)), to8(sample(row, 4 * x + 1)), to8(sample(row, 4 * x + 2)), to8(sample(row, 4 * x + 3))],
            };
            img.set(x, y, px);
        }
    }
    Ok(img)
}

// ---- encode

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    let crc = crate::zip::crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

pub fn encode_png(img: &Image) -> Result<Vec<u8>, String> {
    let opaque = img.px.chunks(4).all(|p| p[3] == 255);
    let channels = if opaque { 3 } else { 4 };
    let stride = img.w * channels;
    let mut raw = Vec::with_capacity((stride + 1) * img.h);
    let mut prev = vec![0u8; stride];
    for y in 0..img.h {
        let mut line = Vec::with_capacity(stride);
        for x in 0..img.w {
            let p = img.get(x, y);
            line.extend_from_slice(&p[..channels]);
        }
        // the filter whose output has the smallest sum of absolute signed bytes
        let mut best: Option<(u64, u8, Vec<u8>)> = None;
        for ft in 0..5u8 {
            let mut f = Vec::with_capacity(stride);
            for x in 0..stride {
                let a = if x >= channels { line[x - channels] } else { 0 } as i32;
                let b = prev[x] as i32;
                let c = if x >= channels { prev[x - channels] } else { 0 } as i32;
                let pred = match ft {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => (a + b) / 2,
                    _ => {
                        let p = a + b - c;
                        let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                        if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                    }
                };
                f.push(line[x].wrapping_sub(pred as u8));
            }
            let score: u64 = f.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum();
            if best.as_ref().map_or(true, |(s, _, _)| score < *s) {
                best = Some((score, ft, f));
            }
        }
        let (_, ft, f) = best.unwrap();
        raw.push(ft);
        raw.extend_from_slice(&f);
        prev = line;
    }
    let mut z = vec![0x78, 0xda];
    z.extend_from_slice(&crate::zip::deflate(&raw)?);
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = SIGNATURE.to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(img.w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(img.h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if opaque { 2 } else { 6 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize, alpha: bool) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set(x, y, [(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, ((x ^ y) & 255) as u8, if alpha { ((x + y) * 3 % 256) as u8 } else { 255 }]);
            }
        }
        img
    }

    #[test]
    fn inflate_reads_what_gzip_writes_in_every_block_kind() {
        // a repetitive input makes dynamic blocks, a tiny one fixed blocks, random bytes stored ones
        let rep: Vec<u8> = (0..30_000u32).flat_map(|i| (i % 97).to_le_bytes()).collect();
        let tiny = b"hello hello hello hello".to_vec();
        let mut noise = Vec::new();
        let mut x = 0x1234_5678u32;
        for _ in 0..4000 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            noise.push((x >> 24) as u8);
        }
        for data in [&rep, &tiny, &noise] {
            let d = crate::zip::deflate(data).unwrap();
            assert_eq!(&inflate(&d, 1 << 20).unwrap(), data);
        }
        assert!(inflate(&crate::zip::deflate(&rep).unwrap(), 100).unwrap_err().contains("size limit"));
        assert!(inflate(&[0xff, 0xff], 100).is_err());
        assert!(inflate(&[], 100).is_err());
    }

    #[test]
    fn png_round_trips_with_and_without_alpha() {
        for alpha in [false, true] {
            let img = gradient(37, 23, alpha);
            let bytes = encode_png(&img).unwrap();
            let back = decode_png(&bytes).unwrap();
            assert_eq!((back.w, back.h), (37, 23));
            assert_eq!(back.px, img.px, "alpha {}", alpha);
        }
    }

    // hand made files for the color types the encoder never writes
    fn make(color: u8, depth: u8, w: u32, h: u32, raw_rows: &[Vec<u8>], extra: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        for (k, v) in extra {
            chunk(&mut out, k, v);
        }
        let mut raw = Vec::new();
        for r in raw_rows {
            raw.push(0);
            raw.extend_from_slice(r);
        }
        let mut z = vec![0x78, 0xda];
        z.extend_from_slice(&crate::zip::deflate(&raw).unwrap());
        z.extend_from_slice(&adler32(&raw).to_be_bytes());
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn palette_gray_and_sixteen_bit_files_decode() {
        // 2 bit palette, 4 pixels in one byte: indexes 0 1 2 3, index 1 is transparent
        let plte = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9];
        let p = decode_png(&make(3, 2, 4, 1, &[vec![0b00_01_10_11]], &[(b"PLTE", plte), (b"tRNS", vec![255, 0])])).unwrap();
        assert_eq!(p.get(0, 0), [255, 0, 0, 255]);
        assert_eq!(p.get(1, 0), [0, 255, 0, 0]);
        assert_eq!(p.get(3, 0), [9, 9, 9, 255]);
        // 1 bit gray scales to 0 and 255
        let g = decode_png(&make(0, 1, 8, 1, &[vec![0b1010_0000]], &[])).unwrap();
        assert_eq!((g.get(0, 0)[0], g.get(1, 0)[0]), (255, 0));
        // 16 bit rgba keeps the high bytes
        let rgba16 = decode_png(&make(6, 16, 1, 1, &[vec![0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xff, 0xff]], &[])).unwrap();
        assert_eq!(rgba16.get(0, 0), [0x12, 0x56, 0x9a, 0xff]);
        // gray + alpha
        let ga = decode_png(&make(4, 8, 1, 1, &[vec![100, 50]], &[])).unwrap();
        assert_eq!(ga.get(0, 0), [100, 100, 100, 50]);
    }

    #[test]
    fn damaged_and_unsupported_files_are_errors() {
        let good = encode_png(&gradient(8, 8, true)).unwrap();
        assert!(decode_png(b"GIF89a").unwrap_err().contains("not a png"));
        let mut bad_crc = good.clone();
        bad_crc[20] ^= 1;
        assert!(decode_png(&bad_crc).is_err());
        assert!(decode_png(&good[..good.len() - 20]).is_err());
        // interlaced
        let mut ihdr_il = SIGNATURE.to_vec();
        chunk(&mut ihdr_il, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 1]);
        chunk(&mut ihdr_il, b"IEND", &[]);
        assert!(decode_png(&ihdr_il).unwrap_err().contains("interlaced"));
        // a huge size claim is refused before any allocation
        let mut huge = SIGNATURE.to_vec();
        chunk(&mut huge, b"IHDR", &[0x7f, 0, 0, 0, 0x7f, 0, 0, 0, 8, 2, 0, 0, 0]);
        chunk(&mut huge, b"IEND", &[]);
        assert!(decode_png(&huge).unwrap_err().contains("too large"));
        // apple optimized files carry CgBI first
        let mut cgbi = SIGNATURE.to_vec();
        chunk(&mut cgbi, b"CgBI", &[0; 4]);
        assert!(decode_png(&cgbi).unwrap_err().contains("CgBI"));
    }
}
