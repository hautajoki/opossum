//! TypeScript 5's `formatDiagnosticsWithColorAndContext`, which roblox-ts's `ts.createDiagnosticReporter(ts.sys,
//! true)` and `formatDiagnostics` print with.
//!
//! rbxts-rs: tsrs's writer (`tsrs_compiler::diagnosticwriter`) is TypeScript 7's, whose layout differs from TypeScript
//! 5's for related information (TypeScript 7: `  file:line:col - message` then the code; TypeScript 5: the location,
//! the code, then the message on a line of its own after a blank line) and for an empty span (TypeScript 7 squiggles
//! the next character, TypeScript 5 nothing). Stock roblox-ts runs on TypeScript 5, so this is a port of TypeScript
//! 5.5's `formatDiagnosticsWithColorAndContext`, `formatCodeSpan` and `formatLocation`, text slicing in UTF-16 units
//! as JavaScript's.

use tsrs_ast::{Diagnostic, SourceFileLike};
use tsrs_compiler::diagnosticwriter::{
    FileLike, diagnostic_file, diagnostic_len, diagnostic_pos, flatten_diagnostic_message,
};
use tsrs_core::P;
use tsrs_core::tspath::{self, ComparePathsOptions};
use tsrs_diagnostics::Category;

const GREY: &str = "\u{1b}[90m";
const RED: &str = "\u{1b}[91m";
const YELLOW: &str = "\u{1b}[93m";
const BLUE: &str = "\u{1b}[94m";
const CYAN: &str = "\u{1b}[96m";
const GUTTER_STYLE: &str = "\u{1b}[7m";
const GUTTER_SEPARATOR: &str = " ";
const RESET: &str = "\u{1b}[0m";
const ELLIPSIS: &str = "...";
const HALF_INDENT: &str = "  ";
const INDENT: &str = "    ";
/// `Diagnostics.File_appears_to_be_binary.code`.
const FILE_APPEARS_TO_BE_BINARY: i32 = 1490;
/// `Diagnostics.The_parser_expected_to_find_a_1_to_match_the_0_token_here.code`.
const THE_PARSER_EXPECTED_TO_FIND: i32 = 1007;

/// How a diagnostic is printed.
pub struct Format<'a> {
    /// The directory locations are relative to (`host.getCurrentDirectory()`).
    pub cwd: &'a str,
    /// Whether `host.getCanonicalFileName` keeps the case (roblox-ts's `formatDiagnostics` host) or lowers it
    /// (`ts.sys` on Windows, `createDiagnosticReporter`'s host).
    pub case_sensitive: bool,
    /// `host.getNewLine()`.
    pub new_line: &'a str,
    /// The text after `TS` in the code slot: `diagnostic.code`, or roblox-ts's `" roblox-ts"` (or a plugin's own
    /// source) for its ad hoc diagnostics.
    pub code: &'a dyn Fn(P<Diagnostic>) -> String,
}

/// `formatColorAndReset`.
fn color(text: &str, style: &str) -> String {
    format!("{style}{text}{RESET}")
}

/// `getCategoryFormat`.
fn category_format(category: Category) -> &'static str {
    match category {
        Category::Error => RED,
        Category::Warning => YELLOW,
        Category::Suggestion => GREY,
        Category::Message => BLUE,
    }
}

/// `getLineAndCharacterOfPosition`: the line, and the UTF-16 units from its start.
fn line_and_character(file: &FileLike, pos: u32) -> (usize, usize) {
    let starts = file.ecma_line_map();
    let line = starts.partition_point(|&start| start <= pos).saturating_sub(1);
    let text = file.text();
    let pos = (pos as usize).min(text.len());
    (line, text[starts[line] as usize..pos].encode_utf16().count())
}

/// JavaScript's `String.prototype.trimEnd`: WhiteSpace and LineTerminator code points.
fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(|c: char| {
        matches!(c, '\t' | '\u{b}' | '\u{c}' | ' ' | '\u{a0}' | '\u{feff}' | '\n' | '\r' | '\u{2028}' | '\u{2029}')
            || (c != '\u{85}' && c != '\u{180e}' && c.is_whitespace())
    })
}

/// Whether a UTF-16 unit is JavaScript whitespace (`\s`).
fn is_js_space(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(|c| {
        matches!(c, '\t' | '\u{b}' | '\u{c}' | ' ' | '\u{a0}' | '\u{feff}' | '\n' | '\r' | '\u{2028}' | '\u{2029}')
            || (c != '\u{85}' && c != '\u{180e}' && c.is_whitespace())
    })
}

/// `String.prototype.slice` on UTF-16 units, `end` `None` for the rest.
fn slice(units: &[u16], start: usize, end: Option<usize>) -> &[u16] {
    let start = start.min(units.len());
    let end = end.map_or(units.len(), |end| end.min(units.len()));
    if end <= start { &[] } else { &units[start..end] }
}

/// `formatCodeSpan`.
fn code_span(file: &FileLike, start: u32, length: u32, indent: &str, squiggle: &str, nl: &str) -> String {
    let (first_line, first_line_char) = line_and_character(file, start);
    let (last_line, last_line_char) = line_and_character(file, start + length);
    let text = file.text();
    let starts = file.ecma_line_map();
    let last_line_in_file = line_and_character(file, text.len() as u32).0;
    let more_than_five = last_line - first_line >= 4;
    let mut gutter_width = (last_line + 1).to_string().len();
    if more_than_five {
        gutter_width = gutter_width.max(ELLIPSIS.len());
    }
    let mut context = String::new();
    let mut i = first_line;
    while i <= last_line {
        context.push_str(nl);
        if more_than_five && first_line + 1 < i && i < last_line - 1 {
            context.push_str(indent);
            context.push_str(&color(&format!("{ELLIPSIS:>gutter_width$}"), GUTTER_STYLE));
            context.push_str(GUTTER_SEPARATOR);
            context.push_str(nl);
            i = last_line - 1;
        }
        let line_start = starts[i] as usize;
        let line_end = if i < last_line_in_file { starts[i + 1] as usize } else { text.len() };
        let line_content = js_trim_end(&text[line_start..line_end]).replace('\t', " ");
        context.push_str(indent);
        context.push_str(&color(&format!("{:>gutter_width$}", i + 1), GUTTER_STYLE));
        context.push_str(GUTTER_SEPARATOR);
        context.push_str(&line_content);
        context.push_str(nl);
        context.push_str(indent);
        context.push_str(&color(&format!("{:>gutter_width$}", ""), GUTTER_STYLE));
        context.push_str(GUTTER_SEPARATOR);
        context.push_str(squiggle);
        let units: Vec<u16> = line_content.encode_utf16().collect();
        // `.replace(/./g, "~")`: every UTF-16 unit but a line terminator (none is left in a trimmed line's middle
        // but a lone `\r`, which ends a line of its own)
        let tildes = |part: &[u16]| "~".repeat(part.len());
        if i == first_line {
            let last_char_for_line = (i == last_line).then_some(last_line_char);
            // `.replace(/\S/g, " ")`: whitespace stays as it is
            let lead: Vec<u16> =
                slice(&units, 0, Some(first_line_char)).iter().map(|&u| if is_js_space(u) { u } else { 32 }).collect();
            context.push_str(&String::from_utf16_lossy(&lead));
            context.push_str(&tildes(slice(&units, first_line_char, last_char_for_line)));
        } else if i == last_line {
            context.push_str(&tildes(slice(&units, 0, Some(last_line_char))));
        } else {
            context.push_str(&tildes(&units));
        }
        context.push_str(RESET);
        i += 1;
    }
    context
}

/// `formatLocation`.
fn location(file: &FileLike, start: u32, format: &Format<'_>) -> String {
    let (line, character) = line_and_character(file, start);
    let options = ComparePathsOptions {
        use_case_sensitive_file_names: format.case_sensitive,
        current_directory: tspath::normalize_path(format.cwd),
    };
    let relative = tspath::convert_to_relative_path(file.file_name(), &options);
    format!(
        "{}:{}:{}",
        color(&relative, CYAN),
        color(&(line + 1).to_string(), YELLOW),
        color(&(character + 1).to_string(), YELLOW)
    )
}

/// A related information's span length. rbxts-rs: TypeScript 7's parser gives the bracket of "The parser expected to
/// find a '}' to match the '{' token here" an empty span (its writer then squiggles one character); TypeScript 5's
/// is the bracket itself (`createDetachedDiagnostic(…, openPosition, 1, …)`).
fn related_length(info: P<Diagnostic>) -> u32 {
    match diagnostic_len(info) {
        0 if info.code() == THE_PARSER_EXPECTED_TO_FIND => 1,
        length => length,
    }
}

/// One diagnostic as `formatDiagnosticsWithColorAndContext([diagnostic], host)` formats it, ending with a line end.
pub fn format_diagnostic(diagnostic: P<Diagnostic>, format: &Format<'_>) -> String {
    let nl = format.new_line;
    let mut output = String::new();
    let file = diagnostic_file(diagnostic);
    if let Some(file) = &file {
        output.push_str(&location(file, diagnostic_pos(diagnostic), format));
        output.push_str(" - ");
    }
    let category = diagnostic.category();
    output.push_str(&color(category.name(), category_format(category)));
    output.push_str(&color(&format!(" TS{}: ", (format.code)(diagnostic)), GREY));
    output.push_str(&flatten_diagnostic_message(diagnostic, nl));
    if let Some(file) = &file
        && diagnostic.code() != FILE_APPEARS_TO_BE_BINARY
    {
        output.push_str(nl);
        let (start, length) = (diagnostic_pos(diagnostic), diagnostic_len(diagnostic));
        output.push_str(&code_span(file, start, length, "", category_format(category), nl));
    }
    let related = diagnostic.related_information();
    if !related.is_empty() {
        output.push_str(nl);
        for &info in related {
            if let Some(file) = diagnostic_file(info) {
                output.push_str(nl);
                output.push_str(HALF_INDENT);
                let start = diagnostic_pos(info);
                output.push_str(&location(&file, start, format));
                output.push_str(&code_span(&file, start, related_length(info), INDENT, CYAN, nl));
            }
            output.push_str(nl);
            output.push_str(INDENT);
            output.push_str(&flatten_diagnostic_message(info, nl));
        }
    }
    output.push_str(nl);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_string_helpers() {
        assert_eq!(js_trim_end("a \u{a0}\u{feff}\t\r\n"), "a");
        assert_eq!(js_trim_end("a\u{85}"), "a\u{85}");
        let units: Vec<u16> = "ab😀".encode_utf16().collect();
        assert_eq!(slice(&units, 1, Some(9)).len(), 3);
        assert!(slice(&units, 3, Some(1)).is_empty());
        assert!(slice(&units, 7, None).is_empty());
        assert!(is_js_space(0xa0) && !is_js_space(0x85) && !is_js_space(u16::from(b'x')));
    }
}
