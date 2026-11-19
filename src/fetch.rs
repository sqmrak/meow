use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// pinned downloads and git checkouts. an archive is only used when its sha256 matches
// the one in meow.toml, a git checkout only at a full commit id, so a source that moved
// or was swapped is an error and not a different build

pub const STAMP: &str = ".meow-dep";

#[derive(Debug, Default)]
pub struct Spec {
    pub name: String,
    pub url: Option<String>,
    pub sha256: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub strip: u32,
    pub cache: String,
    pub to: String,
    pub patches: Vec<String>,
}

pub fn parse_args(args: &[String]) -> Result<Spec, String> {
    let mut s = Spec { strip: 1, ..Default::default() };
    let mut i = 0;
    let take = |i: &mut usize, flag: &str| -> Result<String, String> {
        *i += 1;
        args.get(*i).cloned().ok_or_else(|| format!("fetch: {} needs a value", flag))
    };
    while i < args.len() {
        let flag = args[i].clone();
        match flag.as_str() {
            "--name" => s.name = take(&mut i, &flag)?,
            "--url" => s.url = Some(take(&mut i, &flag)?),
            "--sha256" => s.sha256 = Some(take(&mut i, &flag)?),
            "--git" => s.git = Some(take(&mut i, &flag)?),
            "--rev" => s.rev = Some(take(&mut i, &flag)?),
            "--strip" => s.strip = take(&mut i, &flag)?.parse().map_err(|_| "fetch: --strip needs a number".to_string())?,
            "--cache" => s.cache = take(&mut i, &flag)?,
            "--to" => s.to = take(&mut i, &flag)?,
            "--patch" => s.patches.push(take(&mut i, &flag)?),
            other => return Err(format!("fetch: unknown argument {}", other)),
        }
        i += 1;
    }
    if s.name.is_empty() || s.to.is_empty() || s.cache.is_empty() {
        return Err("fetch: --name, --cache and --to are required".to_string());
    }
    Ok(s)
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("cannot run {}: {}", what, e))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!("{} failed: {}", what, String::from_utf8_lossy(&out.stderr).trim()))
}

fn wipe(p: &Path) -> Result<(), String> {
    match fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {}", p.display(), e)),
    }
}

fn download(url: &str, sha256: &str, cache: &Path) -> Result<PathBuf, String> {
    if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("file://")) {
        return Err(format!("{}: only https, http and file urls are fetched", url));
    }
    let dl = cache.join("dl");
    fs::create_dir_all(&dl).map_err(|e| format!("{}: {}", dl.display(), e))?;
    let target = dl.join(sha256);
    if let Ok(bytes) = fs::read(&target) {
        if crate::sha::sha256_hex(&bytes) == sha256 {
            return Ok(target);
        }
        // a cached file that does not match its own name is damaged
        let _ = fs::remove_file(&target);
    }
    let tmp = dl.join(format!("{}.part", sha256));
    run(Command::new("curl").args(["-fsSL", "--retry", "2", "--proto", "=https,http,file", "-o"]).arg(&tmp).arg(url), "curl")?;
    let bytes = fs::read(&tmp).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    let got = crate::sha::sha256_hex(&bytes);
    if got != sha256 {
        let _ = fs::remove_file(&tmp);
        return Err(format!("{}: sha256 is {}, meow.toml pins {}", url, got, sha256));
    }
    fs::rename(&tmp, &target).map_err(|e| format!("{}: {}", target.display(), e))?;
    Ok(target)
}

fn extract(archive: &Path, into: &Path, strip: u32) -> Result<(), String> {
    fs::create_dir_all(into).map_err(|e| format!("{}: {}", into.display(), e))?;
    let is_zip = fs::read(archive).map_or(false, |b| b.starts_with(b"PK\x03\x04"));
    if is_zip {
        if strip != 0 {
            // unzip cannot strip, so the zip is unpacked and a single top directory is lifted by hand
            run(Command::new("unzip").arg("-q").arg(archive).arg("-d").arg(into), "unzip")?;
            if strip == 1 {
                let entries: Vec<_> = fs::read_dir(into).map_err(|e| format!("{}: {}", into.display(), e))?.flatten().collect();
                if entries.len() == 1 && entries[0].path().is_dir() {
                    let top = entries[0].path();
                    for e in fs::read_dir(&top).map_err(|e| format!("{}: {}", top.display(), e))?.flatten() {
                        fs::rename(e.path(), into.join(e.file_name())).map_err(|x| format!("{}: {}", e.path().display(), x))?;
                    }
                    fs::remove_dir(&top).map_err(|e| format!("{}: {}", top.display(), e))?;
                }
            }
            return Ok(());
        }
        return run(Command::new("unzip").arg("-q").arg(archive).arg("-d").arg(into), "unzip");
    }
    run(
        Command::new("tar").arg("-xf").arg(archive).arg("-C").arg(into).arg(format!("--strip-components={}", strip)),
        "tar",
    )
}

pub fn run_fetch(s: &Spec) -> Result<(), String> {
    let to = PathBuf::from(&s.to);
    let tmp = PathBuf::from(format!("{}.tmp", s.to));
    wipe(&tmp)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), e))?;
    }
    match (&s.url, &s.sha256, &s.git, &s.rev) {
        (Some(url), Some(sha), None, None) => {
            if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("fetch: sha256 must be 64 hex digits".to_string());
            }
            let archive = download(url, &sha.to_ascii_lowercase(), Path::new(&s.cache))?;
            extract(&archive, &tmp, s.strip)?;
        }
        (None, None, Some(git), Some(rev)) => {
            if rev.len() != 40 || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("fetch: rev must be a full 40 digit commit id".to_string());
            }
            run(Command::new("git").args(["clone", "--quiet", "--no-checkout"]).arg(git).arg(&tmp), "git clone")?;
            run(Command::new("git").arg("-C").arg(&tmp).args(["checkout", "--quiet", "--detach"]).arg(rev), "git checkout")?;
            let head = Command::new("git").arg("-C").arg(&tmp).args(["rev-parse", "HEAD"]).output().map_err(|e| format!("git: {}", e))?;
            let got = String::from_utf8_lossy(&head.stdout).trim().to_string();
            if got != rev.to_ascii_lowercase() {
                return Err(format!("git checked out {}, meow.toml pins {}", got, rev));
            }
        }
        _ => return Err("fetch: give --url and --sha256, or --git and --rev".to_string()),
    }
    for p in &s.patches {
        let patch = fs::canonicalize(p).map_err(|e| format!("{}: {}", p, e))?;
        run(Command::new("patch").args(["-p1", "--batch", "--quiet", "-d"]).arg(&tmp).arg("-i").arg(&patch), &format!("patch {}", p))?;
    }
    fs::write(tmp.join(STAMP), format!("{}\n", s.name)).map_err(|e| format!("{}: {}", STAMP, e))?;
    wipe(&to)?;
    fs::rename(&tmp, &to).map_err(|e| format!("{}: {}", to.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meow-fetch-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn have(tool: &str) -> bool {
        crate::toolchain::host_tool_path(tool).is_some()
    }

    // a local archive through file://, no network
    fn archive(d: &Path) -> (String, String) {
        fs::create_dir_all(d.join("pkg-1.0/sub")).unwrap();
        fs::write(d.join("pkg-1.0/a.txt"), "alpha\n").unwrap();
        fs::write(d.join("pkg-1.0/sub/b.txt"), "beta\n").unwrap();
        let tar = d.join("pkg.tar.gz");
        run(Command::new("tar").arg("-czf").arg(&tar).arg("-C").arg(d).arg("pkg-1.0"), "tar").unwrap();
        let sha = crate::sha::sha256_hex(&fs::read(&tar).unwrap());
        (format!("file://{}", tar.display()), sha)
    }

    #[test]
    fn a_pinned_archive_is_fetched_stripped_and_patched() {
        if !have("curl") || !have("tar") || !have("patch") {
            eprintln!("skipped: curl, tar or patch is missing");
            return;
        }
        let d = tmp("ok");
        let (url, sha) = archive(&d);
        fs::write(d.join("a.patch"), "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-alpha\n+patched\n").unwrap();
        let spec = Spec {
            name: "pkg".into(),
            url: Some(url),
            sha256: Some(sha.clone()),
            strip: 1,
            cache: d.join("cache").to_string_lossy().into_owned(),
            to: d.join("out/pkg-x").to_string_lossy().into_owned(),
            patches: vec![d.join("a.patch").to_string_lossy().into_owned()],
            ..Default::default()
        };
        run_fetch(&spec).unwrap();
        assert_eq!(fs::read_to_string(d.join("out/pkg-x/a.txt")).unwrap(), "patched\n");
        assert_eq!(fs::read_to_string(d.join("out/pkg-x/sub/b.txt")).unwrap(), "beta\n");
        assert!(d.join("out/pkg-x").join(STAMP).is_file());
        assert!(d.join("cache/dl").join(&sha).is_file());
        // a second run reuses the cached download and replaces the tree
        run_fetch(&spec).unwrap();
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_wrong_hash_is_refused_and_leaves_nothing() {
        if !have("curl") || !have("tar") {
            eprintln!("skipped: curl or tar is missing");
            return;
        }
        let d = tmp("bad");
        let (url, _) = archive(&d);
        let spec = Spec {
            name: "pkg".into(),
            url: Some(url),
            sha256: Some("0".repeat(64)),
            strip: 1,
            cache: d.join("cache").to_string_lossy().into_owned(),
            to: d.join("out/pkg").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let e = run_fetch(&spec).unwrap_err();
        assert!(e.contains("meow.toml pins"), "{}", e);
        assert!(!d.join("out/pkg").exists());
        assert!(!d.join("cache/dl").join("0".repeat(64)).exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn arguments_and_urls_are_checked() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(parse_args(&a(&["--name", "x"])).is_err());
        assert!(parse_args(&a(&["--name", "x", "--cache", "c", "--to", "t", "--nope"])).is_err());
        let d = tmp("args");
        let spec = Spec { name: "x".into(), url: Some("ftp://h/x".into()), sha256: Some("a".repeat(64)), cache: d.to_string_lossy().into_owned(), to: d.join("o").to_string_lossy().into_owned(), ..Default::default() };
        assert!(run_fetch(&spec).unwrap_err().contains("only https"));
        let git = Spec { name: "x".into(), git: Some("g".into()), rev: Some("main".into()), cache: "c".into(), to: d.join("o2").to_string_lossy().into_owned(), ..Default::default() };
        assert!(run_fetch(&git).unwrap_err().contains("40 digit"));
        fs::remove_dir_all(&d).unwrap();
    }
}
