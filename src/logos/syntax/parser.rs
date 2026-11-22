// second pass: find the directives, build the model and record the patches

use crate::logos::ctx::{Ctx, Fatal, R};
use crate::logos::model::{Model, Property, UNGROUPED};
use crate::logos::emit::patch::{Call, Patch, Source};
use crate::logos::text::{
    falls_between, is_word, matched_parenthesis_set, quotes, skip_space, smart_split,
    trim_end_ws, trim_start_ws, trim_ws,
};

pub struct Parsed {
    pub model: Model,
    pub patches: Vec<Patch>,
    /// every class name seen, in first use order
    pub classes: Vec<String>,
    pub first_directive: Option<(usize, usize)>,
    pub last_init: Option<(usize, usize)>,
    /// brace depth after each token, ordered by position
    pub depth_map: Vec<(usize, usize, i64)>,
}

struct CallSpan {
    args: String,
    end_line: usize,
    end_pos: usize,
}

struct Parser<'a> {
    ctx: &'a mut Ctx,
    lines: &'a [String],
    m: Model,
    patches: Vec<Patch>,
    classes: Vec<String>,
    nesting: Vec<(&'static str, usize)>,
    first_directive: Option<(usize, usize)>,
    last_init: Option<(usize, usize)>,
    depth_map: Vec<(usize, usize, i64)>,
    depth: i64,
    current_group: usize,
    current_class: Option<usize>,
    current_method: Option<usize>,
    new_method_type: Option<String>,
    current_function: Option<usize>,
    skip_to: Option<(usize, usize)>,
}

fn ident_char(b: u8) -> bool {
    is_word(b) || b == b'$'
}

fn scan_while(b: &[u8], mut i: usize, f: impl Fn(u8) -> bool) -> usize {
    while i < b.len() && f(b[i]) {
        i += 1;
    }
    i
}

fn word_end(b: &[u8], i: usize) -> bool {
    i >= b.len() || !is_word(b[i])
}

fn contains_word(s: &str, w: &str) -> bool {
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(off) = s[from..].find(w) {
        let end = from + off + w.len();
        if word_end(b, end) {
            return true;
        }
        from = from + off + 1;
    }
    false
}

/// zero width start of something the directive scanner looks at
fn candidate(b: &[u8], p: usize) -> bool {
    let prev = p > 0 && is_word(b[p - 1]);
    let cur = p < b.len() && is_word(b[p]);
    if prev != cur || p >= b.len() {
        return false;
    }
    match b[p] {
        b'%' => p + 1 < b.len() && is_word(b[p + 1]),
        b'&' => {
            let q = skip_space(b, p + 1);
            q + 1 < b.len() && b[q] == b'%' && is_word(b[q + 1])
        }
        b'+' | b'-' => {
            let q = skip_space(b, p + 1);
            q < b.len() && b[q] == b'(' && b[q + 1..].contains(&b')')
        }
        _ => false,
    }
}

fn find_candidate(b: &[u8], from: usize, forbid: Option<usize>) -> Option<usize> {
    (from..b.len()).find(|&p| Some(p) != forbid && candidate(b, p))
}

fn protocol_forward_declaration(rest: &str) -> bool {
    // `protocol X;`, `protocol X, Y;` or `protocol(`
    let after = rest.strip_prefix("protocol").unwrap_or("").as_bytes();
    let j = skip_space(after, 0);
    if j < after.len() && after[j] == b'(' {
        return true;
    }
    if j == 0 {
        return false;
    }
    let mut i = j;
    loop {
        let e = scan_while(after, i, ident_char);
        if e == i {
            return false;
        }
        i = e;
        if i < after.len() && after[i] == b',' {
            i = skip_space(after, i + 1);
        }
        if i < after.len() && ident_char(after[i]) {
            continue;
        }
        break;
    }
    i < after.len() && after[i] == b';'
}

impl<'a> Parser<'a> {
    fn err(&self, lineno: usize, msg: &str) -> Fatal {
        self.ctx.err(lineno as i64, msg)
    }

    fn note_class(&mut self, name: &str) {
        if !self.classes.iter().any(|c| c == name) {
            self.classes.push(name.to_string());
        }
    }

    fn nesting_contains(&self, kinds: &[&str]) -> Option<(&'static str, usize)> {
        self.nesting.iter().copied().find(|(k, _)| kinds.contains(k))
    }

    fn must_not_contain(&self, lineno: usize, trying: &str, kinds: &[&str]) -> R<()> {
        if let Some((k, l)) = self.nesting_contains(kinds) {
            let msg = format!("{} inside a %{}, opened at {}", trying, k, self.ctx.describe_line(l));
            return Err(self.err(lineno, &msg));
        }
        Ok(())
    }

    fn must_contain(&self, lineno: usize, trying: &str, kinds: &[&str]) -> R<()> {
        if self.nesting_contains(kinds).is_none() {
            return Err(self.err(lineno, &format!("{} found outside of {}", trying, kinds.join(" or "))));
        }
        Ok(())
    }

    fn mark_first(&mut self, lineno: usize, pos: usize) {
        if self.first_directive.is_none() {
            self.first_directive = Some((lineno, pos));
        }
    }

    fn patch_here(&mut self, lineno: usize, start: usize, end: usize, source: Source) {
        self.patches.push(Patch::at(lineno, start, end, source));
    }

    fn pretty_method(&self, mi: usize) -> String {
        let m = &self.m.methods[mi];
        format!("{}[{} {}]", m.scope as char, self.m.classes[m.class].name, m.selector())
    }

    fn util<T>(&self, lineno: usize, r: Result<T, String>) -> R<T> {
        r.map_err(|e| self.err(lineno, &e))
    }

    /// the call arguments behind `from`, which can continue on the following lines
    fn scan_call(&self, lineno: usize, line: &str, from: usize) -> R<Option<CallSpan>> {
        let b = line.as_bytes();
        let i = skip_space(b, from);
        if i >= b.len() || b[i] != b'(' {
            return Ok(None);
        }
        let rem = &line[from..];
        let q = quotes(rem);
        let mut depth = 0i64;
        let mut open_after = 0;
        for (k, &c) in rem.as_bytes().iter().enumerate() {
            if !matches!(c, b'(' | b')') || falls_between(k, &q) {
                continue;
            }
            if c == b'(' {
                if depth == 0 {
                    open_after = from + k + 1;
                }
                depth += 1;
            } else {
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(CallSpan {
                        args: line[open_after..from + k].to_string(),
                        end_line: lineno,
                        end_pos: from + k + 1,
                    }));
                }
            }
        }
        let mut args = line[open_after..].to_string();
        for l in lineno + 1..self.lines.len() {
            let text = &self.lines[l];
            let tq = quotes(text);
            for (k, &c) in text.as_bytes().iter().enumerate() {
                if !matches!(c, b'(' | b')') || falls_between(k, &tq) {
                    continue;
                }
                if c == b'(' {
                    depth += 1;
                } else {
                    depth -= 1;
                    if depth == 0 {
                        args.push(' ');
                        args.push_str(trim_start_ws(&text[..k]));
                        return Ok(Some(CallSpan { args, end_line: l, end_pos: k + 1 }));
                    }
                }
            }
            args.push(' ');
            args.push_str(trim_start_ws(text));
        }
        Err(self.err(lineno, "missing closing parenthesis"))
    }

    fn parse_all(&mut self) -> R<()> {
        let lines = self.lines;
        for (lineno, line) in lines.iter().enumerate() {
            let q = quotes(line);
            let b = line.as_bytes();

            let start_depth = self.depth;
            let mut toks: Vec<(usize, i64)> = Vec::new();
            let mut i = 0;
            while i < b.len() {
                let c = b[i];
                let mut width = 1;
                let mut kind: Option<&str> = None;
                if c == b'{' || c == b'}' {
                    kind = Some(if c == b'{' { "{" } else { "}" });
                } else if i > 0 && b[i - 1] == b'@' {
                    for w in ["interface", "implementation", "protocol", "end"] {
                        if line[i..].starts_with(w) {
                            kind = Some(w);
                            width = w.len();
                            break;
                        }
                    }
                }
                if let Some(k) = kind {
                    if !falls_between(i, &q) {
                        match k {
                            "{" | "implementation" | "interface" => self.depth += 1,
                            "protocol" => {
                                if !protocol_forward_declaration(&line[i..]) {
                                    self.depth += 1;
                                }
                            }
                            _ => self.depth -= 1,
                        }
                        if self.depth < 0 {
                            let msg = format!("fell off the face of the planet when we found a '{}'", k);
                            return Err(self.err(lineno, &msg));
                        }
                        self.depth_map.push((lineno, i + 1, self.depth));
                        toks.push((i + 1, self.depth));
                    }
                }
                i += width;
            }

            let mut start_pos = 0;
            if let Some((sl, sp)) = self.skip_to {
                if lineno < sl {
                    continue;
                }
                if lineno == sl {
                    start_pos = sp;
                    self.skip_to = None;
                }
            }
            self.scan_directives(lineno, line, &q, &toks, start_depth, start_pos)?;
        }
        Ok(())
    }

    fn scan_directives(
        &mut self,
        lineno: usize,
        line: &str,
        q: &[(usize, usize)],
        toks: &[(usize, i64)],
        start_depth: i64,
        start_pos: usize,
    ) -> R<()> {
        let b = line.as_bytes();
        let mut pos = start_pos;
        let mut forbid: Option<usize> = None;
        loop {
            let Some(p) = find_candidate(b, pos, forbid) else { break };
            if falls_between(p, q) {
                pos = p;
                forbid = Some(p);
                continue;
            }
            let mut depth = start_depth;
            for &(tp, d) in toks {
                if tp <= p {
                    depth = d;
                } else {
                    break;
                }
            }
            let new_pos = self.directive(lineno, line, p, depth)?;
            forbid = if new_pos == p { Some(p) } else { None };
            pos = new_pos;
        }
        Ok(())
    }

    /// handles the directive at `p`, returns where scanning goes on
    fn directive(&mut self, lineno: usize, line: &str, p: usize, depth: i64) -> R<usize> {
        let b = line.as_bytes();
        let rest = &line[p..];

        if let Some(r) = rest.strip_prefix("%hook") {
            let rb = r.as_bytes();
            let j = skip_space(rb, 0);
            let k = scan_while(rb, j, |c| ident_char(c) || c == b'.');
            if j > 0 && k > j {
                let end = p + 5 + k;
                return self.dir_hook(lineno, p, end, &r[j..k], depth);
            }
        }

        if let Some(r) = rest.strip_prefix("%subclass") {
            if let Some((name, sup, protos, len)) = match_subclass(r) {
                return self.dir_subclass(lineno, p, p + 9 + len, &name, &sup, protos, depth);
            }
        }

        if let Some(r) = rest.strip_prefix("%group") {
            let rb = r.as_bytes();
            let j = skip_space(rb, 0);
            let k = scan_while(rb, j, ident_char);
            if j > 0 && k > j {
                return self.dir_group(lineno, p, p + 6 + k, &r[j..k], depth);
            }
        }

        if let Some(r) = rest.strip_prefix("%class") {
            let rb = r.as_bytes();
            let j = skip_space(rb, 0);
            if j > 0 {
                let (scope, s) = match rb.get(j) {
                    Some(&c) if c == b'+' || c == b'-' => (Some(c), j + 1),
                    _ => (None, j),
                };
                let k = scan_while(rb, s, ident_char);
                if k > s {
                    let end = p + 6 + k;
                    self.mark_first(lineno, p);
                    self.ctx.warn(
                        lineno as i64,
                        "%class is deprecated and will be removed in the future; consider switching to inline %c()",
                    );
                    let name = &r[s..k];
                    if scope == Some(b'+') {
                        self.m.static_classes.add_used_meta_class(name, name);
                    } else {
                        self.m.static_classes.add_used_class(name, name);
                    }
                    self.note_class(name);
                    self.patch_here(lineno, p, end, Source::None);
                    return Ok(end);
                }
            }
        }

        if let Some(r) = rest.strip_prefix("%c(") {
            let rb = r.as_bytes();
            let mut j = skip_space(rb, 0);
            let scope = match rb.get(j) {
                Some(&c) if c == b'+' || c == b'-' => {
                    j += 1;
                    Some(c)
                }
                _ => None,
            };
            let k = scan_while(rb, j, |c| ident_char(c) || c == b'.');
            let e = skip_space(rb, k);
            if k > j && e < rb.len() && rb[e] == b')' {
                let end = p + 3 + e + 1;
                return self.dir_class_ref(lineno, p, end, &r[j..k], scope.unwrap_or(b'-'));
            }
        }

        if let Some(r) = rest.strip_prefix("%new") {
            self.must_contain(lineno, "%new", &["hook", "subclass"])?;
            let mut end = p + 4;
            let mut xtype = String::new();
            if r.starts_with('(') {
                if let Some(close) = r[1..].find(')') {
                    xtype = r[1..1 + close].to_string();
                    end = p + 4 + 1 + close + 1;
                }
            }
            if xtype == "0" {
                xtype.clear();
            }
            self.new_method_type = Some(xtype);
            self.patch_here(lineno, p, end, Source::None);
            return Ok(end);
        }

        if self.current_class.is_some() && (b[p] == b'+' || b[p] == b'-') {
            if let Some((scope, ret, end)) = match_method_head(line, p) {
                if depth < 1 {
                    return self.dir_method(lineno, line, p, scope, &ret, end);
                }
                return Ok(end);
            }
        }

        if rest.starts_with("%orig") && word_end(b, p + 5) {
            return self.dir_orig(lineno, line, p, depth);
        }

        if rest.starts_with('&') {
            let j = skip_space(b, p + 1);
            if line[j..].starts_with("%orig") && word_end(b, j + 5) {
                return self.dir_orig_name(lineno, p, j + 5, depth);
            }
        }

        if rest.starts_with("%log") && word_end(b, p + 4) {
            return self.dir_log(lineno, line, p);
        }

        for (word, attr, tag) in [
            ("%ctor", "constructor", "_logosLocalCtor_"),
            ("%dtor", "destructor", "_logosLocalDtor_"),
        ] {
            if rest.starts_with(word) && word_end(b, p + 5) {
                if depth >= 1 {
                    return Err(self.err(lineno, &format!("{} does not make sense inside a block", word)));
                }
                self.must_not_contain(lineno, word, &["hook", "subclass"])?;
                let hash_input = format!("{}{}{}", &line[..p], lineno, &line[p + 5..]);
                let hash = crate::md5::hex(hash_input.as_bytes());
                let text = format!(
                    "static __attribute__(({})) void {}{}(int __unused argc, char __unused **argv, char __unused **envp)",
                    attr,
                    tag,
                    &hash[..8]
                );
                self.patch_here(lineno, p, p + 5, Source::Text(text));
                return Ok(p + 5);
            }
        }

        if rest.starts_with("%init") && word_end(b, p + 5) {
            return self.dir_init(lineno, line, p, depth);
        }

        if rest.starts_with("%end") && word_end(b, p + 4) {
            if depth >= 1 {
                return Err(self.err(lineno, "%end does not make sense inside a block"));
            }
            let Some((closing, _)) = self.nesting.pop() else {
                return Err(self.err(lineno, "dangling %end"));
            };
            if closing == "group" {
                self.current_group = 0;
            }
            if closing == "hook" || closing == "subclass" {
                self.current_class = None;
            }
            self.patch_here(lineno, p, p + 4, Source::None);
            return Ok(p + 4);
        }

        if let Some(r) = rest.strip_prefix("%config") {
            if let Some((key, value, len)) = match_config(r) {
                match key.as_str() {
                    "generator" => self.ctx.config.generator = value,
                    "warnings" => self.ctx.config.warnings = value,
                    _ => {}
                }
                let end = p + 7 + len;
                self.patch_here(lineno, p, end, Source::None);
                return Ok(end);
            }
        }

        if let Some(r) = rest.strip_prefix("%property") {
            if let Some(prop) = match_property(r) {
                return self.dir_property(lineno, p, p + 9 + prop.len, prop);
            }
        }

        if rest.starts_with("%hookf") && word_end(b, p + 6) {
            return self.dir_hookf(lineno, line, p, depth);
        }

        Ok(p)
    }

    fn dir_hook(&mut self, lineno: usize, p: usize, end: usize, name: &str, depth: i64) -> R<usize> {
        if depth >= 1 {
            return Err(self.err(lineno, "%hook does not make sense inside a block"));
        }
        self.must_not_contain(lineno, "%hook", &["hook", "subclass"])?;
        self.mark_first(lineno, p);
        self.nesting.push(("hook", lineno));

        let mut class_name = name.to_string();
        let mut expr = None;
        if class_name.contains('.') {
            self.ctx.warn(
                lineno as i64,
                &format!("Hooking Objective-C bridged Swift class {} has unintuitive behaviour and won\u{2019}t catch all invocations, see https://theos.dev/docs/swift for more details.", class_name),
            );
            expr = Some(format!("objc_getClass(\"{}\")", class_name));
            class_name = class_name.replace('.', "_");
        }
        let c = self.m.add_class_named(self.current_group, &class_name);
        if let Some(e) = expr {
            self.m.classes[c].set_expression(&e);
        }
        self.current_class = Some(c);
        self.note_class(&class_name);
        self.patch_here(lineno, p, end, Source::None);
        Ok(end)
    }

    #[allow(clippy::too_many_arguments)]
    fn dir_subclass(
        &mut self,
        lineno: usize,
        p: usize,
        end: usize,
        name: &str,
        sup: &str,
        protocols: Vec<String>,
        depth: i64,
    ) -> R<usize> {
        if depth >= 1 {
            return Err(self.err(lineno, "%subclass does not make sense inside a block"));
        }
        self.must_not_contain(lineno, "%subclass", &["hook", "subclass"])?;
        self.mark_first(lineno, p);
        self.nesting.push(("subclass", lineno));

        let superclass = self.m.add_class_named(self.current_group, sup);
        self.m.classes[superclass].required = true;
        let c = self.m.add_subclass(self.current_group, name, superclass);
        for proto in protocols {
            let sub = self.m.classes[c].subclass.as_mut().expect("subclass was just added");
            if !sub.protocols.contains(&proto) {
                sub.protocols.push(proto);
            }
        }
        self.current_class = Some(c);
        self.m.static_classes.add_declared_only(name);
        self.note_class(sup);
        self.note_class(name);
        self.patch_here(lineno, p, end, Source::None);
        Ok(end)
    }

    fn dir_group(&mut self, lineno: usize, p: usize, end: usize, name: &str, depth: i64) -> R<usize> {
        if depth >= 1 {
            return Err(self.err(lineno, "%group does not make sense inside a block"));
        }
        for &(k, l) in &self.nesting {
            if k == "hook" && l < lineno {
                return Err(self.err(lineno, "%group does not make sense inside a hook"));
            }
        }
        self.must_not_contain(lineno, "%group", &["group"])?;
        self.mark_first(lineno, p);
        self.nesting.push(("group", lineno));

        let source = match self.m.get_group(name) {
            Some(g) => {
                self.current_group = g;
                Source::None
            }
            None => {
                let g = self.m.add_group(name);
                self.current_group = g;
                Source::Gen(Call::GroupDeclarations(g))
            }
        };
        self.patch_here(lineno, p, end, source);
        Ok(end)
    }

    fn dir_class_ref(&mut self, lineno: usize, p: usize, end: usize, written: &str, scope: u8) -> R<usize> {
        self.mark_first(lineno, p);
        let mut class_name = written.to_string();
        if class_name.contains('.') {
            self.ctx.warn(
                lineno as i64,
                &format!("Dynamic lookup of Objective-C bridged Swift class {} has unintuitive behaviour and won\u{2019}t catch all invocations, see https://theos.dev/docs/swift for more details.", class_name),
            );
            class_name = class_name.replace('.', "_");
        }
        if scope == b'+' {
            self.m.static_classes.add_used_meta_class(&class_name, written);
        } else {
            self.m.static_classes.add_used_class(&class_name, written);
        }
        self.note_class(&class_name);
        self.patch_here(lineno, p, end, Source::Gen(Call::ClassReference(class_name, scope)));
        Ok(end)
    }

    fn dir_method(&mut self, lineno: usize, line: &str, p: usize, scope: u8, ret: &str, head_end: usize) -> R<usize> {
        let mut class = self.current_class.expect("checked by the caller");
        if self.m.classes[class].group != self.current_group {
            let name = self.m.classes[class].name.clone();
            class = self.m.add_class_named(self.current_group, &name);
            self.current_class = Some(class);
        }

        let mut method = crate::logos::model::Method {
            class,
            scope,
            ret: ret.to_string(),
            selector_parts: Vec::new(),
            arg_names: Vec::new(),
            arg_types: Vec::new(),
            is_new: false,
            type_encoding: String::new(),
            variadic: false,
        };
        if scope == b'+' {
            self.m.classes[class].has_meta_hooks = true;
        } else {
            self.m.classes[class].has_instance_hooks = true;
        }
        if let Some(t) = self.new_method_type.take() {
            method.is_new = true;
            method.type_encoding = t;
        }

        let (parts, end) = parse_selector(line, head_end, &mut method);
        method.selector_parts = parts;
        let mi = self.m.methods.len();
        self.m.methods.push(method);
        self.m.classes[class].methods.push(mi);
        self.current_method = Some(mi);
        self.patch_here(lineno, p, end, Source::Gen(Call::MethodDefinition(mi)));
        Ok(end)
    }

    fn dir_orig(&mut self, lineno: usize, line: &str, p: usize, depth: i64) -> R<usize> {
        let kw_end = p + 5;
        let call = self.scan_call(lineno, line, kw_end)?;
        let args = call.as_ref().map(|c| c.args.clone());
        if let Some(a) = &args {
            if contains_word(a, "%orig") || contains_word(a, "%log") {
                return Err(self.err(lineno, "Logos cannot be used within arguments to %orig"));
            }
        }

        enum Target {
            Method(usize),
            Function(usize),
        }
        let target = match self.current_class {
            None => {
                let Some(f) = self.current_function else {
                    return Err(self.err(lineno, "%orig does not make sense outside a function"));
                };
                if args.as_deref().map_or(true, |a| a.is_empty()) && self.m.functions[f].args.iter().any(|a| a == "...") {
                    return Err(self.err(lineno, "%orig requires arguments when hooking variadic functions"));
                }
                Target::Function(f)
            }
            Some(_) => {
                self.must_contain(lineno, "%orig", &["hook", "subclass"])?;
                let Some(mi) = self.current_method else {
                    return Err(self.err(lineno, "%orig does not make sense outside a method"));
                };
                if depth < 1 {
                    return Err(self.err(lineno, "%orig does not make sense outside a block"));
                }
                if self.m.methods[mi].is_new {
                    let msg = format!("%orig in new method {} will be non-operative.", self.pretty_method(mi));
                    self.ctx.warn(lineno as i64, &msg);
                }
                Target::Method(mi)
            }
        };
        let source = |t: &Target, a: Option<String>| match t {
            Target::Method(m) => Source::Gen(Call::MethodOriginalCall(*m, a)),
            Target::Function(f) => Source::Gen(Call::FunctionOriginalCall(*f, a)),
        };

        match call {
            None => {
                self.patch_here(lineno, p, kw_end, source(&target, None));
                Ok(kw_end)
            }
            Some(c) if c.end_line == lineno => {
                self.patch_here(lineno, p, c.end_pos, source(&target, args));
                Ok(c.end_pos)
            }
            Some(c) => {
                // the call continues on later lines, the first line carries all of it
                self.patch_here(lineno, p, line.len(), source(&target, args));
                for l in lineno + 1..c.end_line {
                    self.patches.push(Patch::at(l, 0, self.lines[l].len(), Source::None));
                }
                self.patches.push(Patch::at(c.end_line, 0, c.end_pos, Source::None));
                self.skip_to = Some((c.end_line, c.end_pos));
                Ok(line.len())
            }
        }
    }

    fn dir_orig_name(&mut self, lineno: usize, p: usize, end: usize, depth: i64) -> R<usize> {
        match self.current_class {
            None => {
                let Some(f) = self.current_function else {
                    return Err(self.err(lineno, "%orig does not make sense outside a function"));
                };
                self.patch_here(lineno, p, end, Source::Gen(Call::FunctionOriginalName(f)));
            }
            Some(_) => {
                self.must_contain(lineno, "%orig", &["hook", "subclass"])?;
                let Some(mi) = self.current_method else {
                    return Err(self.err(lineno, "%orig does not make sense outside a method"));
                };
                if depth < 1 {
                    return Err(self.err(lineno, "%orig does not make sense outside a block"));
                }
                if self.m.methods[mi].is_new {
                    let msg = format!(
                        "no original method pointer for &%orig in new method {}.",
                        self.pretty_method(mi)
                    );
                    return Err(self.err(lineno, &msg));
                }
                self.patch_here(lineno, p, end, Source::Gen(Call::MethodOriginalName(mi)));
            }
        }
        Ok(end)
    }

    fn dir_log(&mut self, lineno: usize, line: &str, p: usize) -> R<usize> {
        self.must_contain(lineno, "%log", &["hook", "subclass"])?;
        let Some(mi) = self.current_method else {
            return Err(self.err(lineno, "%log does not make sense outside a method"));
        };
        let kw_end = p + 4;
        let remaining = &line[kw_end..];
        let pairs = self.util(lineno, matched_parenthesis_set(remaining, true, false))?;
        let mut end = kw_end;
        let mut args = None;
        if let Some(&(o, c)) = pairs.first() {
            args = Some(remaining[o..c - 1].to_string());
            end = kw_end + c;
        }
        self.patch_here(lineno, p, end, Source::Gen(Call::MethodLog(mi, args)));
        Ok(end)
    }

    fn dir_init(&mut self, lineno: usize, line: &str, p: usize, depth: i64) -> R<usize> {
        if depth < 1 {
            return Err(self.err(lineno, "%init does not make sense outside a block"));
        }
        let kw_end = p + 5;
        let remaining = &line[kw_end..];
        let pairs = self.util(lineno, matched_parenthesis_set(remaining, true, false))?;
        let mut end = kw_end;
        let mut argstring = None;
        if let Some(&(o, c)) = pairs.first() {
            argstring = Some(remaining[o..c - 1].to_string());
            end = kw_end + c;
        }
        let mut args: Vec<String> = match &argstring {
            Some(s) => self.util(lineno, smart_split(b',', s, 0))?,
            None => Vec::new(),
        };

        let mut group_name = UNGROUPED.to_string();
        if let Some(first) = args.first() {
            if !first.is_empty() && first != "0" && !first.contains('=') {
                group_name = first.clone();
                args.remove(0);
            }
        }
        let Some(g) = self.m.get_group(&group_name) else {
            return Err(self.err(lineno, &format!("%init for an undefined %group {}", group_name)));
        };
        if self.m.groups[g].initialized {
            let msg = format!(
                "re-%init of %group {}, first initialized at {}",
                group_name,
                self.ctx.describe_line(self.m.groups[g].init_line.max(0) as usize)
            );
            return Err(self.err(lineno, &msg));
        }

        for arg in &args {
            if !arg.contains('=') {
                self.ctx.warn(lineno as i64, &format!("unknown argument to %init: {}", arg));
                continue;
            }
            let parts = self.util(lineno, smart_split(b'=', arg, 2))?;
            if parts.len() < 2 {
                self.ctx.warn(lineno as i64, "invalid symbol=expr in %init");
                continue;
            }
            let expr = parts[1].clone();
            let mut classname = trim_start_ws(&parts[0]).to_string();
            let mut scope = b'-';
            if classname.starts_with('+') || classname.starts_with('-') {
                scope = classname.as_bytes()[0];
                classname = classname[1..].to_string();
            }
            if let Some(c) = self.m.class_named(g, &classname) {
                if scope == b'-' {
                    self.m.classes[c].set_expression(&expr);
                } else {
                    self.m.classes[c].set_meta_expression(&expr);
                }
                continue;
            }
            if let Some(f) = self.m.function_named(g, &parts[0]) {
                self.m.functions[f].expression = Some(expr);
                continue;
            }
            self.ctx.warn(
                lineno as i64,
                &format!("tried to set expression for unknown class or function {} in group {}", classname, group_name),
            );
        }

        self.m.groups[g].init_line = lineno as i64;
        self.m.groups[g].initialized = true;

        let b = line.as_bytes();
        let mut e = end;
        loop {
            let j = skip_space(b, e);
            if j < b.len() && b[j] == b';' {
                e = j + 1;
            } else {
                break;
            }
        }
        self.patch_here(lineno, p, e, Source::Gen(Call::GroupInitializers(g)));
        self.last_init = Some((lineno, e));
        Ok(e)
    }

    fn dir_property(&mut self, lineno: usize, p: usize, end: usize, prop: PropertyMatch) -> R<usize> {
        self.must_contain(lineno, "%property", &["hook", "subclass"])?;
        let class = self.current_class.expect("hook or subclass is open");
        self.m.classes[class].has_instance_hooks = true;

        let attrs = match &prop.attributes {
            Some(a) if !a.is_empty() => a.clone(),
            _ => {
                return Err(self.err(
                    lineno,
                    "%property declaration is missing attributes (e.g., nonatomic, retain, etc)",
                ))
            }
        };

        let mut fields: Vec<String> = attrs.split(',').map(|s| s.to_string()).collect();
        let n = fields.len();
        for (i, f) in fields.iter_mut().enumerate() {
            if i > 0 {
                *f = trim_start_ws(f).to_string();
            }
            if i + 1 < n {
                *f = trim_end_ws(f).to_string();
            }
        }
        while fields.last().map_or(false, |f| f.is_empty()) {
            fields.pop();
        }

        let (mut assign, mut retain, mut copy, mut nonatomic, mut readonly) = (false, false, false, false, false);
        let (mut getter, mut setter): (Option<String>, Option<String>) = (None, None);
        for a in &fields {
            if let Some(g) = after_key(a, "getter=") {
                getter = Some(g);
            } else if let Some(s) = after_key_colon(a, "setter=") {
                setter = Some(s);
            } else if a == "assign" || a == "unsafe_unretained" {
                assign = true;
            } else if a == "retain" || a == "strong" {
                retain = true;
            } else if a == "copy" {
                copy = true;
            } else if a == "nonatomic" {
                nonatomic = true;
            } else if a == "readonly" {
                readonly = true;
            } else if a.contains("readwrite") || a.contains("weak") || a.contains("class") {
                return Err(self.err(lineno, &format!("property attribute '{}' not supported.", a)));
            } else {
                return Err(self.err(lineno, &format!("unknown property attribute '{}'.", a)));
            }
        }

        if assign && retain {
            return Err(self.err(lineno, "property attributes 'assign' and 'retain' are mutually exclusive."));
        }
        if assign && copy {
            return Err(self.err(lineno, "property attributes 'assign' and 'copy' are mutually exclusive."));
        }
        if copy && retain {
            return Err(self.err(lineno, "property attributes 'copy' and 'retain' are mutually exclusive."));
        }
        if readonly && (retain || copy || assign) {
            return Err(self.err(
                lineno,
                "property attribute 'readonly' cannot be used with memory management attributes (assign, retain, copy).",
            ));
        }

        if !assign && !retain && !copy {
            assign = true;
        }
        let mut policy = String::from("OBJC_ASSOCIATION_");
        if retain {
            policy.push_str("RETAIN");
        } else if copy {
            policy.push_str("COPY");
        } else {
            policy.push_str("ASSIGN");
        }
        if nonatomic && !assign {
            policy.push_str("_NONATOMIC");
        }

        let getter = getter.unwrap_or_else(|| prop.name.clone());
        let setter = setter.unwrap_or_else(|| {
            let n = prop.name.replace(':', "");
            let mut chars = n.chars();
            let cap = match chars.next() {
                Some(c) if c.is_ascii_lowercase() => format!("{}{}", c.to_ascii_uppercase(), chars.as_str()),
                _ => n.clone(),
            };
            format!("set{}", cap)
        });

        let pi = self.m.properties.len();
        self.m.properties.push(Property {
            class,
            group: self.current_group,
            name: prop.name,
            type_name: prop.type_name,
            readonly,
            policy,
            getter,
            setter,
            retain,
            copy,
            nonatomic,
        });
        self.m.classes[class].properties.push(pi);
        self.patch_here(lineno, p, end, Source::Gen(Call::PropertyDefinition(pi)));
        Ok(end)
    }

    fn dir_hookf(&mut self, lineno: usize, line: &str, p: usize, depth: i64) -> R<usize> {
        if depth >= 1 {
            return Err(self.err(lineno, "%hookf does not make sense inside a block"));
        }
        self.must_not_contain(lineno, "%hookf", &["hook", "subclass"])?;
        self.mark_first(lineno, p);

        let kw_end = p + 6;
        let remaining = &line[kw_end..];
        let pairs = self.util(lineno, matched_parenthesis_set(remaining, true, false))?;
        let Some(&(o, c)) = pairs.first() else {
            return Err(self.err(lineno, "%hookf needs (return type, function name, arguments...)"));
        };
        let args = self.util(lineno, smart_split(b',', &remaining[o..c - 1], 0))?;
        if args.len() < 2 {
            return Err(self.err(lineno, "%hookf needs (return type, function name, arguments...)"));
        }
        let end = kw_end + c;

        let fi = self.m.functions.len();
        self.m.functions.push(crate::logos::model::Function {
            name: args[1].clone(),
            expression: None,
            retval: args[0].clone(),
            args: args[2..].to_vec(),
            group: self.current_group,
        });
        self.m.groups[self.current_group].functions.push(fi);
        self.current_function = Some(fi);
        self.patch_here(lineno, p, end, Source::Gen(Call::FunctionDeclaration(fi)));
        Ok(end)
    }
}

/// `-(type)keyword` head of a method, returns scope, return type and the end offset
fn match_method_head(line: &str, p: usize) -> Option<(u8, String, usize)> {
    let b = line.as_bytes();
    let scope = b[p];
    let open = skip_space(b, p + 1);
    if open >= b.len() || b[open] != b'(' {
        return None;
    }
    for q in open + 1..b.len() {
        if b[q] != b')' {
            continue;
        }
        let j = skip_space(b, q + 1);
        if j < b.len() && (ident_char(b[j]) || b[j] == b':') {
            return Some((scope, trim_ws(&line[open + 1..q]).to_string(), q + 1));
        }
    }
    None
}

/// shortest `[\$\w]+?` that ends on a word boundary
fn lazy_name(b: &[u8], n: usize) -> Option<usize> {
    let mut e = n;
    while e < b.len() && ident_char(b[e]) {
        e += 1;
        let prev = is_word(b[e - 1]);
        let next = e < b.len() && is_word(b[e]);
        if prev != next {
            return Some(e);
        }
    }
    None
}

/// selector parts and arguments behind the `(type)` of a method head
fn parse_selector(line: &str, start: usize, method: &mut crate::logos::model::Method) -> (Vec<String>, usize) {
    let b = line.as_bytes();
    let mut parts = Vec::new();
    let mut pos = start;
    loop {
        let s = pos;
        let i = skip_space(b, s);
        let kw_end = scan_while(b, i, ident_char);
        let keyword = line[i..kw_end].to_string();

        let mut group: Option<(String, String, usize)> = None;
        let j = skip_space(b, kw_end);
        if j < b.len() && b[j] == b':' {
            let k = skip_space(b, j + 1);
            if k < b.len() && b[k] == b'(' {
                for q in k + 2..b.len() {
                    if b[q] != b')' {
                        continue;
                    }
                    let n = skip_space(b, q + 1);
                    if let Some(e) = lazy_name(b, n) {
                        group = Some((line[k + 1..q].to_string(), line[n..e].to_string(), e));
                        break;
                    }
                }
            }
            if group.is_none() {
                let n = skip_space(b, k);
                if let Some(e) = lazy_name(b, n) {
                    group = Some(("id".to_string(), line[n..e].to_string(), e));
                }
            }
        }

        if keyword.is_empty() && group.is_none() {
            pos = s;
            break;
        }
        parts.push(keyword);
        match group {
            None => {
                pos = kw_end;
                break;
            }
            Some((ty, name, e)) => {
                method.arg_types.push(ty);
                method.arg_names.push(name);
                pos = e;
            }
        }
    }

    // the dots are any three characters in the original pattern
    let i = skip_space(b, pos);
    if i < b.len() && b[i] == b',' {
        let j = skip_space(b, i + 1);
        let mut end = Some(0);
        let mut chars = line[j..].char_indices();
        for _ in 0..3 {
            end = chars.next().map(|(o, c)| o + c.len_utf8());
            if end.is_none() {
                break;
            }
        }
        if let Some(e) = end {
            method.variadic = true;
            pos = j + e;
        }
    }
    (parts, pos)
}

fn match_subclass(r: &str) -> Option<(String, String, Vec<String>, usize)> {
    let b = r.as_bytes();
    let j = skip_space(b, 0);
    if j == 0 {
        return None;
    }
    let k = scan_while(b, j, ident_char);
    if k == j {
        return None;
    }
    let c = skip_space(b, k);
    if c >= b.len() || b[c] != b':' {
        return None;
    }
    let s = skip_space(b, c + 1);
    let e = scan_while(b, s, ident_char);
    if e == s {
        return None;
    }
    let mut end = skip_space(b, e);
    let mut protocols = Vec::new();
    if end < b.len() && b[end] == b'<' {
        if let Some(gt) = r[end + 1..].find('>') {
            let inner = trim_ws(&r[end + 1..end + 1 + gt]);
            let mut fields: Vec<&str> = inner.split(',').map(trim_ws).collect();
            while fields.last().map_or(false, |f| f.is_empty()) {
                fields.pop();
            }
            protocols = fields.into_iter().map(|s| s.to_string()).collect();
            end = end + 1 + gt + 1;
        }
    }
    Some((r[j..k].to_string(), r[s..e].to_string(), protocols, end))
}

fn match_config(r: &str) -> Option<(String, String, usize)> {
    let b = r.as_bytes();
    let o = skip_space(b, 0);
    if o >= b.len() || b[o] != b'(' {
        return None;
    }
    let ks = skip_space(b, o + 1);
    let ke = scan_while(b, ks, is_word);
    if ke == ks {
        return None;
    }
    let eq = skip_space(b, ke);
    if eq >= b.len() || b[eq] != b'=' {
        return None;
    }
    let vs = skip_space(b, eq + 1);
    let close = r[vs..].find(')')? + vs;
    Some((r[ks..ke].to_string(), trim_end_ws(&r[vs..close]).to_string(), close + 1))
}

struct PropertyMatch {
    attributes: Option<String>,
    type_name: String,
    name: String,
    len: usize,
}

fn match_property(r: &str) -> Option<PropertyMatch> {
    let b = r.as_bytes();
    let mut i = skip_space(b, 0);
    let mut attributes = None;
    if i < b.len() && b[i] == b'(' {
        // `\s*\w+\s*(,\s*[\w=:]+\s*)*` and a closing parenthesis
        let start = i + 1;
        let mut k = skip_space(b, start);
        let w = scan_while(b, k, is_word);
        if w == k {
            return None;
        }
        k = skip_space(b, w);
        while k < b.len() && b[k] == b',' {
            let s = skip_space(b, k + 1);
            let e = scan_while(b, s, |c| is_word(c) || c == b'=' || c == b':');
            if e == s {
                return None;
            }
            k = skip_space(b, e);
        }
        if k >= b.len() || b[k] != b')' {
            return None;
        }
        attributes = Some(r[start..k].to_string());
        i = skip_space(b, k + 1);
    }

    let ident_start = |c: u8| c.is_ascii_alphabetic() || c == b'_' || c == b'$';
    if i >= b.len() || !ident_start(b[i]) {
        return None;
    }
    let type_start = i;
    let mut words: Vec<(usize, usize)> = Vec::new();
    let e = scan_while(b, i, |c| is_word(c) || c == b'$');
    words.push((i, e));
    let mut cur = e;
    loop {
        let s = skip_space(b, cur);
        if s < b.len() && ident_start(b[s]) {
            let e = scan_while(b, s, |c| is_word(c) || c == b'$');
            words.push((s, e));
            cur = e;
        } else {
            break;
        }
    }
    let mut stars_end = None;
    let mut c2 = cur;
    loop {
        let s = skip_space(b, c2);
        if s < b.len() && b[s] == b'*' {
            c2 = scan_while(b, s, |c| c == b'*');
            stars_end = Some(c2);
        } else {
            break;
        }
    }

    let (type_end, name_start, name_end);
    if let Some(se) = stars_end {
        let s = skip_space(b, se);
        if s >= b.len() || !(b[s].is_ascii_alphabetic() || b[s] == b'_') {
            return None;
        }
        type_end = se;
        name_start = s;
        name_end = scan_while(b, s, |c| is_word(c) || c == b'$');
    } else {
        if words.len() < 2 {
            return None;
        }
        let (ns, ne) = *words.last().expect("two words");
        if !(b[ns].is_ascii_alphabetic() || b[ns] == b'_') {
            return None;
        }
        type_end = words[words.len() - 2].1;
        name_start = ns;
        name_end = ne;
    }
    let semi = skip_space(b, name_end);
    if semi >= b.len() || b[semi] != b';' {
        return None;
    }
    Some(PropertyMatch {
        attributes,
        type_name: r[type_start..type_end].to_string(),
        name: r[name_start..name_end].to_string(),
        len: semi + 1,
    })
}

fn after_key(a: &str, key: &str) -> Option<String> {
    let i = a.find(key)?;
    let rest = &a[i + key.len()..];
    let e = scan_while(rest.as_bytes(), 0, is_word);
    (e > 0).then(|| rest[..e].to_string())
}

fn after_key_colon(a: &str, key: &str) -> Option<String> {
    let i = a.find(key)?;
    let rest = &a[i + key.len()..];
    let e = scan_while(rest.as_bytes(), 0, is_word);
    (e > 0 && rest[e..].starts_with(':')).then(|| rest[..e].to_string())
}

pub fn parse(ctx: &mut Ctx, lines: &[String]) -> R<Parsed> {
    let mut p = Parser {
        ctx,
        lines,
        m: Model::new(),
        patches: Vec::new(),
        classes: Vec::new(),
        nesting: Vec::new(),
        first_directive: None,
        last_init: None,
        depth_map: vec![(0, 0, 0)],
        depth: 0,
        current_group: 0,
        current_class: None,
        current_method: None,
        new_method_type: None,
        current_function: None,
        skip_to: None,
    };
    p.parse_all()?;

    let leftover: Vec<(&'static str, usize)> = p.nesting.iter().rev().copied().collect();
    for (kind, opened) in leftover {
        let msg = format!(
            "missing %end (%{} opened at {} extends to EOF)",
            kind,
            p.ctx.describe_line(opened)
        );
        p.ctx.warn(lines.len() as i64, &msg);
    }
    Ok(Parsed {
        model: p.m,
        patches: p.patches,
        classes: p.classes,
        first_directive: p.first_directive,
        last_init: p.last_init,
        depth_map: p.depth_map,
    })
}
