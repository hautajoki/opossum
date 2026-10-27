//! Which functions get Luau's `@native` attribute (with every function nested in them): a `/** @native */` JSDoc on the
//! function, on the variable statement or property it initializes, or on its class; every function of a file that
//! starts with a `// @native` pragma or is listed in [`crate::Options::native_files`].

use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::traversal::skip_upwards;

/// `node` carries a `@native` JSDoc tag.
pub fn has_native_jsdoc(node: P<Node>) -> bool {
    node.jsdoc(None).iter().any(|doc| {
        doc.as_jsdoc().tags.is_some_and(|tags| {
            tags.nodes().iter().any(|&tag| tag.kind() == Kind::JSDocUnknownTag && tag.tag_name().text() == "native")
        })
    })
}

/// The comment text (after `//` or inside `/** */`) of a comment that only says `@native`: the attribute replaces it.
pub fn is_native_directive(text: &str) -> bool {
    text.trim_matches(|c: char| c.is_whitespace() || c == '*') == "@native"
}

/// The function-like `node` (a declaration, method, constructor, function expression or arrow function) is marked
/// `@native` itself, through the statement or property it initializes, or through its class.
pub fn is_native_function(node: P<Node>) -> bool {
    if has_native_jsdoc(node) {
        return true;
    }
    let Some(parent) = skip_upwards(node).parent() else { return false };
    match parent.kind() {
        // `/** @native */ const f = () => …`
        Kind::VariableDeclaration
            if parent.as_variable_declaration().initializer().map(skip_upwards) == Some(skip_upwards(node)) =>
        {
            let statement = parent.parent().and_then(|list| list.parent());
            statement.is_some_and(|s| s.kind() == Kind::VariableStatement && has_native_jsdoc(s))
        }
        // `{ /** @native */ f: () => … }`, `class { /** @native */ f = () => … }`
        Kind::PropertyAssignment => has_native_jsdoc(parent),
        Kind::PropertyDeclaration => {
            has_native_jsdoc(parent)
                || parent.parent().is_some_and(|class| tsrs_ast::is_class_like(class) && has_native_jsdoc(class))
        }
        // methods and constructors of a `/** @native */` class
        _ if tsrs_ast::is_class_like(parent) => {
            matches!(node.kind(), Kind::MethodDeclaration | Kind::Constructor | Kind::GetAccessor | Kind::SetAccessor)
                && has_native_jsdoc(parent)
        }
        _ => false,
    }
}

/// Every function of the file is `@native`: the file is in [`crate::Options::native_files`] or its leading comments
/// hold a `// @native` line.
pub fn is_native_file(state: &TransformState<'_>) -> bool {
    let file = state.source_file.get();
    if state.options.native_files.contains(file.file_name()) {
        return true;
    }
    let text = file.text();
    tsrs_scanner::get_leading_comment_ranges(text, 0).any(|range| {
        let (start, end) = (
            range.text_range.pos().expect_source("comment") as usize,
            range.text_range.end().expect_source("comment") as usize,
        );
        range.kind == Kind::SingleLineCommentTrivia && is_native_directive(&text[start + 2..end])
    })
}

#[cfg(test)]
mod tests {
    use crate::Options;
    use crate::tests::lower;

    fn attributes(text: &str) -> usize {
        text.matches("@native\n").count() + text.matches("@native function").count()
    }

    #[test]
    fn native_marks_tagged_functions_classes_and_whole_files() {
        let tagged = r#"
            /** @native */
            export function a() { return () => 1; }
            /** @native */
            export const b = (x: number) => x * 2;
            export function plain() { return 0; }
            /** @native */
            export class C {
                constructor(public x: number) {}
                m() { return this.x; }
            }
        "#;
        let pragma = "// @native\nexport function d() { return () => 2; }\n";
        let listed = "export function e() { return 3; }\nexport const f = () => 4;\n";
        let mut options = Options { native_types: false, ..Options::default() };
        options.native_files.insert("listed.ts".to_owned());
        let lowered = lower(&[("tagged.ts", tagged), ("pragma.ts", pragma), ("listed.ts", listed)], options);
        let tagged = &lowered["tagged.ts"];
        // a and its closure (Luau compiles natively only functions that carry the attribute), b, the constructor and
        // m; not plain; the JSDoc is not echoed as a comment
        assert_eq!(attributes(tagged), 5, "{tagged}");
        assert!(tagged.contains("const b = @native function(x)") && !tagged.contains("-- @native"), "{tagged}");
        // every function, nested ones included, and the pragma is gone
        assert_eq!(attributes(&lowered["pragma.ts"]), 2, "{}", lowered["pragma.ts"]);
        assert!(!lowered["pragma.ts"].contains("-- @native"));
        assert_eq!(attributes(&lowered["listed.ts"]), 2, "{}", lowered["listed.ts"]);
        assert!(!tagged.contains("--!native"));
    }
}
