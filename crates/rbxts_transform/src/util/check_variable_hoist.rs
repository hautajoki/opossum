//! `util/checkVariableHoist.ts`: a variable declared in a case clause and used outside it is declared before the
//! switch.

use tsrs_ast::{Kind, Node, Symbol};
use tsrs_core::P;

use crate::classes::transform_state::TransformState;
use crate::util::traversal::{get_ancestor, is_ancestor_of, source_file_of};

/// `checkVariableHoist`
pub fn check_variable_hoist(state: &mut TransformState<'_>, node: P<Node>, symbol: P<Symbol>) {
    if state.is_hoisted.contains_key(&symbol) {
        return;
    }

    let statement = get_ancestor(node, tsrs_ast::is_statement).expect("a declaration is inside a statement");
    let Some(case_clause) = statement.parent().filter(|p| p.kind() == Kind::CaseClause) else { return };
    let case_block = case_clause.parent().expect("a case clause is inside a case block");

    // rbxts-rs: `FindAllReferences.Core.eachSymbolReferenceInFile` becomes the checker's in-file reference search,
    // limited to the case block as roblox-ts's search container
    let file = source_file_of(node);
    let references = state.checker.get_references_to_symbol_in_file(file, symbol);
    let is_used_outside_of_case_clause = references
        .into_iter()
        .any(|token| token != node && is_ancestor_of(case_block, token) && !is_ancestor_of(case_clause, token));

    if is_used_outside_of_case_clause {
        state.hoists_by_statement.entry(case_clause).or_default().push(node);
        state.is_hoisted.insert(symbol, true);
    }
}
