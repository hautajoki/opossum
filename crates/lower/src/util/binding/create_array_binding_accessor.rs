//! `util/binding/createArrayBindingAccessor.ts`: how an array pattern reads its elements, per source type.
//!
//! rbxts-rs: TypeScript's accessor is an object of two closures over shared mutable state; here it is an enum whose
//! iterator variants keep that state in fields. Reading needs no `TransformState`, so the loops area can drive an
//! accessor between its own transforms.

use rbxts_luau::{
    AnyIdentifier, Assignment, AssignmentOperator, BinaryOperator, ElseBody, Expr, Stmt, UnaryOperator, create,
    globals,
};
use tsrs_ast::Node;
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::errors;
use crate::util::types::{
    is_array_type, is_definitely_type, is_iterable_function_lua_tuple_type, is_iterable_function_type,
    is_iterable_type, is_map_type, is_set_type, is_shared_table_type, is_string_type,
};

/// `BindingAccessor`: reads the elements of an array pattern's source in order (`read`), and the rest of it
/// (`rest`).
pub enum BindingAccessor {
    /// Arrays and tuples: `parent[i + 1]`, `table.move(parent, i + 1, #parent, 1, {})`.
    Array {
        /// The array.
        parent_id: Expr,
    },
    /// Strings, by UTF-8 character through one `string.gmatch` matcher.
    String {
        /// The `string.gmatch(s, utf8.charpattern)` iterator, in a temporary.
        matcher: Expr,
    },
    /// Iterators (iterable functions, `SharedTable`, objects with `next`), consumed once.
    Iterator(IteratorAccessor),
    /// Sets and maps, walked with `next`.
    Collection(CollectionAccessor),
    /// The source type cannot be destructured; a diagnostic was reported.
    Unsupported,
}

impl BindingAccessor {
    /// `read`: element `index`. `omitted` elements (`[, b]`) are still consumed, but give `luau.none()`.
    pub fn read(&mut self, prereqs: &mut Prereqs, index: usize, omitted: bool) -> Expr {
        match self {
            BindingAccessor::Array { parent_id } => {
                create::computed_index(parent_id.clone(), create::number((index + 1) as f64))
            }
            BindingAccessor::String { matcher } => {
                let value = create::call(matcher.clone(), Vec::new());
                if omitted {
                    prereqs.push(create::call_statement(value));
                    return Expr::None;
                }
                value
            }
            BindingAccessor::Iterator(accessor) => accessor.read(prereqs, omitted),
            BindingAccessor::Collection(accessor) => accessor.read(prereqs, index, omitted),
            BindingAccessor::Unsupported => Expr::None,
        }
    }

    /// `rest`: the elements from `index` on, as a new array.
    pub fn rest(&mut self, prereqs: &mut Prereqs, index: usize) -> Expr {
        match self {
            BindingAccessor::Array { parent_id } => create::call(
                globals::table::r#move(),
                vec![
                    parent_id.clone(),
                    create::number((index + 1) as f64),
                    create::unary(UnaryOperator::Len, parent_id.clone()),
                    create::number(1.0),
                    create::array(Vec::new()),
                ],
            ),
            BindingAccessor::String { matcher } => {
                let rest = prereqs.push_to_var(create::array(Vec::new()), "rest");
                let char = temp_id("char");
                prereqs.push(create::for_in(
                    vec![AnyIdentifier::Temporary(char.clone())],
                    matcher.clone(),
                    vec![create::call_statement(create::call(
                        globals::table::insert(),
                        vec![rest.clone(), Expr::TemporaryIdentifier(char)],
                    ))],
                ));
                rest
            }
            BindingAccessor::Iterator(accessor) => accessor.rest(prereqs),
            BindingAccessor::Collection(accessor) => accessor.inner.rest(prereqs),
            BindingAccessor::Unsupported => Expr::None,
        }
    }
}

/// One step of an iterator: the value it produced and the condition that it was exhausted.
struct IteratorStep {
    value: Expr,
    done: Expr,
}

/// How an [`IteratorAccessor`] advances.
enum StepKind {
    /// `createFunctionAccessor`: calls `iterator`; `tuple` iterators return several values.
    Function { iterator: Expr, tuple: bool },
    /// `createCollectionAccessor`'s step: `key = next(parent, key)`, restarting past `consumed` keys.
    Collection { parent_id: Expr, key: Expr, map: bool, consumed: Option<Expr> },
    /// An object with a `next` method returning `{ value, done }`.
    Next { next: Expr },
}

impl StepKind {
    fn step(&self, prereqs: &mut Prereqs, omitted: bool) -> IteratorStep {
        match self {
            StepKind::Function { iterator, tuple } => {
                let call = create::call(iterator.clone(), Vec::new());
                let value = prereqs.push_to_var(if *tuple && !omitted { create::array(vec![call]) } else { call }, "value");
                let first = if *tuple && !omitted {
                    create::computed_index(value.clone(), create::number(1.0))
                } else {
                    value.clone()
                };
                IteratorStep { value, done: create::binary(first, BinaryOperator::Eq, Expr::Nil) }
            }
            StepKind::Collection { parent_id, key, map, consumed } => {
                let member = (*map && !omitted).then(|| prereqs.push_to_var_empty("value"));
                let advance = || {
                    let left = match &member {
                        Some(member) => vec![key.clone(), member.clone()],
                        None => vec![key.clone()],
                    };
                    Stmt::Assignment(Box::new(Assignment {
                        left,
                        operator: AssignmentOperator::Assign,
                        right: vec![create::call(globals::next(), vec![parent_id.clone(), key.clone()])],
                    }))
                };
                if consumed.is_some() {
                    // destination evaluation can remove the cursor and invalidate next's continuation key
                    prereqs.push(create::if_stmt(
                        create::binary(
                            create::binary(key.clone(), BinaryOperator::Ne, Expr::Nil),
                            BinaryOperator::And,
                            create::binary(
                                create::computed_index(parent_id.clone(), key.clone()),
                                BinaryOperator::Eq,
                                Expr::Nil,
                            ),
                        ),
                        vec![create::assignment(key.clone(), AssignmentOperator::Assign, Expr::Nil)],
                        ElseBody::default(),
                    ));
                }
                prereqs.push(advance());
                if let Some(consumed) = consumed {
                    // restarting must not return prefix elements that were already consumed
                    prereqs.push(create::while_stmt(
                        create::binary(
                            create::binary(key.clone(), BinaryOperator::Ne, Expr::Nil),
                            BinaryOperator::And,
                            create::computed_index(consumed.clone(), key.clone()),
                        ),
                        vec![advance()],
                    ));
                }
                let value = match member {
                    Some(member) => create::array(vec![key.clone(), member]),
                    None => key.clone(),
                };
                IteratorStep { value, done: create::binary(key.clone(), BinaryOperator::Eq, Expr::Nil) }
            }
            StepKind::Next { next } => {
                let result = prereqs.push_to_var(create::call(next.clone(), Vec::new()), "result");
                IteratorStep { value: create::property(result.clone(), "value"), done: create::property(result, "done") }
            }
        }
    }
}

fn if_not_done(done: Expr, value: Expr) -> Expr {
    create::if_expr(done, Expr::Nil, value)
}

/// `createIteratorAccessor`: prefix elements and rest consume one iterator and share its terminal state.
pub struct IteratorAccessor {
    step: StepKind,
    has_completion_value: bool,
    done_id: Option<Expr>,
}

impl IteratorAccessor {
    fn new(step: StepKind, has_completion_value: bool) -> IteratorAccessor {
        IteratorAccessor { step, has_completion_value, done_id: None }
    }

    fn read(&mut self, prereqs: &mut Prereqs, omitted: bool) -> Expr {
        let previous_done = self.done_id.clone();
        let mut step_prereqs = Prereqs::new();
        let IteratorStep { value, done } = self.step.step(&mut step_prereqs, omitted);
        let Some(previous_done) = previous_done else {
            let done_id = step_prereqs.push_to_var(done, "done");
            self.done_id = Some(done_id.clone());
            prereqs.push_list(step_prereqs.statements);
            return if omitted {
                Expr::None
            } else if self.has_completion_value {
                if_not_done(done_id, value)
            } else {
                value
            };
        };

        let value_id = (!omitted).then(|| prereqs.push_to_var_empty("value"));
        step_prereqs.push(create::assignment(previous_done.clone(), AssignmentOperator::Assign, done));
        if let Some(value_id) = &value_id {
            let value = if self.has_completion_value { if_not_done(previous_done.clone(), value) } else { value };
            step_prereqs.push(create::assignment(value_id.clone(), AssignmentOperator::Assign, value));
        }
        prereqs.push(create::if_stmt(
            create::unary(UnaryOperator::Not, previous_done),
            step_prereqs.statements,
            ElseBody::default(),
        ));
        value_id.unwrap_or(Expr::None)
    }

    fn rest(&mut self, prereqs: &mut Prereqs) -> Expr {
        let rest = prereqs.push_to_var(create::array(Vec::new()), "rest");
        let mut rest_prereqs = Prereqs::new();
        let length = rest_prereqs.push_to_var(create::number(0.0), "length");
        let mut body = Prereqs::new();
        let IteratorStep { value, done } = self.step.step(&mut body, false);
        body.push(create::if_stmt(done, vec![create::break_stmt()], ElseBody::default()));
        body.push(create::assignment(length.clone(), AssignmentOperator::Add, create::number(1.0)));
        body.push(create::assignment(
            create::computed_index(rest.clone(), length),
            AssignmentOperator::Assign,
            value,
        ));
        rest_prereqs.push(create::while_stmt(create::bool(true), body.statements));
        match &self.done_id {
            Some(done_id) => prereqs.push(create::if_stmt(
                create::unary(UnaryOperator::Not, done_id.clone()),
                rest_prereqs.statements,
                ElseBody::default(),
            )),
            None => prereqs.push_list(rest_prereqs.statements),
        }
        rest
    }
}

fn create_function_accessor(iterator: Expr, tuple: bool) -> IteratorAccessor {
    IteratorAccessor::new(StepKind::Function { iterator, tuple }, tuple)
}

/// `createCollectionAccessor`: sets and maps, walked with `next`. Keys read before the last element are recorded,
/// so a walk restarted after the destinations removed the cursor skips them.
pub struct CollectionAccessor {
    inner: IteratorAccessor,
    key: Expr,
    element_count: usize,
}

impl CollectionAccessor {
    fn new(prereqs: &mut Prereqs, parent_id: Expr, map: bool, element_count: usize) -> CollectionAccessor {
        let key = prereqs.push_to_var_empty("key");
        let inner = IteratorAccessor::new(
            StepKind::Collection { parent_id, key: key.clone(), map, consumed: None },
            map,
        );
        CollectionAccessor { inner, key, element_count }
    }

    fn consumed_mut(&mut self) -> &mut Option<Expr> {
        match &mut self.inner.step {
            StepKind::Collection { consumed, .. } => consumed,
            _ => unreachable!("a collection accessor steps through a collection"),
        }
    }

    fn read(&mut self, prereqs: &mut Prereqs, index: usize, omitted: bool) -> Expr {
        let value = self.inner.read(prereqs, omitted);
        if index + 1 == self.element_count {
            return value;
        }

        let consumed = match self.consumed_mut() {
            Some(consumed) => consumed.clone(),
            None => {
                let consumed = prereqs.push_to_var(create::array(Vec::new()), "consumed");
                *self.consumed_mut() = Some(consumed.clone());
                consumed
            }
        };
        prereqs.push(create::if_stmt(
            create::binary(self.key.clone(), BinaryOperator::Ne, Expr::Nil),
            vec![create::assignment(
                create::computed_index(consumed, self.key.clone()),
                AssignmentOperator::Assign,
                Expr::True,
            )],
            ElseBody::default(),
        ));
        value
    }
}

fn has_callable_next(state: &mut TransformState<'_>, t: P<Type>) -> bool {
    match state.checker.get_type_of_property_of_type_exported(t, "next") {
        Some(next) => !state.checker.get_call_signatures(next).is_empty(),
        None => false,
    }
}

/// `createArrayBindingAccessor`: the accessor for an array pattern `node` (an `ArrayBindingPattern` or an
/// `ArrayLiteralExpression` assignment pattern) over `parent_id`, whose type is `t`. Unsupported source types are
/// reported at `node`.
pub fn create_array_binding_accessor(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    t: P<Type>,
    parent_id: Expr,
) -> BindingAccessor {
    if is_definitely_type(state, t, &[&is_array_type]) {
        return BindingAccessor::Array { parent_id };
    }

    if is_definitely_type(state, t, &[&is_string_type]) {
        let matcher = prereqs.push_to_var(
            create::call(globals::string::gmatch(), vec![parent_id, globals::utf8::charpattern()]),
            "matcher",
        );
        return BindingAccessor::String { matcher };
    }

    if is_definitely_type(state, t, &[&is_set_type]) || is_definitely_type(state, t, &[&is_map_type]) {
        let map = is_definitely_type(state, t, &[&is_map_type]);
        return BindingAccessor::Collection(CollectionAccessor::new(prereqs, parent_id, map, node.elements().len()));
    }

    if is_definitely_type(state, t, &[&is_shared_table_type]) {
        // SharedTable supports generalized iteration, but not next()
        let source = prereqs.push_to_var(parent_id, "iterable");
        let key = temp_id("key");
        let value = temp_id("value");
        let iterator = prereqs.push_to_var(
            create::call(
                create::property(create::id("coroutine"), "wrap"),
                vec![create::function(
                    Vec::new(),
                    false,
                    vec![create::for_in(
                        vec![AnyIdentifier::Temporary(key.clone()), AnyIdentifier::Temporary(value.clone())],
                        source,
                        vec![create::call_statement(create::call(
                            globals::coroutine::r#yield(),
                            vec![Expr::TemporaryIdentifier(key), Expr::TemporaryIdentifier(value)],
                        ))],
                    )],
                )],
            ),
            "iterator",
        );
        return BindingAccessor::Iterator(create_function_accessor(iterator, true));
    }

    if is_definitely_type(state, t, &[&is_iterable_function_type]) {
        let tuple = is_definitely_type(state, t, &[&is_iterable_function_lua_tuple_type]);
        return BindingAccessor::Iterator(create_function_accessor(parent_id, tuple));
    }

    if is_definitely_type(state, t, &[&has_callable_next]) {
        let next = prereqs.push_to_var(create::property(parent_id, "next"), "next");
        return BindingAccessor::Iterator(IteratorAccessor::new(StepKind::Next { next }, true));
    }

    if is_definitely_type(state, t, &[&is_iterable_type]) {
        state.add_diagnostic(errors::no_iterable_iteration(node));
        return BindingAccessor::Unsupported;
    }

    state.add_diagnostic(errors::no_unsupported_iteration(node));
    BindingAccessor::Unsupported
}
