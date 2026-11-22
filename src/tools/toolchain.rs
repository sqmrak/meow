use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// every tool and sdk meow can find is exported as a MEOW_* variable, so a meow.toml
// can name ${MEOW_CLANG} without the developer sourcing an environment script.
// an explicit MEOW_* environment variable always wins over a search

#[derive(Debug, Clone)]
pub struct Found {
    pub path: PathBuf,
    pub via: String,
}

#[derive(Debug)]
pub struct Item {
    pub label: &'static str,
    pub var: Option<&'static str>,
    pub required: bool,
    pub found: Option<Found>,
    // what was looked at, shown when nothing was found
    pub hint: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub items: Vec<Item>,
}

impl Report {
    fn get(&self, label: &str) -> Option<&Found> {
        self.items.iter().find(|i| i.label == label).and_then(|i| i.found.as_ref())
    }

    pub fn vars(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for i in &self.items {
            if let (Some(var), Some(f)) = (i.var, &i.found) {
                out.insert(var.to_string(), f.path.to_string_lossy().into_owned());
            }
        }
        out
    }

    pub fn missing_required(&self) -> Vec<&Item> {
        self.items.iter().filter(|i| i.required && i.found.is_none()).collect()
    }
}

type Get<'a> = &'a dyn Fn(&str) -> Option<String>;

#[cfg(unix)]
fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

// windows has no execute bit, and a tool is looked up without its .exe suffix
#[cfg(not(unix))]
fn is_exec(p: &Path) -> bool {
    let mut exe = p.as_os_str().to_owned();
    exe.push(".exe");
    p.is_file() || Path::new(&exe).is_file()
}

fn path_dirs(get: Get) -> Vec<PathBuf> {
    get("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}

fn on_path(get: Get, name: &str) -> Option<PathBuf> {
    path_dirs(get).into_iter().map(|d| d.join(name)).find(|p| is_exec(p))
}

fn os_dir() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

// directories that may hold the theos toolchain, in search order
fn tc_candidates(get: Get) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let sub = format!("toolchain/{}/iphone/bin", os_dir());
    if let Some(t) = get("THEOS") {
        out.push((Path::new(&t).join(&sub), "THEOS".to_string()));
    }
    if let Some(t) = get("MEOW_TOOLS") {
        out.push((Path::new(&t).join("theos").join(&sub), "MEOW_TOOLS".to_string()));
    }
    out
}

fn sdk_version(name: &str) -> Option<(u32, u32)> {
    let v = name.strip_prefix("iPhoneOS")?.strip_suffix(".sdk")?;
    let (a, b) = v.split_once('.').unwrap_or((v, "0"));
    Some((a.parse().ok()?, b.parse().ok()?))
}

// a sdk whose headers are missing, for example a dangling symlink, is not usable
fn sdk_ok(p: &Path) -> bool {
    p.join("usr/include/stdio.h").is_file()
}

// the best sdk directory inside dir: newest, or newest not above max
fn pick_sdk(dir: &Path, max: Option<(u32, u32)>) -> Option<PathBuf> {
    let mut best: Option<((u32, u32), PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(v) = sdk_version(&name) else { continue };
        if max.is_some_and(|m| v > m) || !sdk_ok(&e.path()) {
            continue;
        }
        if best.as_ref().map_or(true, |(bv, _)| v > *bv) {
            best = Some((v, e.path()));
        }
    }
    best.map(|(_, p)| p)
}

fn explicit(get: Get, var: &str, ok: impl Fn(&Path) -> bool) -> Option<Result<Found, String>> {
    let v = get(var)?;
    let p = PathBuf::from(&v);
    if ok(&p) {
        Some(Ok(Found { path: p, via: var.to_string() }))
    } else {
        Some(Err(format!("{} is set to {} which is not usable", var, v)))
    }
}

fn tool(get: Get, tc: Option<&Found>, var: &'static str, label: &'static str, names: &[&str], required: bool) -> Item {
    if let Some(r) = explicit(get, var, is_exec) {
        return match r {
            Ok(f) => Item { label, var: Some(var), required, found: Some(f), hint: String::new() },
            Err(h) => Item { label, var: Some(var), required, found: None, hint: h },
        };
    }
    for n in names {
        if let Some(tc) = tc {
            let p = tc.path.join(n);
            if is_exec(&p) {
                return Item { label, var: Some(var), required, found: Some(Found { path: p, via: format!("toolchain {}", tc.via) }), hint: String::new() };
            }
        }
    }
    for n in names {
        if let Some(p) = on_path(get, n) {
            return Item { label, var: Some(var), required, found: Some(Found { path: p, via: "PATH".to_string() }), hint: String::new() };
        }
    }
    Item {
        label,
        var: Some(var),
        required,
        found: None,
        hint: format!("looked for {} in the toolchain and on PATH, or set {}", names.join(" or "), var),
    }
}

fn host_tool(get: Get, label: &'static str, name: &str, required: bool) -> Item {
    let found = on_path(get, name).map(|p| Found { path: p, via: "PATH".to_string() });
    Item { label, var: None, required, found, hint: format!("install {} on the host", name) }
}

pub fn discover(get: Get) -> Report {
    let mut items = Vec::new();

    let tc_item = match explicit(get, "MEOW_TC", |p| is_exec(&p.join("clang"))) {
        Some(Ok(f)) => Item { label: "toolchain", var: Some("MEOW_TC"), required: true, found: Some(f), hint: String::new() },
        Some(Err(h)) => Item { label: "toolchain", var: Some("MEOW_TC"), required: true, found: None, hint: h },
        None => {
            let mut found = None;
            for (dir, via) in tc_candidates(get) {
                if is_exec(&dir.join("clang")) {
                    found = Some(Found { path: dir, via });
                    break;
                }
            }
            // a PATH directory that holds clang and ldid together is a cross toolchain,
            // the host clang usually has no ldid next to it
            if found.is_none() {
                found = path_dirs(get)
                    .into_iter()
                    .find(|d| is_exec(&d.join("clang")) && is_exec(&d.join("ldid")))
                    .map(|path| Found { path, via: "PATH".to_string() });
            }
            Item {
                label: "toolchain",
                var: Some("MEOW_TC"),
                required: true,
                found,
                hint: "set MEOW_TC to the directory with clang and ldid, or THEOS or MEOW_TOOLS to a tree that has theos/toolchain".to_string(),
            }
        }
    };
    let tc = tc_item.found.clone();
    items.push(tc_item);

    let tc_ref = tc.as_ref();
    items.push(tool(get, tc_ref, "MEOW_CLANG", "clang", &["clang"], true));
    items.push(tool(get, tc_ref, "MEOW_LDID", "ldid", &["ldid"], true));
    items.push(tool(get, tc_ref, "MEOW_LIPO", "lipo", &["lipo", "llvm-lipo"], true));
    items.push(tool(get, tc_ref, "MEOW_AR", "ar", &["llvm-ar", "ar"], true));
    items.push(tool(get, tc_ref, "MEOW_STRIP", "strip", &["strip", "llvm-strip"], true));
    items.push(tool(get, tc_ref, "MEOW_NM", "nm", &["nm", "llvm-nm"], false));

    // the substrate, libhooker and private framework headers, and the stub libraries of the common tweak frameworks
    let theos_dir = |sub: &str, probe: &str| -> Option<Found> {
        let mut dirs: Vec<(PathBuf, &str)> = Vec::new();
        if let Some(t) = get("THEOS") {
            dirs.push((Path::new(&t).join(sub), "THEOS"));
        }
        if let Some(t) = get("MEOW_TOOLS") {
            dirs.push((Path::new(&t).join("theos").join(sub), "MEOW_TOOLS"));
        }
        dirs.into_iter().find(|(d, _)| d.join(probe).exists()).map(|(path, via)| Found { path, via: via.to_string() })
    };
    for (label, var, sub, probe, hint) in [
        ("headers", "MEOW_HEADERS", "vendor/include", "substrate.h", "`meow setup headers`, or set MEOW_HEADERS to a directory with substrate.h"),
        ("vendor lib", "MEOW_VENDOR_LIB", "vendor/lib", "libsubstrate.tbd", "`meow setup headers`, or set MEOW_VENDOR_LIB to a directory with the stub libraries"),
    ] {
        let found = match explicit(get, var, |p| p.is_dir()) {
            Some(Ok(f)) => Some(f),
            Some(Err(_)) => None,
            None => theos_dir(sub, probe),
        };
        items.push(Item { label, var: Some(var), required: false, found, hint: hint.to_string() });
    }

    // cargo for kind = "rust": the one next to the rust std for ios that meow can find, then PATH
    let cargo = match explicit(get, "MEOW_CARGO", is_exec) {
        Some(Ok(f)) => Item { label: "cargo", var: Some("MEOW_CARGO"), required: false, found: Some(f), hint: String::new() },
        Some(Err(h)) => Item { label: "cargo", var: Some("MEOW_CARGO"), required: false, found: None, hint: h },
        None => {
            let mut found = None;
            if let Some(t) = get("MEOW_TOOLS") {
                let p = Path::new(&t).join("rust-official/bin/cargo");
                if is_exec(&p) {
                    found = Some(Found { path: p, via: "MEOW_TOOLS".to_string() });
                }
            }
            if found.is_none() {
                found = on_path(get, "cargo").map(|path| Found { path, via: "PATH".to_string() });
            }
            Item {
                label: "cargo",
                var: Some("MEOW_CARGO"),
                required: false,
                found,
                hint: "kind = \"rust\" needs cargo with the std for aarch64-apple-ios, set MEOW_CARGO or MEOW_TOOLS to a tree with rust-official".to_string(),
            }
        }
    };
    // the rustc that sits next to that cargo, cargo would otherwise take the one on PATH,
    // which may not have the std for ios
    let rustc = match explicit(get, "MEOW_RUSTC", is_exec) {
        Some(Ok(f)) => Item { label: "rustc", var: Some("MEOW_RUSTC"), required: false, found: Some(f), hint: String::new() },
        Some(Err(h)) => Item { label: "rustc", var: Some("MEOW_RUSTC"), required: false, found: None, hint: h },
        None => {
            let beside = cargo.found.as_ref().and_then(|c| c.path.parent().map(|d| d.join("rustc"))).filter(|p| is_exec(p));
            let found = beside.map(|path| Found { path, via: "next to cargo".to_string() }).or_else(|| on_path(get, "rustc").map(|path| Found { path, via: "PATH".to_string() }));
            Item { label: "rustc", var: Some("MEOW_RUSTC"), required: false, found, hint: "set MEOW_RUSTC, kind = \"rust\" needs it".to_string() }
        }
    };
    // go for kind = "go": a current go builds arm64, armv7 needs the last go that had ios/arm, 1.14
    let go_item = |label: &'static str, var: &'static str, sub: &str, path_fallback: bool, hint: &str| -> Item {
        if let Some(r) = explicit(get, var, is_exec) {
            return match r {
                Ok(f) => Item { label, var: Some(var), required: false, found: Some(f), hint: String::new() },
                Err(h) => Item { label, var: Some(var), required: false, found: None, hint: h },
            };
        }
        let mut found = None;
        if let Some(t) = get("MEOW_TOOLS") {
            let p = Path::new(&t).join(sub);
            if is_exec(&p) {
                found = Some(Found { path: p, via: "MEOW_TOOLS".to_string() });
            }
        }
        if found.is_none() && path_fallback {
            found = on_path(get, "go").map(|path| Found { path, via: "PATH".to_string() });
        }
        Item { label, var: Some(var), required: false, found, hint: hint.to_string() }
    };
    items.push(go_item("go", "MEOW_GO", "go/bin/go", true, "kind = \"go\" needs go for arm64, set MEOW_GO or MEOW_TOOLS to a tree with go/bin/go"));
    items.push(go_item("go armv7", "MEOW_GO_ARMV7", "go-1.14/bin/go", false, "go for armv7 is go 1.14, the last release with ios/arm, set MEOW_GO_ARMV7 or put it in $MEOW_TOOLS/go-1.14"));

    // the sources of the std, `-Zbuild-std` builds it for armv7 from them
    let rust_src = rustc.found.as_ref().and_then(|r| r.path.parent().and_then(|b| b.parent()).map(|p| p.join("lib/rustlib/src/rust/library"))).filter(|p| p.is_dir());
    items.push(cargo);
    items.push(rustc);
    items.push(Item {
        label: "rust src",
        var: None,
        required: false,
        found: rust_src.map(|path| Found { path, via: "next to rustc".to_string() }),
        hint: "armv7 rust needs rust-src, unpack rust-src-<version>.tar.xz from static.rust-lang.org and run its install.sh --prefix=<the install>".to_string(),
    });

    // swift for ios on a linux host: the host swiftc plus the ios sdk and the swift resources
    // of an xcode, made by `xtool sdk build Xcode.xip DIR`. MEOW_SWIFT_ROOT or
    // $MEOW_TOOLS/swift-sdk is that directory
    let swiftc = tool(get, None, "MEOW_SWIFTC", "swiftc", &["swiftc"], false);
    let swift_root = get("MEOW_SWIFT_ROOT").map(PathBuf::from).or_else(|| get("MEOW_TOOLS").map(|t| Path::new(&t).join("swift-sdk")));
    let under = |sub: &str| swift_root.as_ref().map(|r| r.join(sub)).filter(|p| p.is_dir());
    let swift_sdk = explicit(get, "MEOW_SWIFT_SDK", sdk_ok).unwrap_or_else(|| {
        match under("Developer/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS.sdk") {
            Some(p) if sdk_ok(&p) => Ok(Found { path: p, via: "swift sdk root".to_string() }),
            _ => Err("set MEOW_SWIFT_SDK, or MEOW_SWIFT_ROOT to the directory `xtool sdk build` made".to_string()),
        }
    });
    // the clang builtin headers of the host swift: the resource dir of the xcode has none, and the
    // clang importer of swift cannot build the modules of the sdk without them, so `meow setup swift`
    // links them into the sdk
    let swift_clang = explicit(get, "MEOW_SWIFT_CLANG", |p| p.join("include").is_dir()).unwrap_or_else(|| {
        let beside = swiftc
            .found
            .as_ref()
            .and_then(|f| std::fs::canonicalize(&f.path).ok())
            .and_then(|p| p.parent().and_then(|b| b.parent()).map(|r| r.join("lib/swift/clang")))
            .filter(|p| p.join("include").is_dir());
        match beside {
            Some(path) => Ok(Found { path, via: "next to swiftc".to_string() }),
            None => Err("set MEOW_SWIFT_CLANG to the clang resource directory of the host swift, the one with include/".to_string()),
        }
    });
    items.push(swiftc);
    items.push(match swift_clang {
        Ok(f) => Item { label: "swift clang", var: Some("MEOW_SWIFT_CLANG"), required: false, found: Some(f), hint: String::new() },
        Err(h) => Item { label: "swift clang", var: Some("MEOW_SWIFT_CLANG"), required: false, found: None, hint: h },
    });
    items.push(match swift_sdk {
        Ok(f) => Item { label: "swift sdk", var: Some("MEOW_SWIFT_SDK"), required: false, found: Some(f), hint: String::new() },
        Err(h) => Item { label: "swift sdk", var: Some("MEOW_SWIFT_SDK"), required: false, found: None, hint: h },
    });
    let swift_res = explicit(get, "MEOW_SWIFT_RESOURCES", |p| p.join("iphoneos").is_dir()).unwrap_or_else(|| {
        match under("Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift") {
            Some(p) if p.join("iphoneos").is_dir() => Ok(Found { path: p, via: "swift sdk root".to_string() }),
            _ => Err("set MEOW_SWIFT_RESOURCES, or MEOW_SWIFT_ROOT to the directory `xtool sdk build` made".to_string()),
        }
    });
    items.push(match swift_res {
        Ok(f) => Item { label: "swift resources", var: Some("MEOW_SWIFT_RESOURCES"), required: false, found: Some(f), hint: String::new() },
        Err(h) => Item { label: "swift resources", var: Some("MEOW_SWIFT_RESOURCES"), required: false, found: None, hint: h },
    });

    // host tests need a compiler that is not the cross one: a theos directory first on
    // PATH shadows the system assembler and linker
    let host = |label: &'static str, var: &'static str, names: &[&str]| -> Item {
        if let Some(r) = explicit(get, var, is_exec) {
            return match r {
                Ok(f) => Item { label, var: Some(var), required: false, found: Some(f), hint: String::new() },
                Err(h) => Item { label, var: Some(var), required: false, found: None, hint: h },
            };
        }
        let skip = tc_ref.map(|t| t.path.clone());
        let found = path_dirs(get)
            .into_iter()
            .filter(|d| Some(d) != skip.as_ref() && !d.to_string_lossy().contains("/iphone/bin"))
            .find_map(|d| names.iter().map(|n| d.join(n)).find(|p| is_exec(p)))
            .map(|path| Found { path, via: "PATH, cross toolchain skipped".to_string() });
        Item { label, var: Some(var), required: false, found, hint: format!("set {} to a host compiler, tests need one", var) }
    };
    items.push(host("host cc", "MEOW_HOST_CC", &["cc", "clang", "gcc"]));
    items.push(host("host ar", "MEOW_HOST_AR", &["ar"]));

    let theos_sdks = |get: Get| -> Vec<(PathBuf, String)> {
        let mut v = Vec::new();
        if let Some(t) = get("THEOS") {
            v.push((Path::new(&t).join("sdks"), "THEOS".to_string()));
        }
        if let Some(t) = get("MEOW_TOOLS") {
            v.push((Path::new(&t).join("theos/sdks"), "MEOW_TOOLS".to_string()));
        }
        v
    };

    // armv7 links against an old sdk, the newest one up to 10.3 that has headers
    let v7 = match explicit(get, "MEOW_SDK_ARMV7", sdk_ok) {
        Some(Ok(f)) => (Some(f), String::new()),
        Some(Err(h)) => (None, h),
        None => {
            let mut found = None;
            if let Some(t) = get("MEOW_TOOLS") {
                let p = Path::new(&t).join("sdks-armv7");
                if sdk_ok(&p) {
                    found = Some(Found { path: p, via: "MEOW_TOOLS".to_string() });
                }
            }
            if found.is_none() {
                for (dir, via) in theos_sdks(get) {
                    if let Some(p) = pick_sdk(&dir, Some((10, 3))) {
                        found = Some(Found { path: p, via });
                        break;
                    }
                }
            }
            (found, "set MEOW_SDK_ARMV7 to a sdk with usr/include, or MEOW_TOOLS to a tree with sdks-armv7".to_string())
        }
    };
    items.push(Item { label: "sdk armv7", var: Some("MEOW_SDK_ARMV7"), required: false, found: v7.0, hint: v7.1 });

    let v64 = match explicit(get, "MEOW_SDK_ARM64", sdk_ok) {
        Some(Ok(f)) => (Some(f), String::new()),
        Some(Err(h)) => (None, h),
        None => {
            let mut dirs = Vec::new();
            if let Some(t) = get("MEOW_TOOLS") {
                dirs.push((Path::new(&t).join("sdks"), "MEOW_TOOLS".to_string()));
            }
            dirs.extend(theos_sdks(get));
            let mut found = None;
            for (dir, via) in dirs {
                if let Some(p) = pick_sdk(&dir, None) {
                    found = Some(Found { path: p, via });
                    break;
                }
            }
            (found, "set MEOW_SDK_ARM64 to a sdk with usr/include, or MEOW_TOOLS to a tree with sdks".to_string())
        }
    };
    items.push(Item { label: "sdk arm64", var: Some("MEOW_SDK_ARM64"), required: false, found: v64.0, hint: v64.1 });

    // armv6 needs a sdk up to 5.1, later sdks dropped the slice. the fat libraries of an
    // old sdk are thinned into the cache before the link, see sdkthin.rs
    let v6 = match explicit(get, "MEOW_SDK_ARMV6", sdk_ok) {
        Some(Ok(f)) => (Some(f), String::new()),
        Some(Err(h)) => (None, h),
        None => {
            let mut found = None;
            if let Some(t) = get("MEOW_TOOLS") {
                let p = Path::new(&t).join("sdks-armv6");
                if sdk_ok(&p) {
                    found = Some(Found { path: p, via: "MEOW_TOOLS".to_string() });
                }
            }
            if found.is_none() {
                for (dir, via) in theos_sdks(get) {
                    if let Some(p) = pick_sdk(&dir, Some((5, 1))) {
                        found = Some(Found { path: p, via });
                        break;
                    }
                }
            }
            (found, "armv6 needs an sdk up to 5.1, set MEOW_SDK_ARMV6 or put iPhoneOS5.1.sdk in THEOS/sdks".to_string())
        }
    };
    items.push(Item { label: "sdk armv6", var: Some("MEOW_SDK_ARMV6"), required: false, found: v6.0, hint: v6.1 });

    // armv7 executables link without the default start files, senko does the same
    let crt = match explicit(get, "MEOW_CRT_ARMV7", |p| p.is_file()) {
        Some(Ok(f)) => Some(f),
        Some(Err(_)) => None,
        None => items
            .iter()
            .find(|i| i.label == "sdk armv7")
            .and_then(|i| i.found.as_ref())
            .map(|s| s.path.join("usr/lib/crt1.o"))
            .filter(|p| p.is_file())
            .map(|path| Found { path, via: "sdk armv7".to_string() }),
    };
    items.push(Item {
        label: "crt1 armv7",
        var: Some("MEOW_CRT_ARMV7"),
        required: false,
        found: crt,
        hint: "usr/lib/crt1.o of the armv7 sdk, or set MEOW_CRT_ARMV7".to_string(),
    });

    let cache = get("MEOW_CACHE")
        .map(|c| (PathBuf::from(c), "MEOW_CACHE"))
        .or_else(|| get("XDG_CACHE_HOME").map(|c| (Path::new(&c).join("meow"), "XDG_CACHE_HOME")))
        .or_else(|| get("LOCALAPPDATA").map(|c| (Path::new(&c).join("meow"), "LOCALAPPDATA")))
        .or_else(|| get("HOME").map(|h| (Path::new(&h).join(".cache/meow"), "HOME")));
    items.push(Item {
        label: "cache dir",
        var: Some("MEOW_CACHE_DIR"),
        required: false,
        found: cache.map(|(path, via)| Found { path, via: via.to_string() }),
        hint: "set MEOW_CACHE".to_string(),
    });

    // macos ships bsdtar as tar, deb packing needs gnu tar, which homebrew installs as gtar
    let mut tar = host_tool(get, "tar", "gtar", true);
    if tar.found.is_none() {
        tar = host_tool(get, "tar", "tar", true);
    }
    items.push(tar);
    items.push(host_tool(get, "gzip", "gzip", true));
    items.push(host_tool(get, "dpkg-deb", "dpkg-deb", false));
    Report { items }
}

fn real_env(name: &str) -> Option<String> {
    let set = std::env::var(name).ok().filter(|v| !v.is_empty());
    if set.is_none() && name == "MEOW_TOOLS" {
        return default_tools_root().filter(|p| p.is_dir()).map(|p| p.to_string_lossy().into_owned());
    }
    set
}

// where `meow setup` installs when MEOW_TOOLS is not set, and where meow looks without it
pub fn default_tools_root() -> Option<PathBuf> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    get("XDG_DATA_HOME")
        .map(|d| Path::new(&d).join("meow/tools"))
        .or_else(|| get("HOME").map(|h| Path::new(&h).join(".local/share/meow/tools")))
}

// the tools directory in use: MEOW_TOOLS, or the default one when it exists
pub fn tools_root() -> Option<PathBuf> {
    real_env("MEOW_TOOLS").map(PathBuf::from)
}

// the sdk of one version, `9.3` for iPhoneOS9.3.sdk, where `meow sdk` puts it or where an
// older setup keeps it. a sdk without headers is skipped
pub fn sdk_named(version: &str) -> Option<PathBuf> {
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let dir = format!("iPhoneOS{}.sdk", version);
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(t) = real_env("MEOW_TOOLS") {
        for sub in ["sdks", "sdks-armv7", "theos/sdks"] {
            roots.push(Path::new(&t).join(sub));
        }
    }
    if let Some(t) = real_env("THEOS") {
        roots.push(Path::new(&t).join("sdks"));
    }
    roots.into_iter().map(|r| r.join(&dir)).find(|p| sdk_ok(p))
}

// searched once per process, only when a MEOW_* variable is not in the environment
pub fn discovered_vars() -> &'static BTreeMap<String, String> {
    static VARS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    VARS.get_or_init(|| discover(&real_env).vars())
}

pub fn host_tool_path(name: &str) -> Option<PathBuf> {
    on_path(&real_env, name)
}

pub fn doctor(color: bool) -> Result<(), String> {
    let paint = |code: &str, t: &str| if color { format!("\x1b[{}m{}\x1b[0m", code, t) } else { t.to_string() };
    let report = discover(&real_env);
    println!("{}", paint("1;38;5;213", "meow doctor"));
    let width = report.items.iter().map(|i| i.label.len()).max().unwrap_or(0);
    for i in &report.items {
        match &i.found {
            Some(f) => println!(
                "  {} {:w$}  {}  {}",
                paint("38;5;114", "✓"),
                i.label,
                f.path.display(),
                paint("38;5;245", &format!("via {}", f.via)),
                w = width
            ),
            None => {
                let (mark, code) = if i.required { ("✗", "38;5;203") } else { ("-", "38;5;221") };
                println!("  {} {:w$}  {}", paint(code, mark), i.label, paint("38;5;245", &i.hint), w = width);
            }
        }
    }
    let missing = report.missing_required();
    if report.get("sdk armv7").is_none() && report.get("sdk arm64").is_none() {
        return Err("no sdk found, set MEOW_SDK_ARM64 or MEOW_TOOLS".to_string());
    }
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|i| i.label).collect();
        return Err(format!("missing: {}", names.join(", ")));
    }
    println!("{}", paint("1;38;5;114", "ready"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn touch_exec(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn fake_sdk(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join(name).join("usr/include")).unwrap();
        std::fs::write(dir.join(name).join("usr/include/stdio.h"), "").unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meow-tc-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn lookup(map: HashMap<String, String>) -> impl Fn(&str) -> Option<String> {
        move |k| map.get(k).cloned()
    }

    #[test]
    fn finds_the_theos_toolchain_and_picks_sdks_by_version() {
        let d = tmp("theos");
        let bin = d.join(format!("theos/toolchain/{}/iphone/bin", os_dir()));
        for t in ["clang", "ldid", "lipo", "llvm-ar", "strip"] {
            touch_exec(&bin.join(t));
        }
        let sdks = d.join("theos/sdks");
        for n in ["iPhoneOS6.1.sdk", "iPhoneOS10.3.sdk", "iPhoneOS16.5.sdk"] {
            fake_sdk(&sdks, n);
        }
        // a dangling sdk must lose to an older complete one
        std::fs::create_dir_all(sdks.join("iPhoneOS14.5.sdk")).unwrap();
        let env = lookup(HashMap::from([("THEOS".into(), d.join("theos").to_string_lossy().into_owned())]));
        let r = discover(&env);
        assert_eq!(r.get("toolchain").unwrap().path, bin);
        assert_eq!(r.get("clang").unwrap().path, bin.join("clang"));
        assert_eq!(r.get("ar").unwrap().path, bin.join("llvm-ar"));
        assert_eq!(r.get("sdk armv7").unwrap().path, sdks.join("iPhoneOS10.3.sdk"));
        assert_eq!(r.get("sdk arm64").unwrap().path, sdks.join("iPhoneOS16.5.sdk"));
        let v = r.vars();
        assert_eq!(v["MEOW_LDID"], bin.join("ldid").to_string_lossy());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn explicit_variable_wins_and_a_bad_one_is_reported() {
        let d = tmp("explicit");
        touch_exec(&d.join("mytools/clang"));
        touch_exec(&d.join("mytools/ldid"));
        let env = lookup(HashMap::from([
            ("MEOW_TC".into(), d.join("mytools").to_string_lossy().into_owned()),
            ("MEOW_SDK_ARM64".into(), d.join("nope.sdk").to_string_lossy().into_owned()),
        ]));
        let r = discover(&env);
        assert_eq!(r.get("toolchain").unwrap().path, d.join("mytools"));
        assert_eq!(r.get("ldid").unwrap().path, d.join("mytools/ldid"));
        let sdk = r.items.iter().find(|i| i.label == "sdk arm64").unwrap();
        assert!(sdk.found.is_none());
        assert!(sdk.hint.contains("MEOW_SDK_ARM64 is set to"), "{}", sdk.hint);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn host_clang_alone_is_not_a_cross_toolchain() {
        let d = tmp("hostclang");
        touch_exec(&d.join("bin/clang"));
        let env = lookup(HashMap::from([("PATH".into(), d.join("bin").to_string_lossy().into_owned())]));
        let r = discover(&env);
        assert!(r.get("toolchain").is_none());
        assert!(r.get("clang").is_some());
        assert!(r.missing_required().iter().any(|i| i.label == "toolchain"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn sdk_names_parse() {
        assert_eq!(sdk_version("iPhoneOS16.5.sdk"), Some((16, 5)));
        assert_eq!(sdk_version("iPhoneOS7.sdk"), Some((7, 0)));
        assert_eq!(sdk_version("MacOSX.sdk"), None);
    }
}
