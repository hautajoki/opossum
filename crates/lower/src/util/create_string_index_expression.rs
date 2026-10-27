//! `util/createStringIndexExpression.ts`: `str[i]` as a guarded `string.sub`.

use rbxts_luau::{BinaryOperator, Expr, UnaryOperator, create, globals};
use tsrs_checker::Type;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::util::evaluation::plan::{EvaluationOperand, plan_evaluation};
use crate::util::get_literal_number_value::get_literal_number_value;
use crate::util::offset::offset;
use crate::util::types::{is_definitely_type, is_possibly_type, is_string_type, string_literal_value};

/// `createStringIndexExpression`
pub fn create_string_index_expression(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    expression: Expr,
    index_operand: EvaluationOperand,
    index_type: P<Type>,
) -> Expr {
    let string_literal_index = string_literal_value(index_type);
    let literal_index = match string_literal_index {
        Some(s) => Some(rbxts_luau::js_string_to_number(s)),
        None => get_literal_number_value(&index_operand.expression),
    };
    // decided before the expansion, which cannot borrow the state
    let possibly_string = literal_index.is_none() && is_possibly_type(state, index_type, &[&is_string_type]);
    let definitely_string = possibly_string && is_definitely_type(state, index_type, &[&is_string_type]);

    plan_evaluation(
        prereqs,
        vec![EvaluationOperand { expression, prereqs: Vec::new() }, index_operand],
        |index_prereqs, references| {
            let mut references = references.into_iter();
            let value = references.next().expect("value operand");
            let mut index = references.next().expect("index operand");
            if let Some(literal_index) = literal_index {
                if literal_index.fract() != 0.0 || !literal_index.is_finite() || literal_index < 0.0 {
                    return Expr::Nil;
                }
                if string_literal_index.is_some() {
                    index = create::number(literal_index);
                }
            }

            let mut conditions = Vec::new();
            if literal_index.is_none() {
                // TypeScript also accepts numeric string keys such as "0"
                if possibly_string {
                    let key = index.clone();
                    index = index_prereqs.push_to_var(create::call(create::id("tonumber"), vec![index]), "index");
                    conditions.push(create::binary(index.clone(), BinaryOperator::Ne, Expr::Nil));

                    // string keys must spell a nonnegative integer without signs, padding, or exponents
                    let mut canonical_key = create::binary(
                        create::binary(key.clone(), BinaryOperator::Eq, create::string("0")),
                        BinaryOperator::Or,
                        create::binary(
                            create::call(globals::string::r#match(), vec![key.clone(), create::string("^[1-9]%d*$")]),
                            BinaryOperator::Ne,
                            Expr::Nil,
                        ),
                    );
                    if !definitely_string {
                        canonical_key = create::binary(
                            create::binary(
                                create::call(globals::r#type(), vec![key]),
                                BinaryOperator::Eq,
                                create::string("number"),
                            ),
                            BinaryOperator::Or,
                            canonical_key,
                        );
                    }
                    conditions.push(canonical_key);
                }
                conditions.push(create::binary(index.clone(), BinaryOperator::Ge, create::number(0.0)));
                conditions.push(create::binary(
                    create::binary(index.clone(), BinaryOperator::Mod, create::number(1.0)),
                    BinaryOperator::Eq,
                    create::number(0.0),
                ));
            }
            conditions.push(create::binary(
                index.clone(),
                BinaryOperator::Lt,
                create::unary(UnaryOperator::Len, value.clone()),
            ));

            // guard before string.sub so negative offsets and fractional indices cannot select a byte
            let at = offset(index, 1.0);
            let mut conditions = conditions.into_iter();
            let first = conditions.next().expect("at least one condition");
            let condition = conditions.fold(first, |left, right| create::binary(left, BinaryOperator::And, right));
            create::if_expr(condition, create::call(globals::string::sub(), vec![value, at.clone(), at]), Expr::Nil)
        },
    )
}
