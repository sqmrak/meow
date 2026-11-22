// string helpers shared by the reader, the parser and the generators
// offsets are byte offsets, every special character is ascii

pub fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

pub fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

pub fn skip_space(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_space(b[i]) {
        i += 1;
    }
    i
}

pub fn trim_ws(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii() && is_space(c as u8))
}

pub fn trim_start_ws(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8))
}

pub fn trim_end_ws(s: &str) -> &str {
    s.trim_end_matches(|c: char| c.is_ascii() && is_space(c as u8))
}

/// spans of quoted text: a quote not preceded by a backslash up to the next
/// quote of the same kind not preceded by a backslash
pub fn quotes(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if (c == b'"' || c == b'\'') && (i == 0 || b[i - 1] != b'\\') {
            let mut j = i + 1;
            let mut end = None;
            while j < b.len() {
                if b[j] == c && b[j - 1] != b'\\' {
                    end = Some(j + 1);
                    break;
                }
                j += 1;
            }
            if let Some(e) = end {
                out.push((i, e));
                i = e;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// strictly inside a span, the quote characters themselves are outside
pub fn falls_between(idx: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(s, e)| s < idx && e > idx)
}

/// offsets just after each outermost parenthesis pair, `(open, close)` where
/// the text between the parentheses is `input[open..close - 1]`
/// an empty result means no pair was found
pub fn matched_parenthesis_set(
    input: &str,
    at_start: bool,
    until_end: bool,
) -> Result<Vec<(usize, usize)>, String> {
    let b = input.as_bytes();
    let mut flat: Vec<usize> = Vec::new();
    let starts_with_paren = {
        let i = skip_space(b, 0);
        i < b.len() && b[i] == b'('
    };
    if !at_start || starts_with_paren {
        let pq = quotes(input);
        let mut depth: i64 = 0;
        for (i, &c) in b.iter().enumerate() {
            if !matches!(c, b';' | b'(' | b')') || falls_between(i, &pq) {
                continue;
            }
            if c == b'(' {
                if depth == 0 {
                    flat.push(i + 1);
                }
                depth += 1;
            } else if c == b')' {
                depth -= 1;
                if depth == 0 {
                    flat.push(i + 1);
                    if !until_end {
                        break;
                    }
                }
            }
        }
    }
    if flat.len() % 2 == 1 {
        return Err("missing closing parenthesis".to_string());
    }
    Ok(flat.chunks(2).map(|c| (c[0], c[1])).collect())
}

/// split on `\s*<delim>\s*`, skipping delimiters inside quotes or parentheses
/// `limit` of 2 stops after the first split, 0 means no limit
pub fn smart_split(delim: u8, input: &str, limit: i32) -> Result<Vec<String>, String> {
    if input.is_empty() || input == "0" {
        return Ok(Vec::new());
    }
    let b = input.as_bytes();
    let q = quotes(input);
    let parens = matched_parenthesis_set(input, false, true)?;
    let mut limit = limit;
    let mut lstart = 0;
    let mut search = 0;
    let mut pieces = Vec::new();
    while let Some(off) = b[search..].iter().position(|&c| c == delim) {
        let d = search + off;
        let mut s = d;
        while s > search && is_space(b[s - 1]) {
            s -= 1;
        }
        let e = skip_space(b, d + 1);
        search = e;
        if falls_between(s, &parens) || falls_between(s, &q) {
            continue;
        }
        pieces.push(input[lstart..s].to_string());
        lstart = e;
        limit -= 1;
        if limit == 1 {
            break;
        }
    }
    pieces.push(input[lstart..].to_string());
    Ok(pieces)
}

/// the innermost parenthesis group reached by always descending into the first
/// group, as `(open, close)` offsets just after each parenthesis
pub fn deepest_paren_group(s: &str) -> Result<Option<(usize, usize)>, String> {
    let first = matched_parenthesis_set(s, false, false)?;
    let Some(&(o, c)) = first.first() else {
        return Ok(None);
    };
    let mut offset = o;
    let mut inner_end = c - 1;
    loop {
        let inner = &s[offset..inner_end];
        let next = matched_parenthesis_set(inner, false, false)?;
        match next.first() {
            None => return Ok(Some((offset, inner_end + 1))),
            Some(&(no, nc)) => {
                inner_end = offset + nc - 1;
                offset += no;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_pair_and_escape() {
        assert_eq!(quotes(r#"a "b" 'c'"#), vec![(2, 5), (6, 9)]);
        assert_eq!(quotes(r#"x "a\"b" y"#), vec![(2, 8)]);
        assert!(quotes("it's").is_empty());
    }

    #[test]
    fn falls_between_is_strict() {
        let q = [(2, 5)];
        assert!(!falls_between(2, &q));
        assert!(falls_between(3, &q));
        assert!(!falls_between(5, &q));
    }

    #[test]
    fn parens() {
        assert_eq!(matched_parenthesis_set("(a(b)c) x", true, false).unwrap(), vec![(1, 7)]);
        assert!(matched_parenthesis_set("x (a)", true, false).unwrap().is_empty());
        assert!(matched_parenthesis_set("(a", true, false).is_err());
        assert_eq!(
            matched_parenthesis_set("(a) (b)", false, true).unwrap(),
            vec![(1, 3), (5, 7)]
        );
        assert_eq!(matched_parenthesis_set("(\")\")", true, false).unwrap(), vec![(1, 5)]);
    }

    #[test]
    fn split_respects_parens_and_quotes() {
        assert_eq!(smart_split(b',', "a, b(c, d), \"e,f\", g", 0).unwrap(), vec!["a", "b(c, d)", "\"e,f\"", "g"]);
        assert_eq!(smart_split(b'=', "a = b = c", 2).unwrap(), vec!["a", "b = c"]);
        assert!(smart_split(b',', "", 0).unwrap().is_empty());
        assert_eq!(smart_split(b',', "a,", 0).unwrap(), vec!["a", ""]);
    }

    #[test]
    fn deepest_group() {
        assert_eq!(deepest_paren_group("void (^)(int)").unwrap(), Some((6, 8)));
        assert_eq!(deepest_paren_group("int").unwrap(), None);
    }
}
