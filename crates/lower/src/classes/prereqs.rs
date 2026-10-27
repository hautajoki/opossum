//! `classes/Prereqs.ts`: a collector of the statements an expression needs to run first. Each collector owns one
//! destination for generated statements.

use std::cell::RefCell;

use rbxts_luau::{AnyIdentifier, Expr, Stmt, TempIds, TemporaryIdentifier, create};

use crate::util::value_to_id_str::value_to_id_str;

thread_local! {
    // rbxts-rs: `luau.tempId` hands out ids from one global counter; here they come from a per-thread counter that is
    // reset per file (`reset_temp_ids`), since ids only need to be unique within the tree being rendered.
    static TEMP_IDS: RefCell<TempIds> = RefCell::new(TempIds::new());
}

/// Starts a new file's temporary ids.
pub fn reset_temp_ids() {
    TEMP_IDS.with(|ids| *ids.borrow_mut() = TempIds::new());
}

/// rbxts-rs: the id the next temporary of this file gets (every temporary made so far has a smaller one).
pub fn next_temp_id() -> u32 {
    TEMP_IDS.with(|ids| ids.borrow().next_id())
}

/// `luau.tempId(name)`
pub fn temp_id(name: impl Into<String>) -> TemporaryIdentifier {
    TEMP_IDS.with(|ids| ids.borrow_mut().temp_id(name))
}

/// `luau.tempId(name)` as an expression.
pub fn temp_id_expr(name: impl Into<String>) -> Expr {
    Expr::TemporaryIdentifier(temp_id(name))
}

/// `Prereqs`
#[derive(Debug, Default)]
pub struct Prereqs {
    /// The statements, in order.
    pub statements: Vec<Stmt>,
}

impl Prereqs {
    /// No statements.
    pub fn new() -> Prereqs {
        Prereqs::default()
    }

    /// `push`
    pub fn push(&mut self, statement: Stmt) {
        self.statements.push(statement);
    }

    /// `pushList`
    pub fn push_list(&mut self, statements: Vec<Stmt>) {
        self.statements.extend(statements);
    }

    /// `unshift`
    pub fn unshift(&mut self, statement: Stmt) {
        self.statements.insert(0, statement);
    }

    /// `unshiftList`
    pub fn unshift_list(&mut self, statements: Vec<Stmt>) {
        self.statements.splice(0..0, statements);
    }

    /// `pushToVar`: `local _name = expression`, returning the temporary. The name defaults to one derived from the
    /// value (`valueToIdStr`).
    pub fn push_to_var(&mut self, expression: Expr, name: &str) -> Expr {
        let name = if name.is_empty() { value_to_id_str(&expression) } else { name.to_owned() };
        let temp = temp_id(name);
        self.push(create::local(temp.clone(), Some(expression)));
        Expr::TemporaryIdentifier(temp)
    }

    /// `pushToVar(undefined, name)`: `local _name`.
    pub fn push_to_var_empty(&mut self, name: &str) -> Expr {
        let temp = temp_id(name);
        self.push(create::local(temp.clone(), None));
        Expr::TemporaryIdentifier(temp)
    }

    /// `pushToVarIfComplex`: simple expressions (identifiers and primitive literals) are kept as they are.
    pub fn push_to_var_if_complex(&mut self, expression: Expr, name: &str) -> Expr {
        if expression.is_simple() { expression } else { self.push_to_var(expression, name) }
    }

    /// `pushToVarIfNonId`
    pub fn push_to_var_if_non_id(&mut self, expression: Expr, name: &str) -> Expr {
        if expression.is_any_identifier() { expression } else { self.push_to_var(expression, name) }
    }
}

/// An identifier expression as an [`AnyIdentifier`]; panics on other expressions (a compiler bug).
pub fn as_any_identifier(expression: Expr) -> AnyIdentifier {
    AnyIdentifier::try_from(expression).unwrap_or_else(|e| panic!("expected an identifier, got {e:?}"))
}
