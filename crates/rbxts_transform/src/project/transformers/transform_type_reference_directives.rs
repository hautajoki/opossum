//! `Project/transformers/builtin/transformTypeReferenceDirectives.ts`: in an emitted declaration, a
//! `/// <reference types="types" />` directive names `@rbxts/types`.
//!
//! rbxts-rs: on the declaration text (see [`super::transform_paths`]): the directive's name is replaced in place.

use tsrs_ast::SourceFile;
use tsrs_core::P;

use super::transform_paths::Rewrite;

const SCOPE: &str = "@rbxts/";
const PACKAGE: &str = "types";

/// The rewrites of `file`'s `types` reference directives.
pub fn rewrites(file: P<SourceFile>) -> Vec<Rewrite> {
    let text = file.get().text();
    file.type_reference_directives()
        .iter()
        .filter(|directive| directive.file_name == PACKAGE)
        .filter_map(|directive| {
            let (start, end) =
                (directive.text_range.pos().source_offset()?, directive.text_range.end().source_offset()?);
            // the range is the name's text inside the comment
            (text.get(start as usize..end as usize) == Some(PACKAGE)).then(|| Rewrite {
                start,
                end,
                text: format!("{SCOPE}{PACKAGE}"),
            })
        })
        .collect()
}
