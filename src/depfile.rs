// parses the make rule that clang -MMD -MF writes, returns the prerequisites
pub fn parse(text: &str) -> Vec<String> {
    let joined = text.replace("\\\r\n", " ").replace("\\\n", " ");
    let bytes = joined.as_bytes();
    // the target ends at the first colon that is followed by a blank or the end
    let mut start = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b':' if i + 1 >= bytes.len() || bytes[i + 1] == b' ' || bytes[i + 1] == b'\n' => {
                start = Some(i + 1);
                break;
            }
            _ => i += 1,
        }
    }
    let Some(start) = start else { return Vec::new() };
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = joined[start..].chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some(' ') | Some('#') | Some('\\') => cur.push(chars.next().unwrap()),
                _ => cur.push('\\'),
            },
            ' ' | '\t' | '\n' | '\r' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn continuation_and_escaped_space() {
        let t = "build/a.o: app/a.m app/a.h \\\n  app/with\\ space.h \\\n  core/b.h\n";
        assert_eq!(
            parse(t),
            vec!["app/a.m", "app/a.h", "app/with space.h", "core/b.h"]
        );
    }

    #[test]
    fn empty_and_malformed() {
        assert!(parse("").is_empty());
        assert!(parse("no rule here").is_empty());
        assert!(parse("a.o:").is_empty());
    }
}
