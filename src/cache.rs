use crate::plan::Node;
use crate::state::{self, Input};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

// a content addressed object cache shared by every build tree of the user. an entry is
// keyed by the compile command, the compiler binary and the environment, and holds the
// object, its depfile and the list of inputs with their hashes. a lookup is a hit only
// when every recorded input, header included, has the same content now, so a clean tree
// or a second checkout skips the compiles it has already done

const VERSION: &str = "meow-obj 1";
const MAX_CANDIDATES: usize = 16;
const DEFAULT_MAX_MIB: u64 = 4096;

pub struct Cache {
    dir: PathBuf,
    // header hashes by path, mtime and size, so a header read by 500 objects is hashed once
    memo: Mutex<HashMap<PathBuf, (u128, u64, String)>>,
}

pub fn default_dir() -> Option<PathBuf> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    get("MEOW_CACHE")
        .map(PathBuf::from)
        .or_else(|| get("XDG_CACHE_HOME").map(|c| Path::new(&c).join("meow")))
        .or_else(|| get("LOCALAPPDATA").map(|c| Path::new(&c).join("meow")))
        .or_else(|| get("HOME").map(|h| Path::new(&h).join(".cache/meow")))
}

pub fn max_bytes() -> u64 {
    std::env::var("MEOW_CACHE_MAX").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(DEFAULT_MAX_MIB) * 1024 * 1024
}

fn hash_bytes(b: &[u8]) -> String {
    blake3::hash(b).to_hex().to_string()
}

impl Cache {
    // none when MEOW_NO_CACHE is set
    pub fn open() -> Option<Cache> {
        if std::env::var_os("MEOW_NO_CACHE").is_some() {
            return None;
        }
        default_dir().map(|d| Cache::at(d.join("obj")))
    }

    pub fn at(dir: PathBuf) -> Cache {
        Cache { dir, memo: Mutex::new(HashMap::new()) }
    }

    // compiles with one output and a depfile, without debug info: the object of a -g
    // build names the directory it was compiled in, which differs between trees
    pub fn eligible(n: &Node) -> bool {
        n.depfile.is_some() && n.outputs.len() == 1 && n.post.is_empty() && n.argv.first().is_some() && !n.argv.iter().any(|a| a.starts_with("-g"))
    }

    fn key(n: &Node) -> String {
        let mut h = blake3::Hasher::new();
        h.update(VERSION.as_bytes());
        for a in &n.argv {
            h.update(a.as_bytes());
            h.update(&[0]);
        }
        for (k, v) in &n.env {
            h.update(k.as_bytes());
            h.update(b"=");
            h.update(v.as_bytes());
            h.update(&[0]);
        }
        // a new compiler build at the same path must not reuse old objects
        let cc = &n.argv[0];
        let resolved = if cc.contains('/') { Some(PathBuf::from(cc)) } else { crate::toolchain::host_tool_path(cc) };
        if let Some(meta) = resolved.and_then(|p| fs::metadata(p).ok()) {
            let ns = meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            h.update(&meta.len().to_le_bytes());
            h.update(&ns.to_le_bytes());
        }
        h.finalize().to_hex().to_string()
    }

    fn file_hash(&self, path: &Path) -> Option<String> {
        let (mtime, size) = state::stat_of(path)?;
        if let Some((m, s, h)) = self.memo.lock().unwrap().get(path) {
            if *m == mtime && *s == size {
                return Some(h.clone());
            }
        }
        let h = state::hash_file(path).ok()?;
        self.memo.lock().unwrap().insert(path.to_path_buf(), (mtime, size, h.clone()));
        Some(h)
    }

    fn entry_dir(&self, key: &str) -> PathBuf {
        self.dir.join(&key[..2]).join(key)
    }

    // on a hit the object and the depfile are in place and the inputs of the entry come back
    pub fn restore(&self, n: &Node, root: &Path) -> Option<Vec<String>> {
        let key = Self::key(n);
        let base = self.entry_dir(&key);
        let mut candidates: Vec<(SystemTime, PathBuf)> = fs::read_dir(&base)
            .ok()?
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, dir) in candidates.into_iter().take(MAX_CANDIDATES) {
            if let Some(inputs) = self.try_entry(&dir, n, root) {
                return Some(inputs);
            }
        }
        None
    }

    fn try_entry(&self, dir: &Path, n: &Node, root: &Path) -> Option<Vec<String>> {
        let manifest = fs::read_to_string(dir.join("manifest")).ok()?;
        let mut lines = manifest.lines();
        if lines.next()? != VERSION {
            return None;
        }
        let (mut out_hash, mut dep_hash) = (None, None);
        let mut inputs: Vec<(String, String)> = Vec::new();
        for l in lines {
            let mut f = l.splitn(3, '\t');
            match (f.next()?, f.next()?, f.next()) {
                ("out", h, None) => out_hash = Some(h.to_string()),
                ("dep", h, None) => dep_hash = Some(h.to_string()),
                ("in", h, Some(p)) => inputs.push((p.to_string(), h.to_string())),
                _ => return None,
            }
        }
        for (p, want) in &inputs {
            if self.file_hash(&root.join(p)).as_deref() != Some(want.as_str()) {
                return None;
            }
        }
        let out = fs::read(dir.join("out")).ok()?;
        let dep = fs::read(dir.join("dep")).ok()?;
        if Some(hash_bytes(&out)) != out_hash || Some(hash_bytes(&dep)) != dep_hash {
            // a damaged entry is removed so it is not tried again
            let _ = fs::remove_dir_all(dir);
            return None;
        }
        let out_path = root.join(&n.outputs[0]);
        let dep_path = root.join(n.depfile.as_ref()?);
        for (p, bytes) in [(&out_path, &out), (&dep_path, &dep)] {
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).ok()?;
            }
            fs::write(p, bytes).ok()?;
        }
        // recent entries are tried first and survive the cleanup
        if let Ok(f) = fs::OpenOptions::new().write(true).open(dir.join("manifest")) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(inputs.into_iter().map(|(p, _)| p).collect())
    }

    pub fn store(&self, n: &Node, root: &Path, inputs: &[Input]) -> Result<(), String> {
        let out = fs::read(root.join(&n.outputs[0])).map_err(|e| format!("{}: {}", n.outputs[0], e))?;
        let dep_name = n.depfile.as_ref().ok_or("no depfile")?;
        let dep = fs::read(root.join(dep_name)).map_err(|e| format!("{}: {}", dep_name, e))?;
        let mut manifest = format!("{}\nout\t{}\ndep\t{}\n", VERSION, hash_bytes(&out), hash_bytes(&dep));
        for i in inputs {
            if i.path.contains(['\t', '\n']) {
                return Ok(());
            }
            manifest.push_str(&format!("in\t{}\t{}\n", i.hash, i.path));
        }
        let key = Self::key(n);
        let base = self.entry_dir(&key);
        let id = &hash_bytes(manifest.as_bytes())[..16];
        let dest = base.join(id);
        if dest.exists() {
            return Ok(());
        }
        fs::create_dir_all(&base).map_err(|e| format!("{}: {}", base.display(), e))?;
        let tmp = base.join(format!(".tmp-{}-{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir(&tmp).map_err(|e| format!("{}: {}", tmp.display(), e))?;
        let write = |name: &str, bytes: &[u8]| fs::write(tmp.join(name), bytes).map_err(|e| format!("{}/{}: {}", tmp.display(), name, e));
        write("out", &out)?;
        write("dep", &dep)?;
        write("manifest", manifest.as_bytes())?;
        match fs::rename(&tmp, &dest) {
            Ok(()) => Ok(()),
            // another process stored the same entry first
            Err(_) if dest.exists() => {
                let _ = fs::remove_dir_all(&tmp);
                Ok(())
            }
            Err(e) => Err(format!("{}: {}", dest.display(), e)),
        }
    }

    // (entries, bytes)
    pub fn stats(&self) -> (usize, u64) {
        let mut n = 0;
        let mut bytes = 0;
        for (_, size, _) in self.entries() {
            n += 1;
            bytes += size;
        }
        (n, bytes)
    }

    pub(crate) fn entries(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut out = Vec::new();
        let Ok(top) = fs::read_dir(&self.dir) else { return out };
        for a in top.flatten() {
            let Ok(keys) = fs::read_dir(a.path()) else { continue };
            for k in keys.flatten() {
                let Ok(ids) = fs::read_dir(k.path()) else { continue };
                for e in ids.flatten() {
                    if e.file_name().to_string_lossy().starts_with('.') {
                        continue;
                    }
                    let size: u64 = fs::read_dir(e.path()).map(|d| d.flatten().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0);
                    let mtime = fs::metadata(e.path().join("manifest")).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    out.push((e.path(), size, mtime));
                }
            }
        }
        out
    }

    // drops the least recently used entries until the cache fits, returns (removed, bytes kept)
    pub fn gc(&self, max: u64) -> (usize, u64) {
        let mut all = self.entries();
        let mut total: u64 = all.iter().map(|e| e.1).sum();
        all.sort_by_key(|e| e.2);
        let mut removed = 0;
        for (path, size, _) in all {
            if total <= max {
                break;
            }
            if fs::remove_dir_all(&path).is_ok() {
                total -= size;
                removed += 1;
                if let Some(key_dir) = path.parent() {
                    let _ = fs::remove_dir(key_dir);
                }
            }
        }
        (removed, total)
    }

    pub fn clear(&self) -> std::io::Result<()> {
        match fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}
