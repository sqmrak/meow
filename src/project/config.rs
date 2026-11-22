use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    #[serde(default)]
    pub slice: BTreeMap<String, Slice>,
    #[serde(default)]
    pub target: BTreeMap<String, Target>,
    #[serde(default)]
    pub fat: BTreeMap<String, Fat>,
    #[serde(default)]
    pub step: BTreeMap<String, Step>,
    #[serde(default)]
    pub deb: BTreeMap<String, Deb>,
    #[serde(default)]
    pub ipa: BTreeMap<String, Ipa>,
    // host test programs, built with the host compiler and run
    #[serde(default)]
    pub test: BTreeMap<String, Test>,
    // syntax-only compiles against another sdk, to catch calls newer than the target
    #[serde(default)]
    pub check: BTreeMap<String, Check>,
    // pinned downloads and git checkouts, cached outside the build directory
    #[serde(default)]
    pub dep: BTreeMap<String, Dep>,
    // release checks on a finished binary
    #[serde(default)]
    pub verify: BTreeMap<String, Verify>,
    // products of one package, see Product
    #[serde(default)]
    pub product: BTreeMap<String, Product>,
    // generated pictures: springboard icons, launch images, glyphs from svg, single images
    #[serde(default)]
    pub write: BTreeMap<String, Write>,
    #[serde(default)]
    pub icons: BTreeMap<String, Icons>,
    #[serde(default)]
    pub launch: BTreeMap<String, Launch>,
    #[serde(default)]
    pub glyphs: BTreeMap<String, Glyphs>,
    #[serde(default)]
    pub image: BTreeMap<String, ImageJob>,
    // a ready made build for one product, expanded into the tables above at load
    pub project: Option<Project>,
    // control fields of the deb the project builds
    pub package: Option<Package>,
    // built when `build` or `plan` is given no names
    #[serde(default)]
    pub default: Vec<String>,
}

// an external command sequence with declared inputs and outputs, for work meow
// does not model itself: dependency builds, host tests, packaging
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub run: Vec<Vec<String>>,
    // globs for files the commands read without naming them, files named in run are
    // inputs already
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    // meow creates this file after the commands succeed, for steps that make nothing
    pub stamp: Option<String>,
    // targets, fat outputs and steps that finish first, ${slice} is not available
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    // directory the commands run in, relative to the project, the project itself when left out
    pub cwd: Option<String>,
    // outputs survive a stale run, for scripts that rebuild only what is missing
    #[serde(default)]
    pub keep: bool,
    // runs only while an output is missing and is never stale otherwise, for one-time
    // dependency builds whose script wipes its output directory first
    #[serde(default)]
    pub bootstrap: bool,
    // a make style dependency file the commands write, the files it lists are inputs of the
    // next run, for a compiler that finds headers on its own
    pub depfile: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slice {
    pub cc: String,
    #[serde(default)]
    pub ar: String,
    // shared by compile and link: triple, arch, deployment target, sysroot
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    Exe,
    Archive,
    None,
}

impl Default for LinkKind {
    fn default() -> Self {
        LinkKind::Exe
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub slices: Vec<String>,
    #[serde(default)]
    pub src: Vec<SrcGroup>,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default)]
    pub link: LinkKind,
    pub output: Option<String>,
    // targets whose objects are linked into this one
    #[serde(default)]
    pub objects_from: Vec<String>,
    // object files that steps make, linked in as they are, ${slice} is available
    #[serde(default)]
    pub objects: Vec<String>,
    // targets that must finish first without contributing objects
    // targets, fat outputs or steps, names may use ${slice}
    #[serde(default)]
    pub needs: Vec<String>,
    // runs after a successful link, for example ldid
    #[serde(default)]
    pub post: Vec<Vec<String>>,
    #[serde(default)]
    pub per_slice: BTreeMap<String, TargetSlice>,
    // after post: strip local symbols, then sign and read the signature back
    #[serde(default)]
    pub strip: bool,
    #[serde(default)]
    pub sign: bool,
    // implies sign
    pub entitlements: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSlice {
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SrcGroup {
    pub files: Vec<String>,
    // globs matched against each path the files patterns found, those that match are left out
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fat {
    pub from: String,
    // defaults to ${MEOW_LIPO}
    pub lipo: Option<String>,
    pub output: String,
    #[serde(default)]
    pub post: Vec<Vec<String>>,
    #[serde(default)]
    pub strip: bool,
    #[serde(default)]
    pub sign: bool,
    pub entitlements: Option<String>,
}

// a debian package built from files and trees, see pack.rs
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deb {
    pub output: String,
    // a ready control file, or the fields to write one, not both
    pub control: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    // maintainer scripts by name: preinst postinst prerm postrm
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
    // put in front of every destination, for example var/jb on a rootless system
    #[serde(default)]
    pub prefix: String,
    // where to write the Packages stanza of the deb: control, Filename, Size and hashes
    pub packages: Option<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub file: Vec<DebFile>,
    #[serde(default)]
    pub tree: Vec<DebTree>,
    #[serde(default)]
    pub data: Vec<DebData>,
    // checks on the package, paths are relative to the package root, DEBIAN/x for
    // control files. each failure names the path
    // files that must exist and not be empty
    #[serde(default)]
    pub require: Vec<String>,
    // files that must exist and carry an execute bit
    #[serde(default)]
    pub require_exec: Vec<String>,
    // globs, nothing in the package may match
    #[serde(default)]
    pub forbid: Vec<String>,
    // no text file in the package may contain one of these strings
    #[serde(default)]
    pub forbid_text: Vec<String>,
    // refuse a setuid or setgid file
    #[serde(default)]
    pub forbid_setuid: bool,
    // files run through `sh -n`
    #[serde(default)]
    pub sh_check: Vec<String>,
    // a file that has to contain a string
    #[serde(default)]
    pub contains: Vec<DebContains>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebContains {
    pub file: String,
    pub text: String,
}

// a small generated file, a wrapper script for a configure step for example
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub to: String,
    // ${...} is expanded, so a path of the build can be written into it
    pub content: String,
    // octal, 644 when left out
    pub mode: Option<String>,
    #[serde(default)]
    pub needs: Vec<String>,
}

// an ipa: a zip with Payload/<name>.app inside, built from the same files and trees
// as a deb. the paths in file, tree and data are relative to prefix
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ipa {
    pub output: String,
    // normally Payload/Name.app
    pub prefix: String,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub file: Vec<DebFile>,
    #[serde(default)]
    pub tree: Vec<DebTree>,
    #[serde(default)]
    pub data: Vec<DebData>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebFile {
    pub from: String,
    pub to: String,
    // octal, 644 when left out
    pub mode: Option<String>,
    // overwrite a file an earlier entry put at the same place
    #[serde(default)]
    pub replace: bool,
}

// a directory copied whole, executables keep 755 and everything else gets 644
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebTree {
    pub from: String,
    pub to: String,
    // globs relative to from, matching files or directories that are left out
    #[serde(default)]
    pub exclude: Vec<String>,
    // files of this tree overwrite what earlier entries put at the same place
    #[serde(default)]
    pub replace: bool,
    // octal mode for every file, otherwise 755 for a source with an execute bit and 644 for the rest
    pub mode: Option<String>,
}

// a small file written from text in the config, for example a substrate filter
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebData {
    pub to: String,
    pub mode: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Tool,
    App,
    Tweak,
    Library,
    // compiled once and linked into other products with uses, installed nowhere
    Objects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    #[default]
    Rootful,
    Rootless,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    // left out when the package is made of [product.*] tables
    pub kind: Option<Kind>,
    // slices to build, armv7 and arm64 when left out
    pub archs: Option<Vec<String>>,
    // deployment target of the oldest slice, 5.0 when left out
    pub min_ios: Option<String>,
    // the sdk to compile against, `9.3` for iPhoneOS9.3.sdk, `meow sdk 9.3` installs it.
    // the newest one that the slice can use when left out
    pub sdk: Option<String>,
    #[serde(default)]
    pub scheme: Scheme,
    // sources, every c, objective-c and c++ file under src/ when left out
    pub src: Option<Vec<String>>,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub libs: Vec<String>,
    // objective-c is manual reference counting unless this is set
    #[serde(default)]
    pub arc: bool,
    // strip local symbols from the release binary, on when left out
    pub strip: Option<bool>,
    pub entitlements: Option<String>,
    // app: Info.plist of the bundle and a directory copied into it
    pub info_plist: Option<String>,
    pub resources: Option<String>,
    // app: bundle directory name, defaults to name
    pub bundle: Option<String>,
    // tweak: what the filter plist lists
    #[serde(default)]
    pub filter_bundles: Vec<String>,
    #[serde(default)]
    pub filter_executables: Vec<String>,
    // bundle: directory extension, bundle by default
    pub bundle_extension: Option<String>,
    // the format Info.plist is packaged in, binary or xml, as written when left out
    pub plist: Option<String>,
    // where a tool, library, bundle or framework lands in the package, relative to the root
    pub install: Option<String>,
    // backend for .x and .xm sources: MobileSubstrate, libhooker or internal
    pub logos: Option<String>,
    // rust: the directory of the crate, with its Cargo.toml, the project directory when left out
    pub crate_dir: Option<String>,
    // swift: flags for swiftc, the module name, a bridging header for the objective-c the swift code uses
    #[serde(default)]
    pub swiftflags: Vec<String>,
    pub swift_module: Option<String>,
    pub bridging_header: Option<String>,
    // rust: cargo features
    #[serde(default)]
    pub features: Vec<String>,
    // rust: more files whose change rebuilds the crate, for path dependencies outside crate_dir
    #[serde(default)]
    pub rust_inputs: Vec<String>,
    // go: the directory with go.mod, the project directory when left out, and the package in it
    pub go_dir: Option<String>,
    pub go_pkg: Option<String>,
    // go: build tags and more flags for `go build`
    #[serde(default)]
    pub go_tags: Vec<String>,
    #[serde(default)]
    pub goflags: Vec<String>,
    // go: more files whose change rebuilds the package, for replaced modules outside go_dir
    #[serde(default)]
    pub go_inputs: Vec<String>,
    #[serde(default)]
    pub file: Vec<DebFile>,
    #[serde(default)]
    pub tree: Vec<DebTree>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub id: String,
    pub name: Option<String>,
    // a version, or @file:key to read it from a text plist or a control file
    pub version: String,
    // files that must carry the same version: a text plist key or a control field
    #[serde(default)]
    pub version_checks: Vec<VersionCheck>,
    // write the Packages stanza next to the deb
    #[serde(default)]
    pub packages_stanza: bool,
    pub description: String,
    pub maintainer: String,
    pub author: Option<String>,
    pub section: Option<String>,
    // replaces the default firmware and substrate dependencies
    pub depends: Option<String>,
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
    // extra control fields
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

const MAX_INCLUDE_DEPTH: u32 = 4;
// tables whose keys are merged, a name defined twice is an error
const MERGED: [&str; 13] = ["vars", "slice", "target", "fat", "step", "deb", "ipa", "test", "check", "dep", "verify", "product", "write"];

// include = ["common.toml"] pulls in the tables of other files. paths inside every
// file are relative to the directory of the file that was named on the command line
fn read_table(path: &Path, depth: u32) -> Result<toml::Table> {
    let text = std::fs::read_to_string(path).map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    let mut table: toml::Table = toml::from_str(&text).map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    let Some(inc) = table.remove("include") else {
        return Ok(table);
    };
    if depth >= MAX_INCLUDE_DEPTH {
        return err(format!("{}: includes nest deeper than {}", path.display(), MAX_INCLUDE_DEPTH));
    }
    let list = inc.as_array().ok_or_else(|| Error(format!("{}: include must be a list of paths", path.display())))?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    for item in list {
        let rel = item.as_str().ok_or_else(|| Error(format!("{}: include entries are strings", path.display())))?;
        let other_path = dir.join(rel);
        let other = read_table(&other_path, depth + 1)?;
        for (key, value) in other {
            if !MERGED.contains(&key.as_str()) {
                return err(format!("{}: {} is only allowed in the main file", other_path.display(), key));
            }
            let toml::Value::Table(incoming) = value else {
                return err(format!("{}: {} is not a table", other_path.display(), key));
            };
            let slot = table.entry(key.clone()).or_insert_with(|| toml::Value::Table(toml::Table::new()));
            let toml::Value::Table(mine) = slot else {
                return err(format!("{}: {} is not a table", path.display(), key));
            };
            for (name, v) in incoming {
                if mine.contains_key(&name) {
                    return err(format!("{} is defined in {} and in {}", name, path.display(), other_path.display()));
                }
                mine.insert(name, v);
            }
        }
    }
    Ok(table)
}

// a file that this meow cannot read is either broken or written for a newer meow
pub const UPDATE_HINT: &str = "meow.toml may be out of date or written for a newer meow, update meow: https://github.com/sqmrak/meow";

fn unreadable(path: &Path, e: impl std::fmt::Display) -> Error {
    Error(format!("{}: {}\n{}", path.display(), e, UPDATE_HINT))
}

pub fn load(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    let has_include = toml::from_str::<toml::Table>(&text).map_or(false, |t| t.contains_key("include"));
    // the include path reads the files again, a failure there is about the files as well
    // without includes the file goes to the parser as text, so errors carry line numbers
    let mut cfg: Config = if has_include {
        toml::Value::Table(read_table(path, 0).map_err(|e| Error(format!("{}\n{}", e.0, UPDATE_HINT)))?).try_into().map_err(|e| unreadable(path, e))?
    } else {
        toml::from_str(&text).map_err(|e| unreadable(path, e))?
    };
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    crate::kinds::expand(&mut cfg, root).map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    validate(&cfg)?;
    Ok(cfg)
}

fn validate(cfg: &Config) -> Result<()> {
    for (name, t) in &cfg.target {
        if t.slices.is_empty() {
            return err(format!("target {}: no slices", name));
        }
        for s in &t.slices {
            if !cfg.slice.contains_key(s) {
                return err(format!("target {}: unknown slice {}", name, s));
            }
        }
        for s in t.per_slice.keys() {
            if !t.slices.contains(s) {
                return err(format!("target {}: per_slice {} is not in slices", name, s));
            }
        }
        for dep in &t.needs {
            if !dep.contains("${") && !known(cfg, dep) {
                return err(format!("target {}: unknown need {}", name, dep));
            }
        }
        for dep in &t.objects_from {
            let d = cfg
                .target
                .get(dep)
                .ok_or_else(|| Error(format!("target {}: unknown target {}", name, dep)))?;
            for s in &t.slices {
                if !d.slices.contains(s) {
                    return err(format!(
                        "target {}: {} has no slice {}",
                        name, dep, s
                    ));
                }
            }
        }
        if t.link != LinkKind::None && t.output.is_none() {
            return err(format!("target {}: output is required to link", name));
        }
        if t.link == LinkKind::Archive && !t.post.is_empty() {
            return err(format!("target {}: post is for linked executables", name));
        }
        if (t.strip || t.sign || t.entitlements.is_some()) && t.link != LinkKind::Exe {
            return err(format!("target {}: strip and sign are for linked executables", name));
        }
    }
    for (name, f) in &cfg.fat {
        let t = cfg
            .target
            .get(&f.from)
            .ok_or_else(|| Error(format!("fat {}: unknown target {}", name, f.from)))?;
        if t.link == LinkKind::None {
            return err(format!("fat {}: {} links nothing", name, f.from));
        }
        if t.link == LinkKind::Archive && (f.strip || f.sign || f.entitlements.is_some() || !f.post.is_empty()) {
            return err(format!("fat {}: strip, sign and post are for linked executables, {} is an archive", name, f.from));
        }
        if t.slices.len() < 2 {
            return err(format!("fat {}: {} has one slice", name, f.from));
        }
    }
    for (name, d) in &cfg.deb {
        if d.output.is_empty() {
            return err(format!("deb {}: output is empty", name));
        }
        if d.control.is_some() == !d.fields.is_empty() {
            return err(format!("deb {}: give either control or fields", name));
        }
        for dep in &d.needs {
            if !known(cfg, dep) {
                return err(format!("deb {}: unknown need {}", name, dep));
            }
        }
        for s in d.scripts.keys() {
            if !["preinst", "postinst", "prerm", "postrm"].contains(&s.as_str()) {
                return err(format!("deb {}: {} is not a maintainer script", name, s));
            }
        }
        for f in &d.file {
            if let Some(m) = &f.mode {
                if u32::from_str_radix(m, 8).is_err() {
                    return err(format!("deb {}: bad mode {} for {}", name, m, f.to));
                }
            }
        }
    }
    for (name, i) in &cfg.ipa {
        if i.output.is_empty() || !i.prefix.starts_with("Payload/") {
            return err(format!("ipa {}: output is required and prefix must start with Payload/", name));
        }
        for dep in &i.needs {
            if !known(cfg, dep) {
                return err(format!("ipa {}: unknown need {}", name, dep));
            }
        }
    }
    for (name, st) in &cfg.step {
        if st.run.is_empty() || st.run.iter().any(|c| c.is_empty()) {
            return err(format!("step {}: run needs at least one non-empty command", name));
        }
        if st.outputs.is_empty() && st.stamp.is_none() {
            return err(format!("step {}: declare outputs or a stamp", name));
        }
        for dep in &st.needs {
            if !known(cfg, dep) {
                return err(format!("step {}: unknown need {}", name, dep));
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in cfg.target.keys().chain(cfg.fat.keys()).chain(cfg.step.keys()).chain(cfg.deb.keys()).chain(cfg.ipa.keys()).chain(cfg.verify.keys()) {
        if !seen.insert(name) {
            return err(format!("{} is defined as more than one of target, fat, step, deb, ipa and verify", name));
        }
    }
    for name in &cfg.default {
        if !known(cfg, name) {
            return err(format!("default: unknown name {}", name));
        }
    }
    Ok(())
}

#[cfg(test)]
pub fn validate_for_test(cfg: &Config) -> Result<()> {
    validate(cfg)
}

fn known(cfg: &Config, name: &str) -> bool {
    cfg.target.contains_key(name) || cfg.fat.contains_key(name) || cfg.step.contains_key(name) || cfg.deb.contains_key(name) || cfg.ipa.contains_key(name) || cfg.verify.contains_key(name)
}

// index of the brace that closes a ${ whose body starts at the front of text,
// nested ${ } in a default value are skipped
fn closing_brace(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'$' if b.get(i + 1) == Some(&b'{') => {
                depth += 1;
                i += 1;
            }
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

// variable lookup walks the layers from the most specific one, then the process
// environment, so a project can pin a default and a developer can override it
pub struct Scope<'a> {
    layers: Vec<&'a BTreeMap<String, String>>,
    builtin: BTreeMap<String, String>,
    root: Option<std::path::PathBuf>,
}

impl<'a> Scope<'a> {
    pub fn new(layers: Vec<&'a BTreeMap<String, String>>) -> Self {
        Scope {
            layers,
            builtin: BTreeMap::new(),
            root: None,
        }
    }

    // gives ${root} and lets ${field:FILE:KEY} find files
    pub fn with_root(mut self, root: &Path) -> Self {
        self.builtin
            .insert("root".to_string(), root.to_string_lossy().into_owned());
        self.root = Some(root.to_path_buf());
        self
    }

    // value of a "KEY: value" line, the shape of debian control files
    fn field(&self, spec: &str) -> Result<String> {
        let (file, key) = spec
            .rsplit_once(':')
            .ok_or_else(|| Error(format!("field:{} needs FILE:KEY", spec)))?;
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| Error("field: needs a project root".to_string()))?;
        let path = root.join(file);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
        let prefix = format!("{}:", key);
        for line in text.lines() {
            if let Some(v) = line.strip_prefix(&prefix) {
                return Ok(v.trim().to_string());
            }
        }
        err(format!("{}: no {} line", path.display(), key))
    }

    pub fn set(&mut self, k: &str, v: &str) {
        self.builtin.insert(k.to_string(), v.to_string());
    }

    fn lookup(&self, name: &str) -> Option<String> {
        if let Some(v) = self.builtin.get(name) {
            return Some(v.clone());
        }
        for l in &self.layers {
            if let Some(v) = l.get(name) {
                return Some(v.clone());
            }
        }
        if let Ok(v) = std::env::var(name) {
            return Some(v);
        }
        if name == "MEOW" {
            return std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned());
        }
        if name == "MEOW_TOOLS" {
            return crate::toolchain::tools_root().map(|p| p.to_string_lossy().into_owned());
        }
        if name == "MEOW_JOBS" {
            return Some(std::thread::available_parallelism().map_or(1, |n| n.get()).to_string());
        }
        // MEOW_SDK_9_3 is the sdk of that version, what `sdk = "9.3"` in a project names
        if let Some(v) = name.strip_prefix("MEOW_SDK_").filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit())) {
            return crate::toolchain::sdk_named(&v.replace('_', ".")).map(|p| p.to_string_lossy().into_owned());
        }
        // tools and sdks that meow found by itself, see toolchain.rs
        if name.starts_with("MEOW_") {
            return crate::toolchain::discovered_vars().get(name).cloned();
        }
        None
    }

    pub fn expand(&self, s: &str) -> Result<String> {
        self.expand_depth(s, 0)
    }

    fn expand_depth(&self, s: &str, depth: u32) -> Result<String> {
        if depth > 8 {
            return err(format!("variable cycle while expanding {:?}", s));
        }
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find("${") {
            out.push_str(&rest[..i]);
            let tail = &rest[i + 2..];
            let end = closing_brace(tail)
                .ok_or_else(|| Error(format!("unterminated ${{ in {:?}", s)))?;
            let body = &tail[..end];
            let (name, default) = match body.find(":-") {
                Some(p) => (&body[..p], Some(&body[p + 2..])),
                None => (body, None),
            };
            if let Some(spec) = name.strip_prefix("field:") {
                out.push_str(&self.field(spec)?);
                rest = &tail[end + 1..];
                continue;
            }
            let value = match self.lookup(name) {
                Some(v) => self.expand_depth(&v, depth + 1)?,
                None => match default {
                    Some(d) => self.expand_depth(d, depth + 1)?,
                    None if name.strip_prefix("MEOW_SDK_").is_some_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit())) => {
                        let v = name["MEOW_SDK_".len()..].replace('_', ".");
                        return err(format!("sdk {} is not installed, run `meow sdk {}` or set THEOS or MEOW_TOOLS to a tree that has iPhoneOS{}.sdk", v, v, v));
                    }
                    None if name.starts_with("MEOW_") => {
                        return err(format!("variable {} is not set and meow found nothing for it, run `meow doctor` (in {:?})", name, s))
                    }
                    None => return err(format!("variable {} is not set (in {:?})", name, s)),
                },
            };
            out.push_str(&value);
            rest = &tail[end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    // empty results are dropped so an optional flag can expand to nothing
    pub fn expand_list(&self, list: &[String]) -> Result<Vec<String>> {
        let mut out = Vec::with_capacity(list.len());
        for item in list {
            let v = self.expand(item)?;
            if !v.is_empty() {
                out.push(v);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn expands_nested_and_defaults() {
        let g = map(&[("TC", "${ROOT}/bin"), ("ROOT", "/opt")]);
        let s = Scope::new(vec![&g]);
        assert_eq!(s.expand("${TC}/clang").unwrap(), "/opt/bin/clang");
        assert_eq!(s.expand("${NOPE_MEOW:-x}y").unwrap(), "xy");
        assert_eq!(s.expand("${NOPE_MEOW:-}").unwrap(), "");
        assert!(s.expand("${NOPE_MEOW}").is_err());
        assert!(s.expand("${unterminated").is_err());
        let g2 = map(&[("B", "bee")]);
        let s2 = Scope::new(vec![&g2]);
        assert_eq!(s2.expand("${NOPE_MEOW:-${B}}!").unwrap(), "bee!");
        assert_eq!(s2.expand("${B:-${NOPE_MEOW}}").unwrap(), "bee");
        assert!(s2.expand("${NOPE_MEOW:-${B}").is_err());
    }

    #[test]
    fn slice_layer_wins_over_global() {
        let g = map(&[("SDK", "global")]);
        let sl = map(&[("SDK", "slice")]);
        let s = Scope::new(vec![&sl, &g]);
        assert_eq!(s.expand("${SDK}").unwrap(), "slice");
    }

    #[test]
    fn field_reads_a_control_line() {
        let d = std::env::temp_dir().join(format!("meow-field-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("control"), "Package: x\nVersion: 1.2.3-stable\n").unwrap();
        let g = BTreeMap::new();
        let s = Scope::new(vec![&g]).with_root(&d);
        assert_eq!(s.expand("v${field:control:Version}.deb").unwrap(), "v1.2.3-stable.deb");
        assert!(s.expand("${field:control:Missing}").is_err());
        assert!(s.expand("${field:nofile:Version}").is_err());
        assert_eq!(s.expand("${root}").unwrap(), d.to_string_lossy());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn cycle_is_an_error() {
        let g = map(&[("A", "${B}"), ("B", "${A}")]);
        let s = Scope::new(vec![&g]);
        assert!(s.expand("${A}").is_err());
    }

    #[test]
    fn builtin_shadows_layers_and_empty_args_drop() {
        let g = map(&[("out", "layer")]);
        let mut s = Scope::new(vec![&g]);
        s.set("out", "real");
        let list = vec!["${out}".to_string(), "${NOPE_MEOW:-}".to_string()];
        assert_eq!(s.expand_list(&list).unwrap(), vec!["real".to_string()]);
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("meow-inc-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn include_merges_tables_and_refuses_clashes() {
        let d = dir("ok");
        std::fs::create_dir_all(d.join("shared")).unwrap();
        std::fs::write(d.join("shared/common.toml"), "[vars]\nTC = \"/tc\"\n[slice.a]\ncc = \"cc\"\ninclude = []\n").unwrap_or(());
        std::fs::write(d.join("shared/common.toml"), "[vars]\nTC = \"/tc\"\n[slice.a]\ncc = \"cc\"\n").unwrap();
        std::fs::write(d.join("meow.toml"), "include = [\"shared/common.toml\"]\n[vars]\nOWN = \"1\"\n[target.t]\nslices = [\"a\"]\nlink = \"none\"\n").unwrap();
        let cfg = load(&d.join("meow.toml")).unwrap();
        assert_eq!(cfg.vars["TC"], "/tc");
        assert_eq!(cfg.vars["OWN"], "1");
        assert!(cfg.slice.contains_key("a"));
        // the same name twice is an error that names both files
        std::fs::write(d.join("meow.toml"), "include = [\"shared/common.toml\"]\n[vars]\nTC = \"/other\"\n").unwrap();
        let e = load(&d.join("meow.toml")).unwrap_err();
        assert!(e.0.contains("TC") && e.0.contains("common.toml"), "{}", e.0);
        // default and project belong to the main file
        std::fs::write(d.join("shared/bad.toml"), "default = [\"x\"]\n").unwrap();
        std::fs::write(d.join("meow.toml"), "include = [\"shared/bad.toml\"]\n").unwrap();
        assert!(load(&d.join("meow.toml")).unwrap_err().0.contains("only allowed in the main file"));
        // a loop of includes stops
        std::fs::write(d.join("shared/a.toml"), "include = [\"b.toml\"]\n").unwrap();
        std::fs::write(d.join("shared/b.toml"), "include = [\"a.toml\"]\n").unwrap();
        std::fs::write(d.join("meow.toml"), "include = [\"shared/a.toml\"]\n").unwrap();
        assert!(load(&d.join("meow.toml")).unwrap_err().0.contains("nest deeper"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_unreadable_file_points_at_the_update() {
        let d = dir("hint");
        std::fs::write(d.join("meow.toml"), "[target.t]\nslices = [\"a\"]\nfuture_option = true\n").unwrap();
        let e = load(&d.join("meow.toml")).unwrap_err();
        assert!(e.0.contains("future_option") && e.0.contains("https://github.com/sqmrak/meow"), "{}", e.0);
        // a file with a syntax error gets it too, a mistake in the build graph does not
        std::fs::write(d.join("meow.toml"), "[target.t\n").unwrap();
        assert!(load(&d.join("meow.toml")).unwrap_err().0.contains("github.com/sqmrak/meow"));
        std::fs::write(d.join("meow.toml"), "default = [\"nope\"]\n").unwrap();
        assert!(!load(&d.join("meow.toml")).unwrap_err().0.contains("github.com"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rejects_unknown_keys() {
        let r: std::result::Result<Config, _> = toml::from_str("[vars]\nA='1'\n[bogus]\n");
        assert!(r.is_err());
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionCheck {
    pub file: String,
    pub key: String,
}

// one product of a package with several: an app, a daemon, an injected library
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub kind: Option<Kind>,
    pub archs: Option<Vec<String>>,
    // required with several products, one product may leave it out
    #[serde(default)]
    pub src: Vec<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub libs: Vec<String>,
    pub arc: Option<bool>,
    pub strip: Option<bool>,
    pub entitlements: Option<String>,
    pub info_plist: Option<String>,
    pub resources: Option<String>,
    pub bundle: Option<String>,
    #[serde(default)]
    pub filter_bundles: Vec<String>,
    #[serde(default)]
    pub filter_executables: Vec<String>,
    // where a tool, library, bundle, framework or appex lands in the package, relative to the root
    pub install: Option<String>,
    pub bundle_extension: Option<String>,
    pub plist: Option<String>,
    pub logos: Option<String>,
    pub crate_dir: Option<String>,
    // swift: flags for swiftc, the module name, a bridging header for the objective-c the swift code uses
    #[serde(default)]
    pub swiftflags: Vec<String>,
    pub swift_module: Option<String>,
    pub bridging_header: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub rust_inputs: Vec<String>,
    // go: the directory with go.mod, the project directory when left out, and the package in it
    pub go_dir: Option<String>,
    pub go_pkg: Option<String>,
    // go: build tags and more flags for `go build`
    #[serde(default)]
    pub go_tags: Vec<String>,
    #[serde(default)]
    pub goflags: Vec<String>,
    // go: more files whose change rebuilds the package, for replaced modules outside go_dir
    #[serde(default)]
    pub go_inputs: Vec<String>,
    // products of kind objects whose objects are linked in
    #[serde(default)]
    pub uses: Vec<String>,
    // steps, deps and products that finish first
    #[serde(default)]
    pub needs: Vec<String>,
}

// a host program: sources, flags, an optional sanitizer, run after the build
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Test {
    // sources of a program, or leave out and give run for a script
    #[serde(default)]
    pub src: Vec<String>,
    // a script or program that is not built here, run as it is
    pub run: Option<Vec<String>>,
    // directory the test runs in, relative to the project
    pub cwd: Option<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default)]
    pub libs: Vec<String>,
    // address, undefined, thread
    pub sanitize: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub needs: Vec<String>,
    // globs the program reads when it runs, for example fixtures
    #[serde(default)]
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    // a slice of the file, its flags are used with the sdk below
    pub slice: String,
    pub sdk: String,
    pub src: Vec<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
    // header globs, a change reruns the check
    #[serde(default)]
    pub headers: Vec<String>,
    #[serde(default)]
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dep {
    // an archive, with the sha256 it must have
    pub url: Option<String>,
    pub sha256: Option<String>,
    // or a git repository at a full commit id
    pub git: Option<String>,
    pub rev: Option<String>,
    // leading path parts removed from the archive, 1 when left out
    pub strip: Option<u32>,
    // applied in order with patch -p1
    #[serde(default)]
    pub patches: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verify {
    pub file: String,
    #[serde(default)]
    pub slices: Vec<String>,
    // the oldest release the binary is meant for, each cpu is raised to its own first release
    pub min_ios: Option<String>,
    #[serde(default)]
    pub signed: bool,
    #[serde(default)]
    pub allow_dylibs: Vec<String>,
    #[serde(default)]
    pub forbid_dylibs: Vec<String>,
    // undefined symbols that must not appear, read with nm
    #[serde(default)]
    pub forbid_symbols: Vec<String>,
    // a file with one symbol per line, # starts a comment
    pub forbid_symbols_file: Option<String>,
    // external symbols the binary has to define, read with nm
    #[serde(default)]
    pub require_symbols: Vec<String>,
    #[serde(default)]
    pub needs: Vec<String>,
}

// every size springboard asks for, cut from one master png
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Icons {
    pub master: String,
    pub out: String,
    // "ios": Icon.png 57 to Icon-83.5@2x.png 167, the names ios 5 to 15 look for
    pub preset: Option<String>,
    // name = edge in pixels, added to the preset, an equal name replaces it
    #[serde(default)]
    pub sizes: BTreeMap<String, u32>,
    // rounded corners, the radius as a fraction of the width: 0.18
    pub round: Option<f64>,
    // a png laid over the icon, scaled to its size
    pub overlay: Option<String>,
    // edges that get the overlay, every size when left out
    #[serde(default)]
    pub overlay_sizes: Vec<u32>,
    // drop the alpha channel, on unless the corners are rounded
    pub drop_alpha: Option<bool>,
    #[serde(default)]
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchImage {
    pub name: String,
    // in points, the file is width*scale by height*scale pixels
    pub width: u32,
    pub height: u32,
    pub scale: Option<u32>,
}

// launch images: a flat color, with the icon in the middle when one is named
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub out: String,
    pub background: String,
    pub icon: Option<String>,
    // edge of the icon in points
    pub icon_points: Option<u32>,
    pub icon_round: Option<f64>,
    pub images: Vec<LaunchImage>,
    #[serde(default)]
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlyphItem {
    pub from: String,
    pub name: String,
}

// svg or android vector files rendered square at several scales
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Glyphs {
    // globs, the file stem is the name
    #[serde(default)]
    pub src: Vec<String>,
    // or files with their own names
    #[serde(default)]
    pub items: Vec<GlyphItem>,
    pub out: String,
    // points at scale 1, 24 when left out
    pub size: Option<u32>,
    // 1 gives name.png, 2 name@2x.png, 3 name@3x.png, [1, 2] when left out
    #[serde(default)]
    pub scales: Vec<u32>,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    // replaces the fill and stroke colors: #ffffff
    pub color: Option<String>,
    #[serde(default)]
    pub needs: Vec<String>,
}

// one picture made from another: crop to the opaque part, resize, round, canvas
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageJob {
    pub from: String,
    pub to: String,
    // exact size, or fit: inside the box, never enlarged
    pub size: Option<[u32; 2]>,
    pub fit: Option<[u32; 2]>,
    // crop to the box of pixels with at least this alpha, before anything else
    pub crop_alpha: Option<u8>,
    pub canvas: Option<[u32; 2]>,
    pub background: Option<String>,
    pub round: Option<f64>,
    #[serde(default)]
    pub drop_alpha: bool,
    // each scale multiplies the sizes and adds @2x to the file name, [1] when left out
    #[serde(default)]
    pub scales: Vec<u32>,
    #[serde(default)]
    pub needs: Vec<String>,
}
