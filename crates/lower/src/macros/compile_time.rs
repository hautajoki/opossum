//! opossum's compile-time macros, declared in `.opossum/opossum.d.ts` ([`crate::overlay::MACROS_D_TS`]): `$env`,
//! `$git`, `$package`, `$file`, `$nameof`, `$keys` and `$buildTime`. Each folds the head of its call or access chain
//! (`$env("A")`, `$package.version`, `$file("x.json").a.b`) to a Luau literal read from [`crate::Options`], the disk
//! or the checker; the rest of the chain lowers on that literal. A file records the [`Input`]s its macros read, so a
//! watcher rebuilds exactly the files whose inputs changed.

use luau::{Expr, create};
use serde_json::Value;
use tsrs_ast::{Kind, Node, Symbol, new_ad_hoc_message, new_diagnostic};
use tsrs_checker::TypeFlags;
use tsrs_core::{P, tspath};

use crate::classes::transform_state::TransformState;
use crate::nodes::expressions::transform_identifier::identifier_symbol;
use crate::nodes::transform_optional_chain::{ChainItem, ChainItemKind};
use crate::shared::diagnostics::node_range;
use crate::util::create_string_from_literal::create_string_from_value;
use crate::util::traversal::skip_downwards;
use crate::util::types::string_literal_value;

/// Something outside the program that a file's macros read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Input {
    /// A `$file`, by absolute path; recorded even when missing, since creating it changes the output.
    File(String),
    /// An `$env` variable, by name.
    Env(String),
    /// `$git`: the repository's HEAD and working tree.
    Git,
    /// `$package`: the project's `package.json`.
    Package,
}

/// The macro `symbol` is, by name (`$env`), when it is one of opossum's declarations.
pub fn macro_name(state: &TransformState<'_>, symbol: P<Symbol>) -> Option<&'static str> {
    let name = symbol.name();
    if !name.starts_with('$') {
        return None;
    }
    let file = symbol.declarations().first().and_then(|&d| tsrs_ast::get_source_file_of_node(d))?;
    (file.get().file_name() == crate::program::macros_file(state.root)).then_some(name)
}

/// The macro the receiver of a chain names (wrappers skipped).
pub fn macro_at(state: &mut TransformState<'_>, node: P<Node>) -> Option<&'static str> {
    let node = skip_downwards(node);
    if node.kind() != Kind::Identifier || !node.text().starts_with('$') {
        return None;
    }
    let symbol = identifier_symbol(state, node)?;
    macro_name(state, symbol)
}

/// Folds macro `name`, the receiver `base` of `chain` (empty for a bare identifier): its value and how many links of
/// `chain` it consumed. After a reported error the value is `Expr::None`.
pub fn fold(state: &mut TransformState<'_>, name: &'static str, base: P<Node>, chain: &[ChainItem]) -> (Expr, usize) {
    let (value, used) = match (name, chain.first()) {
        // only `$package` is a value on its own
        ("$package", _) => package(state, base, chain),
        (_, None) => (misuse(state, base, name), 0),
        ("$env", Some(item)) => (env(state, item), 1),
        ("$git", Some(item)) => (git(state, item), 1),
        ("$file", Some(_)) => file(state, chain),
        ("$nameof", Some(item)) => (nameof(state, item), 1),
        ("$keys", Some(item)) => (keys(state, item), 1),
        ("$buildTime", Some(item)) => (build_time(state, item), 1),
        _ => (misuse(state, base, name), chain.len()),
    };
    // `nil?.…` is nil: the rest of the chain never runs
    if matches!(value, Expr::Nil) && chain.get(used).is_some_and(|item| item.optional) {
        return (Expr::Nil, chain.len());
    }
    (value, used)
}

/// Reports `macro.<name>: message` at `node` (`name` is the macro, or a code of its own: `secretEnv`); the value of a
/// failed macro.
fn error(state: &mut TransformState<'_>, node: P<Node>, name: &str, message: &str) -> Expr {
    let (file, range) = node_range(node);
    let text = format!("macro.{}: {message}", name.trim_start_matches('$'));
    state.add_diagnostic(new_diagnostic(file, range, new_ad_hoc_message(&text), &[]));
    Expr::None
}

fn misuse(state: &mut TransformState<'_>, node: P<Node>, name: &str) -> Expr {
    let usage = match name {
        "$env" => "`$env(\"NAME\")`, `$env(\"NAME\", \"fallback\")`, `$env.NAME` or `$env[\"NAME\"]`",
        "$git" => "`$git(\"sha\" | \"branch\" | \"tag\" | \"dirty\" | \"timestamp\")`",
        "$package" => "`$package.field`",
        "$file" => "`$file(\"relative/path\")`",
        "$nameof" => "`$nameof(name)`",
        "$keys" => "`$keys<T>()`",
        "$buildTime" => "`$buildTime()`",
        _ => return error(state, node, name, &format!("`{name}` has no lowering")),
    };
    error(state, node, name, &format!("use {usage}"))
}

fn record(state: &mut TransformState<'_>, input: Input) {
    if !state.macro_inputs.contains(&input) {
        state.macro_inputs.push(input);
    }
}

/// The text of a string literal argument (wrappers skipped).
fn literal(node: P<Node>) -> Option<&'static str> {
    let node = skip_downwards(node);
    tsrs_ast::is_string_literal_like(node).then(|| node.text())
}

/// The first argument of a call link.
fn first_argument(item: &ChainItem) -> Option<P<Node>> {
    match item.kind {
        ChainItemKind::Call => item.node.arguments().first().copied(),
        _ => None,
    }
}

/// The variable opossum reads its Open Cloud key from, which `$env` refuses, set or not.
const API_KEY: &str = "ROBLOX_OPEN_CLOUD_API_KEY";

/// `$env("NAME")`, `$env("NAME", "fallback")`, `$env.NAME`, `$env["NAME"]`: the variable's value, the fallback or nil.
fn env(state: &mut TransformState<'_>, item: &ChainItem) -> Expr {
    let (name, fallback) = match item.kind {
        ChainItemKind::Call => {
            let args = item.node.arguments();
            let Some(&first) = args.first() else { return misuse(state, item.node, "$env") };
            let Some(name) = literal(first) else {
                return error(state, first, "$env", "the variable name must be a string literal");
            };
            let fallback = match args.get(1) {
                None => None,
                Some(&arg) => match literal(arg) {
                    Some(fallback) => Some(fallback),
                    None => return error(state, arg, "$env", "the fallback must be a string literal"),
                },
            };
            (name, fallback)
        }
        ChainItemKind::PropertyAccess { name } => (name, None),
        ChainItemKind::ElementAccess { expression } => match literal(expression) {
            Some(name) => (name, None),
            None => return error(state, expression, "$env", "the variable name must be a string literal"),
        },
        _ => return misuse(state, item.node, "$env"),
    };
    // the key under another name is the key all the same (`env` holds the key opossum sends as API_KEY)
    let env = &state.options.env;
    let key = env.get(API_KEY).map(|k| k.trim()).filter(|k| !k.is_empty());
    if name == API_KEY || key.is_some() && env.get(name).map(|v| v.trim()) == key {
        let message = format!("{name} holds opossum's Open Cloud key; $env would inline it into the place");
        return error(state, item.node, "secretEnv", &message);
    }
    record(state, Input::Env(name.to_owned()));
    match state.options.env.get(name).map(String::as_str).or(fallback) {
        Some(value) => create_string_from_value(value),
        None => Expr::Nil,
    }
}

/// `$git("sha" | "branch" | "tag" | "dirty" | "timestamp")`.
fn git(state: &mut TransformState<'_>, item: &ChainItem) -> Expr {
    let Some(field) = first_argument(item).and_then(literal) else { return misuse(state, item.node, "$git") };
    record(state, Input::Git);
    let options = state.options;
    let Some(git) = &options.git else {
        return error(state, item.node, "$git", "the project is not in a git repository");
    };
    match field {
        "sha" => create_string_from_value(&git.sha),
        "branch" => create_string_from_value(&git.branch),
        "tag" => create_string_from_value(&git.tag),
        "dirty" => create::bool(git.dirty),
        "timestamp" => create::number(git.timestamp as f64),
        _ => misuse(state, item.node, "$git"),
    }
}

/// `$package.field…`: the value at the path in `package.json`; a bare `$package` is the whole manifest.
fn package(state: &mut TransformState<'_>, base: P<Node>, chain: &[ChainItem]) -> (Expr, usize) {
    record(state, Input::Package);
    let node = chain.first().map_or(base, |item| item.node);
    let options = state.options;
    let Some(text) = &options.package_json else {
        return (error(state, node, "$package", "the project has no package.json"), chain.len());
    };
    match serde_json::from_str::<Value>(text) {
        Ok(root) => json_path(state, "$package", "package.json", &root, chain, 0),
        Err(e) => (error(state, node, "$package", &format!("package.json: {e}")), chain.len()),
    }
}

/// `$file("path")`, relative to the calling file: a `.json` file as a Luau value, any other file as a string.
fn file(state: &mut TransformState<'_>, chain: &[ChainItem]) -> (Expr, usize) {
    let item = &chain[0];
    let Some(arg) = first_argument(item) else { return (misuse(state, item.node, "$file"), chain.len()) };
    let Some(path) = literal(arg) else {
        return (error(state, arg, "$file", "the path must be a string literal"), chain.len());
    };
    let dir = tspath::get_directory_path(state.source_file.get().file_name());
    let absolute = tspath::get_normalized_absolute_path(path, &dir);
    record(state, Input::File(absolute.clone()));
    let text = match std::fs::read_to_string(&absolute) {
        Ok(text) => text,
        Err(e) => {
            let reason = if e.kind() == std::io::ErrorKind::NotFound { "not found".to_owned() } else { e.to_string() };
            return (error(state, item.node, "$file", &format!("{absolute}: {reason}")), chain.len());
        }
    };
    if !absolute.to_ascii_lowercase().ends_with(".json") {
        return (create_string_from_value(&text), 1);
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => json_path(state, "$file", path, &value, chain, 1),
        Err(e) => (error(state, item.node, "$file", &format!("{path}: {e}")), chain.len()),
    }
}

/// The member a static link of `chain` reads in `value`: `.key` or `["key"]` of an object, `[0]` of an array. `None`
/// when the link is not static; `Some(None)` when the member is missing.
fn member<'v>(value: &'v Value, item: &ChainItem) -> Option<Option<&'v Value>> {
    match (value, item.kind) {
        (Value::Object(map), ChainItemKind::PropertyAccess { name }) => Some(map.get(name)),
        (Value::Object(map), ChainItemKind::ElementAccess { expression }) => {
            literal(expression).map(|key| map.get(key))
        }
        (Value::Array(list), ChainItemKind::ElementAccess { expression }) => {
            let expression = skip_downwards(expression);
            let index = (expression.kind() == Kind::NumericLiteral).then(|| expression.text().parse::<usize>().ok());
            index.flatten().map(|i| list.get(i))
        }
        _ => None,
    }
}

/// Follows the static links of `chain[start..]` into `value`: the value reached, as a Luau literal, and the index of
/// the first link not followed. A missing member is nil; reading through one is an error.
fn json_path(
    state: &mut TransformState<'_>,
    name: &str,
    source: &str,
    value: &Value,
    chain: &[ChainItem],
    start: usize,
) -> (Expr, usize) {
    let mut value = value;
    let mut index = start;
    while let Some(item) = chain.get(index)
        && let Some(next) = member(value, item)
    {
        index += 1;
        let Some(next) = next else {
            if chain.get(index).is_some_and(|item| !item.optional) {
                let key = match item.kind {
                    ChainItemKind::ElementAccess { expression } => skip_downwards(expression).text(),
                    ChainItemKind::PropertyAccess { name } => name,
                    _ => "",
                };
                let message = format!("{source} has no `{key}` to read through");
                return (error(state, item.node, name, &message), chain.len());
            }
            return (Expr::Nil, index);
        };
        value = next;
    }
    (json_to_luau(value), index)
}

/// A JSON value as a Luau literal. Object members keep their order; a null member is left out (a nil field is no
/// field), a null element stays nil to keep the positions.
fn json_to_luau(value: &Value) -> Expr {
    match value {
        Value::Null => Expr::Nil,
        Value::Bool(b) => create::bool(*b),
        Value::Number(n) => create::number(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => create_string_from_value(s),
        Value::Array(elements) => create::array(elements.iter().map(json_to_luau).collect()),
        Value::Object(members) => create::map(
            members.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (create_string_from_value(k), json_to_luau(v))),
        ),
    }
}

/// `$nameof(a)` → `"a"`, `$nameof(a.b.c)` → `"c"`.
fn nameof(state: &mut TransformState<'_>, item: &ChainItem) -> Expr {
    let Some(arg) = first_argument(item).map(skip_downwards) else { return misuse(state, item.node, "$nameof") };
    match arg.kind() {
        Kind::Identifier => create_string_from_value(arg.text()),
        Kind::PropertyAccessExpression => create_string_from_value(arg.as_property_access_expression().name.text()),
        _ => error(state, arg, "$nameof", "the argument must be an identifier or a property access"),
    }
}

/// `$keys<T>()`: `T`'s properties named in `Extract<keyof T, string>`, optional ones included, in declaration order.
fn keys(state: &mut TransformState<'_>, item: &ChainItem) -> Expr {
    let type_node = match item.kind {
        ChainItemKind::Call => item.node.type_arguments().first().copied(),
        _ => None,
    };
    let Some(type_node) = type_node else { return misuse(state, item.node, "$keys") };
    // the call's type is `Array<Extract<keyof T, string>>`: never or string literals exactly when the keys are known
    // (not for a generic T, `any` or an index signature), and then it names exactly the keys the array may hold
    let call = state.checker.get_type_at_location(item.node);
    let element = state.checker.get_type_arguments_exported(call).first().copied();
    let keys: Option<Vec<&str>> = element.and_then(|e| match e.flags() {
        f if f.intersects(TypeFlags::Never) => Some(Vec::new()),
        f if f.intersects(TypeFlags::Union) => e.types().iter().map(|&t| string_literal_value(t)).collect(),
        _ => string_literal_value(e).map(|key| vec![key]),
    });
    let Some(keys) = keys else {
        return error(state, type_node, "$keys", "T must have known keys: not generic, any or an index signature");
    };
    let t = state.checker.get_type_from_type_node_exported(type_node);
    let apparent = state.checker.get_apparent_type_exported(t);
    let names = state.checker.get_properties_of_type_exported(apparent).iter().map(|p| p.name());
    create::array(names.filter(|n| keys.contains(n)).map(create_string_from_value).collect())
}

/// `$buildTime()`: the build's ISO-8601 UTC timestamp.
fn build_time(state: &mut TransformState<'_>, item: &ChainItem) -> Expr {
    if !matches!(item.kind, ChainItemKind::Call) {
        return misuse(state, item.node, "$buildTime");
    }
    create_string_from_value(&state.options.build_time)
}
