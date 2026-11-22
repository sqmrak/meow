use crate::config::{err, Config, Error, LinkKind, Result, Scope};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Node {
    pub label: String,
    pub argv: Vec<String>,
    pub post: Vec<Vec<String>>,
    pub outputs: Vec<String>,
    pub depfile: Option<String>,
    pub deps: Vec<usize>,
    pub env: Vec<(String, String)>,
    // globs of files the commands read without naming them
    pub input_globs: Vec<String>,
    // outputs stay when the node is stale, the commands only rebuild what is missing
    pub keep: bool,
    pub bootstrap: bool,
    // created after the commands succeed
    pub stamp: Option<String>,
    // directory of the commands, relative to the root
    pub cwd: Option<String>,
}

#[derive(Debug)]
pub struct Plan {
    pub root: PathBuf,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Default)]
struct Built {
    objs: Vec<String>,
    obj_nodes: Vec<usize>,
    last: Vec<usize>,
    out: Option<String>,
}

struct Builder<'a> {
    cfg: &'a Config,
    root: &'a Path,
    nodes: Vec<Node>,
    memo: BTreeMap<(String, String), Built>,
    named: BTreeMap<String, usize>,
    stack: Vec<String>,
}

pub fn build(
    cfg: &Config,
    root: &Path,
    requests: &[String],
    only_slices: &[String],
) -> Result<Plan> {
    for s in only_slices {
        if !cfg.slice.contains_key(s) {
            return err(format!("unknown slice {}", s));
        }
    }
    let mut b = Builder {
        cfg,
        root,
        nodes: Vec::new(),
        memo: BTreeMap::new(),
        named: BTreeMap::new(),
        stack: Vec::new(),
    };
    for name in requests {
        if let Some(t) = cfg.target.get(name) {
            let slices: Vec<String> = t
                .slices
                .iter()
                .filter(|s| only_slices.is_empty() || only_slices.contains(s))
                .cloned()
                .collect();
            if slices.is_empty() {
                return err(format!("{} has none of the requested slices", name));
            }
            for s in &slices {
                b.target(name, s)?;
            }
        } else if cfg.fat.contains_key(name) || cfg.step.contains_key(name) || cfg.deb.contains_key(name) || cfg.ipa.contains_key(name) || cfg.verify.contains_key(name) {
            if !only_slices.is_empty() {
                return err(format!("{} spans every slice, --slice cannot apply", name));
            }
            b.need(name, None)?;
        } else {
            return err(format!("no target, fat output, step, deb or ipa named {}", name));
        }
    }
    Ok(Plan {
        root: root.to_path_buf(),
        nodes: b.nodes,
    })
}

// object paths mirror the source tree, with .. turned into _ so a source outside
// the project root cannot escape the build directory
fn obj_path(slice: &str, target: &str, src: &str) -> String {
    let mut rel = String::new();
    for part in src.trim_start_matches('/').split('/') {
        if !rel.is_empty() {
            rel.push('/');
        }
        rel.push_str(if part == ".." { "_" } else { part });
    }
    format!("build/obj/{}/{}/{}.o", slice, target, rel)
}

fn display(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() || a.contains(|c: char| c.is_whitespace() || "'\"$\\".contains(c)) {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn render(argv: &[String]) -> String {
    display(argv)
}

impl<'a> Builder<'a> {
    fn push(&mut self, n: Node) -> usize {
        self.nodes.push(n);
        self.nodes.len() - 1
    }

    fn sources(&self, patterns: &[String], exclude: &[String], scope: &Scope, target: &str) -> Result<Vec<String>> {
        let mut out = self.sources_all(patterns, scope, target)?;
        if !exclude.is_empty() {
            let pats = exclude
                .iter()
                .map(|e| scope.expand(e).and_then(|e| glob::Pattern::new(&e).map_err(|x| Error(format!("target {}: exclude {}: {}", target, e, x)))))
                .collect::<Result<Vec<_>>>()?;
            out.retain(|f| !pats.iter().any(|p| p.matches(f)));
            if out.is_empty() {
                return err(format!("target {}: exclude removed every source", target));
            }
        }
        Ok(out)
    }

    fn sources_all(&self, patterns: &[String], scope: &Scope, target: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for p in patterns {
            let p = scope.expand(p)?;
            if !p.contains(['*', '?', '[']) {
                if !self.root.join(&p).is_file() {
                    return err(format!("target {}: source {} not found", target, p));
                }
                out.push(p);
                continue;
            }
            let full = self.root.join(&p);
            let mut hits = Vec::new();
            let paths = glob::glob(&full.to_string_lossy())
                .map_err(|e| Error(format!("target {}: pattern {}: {}", target, p, e)))?;
            for hit in paths {
                let hit = hit.map_err(|e| Error(format!("target {}: {}", target, e)))?;
                let rel = hit.strip_prefix(self.root).unwrap_or(&hit);
                hits.push(rel.to_string_lossy().into_owned());
            }
            if hits.is_empty() {
                return err(format!("target {}: pattern {} matches nothing", target, p));
            }
            hits.sort();
            out.extend(hits);
        }
        Ok(out)
    }

    fn target(&mut self, name: &str, slice: &str) -> Result<Built> {
        let key = (name.to_string(), slice.to_string());
        if let Some(b) = self.memo.get(&key) {
            return Ok(b.clone());
        }
        if self.stack.iter().any(|s| s == name) {
            return err(format!("target cycle through {}", name));
        }
        self.stack.push(name.to_string());
        let r = self.target_inner(name, slice);
        self.stack.pop();
        let built = r?;
        self.memo.insert(key, built.clone());
        Ok(built)
    }

    fn target_inner(&mut self, name: &str, slice: &str) -> Result<Built> {
        let cfg = self.cfg;
        let t = &cfg.target[name];
        let sl = &cfg.slice[slice];
        let empty_ts = Default::default();
        let ts = t.per_slice.get(slice).unwrap_or(&empty_ts);
        let mut scope = Scope::new(vec![&ts.vars, &sl.vars, &cfg.vars]).with_root(self.root);
        scope.set("slice", slice);
        scope.set("target", name);

        let cc = scope.expand(&sl.cc)?;
        let flags = scope.expand_list(&sl.flags)?;
        let tflags = scope.expand_list(&t.cflags)?;
        let sflags = scope.expand_list(&ts.cflags)?;

        let mut gate: Vec<usize> = Vec::new();
        for dep in &t.needs {
            let dep = scope.expand(dep)?;
            gate.extend(self.need(&dep, Some(slice))?);
        }

        let mut built = Built::default();
        for g in &t.src {
            let gflags = scope.expand_list(&g.cflags)?;
            for src in self.sources(&g.files, &g.exclude, &scope, name)? {
                let obj = obj_path(slice, name, &src);
                let deps = gate.clone();
                let compile_src = src.clone();
                let dep = format!("{}.d", obj);
                let mut argv = vec![cc.clone()];
                argv.extend(flags.iter().cloned());
                argv.extend(tflags.iter().cloned());
                argv.extend(sflags.iter().cloned());
                argv.extend(gflags.iter().cloned());
                argv.extend(
                    ["-MMD", "-MF", dep.as_str(), "-c", compile_src.as_str(), "-o", obj.as_str()]
                        .iter()
                        .map(|s| s.to_string()),
                );
                let id = self.push(Node {
                    label: format!("cc {} {}", slice, src),
                    argv,
                    post: Vec::new(),
                    outputs: vec![obj.clone()],
                    depfile: Some(dep),
                    deps,
                    ..Default::default()
                });
                built.objs.push(obj);
                built.obj_nodes.push(id);
            }
        }
        for dep in &t.objects_from {
            let d = self.target(dep, slice)?;
            built.objs.extend(d.objs);
            built.obj_nodes.extend(d.obj_nodes);
        }
        // objects a step makes, the step is in needs so the link waits for it
        built.objs.extend(scope.expand_list(&t.objects)?);

        if t.link == LinkKind::None {
            built.last = built.obj_nodes.clone();
            return Ok(built);
        }
        if built.objs.is_empty() {
            return err(format!("target {}: nothing to link", name));
        }
        scope.set("out", "");
        let out = scope.expand(t.output.as_deref().unwrap_or_default())?;
        if out.is_empty() {
            return err(format!("target {}: output expands to nothing", name));
        }
        scope.set("out", &out);
        let mut post = t
            .post
            .iter()
            .map(|p| scope.expand_list(p))
            .collect::<Result<Vec<_>>>()?;
        post.extend(finishing(&scope, &out, t.strip, t.sign, t.entitlements.as_deref())?);

        let (verb, argv) = match t.link {
            LinkKind::Exe => {
                let mut argv = vec![cc.clone()];
                argv.extend(flags.iter().cloned());
                argv.extend(built.objs.iter().cloned());
                argv.push("-o".to_string());
                argv.push(out.clone());
                argv.extend(scope.expand_list(&t.ldflags)?);
                argv.extend(scope.expand_list(&ts.ldflags)?);
                ("ld", argv)
            }
            LinkKind::Archive => {
                if sl.ar.is_empty() {
                    return err(format!("slice {}: ar is not set, target {} needs it", slice, name));
                }
                let mut argv = vec![scope.expand(&sl.ar)?, "rcs".to_string(), out.clone()];
                argv.extend(built.objs.iter().cloned());
                ("ar", argv)
            }
            LinkKind::None => unreachable!(),
        };
        let id = self.push(Node {
            label: format!("{} {} {}", verb, slice, out),
            argv,
            post,
            outputs: vec![out.clone()],
            deps: built.obj_nodes.iter().copied().chain(gate).collect(),
            ..Default::default()
        });
        built.last = vec![id];
        built.out = Some(out);
        Ok(built)
    }

    // nodes that must finish before a dependent: a step or fat output is one node, a
    // target is its last nodes on the given slice or on every slice
    fn need(&mut self, name: &str, slice: Option<&str>) -> Result<Vec<usize>> {
        let cfg = self.cfg;
        if cfg.step.contains_key(name) {
            return Ok(vec![self.step(name)?]);
        }
        if cfg.fat.contains_key(name) {
            return Ok(vec![self.fat(name)?]);
        }
        if cfg.deb.contains_key(name) || cfg.ipa.contains_key(name) {
            return Ok(vec![self.package(name)?]);
        }
        if cfg.verify.contains_key(name) {
            return Ok(vec![self.verify(name)?]);
        }
        let Some(t) = cfg.target.get(name) else {
            return err(format!("unknown need {}", name));
        };
        let mut out = Vec::new();
        for s in &t.slices {
            if slice.map_or(true, |only| only == s) {
                out.extend(self.target(name, s)?.last);
            }
        }
        if out.is_empty() {
            return err(format!("need {} has no slice {}", name, slice.unwrap_or("")));
        }
        Ok(out)
    }

    fn step(&mut self, name: &str) -> Result<usize> {
        if let Some(&id) = self.named.get(name) {
            return Ok(id);
        }
        if self.stack.iter().any(|s| s == name) {
            return err(format!("dependency cycle through {}", name));
        }
        self.stack.push(name.to_string());
        let r = self.step_inner(name);
        self.stack.pop();
        let id = r?;
        self.named.insert(name.to_string(), id);
        Ok(id)
    }

    fn step_inner(&mut self, name: &str) -> Result<usize> {
        let cfg = self.cfg;
        let st = &cfg.step[name];
        let mut scope = Scope::new(vec![&cfg.vars]).with_root(self.root);
        scope.set("step", name);
        let mut deps = Vec::new();
        for n in &st.needs {
            let n = scope.expand(n)?;
            deps.extend(self.need(&n, None)?);
        }
        let mut run = st
            .run
            .iter()
            .map(|c| scope.expand_list(c))
            .collect::<Result<Vec<_>>>()?;
        if run.iter().any(|c| c.is_empty()) {
            return err(format!("step {}: a command expands to nothing", name));
        }
        let mut outputs = scope.expand_list(&st.outputs)?;
        let stamp = match &st.stamp {
            Some(p) => Some(scope.expand(p)?),
            None => None,
        };
        if let Some(p) = &stamp {
            outputs.push(p.clone());
        }
        let mut env = Vec::new();
        for (k, v) in &st.env {
            env.push((k.clone(), scope.expand(v)?));
        }
        let input_globs = scope.expand_list(&st.inputs)?;
        let depfile = match &st.depfile {
            Some(d) => Some(scope.expand(d)?),
            None => None,
        };
        let argv = run.remove(0);
        Ok(self.push(Node {
            label: format!("step {}", name),
            argv,
            depfile,
            post: run,
            outputs,
            deps,
            env,
            input_globs,
            keep: st.keep || st.bootstrap,
            bootstrap: st.bootstrap,
            stamp,
            cwd: match &st.cwd {
                Some(c) => Some(scope.expand(c)?),
                None => None,
            },
            ..Default::default()
        }))
    }

    fn fat(&mut self, name: &str) -> Result<usize> {
        if let Some(&id) = self.named.get(name) {
            return Ok(id);
        }
        let id = self.fat_inner(name)?;
        self.named.insert(name.to_string(), id);
        Ok(id)
    }

    fn fat_inner(&mut self, name: &str) -> Result<usize> {
        let cfg = self.cfg;
        let f = &cfg.fat[name];
        let t = &cfg.target[&f.from];
        let mut scope = Scope::new(vec![&cfg.vars]).with_root(self.root);
        scope.set("fat", name);
        let lipo = scope.expand(f.lipo.as_deref().unwrap_or("${MEOW_LIPO}"))?;
        let out = scope.expand(&f.output)?;
        scope.set("out", &out);
        let mut argv = vec![lipo, "-create".to_string()];
        let mut deps = Vec::new();
        for s in &t.slices {
            let b = self.target(&f.from, s)?;
            let Some(o) = b.out else {
                return err(format!("fat {}: {} has no output on {}", name, f.from, s));
            };
            argv.push(o);
            deps.extend(b.last);
        }
        argv.push("-output".to_string());
        argv.push(out.clone());
        let mut post = f
            .post
            .iter()
            .map(|p| scope.expand_list(p))
            .collect::<Result<Vec<_>>>()?;
        post.extend(finishing(&scope, &out, f.strip, f.sign, f.entitlements.as_deref())?);
        Ok(self.push(Node {
            label: format!("lipo {}", out),
            argv,
            post,
            outputs: vec![out],
            deps,
            ..Default::default()
        }))
    }

    // nodes that make the file at path: a fat output, a step output or a target output
    // on any of its slices. an expansion that fails cannot match, it belongs to
    // something the request does not need
    fn producers(&mut self, path: &str) -> Result<Vec<usize>> {
        let cfg = self.cfg;
        let mut out = Vec::new();
        for (n, f) in &cfg.fat {
            let scope = Scope::new(vec![&cfg.vars]).with_root(self.root);
            if scope.expand(&f.output).ok().as_deref() == Some(path) {
                out.push(self.fat(n)?);
            }
        }
        for (n, st) in &cfg.step {
            // a keep step writes files that live in the source tree, icons and the like. it runs
            // when asked for by name, never because a package happens to read its output
            if st.keep {
                continue;
            }
            let scope = Scope::new(vec![&cfg.vars]).with_root(self.root);
            if scope.expand_list(&st.outputs).is_ok_and(|o| o.iter().any(|x| x == path)) {
                out.push(self.step(n)?);
            }
        }
        for (n, t) in &cfg.target {
            let Some(output) = &t.output else { continue };
            for s in &t.slices {
                let empty = Default::default();
                let ts = t.per_slice.get(s).unwrap_or(&empty);
                let mut scope = Scope::new(vec![&ts.vars, &cfg.slice[s].vars, &cfg.vars]).with_root(self.root);
                scope.set("slice", s);
                scope.set("target", n);
                if scope.expand(output).ok().as_deref() == Some(path) {
                    out.extend(self.target(n, s)?.last);
                }
            }
        }
        Ok(out)
    }

    fn verify(&mut self, name: &str) -> Result<usize> {
        if let Some(&id) = self.named.get(name) {
            return Ok(id);
        }
        if self.stack.iter().any(|s| s == name) {
            return err(format!("dependency cycle through {}", name));
        }
        self.stack.push(name.to_string());
        let r = self.verify_inner(name);
        self.stack.pop();
        let id = r?;
        self.named.insert(name.to_string(), id);
        Ok(id)
    }

    fn verify_inner(&mut self, name: &str) -> Result<usize> {
        let cfg = self.cfg;
        let v = &cfg.verify[name];
        let scope = Scope::new(vec![&cfg.vars]).with_root(self.root);
        let file = scope.expand(&v.file)?;
        let exe = std::env::current_exe().map_err(|e| Error(format!("cannot find the meow binary: {}", e)))?;
        let mut argv = vec![exe.to_string_lossy().into_owned(), "verify-macho".to_string(), file.clone()];
        if !v.slices.is_empty() {
            argv.extend(["--slices".to_string(), v.slices.join(",")]);
        }
        if let Some(m) = &v.min_ios {
            argv.extend(["--min-ios".to_string(), m.clone()]);
        }
        if v.signed {
            argv.push("--signed".to_string());
        }
        for a in &v.allow_dylibs {
            argv.extend(["--allow-dylib".to_string(), a.clone()]);
        }
        for a in &v.forbid_dylibs {
            argv.extend(["--forbid-dylib".to_string(), a.clone()]);
        }
        if !v.forbid_symbols.is_empty() || v.forbid_symbols_file.is_some() || !v.require_symbols.is_empty() {
            argv.extend(["--nm".to_string(), scope.expand("${MEOW_NM}")?]);
            for sym in &v.require_symbols {
                argv.extend(["--require-symbol".to_string(), sym.clone()]);
            }
            for sym in &v.forbid_symbols {
                argv.extend(["--forbid-symbol".to_string(), sym.clone()]);
            }
            if let Some(f) = &v.forbid_symbols_file {
                argv.extend(["--forbid-symbols-file".to_string(), scope.expand(f)?]);
            }
        }
        let mut deps = self.producers(&file)?;
        for n in &v.needs {
            let n = scope.expand(n)?;
            deps.extend(self.need(&n, None)?);
        }
        deps.sort_unstable();
        deps.dedup();
        let stamp = format!("build/stamps/{}", name);
        Ok(self.push(Node { label: format!("verify {}", file), argv, outputs: vec![stamp.clone()], deps, stamp: Some(stamp), ..Default::default() }))
    }

    fn package(&mut self, name: &str) -> Result<usize> {
        if let Some(&id) = self.named.get(name) {
            return Ok(id);
        }
        if self.stack.iter().any(|s| s == name) {
            return err(format!("dependency cycle through {}", name));
        }
        self.stack.push(name.to_string());
        let r = self.package_inner(name);
        self.stack.pop();
        let id = r?;
        self.named.insert(name.to_string(), id);
        Ok(id)
    }

    // one node for a deb or an ipa, they differ in the control data only
    fn package_inner(&mut self, name: &str) -> Result<usize> {
        let cfg = self.cfg;
        let empty_fields = BTreeMap::new();
        let v = if let Some(d) = cfg.deb.get(name) {
            PackView {
                format: "deb",
                output: &d.output,
                prefix: &d.prefix,
                needs: &d.needs,
                file: &d.file,
                tree: &d.tree,
                data: &d.data,
                control: d.control.as_ref(),
                fields: &d.fields,
                scripts: &d.scripts,
                packages: d.packages.as_ref(),
                deb: Some(d),
            }
        } else {
            let i = &cfg.ipa[name];
            PackView {
                format: "ipa",
                output: &i.output,
                prefix: &i.prefix,
                needs: &i.needs,
                file: &i.file,
                tree: &i.tree,
                data: &i.data,
                control: None,
                fields: &empty_fields,
                scripts: &empty_fields,
                packages: None,
                deb: None,
            }
        };
        let mut scope = Scope::new(vec![&cfg.vars]).with_root(self.root);
        scope.set(v.format, name);
        let out = scope.expand(v.output)?;
        let prefix = scope.expand(v.prefix)?.trim_matches('/').to_string();
        let dest = |to: &str| -> Result<String> {
            let to = scope.expand(to)?;
            let to = to.trim_matches('/');
            Ok(match (prefix.is_empty(), to.is_empty()) {
                (_, true) => prefix.clone(),
                (true, false) => to.to_string(),
                (false, false) => format!("{}/{}", prefix, to),
            })
        };
        let exe = std::env::current_exe().map_err(|e| Error(format!("cannot find the meow binary: {}", e)))?;
        let mut argv = vec![
            exe.to_string_lossy().into_owned(),
            "pack".to_string(),
            "--format".to_string(),
            v.format.to_string(),
            "--out".to_string(),
            out.clone(),
        ];
        let mut deps = Vec::new();
        for n in v.needs {
            let n = scope.expand(n)?;
            deps.extend(self.need(&n, None)?);
        }
        match v.control {
            Some(c) => argv.extend(["--control".to_string(), scope.expand(c)?]),
            None => {
                for (k, val) in v.fields {
                    argv.extend(["--field".to_string(), format!("{}={}", k, scope.expand(val)?)]);
                }
            }
        }
        for (script, path) in v.scripts {
            argv.extend(["--script".to_string(), script.clone(), scope.expand(path)?]);
        }
        let mut input_globs = Vec::new();
        for f in v.file {
            let from = scope.expand(&f.from)?;
            deps.extend(self.producers(&from)?);
            argv.extend(["--file".to_string(), from, dest(&f.to)?, f.mode.clone().unwrap_or_else(|| "644".to_string())]);
            if f.replace {
                argv.push("--replace".to_string());
            }
        }
        for t in v.tree {
            let from = scope.expand(&t.from)?;
            deps.extend(self.producers(&from)?);
            input_globs.push(format!("{}/**/*", from.trim_end_matches('/')));
            argv.extend(["--tree".to_string(), from, dest(&t.to)?]);
            for g in &t.exclude {
                argv.extend(["--tree-exclude".to_string(), g.clone()]);
            }
            if let Some(m) = &t.mode {
                argv.extend(["--tree-mode".to_string(), m.clone()]);
            }
            if t.replace {
                argv.push("--replace".to_string());
            }
        }
        for x in v.data {
            argv.extend(["--data".to_string(), dest(&x.to)?, x.mode.clone().unwrap_or_else(|| "644".to_string()), x.content.clone()]);
        }
        deps.sort_unstable();
        deps.dedup();
        if let Some(d) = v.deb {
            for p in &d.require {
                argv.extend(["--require".to_string(), p.clone()]);
            }
            for p in &d.require_exec {
                argv.extend(["--require-exec".to_string(), p.clone()]);
            }
            for g in &d.forbid {
                argv.extend(["--forbid".to_string(), g.clone()]);
            }
            for t in &d.forbid_text {
                argv.extend(["--forbid-text".to_string(), t.clone()]);
            }
            if d.forbid_setuid {
                argv.push("--forbid-setuid".to_string());
            }
            for p in &d.sh_check {
                argv.extend(["--sh-check".to_string(), p.clone()]);
            }
            for c in &d.contains {
                argv.extend(["--contains".to_string(), c.file.clone(), c.text.clone()]);
            }
        }
        let mut outputs = vec![out.clone()];
        if let Some(p) = v.packages {
            let p = scope.expand(p)?;
            argv.extend(["--packages".to_string(), p.clone()]);
            outputs.push(p);
        }
        Ok(self.push(Node { label: format!("{} {}", v.format, out), argv, outputs, deps, input_globs, ..Default::default() }))
    }
}

struct PackView<'a> {
    format: &'static str,
    output: &'a str,
    prefix: &'a str,
    needs: &'a [String],
    file: &'a [crate::config::DebFile],
    tree: &'a [crate::config::DebTree],
    data: &'a [crate::config::DebData],
    control: Option<&'a String>,
    fields: &'a BTreeMap<String, String>,
    scripts: &'a BTreeMap<String, String>,
    packages: Option<&'a String>,
    deb: Option<&'a crate::config::Deb>,
}

// strip, sign and read the signature back, after the commands the config names
fn finishing(scope: &Scope, out: &str, strip: bool, sign: bool, entitlements: Option<&str>) -> Result<Vec<Vec<String>>> {
    let mut cmds = Vec::new();
    if strip {
        cmds.push(vec![scope.expand("${MEOW_STRIP}")?, "-x".to_string(), out.to_string()]);
    }
    if sign || entitlements.is_some() {
        let ldid = scope.expand("${MEOW_LDID}")?;
        let ents = match entitlements {
            Some(e) => Some(scope.expand(e)?),
            None => None,
        };
        cmds.push(vec![ldid.clone(), format!("-S{}", ents.as_deref().unwrap_or("")), out.to_string()]);
        let exe = std::env::current_exe().map_err(|e| Error(format!("cannot find the meow binary: {}", e)))?;
        let mut check = vec![exe.to_string_lossy().into_owned(), "verify-sign".to_string(), out.to_string(), "--ldid".to_string(), ldid];
        if let Some(e) = ents {
            check.extend(["--entitlements".to_string(), e]);
        }
        cmds.push(check);
    }
    Ok(cmds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn project(tag: &str, toml: &str) -> (PathBuf, Config) {
        let d = std::env::temp_dir().join(format!("meow-plan-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        for f in ["a.c", "b.c"] {
            std::fs::write(d.join("src").join(f), "int x;").unwrap();
        }
        let cfg: Config = toml::from_str(toml).unwrap();
        (d, cfg)
    }

    const BASE: &str = r#"
[vars]
TC = "/tc"
[slice.v7]
cc = "${TC}/clang"
ar = "${TC}/ar"
flags = ["-arch", "armv7"]
[slice.v7.vars]
SDK = "sdk7"
[slice.a64]
cc = "${TC}/clang"
flags = ["-arch", "arm64"]
[slice.a64.vars]
SDK = "sdk64"
[target.lib]
slices = ["v7", "a64"]
link = "none"
cflags = ["-isysroot", "${SDK}"]
[[target.lib.src]]
files = ["src/b.c"]
cflags = ["-w"]
[target.app]
slices = ["v7", "a64"]
output = "build/${slice}/app"
objects_from = ["lib"]
ldflags = ["-lz"]
post = [["ldid", "-S", "${out}"]]
[[target.app.src]]
files = ["src/*.c"]
[target.app.per_slice.a64]
cflags = ["-DROOTLESS=1"]
[fat.app-fat]
from = "app"
lipo = "${TC}/lipo"
output = "build/app"
"#;

    #[test]
    fn slices_fan_out_and_flags_layer() {
        let (d, cfg) = project("fan", BASE);
        let p = build(&cfg, &d, &["app".to_string()], &[]).unwrap();
        // two sources and one lib object per slice, plus one link per slice
        assert_eq!(p.nodes.len(), 2 * (2 + 1 + 1));
        let a64_cc = p
            .nodes
            .iter()
            .find(|n| n.label == "cc a64 src/a.c")
            .unwrap();
        assert!(a64_cc.argv.contains(&"-DROOTLESS=1".to_string()));
        let v7_cc = p.nodes.iter().find(|n| n.label == "cc v7 src/a.c").unwrap();
        assert!(!v7_cc.argv.contains(&"-DROOTLESS=1".to_string()));
        let lib_cc = p
            .nodes
            .iter()
            .find(|n| n.outputs[0] == "build/obj/a64/lib/src/b.c.o")
            .unwrap();
        assert!(lib_cc.argv.windows(2).any(|w| w == ["-isysroot", "sdk64"]));
        assert!(lib_cc.argv.contains(&"-w".to_string()));
        let ld = p.nodes.iter().find(|n| n.label == "ld v7 build/v7/app").unwrap();
        assert_eq!(ld.post, vec![vec!["ldid", "-S", "build/v7/app"]]);
        // link waits for its own objects and the lib object of the same slice
        assert_eq!(ld.deps.len(), 3);
        assert_eq!(ld.argv.last().unwrap(), "-lz");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn fat_joins_every_slice_and_rejects_slice_filter() {
        let (d, cfg) = project("fat", BASE);
        let p = build(&cfg, &d, &["app-fat".to_string()], &[]).unwrap();
        let fat = p.nodes.last().unwrap();
        assert!(fat.label.starts_with("lipo "));
        assert_eq!(fat.deps.len(), 2);
        assert_eq!(
            fat.argv,
            vec!["/tc/lipo", "-create", "build/v7/app", "build/a64/app", "-output", "build/app"]
        );
        assert!(build(&cfg, &d, &["app-fat".to_string()], &["v7".to_string()]).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn fat_accepts_an_archive_and_refuses_signing_it() {
        let toml = format!("{}\n[target.lib2]\nslices = [\"v7\", \"a64\"]\nlink = \"archive\"\noutput = \"build/${{slice}}/lib2.a\"\n[[target.lib2.src]]\nfiles = [\"src/a.c\"]\n[fat.lib2-fat]\nfrom = \"lib2\"\nlipo = \"${{TC}}/lipo\"\noutput = \"build/lib2.a\"\n", BASE)
            .replace("[slice.a64]\ncc", "[slice.a64]\nar = \"${TC}/ar\"\ncc");
        let (d, cfg) = project("fatar", &toml);
        crate::config::validate_for_test(&cfg).unwrap();
        let p = build(&cfg, &d, &["lib2-fat".to_string()], &[]).unwrap();
        assert_eq!(p.nodes.last().unwrap().argv[..2], ["/tc/lipo", "-create"]);
        let signed = format!("{}sign = true\n", toml);
        let cfg: Config = toml::from_str(&signed).unwrap();
        assert!(crate::config::validate_for_test(&cfg).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn slice_filter_builds_one_slice() {
        let (d, cfg) = project("one", BASE);
        let p = build(&cfg, &d, &["app".to_string()], &["a64".to_string()]).unwrap();
        assert!(p.nodes.iter().all(|n| !n.label.contains(" v7 ")));
        assert_eq!(p.nodes.len(), 4);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn missing_source_and_empty_glob_fail_at_plan_time() {
        let toml = BASE.replace("src/b.c", "src/missing.c");
        let (d, cfg) = project("miss", &toml);
        assert!(build(&cfg, &d, &["app".to_string()], &[]).is_err());
        let toml = BASE.replace("src/*.c", "src/*.zz");
        let cfg: Config = toml::from_str(&toml).unwrap();
        assert!(build(&cfg, &d, &["app".to_string()], &[]).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn steps_gate_targets_per_slice_and_default_names_validate() {
        let toml = format!(
            "default = [\"pack\"]\n{}{}",
            BASE.replace("objects_from = [\"lib\"]", "objects_from = [\"lib\"]\nneeds = [\"dep-${slice}\"]"),
            r#"
[step.dep-v7]
run = [["/bin/true"]]
outputs = ["build/dep-v7"]
[step.dep-a64]
run = [["/bin/true"]]
outputs = ["build/dep-a64"]
[step.pack]
run = [["/bin/echo", "${field:src/ctl:Version}"], ["/bin/true"]]
stamp = "build/pack.stamp"
env = { K = "${root}" }
inputs = ["src/*.c"]
needs = ["app-fat"]
"#
        );
        let (d, cfg) = project("step", &toml);
        std::fs::write(d.join("src/ctl"), "Version: 9.9\n").unwrap();
        crate::config::validate_for_test(&cfg).unwrap();
        let p = build(&cfg, &d, &cfg.default.clone(), &[]).unwrap();
        let pack = p.nodes.last().unwrap();
        assert_eq!(pack.argv, vec!["/bin/echo", "9.9"]);
        assert_eq!(pack.post, vec![vec!["/bin/true"]]);
        assert_eq!(pack.outputs, vec!["build/pack.stamp"]);
        assert_eq!(pack.env[0].1, d.to_string_lossy());
        // each app link waits for the step of its own slice
        let v7 = p.nodes.iter().position(|n| n.label == "step dep-v7").unwrap();
        let a64 = p.nodes.iter().position(|n| n.label == "step dep-a64").unwrap();
        let ld7 = p.nodes.iter().find(|n| n.label == "ld v7 build/v7/app").unwrap();
        assert!(ld7.deps.contains(&v7) && !ld7.deps.contains(&a64));
        // the step is planned once however many nodes need it
        assert_eq!(p.nodes.iter().filter(|n| n.label == "step dep-v7").count(), 1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn verify_and_package_nodes_wait_for_what_makes_their_files() {
        let toml = format!(
            "{}\n[verify.chk]\nfile = \"build/app\"\nslices = [\"v7\", \"a64\"]\nmin_ios = \"5.0\"\nsigned = true\nforbid_dylibs = [\"/usr/lib/libz\"]\n[deb.pkg]\noutput = \"build/p.deb\"\nfields = {{ Package = \"p\" }}\npackages = \"build/p.Packages\"\nneeds = [\"chk\"]\n[[deb.pkg.file]]\nfrom = \"build/app\"\nto = \"usr/bin/app\"\nmode = \"755\"\nreplace = true\n[[deb.pkg.tree]]\nfrom = \"src\"\nto = \"x\"\nexclude = [\"*.h\"]\n",
            BASE
        );
        let (d, cfg) = project("verify", &toml);
        let p = build(&cfg, &d, &["pkg".to_string()], &[]).unwrap();
        let fat = p.nodes.iter().position(|n| n.label == "lipo build/app").unwrap();
        let ver = p.nodes.iter().position(|n| n.label == "verify build/app").unwrap();
        let deb = p.nodes.iter().position(|n| n.label.starts_with("deb ")).unwrap();
        assert_eq!(p.nodes[ver].deps, vec![fat]);
        assert!(p.nodes[ver].argv.windows(2).any(|w| w == ["--slices", "v7,a64"]));
        assert!(p.nodes[ver].argv.contains(&"--signed".to_string()));
        assert_eq!(p.nodes[ver].stamp.as_deref(), Some("build/stamps/chk"));
        // the deb needs the fat output (by path) and the verify named in needs
        assert!(p.nodes[deb].deps.contains(&fat) && p.nodes[deb].deps.contains(&ver));
        let a = &p.nodes[deb].argv;
        assert!(a.windows(2).any(|w| w == ["--tree-exclude", "*.h"]));
        assert!(a.windows(2).any(|w| w == ["--packages", "build/p.Packages"]));
        assert!(a.contains(&"--replace".to_string()));
        assert_eq!(p.nodes[deb].outputs, vec!["build/p.deb", "build/p.Packages"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_package_does_not_start_a_keep_step_that_writes_into_the_source_tree() {
        let toml = format!(
            "{}\n[step.icons]\nrun = [[\"true\"]]\noutputs = [\"src/icon.png\"]\nkeep = true\n[step.gen]\nrun = [[\"true\"]]\noutputs = [\"build/gen.txt\"]\n[deb.pkg]\noutput = \"build/p.deb\"\nfields = {{ Package = \"p\" }}\n[[deb.pkg.file]]\nfrom = \"src/icon.png\"\nto = \"a/icon.png\"\n[[deb.pkg.file]]\nfrom = \"build/gen.txt\"\nto = \"a/gen.txt\"\n",
            BASE
        );
        let (d, cfg) = project("keepstep", &toml);
        std::fs::write(d.join("src/icon.png"), "x").unwrap();
        let p = build(&cfg, &d, &["pkg".to_string()], &[]).unwrap();
        // the ordinary step is a dependency, the keep step is not even in the plan
        assert!(p.nodes.iter().any(|n| n.label == "step gen"));
        assert!(!p.nodes.iter().any(|n| n.label == "step icons"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn object_paths_cannot_escape_build_dir() {
        assert_eq!(obj_path("v7", "t", "../daemon/core/b64.c"), "build/obj/v7/t/_/daemon/core/b64.c.o");
    }
}
