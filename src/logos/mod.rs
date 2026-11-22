// logos preprocessor for objective-c tweaks, `meow logos`
// `process` turns a `.x` or `.xm` source into plain objective-c, the same job as `logos.pl` from theos

mod cli;
mod ctx;
mod emit;
mod model;
mod syntax;
mod text;

mod logify;

pub use cli::run;
pub use ctx::{Config, Failure};

use ctx::{Ctx, Fatal};
use emit::generator::{Backend, Gen};
use emit::patch::{apply, Call, Patch, Source};
use text::{is_space, skip_space, trim_start_ws};

pub struct Output {
    pub text: String,
    /// warnings in the order they were found, one per line
    pub diagnostics: Vec<String>,
}

enum Stop {
    Fatal(String),
    WarningsAsErrors,
}

impl From<Fatal> for Stop {
    fn from(f: Fatal) -> Stop {
        Stop::Fatal(f.0)
    }
}

pub fn process(source: &str, filename: &str, config: &Config) -> Result<Output, Failure> {
    let read = syntax::reader::read_lines(source, filename);
    let mut ctx = Ctx {
        filename: read.filename.clone(),
        preprocessed: read.preprocessed,
        line_map: read.line_map,
        diagnostics: Vec::new(),
        warning_count: 0,
        config: config.clone(),
    };
    match preprocess(&mut ctx, &read.lines) {
        Ok(text) => Ok(Output { text, diagnostics: ctx.diagnostics }),
        Err(Stop::Fatal(msg)) => Err(Failure { diagnostics: ctx.diagnostics, fatal: Some(msg) }),
        Err(Stop::WarningsAsErrors) => Err(Failure { diagnostics: ctx.diagnostics, fatal: None }),
    }
}

fn depth_at(map: &[(usize, usize, i64)], line: usize, pos: usize) -> i64 {
    map.iter()
        .rev()
        .find(|&&(l, p, _)| (l, p) <= (line, pos))
        .map_or(0, |&(_, _, d)| d)
}

fn is_import_line(s: &str) -> bool {
    let t = trim_start_ws(s);
    for word in ["#import", "@import", "#include"] {
        if let Some(r) = t.strip_prefix(word) {
            if r.bytes().next().map_or(false, is_space) {
                return true;
            }
        }
    }
    false
}

fn is_blank(s: &str) -> bool {
    skip_space(s.as_bytes(), 0) == s.len()
}

fn preprocess(ctx: &mut Ctx, lines: &[String]) -> Result<String, Stop> {
    let parsed = syntax::parser::parse(ctx, lines)?;
    let syntax::parser::Parsed { mut model, mut patches, classes, first_directive, last_init, depth_map } = parsed;
    let total = lines.len();

    let name = ctx.config.generator.clone();
    let Some(backend) = Backend::from_name(&name) else {
        let msg = if name.contains('@') {
            format!("generators loaded from a path are not supported: {}", name)
        } else {
            format!("I can't find the {} Generator!", name)
        };
        return Err(ctx.err(-1, &msg).into());
    };

    let has_preamble = {
        let gen = Gen { m: &model, b: backend };
        ctx.preprocessed || gen.find_preamble(lines)
    };

    if let Some((first_line, first_pos)) = first_directive {
        // splice the declarations in at the nearest blank or import line outside any block
        let (mut line, mut pos) = (first_line, first_pos);
        loop {
            let depth = depth_at(&depth_map, line, pos);
            let mut above = if line == 0 { Some(String::new()) } else { None };
            if ctx.preprocessed {
                let (file, n) = ctx.lookup(line as i64);
                if file == ctx.filename && n == 1 {
                    above = Some(String::new());
                }
            }
            let above = above.unwrap_or_else(|| lines[line - 1].clone());
            if depth == 0 && (is_blank(&above) || is_import_line(&above)) {
                break;
            }
            if pos == 0 {
                line -= 1;
            }
            pos = 0;
        }

        let mut parts = Vec::new();
        if !has_preamble {
            parts.push(Source::Gen(Call::Preamble));
        }
        parts.push(Source::Gen(Call::StaticDeclarations));
        parts.push(Source::Gen(Call::ClassList(classes.clone())));
        parts.push(Source::Gen(Call::GroupDeclarations(0)));
        parts.push(Source::Gen(Call::StaticClassDeclarations));
        patches.push(Patch::insert(line, Source::List(parts)));

        if last_init.is_none() {
            // no %init anywhere, run every group from a generated constructor
            let required: Vec<usize> = (0..model.groups.len()).filter(|&g| model.group_init_required(g)).collect();
            if required.iter().any(|&g| model.groups[g].explicit) {
                return Err(ctx
                    .err(
                        total as i64,
                        "Cannot generate an autoconstructor with multiple %groups. Please explicitly create a constructor.",
                    )
                    .into());
            }
            let source = if required.is_empty() {
                Source::None
            } else {
                let mut parts = vec![Source::Text(
                    "static __attribute__((constructor)) void _logosLocalInit() {\n".to_string(),
                )];
                for &g in &required {
                    parts.push(Source::Gen(Call::GroupInitializers(g)));
                    parts.push(Source::Text(" ".to_string()));
                    model.groups[g].init_line = total as i64;
                    model.groups[g].initialized = true;
                }
                parts.push(Source::Text("}\n".to_string()));
                Source::List(parts)
            };
            patches.push(Patch { line: total, range: None, source, squash: true });
        }
    }

    let uninitialized: Vec<&str> = model
        .groups
        .iter()
        .enumerate()
        .filter(|(g, grp)| !grp.initialized && model.group_init_required(*g))
        .map(|(_, grp)| grp.name.as_str())
        .collect();
    if !uninitialized.is_empty() {
        let msg = format!(
            "non-initialized hook group{}: {}",
            if uninitialized.len() == 1 { "" } else { "s" },
            uninitialized.join(", ")
        );
        return Err(ctx.err(total as i64, &msg).into());
    }

    patches.sort_by_key(|p| std::cmp::Reverse(p.sort_key()));

    if ctx.warning_count > 0 && ctx.config.warnings == "error" {
        return Err(Stop::WarningsAsErrors);
    }

    let mut out: Vec<String> = lines.to_vec();
    let gen = Gen { m: &model, b: backend };
    apply(&patches, &mut out, &gen, ctx).map_err(|e| Stop::from(ctx.err(total as i64, &e)))?;

    if !ctx.preprocessed {
        out.insert(0, ctx.line_directive(0));
    }
    let mut text = String::new();
    for l in out {
        text.push_str(&l);
        text.push('\n');
    }
    Ok(text)
}
