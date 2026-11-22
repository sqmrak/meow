use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// builds a deb or an ipa from a package tree kept in memory: gnu tar members, gzip
// data, ar members in dpkg order, root ownership, fixed mtimes so the same payload
// gives the same bytes. modes live in the tree and never touch a filesystem, so the
// bytes are the same on every host, windows included

const DEFAULT_MTIME: u64 = 1_577_836_800; // 2020-01-01 utc
const MAX_FILES: usize = 100_000;
const SCRIPTS: [&str; 4] = ["preinst", "postinst", "prerm", "postrm"];


#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    #[default]
    Deb,
    Ipa,
}

// checks on the package tree before it is packed
#[derive(Debug, Default, Clone)]
pub struct Rules {
    pub require: Vec<String>,
    pub require_exec: Vec<String>,
    pub forbid: Vec<String>,
    pub forbid_text: Vec<String>,
    pub forbid_setuid: bool,
    pub sh_check: Vec<String>,
    pub contains: Vec<(String, String)>,
}

#[derive(Debug, Default, Clone)]
pub struct FileEnt {
    pub from: String,
    pub to: String,
    pub mode: u32,
    pub replace: bool,
}

#[derive(Debug, Default, Clone)]
pub struct TreeEnt {
    pub from: String,
    pub to: String,
    pub exclude: Vec<String>,
    pub replace: bool,
    pub mode: Option<u32>,
}

#[derive(Debug, Default)]
pub struct Spec {
    pub format: Format,
    pub out: String,
    pub control: Option<String>,
    pub fields: Vec<(String, String)>,
    pub files: Vec<FileEnt>,
    pub trees: Vec<TreeEnt>,
    // where the Packages stanza goes
    pub packages: Option<String>,
    pub rules: Rules,
    pub data: Vec<(String, u32, String)>,
    pub scripts: Vec<(String, String)>,
}

fn mode_of(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s, 8).ok().filter(|m| *m <= 0o7777).ok_or_else(|| format!("bad mode {:?}, use octal like 755", s))
}

fn take(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i).cloned().ok_or_else(|| format!("pack: {} needs a value", flag))
}

pub fn parse_args(args: &[String]) -> Result<Spec, String> {
    let mut spec = Spec::default();
    let mut last_is_tree = false;
    let mut i = 0usize;
    while i < args.len() {
        let flag = args[i].clone();
        match flag.as_str() {
            "--format" => {
                spec.format = match take(args, &mut i, &flag)?.as_str() {
                    "deb" => Format::Deb,
                    "ipa" => Format::Ipa,
                    other => return Err(format!("pack: unknown format {}, use deb or ipa", other)),
                }
            }
            "--out" => spec.out = take(args, &mut i, &flag)?,
            "--control" => spec.control = Some(take(args, &mut i, &flag)?),
            "--field" => {
                let kv = take(args, &mut i, &flag)?;
                let (k, v) = kv.split_once('=').ok_or_else(|| format!("pack: --field {:?} needs KEY=VALUE", kv))?;
                spec.fields.push((k.to_string(), v.to_string()));
            }
            "--file" => {
                let from = take(args, &mut i, &flag)?;
                let to = take(args, &mut i, &flag)?;
                let mode = take(args, &mut i, &flag)?;
                spec.files.push(FileEnt { from, to, mode: mode_of(&mode)?, replace: false });
                last_is_tree = false;
            }
            "--tree" => {
                let from = take(args, &mut i, &flag)?;
                let to = take(args, &mut i, &flag)?;
                spec.trees.push(TreeEnt { from, to, ..Default::default() });
                last_is_tree = true;
            }
            // these two modify the entry given just before them
            "--tree-exclude" => {
                let g = take(args, &mut i, &flag)?;
                glob::Pattern::new(&g).map_err(|e| format!("pack: bad exclude pattern {}: {}", g, e))?;
                spec.trees.last_mut().ok_or("pack: --tree-exclude needs a --tree before it")?.exclude.push(g);
            }
            "--tree-mode" => {
                let m = take(args, &mut i, &flag)?;
                spec.trees.last_mut().ok_or("pack: --tree-mode needs a --tree before it")?.mode = Some(mode_of(&m)?);
            }
            "--replace" => {
                if last_is_tree {
                    spec.trees.last_mut().ok_or("pack: --replace needs a --file or --tree before it")?.replace = true;
                } else {
                    spec.files.last_mut().ok_or("pack: --replace needs a --file or --tree before it")?.replace = true;
                }
            }
            "--packages" => spec.packages = Some(take(args, &mut i, &flag)?),
            "--require" => spec.rules.require.push(take(args, &mut i, &flag)?),
            "--require-exec" => spec.rules.require_exec.push(take(args, &mut i, &flag)?),
            "--forbid" => {
                let g = take(args, &mut i, &flag)?;
                glob::Pattern::new(&g).map_err(|e| format!("pack: bad forbid pattern {}: {}", g, e))?;
                spec.rules.forbid.push(g);
            }
            "--forbid-text" => spec.rules.forbid_text.push(take(args, &mut i, &flag)?),
            "--forbid-setuid" => spec.rules.forbid_setuid = true,
            "--sh-check" => spec.rules.sh_check.push(take(args, &mut i, &flag)?),
            "--contains" => {
                let f = take(args, &mut i, &flag)?;
                let t = take(args, &mut i, &flag)?;
                spec.rules.contains.push((f, t));
            }
            "--data" => {
                let to = take(args, &mut i, &flag)?;
                let mode = take(args, &mut i, &flag)?;
                let content = take(args, &mut i, &flag)?;
                spec.data.push((to, mode_of(&mode)?, content));
            }
            "--script" => {
                let name = take(args, &mut i, &flag)?;
                let from = take(args, &mut i, &flag)?;
                spec.scripts.push((name, from));
            }
            other => return Err(format!("pack: unknown argument {}", other)),
        }
        i += 1;
    }
    if spec.out.is_empty() {
        return Err("pack: --out is required".to_string());
    }
    Ok(spec)
}

// a package path must stay inside the package: relative, no empty or dot parts
fn clean_dest(p: &str) -> Result<String, String> {
    let t = p.trim_start_matches('/');
    if t.is_empty() {
        return Err(format!("empty package path {:?}", p));
    }
    for part in t.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("bad package path {:?}", p));
        }
    }
    if t == "DEBIAN" || t.starts_with("DEBIAN/") {
        return Err(format!("{:?}: DEBIAN is reserved for control files", p));
    }
    Ok(t.to_string())
}

enum Kind {
    Dir,
    File(Vec<u8>),
    Link(String),
}

struct Node {
    kind: Kind,
    mode: u32,
}

// the package as it will be archived, keyed by path without a leading ./
#[derive(Default)]
struct Tree {
    nodes: BTreeMap<String, Node>,
}

impl Tree {
    // parents of a package path get 755, whatever the builder's umask is
    fn parents(&mut self, dest: &str) -> Result<(), String> {
        let parts: Vec<&str> = dest.split('/').collect();
        let mut cur = String::new();
        for part in &parts[..parts.len() - 1] {
            if !cur.is_empty() {
                cur.push('/');
            }
            cur.push_str(part);
            match self.nodes.get(&cur) {
                Some(Node { kind: Kind::Dir, .. }) => {}
                Some(_) => return Err(format!("{} is a file, {} cannot go under it", cur, dest)),
                None => {
                    self.nodes.insert(cur.clone(), Node { kind: Kind::Dir, mode: 0o755 });
                }
            }
        }
        Ok(())
    }

    fn put(&mut self, dest: &str, kind: Kind, mode: u32, replace: bool) -> Result<(), String> {
        match self.nodes.get(dest) {
            None => {}
            Some(Node { kind: Kind::Dir, .. }) => return Err(format!("{} is a directory in the package", dest)),
            Some(_) if !replace => return Err(format!("{} is packed twice", dest)),
            Some(_) => {}
        }
        self.parents(dest)?;
        self.nodes.insert(dest.to_string(), Node { kind, mode });
        Ok(())
    }

    fn dir(&mut self, dest: &str) -> Result<(), String> {
        self.parents(&format!("{}/x", dest))
    }

    fn file(&self, p: &str) -> Option<(&[u8], u32)> {
        match self.nodes.get(p) {
            Some(Node { kind: Kind::File(d), mode }) => Some((d, *mode)),
            _ => None,
        }
    }

    // depth first with the names of each directory sorted, the order gnu tar --sort=name walks
    fn ordered(&self) -> Vec<(&String, &Node)> {
        let mut v: Vec<_> = self.nodes.iter().collect();
        v.sort_by(|a, b| a.0.split('/').cmp(b.0.split('/')));
        v
    }

    fn files(&self) -> impl Iterator<Item = (&String, &[u8], u32)> {
        self.nodes.iter().filter_map(|(p, n)| match &n.kind {
            Kind::File(d) => Some((p, d.as_slice(), n.mode)),
            _ => None,
        })
    }
}

// the mode of a source file in a tree that sets none. windows keeps no mode bits, so a
// mach-o file or a #! script there is taken as executable, a package whose binary lost
// its execute bit installs and does not run
#[cfg(unix)]
fn source_mode(meta: &fs::Metadata, _data: &[u8]) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if meta.permissions().mode() & 0o111 != 0 {
        0o755
    } else {
        0o644
    }
}

#[cfg(not(unix))]
fn source_mode(_meta: &fs::Metadata, data: &[u8]) -> u32 {
    const MACHO: [[u8; 4]; 6] = [[0xfe, 0xed, 0xfa, 0xce], [0xfe, 0xed, 0xfa, 0xcf], [0xce, 0xfa, 0xed, 0xfe], [0xcf, 0xfa, 0xed, 0xfe], [0xca, 0xfe, 0xba, 0xbe], [0xbe, 0xba, 0xfe, 0xca]];
    if data.starts_with(b"#!") || data.get(..4).is_some_and(|m| MACHO.iter().any(|x| x == m)) {
        0o755
    } else {
        0o644
    }
}

fn read_source(p: &Path) -> Result<Vec<u8>, String> {
    fs::read(p).map_err(|e| format!("{}: {}", p.display(), e))
}

struct TreeOpts {
    exclude: Vec<glob::Pattern>,
    replace: bool,
    mode: Option<u32>,
}

fn put_tree(tree: &mut Tree, root: &Path, from: &Path, to: &str, count: &mut usize, o: &TreeOpts) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(from).map_err(|e| format!("{}: {}", from.display(), e))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let rel = e.path().strip_prefix(root).unwrap().to_path_buf();
        if o.exclude.iter().any(|p| p.matches_path(&rel)) {
            continue;
        }
        *count += 1;
        if *count > MAX_FILES {
            return Err(format!("more than {} files in one package", MAX_FILES));
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let dest = format!("{}/{}", to, name);
        let meta = fs::symlink_metadata(e.path()).map_err(|e2| format!("{}: {}", e.path().display(), e2))?;
        if meta.file_type().is_symlink() {
            let link = fs::read_link(e.path()).map_err(|e2| format!("{}: {}", e.path().display(), e2))?;
            // a windows link may be spelled with backslashes, a package link never is
            let target = link.to_string_lossy().replace('\\', "/");
            tree.put(&dest, Kind::Link(target), 0o777, o.replace)?;
        } else if meta.is_dir() {
            tree.dir(&dest)?;
            put_tree(tree, root, &e.path(), &dest, count, o)?;
        } else if meta.is_file() {
            let data = read_source(&e.path())?;
            let mode = o.mode.unwrap_or_else(|| source_mode(&meta, &data));
            tree.put(&dest, Kind::File(data), mode, o.replace)?;
        } else {
            return Err(format!("{}: not a regular file, directory or symlink", e.path().display()));
        }
    }
    Ok(())
}

// DEBIAN/x lives in the control tree, everything else in the payload
fn rule_file<'a>(tree: &'a Tree, control: &'a Tree, p: &str) -> Option<(&'a [u8], u32)> {
    match p.strip_prefix("DEBIAN/") {
        Some(rest) => control.file(rest),
        None => tree.file(p),
    }
}

fn check_rules(r: &Rules, tree: &Tree, control: &Tree) -> Result<(), String> {
    for p in &r.require {
        let (data, _) = rule_file(tree, control, p).ok_or_else(|| format!("package check: {} is missing", p))?;
        if data.is_empty() {
            return Err(format!("package check: {} is empty or not a file", p));
        }
    }
    for p in &r.require_exec {
        let (_, mode) = rule_file(tree, control, p).ok_or_else(|| format!("package check: {} is missing", p))?;
        if mode & 0o111 == 0 {
            return Err(format!("package check: {} is not an executable file", p));
        }
    }
    for g in &r.forbid {
        let pat = glob::Pattern::new(g).map_err(|e| format!("{}: {}", g, e))?;
        if let Some(hit) = tree.nodes.keys().find(|p| pat.matches(p)) {
            return Err(format!("package check: {} must not be packaged (matches {})", hit, g));
        }
    }
    if r.forbid_setuid || !r.forbid_text.is_empty() {
        let payload = tree.files().map(|(p, d, m)| (p.clone(), d, m));
        let ctl = control.files().map(|(p, d, m)| (format!("DEBIAN/{}", p), d, m));
        for (name, data, mode) in payload.chain(ctl) {
            if r.forbid_setuid && mode & 0o6000 != 0 {
                return Err(format!("package check: {} has a setuid or setgid bit", name));
            }
            for t in &r.forbid_text {
                if data.windows(t.len()).any(|w| w == t.as_bytes()) {
                    return Err(format!("package check: {} contains {:?}", name, t));
                }
            }
        }
    }
    for p in &r.sh_check {
        let (data, _) = rule_file(tree, control, p).ok_or_else(|| format!("package check: {} is missing", p))?;
        sh_syntax(p, data)?;
    }
    for (file, text) in &r.contains {
        let (body, _) = rule_file(tree, control, file).ok_or_else(|| format!("package check: {} is missing", file))?;
        if !body.windows(text.len()).any(|w| w == text.as_bytes()) {
            return Err(format!("package check: {} does not contain {:?}", file, text));
        }
    }
    Ok(())
}

// the script goes to sh -n on stdin, it is not a file on disk any more
fn sh_syntax(name: &str, script: &[u8]) -> Result<(), String> {
    let mut child = Command::new("sh")
        .arg("-n")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run sh: {}", e))?;
    let mut stdin = child.stdin.take().ok_or("sh has no stdin")?;
    // sh -n may stop reading at the first error and close the pipe
    let sent = stdin.write_all(script);
    drop(stdin);
    let out = child.wait_with_output().map_err(|e| format!("sh: {}", e))?;
    if !out.status.success() {
        return Err(format!("package check: sh -n {} failed: {}", name, String::from_utf8_lossy(&out.stderr).trim()));
    }
    sent.map_err(|e| format!("sh: {}", e))
}

// the payload as gnu tar writes `tar -c .`: the root, then every path, names starting with ./
fn data_tar(tree: &Tree, mtime: u64) -> Result<Vec<u8>, String> {
    let mut t = crate::tar::Tar::new(mtime);
    t.dir("./", 0o755)?;
    for (path, node) in tree.ordered() {
        let name = format!("./{}", path);
        match &node.kind {
            Kind::Dir => t.dir(&name, node.mode)?,
            Kind::File(d) => t.file(&name, node.mode, d)?,
            Kind::Link(target) => t.symlink(&name, target)?,
        }
    }
    Ok(t.finish())
}

// control members are named bare, with no ./ and no directory member
fn control_tar(control: &Tree, mtime: u64) -> Result<Vec<u8>, String> {
    let mut t = crate::tar::Tar::new(mtime);
    for (name, data, mode) in control.files() {
        t.file(name, mode, data)?;
    }
    Ok(t.finish())
}


fn control_field(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    text.lines().find_map(|l| l.strip_prefix(&prefix)).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn render_control(fields: &[(String, String)], installed_kib: u64) -> Result<String, String> {
    let mut seen = std::collections::BTreeSet::new();
    for (k, v) in fields {
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("bad control field name {:?}", k));
        }
        if v.contains('\n') || v.trim().is_empty() {
            return Err(format!("control field {} must be one non-empty line", k));
        }
        if !seen.insert(k.to_ascii_lowercase()) {
            return Err(format!("control field {} is given twice", k));
        }
    }
    let order = ["Package", "Name", "Version", "Architecture", "Description", "Maintainer", "Author", "Section", "Priority", "Depends"];
    let mut out = String::new();
    for key in order {
        if let Some((k, v)) = fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            out.push_str(&format!("{}: {}\n", k, v));
        }
    }
    let mut rest: Vec<&(String, String)> = fields.iter().filter(|(k, _)| !order.iter().any(|o| o.eq_ignore_ascii_case(k))).collect();
    rest.sort();
    for (k, v) in rest {
        out.push_str(&format!("{}: {}\n", k, v));
    }
    out.push_str(&format!("Installed-Size: {}\n", installed_kib));
    for need in ["Package", "Version", "Architecture", "Description", "Maintainer"] {
        if control_field(&out, need).is_none() {
            return Err(format!("control needs a {} field", need));
        }
    }
    Ok(out)
}

fn ar_member(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    // gnu ar spelling of a member name, 16 columns
    let head = format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", format!("{}/", name), 0, 0, 0, "100644", data.len());
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(b'\n');
    }
}

// members of an ar file in order, for checking a finished deb
pub fn ar_members(bytes: &[u8]) -> Result<Vec<(String, usize)>, String> {
    if bytes.get(..8) != Some(b"!<arch>\n") {
        return Err("not an ar archive".to_string());
    }
    let mut at = 8;
    let mut out = Vec::new();
    while at < bytes.len() {
        let h = bytes.get(at..at + 60).ok_or("truncated ar header")?;
        if &h[58..60] != b"`\n" {
            return Err("bad ar header terminator".to_string());
        }
        let name = String::from_utf8_lossy(&h[..16]).trim_end().trim_end_matches('/').to_string();
        let size: usize = String::from_utf8_lossy(&h[48..58]).trim().parse().map_err(|_| "bad ar member size")?;
        at += 60;
        if at + size > bytes.len() {
            return Err(format!("ar member {} runs past the end", name));
        }
        out.push((name, size));
        at += size + size % 2;
    }
    Ok(out)
}


fn zip_entries(tree: &Tree) -> Vec<crate::zip::Entry> {
    tree.ordered()
        .into_iter()
        .map(|(path, node)| match &node.kind {
            Kind::Dir => crate::zip::Entry { name: format!("{}/", path), data: Vec::new(), mode: node.mode, link: false },
            Kind::File(d) => crate::zip::Entry { name: path.clone(), data: d.clone(), mode: node.mode, link: false },
            Kind::Link(t) => crate::zip::Entry { name: path.clone(), data: t.clone().into_bytes(), mode: 0o777, link: true },
        })
        .collect()
}

// what an installer needs from the bundle: one Payload/X.app with an Info.plist, and
// when the plist is text, an identifier and an executable that is a mach-o file
fn check_bundle(tree: &Tree) -> Result<(), String> {
    let apps: Vec<&str> = tree.nodes.keys().filter_map(|p| p.strip_prefix("Payload/")).filter(|r| !r.contains('/')).collect();
    let is_dir = |p: &str| matches!(tree.nodes.get(p), Some(Node { kind: Kind::Dir, .. }));
    if apps.len() != 1 || !apps[0].ends_with(".app") || !is_dir(&format!("Payload/{}", apps[0])) {
        return Err(format!("Payload must hold exactly one .app directory, it holds {:?}", apps));
    }
    let app = format!("Payload/{}", apps[0]);
    let (plist, _) = tree.file(&format!("{}/Info.plist", app)).ok_or_else(|| format!("{}/Info.plist: not in the bundle", apps[0]))?;
    if plist.starts_with(b"bplist") {
        return Ok(());
    }
    let text = String::from_utf8_lossy(plist);
    if crate::kinds::plist_string(&text, "CFBundleIdentifier").is_none() {
        return Err(format!("{}/Info.plist has no CFBundleIdentifier", apps[0]));
    }
    let exe = crate::kinds::plist_string(&text, "CFBundleExecutable").ok_or_else(|| format!("{}/Info.plist has no CFBundleExecutable", apps[0]))?;
    let (bytes, mode) = tree.file(&format!("{}/{}", app, exe)).ok_or_else(|| format!("CFBundleExecutable {} is not in the bundle", exe))?;
    if mode & 0o111 == 0 {
        return Err(format!("{} is not executable", exe));
    }
    crate::macho::inspect(bytes).map_err(|e| format!("{} is not a mach-o executable: {}", exe, e))?;
    Ok(())
}

fn write_out(spec: &Spec, base: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let out = base.join(&spec.out);
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    let tmp = base.join(format!("{}.tmp", spec.out));
    fs::write(&tmp, bytes).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    fs::rename(&tmp, &out).map_err(|e| format!("{}: {}", out.display(), e))?;
    Ok(out)
}

fn finish_ipa(spec: &Spec, base: &Path, tree: &Tree) -> Result<(), String> {
    check_bundle(tree)?;
    let entries = zip_entries(tree);
    let out = write_out(spec, base, &crate::zip::write(&entries)?)?;

    let written = fs::read(&out).map_err(|e| format!("{}: {}", out.display(), e))?;
    let listed = crate::zip::list(&written)?;
    if listed.len() != entries.len() {
        return Err(format!("{}: {} entries written, {} read back", spec.out, entries.len(), listed.len()));
    }
    for l in &listed {
        crate::zip::check_entry(&written, l)?;
        // installd refuses a file its owner cannot read
        if l.mode & 0o170000 == 0o100000 && l.mode & 0o400 == 0 {
            return Err(format!("{}: stored without owner read permission", l.name));
        }
    }
    Ok(())
}

fn host_dpkg_check(out: &Path) -> Result<(), String> {
    let Some(dpkg) = crate::toolchain::host_tool_path("dpkg-deb") else {
        return Ok(());
    };
    for flag in ["--info", "--contents"] {
        let r = Command::new(&dpkg).arg(flag).arg(out).output().map_err(|e| format!("cannot run dpkg-deb: {}", e))?;
        if !r.status.success() {
            return Err(format!("dpkg-deb {} rejects the package: {}", flag, String::from_utf8_lossy(&r.stderr).trim()));
        }
    }
    Ok(())
}

// paths in the spec are relative to base, the project root
fn build_tree(spec: &Spec, base: &Path) -> Result<Tree, String> {
    let mut tree = Tree::default();
    let mut count = 0usize;
    for f in &spec.files {
        let dest = clean_dest(&f.to)?;
        let src = base.join(&f.from);
        if !src.is_file() {
            return Err(format!("{}: not a file", f.from));
        }
        tree.put(&dest, Kind::File(read_source(&src)?), f.mode, f.replace)?;
        count += 1;
    }
    for t in &spec.trees {
        let dest = clean_dest(&t.to)?;
        let src = base.join(&t.from);
        if !src.is_dir() {
            return Err(format!("{}: not a directory", t.from));
        }
        let opts = TreeOpts { exclude: t.exclude.iter().map(|g| glob::Pattern::new(g).unwrap()).collect(), replace: t.replace, mode: t.mode };
        put_tree(&mut tree, &src, &src, &dest, &mut count, &opts)?;
    }
    for (to, mode, content) in &spec.data {
        let dest = clean_dest(to)?;
        tree.put(&dest, Kind::File(content.clone().into_bytes()), *mode, false)?;
    }
    Ok(tree)
}

pub fn run(spec: &Spec, base: &Path) -> Result<(), String> {
    let mtime = std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_MTIME);
    let tree = build_tree(spec, base)?;

    if spec.format == Format::Ipa {
        if spec.control.is_some() || !spec.fields.is_empty() || !spec.scripts.is_empty() {
            return Err("pack: an ipa has no control file, fields or maintainer scripts".to_string());
        }
        return finish_ipa(spec, base, &tree);
    }

    let installed: u64 = tree.files().map(|(_, d, _)| d.len() as u64).sum();
    let control_text = match (&spec.control, spec.fields.is_empty()) {
        (Some(path), true) => {
            let text = fs::read_to_string(base.join(path)).map_err(|e| format!("{}: {}", path, e))?;
            for need in ["Package", "Version", "Architecture", "Description", "Maintainer"] {
                if control_field(&text, need).is_none() {
                    return Err(format!("{}: no {} field", path, need));
                }
            }
            text
        }
        (None, false) => render_control(&spec.fields, installed.div_ceil(1024))?,
        _ => return Err("pack: give either --control or --field, not both and not neither".to_string()),
    };

    // md5sums lists regular files only, sorted as plain strings
    let mut sums = String::new();
    for (path, data, _) in tree.files() {
        sums.push_str(&format!("{}  {}\n", crate::md5::hex(data), path));
    }

    let mut control = Tree::default();
    control.put("control", Kind::File(control_text.clone().into_bytes()), 0o644, false)?;
    control.put("md5sums", Kind::File(sums.into_bytes()), 0o644, false)?;
    for (name, from) in &spec.scripts {
        if !SCRIPTS.contains(&name.as_str()) {
            return Err(format!("{} is not a maintainer script name", name));
        }
        control.put(name, Kind::File(read_source(&base.join(from))?), 0o755, false)?;
    }

    check_rules(&spec.rules, &tree, &control)?;

    let mut deb = b"!<arch>\n".to_vec();
    ar_member(&mut deb, "debian-binary", b"2.0\n");
    ar_member(&mut deb, "control.tar.gz", &crate::deflate::gzip(&control_tar(&control, mtime)?)?);
    ar_member(&mut deb, "data.tar.gz", &crate::deflate::gzip(&data_tar(&tree, mtime)?)?);
    let out = write_out(spec, base, &deb)?;

    let names: Vec<String> = ar_members(&fs::read(&out).map_err(|e| format!("{}: {}", out.display(), e))?)?.into_iter().map(|(n, _)| n).collect();
    if names != ["debian-binary", "control.tar.gz", "data.tar.gz"] {
        return Err(format!("{}: members are {:?}, dpkg wants debian-binary, control.tar.gz, data.tar.gz", spec.out, names));
    }
    host_dpkg_check(&out)?;
    if let Some(p) = &spec.packages {
        let mut stanza = control_text.clone();
        if !stanza.ends_with('\n') {
            stanza.push('\n');
        }
        let file = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let bytes = fs::read(&out).map_err(|e| format!("{}: {}", out.display(), e))?;
        stanza.push_str(&format!(
            "Filename: debs/{}\nSize: {}\nMD5sum: {}\nSHA1: {}\nSHA256: {}\n",
            file,
            bytes.len(),
            crate::md5::hex(&bytes),
            crate::sha::sha1_hex(&bytes),
            crate::sha::sha256_hex(&bytes)
        ));
        let dest = base.join(p);
        fs::write(&dest, stanza).map_err(|e| format!("{}: {}", dest.display(), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("meow-pack-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn fields() -> Vec<(String, String)> {
        [("Package", "com.x.t"), ("Version", "1.0"), ("Architecture", "iphoneos-arm"), ("Description", "t"), ("Maintainer", "m")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn destinations_cannot_escape() {
        assert_eq!(clean_dest("/usr/bin/x").unwrap(), "usr/bin/x");
        for bad in ["", "/", "../x", "a/../b", "a//b", "DEBIAN/control", "a/./b"] {
            assert!(clean_dest(bad).is_err(), "{:?}", bad);
        }
    }

    #[test]
    fn control_is_ordered_and_checked() {
        let mut f = fields();
        f.push(("Depends".into(), "firmware (>= 5.0)".into()));
        f.push(("X-Custom".into(), "1".into()));
        let text = render_control(&f, 7).unwrap();
        assert!(text.starts_with("Package: com.x.t\nVersion: 1.0\nArchitecture: iphoneos-arm\n"), "{}", text);
        assert!(text.ends_with("X-Custom: 1\nInstalled-Size: 7\n"), "{}", text);
        f.pop();
        f.pop();
        f.remove(0);
        assert!(render_control(&f, 0).unwrap_err().contains("Package"));
        let mut dup = fields();
        dup.push(("package".into(), "x".into()));
        assert!(render_control(&dup, 0).is_err());
        let mut multi = fields();
        multi[3].1 = "a\nb".into();
        assert!(render_control(&multi, 0).is_err());
    }

    #[test]
    fn argument_parsing_checks_modes() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let spec = parse_args(&a(&["--out", "o.deb", "--file", "a", "usr/bin/a", "755", "--field", "Package=x"])).unwrap();
        assert_eq!((spec.files[0].from.as_str(), spec.files[0].to.as_str(), spec.files[0].mode), ("a", "usr/bin/a", 0o755));
        assert!(parse_args(&a(&["--out", "o", "--file", "a", "b", "999"])).is_err());
        assert!(parse_args(&a(&["--nope"])).is_err());
        assert!(parse_args(&a(&[])).is_err());
        assert!(parse_args(&a(&["--out", "o", "--nope"])).is_err());
        assert!(parse_args(&a(&["--out", "o", "--replace"])).is_err());
        let t = parse_args(&a(&["--out", "o", "--tree", "r", "d", "--tree-exclude", "*.psd", "--replace"])).unwrap();
        assert_eq!((t.trees[0].exclude.clone(), t.trees[0].replace), (vec!["*.psd".to_string()], true));
    }

    #[test]
    fn package_rules_name_the_path_that_breaks_them() {
        let d = tmp("rules");
        fs::write(d.join("bin"), "binary @TOKEN@").unwrap();
        fs::write(d.join("empty"), "").unwrap();
        fs::write(d.join("good.sh"), "#!/bin/sh\necho install_layout\n").unwrap();
        fs::write(d.join("bad.sh"), "if then\n").unwrap();
        fs::write(d.join("suid"), "x").unwrap();
        let base = |rules: Rules, files: Vec<FileEnt>| Spec { out: "build/r.deb".into(), fields: fields(), files, rules, ..Default::default() };
        let f = |from: &str, to: &str, mode: u32| FileEnt { from: from.into(), to: to.into(), mode, replace: false };
        let ok = base(
            Rules {
                require: vec!["usr/bin/t".into()],
                require_exec: vec!["usr/bin/t".into()],
                forbid: vec!["etc".into(), "usr/bin/*awgd".into()],
                forbid_setuid: true,
                sh_check: vec!["usr/lib/good.sh".into()],
                contains: vec![("usr/lib/good.sh".into(), "install_layout".into())],
                ..Default::default()
            },
            vec![f("bin", "usr/bin/t", 0o755), f("good.sh", "usr/lib/good.sh", 0o644)],
        );
        run(&ok, &d).unwrap();
        let fails = |rules: Rules, files: Vec<FileEnt>, want: &str| {
            let e = run(&base(rules, files), &d).unwrap_err();
            assert!(e.contains(want), "{}: {}", want, e);
        };
        let one = || vec![f("bin", "usr/bin/t", 0o755)];
        fails(Rules { require: vec!["usr/bin/nope".into()], ..Default::default() }, one(), "usr/bin/nope is missing");
        fails(Rules { require: vec!["usr/bin/e".into()], ..Default::default() }, vec![f("empty", "usr/bin/e", 0o755)], "empty");
        fails(Rules { require_exec: vec!["usr/bin/t".into()], ..Default::default() }, vec![f("bin", "usr/bin/t", 0o644)], "not an executable");
        fails(Rules { forbid: vec!["usr/bin/*".into()], ..Default::default() }, one(), "must not be packaged");
        fails(Rules { forbid_text: vec!["@TOKEN@".into()], ..Default::default() }, one(), "contains");
        fails(Rules { forbid_setuid: true, ..Default::default() }, vec![f("suid", "usr/bin/s", 0o4755)], "setuid");
        fails(Rules { sh_check: vec!["usr/lib/bad.sh".into()], ..Default::default() }, vec![f("bad.sh", "usr/lib/bad.sh", 0o755)], "sh -n");
        fails(Rules { contains: vec![("usr/bin/t".into(), "absent text".into())], ..Default::default() }, one(), "does not contain");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn exclude_replace_and_the_packages_stanza() {
        let d = tmp("opts");
        fs::create_dir_all(d.join("base/keep")).unwrap();
        fs::create_dir_all(d.join("over")).unwrap();
        fs::write(d.join("base/a.png"), "base-a").unwrap();
        fs::write(d.join("base/art.psd"), "huge").unwrap();
        fs::write(d.join("base/keep/b.txt"), "b").unwrap();
        fs::write(d.join("base/keep/c.psd"), "c").unwrap();
        fs::write(d.join("over/a.png"), "over-a").unwrap();
        let spec = Spec {
            out: "build/o.deb".into(),
            fields: fields(),
            trees: vec![
                TreeEnt { from: "base".into(), to: "usr/share/x".into(), exclude: vec!["*.psd".into(), "keep/*.psd".into()], replace: false, mode: Some(0o600) },
                TreeEnt { from: "over".into(), to: "usr/share/x".into(), exclude: vec![], replace: true, mode: None },
            ],
            packages: Some("build/o.Packages".into()),
            ..Default::default()
        };
        run(&spec, &d).unwrap();
        let tree = build_tree(&spec, &d).unwrap();
        assert_eq!(tree.file("usr/share/x/a.png").unwrap().0, b"over-a");
        assert!(!tree.nodes.contains_key("usr/share/x/art.psd") && !tree.nodes.contains_key("usr/share/x/keep/c.psd"));
        assert!(tree.file("usr/share/x/keep/b.txt").is_some());
        // a forced mode applies to the tree it belongs to, not to the overlay
        assert_eq!(tree.file("usr/share/x/keep/b.txt").unwrap().1, 0o600);
        assert_eq!(tree.file("usr/share/x/a.png").unwrap().1, 0o644);
        let stanza = fs::read_to_string(d.join("build/o.Packages")).unwrap();
        let deb = fs::read(d.join("build/o.deb")).unwrap();
        assert!(stanza.contains("Package: com.x.t\n") && stanza.contains("Filename: debs/o.deb\n"), "{}", stanza);
        assert!(stanza.contains(&format!("Size: {}\n", deb.len())));
        assert!(stanza.contains(&format!("SHA256: {}\n", crate::sha::sha256_hex(&deb))));
        assert!(stanza.contains(&format!("MD5sum: {}\n", crate::md5::hex(&deb))));
        // without replace the same overlay is a duplicate
        let mut again = spec;
        again.trees[1].replace = false;
        assert!(run(&again, &d).unwrap_err().contains("twice"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn ar_members_round_trip_with_odd_sizes() {
        let mut ar = b"!<arch>\n".to_vec();
        ar_member(&mut ar, "debian-binary", b"2.0\n");
        ar_member(&mut ar, "odd", b"abc");
        ar_member(&mut ar, "last", b"");
        let m = ar_members(&ar).unwrap();
        assert_eq!(m, vec![("debian-binary".to_string(), 4), ("odd".to_string(), 3), ("last".to_string(), 0)]);
        assert!(ar_members(b"nope").is_err());
        assert!(ar_members(&ar[..ar.len() - 70]).is_err());
    }

    // (name, kind, mode, uid, gid, body or link target) of every member of a tar, names under 100 bytes
    fn tar_members(tar: &[u8]) -> Vec<(String, u8, u32, u32, u32, Vec<u8>)> {
        let field = |h: &[u8], a: usize, b: usize| String::from_utf8_lossy(&h[a..b]).trim_end_matches('\0').trim().to_string();
        let num = |h: &[u8], a: usize, b: usize| u32::from_str_radix(&field(h, a, b), 8).unwrap();
        let mut out = Vec::new();
        let mut at = 0;
        while at + 512 <= tar.len() && tar[at] != 0 {
            let h = &tar[at..at + 512];
            let size = num(h, 124, 136) as usize;
            let body = if h[156] == b'2' { field(h, 157, 257).into_bytes() } else { tar[at + 512..at + 512 + size].to_vec() };
            out.push((field(h, 0, 100), h[156], num(h, 100, 108), num(h, 108, 116), num(h, 116, 124), body));
            at += 512 + size.div_ceil(512) * 512;
        }
        out
    }

    fn deb_member(deb: &[u8], name: &str) -> Vec<u8> {
        let mut at = 8;
        for (n, size) in ar_members(deb).unwrap() {
            at += 60;
            if n == name {
                let gz = &deb[at..at + size];
                // ten byte gzip header, eight byte trailer
                let raw = crate::png::inflate(&gz[10..gz.len() - 8], 1 << 24).unwrap();
                assert_eq!(&gz[gz.len() - 8..gz.len() - 4], &crate::zip::crc32(&raw).to_le_bytes());
                return raw;
            }
            at += size + size % 2;
        }
        panic!("no member {}", name);
    }

    #[test]
    fn packs_a_deb_with_modes_md5sums_and_a_tree() {
        let d = tmp("deb");
        fs::create_dir_all(d.join("res/sub")).unwrap();
        fs::write(d.join("bin"), "binary").unwrap();
        fs::write(d.join("res/a.txt"), "a").unwrap();
        fs::write(d.join("res/sub/b.sh"), "#!/bin/sh\n").unwrap();
        fs::write(d.join("postinst"), "#!/bin/sh\nexit 0\n").unwrap();
        let spec = Spec {
            out: "build/x.deb".into(),
            fields: fields(),
            files: vec![FileEnt { from: "bin".into(), to: "/usr/bin/t".into(), mode: 0o755, replace: false }],
            trees: vec![TreeEnt { from: "res".into(), to: "usr/share/t".into(), ..Default::default() }],
            data: vec![("usr/share/t/gen.plist".into(), 0o644, "<plist/>".into())],
            scripts: vec![("postinst".into(), "postinst".into())],
            ..Default::default()
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(d.join("res/sub/b.sh"), fs::Permissions::from_mode(0o700)).unwrap();
            std::os::unix::fs::symlink("a.txt", d.join("res/link")).unwrap();
        }
        run(&spec, &d).unwrap();
        let deb = fs::read(d.join("build/x.deb")).unwrap();
        let names: Vec<String> = ar_members(&deb).unwrap().into_iter().map(|m| m.0).collect();
        assert_eq!(names, ["debian-binary", "control.tar.gz", "data.tar.gz"]);

        let data = tar_members(&deb_member(&deb, "data.tar.gz"));
        let get = |n: &str| data.iter().find(|m| m.0 == n).unwrap_or_else(|| panic!("{} not in {:?}", n, data.iter().map(|m| &m.0).collect::<Vec<_>>()));
        // every member is owned by uid and gid 0
        assert!(data.iter().all(|m| m.3 == 0 && m.4 == 0));
        assert_eq!((get("./").1, get("./").2), (b'5', 0o755));
        assert_eq!((get("./usr/share/").1, get("./usr/share/").2), (b'5', 0o755));
        assert_eq!(get("./usr/bin/t").2, 0o755);
        assert_eq!(get("./usr/share/t/a.txt").2, 0o644);
        assert_eq!(get("./usr/share/t/gen.plist").5, b"<plist/>");
        // a script with a shebang is executable even where the filesystem has no mode bits
        assert_eq!(get("./usr/share/t/sub/b.sh").2, 0o755);
        #[cfg(unix)]
        {
            assert_eq!((get("./usr/share/t/link").1, get("./usr/share/t/link").5.clone()), (b'2', b"a.txt".to_vec()));
        }
        assert!(!data.iter().any(|m| m.0.contains("DEBIAN")));
        // a directory comes before its children, and names sort within a directory
        let at = |n: &str| data.iter().position(|m| m.0 == n).unwrap();
        assert!(at("./usr/") < at("./usr/bin/") && at("./usr/bin/") < at("./usr/bin/t") && at("./usr/bin/t") < at("./usr/share/"));

        let control = tar_members(&deb_member(&deb, "control.tar.gz"));
        let cn: Vec<&str> = control.iter().map(|m| m.0.as_str()).collect();
        assert_eq!(cn, ["control", "md5sums", "postinst"]);
        assert_eq!(control[2].2, 0o755);
        let sums = String::from_utf8(control[1].5.clone()).unwrap();
        assert!(sums.contains(&format!("{}  usr/bin/t\n", crate::md5::hex(b"binary"))), "{}", sums);
        assert!(!sums.contains("link"), "{}", sums);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn names_sort_per_directory_not_as_whole_paths() {
        let mut t = Tree::default();
        for p in ["a/c", "a-b", "a.d"] {
            t.put(p, Kind::File(Vec::new()), 0o644, false).unwrap();
        }
        let order: Vec<&str> = t.ordered().into_iter().map(|(p, _)| p.as_str()).collect();
        // "a" holds "c", and "a-b" sorts after the whole directory, as tar walks it
        assert_eq!(order, ["a", "a/c", "a-b", "a.d"]);
    }

    #[test]
    fn a_file_cannot_become_a_directory_or_the_reverse() {
        let mut t = Tree::default();
        t.put("usr/bin", Kind::File(Vec::new()), 0o644, false).unwrap();
        assert!(t.put("usr/bin/x", Kind::File(Vec::new()), 0o644, false).unwrap_err().contains("usr/bin is a file"));
        t.put("usr/lib/x", Kind::File(Vec::new()), 0o644, false).unwrap();
        assert!(t.put("usr/lib", Kind::File(Vec::new()), 0o644, true).unwrap_err().contains("directory"));
    }

    #[test]
    fn an_ipa_keeps_modes_and_needs_one_app_with_an_executable() {
        let d = tmp("ipa");
        fs::write(d.join("exe"), "not looked at").unwrap();
        fs::write(d.join("text.plist"), "<plist><dict><key>CFBundleIdentifier</key><string>com.x.a</string><key>CFBundleExecutable</key><string>a</string></dict></plist>").unwrap();
        fs::write(d.join("bin.plist"), "bplist00").unwrap();
        let ipa = |plist: &str, app: &str, exe_mode: u32| Spec {
            format: Format::Ipa,
            out: "build/a.ipa".into(),
            files: vec![
                FileEnt { from: "exe".into(), to: format!("Payload/{}/a", app), mode: exe_mode, replace: false },
                FileEnt { from: plist.into(), to: format!("Payload/{}/Info.plist", app), mode: 0o644, replace: false },
            ],
            ..Default::default()
        };
        assert!(run(&ipa("text.plist", "A.app", 0o644), &d).unwrap_err().contains("not executable"));
        assert!(run(&ipa("bin.plist", "A", 0o755), &d).unwrap_err().contains("exactly one .app"));
        run(&ipa("bin.plist", "A.app", 0o755), &d).unwrap();
        let zip = fs::read(d.join("build/a.ipa")).unwrap();
        let listed = crate::zip::list(&zip).unwrap();
        let mode = |n: &str| listed.iter().find(|l| l.name == n).unwrap_or_else(|| panic!("{} missing", n)).mode & 0o777;
        assert_eq!((mode("Payload/"), mode("Payload/A.app/"), mode("Payload/A.app/a"), mode("Payload/A.app/Info.plist")), (0o755, 0o755, 0o755, 0o644));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn identical_input_gives_identical_bytes() {
        let d = tmp("repro");
        fs::write(d.join("bin"), "binary").unwrap();
        let make = |out: &str| {
            run(&Spec {
                out: out.into(),
                fields: fields(),
                files: vec![FileEnt { from: "bin".into(), to: "usr/bin/t".into(), mode: 0o755, replace: false }],
                ..Default::default()
            }, &d)
        };
        make("build/a.deb").unwrap();
        make("build/b.deb").unwrap();
        assert_eq!(fs::read(d.join("build/a.deb")).unwrap(), fs::read(d.join("build/b.deb")).unwrap());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn duplicate_paths_and_missing_sources_fail() {
        let d = tmp("dup");
        fs::write(d.join("bin"), "x").unwrap();
        let spec = |files: Vec<FileEnt>| Spec { out: "build/o.deb".into(), fields: fields(), files, ..Default::default() };
        let fe = |from: &str| FileEnt { from: from.into(), to: "usr/bin/t".into(), mode: 0o755, replace: false };
        let dup = run(&spec(vec![fe("bin"), fe("bin")]), &d);
        let missing = run(&spec(vec![fe("nope")]), &d);
        assert!(dup.unwrap_err().contains("twice"));
        assert!(missing.is_err());
        fs::remove_dir_all(&d).unwrap();
    }
}
