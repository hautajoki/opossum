//! `util/createBinaryFromOperator.ts`, `util/bitwise.ts`, `util/assignment.ts` and `util/getAssignableValue.ts`:
//! TypeScript operators as Luau expressions and assignments.

use rbxts_luau::{AssignmentOperator, BinaryOperator, Expr, Stmt, create, globals};
use tsrs_ast::{Kind, Node};
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::evaluation::effects::{effects_commute, get_effects, is_late_read};
use crate::util::traversal::skip_downwards;
use crate::util::types::{is_definitely_type, is_string_type};
use crate::util::wrap_expression_statement::wrap_expression_statement;

fn simple_operator(kind: Kind) -> Option<BinaryOperator> {
    Some(match kind {
        // comparison
        Kind::LessThanToken => BinaryOperator::Lt,
        Kind::GreaterThanToken => BinaryOperator::Gt,
        Kind::LessThanEqualsToken => BinaryOperator::Le,
        Kind::GreaterThanEqualsToken => BinaryOperator::Ge,
        Kind::EqualsEqualsEqualsToken => BinaryOperator::Eq,
        Kind::ExclamationEqualsEqualsToken => BinaryOperator::Ne,
        // math
        Kind::MinusToken => BinaryOperator::Sub,
        Kind::AsteriskToken => BinaryOperator::Mul,
        Kind::SlashToken => BinaryOperator::Div,
        Kind::AsteriskAsteriskToken => BinaryOperator::Pow,
        Kind::PercentToken => BinaryOperator::Mod,
        _ => return None,
    })
}

fn create_binary_add(
    state: &mut TransformState<'_>,
    left: Expr,
    left_type: P<Type>,
    right: Expr,
    right_type: P<Type>,
) -> Expr {
    let left_is_string = is_definitely_type(state, left_type, &[&is_string_type]);
    let right_is_string = is_definitely_type(state, right_type, &[&is_string_type]);
    if left_is_string || right_is_string {
        create::binary(
            if left_is_string { left } else { create::call(globals::tostring(), vec![left]) },
            BinaryOperator::Concat,
            if right_is_string { right } else { create::call(globals::tostring(), vec![right]) },
        )
    } else {
        create::binary(left, BinaryOperator::Add, right)
    }
}

/// `createBinaryFromOperator`
pub fn create_binary_from_operator(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    mut left: Expr,
    left_type: P<Type>,
    operator_kind: Kind,
    right: Expr,
    right_type: P<Type>,
) -> Expr {
    // arithmetic and comparison instructions can read a local after the RHS call ran
    if is_late_read(&left) && !effects_commute(&get_effects(&left), &get_effects(&right)) {
        left = prereqs.push_to_var(left, "left");
    }

    // simple
    if let Some(operator) = simple_operator(operator_kind) {
        return create::binary(left, operator, right);
    }

    // plus
    if matches!(operator_kind, Kind::PlusToken | Kind::PlusEqualsToken) {
        return create_binary_add(state, left, left_type, right, right_type);
    }

    // bitwise assignment
    if is_bitwise_operator(operator_kind) {
        return create_bitwise_call(operator_kind, vec![left, right]);
    }

    if operator_kind == Kind::CommaToken {
        prereqs.push_list(wrap_expression_statement(left));
        return right;
    }

    panic!("createBinaryFromOperator unknown operator: {operator_kind:?}");
}

// bitwise

fn bitwise_name(kind: Kind) -> Option<&'static str> {
    Some(match kind {
        // bitwise
        Kind::LessThanLessThanToken => "lshift",
        Kind::GreaterThanGreaterThanGreaterThanToken => "rshift",
        Kind::GreaterThanGreaterThanToken => "arshift",
        // bitwise compound assignment
        Kind::AmpersandEqualsToken => "band",
        Kind::BarEqualsToken => "bor",
        Kind::CaretEqualsToken => "bxor",
        Kind::LessThanLessThanEqualsToken => "lshift",
        Kind::GreaterThanGreaterThanGreaterThanEqualsToken => "rshift",
        Kind::GreaterThanGreaterThanEqualsToken => "arshift",
        _ => return None,
    })
}

fn bitwise_logical_name(kind: Kind) -> Option<&'static str> {
    Some(match kind {
        Kind::AmpersandToken => "band",
        Kind::BarToken => "bor",
        Kind::CaretToken => "bxor",
        _ => return None,
    })
}

/// `isBitwiseOperator`
pub fn is_bitwise_operator(kind: Kind) -> bool {
    bitwise_name(kind).is_some() || bitwise_logical_name(kind).is_some()
}

/// `createBitwiseCall`: `bit32.<op>(expressions)`.
pub fn create_bitwise_call(operator_kind: Kind, expressions: Vec<Expr>) -> Expr {
    let name = bitwise_name(operator_kind)
        .or_else(|| bitwise_logical_name(operator_kind))
        .unwrap_or_else(|| panic!("createBitwiseFromOperator unknown operator: {operator_kind:?}"));
    create::call(create::property(globals::bit32(), name), expressions)
}

fn flatten_bitwise_segment_into(list: &mut Vec<P<Node>>, operator_kind: Kind, node: P<Node>) {
    if node.kind() == Kind::BinaryExpression {
        let binary = node.as_binary_expression();
        if binary.operator_token.kind() == operator_kind {
            flatten_bitwise_segment_into(list, operator_kind, binary.left);
            list.push(skip_downwards(binary.right()));
        } else {
            list.push(node);
        }
        return;
    }
    list.push(skip_downwards(node));
}

/// `createBitwiseFromOperator`: `a | b | c` → `bit32.bor(a, b, c)`.
pub fn create_bitwise_from_operator(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    operator_kind: Kind,
    node: P<Node>,
) -> Expr {
    let binary = node.as_binary_expression();
    create_bitwise_from_operands(state, prereqs, operator_kind, binary.left, binary.right())
}

/// `createBitwiseFromOperator` on the operands of `left op right` (a left operand of the same logical bitwise
/// operator is flattened into the call).
pub fn create_bitwise_from_operands(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    operator_kind: Kind,
    left: P<Node>,
    right: P<Node>,
) -> Expr {
    let mut flattened = Vec::new();
    if bitwise_logical_name(operator_kind).is_some() {
        flatten_bitwise_segment_into(&mut flattened, operator_kind, left);
        flattened.push(right);
    } else {
        flattened.push(left);
        flattened.push(right);
    }
    let expressions = ensure_transform_order(state, prereqs, &flattened);
    create_bitwise_call(operator_kind, expressions)
}

// assignment

fn compound_operator(kind: Kind) -> Option<AssignmentOperator> {
    Some(match kind {
        // compound assignment
        Kind::MinusEqualsToken => AssignmentOperator::Sub,
        Kind::AsteriskEqualsToken => AssignmentOperator::Mul,
        Kind::SlashEqualsToken => AssignmentOperator::Div,
        Kind::AsteriskAsteriskEqualsToken => AssignmentOperator::Pow,
        Kind::PercentEqualsToken => AssignmentOperator::Mod,
        // unary
        Kind::PlusPlusToken => AssignmentOperator::Add,
        Kind::MinusMinusToken => AssignmentOperator::Sub,
        // normal assignment
        Kind::EqualsToken => AssignmentOperator::Assign,
        _ => return None,
    })
}

/// `getSimpleAssignmentOperator`: the Luau operator for an assignment, if it has one (`+=` is `..=` for strings).
pub fn get_simple_assignment_operator(
    state: &mut TransformState<'_>,
    left_type: P<Type>,
    operator_kind: Kind,
    right_type: P<Type>,
) -> Option<AssignmentOperator> {
    if operator_kind == Kind::PlusEqualsToken {
        let is_string = is_definitely_type(state, left_type, &[&is_string_type])
            || is_definitely_type(state, right_type, &[&is_string_type]);
        return Some(if is_string { AssignmentOperator::Concat } else { AssignmentOperator::Add });
    }
    compound_operator(operator_kind)
}

fn binary_of_assignment(operator: AssignmentOperator) -> BinaryOperator {
    // `operator.slice(0, -1)`
    BinaryOperator::parse(&operator.as_str()[..operator.as_str().len() - 1])
        .expect("a compound assignment operator has a binary form")
}

/// `createAssignmentStatement`. `readable_is_writable` stands for TypeScript's `readable === writable`.
pub fn create_assignment_statement(
    writable: Expr,
    operator: AssignmentOperator,
    value: Expr,
    readable: Expr,
    readable_is_writable: bool,
) -> Stmt {
    if readable_is_writable {
        create::assignment(writable, operator, value)
    } else {
        create::assignment(
            writable,
            AssignmentOperator::Assign,
            create::binary(readable, binary_of_assignment(operator), value),
        )
    }
}

/// `createAssignmentExpression`: the assignment as a prerequisite; the value is the target.
pub fn create_assignment_expression(
    prereqs: &mut Prereqs,
    writable: Expr,
    operator: AssignmentOperator,
    value: Expr,
    readable: Option<Expr>,
) -> Expr {
    let readable_is_writable = readable.is_none();
    let readable = readable.unwrap_or_else(|| writable.clone());
    prereqs.push(create_assignment_statement(writable.clone(), operator, value, readable, readable_is_writable));
    writable
}

/// `createCompoundAssignmentStatement`
#[allow(clippy::too_many_arguments)]
pub fn create_compound_assignment_statement(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    writable: Expr,
    writable_type: P<Type>,
    readable: Expr,
    operator: Kind,
    value: Expr,
    value_type: P<Type>,
) -> Stmt {
    let right = create_binary_from_operator(state, prereqs, readable, writable_type, operator, value, value_type);
    create::assignment(writable, AssignmentOperator::Assign, right)
}

/// `createCompoundAssignmentExpression`
#[allow(clippy::too_many_arguments)]
pub fn create_compound_assignment_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    writable: Expr,
    writable_type: P<Type>,
    readable: Expr,
    operator: Kind,
    value: Expr,
    value_type: P<Type>,
) -> Expr {
    let right = create_binary_from_operator(state, prereqs, readable, writable_type, operator, value, value_type);
    create_assignment_expression(prereqs, writable, AssignmentOperator::Assign, right, None)
}

/// `getAssignableValue`: `..=` needs a string on the right.
pub fn get_assignable_value(
    state: &mut TransformState<'_>,
    operator: AssignmentOperator,
    value: Expr,
    value_type: P<Type>,
) -> Expr {
    if operator == AssignmentOperator::Concat && !is_definitely_type(state, value_type, &[&is_string_type]) {
        return create::call(globals::tostring(), vec![value]);
    }
    value
}
