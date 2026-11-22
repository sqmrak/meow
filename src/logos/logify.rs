//! turns the methods and properties of an objective-c `@interface` into a logos
//! file that logs every call, the job of `logify.pl`

use crate::logos::emit::ctype::{format_char, print_arg};
use crate::logos::emit::generator::declaration_for_type_with_name;
use crate::logos::text::{is_space, is_word, matched_parenthesis_set, skip_space, smart_split, trim_end_ws};

pub struct Logify {
    include: Option<String>,
    exclude: Option<String>,
    interface: bool,
    logged: Vec<String>,
}

fn ident_start(b: u8) -> bool {
    b == b'$' || b == b'_' || b.is_ascii_alphabetic()
}

/// `[+-]\s*\((.*?)\).*?(?=;)`, the method declaration up to its semicolon
fn method_declaration(line: &str) -> Option<&str> {
    let b = line.as_bytes();
    if !matches!(b.first(), Some(b'+') | Some(b'-')) {
        return None;
    }
    let open = skip_space(b, 1);
    if open >= b.len() || b[open] != b'(' {
        return None;
    }
    for close in open + 1..b.len() {
        if b[close] == b')' {
            if let Some(semi) = line[close + 1..].find(';') {
                return Some(&line[..close + 1 + semi]);
            }
        }
    }
    None
}

struct PropertyLine {
    attributes: String,
    type_name: String,
    name: String,
}

/// `^\s*@property\s*\((.*?)\)\s*(.*?)\b([\$a-zA-Z_][\$_a-zA-Z0-9]*)(?=;)`
fn property_line(line: &str) -> Option<PropertyLine> {
    let b = line.as_bytes();
    let i = skip_space(b, 0);
    let rest = line[i..].strip_prefix("@property")?;
    let base = line.len() - rest.len();
    let open = skip_space(b, base);
    if open >= b.len() || b[open] != b'(' {
        return None;
    }
    for close in open + 1..b.len() {
        if b[close] != b')' {
            continue;
        }
        let start = skip_space(b, close + 1);
        for t in start..b.len() {
            if !ident_start(b[t]) {
                continue;
            }
            let prev_word = t > 0 && is_word(b[t - 1]);
            if prev_word == is_word(b[t]) {
                continue;
            }
            let mut e = t;
            while e < b.len() && (b[e] == b'$' || is_word(b[e])) {
                e += 1;
            }
            if e < b.len() && b[e] == b';' {
                return Some(PropertyLine {
                    attributes: line[open + 1..close].to_string(),
                    type_name: trim_end_ws(&line[start..t]).to_string(),
                    name: line[t..e].to_string(),
                });
            }
        }
    }
    None
}

/// `@interface Name :` or `@interface Name (`, returns the name
fn interface_name(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("@interface")?;
    let b = rest.as_bytes();
    let s = skip_space(b, 0);
    if s == 0 {
        return None;
    }
    let sep = rest[s..].find(|c| c == ':' || c == '(')? + s;
    Some(crate::logos::text::trim_end_ws(&rest[s..sep]))
}

fn ucfirst(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

impl Logify {
    pub fn new(include: Option<&str>, exclude: Option<&str>) -> Result<Logify, String> {
        if include.is_some() && exclude.is_some() {
            return Err("--include and --exclude are mutually exclusive".to_string());
        }
        Ok(Logify {
            include: include.map(|s| s.to_string()),
            exclude: exclude.map(|s| s.to_string()),
            interface: false,
            logged: Vec::new(),
        })
    }

    fn is_logged(&mut self, declaration: &str) -> bool {
        let key: String = declaration.chars().filter(|c| !(c.is_ascii() && is_space(*c as u8))).collect();
        if self.logged.contains(&key) {
            true
        } else {
            self.logged.push(key);
            false
        }
    }

    fn should_log(&self, declaration: &str) -> bool {
        // anything inside parentheses goes first, then everything but letters, digits, colons and spaces
        let mut s = String::new();
        let b = declaration.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'(' {
                if let Some(off) = b[i + 1..].iter().position(|&c| c == b'(' || c == b')') {
                    if b[i + 1 + off] == b')' {
                        i += off + 2;
                        continue;
                    }
                }
            }
            s.push(b[i] as char);
            i += 1;
        }
        let mut s: String = s
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == ':' || (c.is_ascii() && is_space(*c as u8)))
            .collect();

        let opt = self.exclude.as_deref().or(self.include.as_deref()).unwrap_or("");
        let mut filters: Vec<&str> = if opt.contains(',') { opt.split(',').collect() } else { vec![opt] };
        if opt.contains(',') {
            while filters.last().map_or(false, |f| f.is_empty()) {
                filters.pop();
            }
        }

        if filters.iter().any(|f| f.contains(':')) && s.contains(':') {
            let spaced = s.replace(':', ": ");
            s = spaced.split_whitespace().filter(|w| w.contains(':')).collect();
        }
        let s: String = s.chars().filter(|c| !(c.is_ascii() && is_space(*c as u8))).collect();

        for f in &filters {
            if *f == s {
                return self.exclude.is_none();
            }
        }
        self.exclude.is_some()
    }

    fn log_line(&self, declaration: &str) -> String {
        if (self.include.is_some() || self.exclude.is_some()) && !self.should_log(declaration) {
            return String::new();
        }
        // the return type ends at the parenthesis that closes its own, block types contain more
        let b = declaration.as_bytes();
        let open = skip_space(b, 1);
        let mut rtype = match matched_parenthesis_set(&declaration[open..], true, false) {
            Ok(pairs) if !pairs.is_empty() => declaration[open + pairs[0].0..open + pairs[0].1 - 1].to_string(),
            _ => {
                let close = declaration[open..].find(')').map_or(declaration.len(), |c| open + c);
                declaration[(open + 1).min(close)..close].to_string()
            }
        };

        let mut innards = String::from("%log; ");
        if rtype != "void" {
            if rtype == "instancetype" {
                rtype = "id".to_string();
            }
            let variable = declaration_for_type_with_name(&rtype, "r").unwrap_or_else(|_| format!("{} r", rtype));
            innards.push_str(&format!("{} = %orig; ", variable));
            if let Some(arg) = print_arg(&rtype, "r") {
                innards.push_str(&format!("NSLog(@\" = {}\", {}); ", format_char(&rtype).0, arg));
            }
            innards.push_str("return r; ");
        } else {
            innards.push_str("%orig; ");
        }
        format!("{} {{ {}}}\n", declaration, innards)
    }

    /// processes the lines of one file, state carries over to the next file
    pub fn feed(&mut self, text: &str) -> Result<String, (usize, String)> {
        let mut out = String::new();
        for (n, raw) in text.split_inclusive('\n').enumerate() {
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            if self.interface {
                if let Some(decl) = method_declaration(line) {
                    if !self.is_logged(decl) {
                        out.push_str(&self.log_line(decl));
                    }
                    continue;
                }
                if let Some(p) = property_line(line) {
                    let attributes = smart_split(b',', &p.attributes, 0).map_err(|e| (n + 1, e))?;
                    let readonly = attributes.iter().any(|a| a.contains("readonly"));
                    let mut setter = format!("set{}:", ucfirst(&p.name));
                    let mut getter = p.name.clone();
                    for a in &attributes {
                        if !a.contains('=') {
                            continue;
                        }
                        let x = smart_split(b'=', a, 0).map_err(|e| (n + 1, e))?;
                        if x.len() >= 2 {
                            match x[0].as_str() {
                                "setter" => setter = x[1].clone(),
                                "getter" => getter = x[1].clone(),
                                _ => {}
                            }
                        }
                    }
                    if !readonly {
                        let decl = format!("- (void){}({}){}", setter, p.type_name, p.name);
                        if !self.is_logged(&decl) {
                            out.push_str(&self.log_line(&decl));
                        }
                    }
                    let decl = format!("- ({}){}", p.type_name, getter);
                    if !self.is_logged(&decl) {
                        out.push_str(&self.log_line(&decl));
                    }
                    continue;
                }
            }
            if !self.interface {
                if let Some(name) = interface_name(line) {
                    out.push_str(&format!("%hook {}\n", name));
                    self.interface = true;
                }
            } else if line.starts_with("@end") {
                out.push_str("%end\n");
                self.interface = false;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations() {
        assert_eq!(method_declaration("- (void)foo:(int)a;"), Some("- (void)foo:(int)a"));
        assert_eq!(method_declaration("- (void)foo"), None);
        assert_eq!(method_declaration("foo;"), None);
    }

    #[test]
    fn block_return_type_names_the_variable_inside_the_block_type() {
        let mut l = Logify::new(None, None).unwrap();
        let out = l.feed("@interface A : B\n- (void (^)(int))handler;\n@end\n").unwrap();
        assert!(
            out.contains("- (void (^)(int))handler { %log; void (^r)(int) = %orig; NSLog(@\" = %@\", r); return r; }"),
            "{}",
            out
        );
    }

    #[test]
    fn properties() {
        let p = property_line("@property (nonatomic, copy) NSString *name;").unwrap();
        assert_eq!((p.attributes.as_str(), p.type_name.as_str(), p.name.as_str()), ("nonatomic, copy", "NSString *", "name"));
        let p = property_line("@property (readonly) int x;").unwrap();
        assert_eq!((p.type_name.as_str(), p.name.as_str()), ("int", "x"));
    }
}
