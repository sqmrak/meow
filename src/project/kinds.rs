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
const SOURCE_EXTENSIONS: [&str; 5] = ["c", "m", "mm", "cpp", "cc"];
const DEFAULT_MIN_IOS: &str = "5.0";
// with an armv6 slice the project has to reach ios 3, where armv6 devices still got updates
const DEFAULT_MIN_IOS_ARMV6: &str = "3.0";

fn valid_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('.') && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

fn valid_package_id(id: &str) -> bool {
    id.len() >= 2 && id.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
}

fn valid_version(v: &str) -> bool {
    v.chars().next().is_some_and(|c| c.is_ascii_digit()) && v.chars().all(|c| c.is_ascii_alphanumeric() || ".+~:-".contains(c))
}

// the armv6 sdk is a thin copy made by the sdk-armv6 step, see sdkthin.rs
fn slice_def(arch: &str, min: ios::Version) -> Slice {
    let (triple, sdk) = match arch {
        "armv6" => ("arm-apple-darwin10", "${MEOW_CACHE_DIR}/sdk-armv6"),
        "armv7" => ("arm-apple-darwin11", "${MEOW_SDK_ARMV7}"),
        "arm64" => ("arm64-apple-darwin", "${MEOW_SDK_ARM64}"),
        _ => ("arm64e-apple-darwin", "${MEOW_SDK_ARM64}"),
    };
    let min = ios::slice_min(arch, min);
    Slice {
        cc: "${MEOW_CLANG}".to_string(),
        ar: "${MEOW_AR}".to_string(),
        flags: ["-target", triple, "-B", "${MEOW_TC}", "-arch", arch, &format!("-miphoneos-version-min={}", ios::text(min)), "-isysroot", "${SDK}"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        vars: BTreeMap::from([("SDK".to_string(), sdk.to_string())]),
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
                cfg.slice.insert(a.clone(), slice_def(a, min));
            }
        }
    }

    // generated names must be free before anything is inserted
    let deb_name = format!("{}-deb", p.name);
    let mut mine: Vec<String> = Vec::new();
    for (pn, pr) in &products {
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
                    "${MEOW_SDK_ARMV6}".to_string(),
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
    let mut apps: Vec<(String, String, String, Option<String>)> = Vec::new();
    let mut built: Vec<Built> = Vec::new();

    for ((pn, pr), archs) in products.iter().zip(&product_archs) {
        let kind = pr.kind.ok_or_else(|| Error(format!("product {}: kind is required", pn)))?;
        if !valid_name(pn) {
            return err(format!("product {}: bad name", pn));
        }
        let exe = matches!(kind, Kind::Tool | Kind::App);
        let dylib = matches!(kind, Kind::Tweak | Kind::Library);
        if kind != Kind::App && (pr.info_plist.is_some() || pr.resources.is_some() || pr.bundle.is_some()) {
            return err(format!("product {}: info_plist, resources and bundle are for kind = \"app\"", pn));
        }
        if kind != Kind::Tweak && (!pr.filter_bundles.is_empty() || !pr.filter_executables.is_empty()) {
            return err(format!("product {}: filter_bundles and filter_executables are for kind = \"tweak\"", pn));
        }
        if pr.install.is_some() && !matches!(kind, Kind::Tool | Kind::Library) {
            return err(format!("product {}: install is for tools and libraries", pn));
        }
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

        let out_file = match kind {
            Kind::Tool | Kind::App | Kind::Objects => pn.clone(),
            Kind::Tweak => format!("{}.dylib", pn),
            Kind::Library => format!("lib{}.dylib", pn),
        };
        // where the file lands on the device, relative to the package root
        let installed = match kind {
            Kind::Tool => Some(pr.install.clone().unwrap_or_else(|| format!("usr/bin/{}", pn))),
            Kind::Library => Some(pr.install.clone().unwrap_or_else(|| format!("usr/lib/{}", out_file))),
            Kind::Tweak => Some(format!("Library/MobileSubstrate/DynamicLibraries/{}", out_file)),
            _ => None,
        };

        let mut cflags: Vec<String> = vec!["-O2".to_string(), "-Wall".to_string()];
        if dylib {
            cflags.push("-fPIC".to_string());
        }
        if arc {
            cflags.push("-fobjc-arc".to_string());
        }
        cflags.extend(shared(&p.cflags));
        cflags.extend(pr.cflags.iter().cloned());

        let mut ldflags: Vec<String> = Vec::new();
        if let (true, Some(path)) = (dylib, &installed) {
            ldflags.extend(["-dynamiclib".to_string(), "-install_name".to_string(), format!("/{}", join(path))]);
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
                objects_from: pr.uses.clone(),
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
            Kind::App => {
                let bundle = pr.bundle.clone().unwrap_or_else(|| pn.clone());
                if !valid_name(&bundle) {
                    return err(format!("product {}: bad bundle name {:?}", pn, bundle));
                }
                let plist = pr.info_plist.clone().ok_or_else(|| Error(format!("product {}: an app needs info_plist", pn)))?;
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
                let dir = format!("Applications/{}.app", bundle);
                files.push(DebFile { from: product.clone(), to: join(&format!("{}/{}", dir, pn)), mode: Some("755".to_string()), replace: false });
                files.push(DebFile { from: plist.clone(), to: join(&format!("{}/Info.plist", dir)), mode: None, replace: false });
                if let Some(r) = &pr.resources {
                    trees.push(DebTree { from: r.clone(), to: join(&dir), ..Default::default() });
                }
                apps.push((pn.clone(), bundle, plist, pr.resources.clone()));
            }
            Kind::Objects => unreachable!(),
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

    for (pn, bundle, plist, resources) in apps {
        let version_part = version.as_ref().map(|v| format!("_{}", v)).unwrap_or_default();
        let product = built.iter().find(|b| b.last == pn || b.last == format!("{}-fat", pn)).map(|b| b.product.clone()).unwrap_or_default();
        let mut needs: Vec<String> = built.iter().filter(|b| b.product == product).filter_map(|b| b.verify.clone()).collect();
        if version_check_needed {
            needs.push("version-check".to_string());
        }
        let mut itrees = Vec::new();
        if let Some(r) = resources {
            itrees.push(DebTree { from: r, to: String::new(), ..Default::default() });
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
