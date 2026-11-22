// first pass: strip comments and `#if 0` blocks and glue method signatures and
// %orig/%init calls that continue on the next line, one output line per input line

use crate::logos::text::{falls_between, quotes, skip_space, trim_start_ws};
use std::collections::BTreeMap;

pub struct Read {
    pub lines: Vec<String>,
    pub preprocessed: bool,
    pub filename: String,
    pub line_map: BTreeMap<usize, (String, usize)>,
}

/// `# 12 "file.x"` as left by a c preprocessor
fn marker(line: &str, anchored_end: bool) -> Option<(usize, String)> {
    let b = line.as_bytes();
    if b.len() < 2 || b[0] != b'#' || b[1] != b' ' {
        return None;
    }
    let mut i = 2;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == 2 || i + 1 >= b.len() || b[i] != b' ' || b[i + 1] != b'"' {
        return None;
    }
    let n: usize = line[2..i].parse().ok()?;
    let rest = &line[i + 2..];
    if anchored_end {
        let name = rest.strip_suffix('"')?;
        Some((n, name.to_string()))
    } else {
        let q = rest.find('"')?;
        Some((n, rest[..q].to_string()))
    }
}

/// `[+-]\s*\(` followed by a `)` somewhere later
fn sign_then_paren(s: &str, at: usize) -> bool {
    let b = s.as_bytes();
    if at >= b.len() || (b[at] != b'+' && b[at] != b'-') {
        return false;
    }
    let q = skip_space(b, at + 1);
    q < b.len() && b[q] == b'(' && s[q + 1..].contains(')')
}

/// a line that starts a method signature, `%new` prefix allowed
fn method_signature_start(line: &str) -> bool {
    let b = line.as_bytes();
    let i = skip_space(b, 0);
    if sign_then_paren(line, i) {
        return true;
    }
    if line[i..].starts_with("%new") {
        for k in i + 4..b.len() {
            if sign_then_paren(line, k) {
                return true;
            }
        }
    }
    false
}

/// leftmost occurrence of `needle` with no `;` after it
fn tail_directive(line: &str, needle: &str) -> Option<usize> {
    let last_semi = line.rfind(';');
    let mut from = 0;
    while let Some(off) = line[from..].find(needle) {
        let i = from + off;
        match last_semi {
            Some(s) if i + needle.len() <= s => from = i + 1,
            _ => return Some(i),
        }
    }
    None
}

fn is_if_zero(line: &str) -> bool {
    let b = line.as_bytes();
    let mut i = skip_space(b, 0);
    if i >= b.len() || b[i] != b'#' {
        return false;
    }
    i = skip_space(b, i + 1);
    if !line[i..].starts_with("if") {
        return false;
    }
    let j = skip_space(b, i + 2);
    j > i + 2 && j < b.len() && b[j] == b'0'
}

fn if_prefix(line: &str) -> bool {
    let b = line.as_bytes();
    if b.first() != Some(&b'#') {
        return false;
    }
    line[skip_space(b, 1)..].starts_with("if")
}

fn endif_end(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    if b.first() != Some(&b'#') {
        return None;
    }
    let i = skip_space(b, 1);
    line[i..].starts_with("endif").then_some(i + 5)
}

/// text after the first `*/` and the whitespace behind it
fn after_comment_end(line: &str) -> Option<&str> {
    let i = line.find("*/")?;
    Some(trim_start_ws(&line[i + 2..]))
}

pub fn read_lines(text: &str, filename: &str) -> Read {
    let raw: Vec<&str> = text
        .split_inclusive('\n')
        .map(|l| l.strip_suffix('\n').unwrap_or(l))
        .collect();

    let mut filename = filename.to_string();
    let mut preprocessed = false;
    if let Some(first) = raw.first() {
        if let Some((_, name)) = marker(first, true) {
            preprocessed = true;
            filename = name;
        }
    }

    let mut lines: Vec<String> = Vec::with_capacity(raw.len());
    let mut line_map: BTreeMap<usize, (String, usize)> = BTreeMap::new();
    let mut readignore = false;
    let mut building = false;
    let mut built = String::new();
    let mut iflevel: i32 = -1;

    for (idx, raw_line) in raw.iter().enumerate() {
        let physical = idx + 1;
        let mut line = raw_line.to_string();
        'redo: loop {
            if preprocessed {
                if let Some((n, name)) = marker(&line, false) {
                    line_map.insert(physical + 1, (name, n));
                }
            }

            if readignore {
                if iflevel > -1 && if_prefix(&line) {
                    iflevel += 1;
                } else if iflevel > 0 {
                    if let Some(end) = endif_end(&line) {
                        line = line[end..].to_string();
                        iflevel -= 1;
                    }
                }
                if iflevel == 0 {
                    readignore = false;
                    iflevel = -1;
                } else if iflevel == -1 {
                    if let Some(rest) = after_comment_end(&line) {
                        readignore = false;
                        line = rest.to_string();
                    }
                }
            }
            if readignore {
                lines.push(String::new());
                break 'redo;
            }

            let q = quotes(&line);

            // line comments, but a line that also closes a block comment is left to the block comment rules
            let mut search = 0;
            while let Some(off) = line[search..].find("//") {
                let i = search + off;
                if line.contains("*/") || falls_between(i, &q) {
                    search = i + 2;
                    continue;
                }
                line.truncate(i);
                continue 'redo;
            }

            // block comments that end on the same line
            let mut search = 0;
            while let Some(off) = line[search..].find("/*") {
                let i = search + off;
                let Some(j) = line[i + 2..].find("*/") else { break };
                let end = i + 2 + j + 2;
                if falls_between(i, &q) {
                    search = end;
                    continue;
                }
                line = format!("{}{}", &line[..i], &line[end..]);
                continue 'redo;
            }

            // a block comment that continues on the next lines
            if let Some(i) = line.find("/*") {
                if !falls_between(i, &q) {
                    line.truncate(i);
                    lines.push(line);
                    readignore = true;
                    break 'redo;
                }
            }

            if is_if_zero(&line) {
                iflevel = 1;
                lines.push(String::new());
                readignore = true;
                break 'redo;
            }

            if !building {
                let start = if method_signature_start(&line) {
                    Some(0)
                } else if let Some(s) = tail_directive(&line, "%orig") {
                    Some(s)
                } else {
                    tail_directive(&line, "%init")
                };
                if let Some(start) = start {
                    let brace_before = line.find('{').map_or(true, |p| p < start);
                    let semi_before = line.find(';').map_or(true, |p| p < start);
                    if brace_before && semi_before {
                        if falls_between(start, &q) {
                            lines.push(line);
                            break 'redo;
                        }
                        building = true;
                        built = line;
                        lines.push(String::new());
                        break 'redo;
                    }
                }
                lines.push(line);
                break 'redo;
            }

            let trimmed = trim_start_ws(&line);
            built.push(' ');
            built.push_str(trimmed);
            if trimmed.contains('{') || trimmed.contains(';') {
                lines.push(std::mem::take(&mut built));
                building = false;
                break 'redo;
            }
            lines.push(String::new());
            break 'redo;
        }
    }
    if building {
        lines.push(built);
    }

    if line_map.is_empty() {
        line_map.insert(0, (filename.clone(), 0));
    }
    Read { lines, preprocessed, filename, line_map }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(s: &str) -> Vec<String> {
        read_lines(s, "t.x").lines
    }

    #[test]
    fn strips_comments_and_keeps_strings() {
        assert_eq!(read("a // b\nc /* d */ e\nf \"//\" g"), vec!["a ", "c  e", "f \"//\" g"]);
        assert_eq!(read("a /* b\nc */ d\ne"), vec!["a ", "d", "e"]);
    }

    #[test]
    fn if_zero_blocks_become_blank() {
        assert_eq!(read("#if 0\nx\n#endif\ny"), vec!["", "", "", "y"]);
        assert_eq!(read("#if 0\n#if 1\nx\n#endif\n#endif\ny"), vec!["", "", "", "", "", "y"]);
    }

    #[test]
    fn glues_signature_lines() {
        assert_eq!(
            read("- (void)foo:(int)a\n bar:(int)b {\n}"),
            vec!["", "- (void)foo:(int)a bar:(int)b {", "}"]
        );
        assert_eq!(read("- (void)foo;\nx"), vec!["- (void)foo;", "x"]);
    }
}
