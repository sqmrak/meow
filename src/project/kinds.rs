use crate::config::{
    err, Check, Config, Dep, Deb, DebData, DebFile, DebTree, Error, Fat, Ipa, Kind, LinkKind, Product, Result, Scheme, Slice, SrcGroup, Step, Target,
    TargetSlice, Test, Verify,
};
use crate::ios;
use std::collections::BTreeMap;
use std::path::Path;

// a [project] table is shorthand for the slices, targets, fat output and deb that
// every product of one kind needs. it is turned into the ordinary tables before the
// config is validated, so the planner never sees a project

const ARCHS: [&str; 4] = ["armv6", "armv7", "arm64", "arm64e"];
const SOURCE_EXTENSIONS: [&str; 8] = ["c", "m", "mm", "cpp", "cc", "x", "xm", "swift"];
const DEFAULT_MIN_IOS: &str = "5.0";
// with an armv6 slice the project has to reach ios 3, where armv6 devices still got updates
const DEFAULT_MIN_IOS_ARMV6: &str = "3.0";

pub(crate) fn valid_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('.') && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn valid_package_id(id: &str) -> bool {
    id.len() >= 2 && id.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
}

fn valid_version(v: &str) -> bool {
    v.chars().next().is_some_and(|c| c.is_ascii_digit()) && v.chars().all(|c| c.is_ascii_alphanumeric() || ".+~:-".contains(c))
}

// the armv6 sdk is a thin copy made by the sdk-armv6 step, see sdkthin.rs
fn slice_def(arch: &str, min: ios::Version, named_sdk: Option<&str>, rust: bool) -> Slice {
    // the variable of a named sdk, MEOW_SDK_9_3 for 9.3
    let named = named_sdk.map(|v| format!("${{MEOW_SDK_{}}}", v.replace('.', "_")));
    let pick = |default: &str| named.clone().unwrap_or_else(|| default.to_string());
    let (triple, sdk) = match arch {
        "armv6" => ("arm-apple-darwin10", "${MEOW_CACHE_DIR}/sdk-armv6".to_string()),
        "armv7" => ("arm-apple-darwin11", pick("${MEOW_SDK_ARMV7}")),
        "arm64" => ("arm64-apple-darwin", pick("${MEOW_SDK_ARM64}")),
        _ => ("arm64e-apple-darwin", pick("${MEOW_SDK_ARM64}")),
    };
    let mut min = ios::slice_min(arch, min);
    // the rust std on arm64 uses thread local variables, which ld refuses to emit below ios 8
    if rust && arch.starts_with("arm64") {
        min = min.max(RUST_ARM64_MIN_IOS);
    }
    Slice {
        cc: "${MEOW_CLANG}".to_string(),
        ar: "${MEOW_AR}".to_string(),
        flags: ["-target", triple, "-B", "${MEOW_TC}", "-arch", arch, &format!("-miphoneos-version-min={}", ios::text(min)), "-isysroot", "${SDK}"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        vars: BTreeMap::from([("SDK".to_string(), sdk)]),
    }
}

fn find_sources(root: &Path) -> Result<Vec<String>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out)?;
            } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| SOURCE_EXTENSIONS.contains(&x)) {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    let dir = root.join("src");
    if !dir.is_dir() {
        return err("project: no src directory, name the sources with src = [...]");
    }
    let mut out = Vec::new();
    walk(root, &dir, &mut out).map_err(|e| Error(format!("{}: {}", dir.display(), e)))?;
    if out.is_empty() {
        return err("project: src/ has no c, objective-c or c++ files");
    }
    Ok(out)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn filter_plist(bundles: &[String], executables: &[String]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>Filter</key>\n\t<dict>\n",
    );
    for (key, list) in [("Bundles", bundles), ("Executables", executables)] {
        if list.is_empty() {
            continue;
        }
        s.push_str(&format!("\t\t<key>{}</key>\n\t\t<array>\n", key));
        for v in list {
            s.push_str(&format!("\t\t\t<string>{}</string>\n", xml_escape(v)));
        }
        s.push_str("\t\t</array>\n");
    }
    s.push_str("\t</dict>\n</dict>\n</plist>\n");
    s
}

// the string after a key in a text plist, none for a binary plist or a missing key
pub fn plist_string(text: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{}</key>", key);
    let at = text.find(&marker)?;
    let rest = &text[at + marker.len()..];
    let a = rest.find("<string>")? + "<string>".len();
    let b = rest[a..].find("</string>")?;
    let v = rest[a..a + b].trim().to_string();
    if v.is_empty() { None } else { Some(v) }
}

fn exe_path() -> Result<String> {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).map_err(|e| Error(format!("cannot find the meow binary: {}", e)))
}

fn taken(cfg: &Config, n: &str) -> bool {
    cfg.target.contains_key(n)
        || cfg.fat.contains_key(n)
        || cfg.step.contains_key(n)
        || cfg.deb.contains_key(n)
        || cfg.ipa.contains_key(n)
        || cfg.verify.contains_key(n)
}

// a version written as @file:key is read from that file, so it has one source
fn read_version_ref(root: &Path, spec: &str) -> Result<String> {
    let body = &spec[1..];
    let (file, key) = body.rsplit_once(':').ok_or_else(|| Error(format!("version {:?} needs @FILE:KEY", spec)))?;
    read_key(root, file, key)
}

pub fn read_key(root: &Path, file: &str, key: &str) -> Result<String> {
    let text = std::fs::read_to_string(root.join(file)).map_err(|e| Error(format!("{}: {}", file, e)))?;
    let found = if file.ends_with(".plist") {
        plist_string(&text, key)
    } else if [".h", ".c", ".m"].iter().any(|e| file.ends_with(e)) {
        // #define KEY @"v1.2.3", the leading v is not part of the version
        let marker = format!("#define {} ", key);
        text.lines()
            .find_map(|l| l.strip_prefix(&marker))
            .and_then(|v| v.trim().strip_prefix("@\""))
            .and_then(|v| v.split('"').next())
            .map(|v| v.strip_prefix('v').unwrap_or(v).to_string())
            .filter(|v| !v.is_empty())
    } else {
        let prefix = format!("{}:", key);
        text.lines().find_map(|l| l.strip_prefix(&prefix)).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
    };
    found.ok_or_else(|| Error(format!("{}: no {}", file, key)))
}

fn host_slice() -> Slice {
    Slice { cc: "${MEOW_HOST_CC}".to_string(), ar: "${MEOW_HOST_AR}".to_string(), ..Default::default() }
}

fn expand_deps(cfg: &mut Config) -> Result<()> {
    let deps: Vec<(String, Dep)> = cfg.dep.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, d) in deps {
        if !valid_name(&n) {
            return err(format!("dep {}: bad name", n));
        }
        let hex = |s: &str, len: usize| s.len() == len && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        let (id, mut run) = match (&d.url, &d.sha256, &d.git, &d.rev) {
            (Some(u), Some(h), None, None) => {
                if !hex(h, 64) {
                    return err(format!("dep {}: sha256 must be 64 lowercase hex digits", n));
                }
                (
                    h[..8].to_string(),
                    vec!["--url".to_string(), u.clone(), "--sha256".to_string(), h.clone(), "--strip".to_string(), d.strip.unwrap_or(1).to_string()],
                )
            }
            (None, None, Some(g), Some(r)) => {
                if !hex(r, 40) {
                    return err(format!("dep {}: rev must be a full 40 digit commit id, a branch or tag can move", n));
                }
                (r[..8].to_string(), vec!["--git".to_string(), g.clone(), "--rev".to_string(), r.clone()])
            }
            _ => return err(format!("dep {}: give url and sha256, or git and rev", n)),
        };
        let dir = format!("${{MEOW_CACHE_DIR}}/src/{}-{}", n, id);
        let mut argv = vec![exe_path()?, "fetch".to_string(), "--name".to_string(), n.clone()];
        argv.append(&mut run);
        argv.extend(["--cache".to_string(), "${MEOW_CACHE_DIR}".to_string(), "--to".to_string(), dir.clone()]);
        for p in &d.patches {
            argv.extend(["--patch".to_string(), p.clone()]);
        }
        let step = format!("dep-{}", n);
        if taken(cfg, &step) {
            return err(format!("dep {} makes step {}, which the file already defines", n, step));
        }
        cfg.step.insert(
            step,
            Step { run: vec![argv], outputs: vec![format!("{}/{}", dir, crate::fetch::STAMP)], keep: true, ..Default::default() },
        );
        cfg.vars.insert(format!("dep.{}", n), dir);
    }
    Ok(())
}

fn expand_writes(cfg: &mut Config) -> Result<()> {
    let writes: Vec<_> = cfg.write.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, w) in writes {
        let name = format!("write-{}", n);
        if taken(cfg, &name) {
            return err(format!("write {} makes {}, which the file already defines", n, name));
        }
        let mode = w.mode.clone().unwrap_or_else(|| "644".to_string());
        cfg.step.insert(
            name,
            Step {
                run: vec![vec![exe_path()?, "write".to_string(), "--to".to_string(), w.to.clone(), "--mode".to_string(), mode, "--content".to_string(), w.content.clone()]],
                outputs: vec![w.to.clone()],
                needs: w.needs.clone(),
                ..Default::default()
            },
        );
    }
    Ok(())
}

fn expand_tests(cfg: &mut Config) -> Result<()> {
    if cfg.test.is_empty() {
        return Ok(());
    }
    if cfg.test.values().any(|t| t.run.is_none()) {
        cfg.slice.entry("host".to_string()).or_insert_with(host_slice);
    }
    let tests: Vec<(String, Test)> = cfg.test.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut runs = Vec::new();
    for (n, t) in tests {
        if !valid_name(&n) {
            return err(format!("test {}: bad name", n));
        }
        let (target, run) = (format!("test-{}", n), format!("run-{}", n));
        for x in [&target, &run] {
            if taken(cfg, x) {
                return err(format!("test {} makes {}, which the file already defines", n, x));
            }
        }
        // a script runs as it is, a program is built first
        let (argv, needs) = if let Some(cmd) = &t.run {
            if cmd.is_empty() || !t.src.is_empty() {
                return err(format!("test {}: run is a command and excludes src", n));
            }
            (cmd.clone(), t.needs.clone())
        } else {
            if t.src.is_empty() {
                return err(format!("test {}: no sources", n));
            }
            let mut cflags: Vec<String> = ["-Wall", "-Wextra"].iter().map(|s| s.to_string()).collect();
            let mut ldflags = Vec::new();
            if let Some(s) = &t.sanitize {
                if !s.split(',').all(|p| ["address", "undefined", "thread", "leak"].contains(&p)) {
                    return err(format!("test {}: sanitize takes address, undefined, thread or leak, joined with commas", n));
                }
                cflags.extend([format!("-fsanitize={}", s), "-fno-omit-frame-pointer".to_string()]);
                ldflags.push(format!("-fsanitize={}", s));
            }
            cflags.extend(t.cflags.iter().cloned());
            ldflags.extend(t.ldflags.iter().cloned());
            ldflags.extend(t.libs.iter().map(|l| format!("-l{}", l)));
            let out = format!("build/host/{}", target);
            cfg.target.insert(
                target.clone(),
                Target {
                    slices: vec!["host".to_string()],
                    src: vec![SrcGroup { files: t.src.clone(), cflags: Vec::new(), exclude: Vec::new() }],
                    cflags,
                    ldflags,
                    link: LinkKind::Exe,
                    output: Some(out.clone()),
                    needs: t.needs.clone(),
                    ..Default::default()
                },
            );
            // the program runs in its own directory, so the path to it cannot be relative
            let mut argv = vec![if t.cwd.is_some() { format!("${{root}}/{}", out) } else { out }];
            argv.extend(t.args.iter().cloned());
            (argv, vec![target])
        };
        cfg.step.insert(
            run.clone(),
            Step { run: vec![argv], needs, env: t.env.clone(), inputs: t.inputs.clone(), cwd: t.cwd.clone(), stamp: Some(format!("build/stamps/{}", run)), ..Default::default() },
        );
        runs.push(run);
    }
    if !taken(cfg, "test") {
        cfg.step.insert("test".to_string(), Step { run: vec![vec!["true".to_string()]], needs: runs, stamp: Some("build/stamps/test".to_string()), ..Default::default() });
    }
    Ok(())
}

fn glob_sources(root: &Path, what: &str, patterns: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for p in patterns {
        if !p.contains(['*', '?', '[']) {
            if !root.join(p).is_file() {
                return err(format!("{}: source {} not found", what, p));
            }
            out.push(p.clone());
            continue;
        }
        let mut hits: Vec<String> = glob::glob(&root.join(p).to_string_lossy())
            .map_err(|e| Error(format!("{}: pattern {}: {}", what, p, e)))?
            .flatten()
            .map(|h| h.strip_prefix(root).unwrap_or(&h).to_string_lossy().into_owned())
            .collect();
        if hits.is_empty() {
            return err(format!("{}: pattern {} matches nothing", what, p));
        }
        hits.sort();
        out.extend(hits);
    }
    Ok(out)
}

// compiles with -fsyntax-only against another sdk: the old sdk has no declaration for a
// call newer than the deployment target, so the compile fails where the app would crash
fn expand_checks(cfg: &mut Config, root: &Path) -> Result<()> {
    let checks: Vec<(String, Check)> = cfg.check.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, c) in checks {
        let name = format!("check-{}", n);
        if taken(cfg, &name) {
            return err(format!("check {} makes {}, which the file already defines", n, name));
        }
        let sl = cfg.slice.get(&c.slice).ok_or_else(|| Error(format!("check {}: unknown slice {}", n, c.slice)))?.clone();
        let sources = glob_sources(root, &format!("check {}", n), &c.src)?;
        let subst = |s: &str| {
            let mut v = s.replace("${SDK}", &c.sdk);
            for (k, val) in &sl.vars {
                if k != "SDK" {
                    v = v.replace(&format!("${{{}}}", k), val);
                }
            }
            v
        };
        let mut argv = vec![subst(&sl.cc)];
        argv.extend(sl.flags.iter().map(|f| subst(f)));
        argv.extend(c.cflags.iter().cloned());
        argv.extend(["-fsyntax-only".to_string(), "-ferror-limit=0".to_string()]);
        argv.extend(sources);
        cfg.step.insert(
            name.clone(),
            Step { run: vec![argv], needs: c.needs.clone(), inputs: c.headers.clone(), stamp: Some(format!("build/stamps/{}", name)), ..Default::default() },
        );
    }
    Ok(())
}

struct Built {
    // the file the product leaves in build/, a fat output or the only slice
    product: String,
    verify: Option<String>,
    last: String,
}

pub fn expand(cfg: &mut Config, root: &Path) -> Result<()> {
    expand_deps(cfg)?;
    expand_writes(cfg)?;
    expand_images(cfg, root)?;
    expand_project(cfg, root)?;
    expand_tests(cfg)?;
    expand_checks(cfg, root)?;
    Ok(())
}

// the names ios 5 to 15 look for in a bundle
const IOS_ICONS: [(&str, u32); 11] = [
    ("Icon.png", 57),
    ("Icon@2x.png", 114),
    ("Icon-57.png", 57),
    ("Icon-57@2x.png", 114),
    ("Icon-60@2x.png", 120),
    ("Icon-60@3x.png", 180),
    ("Icon-72.png", 72),
    ("Icon-72@2x.png", 144),
    ("Icon-76.png", 76),
    ("Icon-76@2x.png", 152),
    ("Icon-83.5@2x.png", 167),
];

fn scale_suffix(scale: u32) -> String {
    if scale == 1 { String::new() } else { format!("@{}x", scale) }
}

fn color_ok(what: &str, c: &str) -> Result<()> {
    crate::image::parse_color(c).map(|_| ()).map_err(|e| Error(format!("{}: {}", what, e)))
}

fn fraction_ok(what: &str, r: Option<f64>) -> Result<()> {
    match r {
        Some(v) if !(0.0..=0.5).contains(&v) => err(format!("{}: round is a fraction of the width between 0 and 0.5, got {}", what, v)),
        _ => Ok(()),
    }
}

// each generator becomes one step whose command is `meow img` with a job per file. the
// outputs are files that live in the source tree, so the step keeps them until the new
// ones are written
fn image_step(cfg: &mut Config, name: String, jobs: Vec<Vec<String>>, outputs: Vec<String>, needs: Vec<String>) -> Result<()> {
    if taken(cfg, &name) {
        return err(format!("{} is already defined", name));
    }
    let mut argv = vec![exe_path()?, "img".to_string()];
    for j in jobs {
        argv.extend(j);
    }
    cfg.step.insert(name, Step { run: vec![argv], outputs, needs, keep: true, ..Default::default() });
    Ok(())
}

fn expand_images(cfg: &mut Config, root: &Path) -> Result<()> {
    let s = |v: &str| v.to_string();
    let icons: Vec<_> = cfg.icons.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, i) in icons {
        let what = format!("icons {}", n);
        fraction_ok(&what, i.round)?;
        let mut sizes: Vec<(String, u32)> = match i.preset.as_deref() {
            Some("ios") => IOS_ICONS.iter().map(|(a, b)| (a.to_string(), *b)).collect(),
            Some(other) => return err(format!("{}: unknown preset {}, use ios", what, other)),
            None => Vec::new(),
        };
        for (name, side) in &i.sizes {
            sizes.retain(|(k, _)| k != name);
            sizes.push((name.clone(), *side));
        }
        if sizes.is_empty() {
            return err(format!("{}: give a preset or sizes", what));
        }
        let drop_alpha = i.drop_alpha.unwrap_or(i.round.is_none());
        let (mut jobs, mut outputs) = (Vec::new(), Vec::new());
        for (name, side) in sizes {
            if side == 0 || side > 4096 {
                return err(format!("{}: size {} for {} is out of range", what, side, name));
            }
            let to = format!("{}/{}", i.out.trim_end_matches('/'), name);
            let mut j = vec![s("--job"), s("--from"), i.master.clone(), s("--to"), to.clone(), s("--size"), format!("{0}x{0}", side)];
            if drop_alpha {
                j.push(s("--drop-alpha"));
            }
            if let Some(r) = i.round {
                j.extend([s("--round"), r.to_string()]);
            }
            if let Some(o) = &i.overlay {
                if i.overlay_sizes.is_empty() || i.overlay_sizes.contains(&side) {
                    j.extend([s("--overlay"), o.clone()]);
                    if i.round.is_some() {
                        j.push(s("--overlay-keep-alpha"));
                    }
                }
            }
            jobs.push(j);
            outputs.push(to);
        }
        image_step(cfg, format!("icons-{}", n), jobs, outputs, i.needs.clone())?;
    }

    let launches: Vec<_> = cfg.launch.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, l) in launches {
        let what = format!("launch {}", n);
        color_ok(&what, &l.background)?;
        fraction_ok(&what, l.icon_round)?;
        if l.images.is_empty() {
            return err(format!("{}: no images", what));
        }
        if l.icon.is_some() != l.icon_points.is_some() {
            return err(format!("{}: icon and icon_points go together", what));
        }
        let (mut jobs, mut outputs) = (Vec::new(), Vec::new());
        for im in &l.images {
            let scale = im.scale.unwrap_or(1);
            if im.width == 0 || im.height == 0 || scale == 0 || im.width * scale > 8192 || im.height * scale > 8192 {
                return err(format!("{}: {} has a size out of range", what, im.name));
            }
            let to = format!("{}/{}", l.out.trim_end_matches('/'), im.name);
            let mut j = vec![s("--job"), s("--solid"), format!("{}x{}", im.width * scale, im.height * scale), s("--background"), l.background.clone(), s("--to"), to.clone()];
            if let (Some(icon), Some(points)) = (&l.icon, l.icon_points) {
                j.extend([s("--paste"), icon.clone(), s("--paste-side"), (points * scale).to_string()]);
                if let Some(r) = l.icon_round {
                    j.extend([s("--paste-round"), r.to_string()]);
                }
            }
            jobs.push(j);
            outputs.push(to);
        }
        image_step(cfg, format!("launch-{}", n), jobs, outputs, l.needs.clone())?;
    }

    let glyphs: Vec<_> = cfg.glyphs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, g) in glyphs {
        let what = format!("glyphs {}", n);
        if let Some(c) = &g.color {
            color_ok(&what, c)?;
        }
        let mut files: Vec<(String, String)> = Vec::new();
        for f in glob_sources(root, &what, &g.src)? {
            let stem = Path::new(&f).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            files.push((f, stem));
        }
        for it in &g.items {
            if !root.join(&it.from).is_file() {
                return err(format!("{}: {} not found", what, it.from));
            }
            files.push((it.from.clone(), it.name.clone()));
        }
        if files.is_empty() {
            return err(format!("{}: give src or items", what));
        }
        let size = g.size.unwrap_or(24);
        let scales = if g.scales.is_empty() { vec![1, 2] } else { g.scales.clone() };
        if size == 0 || scales.iter().any(|&k| k == 0 || size * k > 4096) {
            return err(format!("{}: size or scale out of range", what));
        }
        let (mut jobs, mut outputs) = (Vec::new(), Vec::new());
        for (from, name) in &files {
            for &k in &scales {
                let to = format!("{}/{}{}{}{}.png", g.out.trim_end_matches('/'), g.prefix, name, g.suffix, scale_suffix(k));
                let mut j = vec![s("--svg"), s("--from"), from.clone(), s("--to"), to.clone(), s("--size"), format!("{0}x{0}", size * k)];
                if let Some(c) = &g.color {
                    j.extend([s("--color"), c.clone()]);
                }
                if outputs.contains(&to) {
                    return err(format!("{}: two glyphs write {}", what, to));
                }
                jobs.push(j);
                outputs.push(to);
            }
        }
        image_step(cfg, format!("glyphs-{}", n), jobs, outputs, g.needs.clone())?;
    }

    let images: Vec<_> = cfg.image.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, im) in images {
        let what = format!("image {}", n);
        if let Some(c) = &im.background {
            color_ok(&what, c)?;
        }
        fraction_ok(&what, im.round)?;
        if im.size.is_some() && im.fit.is_some() {
            return err(format!("{}: size and fit exclude each other", what));
        }
        let scales = if im.scales.is_empty() { vec![1] } else { im.scales.clone() };
        let (mut jobs, mut outputs) = (Vec::new(), Vec::new());
        for &k in &scales {
            if k == 0 {
                return err(format!("{}: scale 0", what));
            }
            let to = match (k, im.to.rfind('.')) {
                (1, _) => im.to.clone(),
                (_, Some(dot)) => format!("{}{}{}", &im.to[..dot], scale_suffix(k), &im.to[dot..]),
                (_, None) => format!("{}{}", im.to, scale_suffix(k)),
            };
            let dim = |d: [u32; 2]| format!("{}x{}", d[0] * k, d[1] * k);
            let mut j = vec![s("--job"), s("--from"), im.from.clone(), s("--to"), to.clone()];
            if let Some(t) = im.crop_alpha {
                j.extend([s("--crop-alpha"), t.to_string()]);
            }
            if im.drop_alpha {
                j.push(s("--drop-alpha"));
            }
            if let Some(r) = im.round {
                j.extend([s("--round"), r.to_string()]);
            }
            if let Some(d) = im.size {
                j.extend([s("--size"), dim(d)]);
            }
            if let Some(d) = im.fit {
                j.extend([s("--fit"), dim(d)]);
            }
            if let Some(d) = im.canvas {
                j.extend([s("--canvas"), dim(d)]);
            }
            if let Some(c) = &im.background {
                j.extend([s("--background"), c.clone()]);
            }
            jobs.push(j);
            outputs.push(to);
        }
        image_step(cfg, format!("image-{}", n), jobs, outputs, im.needs.clone())?;
    }
    Ok(())
}

// the rust target of each slice, arm64 is the only one with a std that rust ships
struct RustTarget {
    arch: &'static str,
    // the target name, and the name of the spec file for a target rust no longer ships
    triple: &'static str,
    spec: Option<&'static str>,
}

// armv7 has no std from rust since the target was dropped. its spec is the one of armv7s-apple-ios
// with the cpu and floating point features of a cortex-a8, and cargo builds the std with it
const RUST_ARM64_MIN_IOS: ios::Version = (8, 0);

const ARMV7_IOS_SPEC: &str = include_str!("../../rust/armv7-apple-ios.json");

const RUST_TARGETS: [RustTarget; 2] = [
    RustTarget { arch: "arm64", triple: "aarch64-apple-ios", spec: None },
    RustTarget { arch: "armv7", triple: "armv7-apple-ios", spec: Some(ARMV7_IOS_SPEC) },
];

// go builds a c archive for arm64 with a current go and for armv7 with go 1.14, the last release
// that had ios/arm. there is no go for arm64e, an arm64 archive does not link into an arm64e binary
struct GoTarget {
    arch: &'static str,
    goos: &'static str,
    goarch: &'static str,
    goarm: Option<&'static str>,
    // the variable that names the go command of the slice
    go: &'static str,
    // the newest minor release of go 1 that the slice's go understands
    max_minor: Option<u32>,
    // flags only this go knows: -buildvcs=false keeps a build inside a git checkout from asking git
    flags: &'static [&'static str],
}

const GO_TARGETS: [GoTarget; 2] = [
    GoTarget { arch: "arm64", goos: "ios", goarch: "arm64", goarm: None, go: "${MEOW_GO}", max_minor: None, flags: &["-buildvcs=false"] },
    GoTarget { arch: "armv7", goos: "darwin", goarch: "arm", goarm: Some("7"), go: "${MEOW_GO_ARMV7}", max_minor: Some(14), flags: &[] },
];

fn go_target(arch: &str) -> Option<&'static GoTarget> {
    GO_TARGETS.iter().find(|t| t.arch == arch)
}

// the `go 1.21` line of a go.mod
fn go_mod_minor(text: &str) -> Option<u32> {
    let line = text.lines().map(str::trim).find(|l| l.starts_with("go ") || l.starts_with("go\t"))?;
    let v = line[2..].trim();
    let mut it = v.split('.');
    if it.next()? != "1" {
        return None;
    }
    it.next()?.parse().ok()
}

fn rust_target(arch: &str) -> Option<&'static RustTarget> {
    RUST_TARGETS.iter().find(|t| t.arch == arch)
}

// the first ios release with the swift runtime in the system, older ones need it bundled
const SWIFT_MIN_IOS: ios::Version = (12, 2);

// swift files of a product are compiled by one swiftc run per slice, the whole module at
// once, into one object and an objective-c header. a step makes them, the target links the
// object. returns the files left for the c compiler and what the target gets besides
struct SwiftPlan {
    steps: Vec<String>,
    objects: Vec<String>,
    cflags: BTreeMap<String, Vec<String>>,
    ldflags: BTreeMap<String, Vec<String>>,
}

fn expand_swift(cfg: &mut Config, root: &Path, product: &str, pr: &Product, archs: &[String], min: ios::Version, files: &[String]) -> Result<SwiftPlan> {
    if min < SWIFT_MIN_IOS {
        return err(format!(
            "product {}: swift needs min_ios {} or later, the swift runtime is part of ios from then on and this toolchain cannot bundle it",
            product,
            ios::text(SWIFT_MIN_IOS)
        ));
    }
    // min_ios of 12.2 or later already leaves no device for armv6 and armv7
    let module = pr.swift_module.clone().unwrap_or_else(|| product.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect());
    if module.is_empty() || module.starts_with(|c: char| c.is_ascii_digit()) || !module.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return err(format!("product {}: {:?} is not a swift module name, set swift_module", product, module));
    }
    if let Some(h) = &pr.bridging_header {
        if !root.join(h).is_file() {
            return err(format!("product {}: bridging_header {} not found", product, h));
        }
    }
    let has_main = files.iter().any(|f| Path::new(f).file_name().is_some_and(|n| n == "main.swift"));
    let mut plan = SwiftPlan { steps: Vec::new(), objects: Vec::new(), cflags: BTreeMap::new(), ldflags: BTreeMap::new() };
    for a in archs {
        let dir = format!("build/swift/{}/{}", product, a);
        let obj = format!("{}/{}.o", dir, module);
        let hdr = format!("{}/{}-Swift.h", dir, module);
        let step = format!("swift-{}-{}", product, a);
        if taken(cfg, &step) {
            return err(format!("product {} makes {}, which the file already defines", product, step));
        }
        let mut argv: Vec<String> = ["${MEOW_SWIFTC}", "-c", "-wmo", "-O", "-target"].iter().map(|x| x.to_string()).collect();
        argv.push(format!("{}-apple-ios{}", a, ios::text(min)));
        argv.extend(
            ["-sdk", "${MEOW_SWIFT_SDK}", "-resource-dir", "${MEOW_SWIFT_RESOURCES}", "-module-name"]
                .iter()
                .map(|x| x.to_string()),
        );
        argv.push(module.clone());
        argv.extend(["-emit-objc-header", "-emit-objc-header-path"].iter().map(|x| x.to_string()));
        argv.push(hdr.clone());
        argv.push("-emit-dependencies".to_string());
        if !has_main {
            argv.push("-parse-as-library".to_string());
        }
        if let Some(h) = &pr.bridging_header {
            argv.extend(["-import-objc-header".to_string(), h.clone()]);
        }
        argv.extend(pr.swiftflags.iter().cloned());
        argv.extend(files.iter().cloned());
        argv.extend(["-o".to_string(), obj.clone()]);
        cfg.step.insert(
            step.clone(),
            Step { run: vec![argv], outputs: vec![obj.clone(), hdr], depfile: Some(format!("{}/{}.d", dir, module)), ..Default::default() },
        );
        plan.steps.push(step);
        plan.cflags.insert(a.clone(), vec![format!("-I{}", dir)]);
        // the swift runtime and its stubs come from the sdk the swift code was built against,
        // the link takes that sysroot, the c objects only need the system libraries from it
        plan.ldflags.insert(
            a.clone(),
            ["-isysroot", "${MEOW_SWIFT_SDK}", "-L${MEOW_SWIFT_RESOURCES}/iphoneos", "-L${MEOW_SWIFT_SDK}/usr/lib/swift", "-Wl,-rpath,/usr/lib/swift"]
                .iter()
                .map(|x| x.to_string())
                .collect(),
        );
    }
    plan.objects.push(format!("build/swift/{}/${{slice}}/{}.o", product, module));
    Ok(plan)
}

const LOGOS_GENERATORS: [&str; 3] = ["MobileSubstrate", "libhooker", "internal"];

// .x and .xm sources go through `meow logos` first and the generated file is compiled,
// the step for it is one per source so a changed file only regenerates itself.
// returns the sources to compile, the steps that must finish first and the directories of
// the original files, a generated file in build/ finds its quoted includes through them
fn expand_logos(cfg: &mut Config, root: &Path, product: &str, sources: Vec<String>, generator: &str) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
    let ext_of = |f: &str| Path::new(f).extension().and_then(|e| e.to_str()).map(str::to_string).unwrap_or_default();
    let mut files: Vec<String> = Vec::new();
    for pat in sources {
        let ext = ext_of(&pat);
        let glob = pat.contains(['*', '?', '[']);
        if matches!(ext.as_str(), "x" | "xm" | "swift") || (glob && (ext.starts_with('x') || ext.starts_with('s') || ext.contains('*'))) {
            files.extend(glob_sources(root, &format!("product {}", product), std::slice::from_ref(&pat))?);
        } else {
            files.push(pat);
        }
    }
    if !files.iter().any(|f| matches!(ext_of(f).as_str(), "x" | "xm")) {
        return Ok((files, Vec::new(), Vec::new()));
    }
    if !LOGOS_GENERATORS.contains(&generator) {
        return err(format!("product {}: logos backend {:?}, use {}", product, generator, LOGOS_GENERATORS.join(", ")));
    }
    let exe = exe_path()?;
    let mut out = Vec::new();
    let mut steps = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    for f in files {
        let suffix = match ext_of(&f).as_str() {
            "x" => "m",
            "xm" => "mm",
            _ => {
                out.push(f);
                continue;
            }
        };
        let generated = format!("build/logos/{}/{}.{}", product, f, suffix);
        let step = format!("logos-{}-{}", product, steps.len() + 1);
        if taken(cfg, &step) {
            return err(format!("product {} makes {}, which the file already defines", product, step));
        }
        // the version is part of the command line, so a new meow regenerates every file
        cfg.step.insert(
            step.clone(),
            Step {
                run: vec![vec![
                    exe.clone(),
                    "logos".to_string(),
                    "-o".to_string(),
                    generated.clone(),
                    "-c".to_string(),
                    format!("generator={}", generator),
                    "-c".to_string(),
                    format!("meow={}", env!("CARGO_PKG_VERSION")),
                    f.clone(),
                ]],
                outputs: vec![generated.clone()],
                ..Default::default()
            },
        );
        steps.push(step);
        let dir = Path::new(&f).parent().map(|d| d.to_string_lossy().into_owned()).filter(|d| !d.is_empty()).unwrap_or_else(|| ".".to_string());
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
        out.push(generated);
    }
    Ok((out, steps, dirs))
}

fn expand_project(cfg: &mut Config, root: &Path) -> Result<()> {
    let Some(p) = cfg.project.clone() else {
        if cfg.package.is_some() || !cfg.product.is_empty() {
            return err("[package] and [product.*] need a [project]");
        }
        return Ok(());
    };
    if !valid_name(&p.name) {
        return err(format!("project: bad name {:?}, use letters, digits, . _ -", p.name));
    }
    let multi = !cfg.product.is_empty();
    // one product: the project table is the product. several: the project holds what they share
    let products: Vec<(String, Product)> = if multi {
        if p.kind.is_some() {
            return err("project: kind and [product.*] tables exclude each other");
        }
        cfg.product.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    } else {
        let kind = p.kind.ok_or_else(|| Error("project: give a kind, or [product.*] tables".to_string()))?;
        vec![(
            p.name.clone(),
            Product {
                kind: Some(kind),
                archs: p.archs.clone(),
                src: p.src.clone().unwrap_or_default(),
                cflags: p.cflags.clone(),
                ldflags: p.ldflags.clone(),
                frameworks: p.frameworks.clone(),
                libs: p.libs.clone(),
                arc: Some(p.arc),
                strip: p.strip,
                entitlements: p.entitlements.clone(),
                info_plist: p.info_plist.clone(),
                resources: p.resources.clone(),
                bundle: p.bundle.clone(),
                filter_bundles: p.filter_bundles.clone(),
                filter_executables: p.filter_executables.clone(),
                install: p.install.clone(),
                bundle_extension: p.bundle_extension.clone(),
                plist: p.plist.clone(),
                logos: p.logos.clone(),
                crate_dir: p.crate_dir.clone(),
                go_dir: p.go_dir.clone(),
                go_pkg: p.go_pkg.clone(),
                go_tags: p.go_tags.clone(),
                goflags: p.goflags.clone(),
                go_inputs: p.go_inputs.clone(),
                swiftflags: p.swiftflags.clone(),
                swift_module: p.swift_module.clone(),
                bridging_header: p.bridging_header.clone(),
                features: p.features.clone(),
                rust_inputs: p.rust_inputs.clone(),
                ..Default::default()
            },
        )]
    };
    let shared = |list: &Vec<String>| if multi { list.clone() } else { Vec::new() };

    let rootless = p.scheme == Scheme::Rootless;
    let default_archs: Vec<String> = p.archs.clone().unwrap_or_else(|| {
        if rootless { vec!["arm64".to_string()] } else { vec!["armv7".to_string(), "arm64".to_string()] }
    });
    let mut product_archs: Vec<Vec<String>> = Vec::new();
    for (pn, pr) in &products {
        let archs = pr.archs.clone().unwrap_or_else(|| default_archs.clone());
        if archs.is_empty() {
            return err(format!("product {}: archs is empty", pn));
        }
        if pr.kind == Some(Kind::Go) {
            for a in &archs {
                if go_target(a).is_none() {
                    let why = if a == "arm64e" { ", an arm64 archive does not link into an arm64e binary" } else { ", go never ran there" };
                    return err(format!("product {}: go is built for arm64 and armv7, not {}{}, give the package its own archs", pn, a, why));
                }
            }
        }
        if pr.kind == Some(Kind::Rust) {
            for a in &archs {
                if rust_target(a).is_none() {
                    return err(format!("product {}: rust is built for arm64 and armv7, not {}, give the crate its own archs", pn, a));
                }
            }
        }
        for (i, a) in archs.iter().enumerate() {
            if !ARCHS.contains(&a.as_str()) {
                return err(format!("product {}: unknown arch {}, use {}", pn, a, ARCHS.join(", ")));
            }
            if archs[..i].contains(a) {
                return err(format!("product {}: arch {} is listed twice", pn, a));
            }
            if rootless && (a == "armv7" || a == "armv6") {
                return err(format!("product {}: a rootless package cannot hold {}, rootless systems are arm64 only", pn, a));
            }
        }
        product_archs.push(archs);
    }
    let armv6 = product_archs.iter().flatten().any(|a| a == "armv6");
    let uses_rust = products.iter().any(|(_, pr)| pr.kind == Some(Kind::Rust));
    if let Some(v) = &p.sdk {
        if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit() || c == '.') || v.starts_with('.') || v.ends_with('.') {
            return err(format!("project: bad sdk {:?}, use a version like 9.3", v));
        }
    }
    let min_text = p.min_ios.clone().unwrap_or_else(|| (if armv6 { DEFAULT_MIN_IOS_ARMV6 } else { DEFAULT_MIN_IOS }).to_string());
    let min = ios::parse(&min_text).ok_or_else(|| Error(format!("project: bad min_ios {:?}, use 5.0", min_text)))?;
    for ((pn, _), archs) in products.iter().zip(&product_archs) {
        for a in archs {
            ios::check_min(a, min).map_err(|e| Error(format!("product {}: {}", pn, e)))?;
        }
    }
    for archs in &product_archs {
        for a in archs {
            if !cfg.slice.contains_key(a) {
                cfg.slice.insert(a.clone(), slice_def(a, min, p.sdk.as_deref(), uses_rust));
            }
        }
    }

    // generated names must be free before anything is inserted
    let deb_name = format!("{}-deb", p.name);
    let mut mine: Vec<String> = Vec::new();
    for ((pn, pr), archs) in products.iter().zip(&product_archs) {
        if pr.kind == Some(Kind::Go) {
            for a in archs {
                mine.push(format!("go-{}-{}", pn, a));
            }
            continue;
        }
        if pr.kind == Some(Kind::Rust) {
            for a in archs {
                mine.push(format!("rust-{}-{}", pn, a));
                let spec_step = format!("rust-target-{}", a);
                if rust_target(a).is_some_and(|t| t.spec.is_some()) && !mine.contains(&spec_step) {
                    mine.push(spec_step);
                }
            }
            continue;
        }
        mine.push(pn.clone());
        if pr.kind != Some(Kind::Objects) {
            mine.push(format!("{}-fat", pn));
            mine.push(format!("verify-{}", pn));
        }
        if pr.kind == Some(Kind::App) {
            mine.push(format!("{}-ipa", pn));
        }
    }
    mine.push(deb_name.clone());
    mine.push("version-check".to_string());
    mine.push("sdk-armv6".to_string());
    let mut seen = std::collections::BTreeSet::new();
    for n in &mine {
        if !seen.insert(n.clone()) {
            return err(format!("the project makes {} twice", n));
        }
        if n != "sdk-armv6" && taken(cfg, n) {
            return err(format!("project {} makes {}, which the file already defines", p.name, n));
        }
    }

    if armv6 && !cfg.step.contains_key("sdk-armv6") {
        cfg.step.insert(
            "sdk-armv6".to_string(),
            Step {
                run: vec![vec![
                    exe_path()?,
                    "sdk-thin".to_string(),
                    "--arch".to_string(),
                    "armv6".to_string(),
                    "--from".to_string(),
                    p.sdk.as_deref().map_or("${MEOW_SDK_ARMV6}".to_string(), |v| format!("${{MEOW_SDK_{}}}", v.replace('.', "_"))),
                    "--to".to_string(),
                    "${MEOW_CACHE_DIR}/sdk-armv6".to_string(),
                ]],
                outputs: vec![format!("${{MEOW_CACHE_DIR}}/sdk-armv6/{}", crate::sdkthin::STAMP)],
                keep: true,
                ..Default::default()
            },
        );
    }

    let prefix = if rootless { "var/jb" } else { "" };
    let join = |sub: &str| if prefix.is_empty() { sub.to_string() } else { format!("{}/{}", prefix, sub) };

    let mut files: Vec<DebFile> = Vec::new();
    let mut trees: Vec<DebTree> = Vec::new();
    let mut data: Vec<DebData> = Vec::new();
    let mut verifies: Vec<String> = Vec::new();
    let mut apps: Vec<(String, String, String, Option<String>, String)> = Vec::new();
    let mut built: Vec<Built> = Vec::new();
    let mut plist_steps: Vec<String> = Vec::new();

    // go packages: one step per slice builds the c archive and its header
    let mut go_libs: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for ((pn, pr), archs) in products.iter().zip(&product_archs) {
        if pr.kind != Some(Kind::Go) {
            continue;
        }
        if !valid_name(pn) {
            return err(format!("product {}: bad name", pn));
        }
        let dir = pr.go_dir.clone().unwrap_or_else(|| ".".to_string());
        let dir = dir.trim_end_matches('/').to_string();
        let gomod = if dir == "." { "go.mod".to_string() } else { format!("{}/go.mod", dir) };
        let text = std::fs::read_to_string(root.join(&gomod)).map_err(|e| Error(format!("product {}: {}: {}", pn, gomod, e)))?;
        let wanted = go_mod_minor(&text);
        let pkg = pr.go_pkg.clone().unwrap_or_else(|| ".".to_string());
        let lib = format!("lib{}.a", pn);
        let header = format!("lib{}.h", pn);
        let mut inputs: Vec<String> = Vec::new();
        for pat in ["**/*.go", "**/*.[chsS]", "go.mod", "go.sum"] {
            inputs.push(if dir == "." { pat.to_string() } else { format!("{}/{}", dir, pat) });
        }
        inputs.extend(pr.go_inputs.iter().cloned());
        for a in archs {
            let target = go_target(a).expect("archs were checked above");
            if let (Some(max), Some(have)) = (target.max_minor, wanted) {
                if have > max {
                    return err(format!(
                        "product {}: {} says go 1.{}, but the go for {} is 1.{}, the last release that had ios/arm. lower the go line or build only arm64",
                        pn, gomod, have, a, max
                    ));
                }
            }
            let out_dir = format!("build/go/{}/{}", pn, a);
            let sdk = cfg.slice[a].vars.get("SDK").cloned().unwrap_or_default();
            // cgo gets the compiler of the slice, its flags and its sdk
            let cflags = cfg.slice[a].flags.iter().map(|f| f.replace("${SDK}", &sdk)).collect::<Vec<_>>().join(" ");
            let mut argv: Vec<String> = vec![target.go.to_string(), "build".to_string(), "-buildmode=c-archive".to_string(), "-trimpath".to_string()];
            // the archive needs a ranlib that writes the table of contents the darwin linker reads
            argv.extend(["-ldflags".to_string(), "-extar ${MEOW_AR}".to_string()]);
            if !pr.go_tags.is_empty() {
                argv.extend(["-tags".to_string(), pr.go_tags.join(",")]);
            }
            argv.extend(target.flags.iter().map(|f| f.to_string()));
            argv.extend(pr.goflags.iter().cloned());
            argv.extend(["-o".to_string(), format!("${{root}}/{}/{}", out_dir, lib), pkg.clone()]);
            let mut env = BTreeMap::from([
                ("GOOS".to_string(), target.goos.to_string()),
                ("GOARCH".to_string(), target.goarch.to_string()),
                ("CGO_ENABLED".to_string(), "1".to_string()),
                ("CC".to_string(), format!("${{MEOW_CLANG}} {}", cflags)),
                ("CGO_CFLAGS".to_string(), cflags.clone()),
                ("CGO_LDFLAGS".to_string(), cflags),
                // two go releases do not share a build cache, and a go.mod must not fetch another toolchain
                ("GOCACHE".to_string(), format!("${{MEOW_CACHE_DIR}}/go-build/{}", a)),
                ("GOTOOLCHAIN".to_string(), "local".to_string()),
            ]);
            if let Some(arm) = target.goarm {
                env.insert("GOARM".to_string(), arm.to_string());
            }
            cfg.step.insert(
                format!("go-{}-{}", pn, a),
                Step {
                    run: vec![argv],
                    inputs: inputs.clone(),
                    outputs: vec![format!("{}/{}", out_dir, lib), format!("{}/{}", out_dir, header)],
                    env,
                    cwd: if dir == "." { None } else { Some(dir.clone()) },
                    keep: true,
                    ..Default::default()
                },
            );
        }
        go_libs.insert(pn.clone(), (pn.clone(), archs.clone()));
    }

    // cargo crates: one step per slice builds the static library that other products link
    let mut rust_libs: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for ((pn, pr), archs) in products.iter().zip(&product_archs) {
        if pr.kind != Some(Kind::Rust) {
            continue;
        }
        if !valid_name(pn) {
            return err(format!("product {}: bad name", pn));
        }
        let dir = pr.crate_dir.clone().unwrap_or_else(|| ".".to_string());
        let dir = dir.trim_end_matches('/').to_string();
        let manifest = if dir == "." { "Cargo.toml".to_string() } else { format!("{}/Cargo.toml", dir) };
        let text = std::fs::read_to_string(root.join(&manifest)).map_err(|e| Error(format!("product {}: {}: {}", pn, manifest, e)))?;
        let table: toml::Table = text.parse().map_err(|e| Error(format!("product {}: {}: {}", pn, manifest, e)))?;
        let name_of = |t: &str| table.get(t).and_then(|x| x.get("name")).and_then(|n| n.as_str()).map(str::to_string);
        let Some(crate_name) = name_of("lib").or_else(|| name_of("package")) else {
            return err(format!("product {}: {} has no [package] name, name the member crate with crate_dir", pn, manifest));
        };
        let lib = format!("lib{}.a", crate_name.replace('-', "_"));
        let src_glob = if dir == "." { "src/**/*".to_string() } else { format!("{}/src/**/*", dir) };
        let mut inputs = vec![src_glob];
        for extra in ["Cargo.lock", "build.rs"] {
            inputs.push(if dir == "." { extra.to_string() } else { format!("{}/{}", dir, extra) });
        }
        inputs.extend(pr.rust_inputs.iter().cloned());
        for a in archs {
            let target = rust_target(a).expect("archs were checked above");
            let triple = target.triple;
            let target_dir = format!("build/rust/{}/{}/cargo", pn, a);
            let out = format!("{}/{}/release/{}", target_dir, triple, lib);
            let sdk = cfg.slice[a].vars.get("SDK").cloned().unwrap_or_default();
            let mut argv: Vec<String> = ["${MEOW_CARGO}", "rustc", "--manifest-path", &manifest, "--lib", "--release", "--target-dir", &target_dir, "--crate-type", "staticlib"]
                .iter()
                .map(|x| x.to_string())
                .collect();
            let mut needs: Vec<String> = Vec::new();
            let up = triple.replace('-', "_");
            // what the c compiler of a build script gets: the flags of the slice, with its sdk filled in
            let cflags = cfg.slice[a].flags.iter().map(|f| f.replace("${SDK}", &sdk)).collect::<Vec<_>>().join(" ");
            let mut env = BTreeMap::from([
                ("RUSTC".to_string(), "${MEOW_RUSTC}".to_string()),
                ("IPHONEOS_DEPLOYMENT_TARGET".to_string(), ios::text(min)),
                ("SDKROOT".to_string(), sdk.clone()),
                (format!("CC_{}", up), "${MEOW_CLANG}".to_string()),
                (format!("AR_{}", up), "${MEOW_AR}".to_string()),
                (format!("CFLAGS_{}", up), cflags),
            ]);
            match target.spec {
                None => argv.extend(["--target".to_string(), triple.to_string()]),
                Some(spec) => {
                    // a target rust does not ship: its spec is written once and cargo builds the std for it.
                    // build-std is a nightly feature, RUSTC_BOOTSTRAP lets the stable compiler take it
                    let spec_path = format!("build/rust/{}.json", triple);
                    let spec_step = format!("rust-target-{}", a);
                    if !cfg.step.contains_key(&spec_step) {
                        cfg.step.insert(
                            spec_step.clone(),
                            Step {
                                run: vec![vec![exe_path()?, "write".to_string(), "--to".to_string(), spec_path.clone(), "--content".to_string(), spec.to_string()]],
                                outputs: vec![spec_path.clone()],
                                ..Default::default()
                            },
                        );
                    }
                    needs.push(spec_step);
                    argv.extend(["-Zbuild-std=std,panic_abort".to_string(), "-Zjson-target-spec".to_string(), "--target".to_string(), spec_path]);
                    env.insert("RUSTC_BOOTSTRAP".to_string(), "1".to_string());
                }
            }
            if !pr.features.is_empty() {
                argv.extend(["--features".to_string(), pr.features.join(",")]);
            }
            cfg.step.insert(
                format!("rust-{}-{}", pn, a),
                Step { run: vec![argv], inputs: inputs.clone(), outputs: vec![out], env, needs, keep: true, ..Default::default() },
            );
        }
        rust_libs.insert(pn.clone(), (lib, archs.clone()));
    }

    for ((pn, pr), archs) in products.iter().zip(&product_archs) {
        let kind = pr.kind.ok_or_else(|| Error(format!("product {}: kind is required", pn)))?;
        if kind == Kind::Rust || kind == Kind::Go {
            continue;
        }
        if !valid_name(pn) {
            return err(format!("product {}: bad name", pn));
        }
        let exe = matches!(kind, Kind::Tool | Kind::App | Kind::Appex);
        let dylib = matches!(kind, Kind::Tweak | Kind::Library | Kind::Bundle | Kind::Framework);
        let in_bundle = matches!(kind, Kind::App | Kind::Bundle | Kind::Framework | Kind::Appex);
        if !in_bundle && (pr.info_plist.is_some() || pr.resources.is_some() || pr.bundle.is_some()) {
            return err(format!("product {}: info_plist, resources and bundle are for kind app, bundle, framework and appex", pn));
        }
        if kind != Kind::Bundle && pr.bundle_extension.is_some() {
            return err(format!("product {}: bundle_extension is for kind = \"bundle\"", pn));
        }
        if !in_bundle && pr.plist.is_some() {
            return err(format!("product {}: plist is for kind app, bundle, framework and appex", pn));
        }
        if kind != Kind::Tweak && (!pr.filter_bundles.is_empty() || !pr.filter_executables.is_empty()) {
            return err(format!("product {}: filter_bundles and filter_executables are for kind = \"tweak\"", pn));
        }
        if pr.install.is_some() && !matches!(kind, Kind::Tool | Kind::Library | Kind::Bundle | Kind::Framework | Kind::Appex) {
            return err(format!("product {}: install is for tools, libraries, bundles, frameworks and appex", pn));
        }
        if matches!(kind, Kind::Bundle | Kind::Appex) && pr.install.is_none() {
            return err(format!("product {}: a {} needs install, the directory in the package that holds it", pn, if kind == Kind::Bundle { "bundle" } else { "appex" }));
        }
        // the bundle directory the product lives in, relative to the package root
        let bundle_path: Option<String> = if in_bundle {
            let ext = match kind {
                Kind::App => "app",
                Kind::Framework => "framework",
                Kind::Appex => "appex",
                _ => pr.bundle_extension.as_deref().unwrap_or("bundle"),
            };
            if !valid_name(ext) {
                return err(format!("product {}: bad bundle extension {:?}", pn, ext));
            }
            let bundle = pr.bundle.clone().unwrap_or_else(|| pn.clone());
            if !valid_name(&bundle) {
                return err(format!("product {}: bad bundle name {:?}", pn, bundle));
            }
            let parent = match kind {
                Kind::App => "Applications".to_string(),
                Kind::Framework => pr.install.clone().unwrap_or_else(|| "Library/Frameworks".to_string()),
                _ => pr.install.clone().unwrap_or_default(),
            };
            Some(format!("{}/{}.{}", parent.trim_matches('/'), bundle, ext))
        } else {
            None
        };
        let arc = pr.arc.unwrap_or(p.arc);
        if arc {
            ios::check_arc(min).map_err(|e| Error(format!("product {}: {}", pn, e)))?;
        }
        let sources = if pr.src.is_empty() {
            if multi {
                return err(format!("product {}: src is required", pn));
            }
            find_sources(root)?
        } else {
            pr.src.clone()
        };
        if let Some(e) = &pr.entitlements {
            if !root.join(e).is_file() {
                return err(format!("product {}: entitlements file {} not found", pn, e));
            }
        }
        let generator = pr.logos.clone().or_else(|| p.logos.clone()).unwrap_or_else(|| "MobileSubstrate".to_string());
        let (sources, logos_steps, logos_dirs) = expand_logos(cfg, root, pn, sources, &generator)?;
        let (swift_files, sources): (Vec<String>, Vec<String>) = sources.into_iter().partition(|f| f.ends_with(".swift"));
        let swift = if swift_files.is_empty() { None } else { Some(expand_swift(cfg, root, pn, pr, archs, min, &swift_files)?) };

        let out_file = match kind {
            Kind::Tool | Kind::App | Kind::Objects | Kind::Bundle | Kind::Framework | Kind::Appex => pn.clone(),
            Kind::Rust | Kind::Go => unreachable!("rust and go products are handled before this"),
            Kind::Tweak => format!("{}.dylib", pn),
            Kind::Library => format!("lib{}.dylib", pn),
        };
        // where the file lands on the device, relative to the package root
        let installed = match kind {
            Kind::Tool => Some(pr.install.clone().unwrap_or_else(|| format!("usr/bin/{}", pn))),
            Kind::Library => Some(pr.install.clone().unwrap_or_else(|| format!("usr/lib/{}", out_file))),
            Kind::Tweak => Some(format!("Library/MobileSubstrate/DynamicLibraries/{}", out_file)),
            Kind::Bundle | Kind::Framework => bundle_path.as_ref().map(|b| format!("{}/{}", b, pn)),
            _ => None,
        };

        let mut cflags: Vec<String> = vec!["-O2".to_string(), "-Wall".to_string()];
        if dylib {
            cflags.push("-fPIC".to_string());
        }
        if arc {
            cflags.push("-fobjc-arc".to_string());
        }
        // a generated file is compiled from build/, quoted includes still look next to the .x
        for d in &logos_dirs {
            cflags.push(format!("-I{}", d));
        }
        cflags.extend(shared(&p.cflags));
        cflags.extend(pr.cflags.iter().cloned());

        let mut ldflags: Vec<String> = Vec::new();
        if let (true, Some(path)) = (dylib, &installed) {
            ldflags.extend(["-dynamiclib".to_string(), "-install_name".to_string(), format!("/{}", join(path))]);
        }
        if kind == Kind::Appex {
            ldflags.extend(["-e".to_string(), "_NSExtensionMain".to_string()]);
        }
        for f in shared(&p.frameworks).iter().chain(pr.frameworks.iter()) {
            ldflags.extend(["-framework".to_string(), f.clone()]);
        }
        for l in shared(&p.libs).iter().chain(pr.libs.iter()) {
            ldflags.push(format!("-l{}", l));
        }
        ldflags.extend(shared(&p.ldflags));
        ldflags.extend(pr.ldflags.iter().cloned());

        let mut per_slice = BTreeMap::new();
        // armv7 executables link without the default start files against the old sdk
        if exe && archs.iter().any(|a| a == "armv7") {
            per_slice.insert("armv7".to_string(), TargetSlice { ldflags: vec!["-nostartfiles".to_string(), "${MEOW_CRT_ARMV7}".to_string()], ..Default::default() });
        }
        let mut needs = pr.needs.clone();
        needs.extend(logos_steps.iter().cloned());
        let mut objects: Vec<String> = Vec::new();
        if let Some(sw) = &swift {
            for (a, f) in &sw.cflags {
                per_slice.entry(a.clone()).or_default().cflags.extend(f.iter().cloned());
            }
            for (a, f) in &sw.ldflags {
                per_slice.entry(a.clone()).or_default().ldflags.extend(f.iter().cloned());
            }
            objects.extend(sw.objects.iter().cloned());
            needs.push(format!("swift-{}-${{slice}}", pn));
        }
        // a rust crate is linked as its static library on every slice, the rest of uses are objects
        let mut object_uses: Vec<String> = Vec::new();
        let mut uses_rust = false;
        let mut uses_go = false;
        for u in &pr.uses {
            if let Some((_, go_archs)) = go_libs.get(u) {
                uses_go = true;
                for a in archs {
                    if !go_archs.contains(a) {
                        return err(format!("product {} builds {} but the go product {} builds only {}", pn, a, u, go_archs.join(", ")));
                    }
                    let slice = per_slice.entry(a.clone()).or_default();
                    slice.ldflags.push(format!("build/go/{}/{}/lib{}.a", u, a, u));
                    // the generated header is lib<product>.h
                    slice.cflags.push(format!("-Ibuild/go/{}/{}", u, a));
                }
                needs.push(format!("go-{}-${{slice}}", u));
                continue;
            }
            let Some((lib, rust_archs)) = rust_libs.get(u) else {
                object_uses.push(u.clone());
                continue;
            };
            uses_rust = true;
            for a in archs {
                if !rust_archs.contains(a) {
                    return err(format!("product {} builds {} but the rust product {} builds only {}", pn, a, u, rust_archs.join(", ")));
                }
                let triple = rust_target(a).map(|t| t.triple).unwrap_or_default();
                per_slice.entry(a.clone()).or_default().ldflags.push(format!("build/rust/{}/{}/cargo/{}/release/{}", u, a, triple, lib));
            }
            needs.push(format!("rust-{}-${{slice}}", u));
        }
        if uses_go {
            // what the go runtime of ios asks the link for
            ldflags.extend(["-lresolv", "-framework", "Security", "-framework", "CoreFoundation"].iter().map(|x| x.to_string()));
        }
        if uses_rust {
            // what the rust std of ios asks the link for
            ldflags.extend(["-lresolv", "-liconv", "-framework", "Security", "-framework", "CoreFoundation"].iter().map(|x| x.to_string()));
        }
        if archs.iter().any(|a| a == "armv6") {
            needs.push("sdk-armv6".to_string());
        }

        let strip = pr.strip.or(p.strip).unwrap_or(true);
        let multi_arch = archs.len() > 1;
        let finish = kind != Kind::Objects;
        let (t_strip, t_sign, t_ents, f_strip, f_sign, f_ents) = if !finish {
            (false, false, None, false, false, None)
        } else if multi_arch {
            (false, false, None, strip, true, pr.entitlements.clone())
        } else {
            (strip, true, pr.entitlements.clone(), false, false, None)
        };
        cfg.target.insert(
            pn.clone(),
            Target {
                slices: archs.clone(),
                src: vec![SrcGroup { files: sources, cflags: Vec::new(), exclude: Vec::new() }],
                cflags,
                ldflags,
                link: if finish { LinkKind::Exe } else { LinkKind::None },
                output: if finish { Some(format!("build/${{slice}}/{}", out_file)) } else { None },
                objects_from: object_uses,
                objects,
                needs,
                per_slice,
                strip: t_strip,
                sign: t_sign,
                entitlements: t_ents,
                ..Default::default()
            },
        );
        if !finish {
            built.push(Built { product: String::new(), verify: None, last: pn.clone() });
            continue;
        }
        let product = if multi_arch {
            let out = format!("build/{}", out_file);
            cfg.fat.insert(
                format!("{}-fat", pn),
                Fat { from: pn.clone(), lipo: None, output: out.clone(), post: Vec::new(), strip: f_strip, sign: f_sign, entitlements: f_ents, ..Default::default() },
            );
            out
        } else {
            format!("build/{}/{}", archs[0], out_file)
        };

        // the release check every product gets: its slices, a deployment target that no
        // slice exceeds, and a signature
        let vname = format!("verify-{}", pn);
        cfg.verify.insert(
            vname.clone(),
            Verify { file: product.clone(), slices: archs.clone(), min_ios: Some(min_text.clone()), signed: true, ..Default::default() },
        );
        verifies.push(vname.clone());

        match kind {
            Kind::Tool | Kind::Library => files.push(DebFile { from: product.clone(), to: join(installed.as_ref().unwrap()), mode: Some("755".to_string()), replace: false }),
            Kind::Tweak => {
                if pr.filter_bundles.is_empty() && pr.filter_executables.is_empty() {
                    return err(format!("product {}: a tweak needs filter_bundles or filter_executables, without a filter nothing loads it", pn));
                }
                let dir = "Library/MobileSubstrate/DynamicLibraries";
                files.push(DebFile { from: product.clone(), to: join(&format!("{}/{}", dir, out_file)), mode: Some("755".to_string()), replace: false });
                data.push(DebData { to: join(&format!("{}/{}.plist", dir, pn)), mode: None, content: filter_plist(&pr.filter_bundles, &pr.filter_executables) });
            }
            Kind::App | Kind::Bundle | Kind::Framework | Kind::Appex => {
                let dir = bundle_path.clone().expect("bundle kinds have a bundle path");
                let what = match kind {
                    Kind::App => "an app",
                    Kind::Bundle => "a bundle",
                    Kind::Framework => "a framework",
                    _ => "an appex",
                };
                let plist = pr.info_plist.clone().ok_or_else(|| Error(format!("product {}: {} needs info_plist", pn, what)))?;
                let text = std::fs::read_to_string(root.join(&plist)).or_else(|e| {
                    // a binary plist is not utf-8, the file still has to exist
                    if root.join(&plist).is_file() { Ok(String::new()) } else { Err(Error(format!("{}: {}", plist, e))) }
                })?;
                if let Some(exe_name) = plist_string(&text, "CFBundleExecutable") {
                    if exe_name != *pn {
                        return err(format!("{}: CFBundleExecutable is {} but the binary is named {}", plist, exe_name, pn));
                    }
                }
                if let Some(r) = &pr.resources {
                    if !root.join(r).is_dir() {
                        return err(format!("product {}: resources {} is not a directory", pn, r));
                    }
                }
                // the plist is checked as written, what goes into the package may be converted
                let packaged = match pr.plist.clone().or_else(|| p.plist.clone()) {
                    None => plist.clone(),
                    Some(fmt) => {
                        if fmt != "binary" && fmt != "xml" {
                            return err(format!("product {}: plist is binary or xml, not {:?}", pn, fmt));
                        }
                        let step = format!("plist-{}", pn);
                        if taken(cfg, &step) {
                            return err(format!("product {} makes {}, which the file already defines", pn, step));
                        }
                        let out = format!("build/plist/{}/Info.plist", pn);
                        cfg.step.insert(
                            step.clone(),
                            Step {
                                run: vec![vec![exe_path()?, "plist".to_string(), "--to".to_string(), fmt, "-o".to_string(), out.clone(), plist.clone()]],
                                outputs: vec![out.clone()],
                                ..Default::default()
                            },
                        );
                        plist_steps.push(step);
                        out
                    }
                };
                files.push(DebFile { from: product.clone(), to: join(&format!("{}/{}", dir, pn)), mode: Some("755".to_string()), replace: false });
                files.push(DebFile { from: packaged.clone(), to: join(&format!("{}/Info.plist", dir)), mode: None, replace: false });
                if let Some(r) = &pr.resources {
                    // theos keeps Info.plist in Resources, it is packaged on its own below
                    let inside = plist.strip_prefix(&format!("{}/", r.trim_end_matches('/'))).map(|rel| vec![rel.to_string()]).unwrap_or_default();
                    trees.push(DebTree { from: r.clone(), to: join(&dir), exclude: inside, ..Default::default() });
                }
                if kind == Kind::App {
                    let bundle = pr.bundle.clone().unwrap_or_else(|| pn.clone());
                    apps.push((pn.clone(), bundle, packaged, pr.resources.clone(), plist.clone()));
                }
            }
            Kind::Objects | Kind::Rust | Kind::Go => unreachable!(),
        }
        built.push(Built { product, verify: Some(vname), last: if multi_arch { format!("{}-fat", pn) } else { pn.clone() } });
    }

    for f in &p.file {
        files.push(DebFile { to: join(&f.to), ..f.clone() });
    }
    for t in &p.tree {
        trees.push(DebTree { to: join(&t.to), ..t.clone() });
    }

    let mut version = cfg.package.as_ref().map(|k| k.version.clone());
    if let Some(v) = &version {
        if v.starts_with('@') {
            let resolved = read_version_ref(root, v)?;
            if let Some(k) = cfg.package.as_mut() {
                k.version = resolved.clone();
            }
            version = Some(resolved);
        }
    }
    let mut version_check_needed = false;
    if let (Some(pk), Some(v)) = (cfg.package.clone(), version.clone()) {
        if !valid_package_id(&pk.id) {
            return err(format!("package: bad id {:?}, use lowercase letters, digits, + . -", pk.id));
        }
        if !valid_version(&v) {
            return err(format!("package: bad version {:?}", v));
        }
        if !pk.version_checks.is_empty() {
            let mut argv = vec![exe_path()?, "check-version".to_string(), "--expect".to_string(), v.clone()];
            for c in &pk.version_checks {
                argv.extend([if c.file.ends_with(".plist") { "--plist" } else { "--field" }.to_string(), c.file.clone(), c.key.clone()]);
            }
            cfg.step.insert("version-check".to_string(), Step { run: vec![argv], stamp: Some("build/stamps/version-check".to_string()), ..Default::default() });
            version_check_needed = true;
        }
        let arch_field = if rootless { "iphoneos-arm64" } else { "iphoneos-arm" };
        let mut fields: BTreeMap<String, String> = pk.fields.clone();
        let mut set = |k: &str, val: &str| {
            fields.insert(k.to_string(), val.to_string());
        };
        set("Package", &pk.id);
        set("Version", &v);
        set("Architecture", arch_field);
        set("Description", &pk.description);
        set("Maintainer", &pk.maintainer);
        if let Some(n) = &pk.name {
            set("Name", n);
        }
        if let Some(a) = &pk.author {
            set("Author", a);
        }
        if let Some(s) = &pk.section {
            set("Section", s);
        }
        let depends = pk.depends.clone().unwrap_or_else(|| {
            let mut d = format!("firmware (>= {})", ios::text(min));
            if products.iter().any(|(_, pr)| pr.kind == Some(Kind::Tweak)) && !rootless {
                d.push_str(", mobilesubstrate");
            }
            d
        });
        set("Depends", &depends);

        let output = format!("build/{}_{}_{}.deb", pk.id, v, arch_field);
        let mut needs = verifies.clone();
        needs.extend(plist_steps.iter().cloned());
        if version_check_needed {
            needs.push("version-check".to_string());
        }
        cfg.deb.insert(
            deb_name.clone(),
            Deb {
                packages: if pk.packages_stanza { Some(format!("{}.Packages", output.trim_end_matches(".deb"))) } else { None },
                output,
                fields,
                scripts: pk.scripts.clone(),
                file: files,
                tree: trees,
                data,
                needs,
                ..Default::default()
            },
        );
    }

    for (pn, bundle, plist, resources, plist_of_app) in apps {
        let version_part = version.as_ref().map(|v| format!("_{}", v)).unwrap_or_default();
        let product = built.iter().find(|b| b.last == pn || b.last == format!("{}-fat", pn)).map(|b| b.product.clone()).unwrap_or_default();
        let mut needs: Vec<String> = built.iter().filter(|b| b.product == product).filter_map(|b| b.verify.clone()).collect();
        needs.extend(plist_steps.iter().cloned());
        if version_check_needed {
            needs.push("version-check".to_string());
        }
        let mut itrees = Vec::new();
        if let Some(r) = resources {
            let inside = plist_of_app.strip_prefix(&format!("{}/", r.trim_end_matches('/'))).map(|rel| vec![rel.to_string()]).unwrap_or_default();
            itrees.push(DebTree { from: r, to: String::new(), exclude: inside, ..Default::default() });
        }
        cfg.ipa.insert(
            format!("{}-ipa", pn),
            Ipa {
                output: format!("build/{}{}.ipa", pn, version_part),
                prefix: format!("Payload/{}.app", bundle),
                file: vec![
                    DebFile { from: product, to: pn.clone(), mode: Some("755".to_string()), replace: false },
                    DebFile { from: plist, to: "Info.plist".to_string(), mode: None, replace: false },
                ],
                tree: itrees,
                needs,
                ..Default::default()
            },
        );
    }

    if cfg.default.is_empty() {
        cfg.default = if cfg.deb.contains_key(&deb_name) {
            vec![deb_name]
        } else {
            // without a package an app is only useful as an ipa
            built
                .iter()
                .map(|b| {
                    let ipa = format!("{}-ipa", b.last.trim_end_matches("-fat"));
                    if cfg.ipa.contains_key(&ipa) { ipa } else { b.last.clone() }
                })
                .collect()
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(tag: &str, toml: &str) -> (std::path::PathBuf, Config) {
        let d = std::env::temp_dir().join(format!("meow-kinds-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/main.m"), "int main(void){return 0;}").unwrap();
        std::fs::write(d.join("src/util.c"), "int u;").unwrap();
        std::fs::write(d.join("src/notes.txt"), "not a source").unwrap();
        (d, toml::from_str(toml).unwrap())
    }

    const TOOL: &str = "[project]\nname = \"hello\"\nkind = \"tool\"\nframeworks = [\"Foundation\"]\n[package]\nid = \"com.x.hello\"\nversion = \"1.2\"\ndescription = \"d\"\nmaintainer = \"m\"\n";

    #[test]
    fn a_tool_becomes_slices_target_fat_verify_and_deb() {
        let (d, mut cfg) = project("tool", TOOL);
        expand(&mut cfg, &d).unwrap();
        assert_eq!(cfg.slice.keys().collect::<Vec<_>>(), ["arm64", "armv7"]);
        assert!(cfg.slice["arm64"].flags.contains(&"-miphoneos-version-min=7.0".to_string()));
        assert!(cfg.slice["armv7"].flags.contains(&"-miphoneos-version-min=5.0".to_string()));
        let t = &cfg.target["hello"];
        assert_eq!(t.src[0].files, ["src/main.m", "src/util.c"]);
        assert_eq!(t.output.as_deref(), Some("build/${slice}/hello"));
        assert!(t.ldflags.windows(2).any(|w| w == ["-framework", "Foundation"]));
        assert!(t.per_slice["armv7"].ldflags.contains(&"-nostartfiles".to_string()));
        // two slices: the fat output is stripped and signed, the thin ones are not
        assert!(!t.sign && !t.strip);
        let f = &cfg.fat["hello-fat"];
        assert!(f.sign && f.strip);
        assert_eq!(f.output, "build/hello");
        let v = &cfg.verify["verify-hello"];
        assert_eq!((v.file.as_str(), v.signed, v.min_ios.as_deref()), ("build/hello", true, Some("5.0")));
        let deb = &cfg.deb["hello-deb"];
        assert_eq!(deb.output, "build/com.x.hello_1.2_iphoneos-arm.deb");
        assert_eq!(deb.file[0].from, "build/hello");
        assert_eq!(deb.file[0].to, "usr/bin/hello");
        assert_eq!(deb.needs, ["verify-hello"]);
        assert_eq!(deb.fields["Depends"], "firmware (>= 5.0)");
        assert_eq!(cfg.default, ["hello-deb"]);
        crate::config::validate_for_test(&cfg).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn several_products_share_one_deb() {
        let toml = r#"
[project]
name = "suite"
min_ios = "5.0"
[product.daemon]
kind = "tool"
src = ["src/util.c"]
install = "usr/libexec/suited"
[product.tweak]
kind = "tweak"
src = ["src/main.m"]
archs = ["armv7", "arm64", "arm64e"]
filter_bundles = ["com.apple.springboard"]
[product.core]
kind = "objects"
src = ["src/util.c"]
[package]
id = "com.x.suite"
version = "2"
description = "d"
maintainer = "m"
packages_stanza = true
"#;
        let (d, mut cfg) = project("multi", toml);
        expand(&mut cfg, &d).unwrap();
        assert_eq!(cfg.target["tweak"].slices, ["armv7", "arm64", "arm64e"]);
        assert_eq!(cfg.target["daemon"].slices, ["armv7", "arm64"]);
        assert_eq!(cfg.target["core"].link, LinkKind::None);
        let deb = &cfg.deb["suite-deb"];
        assert!(deb.file.iter().any(|f| f.to == "usr/libexec/suited"));
        assert!(deb.file.iter().any(|f| f.to == "Library/MobileSubstrate/DynamicLibraries/tweak.dylib"));
        assert_eq!(deb.needs, ["verify-daemon", "verify-tweak"]);
        assert_eq!(deb.packages.as_deref(), Some("build/com.x.suite_2_iphoneos-arm.Packages"));
        assert_eq!(deb.fields["Depends"], "firmware (>= 5.0), mobilesubstrate");
        assert!(!cfg.verify.contains_key("verify-core"));
        crate::config::validate_for_test(&cfg).unwrap();
        // a product without sources and a kind next to products are refused
        let (d2, mut bad) = project("multi2", &toml.replace("src = [\"src/main.m\"]", ""));
        assert!(expand(&mut bad, &d2).unwrap_err().0.contains("src is required"));
        let (d3, mut bad2) = project("multi3", &toml.replace("name = \"suite\"", "name = \"suite\"\nkind = \"tool\""));
        assert!(expand(&mut bad2, &d3).unwrap_err().0.contains("exclude each other"));
        for x in [d, d2, d3] {
            std::fs::remove_dir_all(&x).unwrap();
        }
    }

    #[test]
    fn products_use_objects_and_pass_needs_on() {
        let toml = "[project]\nname = \"s\"\n[product.core]\nkind = \"objects\"\nsrc = [\"src/util.c\"]\n[product.a]\nkind = \"tool\"\nsrc = [\"src/main.m\"]\nuses = [\"core\"]\nneeds = [\"dep-x\"]\n[dep.x]\nurl = \"file:///x.tar.gz\"\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n";
        let (d, mut cfg) = project("uses", toml);
        expand(&mut cfg, &d).unwrap();
        assert_eq!(cfg.target["a"].objects_from, ["core"]);
        assert_eq!(cfg.target["a"].needs, ["dep-x"]);
        assert!(cfg.step.contains_key("dep-x"));
        crate::config::validate_for_test(&cfg).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_tweak_needs_a_filter_and_writes_it() {
        let toml = TOOL.replace("tool", "tweak");
        let (d, mut cfg) = project("tweak", &toml);
        assert!(expand(&mut cfg, &d).unwrap_err().0.contains("filter"));
        let (d2, mut cfg2) = project("tweak2", &toml.replace("[package]", "filter_bundles = [\"com.apple.springboard\"]\n[package]"));
        expand(&mut cfg2, &d2).unwrap();
        let deb = &cfg2.deb["hello-deb"];
        assert_eq!(deb.file[0].to, "Library/MobileSubstrate/DynamicLibraries/hello.dylib");
        assert!(deb.data[0].content.contains("<string>com.apple.springboard</string>"));
        assert_eq!(deb.fields["Depends"], "firmware (>= 5.0), mobilesubstrate");
        let t = &cfg2.target["hello"];
        assert!(t.ldflags.contains(&"-dynamiclib".to_string()));
        assert!(t.cflags.contains(&"-fPIC".to_string()));
        assert!(t.per_slice.is_empty());
        std::fs::remove_dir_all(&d).unwrap();
        std::fs::remove_dir_all(&d2).unwrap();
    }

    #[test]
    fn rootless_is_arm64_only_with_a_prefix() {
        let (d, mut cfg) = project("rootless", &TOOL.replace("kind = \"tool\"", "kind = \"tool\"\nscheme = \"rootless\""));
        expand(&mut cfg, &d).unwrap();
        assert_eq!(cfg.target["hello"].slices, ["arm64"]);
        assert!(cfg.fat.is_empty());
        assert!(cfg.target["hello"].sign);
        let deb = &cfg.deb["hello-deb"];
        assert_eq!(deb.file[0].from, "build/arm64/hello");
        assert_eq!(deb.file[0].to, "var/jb/usr/bin/hello");
        assert!(deb.output.ends_with("iphoneos-arm64.deb"));
        let (d2, mut bad) = project("rootless2", &TOOL.replace("kind = \"tool\"", "kind = \"tool\"\nscheme = \"rootless\"\narchs = [\"armv7\"]"));
        assert!(expand(&mut bad, &d2).unwrap_err().0.contains("armv7"));
        std::fs::remove_dir_all(&d).unwrap();
        std::fs::remove_dir_all(&d2).unwrap();
    }

    #[test]
    fn an_app_gets_a_deb_and_an_ipa_and_checks_its_plist() {
        let toml = "[project]\nname = \"hello\"\nkind = \"app\"\ninfo_plist = \"Info.plist\"\nresources = \"res\"\n[package]\nid = \"com.x.hello\"\nversion = \"1\"\ndescription = \"d\"\nmaintainer = \"m\"\n";
        let (d, mut cfg) = project("app", toml);
        std::fs::create_dir_all(d.join("res")).unwrap();
        std::fs::write(d.join("Info.plist"), "<dict><key>CFBundleExecutable</key><string>other</string></dict>").unwrap();
        assert!(expand(&mut cfg, &d).unwrap_err().0.contains("CFBundleExecutable"));
        std::fs::write(d.join("Info.plist"), "<dict><key>CFBundleExecutable</key><string>hello</string></dict>").unwrap();
        let mut cfg: Config = toml::from_str(toml).unwrap();
        expand(&mut cfg, &d).unwrap();
        let deb = &cfg.deb["hello-deb"];
        assert_eq!(deb.file[0].to, "Applications/hello.app/hello");
        assert_eq!(deb.tree[0].to, "Applications/hello.app");
        let ipa = &cfg.ipa["hello-ipa"];
        assert_eq!(ipa.prefix, "Payload/hello.app");
        assert_eq!(ipa.needs, ["verify-hello"]);
        assert_eq!(ipa.output, "build/hello_1.ipa");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn version_can_come_from_a_file_and_is_checked_elsewhere() {
        let toml = TOOL.replace("version = \"1.2\"", "version = \"@packaging/control:Version\"\nversion_checks = [{ file = \"Info.plist\", key = \"GKReleaseVersion\" }]");
        let (d, mut cfg) = project("version", &toml);
        std::fs::create_dir_all(d.join("packaging")).unwrap();
        std::fs::write(d.join("packaging/control"), "Package: x\nVersion: 3.4.5-stable\n").unwrap();
        expand(&mut cfg, &d).unwrap();
        assert_eq!(cfg.deb["hello-deb"].fields["Version"], "3.4.5-stable");
        assert_eq!(cfg.deb["hello-deb"].needs, ["verify-hello", "version-check"]);
        let run = &cfg.step["version-check"].run[0];
        assert!(run.windows(4).any(|w| w == ["--plist", "Info.plist", "GKReleaseVersion", "--expect"]) || run.contains(&"3.4.5-stable".to_string()));
        // a reference to a missing key is an error at load, not at build
        let (d2, mut cfg2) = project("version2", &toml.replace("control:Version", "control:Nope"));
        std::fs::create_dir_all(d2.join("packaging")).unwrap();
        std::fs::write(d2.join("packaging/control"), "Version: 1\n").unwrap();
        assert!(expand(&mut cfg2, &d2).unwrap_err().0.contains("no Nope"));
        std::fs::remove_dir_all(&d).unwrap();
        std::fs::remove_dir_all(&d2).unwrap();
    }

    #[test]
    fn tests_checks_and_deps_become_targets_and_steps() {
        let toml = r#"
[slice.armv7]
cc = "cc"
flags = ["-arch", "armv7", "-isysroot", "${SDK}"]
[slice.armv7.vars]
SDK = "newsdk"
[test.core]
src = ["src/*.c"]
sanitize = "address,undefined"
libs = ["m"]
args = ["--quick"]
[test.script]
run = ["python3", "check.py"]
cwd = "tests"
[check.compat]
slice = "armv7"
sdk = "oldsdk"
src = ["src/main.m"]
headers = ["src/*.h"]
[dep.zlib]
url = "https://example.invalid/zlib.tar.gz"
sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
patches = ["a.patch"]
"#;
        let (d, mut cfg) = project("misc", toml);
        expand(&mut cfg, &d).unwrap();
        assert_eq!(cfg.slice["host"].cc, "${MEOW_HOST_CC}");
        let t = &cfg.target["test-core"];
        assert!(t.cflags.contains(&"-fsanitize=address,undefined".to_string()));
        assert!(t.ldflags.contains(&"-lm".to_string()));
        assert_eq!(cfg.step["run-core"].run[0], ["build/host/test-core", "--quick"]);
        assert_eq!(cfg.step["run-script"].run[0], ["python3", "check.py"]);
        assert_eq!(cfg.step["run-script"].cwd.as_deref(), Some("tests"));
        assert!(cfg.step["test"].needs.contains(&"run-script".to_string()));
        assert_eq!(cfg.step["test"].needs, ["run-core", "run-script"]);
        // the check uses the other sdk and reads no object
        let c = &cfg.step["check-compat"].run[0];
        assert!(c.contains(&"oldsdk".to_string()) && !c.contains(&"newsdk".to_string()));
        assert!(c.contains(&"-fsyntax-only".to_string()) && c.contains(&"src/main.m".to_string()));
        let z = &cfg.step["dep-zlib"].run[0];
        assert!(z.windows(2).any(|w| w == ["--patch", "a.patch"]));
        assert!(cfg.vars["dep.zlib"].ends_with("/src/zlib-11111111"));
        crate::config::validate_for_test(&cfg).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn picture_tables_become_img_steps_with_every_output_listed() {
        let toml = r##"
[icons.plain]
master = "art/master.png"
out = "app/icons"
preset = "ios"
sizes = { "Icon-master.png" = 512, "Icon.png" = 58 }
[icons.deb]
master = "art/master.png"
out = "res/deb"
sizes = { "Icon.png" = 57, "Icon-76.png" = 76 }
round = 0.18
overlay = "art/gloss.png"
overlay_sizes = [57]
[launch.main]
out = "res/launch"
background = "#8455c1"
icon = "art/master.png"
icon_points = 96
icon_round = 0.18
images = [{ name = "Default.png", width = 320, height = 480 }, { name = "Default@2x.png", width = 320, height = 480, scale = 2 }]
[glyphs.ui]
src = ["art/glyphs/*.svg"]
items = [{ from = "art/vec.xml", name = "pikabu" }]
out = "app/glyphs"
prefix = "glyph-"
suffix = "-normal"
scales = [1, 2]
color = "#757880"
[image.mascot]
from = "art/mascot.png"
to = "app/icons/Icon-mascot.png"
crop_alpha = 32
fit = [24, 24]
canvas = [24, 24]
scales = [1, 2]
"##;
        let (d, mut cfg) = project("pics", toml);
        std::fs::create_dir_all(d.join("art/glyphs")).unwrap();
        for f in ["art/glyphs/a.svg", "art/glyphs/b.svg", "art/vec.xml"] {
            std::fs::write(d.join(f), "<svg/>").unwrap();
        }
        expand(&mut cfg, &d).unwrap();
        let plain = &cfg.step["icons-plain"];
        // the preset plus an added size, and the replaced name keeps one entry
        assert_eq!(plain.outputs.len(), 12);
        assert!(plain.outputs.contains(&"app/icons/Icon-master.png".to_string()));
        let argv = &plain.run[0];
        assert_eq!(argv[1], "img");
        assert!(argv.windows(5).any(|w| w == ["--to", "app/icons/Icon.png", "--size", "58x58", "--drop-alpha"]));
        assert!(plain.keep);
        // rounded icons keep their alpha, the veil goes on one size only
        let deb = &cfg.step["icons-deb"].run[0];
        assert_eq!(deb.iter().filter(|a| *a == "--overlay").count(), 1);
        assert!(deb.contains(&"--overlay-keep-alpha".to_string()) && !deb.contains(&"--drop-alpha".to_string()));
        let launch = &cfg.step["launch-main"];
        assert_eq!(launch.outputs, ["res/launch/Default.png", "res/launch/Default@2x.png"]);
        assert!(launch.run[0].windows(2).any(|w| w == ["--solid", "640x960"]));
        assert!(launch.run[0].windows(2).any(|w| w == ["--paste-side", "192"]));
        let g = &cfg.step["glyphs-ui"];
        assert_eq!(g.outputs.len(), 6);
        assert!(g.outputs.contains(&"app/glyphs/glyph-pikabu-normal@2x.png".to_string()));
        assert!(g.run[0].windows(2).any(|w| w == ["--size", "48x48"]) && g.run[0].windows(2).any(|w| w == ["--color", "#757880"]));
        let m = &cfg.step["image-mascot"];
        assert_eq!(m.outputs, ["app/icons/Icon-mascot.png", "app/icons/Icon-mascot@2x.png"]);
        assert!(m.run[0].windows(2).any(|w| w == ["--canvas", "48x48"]));
        crate::config::validate_for_test(&cfg).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn picture_mistakes_are_named() {
        let cases = [
            ("[icons.x]\nmaster = \"m\"\nout = \"o\"\n", "give a preset"),
            ("[icons.x]\nmaster = \"m\"\nout = \"o\"\npreset = \"mac\"\n", "unknown preset"),
            ("[icons.x]\nmaster = \"m\"\nout = \"o\"\npreset = \"ios\"\nround = 0.9\n", "between 0 and 0.5"),
            ("[launch.x]\nout = \"o\"\nbackground = \"purple\"\nimages = [{ name = \"a.png\", width = 1, height = 1 }]\n", "must look like"),
            ("[launch.x]\nout = \"o\"\nbackground = \"#fff\"\nicon = \"i.png\"\nimages = [{ name = \"a.png\", width = 1, height = 1 }]\n", "go together"),
            ("[glyphs.x]\nout = \"o\"\n", "give src or items"),
            ("[glyphs.x]\nout = \"o\"\nitems = [{ from = \"a.svg\", name = \"n\" }, { from = \"b.svg\", name = \"n\" }]\n", "two glyphs write"),
            ("[image.x]\nfrom = \"a.png\"\nto = \"b.png\"\nsize = [1, 1]\nfit = [1, 1]\n", "exclude each other"),
        ];
        for (i, (body, want)) in cases.iter().enumerate() {
            let (d, mut cfg) = project(&format!("pic{}", i), body);
            std::fs::write(d.join("a.svg"), "<svg/>").unwrap();
            std::fs::write(d.join("b.svg"), "<svg/>").unwrap();
            let e = expand(&mut cfg, &d).unwrap_err();
            assert!(e.0.contains(want), "{}: {}", want, e.0);
            std::fs::remove_dir_all(&d).unwrap();
        }
    }

    #[test]
    fn dep_mistakes_are_named() {
        let bad = [
            ("url = \"u\"\nsha256 = \"abc\"", "64 lowercase"),
            ("url = \"u\"", "url and sha256"),
            ("git = \"g\"\nrev = \"main\"", "40 digit"),
            ("url = \"u\"\nsha256 = \"1111111111111111111111111111111111111111111111111111111111111111\"\ngit = \"g\"", "url and sha256"),
        ];
        for (i, (body, want)) in bad.iter().enumerate() {
            let (d, mut cfg) = project(&format!("dep{}", i), &format!("[dep.x]\n{}\n", body));
            let e = expand(&mut cfg, &d).unwrap_err();
            assert!(e.0.contains(want), "{}: {}", want, e.0);
            std::fs::remove_dir_all(&d).unwrap();
        }
    }

    #[test]
    fn mistakes_are_named() {
        let cases = [
            ("name = \"hello\"", "name = \"he llo\"", "bad name"),
            ("kind = \"tool\"", "kind = \"tool\"\narchs = [\"i386\"]", "unknown arch"),
            ("kind = \"tool\"", "kind = \"tool\"\narchs = [\"arm64\", \"arm64\"]", "twice"),
            ("kind = \"tool\"", "kind = \"tool\"\nmin_ios = \"five\"", "min_ios"),
            ("kind = \"tool\"", "kind = \"tool\"\nentitlements = \"nope.plist\"", "entitlements"),
            ("kind = \"tool\"", "kind = \"tool\"\nresources = \"res\"", "kind = \"app\""),
            ("id = \"com.x.hello\"", "id = \"Com.X\"", "bad id"),
            ("version = \"1.2\"", "version = \"v1\"", "bad version"),
            ("kind = \"tool\"", "kind = \"tool\"\narchs = [\"armv6\"]\nmin_ios = \"5.0\"", "4.2.1"),
            ("kind = \"tool\"", "kind = \"tool\"\narchs = [\"armv6\"]\narc = true", "arc needs"),
        ];
        for (i, (from, to, want)) in cases.iter().enumerate() {
            let (d, mut cfg) = project(&format!("bad{}", i), &TOOL.replace(from, to));
            let e = expand(&mut cfg, &d).unwrap_err();
            assert!(e.0.contains(want), "{}: {}", want, e.0);
            std::fs::remove_dir_all(&d).unwrap();
        }
        let (d, mut cfg) = project("collide", &format!("{}[target.hello]\nslices = [\"x\"]\n", TOOL));
        assert!(expand(&mut cfg, &d).unwrap_err().0.contains("already defines"));
        std::fs::remove_dir_all(&d).unwrap();
        let mut none: Config = toml::from_str("[package]\nid = \"aa\"\nversion = \"1\"\ndescription = \"d\"\nmaintainer = \"m\"\n").unwrap();
        assert!(expand(&mut none, Path::new(".")).is_err());
    }

    #[test]
    fn without_a_package_the_default_is_the_binary() {
        let (d, mut cfg) = project("nopkg", "[project]\nname = \"hello\"\nkind = \"tool\"\narchs = [\"arm64\"]\n");
        expand(&mut cfg, &d).unwrap();
        assert!(cfg.deb.is_empty());
        assert_eq!(cfg.default, ["hello"]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
