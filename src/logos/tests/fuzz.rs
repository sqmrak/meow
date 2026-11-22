// the core must return an error and never panic, whatever the text is

use crate::logos::{process, Config};
use std::fs;
use std::panic::catch_unwind;
use std::path::PathBuf;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const PIECES: [&str; 28] = [
    "%hook ", "%end", "%orig", "%orig(", "%log", "%init", "%init(", "%group ", "%new", "%new(", "%ctor", "%c(",
    "%property", "%subclass ", "%hookf(", "- (", "+ (void)", "&%orig", "{", "}", "(", ")", "\"", "'", "/*",
    "//", "\u{e9}\u{20ac}", "@interface ",
];

fn mutate(rng: &mut Rng, src: &str) -> String {
    let mut chars: Vec<char> = src.chars().collect();
    for _ in 0..1 + rng.below(6) {
        if chars.is_empty() {
            break;
        }
        let at = rng.below(chars.len());
        match rng.below(4) {
            0 => {
                chars.remove(at);
            }
            1 => {
                let piece: Vec<char> = PIECES[rng.below(PIECES.len())].chars().collect();
                for (i, c) in piece.into_iter().enumerate() {
                    chars.insert(at + i, c);
                }
            }
            2 => chars.truncate(at),
            _ => {
                let to = (at + rng.below(30)).min(chars.len());
                chars.drain(at..to);
            }
        }
    }
    chars.into_iter().collect()
}

#[test]
fn mutated_sources_never_panic() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/logos/corpus");
    let mut sources: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map_or(false, |e| e == "x"))
        .map(|p| fs::read_to_string(p).unwrap())
        .collect();
    sources.sort();

    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut panics = Vec::new();
    let rounds: usize = std::env::var("LOGOS_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for round in 0..rounds {
        for (i, src) in sources.iter().enumerate() {
            let text = mutate(&mut rng, src);
            let generator = ["MobileSubstrate", "libhooker", "internal"][(round + i) % 3];
            let config = Config { generator: generator.to_string(), warnings: "default".to_string() };
            let t = text.clone();
            if catch_unwind(move || {
                let _ = process(&t, "fuzz.x", &config);
            })
            .is_err()
            {
                panics.push(text);
            }
        }
    }
    assert!(panics.is_empty(), "{} inputs panicked, first:\n{}", panics.len(), panics[0]);
}
