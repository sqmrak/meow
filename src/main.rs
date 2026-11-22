mod build;
mod media;
mod package;
mod plist;
mod project;
mod tools;
mod ui;

use build::{cache, depfile, plan, sched, state};
use media::{image, imgjob, png, svg};
use package::{deflate, md5, pack, sha, tar, zip};
use project::{config, ios, kinds};
use tools::{fetch, install, macho, sdkthin, toolchain};

use config::{err, Error, Result};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: meow [-C dir] [-f file] [-V] <command> [args]

commands:
  build [name]... [-j N] [-v] [--no-cache] [--slice S]...   build targets, fat outputs or steps,
                                               no name builds `default` from the config
  plan  [name]... [--slice S]...               print the commands and what is stale
  clean [name]...                              remove build/ outputs and records of the names
  list                                         show everything the config defines
  init                                         write a meow.toml for the C sources in src/
  doctor                                       show the toolchain, sdks and host tools meow found
  pack --out F.deb ...                         build a deb, meow runs this itself for [deb.*]
  sdk-thin --arch A --from SDK --to DIR        copy a sdk with one slice per fat file, for old sdks on 16k-page hosts
  ios [VERSION]                                which cpu slices run on which ios release
  fetch --name N (--url U --sha256 H | --git U --rev R) --cache D --to D   pinned download, meow runs this for [dep.*]
  check-version --expect V (--plist F K | --field F K)...
                                               fail when a file carries another version
  verify-macho FILE [--slices a,b] [--min-ios V] [--signed] [--allow-dylib P]...
                                               release checks on a binary, meow runs this for [verify.*]
  why [name]...                                say why each step of the names is stale
  compdb [name]...                             write compile_commands.json for the compile steps
  test [name]...                               build and run the host tests, same as `build test`
  cache [stats|gc|clear]                       the shared object cache, `build --no-cache` or MEOW_NO_CACHE=1 skips it
  install --host H [--user root] [--port 22] [--password-env VAR] [--legacy] F.deb
                                               copy a deb to a device and run dpkg -i, the password comes from VAR
  verify-sign FILE [--ldid L] [--entitlements P]
                                               check every slice of FILE is signed
  plist [--to xml|binary|json] [-o OUT | --in-place] FILE...   convert property lists, --get PATH FILE prints a value";

struct Cli {
    dir: Option<PathBuf>,
    file: String,
    command: String,
    names: Vec<String>,
    slices: Vec<String>,
    jobs: Option<usize>,
    verbose: bool,
    no_cache: bool,
}

fn parse_args(args: Vec<String>) -> Result<Cli> {
    let mut cli = Cli {
        dir: None,
        file: "meow.toml".to_string(),
        command: String::new(),
        names: Vec::new(),
        slices: Vec::new(),
        jobs: None,
        verbose: false,
        no_cache: false,
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| -> Result<String> {
            it.next().ok_or_else(|| Error(format!("{} needs a value", flag)))
        };
        match a.as_str() {
            "-C" => cli.dir = Some(PathBuf::from(value("-C")?)),
            "-f" => cli.file = value("-f")?,
            "-j" => {
                let v = value("-j")?;
                let n: usize = v.parse().map_err(|_| Error(format!("bad job count {}", v)))?;
                if n == 0 {
                    return err("job count must be at least 1");
                }
                cli.jobs = Some(n);
            }
            "--slice" => cli.slices.push(value("--slice")?),
            "-v" => cli.verbose = true,
            "--no-cache" => cli.no_cache = true,
            "-h" | "--help" => return err(USAGE),
            "-V" | "--version" => {
                println!("meow v{}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            s if s.starts_with('-') => return err(format!("unknown option {}\n{}", s, USAGE)),
            _ => {
                if cli.command.is_empty() {
                    cli.command = a;
                } else {
                    cli.names.push(a);
                }
            }
        }
    }
    if cli.command.is_empty() {
        return err(USAGE);
    }
    Ok(cli)
}

fn find_config(start: &Path, file: &str) -> Result<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let cand = dir.join(file);
        if cand.is_file() {
            return Ok(cand);
        }
        if !dir.pop() {
            return err(format!("no {} in {} or any parent", file, start.display()));
        }
    }
}

const HEADER: &str = "# make everything outperform without waiting\n# by sqmrak :3\n\n";

// a host build of src/*.c, the smallest config that loads and builds
fn run_init(dir: &Path, file: &str) -> Result<()> {
    let path = dir.join(file);
    if path.exists() {
        return err(format!("{} already exists", path.display()));
    }
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or_else(|| "app".to_string());
    let body = format!(
        "default = [\"{n}\"]\n\n[slice.host]\ncc = \"cc\"\nflags = []\n\n[target.{n}]\nslices = [\"host\"]\noutput = \"build/{n}\"\ncflags = [\"-O2\", \"-Wall\"]\n[[target.{n}.src]]\nfiles = [\"src/*.c\"]\n",
        n = name
    );
    std::fs::write(&path, format!("{}{}", HEADER, body))
        .map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

// commands that take their own arguments and need no meow.toml
fn run_tool_command(args: &[String]) -> Option<Result<()>> {
    let rest = &args[1..];
    match args.first().map(String::as_str)? {
        "doctor" => {
            let color = std::io::IsTerminal::is_terminal(&std::io::stdout()) && std::env::var_os("NO_COLOR").is_none();
            Some(toolchain::doctor(color).map_err(Error))
        }
        "pack" => Some(pack::parse_args(rest).and_then(|spec| pack::run(&spec, Path::new("."))).map_err(Error)),
        "verify-sign" => Some(run_verify_sign(rest)),
        "sdk-thin" => Some(run_sdk_thin(rest)),
        "fetch" => Some(fetch::parse_args(rest).and_then(|spec| fetch::run_fetch(&spec)).map_err(Error)),
        "check-version" => Some(run_check_version(rest)),
        "verify-macho" => Some(run_verify_macho(rest)),
        "plist" => Some(plist::run(rest).map_err(Error)),
        "ios" => Some(run_ios(rest)),
        "cache" => Some(run_cache(rest)),
        "install" => Some(install::parse_args(rest).and_then(|spec| install::run_install(&spec)).map_err(Error)),
        "write" => Some(run_write(rest)),
        "img" => Some(imgjob::parse_args(rest).and_then(|jobs| imgjob::run(&jobs)).map(|n| eprintln!("wrote {} images", n)).map_err(Error)),
        _ => None,
    }
}

fn run_check_version(args: &[String]) -> Result<()> {
    let mut expect = None;
    let mut checks: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--expect" => {
                i += 1;
                expect = args.get(i).cloned();
            }
            "--plist" | "--field" => {
                let (f, k) = (args.get(i + 1), args.get(i + 2));
                let (Some(f), Some(k)) = (f, k) else { return err(format!("{} needs FILE KEY", args[i])) };
                checks.push((f.clone(), k.clone()));
                i += 2;
            }
            other => return err(format!("check-version: unknown argument {}", other)),
        }
        i += 1;
    }
    let expect = expect.ok_or_else(|| Error("check-version needs --expect".to_string()))?;
    let mut bad = Vec::new();
    for (f, k) in &checks {
        match kinds::read_key(Path::new("."), f, k) {
            Ok(v) if v == expect => {}
            Ok(v) => bad.push(format!("{} {} is {}", f, k, v)),
            Err(e) => bad.push(e.0),
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        err(format!("version {} expected, but {}", expect, bad.join(", ")))
    }
}

fn run_verify_macho(args: &[String]) -> Result<()> {
    let mut file = None;
    let mut e = macho::Expect::default();
    let mut min = None;
    let (mut nm, mut symbols, mut symbols_file) = (None, Vec::new(), None);
    let mut required: Vec<String> = Vec::new();
    let mut i = 0;
    let val = |i: &mut usize| -> Result<String> {
        *i += 1;
        args.get(*i).cloned().ok_or_else(|| Error(format!("{} needs a value", args[*i - 1])))
    };
    while i < args.len() {
        match args[i].as_str() {
            "--slices" => e.slices = val(&mut i)?.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
            "--min-ios" => min = Some(val(&mut i)?),
            "--signed" => e.signed = true,
            "--allow-dylib" => e.allow_dylibs.push(val(&mut i)?),
            "--forbid-dylib" => e.forbid_dylibs.push(val(&mut i)?),
            "--nm" => nm = Some(val(&mut i)?),
            "--forbid-symbol" => symbols.push(val(&mut i)?),
            "--require-symbol" => required.push(val(&mut i)?),
            "--forbid-symbols-file" => symbols_file = Some(val(&mut i)?),
            s if s.starts_with('-') => return err(format!("verify-macho: unknown option {}", s)),
            s if file.is_none() => file = Some(s.to_string()),
            _ => return err("verify-macho takes one file"),
        }
        i += 1;
    }
    let file = file.ok_or_else(|| Error("verify-macho needs a file".to_string()))?;
    if let Some(m) = min {
        let v = ios::parse(&m).ok_or_else(|| Error(format!("bad --min-ios {:?}", m)))?;
        e.max_min_os = ios::ARCHS.iter().map(|a| (a.name.to_string(), ios::slice_min(a.name, v))).collect();
    }
    let bytes = std::fs::read(&file).map_err(|x| Error(format!("{}: {}", file, x)))?;
    macho::check_release(&file, &bytes, &e).map_err(Error)?;
    if let Some(f) = symbols_file {
        let text = std::fs::read_to_string(&f).map_err(|x| Error(format!("{}: {}", f, x)))?;
        symbols.extend(text.lines().map(|l| l.split('#').next().unwrap_or("").trim().to_string()).filter(|l| !l.is_empty()));
    }
    if !symbols.is_empty() || !required.is_empty() {
        let nm = nm.ok_or_else(|| Error("symbol checks need --nm".to_string()))?;
        if !symbols.is_empty() {
            macho::check_symbols(&file, &bytes, &nm, &symbols).map_err(Error)?;
        }
        if !required.is_empty() {
            macho::check_defined(&file, &bytes, &nm, &required).map_err(Error)?;
        }
    }
    Ok(())
}

fn run_write(args: &[String]) -> Result<()> {
    let (mut to, mut mode, mut content) = (None, "644".to_string(), None);
    let mut i = 0;
    while i < args.len() {
        let v = |i: &mut usize| -> Result<String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| Error(format!("{} needs a value", args[*i - 1])))
        };
        match args[i].as_str() {
            "--to" => to = Some(v(&mut i)?),
            "--mode" => mode = v(&mut i)?,
            "--content" => content = Some(v(&mut i)?),
            other => return err(format!("write: unknown argument {}", other)),
        }
        i += 1;
    }
    let (Some(to), Some(content)) = (to, content) else { return err("write needs --to and --content") };
    let mode = u32::from_str_radix(&mode, 8).map_err(|_| Error(format!("bad mode {}", mode)))?;
    if let Some(dir) = Path::new(&to).parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error(format!("{}: {}", dir.display(), e)))?;
    }
    let tmp = format!("{}.tmp", to);
    std::fs::write(&tmp, content).map_err(|e| Error(format!("{}: {}", tmp, e)))?;
    // windows files carry no mode bits, so the mode has nothing to apply to there
    #[cfg(unix)]
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(mode)).map_err(|e| Error(format!("{}: {}", tmp, e)))?;
    #[cfg(not(unix))]
    let _ = mode;
    std::fs::rename(&tmp, &to).map_err(|e| Error(format!("{}: {}", to, e)))
}

fn run_cache(args: &[String]) -> Result<()> {
    let Some(c) = cache::Cache::open().or_else(|| cache::default_dir().map(|d| cache::Cache::at(d.join("obj")))) else {
        return err("no cache directory, set MEOW_CACHE");
    };
    match args.first().map(String::as_str) {
        None | Some("stats") => {
            let (n, bytes) = c.stats();
            println!("{} objects, {:.1} MiB, limit {} MiB", n, bytes as f64 / 1048576.0, cache::max_bytes() / 1048576);
            Ok(())
        }
        Some("gc") => {
            let (removed, kept) = c.gc(cache::max_bytes());
            println!("removed {}, kept {:.1} MiB", removed, kept as f64 / 1048576.0);
            Ok(())
        }
        Some("clear") => c.clear().map_err(|e| Error(e.to_string())),
        Some(other) => err(format!("cache: unknown subcommand {}, use stats, gc or clear", other)),
    }
}

fn run_ios(args: &[String]) -> Result<()> {
    match args {
        [] => {
            println!("release  slices that run");
            for (v, archs) in ios::table() {
                println!("ios {:<5}  {}", ios::text(v), archs.join(" "));
            }
            println!();
            for a in &ios::ARCHS {
                let last = a.last.map_or("current".to_string(), ios::text);
                println!("{:<7} ios {} to {}  {}", a.name, ios::text(a.first), last, a.devices);
            }
            Ok(())
        }
        [v] => {
            let ver = ios::parse(v).ok_or_else(|| Error(format!("bad version {:?}, use 5.1", v)))?;
            let archs = ios::archs_running(ver);
            if archs.is_empty() {
                return err(format!("no slice runs on ios {}", ios::text(ver)));
            }
            println!("ios {}: {}", ios::text(ver), archs.join(" "));
            Ok(())
        }
        _ => err("usage: meow ios [VERSION]"),
    }
}

fn run_sdk_thin(args: &[String]) -> Result<()> {
    let (mut arch, mut from, mut to) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = |it: &mut std::slice::Iter<String>| it.next().cloned().ok_or_else(|| Error(format!("{} needs a value", a)));
        match a.as_str() {
            "--arch" => arch = Some(v(&mut it)?),
            "--from" => from = Some(v(&mut it)?),
            "--to" => to = Some(v(&mut it)?),
            other => return err(format!("sdk-thin: unknown argument {}", other)),
        }
    }
    let (Some(arch), Some(from), Some(to)) = (arch, from, to) else {
        return err("usage: meow sdk-thin --arch armv6 --from SDK --to DIR");
    };
    let st = sdkthin::run(Path::new(&from), Path::new(&to), &arch).map_err(Error)?;
    eprintln!("thinned {} fat files, linked {} others into {}", st.thinned, st.linked, to);
    Ok(())
}

fn run_verify_sign(args: &[String]) -> Result<()> {
    let mut file = None;
    let mut ldid = None;
    let mut ents = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ldid" => ldid = Some(it.next().ok_or_else(|| Error("--ldid needs a value".to_string()))?.as_str()),
            "--entitlements" => ents = Some(it.next().ok_or_else(|| Error("--entitlements needs a value".to_string()))?.as_str()),
            s if s.starts_with('-') => return err(format!("unknown option {}", s)),
            s if file.is_none() => file = Some(s),
            _ => return err("verify-sign takes one file"),
        }
    }
    let file = file.ok_or_else(|| Error("verify-sign needs a file".to_string()))?;
    macho::verify(file, ldid, ents).map(|_| ()).map_err(Error)
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn run() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if let Some(r) = run_tool_command(&raw) {
        return r;
    }
    let mut cli = parse_args(raw)?;
    if cli.command == "test" {
        cli.command = "build".to_string();
        if cli.names.is_empty() {
            cli.names.push("test".to_string());
        }
    }
    let start = match &cli.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir().map_err(|e| Error(e.to_string()))?,
    };
    if cli.command == "init" {
        return run_init(&start, &cli.file);
    }
    let cfg_path = find_config(&start, &cli.file)?;
    let root = cfg_path.parent().unwrap().to_path_buf();
    let cfg = config::load(&cfg_path)?;
    let store = state::Store::new(root.join("build/.meow"));

    match cli.command.as_str() {
        "list" => {
            println!("slices: {}", cfg.slice.keys().cloned().collect::<Vec<_>>().join(" "));
            for (n, t) in &cfg.target {
                println!("target {} [{}]", n, t.slices.join(" "));
            }
            for (n, f) in &cfg.fat {
                println!("fat {} <- {}", n, f.from);
            }
            for n in cfg.step.keys() {
                println!("step {}", n);
            }
            for n in cfg.deb.keys() {
                println!("deb {}", n);
            }
            println!("default: {}", cfg.default.join(" "));
            Ok(())
        }
        "plan" | "build" | "clean" | "why" | "compdb" => {
            let names = if cli.names.is_empty() {
                cfg.default.clone()
            } else {
                cli.names.clone()
            };
            if names.is_empty() {
                return err(format!("{} needs a name or a default in the config\n{}", cli.command, USAGE));
            }
            let plan = plan::build(&cfg, &root, &names, &cli.slices)?;
            match cli.command.as_str() {
                "plan" => {
                    let fresh = sched::fresh_map(&plan, &store);
                    for (n, f) in plan.nodes.iter().zip(fresh) {
                        println!("{} {}", if f { "fresh" } else { "stale" }, n.label);
                        println!("    {}", plan::render(&n.argv));
                        for p in &n.post {
                            println!("    {}", plan::render(p));
                        }
                    }
                    Ok(())
                }
                "why" => {
                    let fresh = sched::fresh_map(&plan, &store);
                    let mut stale = 0;
                    for (i, n) in plan.nodes.iter().enumerate() {
                        if fresh[i] {
                            continue;
                        }
                        stale += 1;
                        let reason = sched::why(&plan, i, &fresh, &store);
                        println!("{}\n    {}", n.label, reason);
                    }
                    if stale == 0 {
                        println!("everything is fresh");
                    }
                    Ok(())
                }
                "compdb" => {
                    let mut entries = Vec::new();
                    for n in &plan.nodes {
                        let Some(at) = n.argv.iter().position(|a| a == "-c") else { continue };
                        let Some(file) = n.argv.get(at + 1) else { continue };
                        let args: Vec<String> = n.argv.iter().map(|a| json_string(a)).collect();
                        entries.push(format!(
                            "  {{\"directory\": {}, \"file\": {}, \"arguments\": [{}]}}",
                            json_string(&root.to_string_lossy()),
                            json_string(file),
                            args.join(", ")
                        ));
                    }
                    if entries.is_empty() {
                        return err("no compile steps in the plan");
                    }
                    let path = root.join("compile_commands.json");
                    std::fs::write(&path, format!("[\n{}\n]\n", entries.join(",\n"))).map_err(|e| Error(format!("{}: {}", path.display(), e)))?;
                    eprintln!("wrote {} ({} entries)", path.display(), entries.len());
                    Ok(())
                }
                "clean" => {
                    // dependency caches and release archives live outside build/ and stay
                    let build_dir = root.join("build");
                    for n in &plan.nodes {
                        for o in n.outputs.iter().chain(n.depfile.iter()) {
                            if !root.join(o).starts_with(&build_dir) {
                                continue;
                            }
                            match std::fs::remove_file(root.join(o)) {
                                Ok(()) => {}
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                Err(e) => return err(format!("{}: {}", o, e)),
                            }
                        }
                        store.forget(&n.outputs[0]);
                    }
                    Ok(())
                }
                _ => {
                    let jobs = cli.jobs.unwrap_or_else(|| {
                        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
                    });
                    ui::Ui::new(plan.nodes.len()).banner(&names, jobs, plan.nodes.len());
                    sched::run(&plan, &store, &sched::Options { jobs, verbose: cli.verbose, cache: if cli.no_cache { None } else { cache::Cache::open() } })
                        .map(|_| ())
                }
            }
        }
        other => err(format!("unknown command {}\n{}", other, USAGE)),
    }
}

extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
}

fn main() -> ExitCode {
    // rust ignores SIGPIPE, so `meow plan | head` would panic in println
    // SAFETY: 13 is SIGPIPE and 0 is SIG_DFL, set before any thread starts
    unsafe {
        signal(13, 0);
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let tty = std::io::IsTerminal::is_terminal(&std::io::stderr()) && std::env::var_os("NO_COLOR").is_none();
            if tty {
                eprintln!("\x1b[1;38;5;203mmeow:\x1b[0m {}", e);
            } else {
                eprintln!("meow: {}", e);
            }
            ExitCode::FAILURE
        }
    }
}
