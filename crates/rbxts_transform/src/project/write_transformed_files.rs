//! `--writeTransformedFiles` (`writeTransformedFiles` in `Project/functions/compileFiles.ts`): with TypeScript
//! transformers (tsconfig `plugins` entries with a `transform`), stock roblox-ts writes each file it compiles, as the
//! transformers left it and printed again by TypeScript's printer, to `<out>/<name>.transformed.ts` (`.tsx`); without
//! transformers it writes none, and its clean removes old ones.
//!
//! rbxts-rs: plugins are compiled in and work on the transform to Luau, not on the TypeScript tree, so there is no
//! transformed TypeScript. A project with a plugin that a tsconfig `plugins` entry turns on (`tsconfig_transform`)
//! writes what stock writes with a transformer that changes nothing: each compiled file printed again by TypeScript's
//! printer (tsrs's). A plugin that does change the program in stock (Flamework's macros) has its changes in the
//! Luau only; the `.transformed.ts` shows the source as the compiler read it. A project without such a plugin
//! writes none, as stock. Written with the Luau outputs, all or nothing (stock writes them while transforming, before
//! a later error stops the build).

use tsrs_ast::SourceFile;
use tsrs_core::{NewLineKind, P, ScriptTarget};

/// `ts.createPrinter().printFile(file)`: LF line ends, comments kept.
pub fn print_source_file(file: P<SourceFile>) -> String {
    let mut printer = tsrs_printer::new_printer(
        tsrs_printer::PrinterOptions { new_line: NewLineKind::LF, target: ScriptTarget::ESNext, ..Default::default() },
        tsrs_printer::PrintHandlers::default(),
        None,
    );
    printer.emit_source_file(file)
}
