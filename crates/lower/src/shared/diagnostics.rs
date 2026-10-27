//! roblox-ts's errors and warnings (`Shared/diagnostics.ts`), as tsrs diagnostics.
//!
//! Each `DiagnosticFactory` becomes a function from the node to a `P<Diagnostic>`, with the same text. The message is
//! a `tsrs_ast::new_ad_hoc_message` (code -1), made once per factory; context-dependent ones make one per
//! call. The location is the node's `getStart()` to its end, as `createDiagnosticWithLocation` reports it.
//!
//! opossum's own diagnostics carry a dotted code at the start of their message (`route.boundary: …`).

use std::sync::OnceLock;

use tsrs_ast::{Diagnostic, Node, SourceFile, new_ad_hoc_message, new_diagnostic};
use tsrs_core::{P, TextRange};
use tsrs_diagnostics::{Category, Message};

const REPO_URL: &str = "https://github.com/roblox-ts/roblox-ts";

fn suggestion(text: &str) -> String {
    format!("Suggestion: {text}")
}

fn issue(id: u32) -> String {
    format!("More information: {REPO_URL}/issues/{id}")
}

/// `node.getStart()` (trivia skipped, JSDoc not included) to `node.end`.
pub fn node_range(node: P<Node>) -> (Option<P<SourceFile>>, TextRange) {
    let file = tsrs_ast::get_source_file_of_node(node);
    let (Some(pos), Some(end)) = (node.pos().source_offset(), node.end().source_offset()) else {
        return (file, TextRange::new(0, 0));
    };
    let start = match file {
        Some(file) => tsrs_scanner::get_token_pos_of_node(node, file, false),
        None => pos,
    };
    (file, TextRange::new(start, end.max(start)))
}

fn at_node(node: P<Node>, message: &'static Message, category: Category) -> P<Diagnostic> {
    let (file, range) = node_range(node);
    let diagnostic = new_diagnostic(file, range, message, &[]);
    if category != Category::Error {
        diagnostic.set_category(category);
    }
    diagnostic
}

/// A text diagnostic without a location (`createTextDiagnostic`).
pub fn text_diagnostic(message: &str, category: Category) -> P<Diagnostic> {
    let diagnostic = new_diagnostic(None, None, new_ad_hoc_message(message), &[]);
    if category != Category::Error {
        diagnostic.set_category(category);
    }
    diagnostic
}

macro_rules! factories {
    ($category:ident: $($(#[$meta:meta])* $name:ident = [$($line:expr),+ $(,)?];)*) => {
        $(
            $(#[$meta])*
            #[doc = concat!("roblox-ts's `", stringify!($category), "` diagnostic `", stringify!($name), "` at `node`.")]
            pub fn $name(node: P<Node>) -> P<Diagnostic> {
                static MESSAGE: OnceLock<&'static Message> = OnceLock::new();
                let message = MESSAGE.get_or_init(|| {
                    let lines: Vec<String> = vec![$(String::from($line)),+];
                    new_ad_hoc_message(&lines.join("\n"))
                });
                at_node(node, message, Category::$category)
            }
        )*
    };
}

/// `errors`
pub mod errors {
    use super::*;

    factories!(Error:
        // reserved identifiers
        no_invalid_identifier = [
            "Invalid Luau identifier!",
            "Luau identifiers must start with a letter and only contain letters, numbers, and underscores.",
            "Reserved Luau keywords cannot be used as identifiers.",
        ];
        no_reserved_class_fields = ["Cannot use class field reserved for compiler internal usage."];
        no_class_metamethods = ["Metamethods cannot be used in class definitions!"];
        no_length_index_in_tuples = ["Cannot index member `length` in a tuple!", suggestion("Use .size() instead.")];

        // banned statements
        no_for_in_statement = ["for-in loop statements are not supported!"];
        no_labeled_statement = ["labels are not supported!"];
        no_debugger_statement = ["`debugger` is not supported!"];

        // banned expressions
        no_null_literal = ["`null` is not supported!", suggestion("Use `undefined` instead.")];
        no_private_identifier = ["Private identifiers are not supported!"];
        no_type_of_expression = [
            "`typeof` operator is not supported!",
            suggestion("Use `typeIs(value, type)` or `typeOf(value)` instead."),
        ];
        no_regex = ["Regular expressions are not supported!"];
        no_big_int = ["BigInt literals are not supported!"];

        // banned features
        no_any = ["Using values of type `any` is not supported!", suggestion("Use `unknown` instead.")];
        no_var = ["`var` keyword is not supported!", suggestion("Use `let` or `const` instead.")];
        no_getter_setter = ["Getters and Setters are not supported!", issue(457)];
        no_auto_accessor_modifiers = [
            "Getters and Setters are not supported!",
            "The `accessor` keyword requires generating get/set accessors",
            issue(457),
        ];
        no_equals_equals = ["operator `==` is not supported!", suggestion("Use `===` instead.")];
        no_exclamation_equals = ["operator `!=` is not supported!", suggestion("Use `!==` instead.")];
        no_enum_merging = ["Enum merging is not supported!"];
        no_namespace_merging = ["Namespace merging is not supported!"];
        no_function_expression_name = ["Function expression names are not supported!"];
        no_preceding_spread_element = ["Spread element must come last in a list of arguments!"];
        no_lua_tuple_destructure_assignment_expression = [
            "Cannot destructure LuaTuple<T> expression outside of an ExpressionStatement!",
        ];
        no_export_assignment_let = ["Cannot use `export =` on a `let` variable!", suggestion("Use `const` instead.")];
        no_global_this = ["`globalThis` is not supported!"];
        no_arguments = ["`arguments` is not supported!"];
        no_prototype = ["`prototype` is not supported!"];
        no_roblox_symbol_instanceof = [
            "The `instanceof` operator can only be used on roblox-ts classes!",
            suggestion("Use `typeIs(myThing, \"TypeToCheck\") instead"),
        ];
        no_rest_spreading_of_roblox_types = ["Operator `...` is not allowed on Roblox types!"];
        no_non_number_string_relation_operator = ["Relation operators can only be used on number or string types!"];
        no_instance_method_collisions = ["Static methods cannot use the same name as instance methods!"];
        no_static_method_collisions = ["Instance methods cannot use the same name as static methods!"];
        no_unary_plus = ["Unary `+` is not supported!", suggestion("Use `tonumber(x)` instead.")];
        no_non_number_unary_minus = ["Unary `-` is only supported for number types!"];
        no_await_for_of = ["`await` is not supported in for-of loops!"];
        no_async_generator_functions = ["Async generator functions are not supported!"];
        no_non_string_module_specifier = ["Module specifiers must be a string literal."];
        no_iterable_iteration = ["Iterating on Iterable<T> is not supported! You must use a more specific type."];
        no_unsupported_iteration = [
            "Cannot iterate over this type!",
            "roblox-ts needs a supported, specific iterable type to generate the correct iteration code.",
            suggestion("Use a supported type such as Array<T>, Set<T>, or Map<K, V>."),
        ];
        no_mixed_type_call = [
            "Attempted to call a function with mixed types! All definitions must either be a method or a callback.",
        ];
        no_unstable_this_type = [
            "The generic this type can become void, changing whether a receiver is passed.",
            suggestion("Use this: void for callbacks, or constrain the receiver type to exclude void."),
        ];
        no_index_without_call = [
            "Cannot index a method without calling it!",
            suggestion("Use the form `() => a.b()` instead of `a.b`."),
        ];

        // macro methods
        no_optional_macro_call = [
            "Macro methods can not be optionally called!",
            suggestion("Macros always exist. Use a normal call."),
        ];
        no_constructor_macro_without_new = ["Cannot index a constructor macro without using the `new` operator!"];
        no_macro_extends = [
            "Cannot extend from a macro class!",
            suggestion("Store an instance of the macro class in a property."),
        ];
        no_macro_union = ["Macro cannot be applied to a union type!"];
        no_macro_object_spread = [
            "Macro classes cannot be used in an object spread!",
            suggestion("Did you mean to use an array spread? `[ ...exp ]`"),
        ];
        no_var_args_macro_spread = [
            "Macros which use variadic arguments do not support spread expressions!",
            issue(1149),
        ];
        no_range_macro_outside_for_of = ["$range() macro is only valid as an expression of a for-of loop!"];
        no_tuple_macro_outside_return = ["$tuple() macro is only valid as an expression of a return statement!"];

        // import/export
        no_module_specifier_file = ["Could not find file for import. Did you forget to `npm install`?"];
        no_non_module_import = ["Cannot import a non-ModuleScript!"];

        // jsx
        no_preceding_jsx_spread_element = ["JSX spread expression must come last in children!"];
        /// rbxts-rs: an entity past U+10FFFF (`&#x110000;`), where roblox-ts (and TypeScript's own JSX emit) crash
        no_invalid_jsx_entity = ["JSX character entity is not a valid code point (above U+10FFFF)!"];

        // semantic
        expected_method_got_function = ["Attempted to assign non-method where method was expected."];
        expected_function_got_method = ["Attempted to assign method where non-method was expected."];

        no_mixed_string_index = [
            "Cannot index a type containing both strings and other types!",
            suggestion("Narrow the value to a string or a non-string type before indexing."),
        ];
    );

    /// `route.boundary`: an import the importer's realm cannot reach at run time.
    pub fn route_boundary(node: P<Node>, from: &str, to: &str) -> P<Diagnostic> {
        let text = format!("route.boundary: {from} code cannot require a {to} module");
        at_node(node, new_ad_hoc_message(&text), Category::Error)
    }

    /// `route.unrouted`: a file lowering needs an instance path for (the file itself, an import, the runtime
    /// library) has none.
    pub fn unrouted(node: P<Node>, file: &str) -> P<Diagnostic> {
        let text = format!("route.unrouted: {file} has no instance path");
        at_node(node, new_ad_hoc_message(&text), Category::Error)
    }

    /// `macro.unsupported`: a compile-time macro opossum does not lower yet.
    pub fn unsupported_macro(node: P<Node>, name: &str) -> P<Diagnostic> {
        at_node(node, new_ad_hoc_message(&format!("macro.unsupported: `{name}` is not supported yet")), Category::Error)
    }

    /// `macro.missing`: a method of a macro-only class with no lowering.
    pub fn missing_macro(node: P<Node>, name: &str) -> P<Diagnostic> {
        at_node(node, new_ad_hoc_message(&format!("macro.missing: `{name}` has no lowering")), Category::Error)
    }
}

/// rbxts-rs: a compiler bug caught while transforming a file (a failed assertion or a panic).
pub fn internal_error(node: P<Node>, message: &str) -> P<Diagnostic> {
    at_node(node, new_ad_hoc_message(&format!("opossum internal error: {message}")), Category::Error)
}
