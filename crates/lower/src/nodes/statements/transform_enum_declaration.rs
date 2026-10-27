//! `nodes/statements/transformEnumDeclaration.ts`

use rbxts_luau::{AssignmentOperator, Expr, Stmt, create, globals};
use tsrs_ast::{Kind, ModifierFlags, Node};
use tsrs_checker::LiteralValue;
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, as_any_identifier, temp_id};
use crate::classes::diagnostic_service::{OnceCache, ReportOnce};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::nodes::transform_property_name::transform_property_name;
use crate::shared::diagnostics::errors;
use crate::util::create_string_from_literal::create_string_from_value;
use crate::util::expression_might_mutate::expression_might_mutate;
use crate::util::has_multiple_definitions::has_multiple_definitions;
use crate::util::symbols::validate_identifier;
use crate::util::traversal::has_syntactic_modifier;

/// `state.typeChecker.getConstantValue(member)`.
///
/// rbxts-rs: TypeScript 7 answers from the member's enum value (`get_enum_member_value`); a computed member has none.
fn get_constant_value(state: &mut TransformState<'_>, member: P<Node>) -> Option<LiteralValue> {
    state.checker.get_constant_value(member)
}

/// `needsInverseEntry`: every member but a string-valued one gets a reverse mapping.
fn needs_inverse_entry(state: &mut TransformState<'_>, member: P<Node>) -> bool {
    !matches!(get_constant_value(state, member), Some(LiteralValue::String(_)))
}

/// `transformEnumDeclaration`
pub fn transform_enum_declaration(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    if has_syntactic_modifier(node, ModifierFlags::Const) && !state.compiler_options.preserve_const_enums.is_true() {
        return Vec::new();
    }

    let declaration = node.as_enum_declaration();
    let symbol = state.checker.get_symbol_at_location_exported(declaration.name);
    if let Some(symbol) = symbol
        && has_multiple_definitions(symbol, |d| {
            d.kind() == Kind::EnumDeclaration && !has_syntactic_modifier(d, ModifierFlags::Const)
        })
    {
        // `DiagnosticService.addDiagnosticWithCache`
        let key = ReportOnce::new(OnceCache::MultipleDefinitions, symbol, state.source_file);
        if state.multi_transform_state.reported_once.insert(key) {
            state.diagnostics.add_diagnostic_once(key, errors::no_enum_merging(node));
        }
        return Vec::new();
    }

    validate_identifier(state, declaration.name);

    let left = transform_identifier_defined(state, declaration.name);
    let is_hoisted = symbol.is_some_and(|s| state.is_hoisted.get(&s) == Some(&true));
    let mut statements = Vec::new();
    let members = declaration.members.nodes();

    if members.iter().all(|&member| !needs_inverse_entry(state, member)) {
        let mut prereqs = Prereqs::new();
        let mut fields = Vec::with_capacity(members.len());
        for &member in members {
            let name = transform_property_name(state, &mut prereqs, member.name().expect("an enum member has a name"));
            let key = prereqs.push_to_var_if_complex(name, "");
            let Some(LiteralValue::String(value)) = get_constant_value(state, member) else {
                unreachable!("every member has a string value")
            };
            // rbxts-rs: fixes roblox-ts passing the decoded value to `luau.string`, which renders it as source text
            fields.push((key, create_string_from_value(value)));
        }
        statements.extend(prereqs.statements);
        let right = create::map(fields);
        statements.push(if is_hoisted {
            create::assignment(left, AssignmentOperator::Assign, right)
        } else {
            create::local(as_any_identifier(left), Some(right))
        });
        return statements;
    }

    let mut enum_statements = Vec::new();
    let inverse_id = Expr::TemporaryIdentifier(temp_id("inverse"));
    enum_statements.push(create::local(as_any_identifier(inverse_id.clone()), Some(create::map(Vec::new()))));
    enum_statements.push(create::assignment(
        left.clone(),
        AssignmentOperator::Assign,
        create::call(
            globals::setmetatable(),
            vec![create::map(Vec::new()), create::map(vec![(globals::strings::__index(), inverse_id.clone())])],
        ),
    ));

    for &member in members {
        let mut member_prereqs = Prereqs::new();
        let member_name = member.name().expect("an enum member has a name");
        let name = transform_property_name(state, &mut member_prereqs, member_name);
        let name_node =
            if member_name.kind() == Kind::ComputedPropertyName { member_name.expression() } else { Some(member_name) };
        let index = if expression_might_mutate(state, &name, name_node) {
            // not pushToVarIfComplex: an identifier must be captured too, since computing the value might reassign it
            member_prereqs.push_to_var(name, "")
        } else {
            name
        };

        let value_exp = match get_constant_value(state, member) {
            // rbxts-rs: the value, not source text (`create_string_from_value`)
            Some(LiteralValue::String(value)) => create_string_from_value(value),
            Some(LiteralValue::Number(value)) => create::number(value.0),
            _ => {
                // the constant value is always a number without an initializer
                let initializer = member.initializer().expect("a computed enum member has an initializer");
                let value = transform_expression(state, &mut member_prereqs, initializer);
                member_prereqs.push_to_var_if_complex(value, "value")
            }
        };

        enum_statements.extend(member_prereqs.statements);
        enum_statements.push(create::assignment(
            create::computed_index(left.clone(), index.clone()),
            AssignmentOperator::Assign,
            value_exp.clone(),
        ));

        if needs_inverse_entry(state, member) {
            enum_statements.push(create::assignment(
                create::computed_index(inverse_id.clone(), value_exp),
                AssignmentOperator::Assign,
                index,
            ));
        }
    }

    statements.push(create::do_stmt(enum_statements));
    if !is_hoisted {
        statements.insert(0, create::local(as_any_identifier(left), None));
    }
    statements
}
