// a patch replaces a range of one line, or inserts lines before it

use crate::logos::ctx::Ctx;
use crate::logos::emit::generator::Gen;

pub enum Source {
    /// nothing, the directive only disappears
    None,
    Text(String),
    List(Vec<Source>),
    Gen(Call),
}

/// a generator call whose output depends on the finished model
pub enum Call {
    MethodDefinition(usize),
    MethodOriginalCall(usize, Option<String>),
    MethodOriginalName(usize),
    MethodLog(usize, Option<String>),
    FunctionDeclaration(usize),
    FunctionOriginalCall(usize, Option<String>),
    FunctionOriginalName(usize),
    PropertyDefinition(usize),
    GroupDeclarations(usize),
    GroupInitializers(usize),
    StaticClassDeclarations,
    ClassReference(String, u8),
    Preamble,
    StaticDeclarations,
    ClassList(Vec<String>),
}

pub struct Patch {
    pub line: usize,
    pub range: Option<(usize, usize)>,
    pub source: Source,
    pub squash: bool,
}

impl Patch {
    pub fn at(line: usize, start: usize, end: usize, source: Source) -> Patch {
        Patch { line, range: Some((start, end)), source, squash: false }
    }

    pub fn insert(line: usize, source: Source) -> Patch {
        Patch { line, range: None, source, squash: false }
    }

    /// patches on one line run from the last range to the first, insertions come after ranges
    pub fn sort_key(&self) -> (usize, i64) {
        let start = match self.range {
            Some((s, _)) if s > 0 => s as i64,
            _ => -1,
        };
        (self.line, start)
    }
}

fn eval(source: &Source, gen: &Gen, out: &mut Vec<String>) -> Result<(), String> {
    match source {
        Source::None => {}
        Source::Text(t) => out.push(t.clone()),
        Source::List(items) => {
            for i in items {
                eval(i, gen, out)?;
            }
        }
        Source::Gen(call) => out.push(match call {
            Call::MethodDefinition(m) => gen.method_definition(*m)?,
            Call::MethodOriginalCall(m, a) => gen.method_original_call(*m, a.as_deref()),
            Call::MethodOriginalName(m) => gen.method_original_name(*m),
            Call::MethodLog(m, a) => gen.build_log_call(*m, a.as_deref())?,
            Call::FunctionDeclaration(f) => gen.function_declaration(*f),
            Call::FunctionOriginalCall(f, a) => gen.function_original_call(*f, a.as_deref()),
            Call::FunctionOriginalName(f) => gen.function_original_name(*f),
            Call::PropertyDefinition(p) => gen.property_definition(*p),
            Call::GroupDeclarations(g) => gen.group_declarations(*g)?,
            Call::GroupInitializers(g) => gen.group_initializers(*g)?,
            Call::StaticClassDeclarations => gen.static_class_declarations(),
            Call::ClassReference(c, s) => gen.class_reference(c, *s),
            Call::Preamble => gen.preamble(),
            Call::StaticDeclarations => gen.static_declarations(),
            Call::ClassList(names) => gen.class_list(names),
        }),
    }
    Ok(())
}

/// `patches` must already be sorted
pub fn apply(patches: &[Patch], lines: &mut Vec<String>, gen: &Gen, ctx: &Ctx) -> Result<(), String> {
    for p in patches {
        let mut pieces = Vec::new();
        eval(&p.source, gen, &mut pieces)?;
        match p.range {
            None => {
                pieces.push(ctx.line_directive(p.line));
                if p.squash {
                    lines.push(pieces.concat());
                } else {
                    let at = p.line.min(lines.len());
                    lines.splice(at..at, pieces);
                }
            }
            Some((start, end)) => {
                let text = pieces.concat();
                let line = lines.get_mut(p.line).ok_or_else(|| "patch outside of the file".to_string())?;
                let end = end.min(line.len());
                if start > end || !line.is_char_boundary(start) || !line.is_char_boundary(end) {
                    return Err("patch range does not fit its line".to_string());
                }
                line.replace_range(start..end, &text);
            }
        }
    }
    Ok(())
}
