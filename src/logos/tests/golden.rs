// output of the perl logos on tests/logos/corpus, rebuilt with tests/logos/regen-golden.sh
// perl walks hashes in random order, so lists that come from a hash are sorted before comparing

use crate::logos::{process, Config};
use std::fs;
use std::path::PathBuf;

fn normalize(text: &str) -> String {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@class ") {
            let mut items: Vec<&str> = rest.split("@class ").collect();
            items.sort();
            out.push(format!("@class {}", items.join("@class ")));
        } else if line.matches("static __inline__").count() > 1 {
            let mut items: Vec<&str> = line.split("static __inline__").collect();
            items.sort();
            out.push(items.join("static __inline__"));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

fn dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/logos").join(name)
}

struct Run {
    generator: String,
    warnings: String,
    status: i32,
    stdout: String,
    stderr: String,
}

fn parse_golden(text: &str) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("@@ ") {
            let mut it = head.split(' ');
            runs.push(Run {
                generator: it.next().unwrap().to_string(),
                warnings: it.next().unwrap().to_string(),
                status: it.next().unwrap().parse().unwrap(),
                stdout: String::new(),
                stderr: String::new(),
            });
        } else if let Some(l) = line.strip_prefix("> ") {
            let r = runs.last_mut().unwrap();
            r.stdout.push_str(l);
            r.stdout.push('\n');
        } else if let Some(l) = line.strip_prefix("! ") {
            let r = runs.last_mut().unwrap();
            r.stderr.push_str(l);
            r.stderr.push('\n');
        } else if line == ">" {
            runs.last_mut().unwrap().stdout.push('\n');
        } else if line == "!" {
            runs.last_mut().unwrap().stderr.push('\n');
        }
    }
    runs
}

#[test]
fn matches_perl_logos() {
    let mut names: Vec<String> = fs::read_dir(dir("corpus"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".x"))
        .collect();
    names.sort();
    assert!(!names.is_empty());

    let mut failures = Vec::new();
    let mut compared = 0;
    for file in &names {
        let stem = file.trim_end_matches(".x");
        let source = fs::read_to_string(dir("corpus").join(file)).unwrap();
        let golden = fs::read_to_string(dir("golden").join(format!("{}.golden", stem))).unwrap();
        for run in parse_golden(&golden) {
            compared += 1;
            let label = format!("{} [{} {}]", file, run.generator, run.warnings);
            let config = Config { generator: run.generator.clone(), warnings: run.warnings.clone() };
            match process(&source, file, &config) {
                Ok(out) => {
                    let diag = out.diagnostics.join("\n");
                    if run.status != 0 {
                        failures.push(format!("{}: perl failed with {:?}, got output", label, run.stderr));
                    } else if normalize(&out.text) != normalize(&run.stdout) {
                        failures.push(format!("{}: output differs", label));
                    } else if diag.trim() != run.stderr.trim() {
                        failures.push(format!("{}: diagnostics differ: {:?} vs {:?}", label, diag, run.stderr));
                    }
                }
                Err(f) => {
                    let mut text = f.diagnostics.join("\n");
                    if let Some(e) = f.fatal {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&e);
                    }
                    if run.status == 0 {
                        failures.push(format!("{}: expected output, got error {:?}", label, text));
                    } else if text.trim() != run.stderr.trim() {
                        failures.push(format!("{}: expected {:?}, got {:?}", label, run.stderr, text));
                    }
                }
            }
        }
    }
    assert!(compared >= names.len() * 3, "too few runs compared: {}", compared);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
