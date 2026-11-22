// `meow logos [options] FILE` and `meow logos logify [options] FILE...`

use super::logify::Logify;
use super::{process, Config, Failure};
use std::io::{Read, Write};

pub const USAGE: &str = "usage: meow logos [-o OUT] [-c key=value]... FILE
       meow logos logify [-i LIST | -e LIST] [FILE]...

  -c generator=[MobileSubstrate|libhooker|internal]   backend, default MobileSubstrate
  -c warnings=[default|error|none]
  -o OUT    write the result to OUT, the directories are created, instead of standard output
  logify    turn the @interface of a header into a logos file that logs every call
  -i LIST   only these methods, comma separated, like launchedTaskWithLaunchPath:arguments:,arguments
  -e LIST   all methods but these";

fn read_utf8(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("could not read {}: {}", path, e))?;
    String::from_utf8(bytes).map_err(|_| format!("{}: the file is not valid utf-8", path))
}

fn write_stdout(text: &str) -> Result<(), String> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    lock.write_all(text.as_bytes()).and_then(|_| lock.flush()).map_err(|e| format!("could not write the output: {}", e))
}

fn write_file(path: &str, text: &str) -> Result<(), String> {
    if let Some(dir) = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {}", path, e))
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) == Some("logify") {
        return run_logify(&args[1..]);
    }
    let mut config = Config::default();
    let mut file: Option<&str> = None;
    let mut out_path: Option<&str> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", USAGE);
                return Ok(());
            }
            "-o" => out_path = Some(it.next().ok_or("-o needs a path")?),
            "-c" | "--config" => {
                let v = it.next().ok_or("-c needs key=value")?;
                match v.split_once('=') {
                    Some(("generator", g)) => config.generator = g.to_string(),
                    Some(("warnings", w)) => config.warnings = w.to_string(),
                    Some(_) => {}
                    None => return Err(format!("-c needs key=value, got {}", v)),
                }
            }
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {}\n{}", s, USAGE)),
            _ if file.is_none() => file = Some(a),
            _ => return Err(format!("only one file is read at a time\n{}", USAGE)),
        }
    }
    let file = file.ok_or(USAGE)?;
    let text = read_utf8(file)?;
    match process(&text, file, &config) {
        Ok(out) => {
            for d in &out.diagnostics {
                eprintln!("{}", d);
            }
            match out_path {
                Some(path) => write_file(path, &out.text),
                None => write_stdout(&out.text),
            }
        }
        Err(Failure { diagnostics, fatal }) => {
            for d in &diagnostics {
                eprintln!("{}", d);
            }
            Err(fatal.unwrap_or_else(|| "warnings are treated as errors".to_string()))
        }
    }
}

fn run_logify(args: &[String]) -> Result<(), String> {
    let (mut include, mut exclude): (Option<&str>, Option<&str>) = (None, None);
    let mut files: Vec<&str> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", USAGE);
                return Ok(());
            }
            "-i" | "--include" => include = Some(it.next().ok_or("-i needs a list")?),
            "-e" | "--exclude" => exclude = Some(it.next().ok_or("-e needs a list")?),
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {}\n{}", s, USAGE)),
            _ => files.push(a),
        }
    }
    let mut logify = Logify::new(include, exclude)?;
    let mut out = String::new();
    if files.is_empty() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("could not read standard input: {}", e))?;
        out.push_str(&logify.feed(&text).map_err(|(line, e)| format!("-:{}: {}", line, e))?);
    }
    for f in files {
        let text = read_utf8(f)?;
        out.push_str(&logify.feed(&text).map_err(|(line, e)| format!("{}:{}: {}", f, line, e))?);
    }
    write_stdout(&out)
}
