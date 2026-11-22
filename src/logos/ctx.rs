// diagnostics and the physical line to source line mapping

use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Config {
    pub generator: String,
    pub warnings: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            generator: "MobileSubstrate".to_string(),
            warnings: "default".to_string(),
        }
    }
}

/// a fatal diagnostic, already formatted as `file:line: error: reason`
#[derive(Debug)]
pub struct Fatal(pub String);

pub type R<T> = Result<T, Fatal>;

/// why a run produced no output
#[derive(Debug)]
pub struct Failure {
    /// warnings printed before the failure, in order
    pub diagnostics: Vec<String>,
    /// the error, or none when warnings are treated as errors
    pub fatal: Option<String>,
}

pub struct Ctx {
    pub filename: String,
    pub preprocessed: bool,
    pub line_map: BTreeMap<usize, (String, usize)>,
    pub diagnostics: Vec<String>,
    pub warning_count: usize,
    pub config: Config,
}

impl Ctx {
    /// source file and line of a zero based physical line
    pub fn lookup(&self, fileline: i64) -> (String, i64) {
        let fl = fileline + 1;
        for (&key, (file, line)) in self.line_map.iter().rev() {
            if fl >= key as i64 {
                return (file.clone(), *line as i64 + (fl - key as i64));
            }
        }
        (self.filename.clone(), fl)
    }

    pub fn line_directive(&self, physline: usize) -> String {
        let (file, line) = self.lookup(physline as i64);
        format!("{} {} \"{}\"", if self.preprocessed { "#" } else { "#line" }, line, file)
    }

    pub fn describe_line(&self, physline: usize) -> String {
        let (file, line) = self.lookup(physline as i64);
        format!("{}:{}", file, line)
    }

    pub fn warn(&mut self, curline: i64, reason: &str) {
        if self.config.warnings == "error" {
            if self.warning_count == 0 {
                self.diagnostics.push("logos: warnings being treated as errors".to_string());
            }
        } else if self.config.warnings == "none" {
            self.warning_count += 1;
            return;
        }
        let (file, line) = self.lookup(curline);
        let at = if curline > -1 { format!("{}:", line) } else { String::new() };
        self.diagnostics.push(format!("{}:{} warning: {}", file, at, reason));
        self.warning_count += 1;
    }

    pub fn err(&self, curline: i64, reason: &str) -> Fatal {
        let (file, line) = self.lookup(curline);
        let at = if curline > -1 { format!("{}:", line) } else { String::new() };
        Fatal(format!("{}:{} error: {}", file, at, reason))
    }
}
