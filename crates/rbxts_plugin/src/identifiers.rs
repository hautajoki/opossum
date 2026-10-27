//! TypeScript 5's `sourceFile.identifiers`, the set its printer checks a unique name against
//! (`isFileLevelUniqueName`).

use tsrs_ast::{Kind, Node, SourceFile};
use tsrs_core::P;

/// Every text TypeScript 5's parser interns into `sourceFile.identifiers`, sorted and without duplicates:
/// - every identifier and private identifier, those in JSDoc included (tag names, `@param` names, `{@link}`
///   names, names in JSDoc types): TypeScript 5 parses the JSDoc of every node that can have one;
/// - string, numeric and big-integer literals the parser interns: property names (`parsePropertyName`: object
///   literal members, class and type members, enum members, `{ "a": x }` in a binding), element access arguments
///   that are string, numeric or no-substitution template literals (`o["a"]`, `o[0]`), module specifiers of
///   imports and exports (and `import x = require("m")`), and ambient module names (`declare module "m"`).
///
/// A numeric literal is interned as its value's text (`0x10` as `16`), which is how tsrs keeps it too.
pub fn ts5_file_identifiers(file: P<SourceFile>) -> Vec<&'static str> {
    fn walk(node: P<Node>, file: &'static SourceFile, out: &mut Vec<&'static str>) {
        match node.kind() {
            Kind::Identifier | Kind::PrivateIdentifier => out.push(node.text()),
            Kind::StringLiteral
            | Kind::NumericLiteral
            | Kind::BigIntLiteral
            | Kind::NoSubstitutionTemplateLiteral
                if is_interned_literal(node) =>
            {
                out.push(node.text())
            }
            _ => {}
        }
        for &jsdoc in node.jsdoc(Some(file)) {
            walk(jsdoc, file, out);
        }
        node.for_each_child(&mut |child| {
            walk(child, file, out);
            false
        });
    }
    let mut out = Vec::new();
    walk(file.as_node(), file.get(), &mut out);
    out.sort_unstable();
    out.dedup();
    out
}

/// A literal whose text TypeScript 5's parser interns (`internIdentifier`), by where it sits.
fn is_interned_literal(node: P<Node>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        // `parsePropertyNameWorker`; an ambient module's name
        Kind::PropertyAssignment
        | Kind::PropertyDeclaration
        | Kind::PropertySignature
        | Kind::MethodDeclaration
        | Kind::MethodSignature
        | Kind::GetAccessor
        | Kind::SetAccessor
        | Kind::EnumMember
        | Kind::ModuleDeclaration => {
            parent.name() == Some(node) && node.kind() != Kind::NoSubstitutionTemplateLiteral
        }
        Kind::BindingElement => parent.property_name() == Some(node),
        // `isStringOrNumericLiteralLike(argument)`, as written (parentheses are not skipped)
        Kind::ElementAccessExpression => {
            parent.as_element_access_expression().argument_expression == node
                && node.kind() != Kind::BigIntLiteral
        }
        // `parseModuleSpecifier`
        Kind::ImportDeclaration => parent.as_import_declaration().module_specifier == node,
        Kind::ExportDeclaration => parent.as_export_declaration().module_specifier == Some(node),
        Kind::ExternalModuleReference => parent.as_external_module_reference().expression == node,
        _ => false,
    }
}
