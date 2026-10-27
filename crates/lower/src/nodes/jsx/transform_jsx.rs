//! `nodes/jsx/transformJsx.ts`

use rbxts_luau::{Expr, Stmt, create};
use tsrs_ast::Node;
use tsrs_core::P;

use crate::classes::prereqs::Prereqs;
use crate::classes::transform_state::TransformState;
use crate::nodes::jsx::transform_jsx_attributes::transform_jsx_attributes;
use crate::nodes::jsx::transform_jsx_children::transform_jsx_children;
use crate::nodes::jsx::transform_jsx_tag_name::transform_jsx_tag_name;
use crate::nodes::transform_entity_name::transform_entity_name;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::evaluation::effects::{effects_commute, get_jsx_effects, get_jsx_effects_of_statements};
use crate::util::pointer::{MapPointer, create_map_pointer, disable_map_inline};

/// The JSX factory as an indexable expression (`React.createElement`): the file's `@jsx` pragma, else the
/// `jsxFactory` option, else `React.createElement`. rbxts-rs: the lines `transformJsx` and `transformJsxFragment` share.
pub fn transform_jsx_factory_entity(state: &mut TransformState<'_>, node: P<Node>) -> Expr {
    // jsxFactoryEntity seems to always be defined and will default to `React.createElement`
    let jsx_factory_entity =
        state.resolver.get_jsx_factory_entity(state.checker, node).expect("Expected jsxFactoryEntity to be defined");
    convert_to_indexable_expression(transform_entity_name(state, jsx_factory_entity))
}

/// rbxts-rs: `expression` in a temporary when `statements`, which run after it is read in JavaScript, could change it.
fn capture_before(prereqs: &mut Prereqs, expression: Expr, statements: &[Stmt], name: &str) -> Expr {
    if effects_commute(&get_jsx_effects(&expression), &get_jsx_effects_of_statements(statements)) {
        expression
    } else {
        prereqs.push_to_var(expression, name)
    }
}

/// `transformJsx`: `factory(tag, attributes, ...children)`, with `nil` attributes when there are children but no
/// attributes.
pub fn transform_jsx(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    tag_name: P<Node>,
    attributes: P<Node>,
    children: &[P<Node>],
) -> Expr {
    let create_element_expression = transform_jsx_factory_entity(state, node);

    let mut tag_name_exp = transform_jsx_tag_name(state, prereqs, tag_name);

    // rbxts-rs: fixes roblox-ts running the attributes' and the children's prerequisites before reading what came
    // before them: `<frame Size={size}>{holder?.bump()}</frame>` built `{ Size = size }` after `bump()` changed
    // `size`. JavaScript evaluates the tag, then the attributes, then the children; a value is captured when later
    // prerequisites could change it. Roblox datatype constructors (`UDim2.fromScale(1, 1)`, `new Vector2(0, 1)`)
    // count as allocations here and Enum items and datatype constants (`Enum.AutomaticSize.Y`, `Vector2.zero`) as
    // constants (`get_jsx_effects`), so they force no capture; any other call (`Instance.new`, the user's own) or
    // property read still does.
    let mut attributes_ptr: Option<MapPointer> = None;
    if !attributes.properties().is_empty() {
        let mut ptr = create_map_pointer("attributes");
        let mut attributes_prereqs = Prereqs::new();
        transform_jsx_attributes(state, &mut attributes_prereqs, attributes, &mut ptr);
        tag_name_exp = capture_before(prereqs, tag_name_exp, &attributes_prereqs.statements, "tag");
        prereqs.push_list(attributes_prereqs.statements);
        attributes_ptr = Some(ptr);
    }

    let mut children_prereqs = Prereqs::new();
    let transformed_children = transform_jsx_children(state, &mut children_prereqs, children);
    tag_name_exp = capture_before(prereqs, tag_name_exp, &children_prereqs.statements, "tag");
    if let Some(ptr) = &mut attributes_ptr
        && !effects_commute(&get_jsx_effects(&ptr.value), &get_jsx_effects_of_statements(&children_prereqs.statements))
    {
        disable_map_inline(prereqs, ptr);
    }
    prereqs.push_list(children_prereqs.statements);

    let mut args = vec![tag_name_exp];

    if let Some(ptr) = attributes_ptr {
        args.push(ptr.value);
    } else if !transformed_children.is_empty() {
        args.push(create::nil());
    }

    args.extend(transformed_children);

    create::call(create_element_expression, args)
}
