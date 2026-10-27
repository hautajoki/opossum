//! `util/traversal.ts`, plus the few `ts.*` node helpers the transformer needs that tsrs spells differently.

use tsrs_ast::{Kind, ModifierFlags, Node, SourceFile};
use tsrs_core::P;

/// `isAncestorOf`: `ancestor` is `node` or one of its ancestors.
pub fn is_ancestor_of(ancestor: P<Node>, node: P<Node>) -> bool {
    let mut current = Some(node);
    while let Some(n) = current {
        if n == ancestor {
            return true;
        }
        current = n.parent();
    }
    false
}

fn is_skipped_wrapper(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::NonNullExpression
            | Kind::ParenthesizedExpression
            | Kind::AsExpression
            | Kind::TypeAssertionExpression
            | Kind::SatisfiesExpression
    )
}

/// `skipDownwards`: through `!`, parentheses, `as`, `<T>` and `satisfies`.
pub fn skip_downwards(mut node: P<Node>) -> P<Node> {
    while is_skipped_wrapper(node.kind()) {
        node = node.expression().expect("a wrapper expression has an expression");
    }
    node
}

/// `skipUpwards`: the outermost wrapper (`!`, parentheses, `as`, `<T>`, `satisfies`) around `node`.
pub fn skip_upwards(mut node: P<Node>) -> P<Node> {
    while let Some(parent) = node.parent()
        && is_skipped_wrapper(parent.kind())
    {
        node = parent;
    }
    node
}

/// `getAncestor`: the first of `node` and its ancestors that passes `check`.
pub fn get_ancestor(node: P<Node>, check: impl Fn(P<Node>) -> bool) -> Option<P<Node>> {
    let mut current = Some(node);
    while let Some(n) = current {
        if check(n) {
            return Some(n);
        }
        current = n.parent();
    }
    None
}

/// `getModuleAncestor`: the enclosing source file or module declaration.
pub fn get_module_ancestor(node: P<Node>) -> P<Node> {
    get_ancestor(node, |n| matches!(n.kind(), Kind::SourceFile | Kind::ModuleDeclaration))
        .expect("every node is inside a source file")
}

/// The source file of a parse-tree node.
pub fn source_file_of(node: P<Node>) -> P<SourceFile> {
    tsrs_ast::get_source_file_of_node(node).expect("a parse-tree node has a source file")
}

/// `node.getStart()`: the start of the node's first token (trivia skipped, JSDoc not included).
pub fn get_start(node: P<Node>) -> u32 {
    tsrs_scanner::get_token_pos_of_node(node, source_file_of(node), false)
}

/// `node.getText()`: the node's source text without leading trivia.
pub fn get_text(node: P<Node>) -> &'static str {
    let file = source_file_of(node);
    let start = tsrs_scanner::get_token_pos_of_node(node, file, false) as usize;
    let end = node.end().expect_source("a parse-tree node has a source position") as usize;
    &file.get().text()[start..end.max(start)]
}

/// `ts.hasSyntacticModifier(node, flags)`
pub fn has_syntactic_modifier(node: P<Node>, flags: ModifierFlags) -> bool {
    tsrs_ast::has_syntactic_modifier(node, flags)
}

/// `ts.getModifiers(node)?.some(m => m.kind === DeclareKeyword)` (decorators are not modifiers here).
pub fn has_declare_modifier(node: P<Node>) -> bool {
    node.modifiers().is_some_and(|m| m.nodes().iter().any(|n| n.kind() == Kind::DeclareKeyword))
}

/// The text of an identifier-like name node (identifiers, private identifiers, string and numeric literals).
pub fn name_text(node: P<Node>) -> &'static str {
    node.text()
}
