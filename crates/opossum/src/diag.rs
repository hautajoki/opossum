//! Every diagnostic a build reports, as one owned shape with a stable dotted code: `route.*`, `tree.*`, `scan.*`
//! (routing), `place.*` (the place reader and mounts), `ts.<number>` (TypeScript), `lower.*` and `macro.*`
//! (lowering), `luau.gate` (the O2 gate). Owned, so it outlives the program whose checker made it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_core::P;
use tsrs_diagnostics::Category;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

#[derive(Clone, Debug)]
pub struct Diag {
    pub code: String,
    pub severity: Severity,
    pub file: Option<PathBuf>,
    /// 1-based; 0 when the diagnostic has no position in the file.
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl Diag {
    pub fn new(code: &str, severity: Severity, file: Option<&Path>, message: String) -> Diag {
        Diag { code: code.to_owned(), severity, file: file.map(Path::to_path_buf), line: 0, col: 0, message }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// `file:line:col code message`, the file relative to `root`.
    pub fn human(&self, root: &Path) -> String {
        let mut out = String::new();
        match &self.file {
            Some(file) => out.push_str(&file.strip_prefix(root).unwrap_or(file).display().to_string()),
            None => out.push_str("opossum"),
        }
        if self.line > 0 {
            let _ = write!(out, ":{}:{}", self.line, self.col);
        }
        let level = match self.severity {
            Severity::Error => "",
            Severity::Warning => " warning:",
            Severity::Note => " note:",
        };
        let _ = write!(out, " {}{level} {}", self.code, self.message.replace('\n', "\n    "));
        out
    }

    pub fn json(&self, root: &Path) -> serde_json::Value {
        let file = self.file.as_ref().map(|f| f.strip_prefix(root).unwrap_or(f).display().to_string());
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        };
        serde_json::json!({
            "code": self.code,
            "severity": severity,
            "file": file,
            "line": self.line,
            "col": self.col,
            "message": self.message,
        })
    }
}

/// A tsrs diagnostic: TypeScript's become `ts.<code>`, lowering's (code -1) carry their dotted code at the start of
/// the message. `file_of` turns a program file name back into the project's path.
pub fn from_ts(d: P<Diagnostic>, file_of: &dyn Fn(&str) -> PathBuf) -> Diag {
    let mut text = d.localize();
    flatten_chain(d, 1, &mut text);
    let (code, message) = match text.split_once(": ") {
        _ if d.code() > 0 => (format!("ts.{}", d.code()), text),
        Some((code, rest)) if code.contains('.') && !code.contains(char::is_whitespace) => {
            (code.to_owned(), rest.to_owned())
        }
        _ => ("lower".to_owned(), text),
    };
    let severity = match d.category() {
        Category::Error => Severity::Error,
        Category::Warning => Severity::Warning,
        _ => Severity::Note,
    };
    let file = d.file();
    let (line, col) = match (file, d.pos()) {
        (Some(f), Some(pos)) => line_col(f, pos),
        _ => (0, 0),
    };
    Diag { code, severity, file: file.map(|f| file_of(f.file_name())), line, col, message }
}

fn flatten_chain(d: P<Diagnostic>, depth: usize, out: &mut String) {
    for &next in d.message_chain() {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
        out.push_str(&next.localize());
        flatten_chain(next, depth + 1, out);
    }
}

/// 1-based line and column (in bytes) of `pos`.
pub fn line_col(file: P<SourceFile>, pos: u32) -> (u32, u32) {
    let starts = file.ecma_line_map();
    let line = starts.partition_point(|&s| s <= pos).max(1);
    (line as u32, pos - starts[line - 1] + 1)
}

pub fn from_route(d: &route::Diag) -> Diag {
    let severity = if d.code.is_error() { Severity::Error } else { Severity::Warning };
    Diag::new(d.code.as_str(), severity, Some(&d.path), d.message.clone())
}

pub fn from_place(d: &place::Diag) -> Diag {
    let severity = match d.severity {
        place::Severity::Error => Severity::Error,
        place::Severity::Warning => Severity::Warning,
        place::Severity::Note => Severity::Note,
    };
    Diag::new(d.code, severity, Some(&d.path), d.message.clone())
}
