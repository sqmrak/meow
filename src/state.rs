use crate::config::{err, Error, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Input {
    pub path: String,
    pub mtime_ns: u128,
    pub size: u64,
    pub hash: String,
}

#[derive(Debug, Default)]
pub struct Record {
    pub cmd: String,
    pub inputs: Vec<Input>,
}

pub fn cmd_hash(argvs: &[&Vec<String>]) -> String {
    let mut h = blake3::Hasher::new();
    for argv in argvs {
        for a in argv.iter() {
            h.update(a.as_bytes());
            h.update(&[0]);
        }
        h.update(&[1]);
    }
    h.finalize().to_hex().to_string()
}

pub fn stat_of(path: &Path) -> Option<(u128, u64)> {
    let md = fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let ns = md
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((ns, md.len()))
}

pub fn hash_file(path: &Path) -> Result<String> {
    let f = fs::File::open(path).map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    let mut h = blake3::Hasher::new();
    h.update_reader(f)
        .map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    Ok(h.finalize().to_hex().to_string())
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    fn rec_path(&self, output: &str) -> PathBuf {
        let key = blake3::hash(output.as_bytes()).to_hex().to_string();
        self.dir.join(&key[..24])
    }

    pub fn load(&self, output: &str) -> Option<Record> {
        let text = fs::read_to_string(self.rec_path(output)).ok()?;
        let mut rec = Record::default();
        let mut lines = text.lines();
        if lines.next()? != "meow-rec 1" {
            return None;
        }
        for line in lines {
            if let Some(c) = line.strip_prefix("cmd\t") {
                rec.cmd = c.to_string();
            } else if let Some(r) = line.strip_prefix("in\t") {
                let mut p = r.splitn(4, '\t');
                rec.inputs.push(Input {
                    mtime_ns: p.next()?.parse().ok()?,
                    size: p.next()?.parse().ok()?,
                    hash: p.next()?.to_string(),
                    path: p.next()?.to_string(),
                });
            } else {
                return None;
            }
        }
        Some(rec)
    }

    pub fn save(&self, output: &str, rec: &Record) -> Result<()> {
        fs::create_dir_all(&self.dir).map_err(|e| Error(format!("{}: {}", self.dir.display(), e)))?;
        let mut text = String::from("meow-rec 1\n");
        text.push_str(&format!("cmd\t{}\n", rec.cmd));
        for i in &rec.inputs {
            if i.path.contains('\n') || i.path.contains('\t') {
                return err(format!("input path with a tab or newline: {:?}", i.path));
            }
            text.push_str(&format!("in\t{}\t{}\t{}\t{}\n", i.mtime_ns, i.size, i.hash, i.path));
        }
        let dst = self.rec_path(output);
        let tmp = dst.with_extension(format!("tmp{}", std::process::id()));
        let mut f = fs::File::create(&tmp).map_err(|e| Error(format!("{}: {}", tmp.display(), e)))?;
        f.write_all(text.as_bytes())
            .map_err(|e| Error(format!("{}: {}", tmp.display(), e)))?;
        fs::rename(&tmp, &dst).map_err(|e| Error(format!("{}: {}", dst.display(), e)))?;
        Ok(())
    }

    pub fn forget(&self, output: &str) {
        let _ = fs::remove_file(self.rec_path(output));
    }
}

// fresh when the command matches and no input changed in content, a touch alone
// does not rebuild
pub fn is_fresh(rec: &Record, cmd: &str, outputs: &[String], root: &Path) -> bool {
    if rec.cmd != cmd {
        return false;
    }
    if !outputs.iter().all(|o| root.join(o).is_file()) {
        return false;
    }
    for i in &rec.inputs {
        let p = root.join(&i.path);
        let Some((mtime, size)) = stat_of(&p) else { return false };
        if mtime == i.mtime_ns && size == i.size {
            continue;
        }
        if size != i.size {
            return false;
        }
        match hash_file(&p) {
            Ok(h) if h == i.hash => {}
            _ => return false,
        }
    }
    true
}

// the same test as is_fresh, with the reason when it fails
pub fn why_stale(rec: &Record, cmd: &str, outputs: &[String], root: &Path) -> Option<String> {
    if rec.cmd != cmd {
        return Some("the command line, the environment or the set of globbed inputs changed".to_string());
    }
    if let Some(o) = outputs.iter().find(|o| !root.join(o).is_file()) {
        return Some(format!("output {} is missing", o));
    }
    for i in &rec.inputs {
        let p = root.join(&i.path);
        let Some((mtime, size)) = stat_of(&p) else {
            return Some(format!("input {} vanished", i.path));
        };
        if mtime == i.mtime_ns && size == i.size {
            continue;
        }
        if size != i.size {
            return Some(format!("input {} changed size", i.path));
        }
        match hash_file(&p) {
            Ok(h) if h == i.hash => {}
            _ => return Some(format!("input {} changed", i.path)),
        }
    }
    None
}

// an input newer than the job start may have changed while the tool read it, so no
// record is written and the next run rebuilds
pub fn snapshot(paths: &[String], root: &Path, started: SystemTime) -> Result<Option<Vec<Input>>> {
    let started_ns = started
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let full = root.join(p);
        let Some((mtime, size)) = stat_of(&full) else {
            return err(format!("input vanished during the build: {}", p));
        };
        if mtime >= started_ns {
            return Ok(None);
        }
        out.push(Input {
            path: p.clone(),
            mtime_ns: mtime,
            size,
            hash: hash_file(&full)?,
        });
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meow-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn record_round_trip_and_touch_stays_fresh() {
        let root = tmpdir("state");
        fs::write(root.join("in.c"), b"int x;").unwrap();
        fs::write(root.join("out.o"), b"o").unwrap();
        let long_ago = SystemTime::now() + std::time::Duration::from_secs(5);
        let inputs = snapshot(&["in.c".to_string()], &root, long_ago).unwrap().unwrap();
        let store = Store::new(root.join("db"));
        let rec = Record { cmd: "c1".into(), inputs };
        store.save("out.o", &rec).unwrap();
        let back = store.load("out.o").unwrap();
        assert!(is_fresh(&back, "c1", &["out.o".to_string()], &root));
        assert!(!is_fresh(&back, "c2", &["out.o".to_string()], &root));
        // same size, new mtime, same bytes: still fresh
        let f = fs::OpenOptions::new().write(true).open(root.join("in.c")).unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(60)).unwrap();
        assert!(is_fresh(&back, "c1", &["out.o".to_string()], &root));
        // same size, different bytes: stale
        fs::write(root.join("in.c"), b"int y;").unwrap();
        assert!(!is_fresh(&back, "c1", &["out.o".to_string()], &root));
        // missing output: stale
        fs::remove_file(root.join("out.o")).unwrap();
        assert!(!is_fresh(&back, "c1", &["out.o".to_string()], &root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn input_newer_than_job_start_is_not_recorded() {
        let root = tmpdir("race");
        fs::write(root.join("a.h"), b"x").unwrap();
        let before = SystemTime::now() - std::time::Duration::from_secs(60);
        assert!(snapshot(&["a.h".to_string()], &root, before).unwrap().is_none());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn corrupt_record_is_ignored() {
        let root = tmpdir("corrupt");
        let store = Store::new(root.join("db"));
        store.save("o", &Record { cmd: "c".into(), inputs: vec![] }).unwrap();
        let p = store.rec_path("o");
        fs::write(&p, "garbage\n").unwrap();
        assert!(store.load("o").is_none());
        fs::remove_dir_all(&root).unwrap();
    }
}
