use crate::cache::Cache;
use crate::config::{err, Error, Result};
use crate::depfile;
use crate::plan::{render, Node, Plan};
use crate::state::{self, Record, Store};
use crate::ui::Ui;
use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::process::Command;
use std::sync::{Condvar, Mutex};
use std::time::SystemTime;

pub struct Options {
    pub jobs: usize,
    pub verbose: bool,
    pub cache: Option<Cache>,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub built: usize,
    pub fresh: usize,
    // compiles taken from the shared cache, counted in built too
    pub cached: usize,
}

// existing files that match the node's input globs, relative to the root when they
// are inside it
fn resolve_inputs(n: &Node, root: &Path) -> Vec<String> {
    let mut out = BTreeSet::new();
    for pat in &n.input_globs {
        let full = root.join(pat);
        let Ok(paths) = glob::glob(&full.to_string_lossy()) else {
            continue;
        };
        for hit in paths.flatten() {
            if state::stat_of(&hit).is_none() {
                continue;
            }
            let rel = hit.strip_prefix(root).unwrap_or(&hit);
            out.insert(rel.to_string_lossy().into_owned());
        }
    }
    out.into_iter().collect()
}

// the hash covers the commands, the environment and the set of glob inputs, so a
// new matching file makes the node stale before any content is compared
fn full_cmd(n: &Node, root: &Path) -> String {
    let mut all: Vec<Vec<String>> = vec![n.argv.clone()];
    all.extend(n.post.iter().cloned());
    all.push(n.env.iter().map(|(k, v)| format!("{}={}", k, v)).collect());
    all.push(resolve_inputs(n, root));
    let refs: Vec<&Vec<String>> = all.iter().collect();
    state::cmd_hash(&refs)
}

// a node is fresh when its record matches, so the answer for a stale dependency is
// already no, the caller passes that through
pub fn node_fresh(n: &Node, store: &Store, root: &Path) -> bool {
    if n.bootstrap {
        return n.outputs.iter().all(|o| root.join(o).is_file());
    }
    let Some(rec) = store.load(&n.outputs[0]) else {
        return false;
    };
    state::is_fresh(&rec, &full_cmd(n, root), &n.outputs, root)
}

// the first reason a node is stale, for `meow why`
pub fn why(plan: &Plan, i: usize, fresh: &[bool], store: &Store) -> String {
    let n = &plan.nodes[i];
    if let Some(&d) = n.deps.iter().find(|&&d| !fresh[d]) {
        return format!("waits for {}, which is stale", plan.nodes[d].label);
    }
    let root = &plan.root;
    if n.bootstrap {
        let missing = n.outputs.iter().find(|o| !root.join(o).is_file());
        return missing.map_or("fresh".to_string(), |o| format!("output {} is missing", o));
    }
    let Some(rec) = store.load(&n.outputs[0]) else {
        return "never built here, or its record was removed".to_string();
    };
    state::why_stale(&rec, &full_cmd(n, root), &n.outputs, root).unwrap_or_else(|| "fresh".to_string())
}

pub fn fresh_map(plan: &Plan, store: &Store) -> Vec<bool> {
    let mut fresh = vec![false; plan.nodes.len()];
    // dependencies always have a lower index than their dependents
    for (i, n) in plan.nodes.iter().enumerate() {
        fresh[i] = n.deps.iter().all(|&d| fresh[d]) && node_fresh(n, store, &plan.root);
    }
    fresh
}

fn run_argv(argv: &[String], env: &[(String, String)], dir: &Path) -> Result<std::process::Output> {
    Command::new(&argv[0])
        .args(&argv[1..])
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(dir)
        .output()
        .map_err(|e| Error(format!("cannot run {}: {}", argv[0], e)))
}

fn output_text(out: &std::process::Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&out.stdout));
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text.trim_end().to_string()
}

fn show_output(ui: &Ui, label: &str, out: &std::process::Output) {
    let text = output_text(out);
    if !text.trim().is_empty() {
        ui.block(label, &text);
    }
}

// files a linker reads without a bare path on the command line: -lname found through
// -L, pieces of -Wl,a,b (force_load, filelist, exported symbols) and @response files.
// without them a changed prebuilt archive would not relink the target
fn hidden_link_inputs(argv: &[String], outputs: &[String], root: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut dirs: Vec<String> = Vec::new();
    let mut libs: Vec<&str> = Vec::new();
    let take = |p: &str, found: &mut BTreeSet<String>| {
        if !p.is_empty() && !outputs.iter().any(|o| o == p) && state::stat_of(&root.join(p)).is_some() {
            found.insert(p.to_string());
        }
    };
    let mut args = argv.iter().skip(1);
    while let Some(a) = args.next() {
        if let Some(d) = a.strip_prefix("-L") {
            let d = if d.is_empty() { args.next().map(|s| s.as_str()).unwrap_or("") } else { d };
            dirs.push(d.to_string());
        } else if let Some(l) = a.strip_prefix("-l") {
            libs.push(l);
        } else if let Some(list) = a.strip_prefix("-Wl,") {
            for piece in list.split(',') {
                take(piece, &mut found);
            }
        } else if let Some(r) = a.strip_prefix('@') {
            take(r, &mut found);
        }
    }
    for l in libs {
        for d in &dirs {
            for ext in ["a", "dylib", "tbd"] {
                take(&format!("{}/lib{}.{}", d.trim_end_matches('/'), l, ext), &mut found);
            }
        }
    }
    found
}

fn execute(n: &Node, store: &Store, root: &Path, verbose: bool, ui: &Ui, cache: Option<&Cache>) -> Result<bool> {
    for o in &n.outputs {
        let p = root.join(o);
        if !n.keep && p.exists() {
            std::fs::remove_file(&p).map_err(|e| Error(format!("{}: {}", o, e)))?;
        }
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Error(format!("{}: {}", dir.display(), e)))?;
        }
        store.forget(o);
    }
    let started = SystemTime::now();

    let cache = cache.filter(|_| Cache::eligible(n));
    if let Some(c) = cache {
        if let Some(list) = c.restore(n, root) {
            if let Some(snap) = state::snapshot(&list, root, started)? {
                store.save(&n.outputs[0], &Record { cmd: full_cmd(n, root), inputs: snap })?;
            }
            return Ok(true);
        }
    }

    // files named on the command line are inputs too: objects, archives, the crt
    let mut inputs: BTreeSet<String> = BTreeSet::new();
    for a in n.argv.iter().chain(n.post.iter().flatten()) {
        if a.starts_with('-') || n.outputs.contains(a) || n.depfile.as_deref() == Some(a) {
            continue;
        }
        if state::stat_of(&root.join(a)).is_some() {
            inputs.insert(a.clone());
        }
    }
    inputs.extend(hidden_link_inputs(&n.argv, &n.outputs, root));

    for argv in std::iter::once(&n.argv).chain(n.post.iter()) {
        if verbose {
            ui.command(&render(argv));
        }
        let dir = match &n.cwd {
            Some(c) => root.join(c),
            None => root.to_path_buf(),
        };
        let out = run_argv(argv, &n.env, &dir)?;
        if !out.status.success() {
            ui.fail_block(&n.label, &render(argv), &output_text(&out));
            if !n.keep {
                for o in &n.outputs {
                    let _ = std::fs::remove_file(root.join(o));
                }
            }
            return err(format!("{} failed ({})", n.label, out.status));
        }
        show_output(ui, &n.label, &out);
    }

    if let Some(stamp) = &n.stamp {
        std::fs::write(root.join(stamp), b"").map_err(|e| Error(format!("{}: {}", stamp, e)))?;
    }
    for o in &n.outputs {
        if !root.join(o).is_file() {
            return err(format!("{} did not produce {}", n.label, o));
        }
    }
    inputs.extend(resolve_inputs(n, root));
    if let Some(dep) = &n.depfile {
        let text = std::fs::read_to_string(root.join(dep))
            .map_err(|e| Error(format!("{}: {}", dep, e)))?;
        for d in depfile::parse(&text) {
            if state::stat_of(&root.join(&d)).is_some() {
                inputs.insert(d);
            }
        }
    }
    let list: Vec<String> = inputs.into_iter().collect();
    if let Some(snap) = state::snapshot(&list, root, started)? {
        if let Some(c) = cache {
            if let Err(e) = c.store(n, root, &snap) {
                ui.line(&format!("meow: object cache: {}", e));
            }
        }
        store.save(&n.outputs[0], &Record { cmd: full_cmd(n, root), inputs: snap })?;
    }
    Ok(false)
}

struct Shared {
    ready: VecDeque<usize>,
    waiting: Vec<usize>,
    running: usize,
    done: usize,
    summary: Summary,
    failed: Option<String>,
}

pub fn run(plan: &Plan, store: &Store, opts: &Options) -> Result<Summary> {
    let n = plan.nodes.len();
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut waiting = vec![0usize; n];
    for (i, node) in plan.nodes.iter().enumerate() {
        waiting[i] = node.deps.len();
        for &d in &node.deps {
            dependents[d].push(i);
        }
    }
    let ready: VecDeque<usize> = (0..n).filter(|&i| waiting[i] == 0).collect();
    let shared = Mutex::new(Shared {
        ready,
        waiting,
        running: 0,
        done: 0,
        summary: Summary::default(),
        failed: None,
    });
    let cv = Condvar::new();
    let jobs = opts.jobs.max(1).min(n.max(1));
    let ui = Ui::new(n);

    std::thread::scope(|scope| {
        scope.spawn(|| ui.animate());
        let workers: Vec<_> = (0..jobs).map(|_| {
            scope.spawn(|| loop {
                let id = {
                    let mut s = shared.lock().unwrap();
                    loop {
                        if s.failed.is_some() && s.running == 0 {
                            cv.notify_all();
                            return;
                        }
                        if s.failed.is_none() {
                            if let Some(i) = s.ready.pop_front() {
                                s.running += 1;
                                break i;
                            }
                        }
                        if s.done == n || (s.failed.is_some() && s.running == 0) {
                            cv.notify_all();
                            return;
                        }
                        s = cv.wait(s).unwrap();
                    }
                };
                let node = &plan.nodes[id];
                let result: Result<Option<bool>> = if node_fresh(node, store, &plan.root) {
                    Ok(None)
                } else {
                    ui.start(id, &node.label);
                    let r = execute(node, store, &plan.root, opts.verbose, &ui, opts.cache.as_ref());
                    if r.is_err() {
                        ui.abort_step(id);
                    }
                    r.map(Some)
                };
                let mut s = shared.lock().unwrap();
                s.running -= 1;
                match result {
                    Ok(outcome) => {
                        ui.finish(id, outcome.is_some());
                        match outcome {
                            Some(from_cache) => {
                                s.summary.built += 1;
                                s.summary.cached += from_cache as usize;
                            }
                            None => s.summary.fresh += 1,
                        }
                        s.done += 1;
                        for &d in &dependents[id] {
                            s.waiting[d] -= 1;
                            if s.waiting[d] == 0 {
                                s.ready.push_back(d);
                            }
                        }
                    }
                    Err(e) => {
                        if s.failed.is_none() {
                            s.failed = Some(e.0);
                        }
                    }
                }
                cv.notify_all();
            })
        }).collect();
        let mut panic = None;
        for w in workers {
            if let Err(p) = w.join() {
                panic = Some(p);
            }
        }
        ui.stop();
        if let Some(p) = panic {
            std::panic::resume_unwind(p);
        }
    });

    let s = shared.into_inner().unwrap();
    if let Some(f) = s.failed {
        return err(f);
    }
    if s.summary.built > s.summary.cached {
        if let Some(c) = &opts.cache {
            c.gc(crate::cache::max_bytes());
        }
    }
    ui.summary(s.summary.built, s.summary.fresh, s.summary.cached);
    Ok(s.summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_link_inputs_follow_l_wl_and_response_files() {
        let d = std::env::temp_dir().join(format!("meow-hidden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("deps/lib")).unwrap();
        for f in ["deps/lib/libweb.a", "deps/lib/libgone.dylib", "force.a", "syms.txt", "link.rsp"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let argv: Vec<String> = ["cc", "a.o", "-L", "deps/lib", "-lweb", "-lmissing", "-Wl,-force_load,force.a,-exported_symbols_list,syms.txt", "@link.rsp", "-Ldeps/lib", "-lgone", "-o", "out"]
            .iter().map(|s| s.to_string()).collect();
        let got = hidden_link_inputs(&argv, &["out".to_string()], &d);
        let want: BTreeSet<String> = ["deps/lib/libweb.a", "deps/lib/libgone.dylib", "force.a", "syms.txt", "link.rsp"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(got, want);
        std::fs::remove_dir_all(&d).unwrap();
    }
    
    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".into(), "-c".into(), script.into()]
    }

    fn node(label: &str, script: &str, out: &str, deps: Vec<usize>) -> Node {
        Node {
            label: label.into(),
            argv: sh(script),
            outputs: vec![out.into()],
            deps,
            ..Default::default()
        }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("meow-sched-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn runs_in_dependency_order_then_everything_is_fresh() {
        let root = tmp("order");
        let nodes = vec![
            node("a", "echo a > out/a", "out/a", vec![]),
            node("b", "echo b > out/b", "out/b", vec![]),
            node("c", "cat out/a out/b > out/c", "out/c", vec![0, 1]),
        ];
        std::fs::create_dir_all(root.join("out")).unwrap();
        let plan = Plan { root: root.clone(), nodes };
        let store = Store::new(root.join("db"));
        let opts = Options { jobs: 4, verbose: false, cache: None };
        let s = run(&plan, &store, &opts).unwrap();
        assert_eq!((s.built, s.fresh), (3, 0));
        assert_eq!(std::fs::read_to_string(root.join("out/c")).unwrap(), "a\nb\n");
        assert!(fresh_map(&plan, &store).iter().all(|&f| f));
        let s = run(&plan, &store, &opts).unwrap();
        assert_eq!((s.built, s.fresh), (0, 3));
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn step(label: &str, cmds: &[&str], outputs: &[&str]) -> Node {
        let mut c = cmds.iter().map(|c| sh(c));
        Node {
            label: label.into(),
            argv: c.next().unwrap(),
            post: c.collect(),
            outputs: outputs.iter().map(|o| o.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn step_stamp_env_and_glob_inputs() {
        let root = tmp("step");
        std::fs::create_dir_all(root.join("in")).unwrap();
        std::fs::write(root.join("in/a.txt"), "a").unwrap();
        let old = SystemTime::now() - std::time::Duration::from_secs(120);
        std::fs::OpenOptions::new().write(true).open(root.join("in/a.txt")).unwrap().set_modified(old).unwrap();
        let mut n = step("s", &["echo $MEOW_T > made.txt", "test -f made.txt"], &["made.txt"]);
        n.env = vec![("MEOW_T".into(), "one".into())];
        n.input_globs = vec!["in/*.txt".into()];
        n.stamp = Some("done.stamp".into());
        n.outputs.push("done.stamp".into());
        let plan = Plan { root: root.clone(), nodes: vec![n] };
        let store = Store::new(root.join("db"));
        let opts = Options { jobs: 1, verbose: false, cache: None };
        assert_eq!(run(&plan, &store, &opts).unwrap().built, 1);
        assert_eq!(std::fs::read_to_string(root.join("made.txt")).unwrap(), "one\n");
        assert!(root.join("done.stamp").exists());
        assert_eq!(run(&plan, &store, &opts).unwrap().fresh, 1);
        // a new file matching the glob changes the input set
        std::fs::write(root.join("in/b.txt"), "b").unwrap();
        assert_eq!(run(&plan, &store, &opts).unwrap().built, 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn step_must_make_its_declared_outputs() {
        let root = tmp("noout");
        let plan = Plan { root: root.clone(), nodes: vec![step("lazy", &["true"], &["never.txt"])] };
        let store = Store::new(root.join("db"));
        let e = run(&plan, &store, &Options { jobs: 1, verbose: false, cache: None }).unwrap_err();
        assert!(e.0.contains("did not produce never.txt"), "{}", e.0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bootstrap_never_runs_while_outputs_exist() {
        let root = tmp("boot");
        std::fs::write(root.join("lib.a"), "built long ago").unwrap();
        // the command would wipe the output, so reaching it is the failure
        let mut n = step("b", &["rm -f lib.a; exit 1"], &["lib.a"]);
        n.bootstrap = true;
        n.keep = true;
        let plan = Plan { root: root.clone(), nodes: vec![n] };
        let store = Store::new(root.join("db"));
        let s = run(&plan, &store, &Options { jobs: 1, verbose: false, cache: None }).unwrap();
        assert_eq!((s.built, s.fresh), (0, 1));
        assert!(root.join("lib.a").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn keep_leaves_outputs_in_place_when_stale() {
        let root = tmp("keep");
        std::fs::write(root.join("cache.bin"), "precious").unwrap();
        let mut n = step("k", &["true"], &["cache.bin"]);
        n.keep = true;
        let plan = Plan { root: root.clone(), nodes: vec![n] };
        let store = Store::new(root.join("db"));
        run(&plan, &store, &Options { jobs: 1, verbose: false, cache: None }).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("cache.bin")).unwrap(), "precious");
        std::fs::remove_dir_all(&root).unwrap();
    }

    // two build trees with the same sources share objects through the cache
    #[test]
    fn a_second_tree_takes_the_object_from_the_cache() {
        let shared = tmp("cache-shared");
        let counter = shared.join("compiles");
        let script = shared.join("cc.sh");
        // a compiler that counts its runs and writes an object and a depfile
        std::fs::write(&script, format!("#!/bin/sh\necho run >> {}\ncat src/a.c inc.h > \"$6\"\nprintf '%s: src/a.c inc.h\\n' \"$6\" > \"$2\"\n", counter.display())).unwrap();
        let old = SystemTime::now() - std::time::Duration::from_secs(300);
        let tree = |tag: &str, header: &str| {
            let root = tmp(tag);
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("src/a.c"), "int a;\n").unwrap();
            std::fs::write(root.join("inc.h"), header).unwrap();
            for f in ["src/a.c", "inc.h"] {
                std::fs::OpenOptions::new().write(true).open(root.join(f)).unwrap().set_modified(old).unwrap();
            }
            let node = Node {
                label: "cc a".into(),
                argv: ["/bin/sh", script.to_str().unwrap(), "-MF", "build/a.o.d", "-c", "src/a.c", "-o", "build/a.o"].iter().map(|s| s.to_string()).collect(),
                outputs: vec!["build/a.o".into()],
                depfile: Some("build/a.o.d".into()),
                ..Default::default()
            };
            (root.clone(), Plan { root: root.clone(), nodes: vec![node] }, Store::new(root.join("db")))
        };
        let opts = |c: bool| Options { jobs: 1, verbose: false, cache: if c { Some(Cache::at(shared.join("objcache"))) } else { None } };
        let runs = || std::fs::read_to_string(&counter).map_or(0, |t| t.lines().count());

        let (r1, p1, s1) = tree("cache-a", "// v1\n");
        let a = run(&p1, &s1, &opts(true)).unwrap();
        assert_eq!((a.built, a.cached, runs()), (1, 0, 1));

        let (r2, p2, s2) = tree("cache-b", "// v1\n");
        let b = run(&p2, &s2, &opts(true)).unwrap();
        assert_eq!((b.built, b.cached, runs()), (1, 1, 1), "the compiler must not run again");
        assert_eq!(std::fs::read(r2.join("build/a.o")).unwrap(), std::fs::read(r1.join("build/a.o")).unwrap());
        assert!(r2.join("build/a.o.d").is_file());
        // the restore wrote a record, so the next run is fresh
        assert_eq!(run(&p2, &s2, &opts(true)).unwrap().fresh, 1);

        // a header with other content is a miss
        let (r3, p3, s3) = tree("cache-c", "// v2\n");
        let c = run(&p3, &s3, &opts(true)).unwrap();
        assert_eq!((c.built, c.cached, runs()), (1, 0, 2));

        // without the cache the compiler always runs
        let (r4, p4, s4) = tree("cache-d", "// v1\n");
        run(&p4, &s4, &opts(false)).unwrap();
        assert_eq!(runs(), 3);

        // a damaged entry is dropped and the compile runs
        let cache = Cache::at(shared.join("objcache"));
        assert_eq!(cache.stats().0, 2);
        for e in cache.entries() {
            std::fs::write(e.0.join("out"), "garbage").unwrap();
        }
        let (r5, p5, s5) = tree("cache-e", "// v1\n");
        let e = run(&p5, &s5, &opts(true)).unwrap();
        assert_eq!((e.cached, runs()), (0, 4));
        // gc keeps the newest entries within the limit
        assert_eq!(cache.gc(0).0 >= 1, true);
        for r in [r1, r2, r3, r4, r5, shared] {
            std::fs::remove_dir_all(&r).unwrap();
        }
    }

    #[test]
    fn debug_builds_and_multi_output_nodes_are_not_cached() {
        let mut n = Node { argv: vec!["cc".into(), "-c".into(), "a.c".into()], outputs: vec!["a.o".into()], depfile: Some("a.d".into()), ..Default::default() };
        assert!(Cache::eligible(&n));
        n.argv.push("-g".into());
        assert!(!Cache::eligible(&n));
        n.argv.pop();
        n.outputs.push("b.o".into());
        assert!(!Cache::eligible(&n));
        n.outputs.pop();
        n.depfile = None;
        assert!(!Cache::eligible(&n));
    }

    #[test]
    fn failure_stops_dependents_and_reports() {
        let root = tmp("fail");
        let nodes = vec![
            node("bad", "echo boom >&2; exit 3", "out/bad", vec![]),
            node("after", "echo x > out/after", "out/after", vec![0]),
        ];
        let plan = Plan { root: root.clone(), nodes };
        let store = Store::new(root.join("db"));
        let e = run(&plan, &store, &Options { jobs: 2, verbose: false, cache: None }).unwrap_err();
        assert!(e.0.contains("bad failed"), "{}", e.0);
        assert!(!root.join("out/after").exists());
        assert!(!root.join("out/bad").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn changed_input_rebuilds_only_its_dependents() {
        let root = tmp("inc");
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("one.txt"), "1").unwrap();
        std::fs::write(root.join("two.txt"), "2").unwrap();
        // the file name on the command line is what makes it an input
        let nodes = vec![
            Node { argv: vec!["/bin/cp".into(), "one.txt".into(), "out/one".into()], ..node("one", "", "out/one", vec![]) },
            Node { argv: vec!["/bin/cp".into(), "two.txt".into(), "out/two".into()], ..node("two", "", "out/two", vec![]) },
        ];
        let plan = Plan { root: root.clone(), nodes };
        let store = Store::new(root.join("db"));
        let opts = Options { jobs: 2, verbose: false, cache: None };
        // inputs younger than the job start are never recorded, so age them first
        let old = SystemTime::now() - std::time::Duration::from_secs(120);
        for f in ["one.txt", "two.txt"] {
            std::fs::OpenOptions::new().write(true).open(root.join(f)).unwrap().set_modified(old).unwrap();
        }
        assert_eq!(run(&plan, &store, &opts).unwrap().built, 2);
        std::fs::write(root.join("one.txt"), "changed").unwrap();
        let s = run(&plan, &store, &opts).unwrap();
        assert_eq!((s.built, s.fresh), (1, 1));
        assert_eq!(std::fs::read_to_string(root.join("out/one")).unwrap(), "changed");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
