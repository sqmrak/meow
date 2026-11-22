// c type names to log format characters, argument expressions and objc type encodings

use crate::logos::text::{is_space, is_word, trim_ws};

fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn strip_ws_before_star(t: &str, base: &str) -> bool {
    // `<base>\s*\*` with nothing after the star
    match t.strip_prefix(base) {
        Some(rest) => {
            let rest = rest.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8));
            rest == "*"
        }
        None => false,
    }
}

/// word at the start of `t`, as `\w+`
fn word_prefix(t: &str) -> &str {
    let end = t.bytes().position(|b| !is_word(b)).unwrap_or(t.len());
    &t[..end]
}

fn is_ns_integer_like(t: &str) -> bool {
    let Some(rest) = t.get(..2).filter(|p| eq_ci(p, "NS")).map(|_| &t[2..]) else {
        return false;
    };
    const NAMES: [&str; 8] = [
        "Integer",
        "SocketNativeHandle",
        "StringEncoding",
        "SortOptions",
        "ComparisonResult",
        "EnumerationOptions",
        "HashTableOptions",
        "MapTableOptions",
    ];
    if NAMES.iter().any(|n| eq_ci(rest, n)) {
        return true;
    }
    if let Some(tail) = rest.get(..10).filter(|p| eq_ci(p, "SearchPath")).map(|_| &rest[10..]) {
        return eq_ci(tail, "Directory") || eq_ci(tail, "DomainMask");
    }
    false
}

/// `((signed|unsigned)\s+)?(unsigned|signed|int|long|long\s+long|bool|BOOL|_Bool|char|short<extra>)`
fn integral_with(t: &str, allow_float: bool) -> bool {
    let mut rest = t;
    for prefix in ["signed", "unsigned"] {
        if let Some(r) = rest.strip_prefix(prefix) {
            let r2 = r.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8));
            if r2.len() < r.len() {
                rest = r2;
                break;
            }
        }
    }
    let base_ok = |s: &str| -> bool {
        let mut names = vec!["unsigned", "signed", "int", "long", "bool", "BOOL", "_Bool", "char", "short"];
        if allow_float {
            names.extend(["float", "double"]);
        }
        if names.contains(&s) {
            return true;
        }
        if let Some(r) = s.strip_prefix("long") {
            let r2 = r.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8));
            return r2.len() < r.len() && r2 == "long";
        }
        false
    };
    base_ok(rest)
}

fn has_word(t: &str, w: &str) -> bool {
    let b = t.as_bytes();
    let mut from = 0;
    while let Some(off) = t[from..].find(w) {
        let i = from + off;
        let before = i == 0 || !is_word(b[i - 1]);
        let after = i + w.len() >= b.len() || !is_word(b[i + w.len()]);
        if before && after {
            return true;
        }
        from = i + 1;
    }
    false
}

/// the printf conversion for a c type, `--` for types that are not logged and
/// a flag for types printed as a raw integer
pub fn format_char(argtype: &str) -> (String, bool) {
    let t = trim_ws(argtype);

    if t == "char" {
        return ("'%c'".to_string(), false);
    }
    if integral_with(t, false) {
        let conversion = if has_word(t, "unsigned") { "u" } else { "d" };
        let mut length = "";
        if has_word(t, "int") {
            length = "";
        }
        if has_word(t, "long") {
            length = "l";
        }
        if t.contains("long long") && has_word(t, "long") {
            length = "ll";
        }
        if has_word(t, "short") {
            length = "h";
        }
        if has_word(t, "char") {
            length = "hh";
        }
        return (format!("%{}{}", length, conversion), false);
    }
    if is_ns_integer_like(t) {
        return ("%ld".to_string(), false);
    }
    if eq_ci(t, "NSUInteger") {
        return ("%lu".to_string(), false);
    }
    if eq_ci(t, "GSFontTraitMask") {
        return ("%d".to_string(), false);
    }

    // pointer types
    if strip_ws_before_star(t, "char") {
        return ("%s".to_string(), false);
    }
    if strip_ws_before_star(t, "void") || strip_ws_before_star(t, "id") {
        return ("%p".to_string(), false);
    }
    if let Some(star) = t.find('*') {
        let (head, stars) = t.split_at(star);
        let head = head.trim_end_matches(|c: char| c.is_ascii() && is_space(c as u8));
        if stars.bytes().all(|b| b == b'*') && integral_with(head, true) {
            return ("%p".to_string(), false);
        }
    }
    if t.starts_with("NS") && (t.ends_with("Pointer") || t.ends_with("Array")) && t.len() >= 2 + 5 {
        return ("%p".to_string(), false);
    }
    if strip_ws_before_star(t, "NSZone") {
        return ("%p".to_string(), false);
    }
    if t.starts_with("struct") && t.ends_with('*') {
        return ("%p".to_string(), false);
    }
    if t.ends_with("**") {
        return ("%p".to_string(), false);
    }
    if let Some(open) = t.find('[') {
        if t[open..].ends_with(']') {
            return ("%p".to_string(), false);
        }
    }

    // objects
    if t == "instancetype" || t == "id" {
        return ("%@".to_string(), false);
    }
    if t.contains("(^)") {
        return ("%@".to_string(), false);
    }
    {
        let w = word_prefix(t);
        if !w.is_empty() {
            let rest = &t[w.len()..];
            let rest = rest.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8));
            if rest == "*" {
                return ("%@".to_string(), false);
            }
        }
        if t.len() > 3 && t.ends_with("Ref") && t.bytes().all(is_word) {
            return ("%@".to_string(), false);
        }
    }

    // floating point
    if matches!(t, "double" | "float" | "CGFloat" | "CGDouble" | "NSTimeInterval") {
        return ("%f".to_string(), false);
    }

    // special types
    if t == "SEL" || t == "Class" {
        return ("%@".to_string(), false);
    }
    if t == "NSRange" {
        return ("(%d:%d)".to_string(), false);
    }
    if t == "CGRect" || t == "NSRect" {
        return ("{{%g, %g}, {%g, %g}}".to_string(), false);
    }
    if matches!(t, "CGPoint" | "NSPoint" | "CGSize" | "NSSize") {
        return ("{%g, %g}".to_string(), false);
    }

    // types that are not logged
    if t == "void" || ((t.starts_with("CG") || t.starts_with("CF")) && t.bytes().all(is_word)) {
        return ("--".to_string(), false);
    }
    if matches!(
        t,
        "NSHashTable"
            | "NSHashTableCallbacks"
            | "NSMapTable"
            | "NSMapTableKeyCallbacks"
            | "NSMapTableValueCallbacks"
            | "NSMapEnumerator"
    ) {
        return ("--".to_string(), false);
    }
    if t.starts_with("struct") {
        return ("--".to_string(), false);
    }

    ("0x%llx".to_string(), true)
}

/// the expression to pass for a logged argument, none for types that are not logged
pub fn print_arg(argtype: &str, argname: &str) -> Option<String> {
    let (format, fallthrough) = format_char(argtype);
    if format == "--" {
        return None;
    }
    let t = trim_ws(argtype);
    if t == "SEL" {
        return Some(format!("NSStringFromSelector({})", argname));
    }
    if t == "Class" {
        return Some(argname.to_string());
    }
    if t == "NSRange" {
        return Some(format!("{0}.location, {0}.length", argname));
    }
    if t == "CGRect" || t == "NSRect" {
        return Some(format!(
            "{0}.origin.x, {0}.origin.y, {0}.size.width, {0}.size.height",
            argname
        ));
    }
    if matches!(t, "CGPoint" | "NSPoint") {
        return Some(format!("{0}.x, {0}.y", argname));
    }
    if matches!(t, "CGSize" | "NSSize") {
        return Some(format!("{0}.width, {0}.height", argname));
    }
    if is_ns_integer_like(t) {
        return Some(format!("(long){}", argname));
    }
    if eq_ci(t, "NSUInteger") {
        return Some(format!("(unsigned long){}", argname));
    }
    Some(format!("{}{}", if fallthrough { "(uint64_t)" } else { "" }, argname))
}

/// objc type encoding of a c type, none when the compiler has to compute it with @encode
pub fn type_encoding(argtype: &str) -> Option<String> {
    let t = trim_ws(argtype);
    let simple = match t {
        "char" => "c",
        "int" => "i",
        "short" => "s",
        "long" => "l",
        "long long" => "q",
        "float" => "f",
        "double" => "d",
        "bool" | "_Bool" => "B",
        "void" => "v",
        "id" | "instancetype" => "@",
        "Class" => "#",
        "SEL" => ":",
        _ => "",
    };
    if !simple.is_empty() {
        return Some(simple.to_string());
    }
    if let Some(rest) = t.strip_prefix("unsigned") {
        let r2 = rest.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8));
        if r2.len() < rest.len() {
            let enc = match r2 {
                "char" => Some("C"),
                "int" => Some("I"),
                "short" => Some("S"),
                "long" => Some("L"),
                "long long" => Some("Q"),
                _ => None,
            };
            if let Some(e) = enc {
                return Some(e.to_string());
            }
        }
    }
    if strip_ws_before_star(t, "char") {
        return Some("*".to_string());
    }
    if let Some(inner) = t.strip_suffix('*') {
        let inner = inner.trim_end_matches(|c: char| c.is_ascii() && is_space(c as u8));
        if !inner.is_empty() && !inner.contains('*') && !inner.bytes().any(|b| is_space(b)) {
            return type_encoding(inner).map(|e| format!("^{}", e));
        }
    }
    None
}
