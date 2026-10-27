//! `util/hasMultipleDefinitions.ts`

use tsrs_ast::{Node, Symbol};
use tsrs_core::P;

/// Whether more than one of the symbol's declarations passes `filter` (enum and namespace merging).
pub fn has_multiple_definitions(symbol: P<Symbol>, filter: impl Fn(P<Node>) -> bool) -> bool {
    symbol.declarations().iter().filter(|&&declaration| filter(declaration)).nth(1).is_some()
}
