// `meow migrate [DIR]` reads a theos project and writes the meow.toml that builds the same
// thing: the Makefile, `control`, the filter plist, `layout/` and the plists of bundles.
// everything it cannot carry over is listed as a note, nothing is dropped quietly

mod make;

use crate::plist;
use make::Make;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "usage: meow migrate [DIR] [--write [--force]] [--set VAR=VALUE]...

  DIR            the theos project, the current directory when left out
  --write        write DIR/meow.toml instead of printing it, an existing file is kept unless --force
  --set VAR=VAL  a variable the Makefile reads from the environment, for ifeq blocks:
                 --set THEOS_PACKAGE_SCHEME=rootless --set FINALPACKAGE=1
  the notes about what did not carry over go to standard error";

pub struct Spec {
    dir: PathBuf,
    write: bool,
    force: bool,
    set: Vec<(String, String)>,
}

pub fn parse_args(args: &[String]) -> Result<Spec, String> {
    let mut s = Spec { dir: PathBuf::from("."), write: false, force: false, set: Vec::new() };
    let mut dir_given = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--write" => s.write = true,
            "--force" => s.force = true,
            "--set" => {
                let v = it.next().ok_or("migrate: --set needs VAR=VALUE")?;
                let (k, val) = v.split_once('=').ok_or_else(|| format!("migrate: --set wants VAR=VALUE, got {}", v))?;
                s.set.push((k.to_string(), val.to_string()));
            }
            o if o.starts_with('-') => return Err(format!("migrate: unknown option {}\n{}", o, USAGE)),
            d if !dir_given => {
                s.dir = PathBuf::from(d);
                dir_given = true;
            }
            _ => return Err(format!("migrate: one directory at a time\n{}", USAGE)),
        }
    }
    Ok(s)
}

#[derive(Default, Clone)]
struct Product {
    name: String,
    kind: &'static str,
    archs: Option<Vec<String>>,
    src: Vec<String>,
    cflags: Vec<String>,
    ldflags: Vec<String>,
    frameworks: Vec<String>,
    libs: Vec<String>,
    arc: Option<bool>,
    entitlements: Option<String>,
    info_plist: Option<String>,
    resources: Option<String>,
    bundle_extension: Option<String>,
    filter_bundles: Vec<String>,
    filter_executables: Vec<String>,
    install: Option<String>,
    logos: Option<String>,
    swiftflags: Vec<String>,
    bridging_header: Option<String>,
}

struct Walk<'a> {
    root: PathBuf,
    set: &'a [(String, String)],
    notes: Vec<String>,
}

impl<'a> Walk<'a> {
    fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.notes.contains(&s) {
            self.notes.push(s);
        }
    }

    // a path inside the project, relative to the project root, `sub` is the directory the
    // makefile sits in
    fn rel(&self, sub: &str, p: &str) -> String {
        let p = p.trim_start_matches("./");
        if Path::new(p).is_absolute() {
            let abs = self.root.canonicalize().unwrap_or_else(|_| self.root.clone());
            if let Ok(r) = Path::new(p).strip_prefix(&abs) {
                return r.to_string_lossy().into_owned();
            }
            return p.to_string();
        }
        if sub.is_empty() { p.to_string() } else { format!("{}/{}", sub, p) }
    }

    // a flag that names a path: -Ipath, -Fpath, -Lpath, and -include path
    fn flags(&self, sub: &str, tokens: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            let t = &tokens[i];
            let mut done = false;
            for pre in ["-I", "-F", "-L"] {
                if let Some(p) = t.strip_prefix(pre) {
                    if !p.is_empty() {
                        out.push(format!("{}{}", pre, self.rel(sub, p)));
                        done = true;
                    }
                }
            }
            if !done && t == "-include" && i + 1 < tokens.len() {
                out.push(t.clone());
                out.push(self.rel(sub, &tokens[i + 1]));
                i += 1;
                done = true;
            }
            if !done {
                out.push(t.clone());
            }
            i += 1;
        }
        out
    }
}

const KINDS: [(&str, &str); 6] = [
    ("TWEAK_NAME", "tweak"),
    ("APPLICATION_NAME", "app"),
    ("TOOL_NAME", "tool"),
    ("BUNDLE_NAME", "bundle"),
    ("FRAMEWORK_NAME", "framework"),
    ("LIBRARY_NAME", "library"),
];

// instance variables that migrate, the rest are reported
const INSTANCE_VARS: [&str; 16] = [
    "SWIFTFLAGS", "SWIFT_BRIDGING_HEADER", "FILES", "FRAMEWORKS", "PRIVATE_FRAMEWORKS", "LIBRARIES", "CFLAGS", "OBJCFLAGS", "LDFLAGS", "CODESIGN_FLAGS", "INSTALL_PATH", "BUNDLE_EXTENSION", "LOGOS_DEFAULT_GENERATOR", "CCFLAGS", "CXXFLAGS",
    "ARCHS",
];

fn archs_of(list: Vec<String>, w: &mut Walk, from: &str) -> Vec<String> {
    let mut out = Vec::new();
    for a in list {
        if ["armv6", "armv7", "arm64", "arm64e"].contains(&a.as_str()) {
            out.push(a);
        } else {
            w.note(format!("{}: architecture {} is not built by meow and was left out", from, a));
        }
    }
    out
}

struct DirInfo {
    products: Vec<Product>,
    sdk: Option<String>,
    archs: Option<Vec<String>>,
    min_ios: Option<String>,
    scheme: Option<String>,
    package_version: Option<String>,
}

fn collect(dir: &Path, sub: &str, w: &mut Walk) -> Result<DirInfo, String> {
    let mk = ["Makefile", "makefile", "GNUmakefile"].iter().map(|n| dir.join(n)).find(|p| p.is_file()).ok_or_else(|| format!("{}: no Makefile", dir.display()))?;
    let text = std::fs::read_to_string(&mk).map_err(|e| format!("{}: {}", mk.display(), e))?;
    let mut m = Make::new(dir, w.set);
    m.parse(&text);
    let at = if sub.is_empty() { "Makefile".to_string() } else { format!("{}/Makefile", sub) };
    for n in std::mem::take(&mut m.notes) {
        w.note(format!("{}: {}", at, n));
    }

    let target = m.get("TARGET").unwrap_or_default();
    let mut min_ios = None;
    let mut sdk = None;
    let parts: Vec<&str> = target.split(':').filter(|p| !matches!(*p, "iphone" | "simulator" | "macosx" | "appletv" | "watchos" | "clang" | "gcc" | "")).collect();
    let numeric = |d: &str| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit() || c == '.');
    if parts.first().is_some_and(|v| numeric(v)) {
        sdk = Some(parts[0].to_string());
    }
    if parts.len() >= 2 {
        let d = parts[1];
        if d.chars().all(|c| c.is_ascii_digit() || c == '.') && !d.is_empty() {
            min_ios = Some(d.to_string());
        }
    } else if !target.is_empty() {
        w.note(format!("{}: TARGET {} names no deployment target, min_ios was left out", at, target));
    }

    let archs = m.get("ARCHS").map(|a| a.split_whitespace().map(str::to_string).collect::<Vec<_>>()).filter(|a| !a.is_empty()).map(|a| archs_of(a, w, &at));
    let scheme = m.get("THEOS_PACKAGE_SCHEME").filter(|s| !s.is_empty());
    let package_version = m.get("PACKAGE_VERSION").filter(|v| !v.is_empty() && !v.contains('$'));

    let mut products: Vec<Product> = Vec::new();
    let mut known_names: Vec<String> = Vec::new();
    for (var, kind) in KINDS {
        for name in m.list(var) {
            known_names.push(name.clone());
            let mut p = Product { name: name.clone(), kind, ..Default::default() };
            let lv = |m: &mut Make, v: &str| m.list(&format!("{}_{}", name, v));
            let get = |m: &mut Make, v: &str| m.get(&format!("{}_{}", name, v)).filter(|s| !s.trim().is_empty());

            let linkage = get(&mut m, "LINKAGE_TYPE").or_else(|| m.get("THEOS_LINKAGE_TYPE"));
            if kind == "library" && linkage.as_deref() == Some("static") {
                w.note(format!("{}: {} is a static library, meow builds dynamic libraries only, write a [target] with link = \"archive\" for it", at, name));
                continue;
            }
            if get(&mut m, "INSTALL").as_deref() == Some("0") {
                w.note(format!("{}: {} sets _INSTALL = 0, it is not packaged, make it kind = \"objects\" by hand if others link it", at, name));
                continue;
            }

            let files = lv(&mut m, "FILES");
            if files.is_empty() {
                w.note(format!("{}: {}_FILES is empty, the sources of {} were left to the walk of src/", at, name, name));
            }
            for f in files {
                if f.ends_with(".swift") && p.src.iter().all(|x| !x.ends_with(".swift")) {
                    w.note(format!("{}: {} is swift, meow builds it for ios 12.2 and later on arm64 only, see docs/swift.md", at, f));
                }
                if f.contains('$') {
                    w.note(format!("{}: source {} still holds a variable", at, f));
                }
                let r = w.rel(sub, &f);
                if !p.src.contains(&r) {
                    p.src.push(r);
                }
            }

            // flags: the instance's own, then the global ones
            let mut cf: Vec<String> = Vec::new();
            for v in ["ADDITIONAL_CFLAGS", "ADDITIONAL_OBJCFLAGS"] {
                cf.extend(m.list(v));
            }
            for v in ["CFLAGS", "OBJCFLAGS"] {
                cf.extend(lv(&mut m, v));
            }
            for v in ["CCFLAGS", "CXXFLAGS"] {
                let l = lv(&mut m, v);
                if !l.is_empty() {
                    w.note(format!("{}: {}_{} applies to c++ only in theos, it was added to cflags", at, name, v));
                    cf.extend(l);
                }
            }
            let mut cflags = Vec::new();
            for t in w.flags(sub, &cf) {
                match t.as_str() {
                    "-fobjc-arc" => p.arc = Some(true),
                    "-fno-objc-arc" => p.arc = Some(false),
                    _ => cflags.push(t),
                }
            }
            p.cflags = cflags;

            let mut lf: Vec<String> = m.list("ADDITIONAL_LDFLAGS");
            lf.extend(lv(&mut m, "LDFLAGS"));
            let lf = w.flags(sub, &lf);
            let mut i = 0;
            while i < lf.len() {
                let t = &lf[i];
                if let Some(l) = t.strip_prefix("-l").filter(|l| !l.is_empty()) {
                    p.libs.push(l.to_string());
                } else if t == "-framework" && i + 1 < lf.len() {
                    p.frameworks.push(lf[i + 1].clone());
                    i += 1;
                } else {
                    p.ldflags.push(t.clone());
                }
                i += 1;
            }
            p.frameworks.extend(lv(&mut m, "FRAMEWORKS"));
            let private = lv(&mut m, "PRIVATE_FRAMEWORKS");
            if !private.is_empty() {
                p.frameworks.extend(private);
                p.ldflags.push("-F${SDK}/System/Library/PrivateFrameworks".to_string());
            }
            for f in lv(&mut m, "EXTRA_FRAMEWORKS") {
                w.note(format!("{}: {}_EXTRA_FRAMEWORKS {} is a theos framework, add its -F and -framework by hand", at, name, f));
            }
            p.libs.extend(lv(&mut m, "LIBRARIES"));
            let mut seen: Vec<String> = Vec::new();
            p.libs.retain(|l| {
                let fresh = !seen.contains(l);
                seen.push(l.clone());
                fresh
            });

            // theos puts its own headers and frameworks on the search path of tweaks and bundles,
            // and a .x file links substrate through the code logos writes
            let logos_src = p.src.iter().any(|f| f.ends_with(".x") || f.ends_with(".xm"));
            if matches!(kind, "tweak" | "bundle") {
                p.cflags.push("-I${MEOW_HEADERS}".to_string());
                p.cflags.push("-F${MEOW_VENDOR_LIB}".to_string());
                p.ldflags.push("-F${MEOW_VENDOR_LIB}".to_string());
                w.note(format!("{}: {} reads the theos headers through ${{MEOW_HEADERS}}, `meow setup headers` installs them", at, name));
            }
            if logos_src && p.libs.iter().any(|l| l == "substrate") {
                p.libs.retain(|l| l != "substrate");
                w.note(format!("{}: -lsubstrate was dropped, the code logos writes links CydiaSubstrate itself", at));
            }

            p.swiftflags = lv(&mut m, "SWIFTFLAGS");
            if let Some(h) = get(&mut m, "SWIFT_BRIDGING_HEADER") {
                p.bridging_header = Some(w.rel(sub, &h));
            }
            if let Some(g) = get(&mut m, "LOGOS_DEFAULT_GENERATOR") {
                p.logos = Some(g);
            }

            // entitlements: -S file in CODESIGN_FLAGS, or the file theos picks up on its own
            let cs = lv(&mut m, "CODESIGN_FLAGS");
            let mut ent: Option<String> = None;
            let mut j = 0;
            while j < cs.len() {
                if let Some(f) = cs[j].strip_prefix("-S") {
                    if !f.is_empty() {
                        ent = Some(f.to_string());
                    } else if j + 1 < cs.len() && !cs[j + 1].starts_with('-') {
                        ent = Some(cs[j + 1].clone());
                        j += 1;
                    }
                } else if cs[j] == "-C" || cs[j] == "-I" {
                    j += 1;
                    w.note(format!("{}: codesign flag {} {} is not used, ldid signs with the ad hoc identity", at, cs[j - 1], cs.get(j).cloned().unwrap_or_default()));
                } else if cs[j].starts_with("-C") || cs[j].starts_with("-I") {
                    w.note(format!("{}: codesign flag {} is not used, ldid signs with the ad hoc identity", at, cs[j]));
                }
                j += 1;
            }
            if ent.is_none() {
                for cand in ["entitlements.plist", "ent.plist"] {
                    if dir.join(cand).is_file() {
                        ent = Some(cand.to_string());
                        break;
                    }
                }
            }
            if let Some(e) = ent {
                if dir.join(&e).is_file() {
                    p.entitlements = Some(w.rel(sub, &e));
                } else {
                    w.note(format!("{}: entitlements file {} was not found", at, e));
                }
            }

            // where it installs. theos names a directory, meow a file for tools and libraries
            let install = get(&mut m, "INSTALL_PATH").map(|s| s.trim_matches('/').to_string());
            p.install = match (kind, install) {
                ("tool", Some(d)) => Some(format!("{}/{}", d, name)),
                ("library", Some(d)) => Some(format!("{}/lib{}.dylib", d, name)),
                ("bundle" | "framework", Some(d)) => Some(d),
                ("app", Some(d)) => {
                    w.note(format!("{}: {}_INSTALL_PATH {} is not used, an app installs in /Applications", at, name, d));
                    None
                }
                (_, d) => d,
            };
            if kind == "bundle" {
                p.bundle_extension = get(&mut m, "BUNDLE_EXTENSION").filter(|e| e != "bundle");
                if p.install.is_none() {
                    w.note(format!("{}: {} has no _INSTALL_PATH, give it install = ... in the product", at, name));
                }
            }

            // Info.plist and resources
            if matches!(kind, "app" | "bundle" | "framework") {
                let plist = ["Info.plist", "Resources/Info.plist"].iter().find(|c| dir.join(c).is_file());
                match plist {
                    Some(c) => p.info_plist = Some(w.rel(sub, c)),
                    None => w.note(format!("{}: {} has no Info.plist, an {} needs one", at, name, kind)),
                }
                if dir.join("Resources").is_dir() {
                    p.resources = Some(w.rel(sub, "Resources"));
                }
            }

            // the filter of a tweak
            if kind == "tweak" {
                let fp = dir.join(format!("{}.plist", name));
                match std::fs::read(&fp).map_err(|e| e.to_string()).and_then(|b| plist::parse(&b)) {
                    Ok(v) => {
                        let list = |k: &str| -> Vec<String> {
                            v.path(&format!("Filter.{}", k)).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
                        };
                        p.filter_bundles = list("Bundles");
                        p.filter_executables = list("Executables");
                        for k in ["Classes", "Mode", "CoreFoundationVersion"] {
                            if v.path(&format!("Filter.{}", k)).is_some() {
                                w.note(format!("{}.plist: Filter.{} is not carried over, add it to the filter by hand", name, k));
                            }
                        }
                        if p.filter_bundles.is_empty() && p.filter_executables.is_empty() {
                            w.note(format!("{}.plist lists no bundles and no executables, meow refuses a tweak without a filter", name));
                        }
                    }
                    Err(e) => w.note(format!("{}: the filter {}.plist was not read: {}", at, name, e)),
                }
            }

            // names assigned for this instance that nothing above understood
            let prefix = format!("{}_", name);
            for a in m.assigned.clone() {
                if let Some(rest) = a.strip_prefix(&prefix) {
                    if !INSTANCE_VARS.contains(&rest) && rest != "LINKAGE_TYPE" && rest != "INSTALL" && rest != "EXTRA_FRAMEWORKS" && rest != "NAME" {
                        w.note(format!("{}: {} is not migrated", at, a));
                    }
                }
            }
            if let Some(per) = get(&mut m, "ARCHS") {
                let a = archs_of(per.split_whitespace().map(str::to_string).collect(), w, &at);
                p.archs = Some(a);
            }
            products.push(p);
        }
    }

    for t in ["NULL_NAME", "AGGREGATE_NAME", "XPC_SERVICE_NAME", "APPEX_NAME", "SIMBLTWEAK_NAME", "XCODEPROJ_NAME"] {
        for n in m.list(t) {
            w.note(format!("{}: {} {} has no kind in meow yet, add it by hand", at, t, n));
        }
    }
    if products.is_empty() && known_names.is_empty() {
        w.note(format!("{}: no *_NAME variable gave a product", at));
    }

    for r in &m.rules {
        if r == "all" || r == ".PHONY" || r.starts_with('%') {
            continue;
        }
        w.note(format!("{}: the rule {} is not migrated, a [step] or a build script does its job", at, r));
    }
    if m.assigned.iter().any(|a| a == "INSTALL_TARGET_PROCESSES") {
        w.note(format!("{}: INSTALL_TARGET_PROCESSES is not used, meow install does not restart processes", at));
    }

    let subs = m.list("SUBPROJECTS");
    let mut info = DirInfo { products, sdk, archs, min_ios, scheme, package_version };
    for s in subs {
        let sd = dir.join(&s);
        let subpath = if sub.is_empty() { s.clone() } else { format!("{}/{}", sub, s) };
        match collect(&sd, &subpath, w) {
            Ok(mut sub_info) => {
                // a subproject that asks for other slices than the root keeps its own
                for p in &mut sub_info.products {
                    if p.archs.is_none() && sub_info.archs.is_some() && sub_info.archs != info.archs {
                        p.archs = sub_info.archs.clone();
                    }
                }
                info.products.extend(sub_info.products);
            }
            Err(e) => w.note(format!("{}: subproject {} was not read: {}", at, s, e)),
        }
    }
    Ok(info)
}

fn q(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn list(key: &str, items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let one: String = format!("{} = [{}]\n", key, items.iter().map(|i| q(i)).collect::<Vec<_>>().join(", "));
    if one.len() <= 100 {
        return one;
    }
    let mut s = format!("{} = [\n", key);
    for i in items {
        s.push_str(&format!("    {},\n", q(i)));
    }
    s.push_str("]\n");
    s
}

fn product_toml(p: &Product, with_kind: bool, out: &mut String) {
    if with_kind {
        out.push_str(&format!("kind = {}\n", q(p.kind)));
    }
    if let Some(a) = &p.archs {
        out.push_str(&list("archs", a));
    }
    out.push_str(&list("src", &p.src));
    out.push_str(&list("cflags", &p.cflags));
    out.push_str(&list("ldflags", &p.ldflags));
    out.push_str(&list("swiftflags", &p.swiftflags));
    out.push_str(&list("frameworks", &p.frameworks));
    out.push_str(&list("libs", &p.libs));
    if let Some(a) = p.arc {
        out.push_str(&format!("arc = {}\n", a));
    }
    for (k, v) in [
        ("entitlements", &p.entitlements),
        ("info_plist", &p.info_plist),
        ("resources", &p.resources),
        ("bundle_extension", &p.bundle_extension),
        ("install", &p.install),
        ("logos", &p.logos),
        ("bridging_header", &p.bridging_header),
    ] {
        if let Some(v) = v {
            out.push_str(&format!("{} = {}\n", k, q(v)));
        }
    }
    out.push_str(&list("filter_bundles", &p.filter_bundles));
    out.push_str(&list("filter_executables", &p.filter_executables));
}

// the control file, `Key: value` with continuation lines
fn read_control(path: &Path) -> Option<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut last: Option<String> = None;
    for l in text.lines() {
        if l.starts_with(' ') || l.starts_with('\t') {
            if let Some(k) = &last {
                if let Some(v) = out.get_mut(k) {
                    v.push('\n');
                    v.push_str(l.trim());
                }
            }
        } else if let Some((k, v)) = l.split_once(':') {
            out.insert(k.trim().to_string(), v.trim().to_string());
            last = Some(k.trim().to_string());
        }
    }
    Some(out)
}

fn project_name(dir: &Path) -> String {
    let n = dir.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    if crate::kinds::valid_name(&n) { n } else { "project".to_string() }
}

pub fn run(s: &Spec) -> Result<(), String> {
    let mut w = Walk { root: s.dir.clone(), set: &s.set, notes: Vec::new() };
    let info = collect(&s.dir, "", &mut w)?;
    if info.products.is_empty() {
        for n in &w.notes {
            eprintln!("note: {}", n);
        }
        return Err(format!("{}: the Makefile gave no product, nothing to write", s.dir.display()));
    }
    let mut scheme = info.scheme.clone();
    let control = read_control(&s.dir.join("control"));
    if let Some(c) = &control {
        if c.get("Architecture").map(String::as_str) == Some("iphoneos-arm64") && scheme.is_none() {
            scheme = Some("rootless".to_string());
        }
    } else {
        w.note("no control file, the package table was left out");
    }
    if scheme.as_deref() == Some("roothide") {
        w.note("THEOS_PACKAGE_SCHEME = roothide is not supported, the package is rootful");
        scheme = None;
    }
    let rootless = scheme.as_deref() == Some("rootless");

    let mut out = String::from("# made by `meow migrate` from a theos project\n");
    let single = info.products.len() == 1;
    out.push_str("[project]\n");
    if single {
        out.push_str(&format!("name = {}\n", q(&info.products[0].name)));
    } else {
        out.push_str(&format!("name = {}\n", q(&project_name(&s.dir))));
    }
    if let Some(a) = &info.archs {
        out.push_str(&list("archs", a));
    }
    if let Some(m) = &info.min_ios {
        out.push_str(&format!("min_ios = {}\n", q(m)));
    }
    if let Some(v) = &info.sdk {
        out.push_str(&format!("sdk = {}\n", q(v)));
        w.note(format!("sdk {} is the one TARGET names, `meow sdk {}` installs it", v, v));
    }
    if rootless {
        out.push_str("scheme = \"rootless\"\n");
    }
    if single {
        let mut p = info.products[0].clone();
        if p.archs == info.archs {
            p.archs = None;
        }
        product_toml(&p, true, &mut out);
    }

    // what the package carries besides the products
    let layout = s.dir.join("layout");
    if layout.is_dir() {
        out.push_str("\n[[project.tree]]\nfrom = \"layout\"\nto = \"\"\n");
        if layout.join("DEBIAN").is_dir() {
            out.push_str("exclude = [\"DEBIAN\"]\n");
        }
    }
    if !single {
        for p in &info.products {
            out.push_str(&format!("\n[product.{}]\n", p.name));
            let mut p = p.clone();
            if p.archs == info.archs {
                p.archs = None;
            }
            product_toml(&p, true, &mut out);
        }
    }

    if let Some(c) = &control {
        out.push_str("\n[package]\n");
        let version = info.package_version.clone().or_else(|| c.get("Version").cloned()).unwrap_or_else(|| "0.0.1".to_string());
        let mut extra: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in c {
            if !["Package", "Name", "Version", "Architecture", "Description", "Maintainer", "Author", "Section", "Depends"].contains(&k.as_str()) {
                extra.insert(k.clone(), v.clone());
            }
        }
        out.push_str(&format!("id = {}\n", q(c.get("Package").map(String::as_str).unwrap_or(""))));
        if let Some(n) = c.get("Name") {
            out.push_str(&format!("name = {}\n", q(n)));
        }
        out.push_str(&format!("version = {}\n", q(&version)));
        out.push_str(&format!("description = {}\n", q(c.get("Description").map(String::as_str).unwrap_or(""))));
        out.push_str(&format!("maintainer = {}\n", q(c.get("Maintainer").map(String::as_str).unwrap_or(""))));
        for (k, key) in [("Author", "author"), ("Section", "section"), ("Depends", "depends")] {
            if let Some(v) = c.get(k) {
                out.push_str(&format!("{} = {}\n", key, q(v)));
            }
        }
        let mut scripts: Vec<(&str, String)> = Vec::new();
        for name in ["preinst", "postinst", "prerm", "postrm"] {
            let f = format!("layout/DEBIAN/{}", name);
            if s.dir.join(&f).is_file() {
                scripts.push((name, f));
            }
        }
        if !scripts.is_empty() {
            out.push_str(&format!("scripts = {{ {} }}\n", scripts.iter().map(|(n, f)| format!("{} = {}", n, q(f))).collect::<Vec<_>>().join(", ")));
        }
        if !extra.is_empty() {
            out.push_str(&format!("fields = {{ {} }}\n", extra.iter().map(|(k, v)| format!("{} = {}", q(k), q(v))).collect::<Vec<_>>().join(", ")));
        }
        if c.get("Package").map_or(true, |p| p.is_empty()) {
            w.note("control has no Package field");
        }
    }

    // the result has to load, otherwise the notes say why
    match toml::from_str::<crate::config::Config>(&out) {
        Ok(mut cfg) => {
            if let Err(e) = crate::kinds::expand(&mut cfg, &s.dir) {
                w.note(format!("the file does not build yet: {}", e));
            }
        }
        Err(e) => w.note(format!("the file does not load yet: {}", e)),
    }

    for n in &w.notes {
        eprintln!("note: {}", n);
    }
    if s.write {
        let to = s.dir.join("meow.toml");
        if to.exists() && !s.force {
            return Err(format!("{} exists, pass --force to replace it", to.display()));
        }
        std::fs::write(&to, &out).map_err(|e| format!("{}: {}", to.display(), e))?;
        eprintln!("wrote {}", to.display());
    } else {
        print!("{}", out);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(tag: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meow-migrate-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (path, text) in files {
            let to = d.join(path);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::write(to, text).unwrap();
        }
        d
    }

    fn migrate(d: &Path, set: &[(&str, &str)]) -> String {
        let set: Vec<(String, String)> = set.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let spec = Spec { dir: d.to_path_buf(), write: true, force: true, set };
        run(&spec).unwrap();
        std::fs::read_to_string(d.join("meow.toml")).unwrap()
    }

    const FILTER: &str = "{ Filter = { Bundles = ( \"com.apple.springboard\" ); Executables = ( Preferences ); Mode = Any; }; }";
    const CONTROL: &str = "Package: com.x.tweak\nName: My Tweak\nVersion: 1.2\nArchitecture: iphoneos-arm\nDescription: does a thing\n and more\nMaintainer: me\nDepends: mobilesubstrate\nSection: Tweaks\nHomepage: http://x\n";

    #[test]
    fn a_tweak_with_logos_sources_layout_and_scripts() {
        let d = project(
            "tweak",
            &[
                ("Makefile", "TARGET := iphone:clang:latest:7.0\nARCHS = armv7 arm64 armv7s\ninclude $(THEOS)/makefiles/common.mk\nTWEAK_NAME = Hello\nHello_FILES = Tweak.xm util.c\nHello_FRAMEWORKS = UIKit\nHello_PRIVATE_FRAMEWORKS = Preferences\nHello_CFLAGS = -fobjc-arc -I$(CURDIR)/inc -DX=1\nHello_LDFLAGS = -lsubstrate -lz -framework Foundation\nHello_LOGOS_DEFAULT_GENERATOR = libhooker\ninclude $(THEOS_MAKE_PATH)/tweak.mk\nafter-install::\n\tinstall.exec \"killall SpringBoard\"\n"),
                ("Tweak.xm", "%hook A\n%end\n"),
                ("util.c", ""),
                ("inc/x.h", ""),
                ("Hello.plist", FILTER),
                ("control", CONTROL),
                ("layout/Library/x.txt", "x"),
                ("layout/DEBIAN/postinst", "#!/bin/sh\n"),
            ],
        );
        let toml = migrate(&d, &[]);
        let cfg: crate::config::Config = toml::from_str(&toml).unwrap();
        assert!(toml.contains("min_ios = \"7.0\""), "{}", toml);
        assert!(toml.contains("archs = [\"armv7\", \"arm64\"]"), "{}", toml);
        assert!(toml.contains("src = [\"Tweak.xm\", \"util.c\"]"), "{}", toml);
        assert!(toml.contains("arc = true") && toml.contains("\"-Iinc\"") && toml.contains("\"-DX=1\""), "{}", toml);
        // substrate comes from the code logos writes, the other -l and -framework are split out
        assert!(toml.contains("libs = [\"z\"]"), "{}", toml);
        assert!(toml.contains("frameworks = [\"UIKit\", \"Foundation\", \"Preferences\"]") || toml.contains("\"Preferences\""), "{}", toml);
        assert!(toml.contains("-F${SDK}/System/Library/PrivateFrameworks"), "{}", toml);
        assert!(toml.contains("logos = \"libhooker\""), "{}", toml);
        assert!(toml.contains("filter_bundles = [\"com.apple.springboard\"]") && toml.contains("filter_executables = [\"Preferences\"]"), "{}", toml);
        assert!(toml.contains("exclude = [\"DEBIAN\"]") && toml.contains("postinst = \"layout/DEBIAN/postinst\""), "{}", toml);
        assert!(toml.contains("id = \"com.x.tweak\"") && toml.contains("version = \"1.2\"") && toml.contains("depends = \"mobilesubstrate\""), "{}", toml);
        assert!(toml.contains("fields = { \"Homepage\" = \"http://x\" }"), "{}", toml);
        assert_eq!(cfg.project.as_ref().map(|p| p.name.as_str()), Some("Hello"));
        // and the file builds a plan
        let mut cfg = cfg;
        crate::kinds::expand(&mut cfg, &d).unwrap();
        assert!(cfg.step.keys().any(|k| k.starts_with("logos-Hello")));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn conditionals_follow_the_variables_given() {
        let mk = "TWEAK_NAME = T\nT_FILES = a.c\nifeq ($(THEOS_PACKAGE_SCHEME),rootless)\nT_CFLAGS += -DROOTLESS\nendif\nifeq ($(FINALPACKAGE),1)\nT_CFLAGS += -DFINAL\nendif\n";
        let d = project("cond", &[("Makefile", mk), ("a.c", ""), ("T.plist", FILTER), ("control", CONTROL)]);
        let plain = migrate(&d, &[]);
        assert!(!plain.contains("-DROOTLESS") && !plain.contains("scheme"), "{}", plain);
        let root = migrate(&d, &[("THEOS_PACKAGE_SCHEME", "rootless"), ("FINALPACKAGE", "1")]);
        assert!(root.contains("-DROOTLESS") && root.contains("-DFINAL") && root.contains("scheme = \"rootless\""), "{}", root);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn subprojects_become_products_and_an_app_keeps_its_plist_and_resources() {
        let d = project(
            "sub",
            &[
                ("Makefile", "TARGET := iphone:clang:latest:6.0\nARCHS = armv7\nTWEAK_NAME = T\nT_FILES = t.x\nSUBPROJECTS = prefs\n"),
                ("t.x", ""),
                ("T.plist", FILTER),
                ("control", CONTROL),
                ("prefs/Makefile", "BUNDLE_NAME = Prefs\nPrefs_FILES = P.m\nPrefs_INSTALL_PATH = /Library/PreferenceBundles\nPrefs_CFLAGS = -Iinclude\n"),
                ("prefs/P.m", ""),
                ("prefs/Resources/Info.plist", "<dict/>"),
                ("prefs/Resources/Root.plist", "<dict/>"),
            ],
        );
        let toml = migrate(&d, &[]);
        assert!(toml.contains("[product.T]") && toml.contains("[product.Prefs]"), "{}", toml);
        assert!(toml.contains("src = [\"prefs/P.m\"]") && toml.contains("\"-Iprefs/include\""), "{}", toml);
        assert!(toml.contains("install = \"Library/PreferenceBundles\""), "{}", toml);
        assert!(toml.contains("info_plist = \"prefs/Resources/Info.plist\"") && toml.contains("resources = \"prefs/Resources\""), "{}", toml);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn what_cannot_be_migrated_is_named() {
        let d = project(
            "notes",
            &[
                ("Makefile", "LIBRARY_NAME = S\nS_FILES = s.c\nS_LINKAGE_TYPE = static\nTOOL_NAME = t\nt_FILES = t.c main.swift\nt_CODESIGN_FLAGS = -Cadhoc\nt_NEEDS_THIS = 1\nXPC_SERVICE_NAME = x\n.PHONY: go\ngo:\n\t@true\n"),
                ("s.c", ""),
                ("t.c", ""),
            ],
        );
        let set = Vec::new();
        let mut w = Walk { root: d.clone(), set: &set, notes: Vec::new() };
        collect(&d, "", &mut w).unwrap();
        let all = w.notes.join("\n");
        for want in ["static library", "main.swift is swift", "-Cadhoc", "t_NEEDS_THIS is not migrated", "XPC_SERVICE_NAME", "the rule go"] {
            assert!(all.contains(want), "{}: {}", want, all);
        }
        // no product and no Makefile are errors
        let e = project("empty", &[("Makefile", "A = 1\n")]);
        assert!(run(&Spec { dir: e.clone(), write: false, force: false, set: Vec::new() }).unwrap_err().contains("no product"));
        let n = project("none", &[("x", "")]);
        assert!(run(&Spec { dir: n.clone(), write: false, force: false, set: Vec::new() }).unwrap_err().contains("no Makefile"));
        for x in [d, e, n] {
            std::fs::remove_dir_all(&x).unwrap();
        }
    }

    #[test]
    fn an_existing_meow_toml_is_kept_without_force() {
        let d = project("keep", &[("Makefile", "TOOL_NAME = t\nt_FILES = t.c\n"), ("t.c", ""), ("meow.toml", "mine")]);
        let spec = Spec { dir: d.clone(), write: true, force: false, set: Vec::new() };
        assert!(run(&spec).unwrap_err().contains("--force"));
        assert_eq!(std::fs::read_to_string(d.join("meow.toml")).unwrap(), "mine");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
