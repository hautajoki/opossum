//! `nodes/statements/transformForStatement.ts`: `for (init; condition; incrementor)` as a numeric `for` loop when
//! `--optimizedLoops` allows it, otherwise as a `while` loop.

use rbxts_luau::{AssignmentOperator, ElseBody, Expr, IfStatement, Stmt, UnaryOperator, create};
use rustc_hash::FxHashSet;
use tsrs_ast::{Kind, Node, NodeFlags, Symbol};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, as_any_identifier, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::nodes::statements::transform_expression_statement::transform_expression_statement_inner;
use crate::nodes::statements::transform_variable_statement::{is_var_declaration, transform_variable_declaration};
use crate::nodes::transform_statement_list::transform_body;
use crate::shared::diagnostics::errors;
use crate::util::create_truthiness_checks::create_truthiness_checks;
use crate::util::get_constant_integer::get_constant_integer;
use crate::util::get_declared_variables::get_declared_variables;
use crate::util::get_literal_number_value::get_literal_number_value;
use crate::util::offset::offset;
use crate::util::traversal::{get_ancestor, is_ancestor_of, skip_downwards, source_file_of};

fn add_finalizers_to_if_statement(node: &mut IfStatement, finalizers: &[Stmt]) {
    if !node.statements.is_empty() {
        add_finalizers(&mut node.statements, finalizers);
    }
    match &mut node.else_body {
        ElseBody::Block(statements) => {
            if !statements.is_empty() {
                add_finalizers(statements, finalizers);
            }
        }
        ElseBody::If(else_if) => add_finalizers_to_if_statement(else_if, finalizers),
    }
}

/// `addFinalizers`: a copy of `finalizers` before every `continue` of the loop body (through `do` and `if` blocks).
fn add_finalizers(list: &mut Vec<Stmt>, finalizers: &[Stmt]) {
    let mut i = 0;
    while i < list.len() {
        match &mut list[i] {
            Stmt::Continue => {
                list.splice(i..i, finalizers.iter().cloned());
                i += finalizers.len();
            }
            Stmt::Do(statement) => {
                if !statement.statements.is_empty() {
                    add_finalizers(&mut statement.statements, finalizers);
                }
            }
            Stmt::If(statement) => add_finalizers_to_if_statement(statement, finalizers),
            _ => {}
        }
        i += 1;
    }
}

/// `ts.FindAllReferences.Core.eachSymbolReferenceInFile(definition, …, searchContainer)`: the references to the
/// definition's symbol inside `container`, besides the definition itself.
///
/// rbxts-rs: through the checker's in-file reference search, as in `checkVariableHoist`.
fn symbol_references_in(state: &mut TransformState<'_>, definition: P<Node>, container: P<Node>) -> Vec<P<Node>> {
    let Some(symbol) = state.checker.get_symbol_at_location_exported(definition) else { return Vec::new() };
    let file = source_file_of(definition);
    state
        .checker
        .get_references_to_symbol_in_file(file, symbol)
        .into_iter()
        .filter(|&token| token != definition && is_ancestor_of(container, token))
        .collect()
}

/// `canSkipClone`: the symbol is not used in the initializer (besides its definition).
fn can_skip_clone(state: &mut TransformState<'_>, initializer: P<Node>, id: P<Node>) -> bool {
    symbol_references_in(state, id, initializer).is_empty()
}

fn is_function_like(node: P<Node>) -> bool {
    tsrs_ast::is_function_like(Some(node))
}

/// `isIdWriteOrAsyncRead`: the variable is written outside the incrementor, or read from a function in the loop.
fn is_id_write_or_async_read(state: &mut TransformState<'_>, for_statement: P<Node>, id: P<Node>) -> bool {
    let incrementor = for_statement.as_for_statement().incrementor;
    symbol_references_in(state, id, for_statement).into_iter().any(|token| {
        // write
        if tsrs_ast::is_write_access(token) && incrementor.is_none_or(|i| !is_ancestor_of(i, token)) {
            return true;
        }

        // async read
        let ancestor = get_ancestor(token, |v| v == for_statement || is_function_like(v));
        ancestor.is_some_and(|a| a != for_statement)
    })
}

fn symbol_of(state: &mut TransformState<'_>, id: P<Node>) -> P<Symbol> {
    state.checker.get_symbol_at_location_exported(id).expect("a declared variable has a symbol")
}

/// `transformForStatementFallback`
fn transform_for_statement_fallback(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    let for_statement = node.as_for_statement();
    let (initializer, condition, incrementor, statement) =
        (for_statement.initializer, for_statement.condition, for_statement.incrementor, for_statement.statement());

    let mut result = Vec::new();
    let mut while_statements = Vec::new();
    let mut finalizer_statements = Vec::new();

    let declaration_list = initializer.filter(|i| i.kind() == Kind::VariableDeclarationList);
    let variables = declaration_list.map(get_declared_variables).unwrap_or_default();
    let mut has_write_or_async_read = FxHashSet::default();
    let mut skip_clone = FxHashSet::default();

    if let Some(declaration_list) = declaration_list {
        for &id in &variables {
            let symbol = symbol_of(state, id);
            if is_id_write_or_async_read(state, node, id) {
                has_write_or_async_read.insert(symbol);
            }
            if can_skip_clone(state, declaration_list, id) {
                skip_clone.insert(symbol);
            }
        }
    }

    if let Some(initializer) = initializer {
        if let Some(declaration_list) = declaration_list {
            if is_var_declaration(declaration_list) {
                state.add_diagnostic(errors::no_var(node));
            }

            for &id in &variables {
                let symbol = symbol_of(state, id);
                if has_write_or_async_read.contains(&symbol) {
                    let text = id.text();
                    let temp =
                        if skip_clone.contains(&symbol) { temp_id(text) } else { temp_id(format!("{text}Copy")) };
                    state.symbol_to_id_map.insert(symbol, temp);
                }
            }

            for &declaration in declaration_list.as_variable_declaration_list().declarations.nodes() {
                result.extend(transform_variable_declaration(state, declaration));
            }

            for &id in &variables {
                let symbol = symbol_of(state, id);
                if has_write_or_async_read.contains(&symbol) {
                    let temp = if skip_clone.contains(&symbol) {
                        state.symbol_to_id_map.get(&symbol).cloned().expect("the loop variable's temporary was set")
                    } else {
                        let temp = temp_id(id.text());
                        let copy_id =
                            state.symbol_to_id_map.get(&symbol).cloned().expect("the loop variable's copy was set");

                        // local _i = _iCopy
                        result.push(create::local(temp.clone(), Some(Expr::TemporaryIdentifier(copy_id))));
                        temp
                    };
                    state.symbol_to_id_map.remove(&symbol);
                    let real_id = transform_identifier_defined(state, id);

                    // local i = _i
                    while_statements.push(create::local(
                        as_any_identifier(real_id.clone()),
                        Some(Expr::TemporaryIdentifier(temp.clone())),
                    ));

                    // _i = i
                    finalizer_statements.push(create::assignment(
                        Expr::TemporaryIdentifier(temp),
                        AssignmentOperator::Assign,
                        real_id,
                    ));
                }
            }
        } else {
            result.extend(transform_expression_statement_inner(state, initializer));
        }
    }

    if let Some(incrementor) = incrementor {
        let should_increment = temp_id("shouldIncrement");

        // local _shouldIncrement = false
        result.push(create::local(should_increment.clone(), Some(create::bool(false))));

        let incrementor_statements = transform_expression_statement_inner(state, incrementor);

        // if _shouldIncrement then
        // 	[incrementorStatements]
        // else
        // 	_shouldIncrement = true
        // end
        while_statements.push(create::if_stmt(
            Expr::TemporaryIdentifier(should_increment.clone()),
            incrementor_statements,
            ElseBody::Block(vec![create::assignment(
                Expr::TemporaryIdentifier(should_increment),
                AssignmentOperator::Assign,
                create::bool(true),
            )]),
        ));
    }

    let mut condition_prereqs = Prereqs::new();
    let mut condition_exp = match condition {
        Some(condition) => {
            let expression = transform_expression(state, &mut condition_prereqs, condition);
            create_truthiness_checks(state, &mut condition_prereqs, expression, condition)
        }
        None => create::bool(true),
    };

    while_statements.extend(condition_prereqs.statements);

    if !while_statements.is_empty() {
        if condition.is_some() {
            // if not [conditionExp] then
            //	break
            // end
            while_statements.push(create::if_stmt(
                create::unary(UnaryOperator::Not, condition_exp),
                vec![create::break_stmt()],
                ElseBody::default(),
            ));
        }
        condition_exp = create::bool(true);
    }

    while_statements.extend(transform_body(state, statement));

    if !while_statements.is_empty() && !finalizer_statements.is_empty() {
        add_finalizers(&mut while_statements, &finalizer_statements);
    }

    if while_statements.last().is_none_or(|last| !last.is_final()) {
        while_statements.extend(finalizer_statements);
    }

    result.push(create::while_stmt(condition_exp, while_statements));

    if result.len() == 1 { result } else { vec![create::do_stmt(result)] }
}

/// `isLoopVariable`
fn is_loop_variable(state: &mut TransformState<'_>, expression: P<Node>, symbol: P<Symbol>) -> bool {
    let node = skip_downwards(expression);
    node.kind() == Kind::Identifier && state.checker.get_symbol_at_location_exported(node) == Some(symbol)
}

/// `OptimizedStep`
struct OptimizedStep {
    value: f64,
    expression: Option<P<Node>>,
    negate: bool,
}

/// `getOptimizedIncrementorStep`: a constant step of `i++`, `i--`, `i += n`, `i -= n`, `i = i + n`, `i = n + i` or
/// `i = i - n`.
fn get_optimized_incrementor_step(
    state: &mut TransformState<'_>,
    incrementor: P<Node>,
    id_symbol: P<Symbol>,
) -> Option<OptimizedStep> {
    let incrementor = skip_downwards(incrementor);
    if incrementor.kind() == Kind::BinaryExpression
        && is_loop_variable(state, incrementor.as_binary_expression().left, id_symbol)
    {
        let mut update = incrementor;
        let incrementor_operator = incrementor.as_binary_expression().operator_token.kind();
        if incrementor_operator == Kind::EqualsToken {
            let right = skip_downwards(incrementor.as_binary_expression().right());
            if right.kind() != Kind::BinaryExpression {
                return None;
            }
            update = right;
        } else if incrementor_operator != Kind::PlusEqualsToken && incrementor_operator != Kind::MinusEqualsToken {
            return None;
        }

        let update = update.as_binary_expression();
        let operator = update.operator_token.kind();
        let negate = match operator {
            Kind::PlusToken | Kind::PlusEqualsToken => false,
            Kind::MinusToken | Kind::MinusEqualsToken => true,
            _ => return None,
        };

        let expression = if is_loop_variable(state, update.left, id_symbol) {
            update.right()
        } else if operator == Kind::PlusToken && is_loop_variable(state, update.right(), id_symbol) {
            update.left
        } else {
            return None;
        };

        let value = get_constant_integer(state, expression, true)?;
        return Some(OptimizedStep {
            value: if negate { -value } else { value },
            expression: Some(expression),
            negate,
        });
    }

    let (operand, operator) = match incrementor.kind() {
        Kind::PostfixUnaryExpression => {
            let unary = incrementor.as_postfix_unary_expression();
            (unary.operand, unary.operator)
        }
        Kind::PrefixUnaryExpression => {
            let unary = incrementor.as_prefix_unary_expression();
            (unary.operand, unary.operator)
        }
        _ => return None,
    };
    if !is_loop_variable(state, operand, id_symbol) {
        return None;
    }
    match operator {
        Kind::PlusPlusToken => Some(OptimizedStep { value: 1.0, expression: None, negate: false }),
        Kind::MinusMinusToken => Some(OptimizedStep { value: -1.0, expression: None, negate: false }),
        _ => None,
    }
}

/// `getOptimizedCondition`: the bound and comparison of `i < n` (or `n > i`, flipped).
fn get_optimized_condition(
    state: &mut TransformState<'_>,
    condition: P<Node>,
    id_symbol: P<Symbol>,
) -> Option<(P<Node>, Kind)> {
    let condition = skip_downwards(condition);
    if condition.kind() != Kind::BinaryExpression {
        return None;
    }
    let binary = condition.as_binary_expression();

    let mut bound = binary.right();
    let mut operator = binary.operator_token.kind();
    if !is_loop_variable(state, binary.left, id_symbol) {
        if !is_loop_variable(state, binary.right(), id_symbol) {
            return None;
        }

        bound = binary.left;
        operator = match operator {
            Kind::LessThanToken => Kind::GreaterThanToken,
            Kind::LessThanEqualsToken => Kind::GreaterThanEqualsToken,
            Kind::GreaterThanToken => Kind::LessThanToken,
            Kind::GreaterThanEqualsToken => Kind::LessThanEqualsToken,
            operator => operator,
        };
    }

    Some((bound, operator))
}

/// `isMutatedInBody`
fn is_mutated_in_body(state: &mut TransformState<'_>, identifier: P<Node>, body: P<Node>) -> bool {
    symbol_references_in(state, identifier, body).into_iter().any(tsrs_ast::is_write_access)
}

/// `transformForStatementOptimized`: `for i = start, end, step do`, when the loop variable is a `let` with constant
/// integer start, bound and step, and the body never writes it.
fn transform_for_statement_optimized(state: &mut TransformState<'_>, node: P<Node>) -> Option<Vec<Stmt>> {
    let for_statement = node.as_for_statement();
    let (initializer, condition, incrementor, statement) =
        (for_statement.initializer, for_statement.condition, for_statement.incrementor, for_statement.statement());

    let initializer = initializer?;
    if initializer.kind() != Kind::VariableDeclarationList
        || !initializer.flags().intersects(NodeFlags::Let)
        || initializer.as_variable_declaration_list().declarations.nodes().len() != 1
    {
        return None;
    }

    let declaration = initializer.as_variable_declaration_list().declarations.nodes()[0].as_variable_declaration();
    let dec_name = declaration.name;
    let dec_init = declaration.initializer()?;
    if dec_name.kind() != Kind::Identifier {
        return None;
    }

    let id_symbol = symbol_of(state, dec_name);

    get_constant_integer(state, dec_init, false)?;

    // require a nonzero constant step that updates the declared loop variable

    let increment = get_optimized_incrementor_step(state, incrementor?, id_symbol)?;
    if increment.value == 0.0 {
        return None;
    }

    let (bound, operator) = get_optimized_condition(state, condition?, id_symbol)?;
    match operator {
        Kind::LessThanToken | Kind::LessThanEqualsToken => {
            if increment.value < 0.0 {
                return None;
            }
        }
        Kind::GreaterThanToken | Kind::GreaterThanEqualsToken => {
            if increment.value > 0.0 {
                return None;
            }
        }
        _ => return None,
    }

    get_constant_integer(state, bound, false)?;

    if is_mutated_in_body(state, dec_name, statement) {
        return None;
    }

    // commit to the optimization and start transforming..

    let id = as_any_identifier(transform_identifier_defined(state, dec_name));

    let mut start_prereqs = Prereqs::new();
    let start = transform_expression(state, &mut start_prereqs, dec_init);
    assert!(start_prereqs.statements.is_empty(), "a constant loop start has no prerequisites");
    let mut end_prereqs = Prereqs::new();
    let mut end = transform_expression(state, &mut end_prereqs, bound);
    assert!(end_prereqs.statements.is_empty(), "a constant loop bound has no prerequisites");

    let mut step = create::number(increment.value);
    if let Some(step_expression) = increment.expression {
        let mut step_prereqs = Prereqs::new();
        let expression = transform_expression(state, &mut step_prereqs, step_expression);
        assert!(step_prereqs.statements.is_empty(), "a constant loop step has no prerequisites");

        // retain references and arithmetic, but keep the existing compact spelling of literal steps
        if get_literal_number_value(&expression).is_none() {
            step = if increment.negate { create::unary(UnaryOperator::Neg, expression) } else { expression };
        }
    }

    let statements = transform_body(state, statement);

    if operator == Kind::LessThanToken {
        end = offset(end, -1.0);
    } else if operator == Kind::GreaterThanToken {
        end = offset(end, 1.0);
    }

    Some(vec![create::numeric_for(id, start, end, Some(step), statements)])
}

/// `transformForStatement`
pub fn transform_for_statement(state: &mut TransformState<'_>, node: P<Node>) -> Vec<Stmt> {
    if state.services.data.options.optimized_loops
        && let Some(optimized) = transform_for_statement_optimized(state, node)
    {
        return optimized;
    }
    transform_for_statement_fallback(state, node)
}
