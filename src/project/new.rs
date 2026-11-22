// `meow new` lists the templates and makes a project from one. the files are in templates/,
// this table names them, a new file there needs a line here

use crate::kinds::valid_name;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
struct File {
    path: &'static str,
    text: &'static str,
}

#[derive(Debug)]
struct Template {
    name: &'static str,
    about: &'static str,
    files: &'static [File],
}

const TEMPLATES: &[Template] = &[
    Template {
        name: "tweak",
        about: "a MobileSubstrate tweak with a logos source",
        files: &[
            File { path: "meow.toml", text: include_str!("../../templates/tweak/meow.toml") },
            File { path: "src/Tweak.x", text: include_str!("../../templates/tweak/src/Tweak.x") },
        ],
    },
    Template {
        name: "tool",
        about: "a command line tool",
        files: &[
            File { path: "meow.toml", text: include_str!("../../templates/tool/meow.toml") },
            File { path: "src/main.m", text: include_str!("../../templates/tool/src/main.m") },
        ],
    },
    Template {
        name: "library",
        about: "a dynamic library",
        files: &[
            File { path: "meow.toml", text: include_str!("../../templates/library/meow.toml") },
            File { path: "src/@NAME@.c", text: include_str!("../../templates/library/src/@NAME@.c") },
            File { path: "src/@NAME@.h", text: include_str!("../../templates/library/src/@NAME@.h") },
        ],
    },
    Template {
        name: "framework",
        about: "a framework",
        files: &[
            File { path: "Info.plist", text: include_str!("../../templates/framework/Info.plist") },
            File { path: "meow.toml", text: include_str!("../../templates/framework/meow.toml") },
            File { path: "src/@NAME@.m", text: include_str!("../../templates/framework/src/@NAME@.m") },
        ],
    },
    Template {
        name: "app",
        about: "a UIKit application",
        files: &[
            File { path: "Info.plist", text: include_str!("../../templates/app/Info.plist") },
            File { path: "meow.toml", text: include_str!("../../templates/app/meow.toml") },
            File { path: "src/@PREFIX@AppDelegate.h", text: include_str!("../../templates/app/src/@PREFIX@AppDelegate.h") },
            File { path: "src/@PREFIX@AppDelegate.m", text: include_str!("../../templates/app/src/@PREFIX@AppDelegate.m") },
            File { path: "src/main.m", text: include_str!("../../templates/app/src/main.m") },
        ],
    },
    Template {
        name: "prefs",
        about: "a preference bundle for PreferenceLoader",
        files: &[
            File { path: "Resources/Root.plist", text: include_str!("../../templates/prefs/Resources/Root.plist") },
            File { path: "Info.plist", text: include_str!("../../templates/prefs/Info.plist") },
            File { path: "entry.plist", text: include_str!("../../templates/prefs/entry.plist") },
            File { path: "meow.toml", text: include_str!("../../templates/prefs/meow.toml") },
            File { path: "src/@PREFIX@RootListController.h", text: include_str!("../../templates/prefs/src/@PREFIX@RootListController.h") },
            File { path: "src/@PREFIX@RootListController.m", text: include_str!("../../templates/prefs/src/@PREFIX@RootListController.m") },
        ],
    },
    Template {
        name: "appex",
        about: "an app extension",
        files: &[
            File { path: "Info.plist", text: include_str!("../../templates/appex/Info.plist") },
            File { path: "meow.toml", text: include_str!("../../templates/appex/meow.toml") },
            File { path: "src/@PREFIX@Extension.m", text: include_str!("../../templates/appex/src/@PREFIX@Extension.m") },
        ],
    },
    Template {
        name: "swift",
        about: "a tool written in swift (ios 12.2 and later, arm64)",
        files: &[
            File { path: "meow.toml", text: include_str!("../../templates/swift/meow.toml") },
            File { path: "src/Greeting.swift", text: include_str!("../../templates/swift/src/Greeting.swift") },
            File { path: "src/main.swift", text: include_str!("../../templates/swift/src/main.swift") },
        ],
    },
    Template {
        name: "rust",
        about: "a c tool that links a rust library (arm64)",
        files: &[
            File { path: "core/Cargo.toml", text: include_str!("../../templates/rust/core/Cargo.toml") },
            File { path: "core/src/lib.rs", text: include_str!("../../templates/rust/core/src/lib.rs") },
            File { path: "meow.toml", text: include_str!("../../templates/rust/meow.toml") },
            File { path: "src/main.c", text: include_str!("../../templates/rust/src/main.c") },
        ],
    },
];

pub const USAGE: &str = "usage: meow new                        list the templates
       meow new TEMPLATE NAME [--id ID] [--maintainer WHO] [--dir DIR]

  NAME          letters, digits, . _ -, the project and its binary
  --id ID       package id, com.example.<name> when left out
  --maintainer  the Maintainer field, git's user.name when left out
  --dir DIR     where to write the project, ./NAME when left out, it must be empty or new";

// the objective-c class prefix, the letters and digits of the name with a capital first
fn class_prefix(name: &str) -> String {
    let mut s: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, 'X');
    }
    let mut c = s.chars();
    let first = c.next().map(|f| f.to_ascii_uppercase()).unwrap_or('X');
    format!("{}{}", first, c.as_str())
}

fn default_id(name: &str) -> String {
    let tail: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    format!("com.example.{}", if tail.is_empty() { "app".to_string() } else { tail })
}

fn git_user() -> Option<String> {
    let out = Command::new("git").args(["config", "user.name"]).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !name.is_empty() { Some(name) } else { None }
}

#[derive(Debug)]
pub struct Spec {
    template: &'static Template,
    name: String,
    id: String,
    maintainer: String,
    dir: PathBuf,
}

pub fn parse_args(args: &[String]) -> Result<Option<Spec>, String> {
    let mut pos: Vec<&str> = Vec::new();
    let (mut id, mut maintainer, mut dir): (Option<String>, Option<String>, Option<PathBuf>) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or_else(|| format!("new: {} needs a value", flag));
        match a.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--id" => id = Some(value("--id")?),
            "--maintainer" => maintainer = Some(value("--maintainer")?),
            "--dir" => dir = Some(PathBuf::from(value("--dir")?)),
            o if o.starts_with('-') => return Err(format!("new: unknown option {}\n{}", o, USAGE)),
            p => pos.push(p),
        }
    }
    match pos.as_slice() {
        [] => Ok(None),
        [t, name] => {
            let template = TEMPLATES.iter().find(|x| x.name == *t).ok_or_else(|| {
                format!("new: no template {}, the templates are {}", t, TEMPLATES.iter().map(|x| x.name).collect::<Vec<_>>().join(", "))
            })?;
            if !valid_name(name) {
                return Err(format!("new: bad name {:?}, use letters, digits, . _ -", name));
            }
            let id = id.unwrap_or_else(|| default_id(name));
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c)) {
                return Err(format!("new: bad package id {:?}, use lowercase letters, digits, + . -", id));
            }
            Ok(Some(Spec {
                template,
                name: name.to_string(),
                id,
                maintainer: maintainer.or_else(git_user).unwrap_or_else(|| "you".to_string()),
                dir: dir.unwrap_or_else(|| PathBuf::from(name)),
            }))
        }
        _ => Err(format!("new: give a template and a name\n{}", USAGE)),
    }
}

pub fn list() {
    println!("{:<10} {}", "TEMPLATE", "ABOUT");
    for t in TEMPLATES {
        println!("{:<10} {}", t.name, t.about);
    }
    println!("\nmake one with `meow new TEMPLATE NAME`");
}

fn fill(text: &str, s: &Spec) -> String {
    text.replace("@NAME@", &s.name)
        .replace("@ID@", &s.id)
        .replace("@MAINTAINER@", &s.maintainer)
        .replace("@PREFIX@", &class_prefix(&s.name))
}

// the files a spec writes, relative to its directory
fn render(s: &Spec) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = s.template.files.iter().map(|f| (fill(f.path, s), fill(f.text, s))).collect();
    out.push((".gitignore".to_string(), "build/\n".to_string()));
    out
}

pub fn run(s: &Spec) -> Result<(), String> {
    if s.dir.exists() {
        let empty = s.dir.is_dir() && std::fs::read_dir(&s.dir).map_err(|e| format!("{}: {}", s.dir.display(), e))?.next().is_none();
        if !empty {
            return Err(format!("{} exists and is not empty, nothing was written", s.dir.display()));
        }
    }
    let files = render(s);
    for (path, text) in &files {
        let to = s.dir.join(path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), e))?;
        }
        std::fs::write(&to, text).map_err(|e| format!("{}: {}", to.display(), e))?;
        eprintln!("wrote {}", to.display());
    }
    eprintln!("\n{} is a {}: cd {} && meow build", s.name, s.template.name, shown(&s.dir));
    Ok(())
}

fn shown(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(template: &str, name: &str) -> Spec {
        parse_args(&[template.to_string(), name.to_string(), "--dir".to_string(), "x".to_string(), "--maintainer".to_string(), "me".to_string()]).unwrap().unwrap()
    }

    #[test]
    fn names_and_ids_are_derived() {
        assert_eq!(class_prefix("my-tweak"), "Mytweak");
        assert_eq!(class_prefix("9lives"), "X9lives");
        assert_eq!(default_id("My-Tweak"), "com.example.mytweak");
        assert!(parse_args(&["nope".into(), "a".into()]).unwrap_err().contains("no template"));
        assert!(parse_args(&["tweak".into(), "bad name".into()]).unwrap_err().contains("bad name"));
        assert!(parse_args(&["tweak".into(), "a".into(), "--id".into(), "Com.X".into()]).unwrap_err().contains("bad package id"));
        assert!(parse_args(&[]).unwrap().is_none());
    }

    #[test]
    fn no_placeholder_is_left_and_every_project_expands() {
        for t in TEMPLATES {
            let s = spec(t.name, "demo-kit");
            let files = render(&s);
            for (path, text) in &files {
                assert!(!path.contains('@') && !text.contains("@NAME@") && !text.contains("@ID@") && !text.contains("@PREFIX@") && !text.contains("@MAINTAINER@"), "{}/{}", t.name, path);
            }
            // the project must expand, with the files it names in place
            let d = std::env::temp_dir().join(format!("meow-new-{}-{}", t.name, std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            for (path, text) in &files {
                let to = d.join(path);
                std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                std::fs::write(to, text).unwrap();
            }
            let text = std::fs::read_to_string(d.join("meow.toml")).unwrap();
            let mut cfg: crate::config::Config = toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {}", t.name, e));
            crate::kinds::expand(&mut cfg, &d).unwrap_or_else(|e| panic!("{}: {}", t.name, e));
            crate::config::validate_for_test(&cfg).unwrap_or_else(|e| panic!("{}: {}", t.name, e));
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn an_existing_directory_is_never_overwritten() {
        let d = std::env::temp_dir().join(format!("meow-new-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("mine.txt"), "keep").unwrap();
        let mut s = spec("tool", "demo");
        s.dir = d.clone();
        assert!(run(&s).unwrap_err().contains("not empty"));
        assert_eq!(std::fs::read_to_string(d.join("mine.txt")).unwrap(), "keep");
        std::fs::remove_file(d.join("mine.txt")).unwrap();
        run(&s).unwrap();
        assert!(d.join("meow.toml").is_file() && d.join("src/main.m").is_file());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
