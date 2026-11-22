use crate::fetch;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// `meow sdk` lists the sdks of sdk/manifest.toml, `meow sdk VERSION` installs one where the
// toolchain lookup finds it. archives are pinned by sha256 and git checkouts by commit id

const MANIFEST: &str = include_str!("../../sdk/manifest.toml");
const STAMP: &str = ".meow-sdk-src";

#[derive(Deserialize)]
struct Manifest {
    sdk: Vec<Entry>,
}

#[derive(Deserialize)]
struct Part {
    url: String,
    sha256: String,
}

#[derive(Deserialize)]
struct Entry {
    version: String,
    source: String,
    #[serde(default)]
    patched: bool,
    url: Option<String>,
    sha256: Option<String>,
    #[serde(default)]
    parts: Vec<Part>,
    #[serde(default)]
    strip: u32,
    git: Option<String>,
    rev: Option<String>,
    path: Option<String>,
}

impl Entry {
    fn dir_name(&self) -> String {
        format!("iPhoneOS{}.sdk", self.version)
    }

    fn pin(&self) -> String {
        match (&self.rev, &self.sha256, self.parts.first()) {
            (Some(r), _, _) => r.clone(),
            (_, Some(s), _) => s.clone(),
            (_, _, Some(p)) => p.sha256.clone(),
            _ => String::new(),
        }
    }
}

fn manifest() -> Result<Vec<Entry>, String> {
    toml::from_str::<Manifest>(MANIFEST).map(|m| m.sdk).map_err(|e| format!("sdk/manifest.toml: {}", e))
}

pub struct Spec {
    pub versions: Vec<String>,
    pub to: Option<PathBuf>,
    pub cache: Option<PathBuf>,
    pub force: bool,
}

pub fn parse_args(args: &[String]) -> Result<Spec, String> {
    let mut s = Spec { versions: Vec::new(), to: None, cache: None, force: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or_else(|| format!("sdk: {} needs a value", flag));
        match a.as_str() {
            "--to" => s.to = Some(PathBuf::from(value("--to")?)),
            "--cache" => s.cache = Some(PathBuf::from(value("--cache")?)),
            "--force" => s.force = true,
            o if o.starts_with('-') => return Err(format!("sdk: unknown option {}", o)),
            v => s.versions.push(v.to_string()),
        }
    }
    Ok(s)
}

// where the toolchain lookup finds sdks: $THEOS/sdks, then the tools directory (MEOW_TOOLS, or the default one) /theos/sdks
fn install_dir(to: &Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(t) = to {
        return Ok(t.clone());
    }
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let Some(t) = get("THEOS") {
        return Ok(Path::new(&t).join("sdks"));
    }
    if let Some(t) = crate::toolchain::tools_root() {
        return Ok(t.join("theos/sdks"));
    }
    if let Some(t) = crate::toolchain::default_tools_root() {
        return Ok(t.join("theos/sdks"));
    }
    Err("sdk: set THEOS or MEOW_TOOLS, or pass --to DIR".to_string())
}

fn state(dir: Option<&Path>, e: &Entry) -> &'static str {
    let Some(dir) = dir else { return "" };
    let p = dir.join(e.dir_name());
    match fs::read_to_string(p.join(STAMP)) {
        Ok(s) if s.trim() == format!("{} {}", e.source, e.pin()) => "installed",
        Ok(_) => "other pin",
        Err(_) if p.join("usr/include/stdio.h").is_file() => "present",
        Err(_) => "",
    }
}

pub fn list(to: &Option<PathBuf>) -> Result<(), String> {
    let entries = manifest()?;
    let dir = install_dir(to).ok();
    println!("{:<8} {:<30} {:<10} {}", "VERSION", "SOURCE", "STATE", "NOTE");
    for e in &entries {
        let note = if e.patched { "patched, private symbols restored" } else { "" };
        println!("{:<8} {:<30} {:<10} {}", e.version, e.source, state(dir.as_deref(), e), note);
    }
    match &dir {
        Some(d) => println!("\ninstall with `meow sdk VERSION` into {}", d.display()),
        None => println!("\ninstall with `meow sdk VERSION --to DIR`, or set THEOS or MEOW_TOOLS"),
    }
    Ok(())
}

#[cfg(unix)]
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    // cp -a keeps symlinks, modes and the frameworks' version links
    fetch::run(Command::new("cp").arg("-a").arg(from).arg(to), "cp")
}

#[cfg(not(unix))]
fn copy_tree(_: &Path, _: &Path) -> Result<(), String> {
    Err("sdk: installing from a git source needs a unix host".to_string())
}

// one partial clone per repository and commit, shared by every sdk of that repository
fn checkout(cache: &Path, git: &str, rev: &str, path: &str) -> Result<PathBuf, String> {
    if rev.len() != 40 || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{}: rev must be a full 40 digit commit id", git));
    }
    let name: String = git.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let dir = cache.join("git").join(format!("{}-{}", name, &rev[..12]));
    if !dir.join(".git").is_dir() {
        fetch::wipe(&dir)?;
        fs::create_dir_all(dir.parent().unwrap()).map_err(|e| format!("{}: {}", dir.display(), e))?;
        fetch::run(
            Command::new("git").args(["clone", "--quiet", "--filter=blob:none", "--no-checkout"]).arg(git).arg(&dir),
            "git clone",
        )?;
    }
    fetch::run(Command::new("git").arg("-C").arg(&dir).args(["sparse-checkout", "set", path]), "git sparse-checkout")?;
    fetch::run(Command::new("git").arg("-C").arg(&dir).args(["checkout", "--quiet", "--detach", rev]), "git checkout")?;
    let head = Command::new("git").arg("-C").arg(&dir).args(["rev-parse", "HEAD"]).output().map_err(|e| format!("git: {}", e))?;
    let got = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if got != rev.to_ascii_lowercase() {
        return Err(format!("git checked out {}, the manifest pins {}", got, rev));
    }
    Ok(dir.join(path))
}

fn install(e: &Entry, dir: &Path, cache: &Path, force: bool) -> Result<(), String> {
    let target = dir.join(e.dir_name());
    let tmp = dir.join(format!(".{}.tmp", e.dir_name()));
    let meta = fs::symlink_metadata(&target);
    match &meta {
        // a dangling link is replaced, a real sdk only when meow put it there or --force says so
        Ok(m) if m.file_type().is_symlink() && !target.exists() => {}
        Ok(_) if !force && state(Some(dir), e) == "present" => {
            return Err(format!(
                "{} is already there and meow did not install it, pass --force to replace it",
                target.display()
            ));
        }
        _ => {}
    }
    fs::create_dir_all(dir).map_err(|x| format!("{}: {}", dir.display(), x))?;
    fetch::wipe(&tmp)?;

    if let (Some(git), Some(rev), Some(path)) = (&e.git, &e.rev, &e.path) {
        let src = checkout(cache, git, rev, path)?;
        copy_tree(&src, &tmp)?;
    } else if let (Some(url), Some(sha)) = (&e.url, &e.sha256) {
        let archive = fetch::download(url, &sha.to_ascii_lowercase(), cache)?;
        fetch::extract(&archive, &tmp, e.strip)?;
    } else if !e.parts.is_empty() {
        // the archive is one file cut into pieces, every piece is checked on its own
        let joined = cache.join(format!("sdk-{}.zip", e.version));
        let mut out = fs::File::create(&joined).map_err(|x| format!("{}: {}", joined.display(), x))?;
        for p in &e.parts {
            let piece = fetch::download(&p.url, &p.sha256.to_ascii_lowercase(), cache)?;
            let mut f = fs::File::open(&piece).map_err(|x| format!("{}: {}", piece.display(), x))?;
            std::io::copy(&mut f, &mut out).map_err(|x| format!("{}: {}", joined.display(), x))?;
        }
        drop(out);
        let r = fetch::extract(&joined, &tmp, e.strip);
        let _ = fs::remove_file(&joined);
        r?;
    } else {
        return Err(format!("sdk {}: the manifest gives no source", e.version));
    }

    if !tmp.join("usr/include/stdio.h").is_file() {
        fetch::wipe(&tmp)?;
        return Err(format!("sdk {}: the result has no usr/include/stdio.h", e.version));
    }
    fs::write(tmp.join(STAMP), format!("{} {}\n", e.source, e.pin())).map_err(|x| format!("{}: {}", STAMP, x))?;
    if meta.is_ok() {
        if meta.as_ref().map_or(false, |m| m.file_type().is_symlink()) {
            fs::remove_file(&target).map_err(|x| format!("{}: {}", target.display(), x))?;
        } else {
            fetch::wipe(&target)?;
        }
    }
    fs::rename(&tmp, &target).map_err(|x| format!("{}: {}", target.display(), x))
}

pub fn run(s: &Spec) -> Result<(), String> {
    if s.versions.is_empty() {
        return list(&s.to);
    }
    let entries = manifest()?;
    let mut chosen: Vec<&Entry> = Vec::new();
    for v in &s.versions {
        if v == "all" {
            chosen.extend(entries.iter());
            continue;
        }
        let v = v.trim_start_matches("iPhoneOS").trim_end_matches(".sdk");
        match entries.iter().find(|e| e.version == v) {
            Some(e) => chosen.push(e),
            None => return Err(format!("sdk: no sdk {} in the manifest, `meow sdk` lists them", v)),
        }
    }
    let dir = install_dir(&s.to)?;
    let cache = match &s.cache {
        Some(c) => c.clone(),
        None => crate::cache::default_dir().ok_or("sdk: no cache directory, set MEOW_CACHE or pass --cache")?,
    };
    for e in chosen {
        eprintln!("sdk {}: {}", e.version, e.source);
        install(e, &dir, &cache, s.force)?;
        eprintln!("sdk {}: installed in {}", e.version, dir.join(e.dir_name()).display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_complete_and_pinned() {
        let m = manifest().unwrap();
        assert!(m.len() >= 30);
        let mut seen = std::collections::HashSet::new();
        for e in &m {
            assert!(seen.insert(e.version.clone()), "{} twice", e.version);
            let archive = e.url.is_some() && e.sha256.as_ref().map_or(false, |s| s.len() == 64);
            let parts = !e.parts.is_empty() && e.parts.iter().all(|p| p.sha256.len() == 64 && p.url.starts_with("https://"));
            let git = e.git.is_some() && e.path.is_some() && e.rev.as_ref().map_or(false, |r| r.len() == 40);
            assert_eq!(archive as u8 + parts as u8 + git as u8, 1, "{} needs exactly one pinned source", e.version);
        }
        for v in ["2.0", "6.1", "9.3", "16.5"] {
            assert!(m.iter().any(|e| e.version == v), "{} missing", v);
        }
    }

    #[test]
    fn arguments() {
        let s = parse_args(&["9.3".into(), "--to".into(), "/x".into(), "--force".into()]).unwrap();
        assert_eq!(s.versions, vec!["9.3"]);
        assert!(s.force && s.to == Some(PathBuf::from("/x")));
        assert!(parse_args(&["--bogus".into()]).is_err());
    }
}
