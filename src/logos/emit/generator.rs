// text generators, one per backend, evaluated after the whole file is parsed

use crate::logos::emit::ctype::{format_char, print_arg, type_encoding};
use crate::logos::model::{Class, Method, Model};
use crate::logos::text::{
    deepest_paren_group, is_space, matched_parenthesis_set, skip_space, smart_split,
};

type G = Result<String, String>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    MobileSubstrate,
    Libhooker,
    Internal,
}

impl Backend {
    pub fn from_name(name: &str) -> Option<Backend> {
        match name {
            "MobileSubstrate" => Some(Backend::MobileSubstrate),
            "libhooker" => Some(Backend::Libhooker),
            "internal" => Some(Backend::Internal),
            _ => None,
        }
    }
}

pub fn sigil(parts: &[&str]) -> String {
    format!("_logos_{}", parts.join("$"))
}

const BASE_STATIC_DECLARATIONS: &str = "#if defined(__clang__)
#if __has_feature(objc_arc)
#define _LOGOS_SELF_TYPE_NORMAL __unsafe_unretained
#define _LOGOS_SELF_TYPE_INIT __attribute__((ns_consumed))
#define _LOGOS_SELF_CONST const
#define _LOGOS_RETURN_RETAINED __attribute__((ns_returns_retained))
#else
#define _LOGOS_SELF_TYPE_NORMAL
#define _LOGOS_SELF_TYPE_INIT
#define _LOGOS_SELF_CONST
#define _LOGOS_RETURN_RETAINED
#endif
#else
#define _LOGOS_SELF_TYPE_NORMAL
#define _LOGOS_SELF_TYPE_INIT
#define _LOGOS_SELF_CONST
#define _LOGOS_RETURN_RETAINED
#endif
";

const INTERNAL_REGISTER_HOOK: [&str; 22] = [
    "(Class _class, SEL _cmd, IMP _new, IMP *_old) {",
    "unsigned int _count, _i;",
    "Class _searchedClass = _class;",
    "Method *_methods;",
    "while (_searchedClass) {",
    "_methods = class_copyMethodList(_searchedClass, &_count);",
    "for (_i = 0; _i < _count; _i++) {",
    "if (method_getName(_methods[_i]) == _cmd) {",
    "if (_class == _searchedClass) {",
    "*_old = method_setImplementation(_methods[_i], _new);",
    "} else {",
    "*_old = method_getImplementation(_methods[_i]);",
    "class_addMethod(_class, _cmd, _new, method_getTypeEncoding(_methods[_i]));",
    "}",
    "free(_methods);",
    "return;",
    "}",
    "}",
    "free(_methods);",
    "_searchedClass = class_getSuperclass(_searchedClass);",
    "}",
    "}",
];

/// `<header>` or `"header"` after `#import` or `#include`
fn has_include(line: &str, header: &str) -> bool {
    let b = line.as_bytes();
    let mut from = 0;
    while let Some(off) = line[from..].find('#') {
        let hash = from + off;
        let i = skip_space(b, hash + 1);
        let rest = &line[i..];
        for word in ["import", "include"] {
            if let Some(r) = rest.strip_prefix(word) {
                let rb = r.as_bytes();
                let j = skip_space(rb, 0);
                if j < rb.len() && (rb[j] == b'<' || rb[j] == b'"') {
                    if let Some(t) = r[j + 1..].strip_prefix(header) {
                        if t.starts_with('>') || t.starts_with('"') {
                            return true;
                        }
                    }
                }
            }
        }
        from = hash + 1;
    }
    false
}

fn has_libhooker_include(line: &str) -> bool {
    for marker in ["#", "@"] {
        let mut from = 0;
        while let Some(off) = line[from..].find(marker) {
            let at = from + off;
            let rest = &line[at + 1..];
            for word in ["import", "include"] {
                if let Some(r) = rest.strip_prefix(word) {
                    if let Some(r) = r.strip_prefix(' ') {
                        let r = r.strip_prefix('<').unwrap_or(r);
                        if r.starts_with("libhooker") {
                            return true;
                        }
                    }
                }
            }
            from = at + 1;
        }
    }
    false
}

pub struct Gen<'a> {
    pub m: &'a Model,
    pub b: Backend,
}

impl<'a> Gen<'a> {
    // ---- file level ----

    pub fn find_preamble(&self, lines: &[String]) -> bool {
        let base = lines.iter().any(|l| has_include(l, "logos/logos.h"));
        let own = match self.b {
            Backend::MobileSubstrate => lines.iter().any(|l| has_include(l, "substrate.h")),
            Backend::Libhooker => lines.iter().any(|l| has_libhooker_include(l)),
            Backend::Internal => lines.iter().any(|l| has_include(l, "objc/message.h")),
        };
        base && own
    }

    pub fn preamble(&self) -> String {
        match self.b {
            Backend::MobileSubstrate => "\n#include <substrate.h>".to_string(),
            Backend::Libhooker => "\n#import <libhooker/libblackjack.h>\n#import <objc/runtime.h>".to_string(),
            Backend::Internal => "\n#include <objc/message.h>".to_string(),
        }
    }

    pub fn static_declarations(&self) -> String {
        let mut parts: Vec<String> = vec![BASE_STATIC_DECLARATIONS.to_string()];
        match self.b {
            Backend::MobileSubstrate => {
                parts.push("__asm__(\".linker_option \\\"-framework\\\", \\\"CydiaSubstrate\\\"\");".to_string());
                parts.push(String::new());
            }
            Backend::Libhooker => {
                parts.push("__asm__(\".linker_option \\\"-lhooker\\\"\");".to_string());
                parts.push("__asm__(\".linker_option \\\"-lblackjack\\\"\");".to_string());
                parts.push(String::new());
            }
            Backend::Internal => {
                parts.push(format!(
                    "__attribute__((unused)) static void {}{}",
                    sigil(&["register_hook"]),
                    INTERNAL_REGISTER_HOOK[0]
                ));
                for l in &INTERNAL_REGISTER_HOOK[1..] {
                    parts.push((*l).to_string());
                }
            }
        }
        parts.join("\n")
    }

    pub fn class_list(&self, names: &[String]) -> String {
        names.iter().map(|n| format!("@class {}; ", n)).collect()
    }

    pub fn class_reference(&self, class: &str, scope: u8) -> String {
        let tag = if scope == b'+' { "static_metaclass_lookup" } else { "static_class_lookup" };
        format!("{}()", sigil(&[tag, class]))
    }

    pub fn static_class_declarations(&self) -> String {
        let s = &self.m.static_classes;
        let mut out = String::new();
        for c in &s.used_meta_classes {
            out.push_str(&self.static_lookup(c, b'+'));
        }
        for c in &s.used_classes {
            out.push_str(&self.static_lookup(c, b'-'));
        }
        out
    }

    fn static_lookup(&self, class: &str, scope: u8) -> String {
        let tag = if scope == b'+' { "static_metaclass_lookup" } else { "static_class_lookup" };
        let lookup = if scope == b'+' { "objc_getMetaClass" } else { "objc_getClass" };
        format!(
            "static __inline__ __attribute__((always_inline)) __attribute__((unused)) Class {}(void) {{ static Class _klass; if(!_klass) {{ _klass = {}(\"{}\"); }} return _klass; }}",
            sigil(&[tag, class]),
            lookup,
            self.m.static_classes.real_name(class)
        )
    }

    // ---- groups ----

    pub fn group_declarations(&self, g: usize) -> G {
        let mut out = String::new();
        for &c in &self.m.groups[g].classes {
            if self.m.classes[c].init_required() {
                out.push_str(&self.class_declarations(c)?);
            }
        }
        Ok(out)
    }

    pub fn group_initializers(&self, g: usize) -> G {
        let group = &self.m.groups[g];
        let mut out = String::from("{");
        for &c in &group.classes {
            if self.m.classes[c].init_required() {
                out.push_str(&self.class_initializers(c)?);
            }
        }
        if self.b == Backend::Libhooker {
            let structs: Vec<String> = group.functions.iter().map(|&f| self.function_initializers(f)).collect();
            if !structs.is_empty() {
                out.push_str(&format!(
                    "LHHookFunctions((struct LHFunctionHook []){{{}}}, {});",
                    structs.join(","),
                    group.functions.len()
                ));
            }
        } else {
            for &f in &group.functions {
                out.push_str(&self.function_initializers(f));
            }
        }
        out.push('}');
        Ok(out)
    }

    // ---- classes ----

    fn class_var(&self, c: usize) -> String {
        let cl = &self.m.classes[c];
        sigil(&["class", self.m.group_name_of_class(c), &cl.name])
    }

    fn meta_var(&self, c: usize) -> String {
        let cl = &self.m.classes[c];
        sigil(&["metaclass", self.m.group_name_of_class(c), &cl.name])
    }

    fn super_var(&self, c: usize) -> String {
        let cl = &self.m.classes[c];
        sigil(&["superclass", self.m.group_name_of_class(c), &cl.name])
    }

    fn super_meta_var(&self, c: usize) -> String {
        let cl = &self.m.classes[c];
        sigil(&["supermetaclass", self.m.group_name_of_class(c), &cl.name])
    }

    fn init_expression(&self, c: usize) -> String {
        let cl = &self.m.classes[c];
        if let Some(sub) = &cl.subclass {
            let alloc = format!(
                "objc_allocateClassPair({}, \"{}\", 0)",
                self.class_var(sub.superclass),
                cl.name
            );
            return match self.b {
                Backend::Internal => alloc,
                _ => format!("{}; objc_registerClassPair({})", alloc, self.class_var(c)),
            };
        }
        match &cl.expression {
            Some(e) if !e.is_empty() => e.clone(),
            _ => format!("objc_getClass(\"{}\")", cl.name),
        }
    }

    fn meta_init_expression(&self, c: usize) -> String {
        match &self.m.classes[c].meta_expression {
            Some(e) if !e.is_empty() => e.clone(),
            _ => format!("object_getClass({})", self.class_var(c)),
        }
    }

    fn class_declarations(&self, c: usize) -> G {
        let cl = &self.m.classes[c];
        let mut out = String::new();
        if self.b == Backend::Internal {
            if cl.has_instance_hooks {
                out.push_str(&format!("static Class {}; ", self.super_var(c)));
            }
            if cl.has_meta_hooks {
                out.push_str(&format!("static Class {}; ", self.super_meta_var(c)));
            }
        }
        for &mi in &cl.methods {
            out.push_str(&self.method_declarations(mi)?);
        }
        Ok(out)
    }

    fn class_body_initializers(&self, cl: &Class) -> G {
        let mut out = String::new();
        for &p in &cl.properties {
            out.push_str(&self.property_initializers(p));
        }
        for &mi in &cl.methods {
            out.push_str(&self.method_initializers(mi));
        }
        Ok(out)
    }

    fn class_initializers(&self, c: usize) -> G {
        let cl = &self.m.classes[c];
        let mut out = String::new();
        if cl.subclass.is_some() {
            out.push_str("{ ");
        }
        if cl.required || cl.overridden || cl.has_instance_hooks || cl.has_meta_hooks {
            out.push_str(&format!("Class {} = {}; ", self.class_var(c), self.init_expression(c)));
        }
        if cl.has_meta_hooks {
            out.push_str(&format!("Class {} = {}; ", self.meta_var(c), self.meta_init_expression(c)));
        }
        if self.b == Backend::Internal {
            if cl.has_instance_hooks {
                out.push_str(&format!("{} = class_getSuperclass({}); ", self.super_var(c), self.class_var(c)));
            }
            if cl.has_meta_hooks {
                out.push_str(&format!(
                    "{} = class_getSuperclass({}); ",
                    self.super_meta_var(c),
                    self.meta_var(c)
                ));
            }
        }
        out.push_str(&self.class_body_initializers(cl)?);
        if let Some(sub) = &cl.subclass {
            out.push(' ');
            if self.b == Backend::Internal {
                out.push_str(&format!("objc_registerClassPair({}); ", self.class_var(c)));
            }
            for p in &sub.protocols {
                out.push_str(&format!(
                    "class_addProtocol({}, objc_getProtocol(\"{}\")); ",
                    self.class_var(c),
                    p
                ));
            }
            out.push('}');
        }
        Ok(out)
    }

    // ---- methods ----

    fn method_symbol(&self, tag: &str, m: &Method) -> String {
        let cl = &self.m.classes[m.class];
        let scoped = if m.scope == b'+' { format!("meta_{}", tag) } else { tag.to_string() };
        sigil(&[&scoped, self.m.group_name_of_class(m.class), &cl.name, &m.new_selector()])
    }

    pub fn method_original_name(&self, mi: usize) -> String {
        self.method_symbol("orig", &self.m.methods[mi])
    }

    fn method_new_name(&self, m: &Method) -> String {
        self.method_symbol("method", m)
    }

    fn selector_ref(&self, selector: &str) -> String {
        if selector == "dealloc" {
            format!("sel_registerName(\"{}\")", selector)
        } else {
            format!("@selector({})", selector)
        }
    }

    fn self_type(&self, m: &Method) -> String {
        if m.scope == b'+' {
            return "_LOGOS_SELF_TYPE_NORMAL Class _LOGOS_SELF_CONST".to_string();
        }
        let ty = self.m.classes[m.class].type_name();
        if m.family() == "init" {
            return format!("_LOGOS_SELF_TYPE_INIT {}", ty);
        }
        format!("_LOGOS_SELF_TYPE_NORMAL {} _LOGOS_SELF_CONST", ty)
    }

    fn return_type(&self, m: &Method) -> String {
        if !m.family().is_empty() || m.ret == "instancetype" {
            return self.m.classes[m.class].type_name();
        }
        m.ret.clone()
    }

    fn function_attributes(&self, m: &Method) -> &'static str {
        if m.family().is_empty() {
            ""
        } else {
            " _LOGOS_RETURN_RETAINED"
        }
    }

    fn orig_pointer_declaration(&self, mi: usize) -> Result<Option<String>, String> {
        let m = &self.m.methods[mi];
        if m.is_new {
            return Ok(None);
        }
        let mut name = format!("(*{})({}, SEL", self.method_original_name(mi), self.self_type(m));
        let types = m.arg_types.join(", ");
        if !types.is_empty() {
            name.push_str(", ");
            name.push_str(&types);
        }
        if m.variadic && self.b == Backend::MobileSubstrate {
            name.push_str(", ...");
        }
        name.push(')');
        let decl = declaration_for_type_with_name(&self.return_type(m), &name)?;
        Ok(Some(format!("static {}{}", decl, self.function_attributes(m))))
    }

    fn method_prototype(&self, mi: usize, with_names: bool) -> G {
        let m = &self.m.methods[mi];
        let mut arglist = String::new();
        if with_names {
            for i in 0..m.num_args() {
                arglist.push_str(", ");
                arglist.push_str(&declaration_for_type_with_name(&m.arg_types[i], &m.arg_names[i])?);
            }
        } else {
            let types = m.arg_types.join(", ");
            if !types.is_empty() {
                arglist = format!(", {}", types);
            }
        }
        let mut name = format!(
            "{}({}{}, SEL{}{}",
            self.method_new_name(m),
            self.self_type(m),
            if with_names { " __unused self" } else { "" },
            if with_names { " __unused _cmd" } else { "" },
            arglist
        );
        if m.variadic && self.b == Backend::MobileSubstrate {
            name.push_str(", ...");
        }
        name.push(')');
        let decl = declaration_for_type_with_name(&self.return_type(m), &name)?;
        Ok(format!("static {}{}", decl, self.function_attributes(m)))
    }

    pub fn method_definition(&self, mi: usize) -> G {
        let m = &self.m.methods[mi];
        if self.b != Backend::Internal {
            return self.method_prototype(mi, true);
        }
        let mut arglist = String::new();
        for i in 0..m.num_args() {
            arglist.push_str(", ");
            arglist.push_str(&declaration_for_type_with_name(&m.arg_types[i], &m.arg_names[i])?);
        }
        let mut params = format!("({} __unused self, SEL __unused _cmd{}", self.self_type(m), arglist);
        if m.variadic {
            params.push_str(", ...");
        }
        params.push(')');
        let decl = declaration_for_type_with_name(&self.return_type(m), &format!("{}{}", self.method_new_name(m), params))?;
        Ok(format!("static {}{}", decl, self.function_attributes(m)))
    }

    pub fn method_original_call(&self, mi: usize, custom: Option<&str>) -> String {
        let m = &self.m.methods[mi];
        if m.is_new && self.b != Backend::Internal {
            return String::new();
        }
        let orig = self.method_original_name(mi);
        if m.is_new {
            return orig;
        }
        let mut build = format!("{}(self, _cmd", orig);
        match custom {
            Some(c) if !c.is_empty() => {
                build.push_str(", ");
                build.push_str(c);
            }
            _ if m.num_args() > 0 => {
                build.push_str(", ");
                build.push_str(&m.arg_names.join(", "));
            }
            _ => {}
        }
        build.push(')');
        build
    }

    fn method_declarations(&self, mi: usize) -> G {
        let m = &self.m.methods[mi];
        if self.b == Backend::Internal {
            if m.is_new {
                return Ok(String::new());
            }
            let mut name = format!(
                "{}(*{})({}, SEL",
                self.function_attributes(m),
                self.method_original_name(mi),
                self.self_type(m)
            );
            let types = m.arg_types.join(", ");
            if !types.is_empty() {
                name.push_str(", ");
                name.push_str(&types);
            }
            if m.variadic {
                name.push_str(", ...");
            }
            name.push(')');
            let decl = declaration_for_type_with_name(&self.return_type(m), &name)?;
            return Ok(format!("static {};", decl));
        }
        let mut out = String::new();
        if let Some(orig) = self.orig_pointer_declaration(mi)? {
            out.push_str(&orig);
            out.push_str("; ");
        }
        out.push_str(&self.method_prototype(mi, false)?);
        out.push_str("; ");
        Ok(out)
    }

    fn new_method_encoding(&self, m: &Method) -> String {
        if !m.type_encoding.is_empty() {
            return format!("const char *_typeEncoding = \"{}\"; ", m.type_encoding);
        }
        let mut r = String::from("char _typeEncoding[1024]; unsigned int i = 0; ");
        let types = std::iter::once(m.ret.as_str())
            .chain(["id", "SEL"])
            .chain(m.arg_types.iter().map(|s| s.as_str()));
        for t in types {
            match type_encoding(t) {
                Some(enc) => {
                    for (i, ch) in enc.chars().enumerate() {
                        if i > 0 {
                            r.push_str(&format!("_typeEncoding[i + {}] = '{}'; ", i, ch));
                        } else {
                            r.push_str(&format!("_typeEncoding[i] = '{}'; ", ch));
                        }
                    }
                    r.push_str(&format!("i += {}; ", enc.chars().count()));
                }
                None => r.push_str(&format!(
                    "memcpy(_typeEncoding + i, @encode({0}), strlen(@encode({0}))); i += strlen(@encode({0})); ",
                    t
                )),
            }
        }
        r.push_str("_typeEncoding[i] = '\\0'; ");
        r
    }

    fn method_initializers(&self, mi: usize) -> String {
        let m = &self.m.methods[mi];
        let classvar = if m.scope == b'+' { self.meta_var(m.class) } else { self.class_var(m.class) };
        let sel = self.selector_ref(&m.selector());
        if m.is_new {
            return format!(
                "{{ {}class_addMethod({}, {}, (IMP)&{}, _typeEncoding); }}",
                self.new_method_encoding(m),
                classvar,
                sel,
                self.method_new_name(m)
            );
        }
        let orig = self.method_original_name(mi);
        let new = self.method_new_name(m);
        match self.b {
            Backend::MobileSubstrate => format!(
                "{{ MSHookMessageEx({}, {}, (IMP)&{}, (IMP*)&{});}}",
                classvar, sel, new, orig
            ),
            Backend::Libhooker => format!(
                "LBHookMessage({}, {}, (void *)&{}, (void *)&{});",
                classvar, sel, new, orig
            ),
            Backend::Internal => format!(
                "{{ {}({}, {}, (IMP)&{}, (IMP *)&{});}}",
                sigil(&["register_hook"]),
                classvar,
                sel,
                new,
                orig
            ),
        }
    }

    pub fn build_log_call(&self, mi: usize, args: Option<&str>) -> G {
        let m = &self.m.methods[mi];
        let cl = &self.m.classes[m.class];
        let mut build = format!("NSLog(@\"{}[<{}: %p>", m.scope as char, cl.name);
        let mut argnamelist = String::new();
        if m.num_args() > 0 {
            for i in 0..m.num_args() {
                build.push_str(&format!(
                    " {}:{}",
                    m.selector_parts.get(i).map(|s| s.as_str()).unwrap_or(""),
                    format_char(&m.arg_types[i]).0
                ));
            }
            let existing: Vec<String> = (0..m.num_args())
                .filter_map(|i| print_arg(&m.arg_types[i], &m.arg_names[i]))
                .collect();
            if !existing.is_empty() {
                argnamelist = format!(", {}", existing.join(", "));
            }
        } else {
            build.push(' ');
            build.push_str(&m.selector());
        }

        let mut extra_formats: Vec<String> = Vec::new();
        let mut extra_args: Vec<String> = Vec::new();
        for piece in smart_split(b',', args.unwrap_or(""), 0)? {
            let pairs = matched_parenthesis_set(&piece, true, false)?;
            let ty = match pairs.first() {
                Some(&(o, c)) => piece[o..c - 1].to_string(),
                None => "id".to_string(),
            };
            extra_formats.push(format_char(&ty).0);
            if let Some(n) = print_arg(&ty, &format!("({})", piece)) {
                extra_args.push(n);
            }
        }

        build.push(']');
        if !extra_formats.is_empty() {
            build.push_str(": ");
            build.push_str(&extra_formats.join(", "));
        }
        build.push_str("\", self");
        build.push_str(&argnamelist);
        if !extra_args.is_empty() {
            build.push_str(", ");
            build.push_str(&extra_args.join(", "));
        }
        build.push(')');
        Ok(build)
    }

    // ---- functions ----

    fn function_symbol(&self, tag: &str, f: usize) -> String {
        let func = &self.m.functions[f];
        sigil(&[tag, &self.m.groups[func.group].name, &func.name])
    }

    pub fn function_original_name(&self, f: usize) -> String {
        self.function_symbol("orig", f)
    }

    fn function_new_name(&self, f: usize) -> String {
        self.function_symbol("function", f)
    }

    fn function_init_expression(&self, f: usize) -> String {
        match &self.m.functions[f].expression {
            Some(e) if !e.is_empty() => e.clone(),
            _ => format!("(void *){}", self.m.functions[f].name),
        }
    }

    pub fn function_declaration(&self, f: usize) -> String {
        let func = &self.m.functions[f];
        let args = func.args.join(", ");
        format!(
            "__unused static {ret} (*{orig})({args}); __unused static {ret} {new}({args})",
            ret = func.retval,
            orig = self.function_original_name(f),
            new = self.function_new_name(f),
            args = args
        )
    }

    pub fn function_original_call(&self, f: usize, args: Option<&str>) -> String {
        let func = &self.m.functions[f];
        let mut out = format!("{}(", self.function_original_name(f));
        let has_args = args.map_or(false, |a| !a.is_empty() && a != "0");
        if !has_args && func.args.first().map_or(true, |a| a != "void") {
            let names: Vec<String> = func.args.iter().map(|a| argument_name(a)).collect();
            out.push_str(&names.join(", "));
        } else if let Some(a) = args {
            out.push_str(a);
        }
        out.push(')');
        out
    }

    fn function_initializers(&self, f: usize) -> String {
        match self.b {
            Backend::MobileSubstrate => {
                let sym = self.function_symbol("symbol", f);
                format!(
                    "void * {sym} = {expr}; MSHookFunction((void *){sym}, (void *)&{new}, (void **)&{orig});",
                    sym = sym,
                    expr = self.function_init_expression(f),
                    new = self.function_new_name(f),
                    orig = self.function_original_name(f)
                )
            }
            Backend::Libhooker => format!(
                "{{{}, (void *)&{}, (void **)&{}}}",
                self.function_init_expression(f),
                self.function_new_name(f),
                self.function_original_name(f)
            ),
            Backend::Internal => " /* internal::Function does not implement initializers */;".to_string(),
        }
    }

    // ---- properties ----

    fn property_getter_name(&self, p: usize) -> String {
        let prop = &self.m.properties[p];
        sigil(&["property", &self.m.groups[prop.group].name, &self.m.classes[prop.class].name, &prop.getter])
    }

    fn property_setter_name(&self, p: usize) -> String {
        let prop = &self.m.properties[p];
        sigil(&["property", &self.m.groups[prop.group].name, &self.m.classes[prop.class].name, &prop.setter])
    }

    pub fn property_definition(&self, p: usize) -> String {
        let prop = &self.m.properties[p];
        if prop.readonly {
            return String::new();
        }
        let ty = &prop.type_name;
        let class = &self.m.classes[prop.class].name;
        let getter = self.property_getter_name(p);
        let setter = self.property_setter_name(p);
        let wrap = !(prop.policy.contains("RETAIN") || prop.policy.contains("COPY"));

        let mut g = format!("__attribute__((used)) static {} {}({} * __unused self, SEL __unused _cmd) ", ty, getter, class);
        if wrap {
            g.push_str(&format!(
                "{{ NSValue * value = objc_getAssociatedObject(self, (void *){}); {} rawValue; [value getValue:&rawValue]; return rawValue; }}",
                getter, ty
            ));
        } else {
            g.push_str(&format!("{{ return ({})objc_getAssociatedObject(self, (void *){}); }}", ty, getter));
        }

        let mut s = format!(
            "__attribute__((used)) static void {}({} * __unused self, SEL __unused _cmd, {} rawValue) ",
            setter, class, ty
        );
        if wrap {
            s.push_str(&format!(
                "{{ NSValue * value = [NSValue valueWithBytes:&rawValue objCType:@encode({})]; objc_setAssociatedObject(self, (void *){}, value, OBJC_ASSOCIATION_RETAIN_NONATOMIC); }}",
                ty, getter
            ));
        } else {
            s.push_str(&format!(
                "{{ objc_setAssociatedObject(self, (void *){}, rawValue, {}); }}",
                getter, prop.policy
            ));
        }
        format!("{}; {}", g, s)
    }

    fn property_initializers(&self, p: usize) -> String {
        let prop = &self.m.properties[p];
        let class_var = sigil(&["class", &self.m.groups[prop.group].name, &self.m.classes[prop.class].name]);
        let ty = &prop.type_name;

        // the class name is the first run of the type, the pattern needs one more character behind it
        let mut run_end = ty.bytes().position(|b| b == b' ' || b == b'*' || b == b'<').unwrap_or(ty.len());
        if run_end == ty.len() {
            run_end = run_end.saturating_sub(1);
        }
        let class_name = &ty[..run_end];

        let attr = |name: &str, value: &str| -> String {
            format!(" _attributes[attrc++] = (objc_property_attribute_t) {{ \"{}\", {} }};", name, value)
        };

        let mut b = String::from("{ objc_property_attribute_t _attributes[16]; unsigned int attrc = 0;");
        if prop.retain {
            b.push_str(&attr("T", &format!("\"@\\\"{}\\\"\"", class_name)));
            b.push_str(&attr("&", "\"\""));
        } else {
            b.push_str(&attr("T", &format!("@encode({})", ty)));
        }
        if prop.readonly {
            b.push_str(&attr("R", "\"\""));
        }
        if prop.copy {
            b.push_str(&attr("C", "\"\""));
        }
        if prop.nonatomic {
            b.push_str(&attr("N", "\"\""));
        }
        b.push_str(&format!(" class_addProperty({}, \"{}\", _attributes, attrc);", class_var, prop.name));
        if !prop.readonly {
            b.push_str(" char _typeEncoding[1024];");
            b.push_str(&format!(" snprintf(_typeEncoding, 1024, \"%s@:\", @encode({}));", ty));
            b.push_str(&format!(
                " class_addMethod({}, @selector({}), (IMP)&{}, _typeEncoding);",
                class_var,
                prop.getter,
                self.property_getter_name(p)
            ));
            b.push_str(&format!(" snprintf(_typeEncoding, 1024, \"v@:%s\", @encode({}));", ty));
            b.push_str(&format!(
                " class_addMethod({}, @selector({}:), (IMP)&{}, _typeEncoding);",
                class_var,
                prop.setter,
                self.property_setter_name(p)
            ));
        }
        b.push_str(" } ");
        b
    }
}

/// puts `name` into a function pointer or block type, `void (^)(int)` becomes `void (^name)(int)`
pub fn declaration_for_type_with_name(ty: &str, name: &str) -> G {
    let b = ty.as_bytes();
    let has_pointer_paren = b.iter().enumerate().any(|(i, &c)| {
        c == b'(' && {
            let j = skip_space(b, i + 1);
            j < b.len() && (b[j] == b'*' || b[j] == b'^')
        }
    });
    if !has_pointer_paren {
        return Ok(format!("{} {}", ty, name));
    }
    match deepest_paren_group(ty)? {
        Some((_, close)) => {
            let at = close - 1;
            Ok(format!("{}{}{}", &ty[..at], name, &ty[at..]))
        }
        None => Ok(format!("{} {}", ty, name)),
    }
}

fn split_root(s: &str, delim: u8) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut token = String::new();
    let (mut parens, mut brackets, mut braces, mut chevrons) = (0i32, 0i32, 0i32, 0i32);
    let mut pc: Option<u8> = None;
    let mut input = s.as_bytes().to_vec();
    input.push(delim);
    for &c in &input {
        if c == delim && parens == 0 && brackets == 0 && braces == 0 && chevrons == 0 {
            args.push(std::mem::take(&mut token));
            pc = Some(c);
            continue;
        }
        match c {
            b'(' => parens += 1,
            b')' => parens -= 1,
            b'[' => brackets += 1,
            b']' => brackets -= 1,
            b'{' => braces += 1,
            b'}' => braces -= 1,
            b'<' => chevrons += 1,
            b'>' => chevrons -= 1,
            _ => {}
        }
        if (pc == Some(delim) && c == b' ') || (pc == Some(b' ') && c == b' ') {
            continue;
        }
        token.push(c as char);
        pc = Some(c);
    }
    args
}

/// the parameter name of a c declaration like `const char *path` or `void (*cb)(int)`
pub fn argument_name(arg: &str) -> String {
    let parts = split_root(arg, b' ');
    let last = parts.last().cloned().unwrap_or_default();
    let mut ret = match last.contains('(') && has_pointer_group(&last) {
        true => match deepest_paren_group(&last) {
            Ok(Some((o, c))) => last[o..c - 1].to_string(),
            _ => last,
        },
        false => last,
    };
    ret = ret.replace('[', " [");
    let ret = ret.trim_start_matches(|c| c == ' ' || c == '*').to_string();
    split_root(&ret, b' ').into_iter().next().unwrap_or_default()
}

fn has_pointer_group(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        c == b'(' && {
            let mut j = i + 1;
            while j < b.len() && is_space(b[j]) {
                j += 1;
            }
            j < b.len() && (b[j] == b'*' || b[j] == b'^')
        }
    })
}
