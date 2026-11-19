use std::cmp::Reverse;
use std::collections::BinaryHeap;

// own compressor so packing and png need no host gzip and equal input gives equal bytes
// the ratio trails gzip -9 a little, any inflater reads the output

const WINDOW: usize = 32_768;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const NONE: u32 = u32::MAX;
// caps the time spent on long repetitive input
const MAX_CHAIN: usize = 256;
const GOOD_MATCH: usize = 32;
// searching longer than this rarely finds a better match
const NICE_MATCH: usize = 128;
// a 3 byte match this far costs more bits than three literals
const TOO_FAR: usize = 4096;
const BLOCK_SYMBOLS: usize = 1 << 16;
const MAX_STORED: usize = 65_535;
const MAX_INPUT: usize = u32::MAX as usize;

const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
// the order the code length code lengths are sent in
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
const EOB: usize = 256;

// one u32 per symbol keeps the block buffer small
// bit 31 marks a match, length sits in bits 16 to 24
const MATCH_FLAG: u32 = 1 << 31;

fn match_sym(len: usize, dist: usize) -> u32 {
    MATCH_FLAG | ((len as u32) << 16) | dist as u32
}

fn len_code(len: usize) -> usize {
    if len == MAX_MATCH {
        return 28;
    }
    LEN_BASE.partition_point(|&b| (b as usize) <= len) - 1
}

fn dist_code(dist: usize) -> usize {
    DIST_BASE.partition_point(|&b| (b as usize) <= dist) - 1
}

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, bits: u32, count: u32) {
        self.acc |= (bits as u64) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    // deflate packs huffman codes msb first into an lsb first stream
    fn code(&mut self, code: u16, len: u8) {
        let mut rev = 0u32;
        for i in 0..len {
            rev |= (((code >> i) & 1) as u32) << (len - 1 - i);
        }
        self.put(rev, len as u32);
    }

    fn align(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc = 0;
            self.n = 0;
        }
    }
}

// halving the weights always ends, equal weights give a balanced tree
// the limit is 15 bits for deflate and 7 for the code length code
fn code_lengths(freq: &[u32], limit: u8) -> Vec<u8> {
    let mut f: Vec<u32> = freq.to_vec();
    loop {
        let lens = build_lengths(&f);
        if lens.iter().all(|&l| l <= limit) {
            return lens;
        }
        for x in f.iter_mut().filter(|x| **x > 0) {
            *x = (*x + 1) / 2;
        }
    }
}

fn build_lengths(freq: &[u32]) -> Vec<u8> {
    let used: Vec<usize> = (0..freq.len()).filter(|&i| freq[i] > 0).collect();
    let mut lens = vec![0u8; freq.len()];
    match used.len() {
        0 => return lens,
        1 => {
            lens[used[0]] = 1;
            return lens;
        }
        _ => {}
    }
    // ties break on node number so the tree does not depend on heap order
    let mut parent: Vec<usize> = vec![usize::MAX; used.len()];
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = BinaryHeap::new();
    for (node, &sym) in used.iter().enumerate() {
        heap.push(Reverse((freq[sym] as u64, node)));
    }
    while heap.len() > 1 {
        let Reverse((wa, a)) = heap.pop().unwrap();
        let Reverse((wb, b)) = heap.pop().unwrap();
        let node = parent.len();
        parent.push(usize::MAX);
        parent[a] = node;
        parent[b] = node;
        heap.push(Reverse((wa + wb, node)));
    }
    for (leaf, &sym) in used.iter().enumerate() {
        let mut depth = 0u32;
        let mut at = leaf;
        while parent[at] != usize::MAX {
            at = parent[at];
            depth += 1;
        }
        lens[sym] = depth.min(255) as u8;
    }
    lens
}

// the decoder rebuilds the codes from lengths alone, so they have to be canonical
fn canonical(lens: &[u8]) -> Vec<u16> {
    let mut count = [0u16; 16];
    for &l in lens {
        count[l as usize] += 1;
    }
    count[0] = 0;
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..16 {
        code = (code + count[bits - 1]) << 1;
        next[bits] = code;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[l as usize];
            next[l as usize] += 1;
            c
        })
        .collect()
}

fn fixed_lit_lens() -> Vec<u8> {
    (0..288).map(|i| if i < 144 { 8 } else if i < 256 { 9 } else if i < 280 { 7 } else { 8 }).collect()
}

struct Plan {
    lit_lens: Vec<u8>,
    dist_lens: Vec<u8>,
    cl_lens: Vec<u8>,
    cl_syms: Vec<(u8, u8, u8)>,
    hlit: usize,
    hdist: usize,
    hclen: usize,
}

fn run_length(lens: &[u8]) -> Vec<(u8, u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lens.len() {
        let v = lens[i];
        let mut run = 1;
        while i + run < lens.len() && lens[i + run] == v {
            run += 1;
        }
        i += run;
        if v == 0 {
            let mut left = run;
            while left >= 3 {
                if left >= 11 {
                    let t = left.min(138);
                    out.push((18, (t - 11) as u8, 7));
                    left -= t;
                } else {
                    out.push((17, (left - 3) as u8, 3));
                    left = 0;
                }
            }
            out.extend(std::iter::repeat((0, 0, 0)).take(left));
        } else {
            out.push((v, 0, 0));
            let mut left = run - 1;
            while left >= 3 {
                let t = left.min(6);
                out.push((16, (t - 3) as u8, 2));
                left -= t;
            }
            out.extend(std::iter::repeat((v, 0, 0)).take(left));
        }
    }
    out
}

fn plan_dynamic(lit_freq: &[u32], dist_freq: &[u32]) -> Plan {
    let lit_lens = code_lengths(lit_freq, 15);
    let mut dist_lens = code_lengths(dist_freq, 15);
    // a block without matches still needs one distance code or inflaters reject it
    if dist_lens.iter().all(|&l| l == 0) {
        dist_lens[0] = 1;
    }
    let hlit = (257..=286).rev().find(|&n| lit_lens[n - 1] != 0).unwrap_or(257).max(257);
    let hdist = (1..=30).rev().find(|&n| dist_lens[n - 1] != 0).unwrap_or(1);
    let mut all = lit_lens[..hlit].to_vec();
    all.extend_from_slice(&dist_lens[..hdist]);
    let cl_syms = run_length(&all);
    let mut cl_freq = [0u32; 19];
    for &(s, _, _) in &cl_syms {
        cl_freq[s as usize] += 1;
    }
    let cl_lens = code_lengths(&cl_freq, 7);
    let hclen = (4..=19).rev().find(|&n| cl_lens[CL_ORDER[n - 1]] != 0).unwrap_or(4);
    Plan { lit_lens, dist_lens, cl_lens, cl_syms, hlit, hdist, hclen }
}

// lets write_block price all three block forms before writing one
fn symbol_bits(syms: &[u32], lit_lens: &[u8], dist_lens: &[u8]) -> u64 {
    let mut bits = lit_lens[EOB] as u64;
    for &s in syms {
        if s & MATCH_FLAG == 0 {
            bits += lit_lens[s as usize] as u64;
        } else {
            let len = ((s >> 16) & 0x1ff) as usize;
            let dist = (s & 0xffff) as usize;
            let lc = len_code(len);
            let dc = dist_code(dist);
            bits += lit_lens[257 + lc] as u64 + LEN_EXTRA[lc] as u64 + dist_lens[dc] as u64 + DIST_EXTRA[dc] as u64;
        }
    }
    bits
}

fn write_symbols(w: &mut BitWriter, syms: &[u32], lit_lens: &[u8], dist_lens: &[u8]) {
    let lit_codes = canonical(lit_lens);
    let dist_codes = canonical(dist_lens);
    for &s in syms {
        if s & MATCH_FLAG == 0 {
            w.code(lit_codes[s as usize], lit_lens[s as usize]);
        } else {
            let len = ((s >> 16) & 0x1ff) as usize;
            let dist = (s & 0xffff) as usize;
            let lc = len_code(len);
            let dc = dist_code(dist);
            w.code(lit_codes[257 + lc], lit_lens[257 + lc]);
            w.put((len - LEN_BASE[lc] as usize) as u32, LEN_EXTRA[lc] as u32);
            w.code(dist_codes[dc], dist_lens[dc]);
            w.put((dist - DIST_BASE[dc] as usize) as u32, DIST_EXTRA[dc] as u32);
        }
    }
    w.code(lit_codes[EOB], lit_lens[EOB]);
}

// noise grows under huffman, so stored is tried against fixed and dynamic
fn write_block(w: &mut BitWriter, syms: &[u32], raw: &[u8], last: bool) {
    let mut lit_freq = vec![0u32; 286];
    let mut dist_freq = vec![0u32; 30];
    lit_freq[EOB] = 1;
    for &s in syms {
        if s & MATCH_FLAG == 0 {
            lit_freq[s as usize] += 1;
        } else {
            lit_freq[257 + len_code(((s >> 16) & 0x1ff) as usize)] += 1;
            dist_freq[dist_code((s & 0xffff) as usize)] += 1;
        }
    }
    let plan = plan_dynamic(&lit_freq, &dist_freq);
    let mut dynamic = 3 + 5 + 5 + 4 + 3 * plan.hclen as u64;
    for &(sym, _, extra) in &plan.cl_syms {
        dynamic += plan.cl_lens[sym as usize] as u64 + extra as u64;
    }
    dynamic += symbol_bits(syms, &plan.lit_lens, &plan.dist_lens);
    let fixed_lit = fixed_lit_lens();
    let fixed_dist = vec![5u8; 30];
    let fixed = 3 + symbol_bits(syms, &fixed_lit, &fixed_dist);
    let blocks = raw.len().div_ceil(MAX_STORED).max(1) as u64;
    let stored = blocks * (3 + 7 + 32) + raw.len() as u64 * 8;

    if stored < dynamic.min(fixed) {
        let mut chunks = raw.chunks(MAX_STORED).peekable();
        if raw.is_empty() {
            w.put(last as u32, 1);
            w.put(0, 2);
            w.align();
            w.out.extend_from_slice(&[0, 0, 0xff, 0xff]);
            return;
        }
        while let Some(c) = chunks.next() {
            w.put((last && chunks.peek().is_none()) as u32, 1);
            w.put(0, 2);
            w.align();
            w.out.extend_from_slice(&(c.len() as u16).to_le_bytes());
            w.out.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
            w.out.extend_from_slice(c);
        }
    } else if fixed <= dynamic {
        w.put(last as u32, 1);
        w.put(1, 2);
        write_symbols(w, syms, &fixed_lit, &fixed_dist);
    } else {
        w.put(last as u32, 1);
        w.put(2, 2);
        w.put((plan.hlit - 257) as u32, 5);
        w.put((plan.hdist - 1) as u32, 5);
        w.put((plan.hclen - 4) as u32, 4);
        for &o in &CL_ORDER[..plan.hclen] {
            w.put(plan.cl_lens[o] as u32, 3);
        }
        let cl_codes = canonical(&plan.cl_lens);
        for &(sym, extra, bits) in &plan.cl_syms {
            w.code(cl_codes[sym as usize], plan.cl_lens[sym as usize]);
            w.put(extra as u32, bits as u32);
        }
        write_symbols(w, syms, &plan.lit_lens, &plan.dist_lens);
    }
}

struct Matcher<'a> {
    data: &'a [u8],
    head: Vec<u32>,
    prev: Vec<u32>,
}

impl<'a> Matcher<'a> {
    fn new(data: &'a [u8]) -> Self {
        Matcher { data, head: vec![NONE; HASH_SIZE], prev: vec![NONE; WINDOW] }
    }

    fn hash(&self, i: usize) -> usize {
        let d = self.data;
        let v = u32::from(d[i]) | u32::from(d[i + 1]) << 8 | u32::from(d[i + 2]) << 16;
        (v.wrapping_mul(0x1e35_a7bd) >> (32 - HASH_BITS)) as usize
    }

    fn insert(&mut self, i: usize) {
        if i + MIN_MATCH <= self.data.len() {
            let h = self.hash(i);
            self.prev[i & (WINDOW - 1)] = self.head[h];
            self.head[h] = i as u32;
        }
    }

    // has to beat prev_len or lazy matching would trade a long match for a short one
    fn longest(&self, i: usize, prev_len: usize) -> (usize, usize) {
        let d = self.data;
        let max_len = MAX_MATCH.min(d.len() - i);
        if max_len < MIN_MATCH {
            return (0, 0);
        }
        let mut chain = if prev_len >= GOOD_MATCH { MAX_CHAIN / 4 } else { MAX_CHAIN };
        let mut best = (prev_len.max(MIN_MATCH - 1), 0);
        let mut cand = self.head[self.hash(i)];
        while cand != NONE && chain > 0 {
            let c = cand as usize;
            if c >= i || i - c > WINDOW {
                break;
            }
            // cheap reject before the full compare
            if d[c + best.0.min(max_len - 1)] == d[i + best.0.min(max_len - 1)] {
                let mut l = 0;
                while l < max_len && d[c + l] == d[i + l] {
                    l += 1;
                }
                if l > best.0 {
                    best = (l, i - c);
                    if l >= NICE_MATCH.min(max_len) {
                        break;
                    }
                }
            }
            let next = self.prev[c & (WINDOW - 1)];
            if next != NONE && next as usize >= c {
                break;
            }
            cand = next;
            chain -= 1;
        }
        if best.1 == 0 {
            (0, 0)
        } else {
            best
        }
    }
}

pub fn compress(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() > MAX_INPUT {
        return Err("deflate input is over 4 GiB".to_string());
    }
    let mut w = BitWriter { out: Vec::with_capacity(data.len() / 2 + 64), acc: 0, n: 0 };
    let mut m = Matcher::new(data);
    let mut syms: Vec<u32> = Vec::with_capacity(BLOCK_SYMBOLS + 1);
    let mut block_start = 0usize;
    let n = data.len();
    let mut i = 0usize;
    // lazy matching: the next byte may start a longer match
    let mut pending: Option<(usize, usize)> = None;
    let mut literal_waiting = false;

    macro_rules! flush_if_full {
        ($upto:expr) => {
            if syms.len() >= BLOCK_SYMBOLS {
                write_block(&mut w, &syms, &data[block_start..$upto], false);
                block_start = $upto;
                syms.clear();
            }
        };
    }

    while i < n {
        let prev = pending.unwrap_or((0, 0));
        // searched before i enters its own chain, or the first candidate would be i itself
        let mut cur = if prev.0 < NICE_MATCH { m.longest(i, prev.0) } else { (0, 0) };
        m.insert(i);
        if cur.0 == MIN_MATCH && cur.1 > TOO_FAR {
            cur = (0, 0);
        }
        if prev.0 >= MIN_MATCH && cur.0 <= prev.0 {
            // skipped positions still enter the chains or later matches are missed
            syms.push(match_sym(prev.0, prev.1));
            let end = i - 1 + prev.0;
            for p in i + 1..end {
                m.insert(p);
            }
            i = end;
            pending = None;
            literal_waiting = false;
            flush_if_full!(i);
        } else if literal_waiting {
            syms.push(data[i - 1] as u32);
            pending = if cur.0 >= MIN_MATCH { Some(cur) } else { None };
            i += 1;
            flush_if_full!(i - 1);
        } else {
            pending = if cur.0 >= MIN_MATCH { Some(cur) } else { None };
            literal_waiting = true;
            i += 1;
        }
    }
    if literal_waiting {
        syms.push(data[n - 1] as u32);
    }
    write_block(&mut w, &syms, &data[block_start..], true);
    w.align();
    Ok(w.out)
}

// os byte fixed at 3 so the bytes do not depend on the host
pub fn gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let body = compress(data)?;
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];
    out.extend_from_slice(&body);
    out.extend_from_slice(&crate::zip::crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: u32, n: usize) -> Vec<u8> {
        let mut x = seed;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect()
    }

    fn round(data: &[u8]) -> usize {
        let packed = compress(data).unwrap();
        let back = crate::png::inflate(&packed, data.len() + 16).unwrap();
        assert_eq!(back.len(), data.len());
        assert!(back == data, "round trip differs for {} bytes", data.len());
        packed.len()
    }

    #[test]
    fn every_length_around_the_block_edges_round_trips() {
        for n in 0..600 {
            round(&rng(n as u32, n));
            round(&vec![b'a'; n]);
        }
        round(&rng(7, MAX_STORED + 5));
        round(&rng(8, MAX_STORED * 2 + 1));
    }

    #[test]
    fn shapes_of_input_pick_each_block_kind_and_round_trip() {
        let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog. ".iter().cycle().take(200_000).cloned().collect();
        let mut mixed = rng(3, 50_000);
        mixed.extend_from_slice(&text[..50_000]);
        mixed.extend(rng(4, 50_000));
        let zeros = vec![0u8; 5_000_000];
        let steps: Vec<u8> = (0..300_000u32).map(|i| (i / 7) as u8).collect();
        let small_alphabet: Vec<u8> = rng(5, 400_000).iter().map(|b| b'a' + b % 3).collect();
        assert!(round(&text) < text.len() / 50);
        assert!(round(&zeros) < zeros.len() / 500);
        // noise cannot shrink, only the stored headers may add
        let noise = rng(6, 300_000);
        assert!(round(&noise) <= noise.len() + noise.len() / 1000 + 16);
        round(&mixed);
        round(&steps);
        round(&small_alphabet);
    }

    #[test]
    fn matches_reach_the_far_end_of_the_window() {
        let chunk = rng(9, 1000);
        let mut data = chunk.clone();
        data.extend(rng(10, WINDOW - 1000));
        data.extend_from_slice(&chunk);
        data.extend(rng(11, 100));
        data.extend_from_slice(&chunk[..300]);
        round(&data);
        // one byte repeated gives length 258 matches at distance 1
        round(&vec![7u8; MAX_MATCH * 5 + 3]);
    }

    #[test]
    fn the_host_gzip_reads_what_gzip_writes() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let data: Vec<u8> = b"packing ".iter().cycle().take(100_000).chain(rng(12, 30_000).iter()).cloned().collect();
        let file = gzip(&data).unwrap();
        let Ok(mut child) = Command::new("gzip").arg("-dc").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else {
            return;
        };
        let mut stdin = child.stdin.take().unwrap();
        let writer = std::thread::spawn(move || stdin.write_all(&file));
        let out = child.wait_with_output().unwrap();
        writer.join().unwrap().unwrap();
        assert!(out.status.success());
        assert!(out.stdout == data);
    }

    #[test]
    fn equal_input_gives_equal_bytes() {
        let data: Vec<u8> = rng(13, 5000).into_iter().chain(b"xyz".iter().cycle().take(9000).cloned()).collect();
        assert_eq!(compress(&data).unwrap(), compress(&data).unwrap());
    }

    #[test]
    fn code_lengths_respect_the_limit() {
        // fibonacci weights build the deepest tree
        let mut f = vec![1u32, 1];
        for i in 2..40 {
            let next = f[i - 1].saturating_add(f[i - 2]);
            f.push(next);
        }
        let lens = code_lengths(&f, 15);
        assert!(lens.iter().all(|&l| (1..=15).contains(&l)));
        let kraft: f64 = lens.iter().map(|&l| 0.5f64.powi(l as i32)).sum();
        assert!(kraft <= 1.0 + 1e-9);
    }
}
