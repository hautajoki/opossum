//! `nodes/class/transformClassLikeDeclaration.ts`: class declarations and expressions.
//!
//! ```lua
//! local Class
//! do
//!     -- boilerplate: the class table, its metatable, `Class.new`
//!     -- the constructor, then the methods
//!     -- static properties and static blocks
//!     -- decorators
//! end
//! ```

use rbxts_luau::{AnyIdentifier, AssignmentOperator, BinaryOperator, Expr, Stmt, create, globals};
use tsrs_ast::{Kind, ModifierFlags, Node, SymbolFlags};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, as_any_identifier, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::class::transform_class_constructor::{
    transform_class_constructor, transform_implicit_class_constructor,
};
use crate::nodes::class::transform_decorators::transform_decorators;
use crate::nodes::class::transform_property_declaration::transform_property_declaration;
use crate::nodes::expressions::transform_expression::{kind_name, transform_expression};
use crate::nodes::expressions::transform_identifier::transform_identifier_defined;
use crate::nodes::statements::transform_block::transform_block;
use crate::nodes::transform_method_declaration::transform_method_declaration;
use crate::shared::diagnostics::errors;
use crate::util::find_constructor::find_constructor;
use crate::util::pointer::Pointer;
use crate::util::symbols::{get_extends_node, validate_identifier};
use crate::util::traversal::has_syntactic_modifier;
use crate::util::validate_method_assignment::validate_method_assignment;

const MAGIC_TO_STRING_METHOD: &str = "toString";

/// The result of [`transform_class_like_declaration`]: the statements, and the name the class ends up in.
pub struct TransformedClass {
    /// `local Class` (unless hoisted) and the `do … end` block that builds the class.
    pub statements: Vec<Stmt>,
    /// What the class is stored in: its name, `default`, or a temporary for class expressions.
    pub name: AnyIdentifier,
}

/// `function() return "name" end`
fn create_name_function(name: &str) -> Expr {
    create::function(Vec::new(), false, vec![create::return_stmt(create::string(name))])
}

/// `createBoilerplate`: the class table and its metatable, and `Class.new` unless the class is abstract.
fn create_boilerplate(
    state: &mut TransformState<'_>,
    node: P<Node>,
    class_name: &AnyIdentifier,
    is_class_expression: bool,
) -> Vec<Stmt> {
    let is_abstract = tsrs_ast::has_abstract_modifier(node);
    let class: Expr = class_name.clone().into();
    let mut statements = Vec::new();

    /* boilerplate:
        className = setmetatable({}, {
            __tostring = function() return "className" end;
        });
        className.__index = className;
        function className.new(...)
            local self = setmetatable({}, className);
            self:constructor(...);
            return self;
        end;
        function className:constructor()
        end;
    */

    // if a class is abstract and it does not extend any class, it can just be a plain table
    // otherwise we can use the default boilerplate
    let extends_node = get_extends_node(node);
    if is_abstract && extends_node.is_none() {
        statements.push(create::assignment(class.clone(), AssignmentOperator::Assign, create::map([])));
    } else {
        let name = match class_name {
            AnyIdentifier::Temporary(_) => "Anonymous",
            AnyIdentifier::Identifier(identifier) => identifier.name.as_str(),
        };
        let mut metatable_fields = vec![(globals::strings::__tostring(), create_name_function(name))];

        if let Some(extends_node) = extends_node {
            let mut extends_exp_prereqs = Prereqs::new();
            let extends_expression = extends_node.expression().expect("an extends clause has an expression");
            let extends_exp = transform_expression(state, &mut extends_exp_prereqs, extends_expression);
            let super_id = create::any_id("super");
            statements.extend(extends_exp_prereqs.statements);
            statements.push(create::local(super_id.clone(), Some(extends_exp)));
            metatable_fields.push((globals::strings::__index(), super_id.into()));
        }

        let metatable = create::call(globals::setmetatable(), vec![create::map([]), create::map(metatable_fields)]);

        if is_class_expression && let Some(name) = node.name() {
            let left = as_any_identifier(transform_identifier_defined(state, name));
            statements.push(create::local(left, Some(metatable)));
        } else {
            statements.push(create::assignment(class.clone(), AssignmentOperator::Assign, metatable));
        }

        // className.__index = className;
        statements.push(create::assignment(
            create::property(class.clone(), "__index"),
            AssignmentOperator::Assign,
            class.clone(),
        ));
    }

    // statements for className.new
    if !is_abstract {
        let statements_inner = vec![
            // local self = setmetatable({}, className);
            create::local(
                as_any_identifier(globals::self_()),
                Some(create::call(globals::setmetatable(), vec![create::map([]), class.clone()])),
            ),
            // return self:constructor(...) or self;
            create::return_stmt(create::binary(
                create::method_call(globals::self_(), "constructor", vec![create::var_args()]),
                BinaryOperator::Or,
                globals::self_(),
            )),
        ];

        // function className.new(...)
        // end;
        statements.push(create::function_declaration(
            false,
            create::property(class, "new"),
            Vec::new(),
            true,
            statements_inner,
        ));
    }

    statements
}

/// `isClassHoisted`: the class is used before its declaration, so `local Class` was already emitted above it.
fn is_class_hoisted(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    match node.name() {
        Some(name) => {
            let symbol = state.checker.get_symbol_at_location_exported(name).expect("a class name has a symbol");
            state.is_hoisted.get(&symbol) == Some(&true)
        }
        None => false,
    }
}

/// The text of an identifier or string literal member name (`ts.isIdentifier(name) || ts.isStringLiteral(name)`).
fn identifier_or_string_name(member: P<Node>) -> Option<(P<Node>, &'static str)> {
    let name = member.name()?;
    matches!(name.kind(), Kind::Identifier | Kind::StringLiteral).then(|| (name, name.text()))
}

/// Static properties and static blocks, in source order.
///
/// rbxts-rs: this is where the plugin `class` hook (DESIGN §11.2) adds its statements, right after these and before
/// the decorators are applied; keep the static emission in this one function.
fn transform_static_declarations(
    state: &mut TransformState<'_>,
    node: P<Node>,
    static_declarations: &[P<Node>],
    internal_name: &AnyIdentifier,
) -> Vec<Stmt> {
    let mut statements = Vec::new();
    for &declaration in static_declarations {
        if declaration.kind() == Kind::ClassStaticBlockDeclaration {
            let body = declaration.as_class_static_block_declaration().body;
            // rbxts-rs: the block is a statement a TS transformer sees (TypeScript's `isStatement`), lowered here rather
            // than through `transformStatement`: offered to the plugins' statement hooks first (DESIGN §11.5)
            match crate::plugins::dispatch::claim_statement(state, body) {
                Some(claimed) => statements.extend(claimed),
                None => statements.extend(transform_block(state, body)),
            }
        } else {
            let mut property_prereqs = Prereqs::new();
            let property = transform_property_declaration(state, &mut property_prereqs, declaration, internal_name);
            statements.extend(property_prereqs.statements);
            statements.extend(property);
        }
    }
    statements.extend(crate::plugins::dispatch::class_statements(state, node, internal_name));
    statements
}

/// `transformClassLikeDeclaration`
pub fn transform_class_like_declaration(state: &mut TransformState<'_>, node: P<Node>) -> TransformedClass {
    let is_class_expression = node.kind() == Kind::ClassExpression;
    let mut statements = Vec::new();

    let is_export_default = has_syntactic_modifier(node, ModifierFlags::ExportDefault);

    if let Some(name) = node.name() {
        validate_identifier(state, name);
    }

    /*
        local className;
        do
            OOP boilerplate
            class functions
        end
    */

    let should_use_internal_name = is_class_expression && node.name().is_some();

    let return_var: AnyIdentifier = if should_use_internal_name {
        temp_id("class").into()
    } else if let Some(name) = node.name() {
        as_any_identifier(transform_identifier_defined(state, name))
    } else if is_export_default {
        create::any_id("default")
    } else {
        temp_id("class").into()
    };

    let internal_name = match node.name() {
        Some(name) if should_use_internal_name => as_any_identifier(transform_identifier_defined(state, name)),
        _ => return_var.clone(),
    };
    state.class_identifier_map.insert(node, internal_name.clone());
    if !is_class_hoisted(state, node) {
        statements.push(create::local(return_var.clone(), None));
    }

    // OOP boilerplate + class functions
    let mut statements_inner = create_boilerplate(state, node, &internal_name, is_class_expression);

    match find_constructor(node) {
        Some(constructor) => statements_inner.extend(transform_class_constructor(state, constructor, &internal_name)),
        None => statements_inner.extend(transform_implicit_class_constructor(state, node, &internal_name)),
    }

    for &member in node.members() {
        if matches!(member.kind(), Kind::PropertyDeclaration | Kind::MethodDeclaration)
            && let Some((name, text)) = identifier_or_string_name(member)
            && rbxts_luau::is_reserved_class_field(text)
        {
            state.add_diagnostic(errors::no_reserved_class_fields(name));
        }
        if tsrs_ast::is_auto_accessor_property_declaration(member) {
            // member must have AccessorKeyword to be AutoAccessorPropertyDeclaration
            let keyword = member
                .modifiers()
                .and_then(|m| m.nodes().iter().copied().find(|m| m.kind() == Kind::AccessorKeyword))
                .expect("an auto-accessor has the accessor keyword");
            state.add_diagnostic(errors::no_auto_accessor_modifiers(keyword));
        }
    }

    let mut methods = Vec::new();
    let mut static_declarations = Vec::new();

    for &member in node.members() {
        validate_method_assignment(state, member);
        match member.kind() {
            Kind::Constructor | Kind::IndexSignature | Kind::SemicolonClassElement => {}
            Kind::MethodDeclaration => methods.push(member),
            // non-static properties are done in transformClassConstructor
            Kind::PropertyDeclaration => {
                if tsrs_ast::has_static_modifier(member) {
                    static_declarations.push(member);
                }
            }
            Kind::GetAccessor | Kind::SetAccessor => state.add_diagnostic(errors::no_getter_setter(member)),
            Kind::ClassStaticBlockDeclaration => static_declarations.push(member),
            // rbxts-rs: roblox-ts asserts here (a crash); an internal error diagnostic keeps the build going
            kind => state.internal_error(member, &format!("ClassMember kind not implemented: {}", kind_name(kind))),
        }
    }

    let class_symbol = node.symbol().expect("a class has a symbol");
    let class_type = state
        .checker
        .get_type_of_symbol_at_location(class_symbol, Some(node))
        .unwrap_or_else(|| state.checker.get_error_type());
    let instance_type = state.checker.get_declared_type_of_symbol_exported(class_symbol);

    let mut ptr = Pointer { name: "name".to_owned(), value: internal_name.clone().into() };
    for method in methods {
        if let Some((name, text)) = identifier_or_string_name(method) {
            if rbxts_luau::is_metamethod(text) {
                state.add_diagnostic(errors::no_class_metamethods(name));
            }

            if tsrs_ast::has_static_modifier(method) {
                if state.checker.get_property_of_type_exported(instance_type, text).is_some() {
                    state.add_diagnostic(errors::no_instance_method_collisions(method));
                }
            } else if state.checker.get_property_of_type_exported(class_type, text).is_some() {
                state.add_diagnostic(errors::no_static_method_collisions(method));
            }
        }

        let mut method_prereqs = Prereqs::new();
        let method_statements = transform_method_declaration(state, &mut method_prereqs, method, &mut ptr);
        statements_inner.extend(method_prereqs.statements);
        statements_inner.extend(method_statements);
    }

    let to_string_property = state.checker.get_property_of_type_exported(instance_type, MAGIC_TO_STRING_METHOD);
    if to_string_property.is_some_and(|p| p.flags().intersects(SymbolFlags::Method)) {
        statements_inner.push(create::method_declaration(
            internal_name.clone().into(),
            "__tostring",
            Vec::new(),
            false,
            vec![create::return_stmt(create::method_call(globals::self_(), MAGIC_TO_STRING_METHOD, Vec::new()))],
        ));
    }

    statements_inner.extend(transform_static_declarations(state, node, &static_declarations, &internal_name));

    // if using internal name, assign to return var
    if should_use_internal_name {
        statements_inner.push(create::assignment(
            return_var.clone().into(),
            AssignmentOperator::Assign,
            internal_name.into(),
        ));
    }

    statements_inner.extend(transform_decorators(state, node, &return_var));

    statements.push(create::do_stmt(statements_inner));

    TransformedClass { statements, name: return_var }
}
