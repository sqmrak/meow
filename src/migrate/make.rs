// a reader for the part of make that theos makefiles use: assignments, conditionals,
// include, and the functions that build file lists. nothing is run, a $(shell) other than
// `find` is left as it is and reported

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 40;

#[derive(Clone)]
struct Var {
    // := is expanded when it is assigned, = when it is used
    immediate: bool,
    value: String,
}

pub struct Make {
    dir: PathBuf,
    vars: HashMap<String, Var>,
    // variables the user set on the command line win over the file
    fixed: HashMap<String, String>,
    pub notes: Vec<String>,
    pub rules: Vec<String>,
    // theos makefiles the file includes, `tweak.mk` and the like
    pub includes: Vec<String>,
    // names that were assigned, in order
    pub assigned: Vec<String>,
    expanding: Vec<String>,
}

fn split_words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

// the first `=` at depth 0 and the operator it belongs to
fn find_assign(line: &str) -> Option<(usize, &'static str, usize)> {
    let b = line.as_bytes();
    let mut depth = 0i32;
    for i in 0..b.len() {
        match b[i] {
            b'(' | b'{' => depth += 1,
            b')' | b'}' => depth -= 1,
            b'=' if depth <= 0 => {
                for (op, len) in [("::=", 3), (":=", 2), ("?=", 2), ("+=", 2), ("!=", 2)] {
                    if i + 1 >= len && line[i + 1 - len..=i] == *op {
                        return Some((i + 1 - len, op, len));
                    }
                }
                return Some((i, "=", 1));
            }
            // a rule, `target: deps`
            b':' if depth <= 0 && b.get(i + 1) != Some(&b'=') && b.get(i + 1) != Some(&b':') => return None,
            _ => {}
        }
    }
    None
}

fn strip_comment(line: &str) -> String {
    let b = line.as_bytes();
    let mut depth = 0i32;
    for i in 0..b.len() {
        match b[i] {
            b'(' | b'{' => depth += 1,
            b')' | b'}' => depth -= 1,
            b'#' if depth <= 0 && (i == 0 || b[i - 1] != b'\\') => return line[..i].to_string(),
            _ => {}
        }
    }
    line.to_string()
}

fn glob_in(dir: &Path, pattern: &str) -> Vec<String> {
    let full = if Path::new(pattern).is_absolute() { PathBuf::from(pattern) } else { dir.join(pattern) };
    let mut out: Vec<String> = match glob::glob(&full.to_string_lossy()) {
        Ok(it) => it.flatten().map(|p| p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().into_owned()).collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

// `find DIR... [-name PAT] [-type f|d]`, the one use of shell that migrates cleanly
fn emulate_find(dir: &Path, args: &[String]) -> Option<Vec<String>> {
    let mut roots: Vec<String> = Vec::new();
    let (mut name, mut ty): (Option<String>, Option<String>) = (None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-name" => {
                name = Some(args.get(i + 1)?.trim_matches(|c| c == '\'' || c == '"').to_string());
                i += 2;
            }
            "-type" => {
                ty = Some(args.get(i + 1)?.clone());
                i += 2;
            }
            a if a.starts_with('-') => return None,
            a => {
                roots.push(a.trim_matches(|c| c == '\'' || c == '"').to_string());
                i += 1;
            }
        }
    }
    if roots.is_empty() {
        return None;
    }
    let pat = name.as_deref().map(|n| glob::Pattern::new(n).ok()).unwrap_or(None);
    if name.is_some() && pat.is_none() {
        return None;
    }
    let mut out = Vec::new();
    fn walk(dir: &Path, rel: &Path, ty: &Option<String>, pat: &Option<glob::Pattern>, out: &mut Vec<String>, depth: usize) {
        if depth > 32 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir.join(rel)) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let r = rel.join(e.file_name());
            let is_dir = e.path().is_dir();
            let name_ok = pat.as_ref().map_or(true, |p| p.matches(&e.file_name().to_string_lossy()));
            let type_ok = match ty.as_deref() {
                Some("d") => is_dir,
                Some("f") => !is_dir,
                _ => true,
            };
            if name_ok && type_ok {
                out.push(r.to_string_lossy().into_owned());
            }
            if is_dir {
                walk(dir, &r, ty, pat, out, depth + 1);
            }
        }
    }
    for r in roots {
        let root = Path::new(&r);
        if !dir.join(root).exists() {
            continue;
        }
        // find lists the root itself first
        let is_dir = dir.join(root).is_dir();
        let name_ok = pat.as_ref().map_or(true, |p| root.file_name().is_some_and(|n| p.matches(&n.to_string_lossy())));
        let type_ok = match ty.as_deref() {
            Some("d") => is_dir,
            Some("f") => !is_dir,
            _ => true,
        };
        if name_ok && type_ok {
            out.push(r.clone());
        }
        if is_dir {
            walk(dir, root, &ty, &pat, &mut out, 0);
        }
    }
    Some(out)
}

impl Make {
    pub fn new(dir: &Path, preset: &[(String, String)]) -> Make {
        let mut m = Make {
            dir: dir.to_path_buf(),
            vars: HashMap::new(),
            fixed: preset.iter().cloned().collect(),
            notes: Vec::new(),
            rules: Vec::new(),
            includes: Vec::new(),
            assigned: Vec::new(),
            expanding: Vec::new(),
        };
        let abs = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()).to_string_lossy().into_owned();
        for k in ["CURDIR", "THEOS_PROJECT_DIR"] {
            m.vars.insert(k.to_string(), Var { immediate: true, value: abs.clone() });
        }
        m
    }

    fn note(&mut self, s: String) {
        if !self.notes.contains(&s) {
            self.notes.push(s);
        }
    }

    pub fn get(&mut self, name: &str) -> Option<String> {
        if let Some(v) = self.fixed.get(name) {
            return Some(v.clone());
        }
        let v = self.vars.get(name)?.clone();
        if v.immediate {
            return Some(v.value);
        }
        if self.expanding.iter().any(|e| e == name) {
            self.note(format!("{} refers to itself", name));
            return Some(String::new());
        }
        self.expanding.push(name.to_string());
        let out = self.expand_inner(&v.value, 0);
        self.expanding.pop();
        Some(out)
    }

    pub fn list(&mut self, name: &str) -> Vec<String> {
        self.get(name).map(|v| split_words(&v)).unwrap_or_default()
    }

    pub fn expand(&mut self, s: &str) -> String {
        self.expand_inner(s, 0)
    }

    fn expand_inner(&mut self, s: &str, depth: usize) -> String {
        if depth > MAX_DEPTH {
            self.note("an expression nests too deep".to_string());
            return String::new();
        }
        let b = s.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] != b'$' {
                let ch = s[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
            match b.get(i + 1) {
                Some(b'$') => {
                    out.push('$');
                    i += 2;
                }
                Some(&open) if open == b'(' || open == b'{' => {
                    let close = if open == b'(' { b')' } else { b'}' };
                    let mut d = 0;
                    let mut end = None;
                    for (k, &c) in b.iter().enumerate().skip(i + 1) {
                        if c == open {
                            d += 1;
                        } else if c == close {
                            d -= 1;
                            if d == 0 {
                                end = Some(k);
                                break;
                            }
                        }
                    }
                    let Some(end) = end else {
                        out.push_str(&s[i..]);
                        break;
                    };
                    let inner = &s[i + 2..end];
                    out.push_str(&self.reference(inner, depth));
                    i = end + 1;
                }
                Some(c) if c.is_ascii_alphanumeric() => {
                    // a one letter variable
                    let name = (*c as char).to_string();
                    out.push_str(&self.get(&name).unwrap_or_default());
                    i += 2;
                }
                _ => {
                    out.push('$');
                    i += 1;
                }
            }
        }
        out
    }

    // the text of `$(...)`: a variable or a function call
    fn reference(&mut self, inner: &str, depth: usize) -> String {
        let first = inner.split_whitespace().next().unwrap_or("");
        let is_func = inner.len() > first.len()
            && matches!(
                first,
                "wildcard" | "filter-out" | "filter" | "addprefix" | "addsuffix" | "patsubst" | "subst" | "strip" | "sort" | "notdir" | "dir" | "abspath" | "realpath" | "shell" | "if" | "or" | "and" | "foreach" | "call" | "eval" | "word" | "words" | "firstword" | "findstring" | "basename" | "suffix" | "error" | "warning" | "info" | "origin" | "value" | "join"
            );
        if !is_func {
            let name = self.expand_inner(inner, depth + 1);
            return match self.get(name.trim()) {
                Some(v) => v,
                None => String::new(),
            };
        }
        let rest = inner[first.len()..].trim_start();
        // split the arguments at top level commas
        let mut args: Vec<String> = Vec::new();
        let mut d = 0;
        let mut cur = String::new();
        for c in rest.chars() {
            match c {
                '(' | '{' => {
                    d += 1;
                    cur.push(c);
                }
                ')' | '}' => {
                    d -= 1;
                    cur.push(c);
                }
                ',' if d == 0 => args.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            }
        }
        args.push(cur);
        let ex = |m: &mut Make, k: usize| -> String { args.get(k).map(|a| m.expand_inner(a, depth + 1)).unwrap_or_default() };
        match first {
            "wildcard" => {
                let pats = split_words(&ex(self, 0));
                let mut out = Vec::new();
                for p in pats {
                    if p.contains(['*', '?', '[']) {
                        out.extend(glob_in(&self.dir.clone(), &p));
                    } else if self.dir.join(&p).exists() {
                        out.push(p);
                    }
                }
                out.join(" ")
            }
            "filter-out" | "filter" => {
                let pats = split_words(&ex(self, 0));
                let words = split_words(&ex(self, 1));
                let hit = |w: &str| pats.iter().any(|p| glob::Pattern::new(&p.replace('%', "*")).map_or(false, |g| g.matches(w)));
                words.into_iter().filter(|w| hit(w) == (first == "filter")).collect::<Vec<_>>().join(" ")
            }
            "addprefix" => {
                let p = ex(self, 0);
                split_words(&ex(self, 1)).iter().map(|w| format!("{}{}", p, w)).collect::<Vec<_>>().join(" ")
            }
            "addsuffix" => {
                let p = ex(self, 0);
                split_words(&ex(self, 1)).iter().map(|w| format!("{}{}", w, p)).collect::<Vec<_>>().join(" ")
            }
            "subst" => {
                let (from, to, text) = (ex(self, 0), ex(self, 1), ex(self, 2));
                if from.is_empty() { text } else { text.replace(&from, &to) }
            }
            "patsubst" => {
                let (pat, rep, text) = (ex(self, 0), ex(self, 1), ex(self, 2));
                split_words(&text)
                    .iter()
                    .map(|w| match pat.split_once('%') {
                        Some((pre, post)) if w.starts_with(pre) && w.ends_with(post) && w.len() >= pre.len() + post.len() => {
                            rep.replace('%', &w[pre.len()..w.len() - post.len()])
                        }
                        None if *w == pat => rep.clone(),
                        _ => w.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            "strip" => split_words(&ex(self, 0)).join(" "),
            "sort" => {
                let mut w = split_words(&ex(self, 0));
                w.sort();
                w.dedup();
                w.join(" ")
            }
            "notdir" => split_words(&ex(self, 0)).iter().map(|w| w.rsplit('/').next().unwrap_or(w).to_string()).collect::<Vec<_>>().join(" "),
            "dir" => split_words(&ex(self, 0))
                .iter()
                .map(|w| match w.rfind('/') {
                    Some(i) => w[..=i].to_string(),
                    None => "./".to_string(),
                })
                .collect::<Vec<_>>()
                .join(" "),
            "abspath" | "realpath" => {
                let dir = self.dir.canonicalize().unwrap_or_else(|_| self.dir.clone());
                split_words(&ex(self, 0))
                    .iter()
                    .map(|w| {
                        let p = if Path::new(w).is_absolute() { PathBuf::from(w) } else { dir.join(w) };
                        let mut parts: Vec<std::ffi::OsString> = Vec::new();
                        for c in p.components() {
                            match c {
                                std::path::Component::ParentDir => {
                                    parts.pop();
                                }
                                std::path::Component::CurDir => {}
                                c => parts.push(c.as_os_str().to_os_string()),
                            }
                        }
                        let mut r = PathBuf::new();
                        for c in parts {
                            r.push(c);
                        }
                        r.to_string_lossy().into_owned()
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            "shell" => {
                let cmd = ex(self, 0);
                let words = split_words(&cmd);
                if words.first().map(String::as_str) == Some("find") {
                    if let Some(found) = emulate_find(&self.dir.clone(), &words[1..]) {
                        return found.join(" ");
                    }
                }
                self.note(format!("$(shell {}) is not run, the value is empty, fill it in by hand", cmd));
                String::new()
            }
            "origin" | "value" => String::new(),
            other => {
                self.note(format!("$({} ...) is not supported, the value is empty", other));
                String::new()
            }
        }
    }

    fn cond(&mut self, kw: &str, rest: &str) -> bool {
        match kw {
            "ifdef" | "ifndef" => {
                let name = self.expand(rest.trim());
                let set = self.get(name.trim()).map_or(false, |v| !v.is_empty());
                (kw == "ifdef") == set
            }
            _ => {
                let t = rest.trim();
                let (a, b) = if let Some(inner) = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
                    let mut d = 0;
                    let mut at = None;
                    for (k, c) in inner.char_indices() {
                        match c {
                            '(' | '{' => d += 1,
                            ')' | '}' => d -= 1,
                            ',' if d == 0 => {
                                at = Some(k);
                                break;
                            }
                            _ => {}
                        }
                    }
                    match at {
                        Some(k) => (inner[..k].to_string(), inner[k + 1..].to_string()),
                        None => (inner.to_string(), String::new()),
                    }
                } else {
                    let q: Vec<&str> = t.split(['"', '\'']).filter(|s| !s.trim().is_empty()).collect();
                    (q.first().unwrap_or(&"").to_string(), q.get(1).unwrap_or(&"").to_string())
                };
                let (a, b) = (self.expand(a.trim()), self.expand(b.trim()));
                (a.trim() == b.trim()) == (kw == "ifeq")
            }
        }
    }

    pub fn parse(&mut self, text: &str) {
        self.parse_inner(text, 0);
    }

    fn parse_inner(&mut self, text: &str, depth: usize) {
        // logical lines, a trailing backslash joins the next line
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cont = false;
        for raw in text.lines() {
            let l = raw.trim_end_matches('\r');
            if cont {
                cur.push(' ');
                cur.push_str(l.trim_start());
            } else {
                cur = l.to_string();
            }
            if cur.ends_with('\\') {
                cur.pop();
                cont = true;
            } else {
                cont = false;
                lines.push(std::mem::take(&mut cur));
            }
        }
        if cont {
            lines.push(cur);
        }

        // (this branch is active, a branch was already taken, the parent is active)
        let mut stack: Vec<(bool, bool, bool)> = Vec::new();
        let mut in_define = false;
        for line in lines {
            let active = stack.last().map_or(true, |s| s.0);
            if in_define {
                if line.trim() == "endef" {
                    in_define = false;
                }
                continue;
            }
            if line.starts_with('\t') {
                // a recipe line
                continue;
            }
            let l = strip_comment(&line);
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            let (kw, rest) = t.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((t, ""));
            match kw {
                "ifeq" | "ifneq" | "ifdef" | "ifndef" => {
                    let v = if active { self.cond(kw, rest) } else { false };
                    if active {
                        self.note(format!("{} {} is {} here, set the variables it reads with --set", kw, rest, v));
                    }
                    stack.push((active && v, active && v, active));
                    continue;
                }
                "else" => {
                    if let Some(top) = stack.last().copied() {
                        let (_, taken, parent) = top;
                        let (k2, r2) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
                        let now = if rest.is_empty() {
                            parent && !taken
                        } else if matches!(k2, "ifeq" | "ifneq" | "ifdef" | "ifndef") {
                            parent && !taken && self.cond(k2, r2)
                        } else {
                            false
                        };
                        let n = stack.len() - 1;
                        stack[n] = (now, taken || now, parent);
                    }
                    continue;
                }
                "endif" => {
                    stack.pop();
                    continue;
                }
                _ => {}
            }
            if !active {
                continue;
            }
            if kw == "define" {
                in_define = true;
                continue;
            }
            let t = t.strip_prefix("override ").unwrap_or(t).trim_start();
            let t = t.strip_prefix("export ").unwrap_or(t).trim_start();
            if let Some(rest) = t.strip_prefix("include ").or_else(|| t.strip_prefix("-include ")).or_else(|| t.strip_prefix("sinclude ")) {
                // theos makefiles are named by their raw text, THEOS is not set here
                if rest.contains("THEOS") || rest.contains("makefiles/") {
                    let name = rest.trim().rsplit('/').next().unwrap_or(rest).to_string();
                    if !self.includes.contains(&name) {
                        self.includes.push(name);
                    }
                    continue;
                }
                let target = self.expand(rest);
                for f in split_words(&target) {
                    self.include(&f, depth);
                }
                continue;
            }
            if let Some((at, op, len)) = find_assign(t) {
                let name = self.expand(t[..at].trim());
                let name = name.trim().to_string();
                let value = t[at + len..].trim().to_string();
                self.assign(&name, op, &value);
                continue;
            }
            // `export NAME` alone, or a rule
            if let Some(colon) = t.find(':') {
                let targets = self.expand(&t[..colon]);
                for r in split_words(&targets) {
                    if !self.rules.contains(&r) {
                        self.rules.push(r);
                    }
                }
            }
        }
    }

    fn include(&mut self, f: &str, depth: usize) {
        let path = if Path::new(f).is_absolute() { PathBuf::from(f) } else { self.dir.join(f) };
        match std::fs::read_to_string(&path) {
            Ok(text) if depth < 8 => self.parse_inner(&text, depth + 1),
            Ok(_) => self.note(format!("include {} nests too deep", f)),
            Err(_) => self.note(format!("include {} is not read, the file is missing", f)),
        }
    }

    fn assign(&mut self, name: &str, op: &str, value: &str) {
        if name.is_empty() {
            return;
        }
        if !self.assigned.iter().any(|a| a == name) {
            self.assigned.push(name.to_string());
        }
        match op {
            "?=" => {
                if !self.vars.contains_key(name) && !self.fixed.contains_key(name) {
                    self.vars.insert(name.to_string(), Var { immediate: false, value: value.to_string() });
                }
            }
            "+=" => {
                let (imm, old) = match self.vars.get(name) {
                    Some(v) => (v.immediate, v.value.clone()),
                    None => (false, String::new()),
                };
                let add = if imm { self.expand(value) } else { value.to_string() };
                let joined = if old.is_empty() { add } else { format!("{} {}", old, add) };
                self.vars.insert(name.to_string(), Var { immediate: imm, value: joined });
            }
            ":=" | "::=" => {
                let v = self.expand(value);
                self.vars.insert(name.to_string(), Var { immediate: true, value: v });
            }
            "!=" => {
                self.note(format!("{} != ... runs a command and is not migrated", name));
            }
            _ => {
                self.vars.insert(name.to_string(), Var { immediate: false, value: value.to_string() });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meow-make-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src/sub")).unwrap();
        for f in ["src/a.m", "src/b.c", "src/sub/c.m", "src/sub/d.mm"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        d
    }

    #[test]
    fn assignments_flavors_and_continuations() {
        let d = dir("assign");
        let mut m = Make::new(&d, &[]);
        m.parse("A = 1\nB := $(A) two\nA = 3\nC ?= x\nC ?= y\nD = a \\\n  b\nD += c\nexport E := $(B)!\nF = $(G)\nG = late\n");
        assert_eq!(m.get("A").unwrap(), "3");
        assert_eq!(m.get("B").unwrap(), "1 two");
        assert_eq!(m.get("C").unwrap(), "x");
        assert_eq!(m.list("D"), ["a", "b", "c"]);
        assert_eq!(m.get("E").unwrap(), "1 two!");
        assert_eq!(m.get("F").unwrap(), "late");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn conditionals_use_the_variables_given() {
        let d = dir("cond");
        let text = "ifeq ($(S),rootless)\nX = root\nelse ifeq ($(S),roothide)\nX = hide\nelse\nX = plain\nendif\nifdef Y\nZ = y\nendif\nifneq (1,2)\nW = 1\nendif\n";
        for (s, want) in [("rootless", "root"), ("roothide", "hide"), ("", "plain")] {
            let mut m = Make::new(&d, &[("S".into(), s.into())]);
            m.parse(text);
            assert_eq!(m.get("X").unwrap(), want, "{}", s);
            assert!(m.get("Z").is_none());
            assert_eq!(m.get("W").unwrap(), "1");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn functions_build_file_lists() {
        let d = dir("fn");
        let mut m = Make::new(&d, &[]);
        m.parse("W = $(wildcard src/*.m src/*/*.m)\nF = $(filter-out src/a.m,$(W))\nP = $(addprefix -I,$(shell find src -type d))\nS = $(shell find src -name '*.c')\nN = $(notdir $(W))\nR = $(patsubst %.m,%.o,$(W))\nB = $(subst a,b,banana)\nU = $(shell uname)\n");
        assert_eq!(m.list("W"), ["src/a.m", "src/sub/c.m"]);
        assert_eq!(m.list("F"), ["src/sub/c.m"]);
        assert_eq!(m.list("P"), ["-Isrc", "-Isrc/sub"]);
        assert_eq!(m.list("S"), ["src/b.c"]);
        assert_eq!(m.list("N"), ["a.m", "c.m"]);
        assert_eq!(m.list("R"), ["src/a.o", "src/sub/c.o"]);
        assert_eq!(m.get("B").unwrap(), "bbnbnb");
        assert_eq!(m.get("U").unwrap(), "");
        assert!(m.notes.iter().any(|n| n.contains("uname")));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rules_includes_and_the_project_directory() {
        let d = dir("rules");
        std::fs::write(d.join("extra.mk"), "FROM_EXTRA = yes\n").unwrap();
        let mut m = Make::new(&d, &[]);
        m.parse("include $(THEOS)/makefiles/common.mk\ninclude extra.mk\n-include missing.mk\nR = $(CURDIR)/x\nall:\n\t@echo hi\nafter-install::\n\tinstall.exec\nTWEAK_NAME = t\ninclude $(THEOS_MAKE_PATH)/tweak.mk\n");
        assert_eq!(m.get("FROM_EXTRA").unwrap(), "yes");
        assert!(m.get("R").unwrap().ends_with("/x"));
        assert!(m.rules.contains(&"all".to_string()) && m.rules.contains(&"after-install".to_string()));
        assert!(m.includes.contains(&"tweak.mk".to_string()));
        assert!(m.notes.iter().any(|n| n.contains("missing.mk")));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_loop_ends() {
        let d = dir("loop");
        let mut m = Make::new(&d, &[]);
        m.parse("A = $(B)\nB = $(A)\n");
        assert_eq!(m.get("A").unwrap(), "");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
