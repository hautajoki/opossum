//! Small symbol and node predicates, one per roblox-ts file: `isSymbolMutable.ts`, `isSymbolOfValue.ts`,
//! `getOriginalSymbolOfNode.ts`, `validateIdentifier.ts`, `getExtendsNode.ts`, `isUsedAsStatement.ts`,
//! `getStatements.ts`, and `typeGuards.ts`.

use tsrs_ast::{Kind, Node, NodeFlags, Symbol, SymbolFlags};
use tsrs_checker::Checker;
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::traversal::{get_ancestor, skip_upwards};

/// `isSymbolMutable`: a parameter or a `let` variable.
pub fn is_symbol_mutable(state: &mut TransformState<'_>, id_symbol: P<Symbol>) -> bool {
    if let Some(&cached) = state.multi_transform_state.is_defined_as_let_cache.get(&id_symbol) {
        return cached;
    }
    let mut result = false;
    if let Some(declaration) = id_symbol.value_declaration() {
        if declaration.kind() == Kind::Parameter {
            result = true;
        } else if let Some(list) = get_ancestor(declaration, |n| n.kind() == Kind::VariableDeclarationList) {
            result = list.flags().intersects(NodeFlags::Let);
        }
    }
    state.multi_transform_state.is_defined_as_let_cache.insert(id_symbol, result);
    result
}

/// `isSymbolOfValue`: a value at runtime (not a type, not a const enum).
pub fn is_symbol_of_value(symbol: P<Symbol>) -> bool {
    symbol.flags().intersects(SymbolFlags::Value) && !symbol.flags().intersects(SymbolFlags::ConstEnum)
}

/// `getOriginalSymbolOfNode`: the node's symbol with aliases skipped.
pub fn get_original_symbol_of_node(checker: &mut Checker, node: P<Node>) -> Option<P<Symbol>> {
    let symbol = checker.get_symbol_at_location_exported(node)?;
    Some(checker.skip_alias(symbol))
}

/// `validateIdentifier`
pub fn validate_identifier(state: &mut TransformState<'_>, node: P<Node>) {
    let text = node.text();
    if !rbxts_luau::is_valid_identifier(text) {
        state.add_diagnostic(errors::no_invalid_identifier(node));
    } else if rbxts_luau::is_reserved_identifier(text) {
        state.add_diagnostic(errors::no_reserved_identifier(node));
    }
}

/// `getExtendsNode`: the class's `extends` clause type.
pub fn get_extends_node(node: P<Node>) -> Option<P<Node>> {
    let clauses = node.class_like_data()?.heritage_clauses()?;
    for &clause in clauses.nodes() {
        let heritage = clause.as_heritage_clause();
        if heritage.token == Kind::ExtendsKeyword {
            return heritage.types.get().nodes().first().copied();
        }
    }
    None
}

/// `isUsedAsStatement`: the expression's value is discarded.
pub fn is_used_as_statement(expression: P<Node>) -> bool {
    let child = skip_upwards(expression);
    let Some(parent) = child.parent() else { return false };
    match parent.kind() {
        Kind::ExpressionStatement => true,
        // part of a for statement's header, except its condition
        Kind::ForStatement => parent.as_for_statement().condition != Some(child),
        Kind::DeleteExpression => is_used_as_statement(parent),
        _ => false,
    }
}

/// `getStatements`: a block's statements, or the statement itself.
pub fn get_statements(statement: P<Node>) -> Vec<P<Node>> {
    if statement.kind() == Kind::Block { statement.statements().to_vec() } else { vec![statement] }
}

/// `isBlockLike`
pub fn is_block_like(node: P<Node>) -> bool {
    matches!(node.kind(), Kind::SourceFile | Kind::Block | Kind::ModuleBlock | Kind::CaseClause | Kind::DefaultClause)
}

/// `isNamespace`: a module declaration written `namespace` (not `module "x"` / `declare global`).
///
/// rbxts-rs: TypeScript 7 has no `NodeFlags.Namespace`; the declaration records its keyword instead (nested
/// `namespace A.B` parts carry the outer keyword).
pub fn is_namespace(node: P<Node>) -> bool {
    node.kind() == Kind::ModuleDeclaration && node.as_module_declaration().keyword == Kind::NamespaceKeyword
}
