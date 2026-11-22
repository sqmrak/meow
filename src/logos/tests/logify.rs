// output of logify.pl on tests/logos/logify/sample.h, rebuilt with tests/logos/regen-golden.sh

use crate::logos::logify::Logify;
use std::fs;
use std::path::PathBuf;

#[test]
fn matches_perl_logify() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/logos/logify");
    let header = fs::read_to_string(dir.join("sample.h")).unwrap();
    let golden = fs::read_to_string(dir.join("sample.golden")).unwrap();

    let mut runs: Vec<(String, String)> = Vec::new();
    for line in golden.lines() {
        if let Some(head) = line.strip_prefix("@@ ") {
            runs.push((head.to_string(), String::new()));
        } else if let Some(l) = line.strip_prefix("> ") {
            let r = runs.last_mut().unwrap();
            r.1.push_str(l);
            r.1.push('\n');
        } else if line == ">" {
            runs.last_mut().unwrap().1.push('\n');
        }
    }
    assert_eq!(runs.len(), 3);

    for (head, expected) in runs {
        let opts = head.split('|').next().unwrap().trim();
        let mut it = opts.split_whitespace();
        let (mut include, mut exclude) = (None, None);
        while let Some(flag) = it.next() {
            match flag {
                "-i" => include = it.next(),
                "-e" => exclude = it.next(),
                other => panic!("unexpected option {}", other),
            }
        }
        let mut logify = Logify::new(include, exclude).unwrap();
        let out = logify.feed(&header).unwrap();
        assert_eq!(out, expected, "options {:?}", opts);
    }
}
