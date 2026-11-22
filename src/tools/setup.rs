use crate::fetch;
use crate::sdkget;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// `meow setup` lists what meow builds with and installs it where meow looks without any
// environment: the clang and ldid of the theos toolchain, the theos headers and stub
// libraries, ios sdks, rust with the std for ios, two gos, and the swift sdk. every download
// is pinned by sha256 and every git checkout by commit, see sdk/setup.toml

const MANIFEST: &str = include_str!("../../sdk/setup.toml");

#[derive(Deserialize)]
struct Manifest {
    sdks: Vec<String>,
    toolchain: Vec<Download>,
    headers: Vec<GitPin>,
    rust: Rust,
    go: Vec<GoEntry>,
    xtool: Vec<Download>,
}

#[derive(Deserialize)]
struct Download {
    os: String,
    arch: String,
    url: String,
    sha256: String,
}

#[derive(Deserialize)]
struct GitPin {
    name: String,
    git: String,
    rev: String,
    to: String,
}

#[derive(Deserialize)]
struct Rust {
    version: String,
    ios_std_sha256: String,
    src_sha256: String,
    host: Vec<RustHost>,
}

#[derive(Deserialize)]
struct RustHost {
    os: String,
    arch: String,
    triple: String,
    rustc_sha256: String,
    cargo_sha256: String,
    std_sha256: String,
}

#[derive(Deserialize)]
struct GoEntry {
    name: String,
    os: String,
    arch: String,
    url: String,
    sha256: String,
}

fn manifest() -> Result<Manifest, String> {
    toml::from_str(MANIFEST).map_err(|e| format!("sdk/setup.toml: {}", e))
}

fn host() -> (&'static str, &'static str) {
    (std::env::consts::OS, std::env::consts::ARCH)
}

const COMPONENTS: [(&str, &str); 6] = [
    ("toolchain", "clang 11.1.0, ldid, lipo and strip for ios, from the theos toolchain (linux)"),
    ("headers", "the theos headers and stub libraries: substrate, libhooker, private frameworks"),
    ("sdks", "ios sdks 5.1, 6.1 and 16.5, and a thin copy of 6.1 for armv7, `--sdk V` names others"),
    ("rust", "rustc and cargo with the std for aarch64-apple-ios, and rust-src for armv7"),
    ("go", "go 1.27 for arm64 and go 1.14 for armv7"),
    ("swift", "the ios sdk and swift resources of an Xcode, needs --xcode Xcode.xip (linux)"),
];

#[derive(Debug)]
pub struct Spec {
    components: Vec<String>,
    to: Option<PathBuf>,
    cache: Option<PathBuf>,
    force: bool,
    xcode: Option<PathBuf>,
    sdks: Vec<String>,
}

pub fn parse_args(args: &[String]) -> Result<Spec, String> {
    let mut s = Spec { components: Vec::new(), to: None, cache: None, force: false, xcode: None, sdks: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or_else(|| format!("setup: {} needs a value", flag));
        match a.as_str() {
            "--to" => s.to = Some(PathBuf::from(value("--to")?)),
            "--cache" => s.cache = Some(PathBuf::from(value("--cache")?)),
            "--xcode" => s.xcode = Some(PathBuf::from(value("--xcode")?)),
            "--sdk" => s.sdks.push(value("--sdk")?),
            "--force" => s.force = true,
            "-h" | "--help" => return Err(USAGE.to_string()),
            o if o.starts_with('-') => return Err(format!("setup: unknown option {}\n{}", o, USAGE)),
            c => {
                if c != "all" && !COMPONENTS.iter().any(|(n, _)| *n == c) {
                    return Err(format!("setup: no component {}, the components are {} and all", c, COMPONENTS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")));
                }
                s.components.push(c.to_string());
            }
        }
    }
    Ok(s)
}

pub const USAGE: &str = "usage: meow setup [COMPONENT...|all] [--to DIR] [--force] [--xcode Xcode.xip] [--sdk VERSION]...

  no component lists them and what is installed
  --to DIR       the tools directory, MEOW_TOOLS or the default one when left out
  --force        install again what is already there
  --xcode PATH   Xcode.xip or Xcode.app, the swift component needs it
  --sdk V        an ios sdk for the sdks component instead of the default set";

fn root_of(spec: &Spec) -> Result<PathBuf, String> {
    spec.to
        .clone()
        .or_else(crate::toolchain::tools_root)
        .or_else(crate::toolchain::default_tools_root)
        .ok_or_else(|| "setup: no place to install, set HOME or pass --to DIR".to_string())
}

fn has(root: &Path, rel: &str) -> bool {
    root.join(rel).exists()
}

fn state(name: &str, root: &Path, m: &Manifest) -> String {
    let ok = match name {
        "toolchain" => has(root, "theos/toolchain/linux/iphone/bin/clang"),
        "headers" => has(root, "theos/vendor/include/substrate.h") && has(root, "theos/vendor/lib/libsubstrate.tbd"),
        "sdks" => {
            let n = m.sdks.iter().filter(|v| has(root, &format!("theos/sdks/iPhoneOS{}.sdk/usr/include/stdio.h", v))).count();
            return if n == m.sdks.len() { "installed".to_string() } else if n > 0 { format!("{} of {}", n, m.sdks.len()) } else { String::new() };
        }
        "rust" => has(root, "rust-official/bin/cargo") && has(root, "rust-official/lib/rustlib/aarch64-apple-ios") && has(root, "rust-official/lib/rustlib/src/rust/library"),
        "go" => has(root, "go/bin/go") && has(root, "go-1.14/bin/go"),
        "swift" => has(root, "swift-sdk/Developer/Toolchains"),
        _ => false,
    };
    if ok { "installed".to_string() } else { String::new() }
}

pub fn list(spec: &Spec) -> Result<(), String> {
    let m = manifest()?;
    let root = root_of(spec)?;
    println!("{:<10} {:<10} {}", "COMPONENT", "STATE", "ABOUT");
    for (name, about) in COMPONENTS {
        println!("{:<10} {:<10} {}", name, state(name, &root, &m), about);
    }
    println!("\ninstall with `meow setup all`, or name components, into {}", root.display());
    Ok(())
}

fn cache_of(spec: &Spec) -> Result<PathBuf, String> {
    match &spec.cache {
        Some(c) => Ok(c.clone()),
        None => crate::cache::default_dir().ok_or_else(|| "setup: no cache directory, set MEOW_CACHE or pass --cache".to_string()),
    }
}

fn find<'a, T>(items: &'a [T], pick: impl Fn(&T) -> (&str, &str)) -> Option<&'a T> {
    let (os, arch) = host();
    items.iter().find(|i| pick(i) == (os, arch))
}

fn tmp_dir(root: &Path, tag: &str) -> Result<PathBuf, String> {
    let d = root.join(format!(".{}.tmp", tag));
    fetch::wipe(&d)?;
    fs::create_dir_all(root).map_err(|e| format!("{}: {}", root.display(), e))?;
    Ok(d)
}

fn replace(from: &Path, to: &Path) -> Result<(), String> {
    fetch::wipe(to)?;
    if let Some(p) = to.parent() {
        fs::create_dir_all(p).map_err(|e| format!("{}: {}", p.display(), e))?;
    }
    fs::rename(from, to).map_err(|e| format!("{}: {}", to.display(), e))
}

fn install_toolchain(m: &Manifest, root: &Path, cache: &Path) -> Result<(), String> {
    if host().0 != "linux" {
        return Err("the iOS toolchain comes from Xcode on macos: install the command line tools, and ldid with `brew install ldid`".to_string());
    }
    let d = find(&m.toolchain, |d| (d.os.as_str(), d.arch.as_str())).ok_or_else(|| format!("no toolchain for linux {}", host().1))?;
    let archive = fetch::download(&d.url, &d.sha256, cache)?;
    let tmp = tmp_dir(root, "toolchain")?;
    fetch::extract(&archive, &tmp, 0)?;
    if !tmp.join("linux/iphone/bin/clang").is_file() {
        fetch::wipe(&tmp)?;
        return Err("the toolchain archive has no linux/iphone/bin/clang".to_string());
    }
    replace(&tmp.join("linux"), &root.join("theos/toolchain/linux"))?;
    fetch::wipe(&tmp)
}

fn install_headers(m: &Manifest, root: &Path, cache: &Path) -> Result<(), String> {
    for g in &m.headers {
        let spec = fetch::Spec {
            name: g.name.clone(),
            git: Some(g.git.clone()),
            rev: Some(g.rev.clone()),
            cache: cache.to_string_lossy().into_owned(),
            to: root.join(&g.to).to_string_lossy().into_owned(),
            ..Default::default()
        };
        eprintln!("setup headers: {} at {}", g.git, &g.rev[..12]);
        fetch::run_fetch(&spec)?;
    }
    Ok(())
}

fn install_sdks(spec: &Spec, m: &Manifest, root: &Path, cache: &Path) -> Result<(), String> {
    let versions = if spec.sdks.is_empty() { m.sdks.clone() } else { spec.sdks.clone() };
    sdkget::run(&sdkget::Spec { versions, to: Some(root.join("theos/sdks")), cache: Some(cache.to_path_buf()), force: spec.force })?;
    // ld64 cannot map a slice of the fat libraries of an old sdk on a host with 16 KiB pages, the
    // armv7 link takes a copy with one slice per file, which meow looks for as sdks-armv7
    let old = root.join("theos/sdks/iPhoneOS6.1.sdk");
    let thin = root.join("sdks-armv7");
    if old.join("usr/include/stdio.h").is_file() && (spec.force || !thin.join("usr/include/stdio.h").is_file()) {
        let st = crate::sdkthin::run(&old, &thin, "armv7")?;
        eprintln!("setup sdks: sdks-armv7 is iPhoneOS6.1.sdk with one slice per file, {} cut, {} linked", st.thinned, st.linked);
    }
    Ok(())
}

fn install_rust(m: &Manifest, root: &Path, cache: &Path) -> Result<(), String> {
    let h = find(&m.rust.host, |h| (h.os.as_str(), h.arch.as_str())).ok_or_else(|| format!("no rust for {} {}", host().0, host().1))?;
    let v = &m.rust.version;
    let prefix = root.join("rust-official");
    let parts = [
        (format!("rustc-{}-{}", v, h.triple), h.rustc_sha256.clone(), "rustc".to_string()),
        (format!("cargo-{}-{}", v, h.triple), h.cargo_sha256.clone(), "cargo".to_string()),
        (format!("rust-std-{}-{}", v, h.triple), h.std_sha256.clone(), format!("rust-std-{}", h.triple)),
        (format!("rust-std-{}-aarch64-apple-ios", v), m.rust.ios_std_sha256.clone(), "rust-std-aarch64-apple-ios".to_string()),
        (format!("rust-src-{}", v), m.rust.src_sha256.clone(), "rust-src".to_string()),
    ];
    for (name, sha, component) in parts {
        eprintln!("setup rust: {}", component);
        let archive = fetch::download(&format!("https://static.rust-lang.org/dist/{}.tar.xz", name), &sha, cache)?;
        let tmp = tmp_dir(root, &format!("rust-{}", component))?;
        fetch::extract(&archive, &tmp, 1)?;
        let r = fetch::run(
            Command::new("sh").arg(tmp.join("install.sh")).arg(format!("--prefix={}", prefix.display())).arg(format!("--components={}", component)).arg("--disable-ldconfig"),
            "rust install.sh",
        );
        fetch::wipe(&tmp)?;
        r?;
    }
    Ok(())
}

fn install_go(m: &Manifest, root: &Path, cache: &Path) -> Result<(), String> {
    let (os, arch) = host();
    for name in ["go", "go-1.14"] {
        let e = m.go.iter().find(|g| g.name == name && g.os == os && g.arch == arch).ok_or_else(|| format!("no {} for {} {}", name, os, arch))?;
        eprintln!("setup go: {}", name);
        let archive = fetch::download(&e.url, &e.sha256, cache)?;
        let tmp = tmp_dir(root, name)?;
        fetch::extract(&archive, &tmp, 1)?;
        if !tmp.join("bin/go").is_file() {
            fetch::wipe(&tmp)?;
            return Err(format!("the {} archive has no bin/go", name));
        }
        replace(&tmp, &root.join(name))?;
    }
    Ok(())
}

// xtool builds the sdk from an Xcode, the xip is Apple's and has to come from the user
fn install_swift(spec: &Spec, m: &Manifest, root: &Path, cache: &Path) -> Result<(), String> {
    if host().0 != "linux" {
        return Err("swift for ios uses the Xcode toolchain on macos, nothing to install".to_string());
    }
    let Some(xcode) = &spec.xcode else {
        return Err("swift needs --xcode PATH, an Xcode.xip or Xcode.app downloaded from Apple".to_string());
    };
    if !xcode.exists() {
        return Err(format!("{} does not exist", xcode.display()));
    }
    let d = find(&m.xtool, |d| (d.os.as_str(), d.arch.as_str())).ok_or_else(|| format!("no xtool for linux {}", host().1))?;
    let image = fetch::download(&d.url, &d.sha256, cache)?;
    let bin = root.join("xtool/xtool");
    fs::create_dir_all(root.join("xtool")).map_err(|e| format!("{}: {}", root.display(), e))?;
    fs::copy(&image, &bin).map_err(|e| format!("{}: {}", bin.display(), e))?;
    #[cfg(unix)]
    std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).map_err(|e| format!("{}: {}", bin.display(), e))?;
    let out = tmp_dir(root, "swift-sdk")?;
    eprintln!("setup swift: xtool sdk build, this unpacks the Xcode and takes a while");
    fetch::run(Command::new(&bin).env("APPIMAGE_EXTRACT_AND_RUN", "1").args(["sdk", "build"]).arg(xcode).arg(&out), "xtool sdk build")?;
    // xtool writes darwin.xtoolsdk, possibly one more level down
    let sdk = [out.clone(), out.join("darwin.xtoolsdk"), out.join("darwin.xtoolsdk/darwin.xtoolsdk")].into_iter().find(|p| p.join("swift-sdk.json").is_file());
    let Some(sdk) = sdk else {
        return Err("xtool made no swift sdk".to_string());
    };
    replace(&sdk, &root.join("swift-sdk"))?;
    fetch::wipe(&out)?;
    Ok(())
}

// the resource directory of the Xcode has no clang builtin headers, and the clang importer of
// swiftc builds the modules of the sdk wrongly without them. the headers of the host swift
// are linked in as usr/lib/clang/<n>/include
fn link_swift_headers(root: &Path) -> Result<bool, String> {
    let clang = root.join("swift-sdk/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/clang");
    let Ok(dirs) = fs::read_dir(&clang) else { return Ok(false) };
    let host = std::env::var("MEOW_SWIFT_CLANG").ok().map(PathBuf::from).or_else(|| {
        let real = fs::canonicalize(crate::toolchain::host_tool_path("swiftc")?).ok()?;
        Some(real.parent()?.parent()?.join("lib/swift/clang"))
    });
    let Some(include) = host.map(|h| h.join("include")).filter(|i| i.is_dir()) else { return Ok(false) };
    let mut linked = false;
    for d in dirs.flatten() {
        let p = d.path();
        // clang/17.0.0 is a link to clang/17
        if !p.is_dir() || p.is_symlink() || p.join("include").exists() {
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&include, p.join("include")).map_err(|e| format!("{}: {}", p.display(), e))?;
        linked = true;
    }
    Ok(linked)
}

pub fn run(spec: &Spec) -> Result<(), String> {
    if spec.components.is_empty() {
        return list(spec);
    }
    let m = manifest()?;
    let root = root_of(spec)?;
    let cache = cache_of(spec)?;
    let all = spec.components.iter().any(|c| c == "all");
    let wanted: Vec<&str> = COMPONENTS.iter().map(|(n, _)| *n).filter(|n| all || spec.components.iter().any(|c| c == n)).collect();
    let mut failed: Vec<String> = Vec::new();
    for name in wanted {
        if !spec.force && state(name, &root, &m) == "installed" {
            eprintln!("setup {}: already installed", name);
            if name == "swift" && link_swift_headers(&root)? {
                eprintln!("setup swift: linked the clang headers of the host swift into the sdk");
            }
            continue;
        }
        // in `all` mode a component that cannot be installed here is skipped with the reason
        let skippable = all && ((name == "swift" && spec.xcode.is_none()) || (host().0 != "linux" && matches!(name, "toolchain" | "swift")));
        let r = match name {
            "toolchain" => install_toolchain(&m, &root, &cache),
            "headers" => install_headers(&m, &root, &cache),
            "sdks" => install_sdks(spec, &m, &root, &cache),
            "rust" => install_rust(&m, &root, &cache),
            "go" => install_go(&m, &root, &cache),
            _ => install_swift(spec, &m, &root, &cache),
        };
        match r {
            Ok(()) => {
                eprintln!("setup {}: installed in {}", name, root.display());
                if name == "swift" {
                    if link_swift_headers(&root)? {
                        eprintln!("setup swift: linked the clang headers of the host swift into the sdk");
                    } else {
                        eprintln!("setup swift: the host swiftc is missing, so the sdk has no clang headers yet: install a swift toolchain from swift.org or your package manager and run `meow setup swift` again");
                    }
                }
            }
            Err(e) if skippable => eprintln!("setup {}: skipped, {}", name, e),
            Err(e) => {
                eprintln!("setup {}: {}", name, e);
                failed.push(name.to_string());
            }
        }
    }
    if crate::toolchain::tools_root().as_deref() != Some(root.as_path()) {
        eprintln!("\nset MEOW_TOOLS={} so that meow finds it", root.display());
    }
    if failed.is_empty() {
        eprintln!("run `meow doctor` to see what meow found");
        Ok(())
    } else {
        Err(format!("not installed: {}", failed.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_is_complete_and_pinned() {
        let m = manifest().unwrap();
        for d in m.toolchain.iter().chain(m.xtool.iter()) {
            assert_eq!(d.sha256.len(), 64, "{}", d.url);
            assert!(d.url.starts_with("https://"));
        }
        for g in &m.headers {
            assert_eq!(g.rev.len(), 40);
        }
        for g in &m.go {
            assert_eq!(g.sha256.len(), 64);
        }
        for (os, arch) in [("linux", "aarch64"), ("linux", "x86_64"), ("macos", "aarch64"), ("macos", "x86_64")] {
            let h = m.rust.host.iter().find(|h| h.os == os && h.arch == arch).unwrap_or_else(|| panic!("rust for {} {}", os, arch));
            assert!(h.rustc_sha256.len() == 64 && h.cargo_sha256.len() == 64 && h.std_sha256.len() == 64);
            for name in ["go", "go-1.14"] {
                assert!(m.go.iter().any(|g| g.name == name && g.os == os && g.arch == arch), "{} for {} {}", name, os, arch);
            }
        }
        assert_eq!(m.rust.ios_std_sha256.len(), 64);
        assert!(m.sdks.contains(&"16.5".to_string()));
    }

    #[test]
    fn arguments() {
        let s = parse_args(&["rust".into(), "go".into(), "--to".into(), "/x".into(), "--sdk".into(), "9.3".into(), "--force".into()]).unwrap();
        assert_eq!(s.components, ["rust", "go"]);
        assert!(s.force && s.to == Some(PathBuf::from("/x")) && s.sdks == ["9.3"]);
        assert!(parse_args(&["nope".into()]).unwrap_err().contains("no component"));
        assert!(parse_args(&["--bogus".into()]).is_err());
        assert!(parse_args(&["all".into()]).is_ok());
    }

    #[test]
    fn state_reads_the_files_of_a_component() {
        let m = manifest().unwrap();
        let d = std::env::temp_dir().join(format!("meow-setup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        assert_eq!(state("go", &d, &m), "");
        fs::create_dir_all(d.join("go/bin")).unwrap();
        fs::create_dir_all(d.join("go-1.14/bin")).unwrap();
        fs::write(d.join("go/bin/go"), "").unwrap();
        assert_eq!(state("go", &d, &m), "");
        fs::write(d.join("go-1.14/bin/go"), "").unwrap();
        assert_eq!(state("go", &d, &m), "installed");
        fs::create_dir_all(d.join("theos/sdks/iPhoneOS5.1.sdk/usr/include")).unwrap();
        fs::write(d.join("theos/sdks/iPhoneOS5.1.sdk/usr/include/stdio.h"), "").unwrap();
        assert_eq!(state("sdks", &d, &m), format!("1 of {}", m.sdks.len()));
        fs::remove_dir_all(&d).unwrap();
    }
}
