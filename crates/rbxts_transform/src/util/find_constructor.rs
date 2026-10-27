//! `util/findConstructor.ts`

use tsrs_ast::{Kind, Node};
use tsrs_core::P;

/// `findConstructor`: the class's constructor implementation (overload signatures have no body).
pub fn find_constructor(node: P<Node>) -> Option<P<Node>> {
    node.members().iter().copied().find(|&member| member.kind() == Kind::Constructor && member.body().is_some())
}
