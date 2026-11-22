// `meow plist [--to xml|binary|json] [-o OUT | --in-place] FILE...` and `meow plist --get PATH FILE`

use super::{parse, write, Format, Value};
use std::io::{Read, Write};

pub const USAGE: &str = "usage: meow plist [--to xml|binary|json] [-o OUT | --in-place] FILE...
       meow plist --get KEY.PATH FILE

  reads xml, binary and old style text plists, - is standard input or output
  --to F        output format, xml when left out. json fails on dates and data
  -o OUT        write the one input file to OUT
  --in-place    rewrite every input file in its format
  --get PATH    print one value, dict keys and array indexes joined with dots: Filter.Bundles.0
                a string, number or boolean prints as text, a dict or array as an xml plist";

fn read_input(path: &str) -> Result<Vec<u8>, String> {
    if path == "-" {
        let mut b = Vec::new();
        std::io::stdin().read_to_end(&mut b).map_err(|e| format!("standard input: {}", e))?;
        return Ok(b);
    }
    std::fs::read(path).map_err(|e| format!("{}: {}", path, e))
}

fn write_output(path: &str, bytes: &[u8]) -> Result<(), String> {
    if path == "-" {
        let out = std::io::stdout();
        let mut l = out.lock();
        return l.write_all(bytes).and_then(|_| l.flush()).map_err(|e| format!("standard output: {}", e));
    }
    // a temporary file in the same directory, so an interrupted write leaves the old file
    let tmp = format!("{}.meow-tmp", path);
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {}", tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {}", path, e)
    })
}

fn format_of(bytes: &[u8]) -> Format {
    if bytes.starts_with(b"bplist00") { Format::Binary } else { Format::Xml }
}

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Int(n) => Some(n.to_string()),
        Value::Real(r) => Some(r.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut to = Format::Xml;
    let mut to_given = false;
    let mut out: Option<&str> = None;
    let mut in_place = false;
    let mut get: Option<&str> = None;
    let mut files: Vec<&str> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", USAGE);
                return Ok(());
            }
            "--to" => {
                to = match it.next().map(String::as_str) {
                    Some("xml") | Some("xml1") => Format::Xml,
                    Some("binary") | Some("binary1") => Format::Binary,
                    Some("json") => Format::Json,
                    other => return Err(format!("--to wants xml, binary or json, got {:?}", other.unwrap_or(""))),
                };
                to_given = true;
            }
            "-o" => out = Some(it.next().ok_or("-o needs a path")?),
            "--in-place" => in_place = true,
            "--get" => get = Some(it.next().ok_or("--get needs a key path")?),
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {}\n{}", s, USAGE)),
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        return Err(USAGE.to_string());
    }

    if let Some(path) = get {
        if files.len() != 1 {
            return Err("--get reads one file".to_string());
        }
        let v = parse(&read_input(files[0])?).map_err(|e| format!("{}: {}", files[0], e))?;
        let found = v.path(path).ok_or_else(|| format!("{}: no value at {}", files[0], path))?;
        match scalar_text(found) {
            Some(t) => println!("{}", t),
            None => {
                let bytes = write(found, to).map_err(|e| format!("{}: {}", files[0], e))?;
                write_output("-", &bytes)?;
            }
        }
        return Ok(());
    }

    if in_place {
        if out.is_some() {
            return Err("--in-place and -o exclude each other".to_string());
        }
        for f in &files {
            if *f == "-" {
                return Err("--in-place needs files".to_string());
            }
            let raw = read_input(f)?;
            let v = parse(&raw).map_err(|e| format!("{}: {}", f, e))?;
            let fmt = if to_given { to } else { format_of(&raw) };
            write_output(f, &write(&v, fmt).map_err(|e| format!("{}: {}", f, e))?)?;
        }
        return Ok(());
    }

    if files.len() > 1 {
        return Err("several files need --in-place".to_string());
    }
    let v = parse(&read_input(files[0])?).map_err(|e| format!("{}: {}", files[0], e))?;
    let bytes = write(&v, to).map_err(|e| format!("{}: {}", files[0], e))?;
    write_output(out.unwrap_or("-"), &bytes)
}
