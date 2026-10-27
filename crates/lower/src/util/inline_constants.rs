//! Literal values inlined at their use sites, across files: a `const` whose initializer is a number, string or
//! boolean literal, and a member of a regular enum whose members are all literals (const enum members are inlined by
//! the checker's constant values already). A literal enum keeps its table unless nothing could read it: an enum that
//! is not exported and only ever has its members read by name.

use luau::{Expr, UnaryOperator, create};
use tsrs_ast::{Kind, ModifierFlags, Node, NodeFlags, Symbol, SymbolFlags};
use tsrs_checker::LiteralValue;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_no_substitution_template_literal::transform_no_substitution_template_literal;
use crate::nodes::expressions::transform_numeric_literal::transform_numeric_literal;
use crate::nodes::expressions::transform_string_literal::transform_string_literal;
use crate::util::create_string_from_literal::create_string_from_value;
use crate::util::traversal::{has_syntactic_modifier, skip_downwards, skip_upwards};

/// Longer strings stay references: inlining copies them into every use.
const MAX_INLINED_STRING: usize = 64;

/// A declaration from the project's own sources: not a `.d.ts`, not `declare`d (whose values live elsewhere).
fn is_project_declaration(declaration: P<Node>) -> bool {
    !declaration.flags().intersects(NodeFlags::Ambient)
        && tsrs_ast::get_source_file_of_node(declaration).is_some_and(|f| !f.is_declaration_file())
}

/// The literal a read of `identifier` (whose symbol is `symbol`) always gives: the identifier names a `const`,
/// possibly imported, initialized with a number, string or boolean literal.
pub fn inline_constant(state: &mut TransformState<'_>, identifier: P<Node>, symbol: P<Symbol>) -> Option<Expr> {
    if tsrs_ast::is_write_access(identifier) {
        return None;
    }
    let symbol = if symbol.flags().intersects(SymbolFlags::Alias) { state.checker.skip_alias(symbol) } else { symbol };
    let declaration = symbol.value_declaration()?;
    if declaration.kind() != Kind::VariableDeclaration
        || declaration.name()?.kind() != Kind::Identifier
        || !declaration.parent()?.flags().intersects(NodeFlags::Const)
        || !is_project_declaration(declaration)
    {
        return None;
    }
    let initializer = skip_downwards(declaration.as_variable_declaration().initializer()?);
    match initializer.kind() {
        Kind::NumericLiteral => Some(transform_numeric_literal(initializer)),
        Kind::PrefixUnaryExpression => {
            let unary = initializer.as_prefix_unary_expression();
            let operand = skip_downwards(unary.operand);
            (unary.operator == Kind::MinusToken && operand.kind() == Kind::NumericLiteral)
                .then(|| create::unary(UnaryOperator::Neg, transform_numeric_literal(operand)))
        }
        Kind::StringLiteral if initializer.text().len() <= MAX_INLINED_STRING => {
            Some(transform_string_literal(initializer))
        }
        Kind::NoSubstitutionTemplateLiteral if initializer.text().len() <= MAX_INLINED_STRING => {
            Some(transform_no_substitution_template_literal(initializer))
        }
        Kind::TrueKeyword => Some(create::bool(true)),
        Kind::FalseKeyword => Some(create::bool(false)),
        _ => None,
    }
}

/// A regular enum of the project whose every member has a literal value.
fn is_literal_enum(state: &mut TransformState<'_>, declaration: P<Node>) -> bool {
    declaration.kind() == Kind::EnumDeclaration
        && !has_syntactic_modifier(declaration, ModifierFlags::Const)
        && is_project_declaration(declaration)
        && declaration.as_enum_declaration().members.nodes().iter().all(|&member| {
            matches!(state.checker.get_constant_value(member), Some(LiteralValue::Number(_) | LiteralValue::String(_)))
        })
}

/// The value of `E.A` or `E["A"]` (`access`) when `E` is a literal regular enum.
pub fn inline_enum_member(state: &mut TransformState<'_>, access: P<Node>) -> Option<Expr> {
    let name = match access.kind() {
        Kind::PropertyAccessExpression => access.name()?,
        Kind::ElementAccessExpression => skip_downwards(access.as_element_access_expression().argument_expression),
        _ => return None,
    };
    let symbol = state.checker.get_symbol_at_location_exported(name)?;
    if !symbol.flags().intersects(SymbolFlags::EnumMember) {
        return None;
    }
    let member = symbol.value_declaration()?;
    if !is_literal_enum(state, member.parent()?) {
        return None;
    }
    match state.checker.get_constant_value(member)? {
        LiteralValue::Number(n) => Some(create::number(n.0)),
        LiteralValue::String(s) => Some(create_string_from_value(s)),
        _ => None,
    }
}

/// The literal enum `declaration` needs no table: it is not exported, and every reference to it in its file reads a
/// member by name (`E.A`, `E["A"]`) or is a type.
pub fn is_enum_table_unused(state: &mut TransformState<'_>, declaration: P<Node>) -> bool {
    if !is_literal_enum(state, declaration) || has_syntactic_modifier(declaration, ModifierFlags::Export) {
        return false;
    }
    let name = declaration.as_enum_declaration().name;
    let Some(symbol) = state.checker.get_symbol_at_location_exported(name) else { return false };
    if symbol.declarations().len() != 1 {
        return false;
    }
    let file = crate::util::traversal::source_file_of(declaration);
    state.checker.get_references_to_symbol_in_file(file, symbol).into_iter().all(|reference| {
        if reference == name || tsrs_ast::is_part_of_type_node(reference) {
            return true;
        }
        let up = skip_upwards(reference);
        let Some(parent) = up.parent() else { return false };
        match parent.kind() {
            Kind::PropertyAccessExpression => parent.expression() == Some(up),
            Kind::ElementAccessExpression => {
                let access = parent.as_element_access_expression();
                parent.expression() == Some(up)
                    && matches!(
                        skip_downwards(access.argument_expression).kind(),
                        Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral
                    )
            }
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use crate::Options;
    use crate::tests::lower;

    #[test]
    fn literals_inline_across_files_and_unread_enum_tables_go() {
        let a = r#"
            export const COUNT = 4;
            export const NAME = "crate";
            export const DOWN = -1;
            export let live = 1;
            export const computed = COUNT * 2;
            export enum Shared { A, B = 5 }
            export function bump() { live += 1; }
        "#;
        let b = r#"
            import { COUNT, NAME, DOWN, live, computed, Shared } from "./a";
            enum Local { X = "x", Y = "y" }
            enum Dynamic { P, Q }
            export function use(key: "P" | "Q") {
                return [COUNT, NAME, DOWN, live, computed, Shared.B, Local.Y, Dynamic[key], Dynamic.Q];
            }
        "#;
        let lowered = lower(&[("a.ts", a), ("b.ts", b)], Options { native_types: false, ..Options::default() });
        let b = &lowered["b.ts"];
        // `live` can change and `computed` is no literal; enum members are literals wherever they are read
        assert!(b.contains(r#"{ 4, "crate", -1, live, computed, 5, "y", Dynamic[key], 1 }"#), "{b}");
        // `Local` is only read by member name; `Dynamic` is indexed by a value, `Shared` is exported
        assert!(!b.contains("Local") && b.contains("Dynamic = setmetatable"), "{b}");
        assert!(lowered["a.ts"].contains("Shared = setmetatable"), "{}", lowered["a.ts"]);
    }
}
