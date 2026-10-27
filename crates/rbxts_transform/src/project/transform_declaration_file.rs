//! `Project/functions/transformDeclarationFile.ts`: a declaration file under a root directory of a declaration build
//! is an asset whose output is its text parsed and printed again by TypeScript's printer, with
//! [`TransformPaths`]'s rewrites (comments kept, the layout the printer's: four-space indents, no blank lines).
//!
//! rbxts-rs: the rewrites go into the source text before the parse (each specifier token replaced by the literal the
//! printer prints for TypeScript's synthesized one), so the printer prints the rewritten specifiers from the source as
//! it prints every other unchanged node: the same text.

use tsrs_ast::{SourceFile, SourceFileParseOptions};
use tsrs_core::{NewLineKind, P, ScriptTarget, tspath};

use super::transformers::transform_paths::{TransformPaths, apply};

/// Parses `text` as TypeScript's `ts.createSourceFile(file_name, text, ScriptTarget.Latest, true)` does.
pub(crate) fn parse(file_name: &str, text: &str) -> P<SourceFile> {
    let file_name = tspath::normalize_path(file_name);
    tsrs_parser::parse_source_file(
        SourceFileParseOptions {
            path: tspath::Path::new(file_name.clone()),
            external_module_indicator_options: Default::default(),
            file_name: file_name.clone(),
        },
        text,
        tsrs_core::get_script_kind_from_file_name(&file_name),
    )
}

/// The output text of the declaration asset `file_name` whose text is `contents`.
pub fn transform_declaration_file(paths: &TransformPaths, file_name: &str, contents: &str) -> String {
    let file = parse(file_name, contents);
    let rewrites = paths.rewrites(file, &tspath::normalize_path(file_name));
    let file = if rewrites.is_empty() { file } else { parse(file_name, &apply(contents, &rewrites)) };
    // `ts.createPrinter()`: LF line ends, comments kept
    let mut printer = tsrs_printer::new_printer(
        tsrs_printer::PrinterOptions { new_line: NewLineKind::LF, target: ScriptTarget::ESNext, ..Default::default() },
        tsrs_printer::PrintHandlers::default(),
        None,
    );
    printer.emit_source_file(file)
}
