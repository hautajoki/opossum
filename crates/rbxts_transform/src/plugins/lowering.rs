//! The lowering primitives that are not calls: `compile_binary` (an operator on original operands) and
//! `compile_object_literal` (an object literal with field edits), both on roblox-ts's own lowerings.

use rbxts_luau::{BinaryOperator, Effects, Expr, MapField, Stmt, create};
use rbxts_plugin::{BinOp, FieldOrder, ObjectEdits};
use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::expressions::transform_object_literal_expression::{FieldEdit, transform_object_literal_with};
use crate::nodes::transform_logical::{flatten_logical_operands, transform_logical_operands};
use crate::nodes::transform_property_name::transform_property_name;
use crate::shared::diagnostics::errors;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::create_binary_from_operator::{
    create_binary_from_operator, create_bitwise_from_operands, is_bitwise_operator,
};
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::evaluation::effects::{effects_commute, get_effects, get_effects_of_statements, join_effects};
use crate::util::types::{
    is_boolean_literal_type, is_definitely_type, is_number_type, is_possibly_type, is_roblox_type, is_string_type,
};
use crate::util::validate_method_assignment::validate_method_assignment;
use crate::util::validate_not_any::validate_not_any_type;

// --- compile_binary ---

/// The operators `compile_binary` lowers: roblox-ts's binary operators, without assignments and without `==` / `!=`
/// (which roblox-ts rejects).
fn binary_operator_supported(op: Kind) -> bool {
    matches!(
        op,
        Kind::AmpersandAmpersandToken
            | Kind::BarBarToken
            | Kind::QuestionQuestionToken
            | Kind::InKeyword
            | Kind::InstanceOfKeyword
            | Kind::PlusToken
            | Kind::MinusToken
            | Kind::AsteriskToken
            | Kind::SlashToken
            | Kind::AsteriskAsteriskToken
            | Kind::PercentToken
            | Kind::LessThanToken
            | Kind::GreaterThanToken
            | Kind::LessThanEqualsToken
            | Kind::GreaterThanEqualsToken
            | Kind::EqualsEqualsEqualsToken
            | Kind::ExclamationEqualsEqualsToken
            | Kind::CommaToken
    ) || (is_bitwise_operator(op) && !tsrs_ast::is_assignment_operator(op))
}

/// The type of `left ?? right`: the checker's own when they are the operands of a `??` or `??=` in the source,
/// else as the checker computes it (`checkBinaryLikeExpression`): the left type when it has no `null` or
/// `undefined`, else the subtype-reduced union of the non-nullable left type and the right type.
fn coalesce_type(state: &mut TransformState<'_>, left: P<Node>, right: P<Node>) -> P<tsrs_checker::Type> {
    if let Some(parent) = left.parent()
        && parent.kind() == Kind::BinaryExpression
        && parent.as_binary_expression().left == left
        && parent.as_binary_expression().right() == right
        && matches!(
            parent.as_binary_expression().operator_token.kind(),
            Kind::QuestionQuestionToken | Kind::QuestionQuestionEqualsToken
        )
    {
        return state.get_type(parent);
    }
    let left_type = state.get_type(left);
    let non_nullable = state.checker.get_non_nullable_type(left_type);
    if non_nullable == left_type {
        return left_type;
    }
    let right_type = state.get_type(right);
    state.checker.get_union_type_ex_exported(&[non_nullable, right_type], tsrs_checker::UnionReduction::Subtype)
}

/// `compile_binary`: `left op right` lowered as `transformBinaryExpression` lowers a binary expression with these
/// operands (a left operand of the same logical or bitwise operator is flattened into the chain, as it would be).
/// `anchor` (the hook's node) carries the diagnostics the binary expression would.
pub(crate) fn lower_binary(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    op: BinOp,
    left: P<Node>,
    right: P<Node>,
    anchor: P<Node>,
) -> Result<Expr, String> {
    if tsrs_ast::is_assignment_operator(op) {
        return Err(format!(
            "compile_binary: {op:?} is an assignment operator; pass the operator it applies (`+=` is PlusToken)"
        ));
    }
    if !binary_operator_supported(op) {
        return Err(format!(
            "compile_binary: {op:?} is not a binary operator roblox-ts lowers (`==` / `!=` are rejected: use \
             EqualsEqualsEqualsToken / ExclamationEqualsEqualsToken)"
        ));
    }
    validate_not_any_type(state, left);
    validate_not_any_type(state, right);

    if matches!(op, Kind::AmpersandAmpersandToken | Kind::BarBarToken | Kind::QuestionQuestionToken) {
        let mut operands = flatten_logical_operands(left, op);
        operands.push(right);
        let coalesce_possibly_false = op == Kind::QuestionQuestionToken && {
            let t = coalesce_type(state, left, right);
            is_possibly_type(state, t, &[&is_boolean_literal_type(false)])
        };
        return Ok(transform_logical_operands(state, prereqs, op, operands, coalesce_possibly_false));
    }

    if is_bitwise_operator(op) {
        return Ok(create_bitwise_from_operands(state, prereqs, op, left, right));
    }

    let mut ordered = ensure_transform_order(state, prereqs, &[left, right]).into_iter();
    let left_value = ordered.next().expect("left");
    let right_value = ordered.next().expect("right");

    if op == Kind::InKeyword {
        return Ok(create::binary(
            create::computed_index(convert_to_indexable_expression(right_value), left_value),
            BinaryOperator::Ne,
            Expr::Nil,
        ));
    } else if op == Kind::InstanceOfKeyword {
        let right_type = state.get_type(right);
        if is_possibly_type(state, right_type, &[&is_roblox_type]) {
            state.add_diagnostic(errors::no_roblox_symbol_instanceof(right));
        }
        let callee = state.ts(anchor, "instanceof");
        return Ok(create::call(callee, vec![left_value, right_value]));
    }

    let left_type = state.get_type(left);
    let right_type = state.get_type(right);
    if matches!(
        op,
        Kind::LessThanToken | Kind::LessThanEqualsToken | Kind::GreaterThanToken | Kind::GreaterThanEqualsToken
    ) {
        let left_ok = is_definitely_type(state, left_type, &[&is_string_type])
            || is_definitely_type(state, left_type, &[&is_number_type]);
        // as transformBinaryExpression (which checks the left type's number-ness twice)
        let right_ok = is_definitely_type(state, right_type, &[&is_string_type])
            || is_definitely_type(state, left_type, &[&is_number_type]);
        if !left_ok || !right_ok {
            state.add_diagnostic(errors::no_non_number_string_relation_operator(anchor));
        }
    }
    Ok(create_binary_from_operator(state, prereqs, left_value, left_type, op, right_value, right_type))
}

// --- compile_object_literal ---

/// The key a property is written with, for edits: an identifier, string literal or numeric literal name (as
/// written). `None` for computed keys, spreads and accessors.
fn written_key(property: P<Node>) -> Option<&'static str> {
    if !matches!(
        property.kind(),
        Kind::PropertyAssignment | Kind::ShorthandPropertyAssignment | Kind::MethodDeclaration
    ) {
        return None;
    }
    let name = property.name()?;
    matches!(name.kind(), Kind::Identifier | Kind::StringLiteral | Kind::NumericLiteral).then(|| name.text())
}

/// The property a written key names: exactly one, or an error.
fn field_index(properties: &[P<Node>], key: &str, what: &str) -> Result<usize, String> {
    let mut found = properties.iter().enumerate().filter(|(_, p)| written_key(**p) == Some(key)).map(|(i, _)| i);
    let first = found
        .next()
        .ok_or_else(|| format!("compile_object_literal: {what} `{key}`, which is not a written key of the literal"))?;
    if found.next().is_some() {
        return Err(format!("compile_object_literal: {what} `{key}`, which the literal writes more than once"));
    }
    Ok(first)
}

/// `compile_object_literal`: the literal lowered as roblox-ts lowers object literals, with `edits` applied: renamed
/// keys, replaced values (the written value is not evaluated), fields appended after the written ones, and an
/// emitted order. Spreads, methods and computed keys are kept and lowered as usual; edits name fields by their
/// written key (an identifier, string or numeric literal name).
pub(crate) fn lower_object_literal(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    edits: ObjectEdits,
) -> Result<Expr, String> {
    if node.kind() != Kind::ObjectLiteralExpression {
        return Err(format!("compile_object_literal takes an ObjectLiteralExpression, not a {:?}", node.kind()));
    }
    if tsrs_ast::get_assignment_target(node).is_some() {
        return Err("compile_object_literal: this object literal is a destructuring assignment target".to_owned());
    }
    let properties = node.properties();
    let mut fields = vec![FieldEdit::default(); properties.len()];
    for (key, emitted) in edits.rename {
        let i = field_index(properties, &key, "rename names")?;
        if properties[i].kind() == Kind::MethodDeclaration {
            return Err(format!("compile_object_literal: rename names `{key}`, a method (only fields can be edited)"));
        }
        if fields[i].key.replace(emitted).is_some() {
            return Err(format!("compile_object_literal: `{key}` is renamed twice"));
        }
    }
    for (key, value) in edits.replace {
        let i = field_index(properties, &key, "replace names")?;
        if properties[i].kind() == Kind::MethodDeclaration {
            return Err(format!("compile_object_literal: replace names `{key}`, a method (only fields can be edited)"));
        }
        if fields[i].value.replace(value).is_some() {
            return Err(format!("compile_object_literal: `{key}` is replaced twice"));
        }
    }
    match edits.order {
        FieldOrder::Source => Ok(transform_object_literal_with(state, prereqs, node, &mut fields, edits.append)),
        FieldOrder::Permuted(order) => lower_permuted(state, prereqs, node, fields, edits.append, &order),
    }
}

/// An object literal emitted in another order than written: every key and value is compiled in source order (each
/// with its own prerequisites), those a later one's evaluation could change are bound to locals (as
/// `ensureTransformOrder` binds arguments), and the map is emitted in `order`. Spreads and methods have no place
/// in a reordered map, so they are refused.
fn lower_permuted(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    mut fields: Vec<FieldEdit>,
    append: Vec<(Expr, Expr)>,
    order: &[u32],
) -> Result<Expr, String> {
    let properties = node.properties();
    let count = properties.len() + append.len();
    let mut seen = vec![false; count];
    if order.len() != count
        || order.iter().any(|&i| (i as usize) >= count || std::mem::replace(&mut seen[i as usize], true))
    {
        return Err(format!(
            "compile_object_literal: FieldOrder::Permuted must list each of the {count} fields (written, then \
             appended) once"
        ));
    }
    // each slot: (value, its prerequisites); a field is two slots, key then value
    let mut slots: Vec<(Expr, Vec<Stmt>)> = Vec::with_capacity(count * 2);
    for (i, &property) in properties.iter().enumerate() {
        validate_method_assignment(state, property);
        let (name, initializer) = match property.kind() {
            Kind::PropertyAssignment => (
                property.name().expect("a property assignment has a name"),
                property.initializer().expect("a property assignment has an initializer"),
            ),
            Kind::ShorthandPropertyAssignment => {
                let name = property.name().expect("a shorthand property assignment has a name");
                (name, name)
            }
            Kind::SpreadAssignment | Kind::MethodDeclaration => {
                return Err(
                    "compile_object_literal: FieldOrder::Permuted cannot reorder a literal with spreads or methods"
                        .to_owned(),
                );
            }
            // an accessor, which roblox-ts rejects
            _ => {
                state.add_diagnostic(errors::no_getter_setter(property));
                return Ok(Expr::None);
            }
        };
        if name.kind() == Kind::PrivateIdentifier {
            state.add_diagnostic(errors::no_private_identifier(name));
            return Ok(Expr::None);
        }
        let edit = std::mem::take(&mut fields[i]);
        let mut key_prereqs = Prereqs::new();
        let key = match edit.key {
            Some(key) => key,
            None => transform_property_name(state, &mut key_prereqs, name),
        };
        slots.push((key, key_prereqs.statements));
        let mut value_prereqs = Prereqs::new();
        let value = match edit.value {
            Some(value) => value,
            None => transform_expression(state, &mut value_prereqs, initializer),
        };
        slots.push((value, value_prereqs.statements));
    }
    for (key, value) in append {
        slots.push((key, Vec::new()));
        slots.push((value, Vec::new()));
    }
    // bind each value that does not commute with everything evaluated after it in source order, every later value
    // included (unlike `ensureTransformOrder`, whose values stay in source order): the values left inline then
    // commute with each other and with the later prerequisites, so any emitted order evaluates the same
    let mut captures = vec![false; slots.len()];
    let mut suffix = Effects::none();
    for i in (0..slots.len()).rev() {
        let (expression, statements) = &slots[i];
        let effects = get_effects(expression);
        captures[i] = !effects_commute(&effects, &suffix);
        suffix = join_effects(effects, &suffix);
        suffix = join_effects(get_effects_of_statements(statements), &suffix);
    }
    let mut values = Vec::with_capacity(slots.len());
    for ((expression, statements), capture) in slots.into_iter().zip(captures) {
        prereqs.push_list(statements);
        values.push(if capture { prereqs.push_to_var(expression, "field") } else { expression });
    }
    let mut pairs: Vec<Option<(Expr, Expr)>> = Vec::with_capacity(count);
    let mut values = values.into_iter();
    while let (Some(key), Some(value)) = (values.next(), values.next()) {
        pairs.push(Some((key, value)));
    }
    let fields = order
        .iter()
        .map(|&i| {
            let (index, value) = pairs[i as usize].take().expect("checked to be a permutation");
            MapField { index, value }
        })
        .collect::<Vec<_>>();
    Ok(Expr::Map(fields))
}
