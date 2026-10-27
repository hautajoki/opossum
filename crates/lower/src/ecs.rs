//! jecs (`@rbxts/jecs`), lowered natively. Recognized by where the called member is declared (the package's
//! `jecs.d.ts`), never by names alone; anything outside the shapes below takes the normal lowering, silently.
//!
//! **Query loops.** `for (const [e, a, b] of world.query(A, B).with(C).without(D))` walks the query's archetypes
//! directly, doing what jecs' own (uncached) iterator does, inlined:
//!
//! ```luau
//! for _, _archetype in _archetypes do                 -- a snapshot: archetypes made by the body are not walked
//!     local _entities = _archetype.entities
//!     local _columns = _archetype.columns_map
//!     local _a, _b = _columns[A], _columns[B]
//!     for _row = #_entities, 1, -1 do                 -- last to first: removing the current entity is safe
//!         local e = _entities[_row]
//!         if e == nil then break end                  -- the body removed rows below: on to the next archetype
//!         local a, b = _a[_row], _b[_row]
//!         <body>
//!     end
//! end
//! ```
//!
//! The archetype list is the one `world:query(…):with(…):without(…):archetypes()` returns, built:
//! - **once per archetype change**, when the world and every term are module-level constants (a `const` at the top
//!   of a file, an import of one, a namespace import's member, a jecs constant, or `pair(…)` of those). The list is
//!   memoized in a module-level local with the world's `max_archetype_id` at the time, and rebuilt when that moves.
//!   An archetype jecs destroys since is empty and stays empty, so walking it is harmless. Loops of one file whose
//!   queries match the same archetypes (`query` ∪ last `with`, last `without`, as sets) share the memo; constant
//!   `pair` terms are memoized too. jecs' own cached queries are not used: their list changes while it is walked
//!   (an archetype made by the body is appended, a destroyed one is swapped with the last), which an uncached
//!   `for…of` never sees, and each one adds observers to every archetype creation.
//! - **at the loop** otherwise (a world passed in, `set.Mover`, `pair(R, entity)`): the list jecs' uncached iterator
//!   walks. Terms the body could change are read once, up front.
//!
//! A query written with `.cached()` is left to jecs' iterator.
//! The binding must be an array pattern of plain names and holes (`[, a]`); a name past the query's terms is `nil`.
//! A `break` aimed at the loop sets a flag that also leaves the archetype walk.
//!
//! **`world.get(e, C)`** with one component calls a module-level helper with `world_get`'s body (the entity
//! index probe, then the column): about 20% faster interpreted and 40% native than the method, and an expression,
//! so it is right in a ternary, after `&&` and in a closure. `has`, `target`, `parent` and `pair` outside query
//! loops are not rewritten: a helper `has` is no faster interpreted, and the others are rare and cold in practice.

use luau::{
    AnyIdentifier, AssignmentOperator, BinaryOperator, ElseBody, Expr, Stmt, TemporaryIdentifier, UnaryOperator, create,
};
use tsrs_ast::{Kind, Node, NodeFlags};
use tsrs_core::P;

use crate::classes::prereqs::{Prereqs, temp_id};
use crate::classes::transform_state::TransformState;
use crate::nodes::binding::transform_binding_name::transform_binding_name;
use crate::nodes::expressions::transform_expression::transform_expression;
use crate::nodes::transform_statement_list::transform_body;
use crate::util::convert_to_indexable_expression::convert_to_indexable_expression;
use crate::util::ensure_transform_order::ensure_transform_order;
use crate::util::evaluation::effects::{effects_commute, get_effects, get_effects_of_statements};
use crate::util::symbols::{get_original_symbol_of_node, is_symbol_mutable};
use crate::util::traversal::{get_ancestor, skip_downwards, source_file_of};
use crate::util::value_to_id_str::value_to_id_str;

/// The module-level locals one file's jecs lowering declares.
#[derive(Default)]
pub struct Ecs {
    /// Memoized archetype lists and `pair` values, by the key of what they hold.
    memos: Vec<(String, TemporaryIdentifier)>,
    /// The `world.get` helper, once a call needs it.
    get: Option<TemporaryIdentifier>,
}

impl Ecs {
    /// The module-level local holding `key`, declared on first use.
    fn memo(&mut self, key: String, name: &str) -> TemporaryIdentifier {
        if let Some((_, id)) = self.memos.iter().find(|(k, _)| *k == key) {
            return id.clone();
        }
        let id = temp_id(name);
        self.memos.push((key, id.clone()));
        id
    }
}

/// The module-level statements the file's jecs lowering needs, for the top of the file.
pub fn module_statements(state: &mut TransformState<'_>) -> Vec<Stmt> {
    let ecs = std::mem::take(&mut state.ecs);
    let mut statements = Vec::new();
    if !ecs.memos.is_empty() {
        statements.push(create::locals(ecs.memos.into_iter().map(|(_, id)| id.into()).collect(), Vec::new()));
    }
    if let Some(get) = ecs.get {
        statements.push(get_helper(get));
    }
    statements
}

/// `local function _get(world, entity, id)`: jecs' `world_get` for one component.
fn get_helper(name: TemporaryIdentifier) -> Stmt {
    let (world, entity, id) = (create::id("world"), create::id("entity"), create::id("id"));
    let (entity_index, record, column) = (create::id("entity_index"), create::id("record"), create::id("column"));
    let any = |name: &str| create::any_id(name);
    let dead = create::binary(
        create::binary(record.clone(), BinaryOperator::Eq, create::nil()),
        BinaryOperator::Or,
        create::binary(
            create::computed_index(
                create::property(entity_index.clone(), "dense_array"),
                create::property(record.clone(), "dense"),
            ),
            BinaryOperator::Ne,
            entity.clone(),
        ),
    );
    let body = vec![
        create::local(any("entity_index"), Some(create::property(world, "entity_index"))),
        // ECS_ENTITY_T_LO: the low 24 bits are the entity's index
        create::local(
            any("record"),
            Some(create::computed_index(
                create::property(entity_index, "sparse_array"),
                create::binary(entity.clone(), BinaryOperator::Mod, create::number(16777216.0)),
            )),
        ),
        create::if_stmt(dead, vec![create::return_stmt(create::nil())], ElseBody::default()),
        create::local(
            any("column"),
            Some(create::computed_index(
                create::property(create::property(record.clone(), "archetype"), "columns_map"),
                id,
            )),
        ),
        create::if_stmt(
            create::binary(column.clone(), BinaryOperator::Eq, create::nil()),
            vec![create::return_stmt(create::nil())],
            ElseBody::default(),
        ),
        create::return_stmt(create::computed_index(column, create::property(record, "row"))),
    ];
    create::function_declaration(true, name.into(), vec![any("world"), any("entity"), any("id")], false, body)
}

// --- recognizing jecs ---

fn is_jecs(node: P<Node>) -> bool {
    source_file_of(node).get().file_name().contains("/node_modules/@rbxts/jecs/")
}

/// The jecs member a call resolves to, as (container, name): `("World", "query")`, `("Query", "with")`,
/// `("", "pair")`.
fn jecs_member(state: &mut TransformState<'_>, call: P<Node>) -> Option<(&'static str, &'static str)> {
    let declaration = state.checker.get_resolved_signature_exported(call).declaration()?;
    if !is_jecs(declaration) {
        return None;
    }
    let name = declaration.name()?.text();
    let container =
        get_ancestor(declaration, |n| matches!(n.kind(), Kind::ClassDeclaration | Kind::TypeAliasDeclaration))
            .and_then(|c| c.name())
            .map_or("", |n| n.text());
    Some((container, name))
}

/// A `const` declared by a statement at the top of a file (`const x = …`, `const { x } = …`, `declare const x`).
fn is_module_const(declaration: P<Node>) -> bool {
    let mut declaration = declaration;
    while declaration.kind() == Kind::BindingElement {
        let Some(declarator) = declaration.parent().and_then(|pattern| pattern.parent()) else { return false };
        declaration = declarator;
    }
    if declaration.kind() != Kind::VariableDeclaration {
        return false;
    }
    let Some(list) = declaration.parent() else { return false };
    let Some(statement) = list.parent() else { return false };
    list.kind() == Kind::VariableDeclarationList
        && (list.flags() & NodeFlags::BlockScoped) == NodeFlags::Const
        && statement.kind() == Kind::VariableStatement
        && statement.parent().is_some_and(|p| p.kind() == Kind::SourceFile)
}

fn declaration_key(declaration: P<Node>) -> Option<String> {
    let pos = declaration.pos().source_offset()?;
    Some(format!("{}:{pos}", source_file_of(declaration).get().file_name()))
}

/// The identity of a value fixed once its module has loaded, or `None`: a module-level const (or an import of
/// one), a namespace import's const member, or `pair(…)` of two such values.
fn static_key(state: &mut TransformState<'_>, node: P<Node>) -> Option<String> {
    let node = skip_downwards(node);
    match node.kind() {
        Kind::Identifier => {
            let declaration = get_original_symbol_of_node(state.checker, node)?.value_declaration()?;
            if is_module_const(declaration) { declaration_key(declaration) } else { None }
        }
        Kind::PropertyAccessExpression => {
            let access = node.as_property_access_expression();
            if access.question_dot_token().is_some() || skip_downwards(access.expression).kind() != Kind::Identifier {
                return None;
            }
            let namespace = state.checker.get_symbol_at_location_exported(skip_downwards(access.expression))?;
            if !namespace.declarations().first().is_some_and(|d| d.kind() == Kind::NamespaceImport) {
                return None;
            }
            let declaration = get_original_symbol_of_node(state.checker, access.name)?.value_declaration()?;
            if is_module_const(declaration) { declaration_key(declaration) } else { None }
        }
        Kind::CallExpression => {
            let arguments = node.arguments();
            if arguments.len() != 2 || jecs_member(state, node) != Some(("", "pair")) {
                return None;
            }
            let (first, second) = (static_key(state, arguments[0])?, static_key(state, arguments[1])?);
            Some(format!("pair({first},{second})"))
        }
        _ => None,
    }
}

fn is_pair_call(state: &mut TransformState<'_>, node: P<Node>) -> bool {
    let node = skip_downwards(node);
    node.kind() == Kind::CallExpression && jecs_member(state, node) == Some(("", "pair"))
}

// --- query loops ---

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    With,
    Without,
}

/// `world.query(ids…)` followed by `.with(…)` and `.without(…)` calls.
struct Chain {
    world: P<Node>,
    ids: &'static [P<Node>],
    /// In call order.
    filters: Vec<(Filter, &'static [P<Node>])>,
}

fn parse_chain(state: &mut TransformState<'_>, expression: P<Node>) -> Option<Chain> {
    let mut node = skip_downwards(expression);
    let mut filters = Vec::new();
    loop {
        if node.kind() != Kind::CallExpression || node.as_call_expression().question_dot_token().is_some() {
            return None;
        }
        let callee = node.as_call_expression().expression;
        if callee.kind() != Kind::PropertyAccessExpression {
            return None;
        }
        let access = callee.as_property_access_expression();
        if access.question_dot_token().is_some() || !matches!(access.name.text(), "query" | "with" | "without") {
            return None;
        }
        let arguments = node.arguments();
        if arguments.iter().any(|a| a.kind() == Kind::SpreadElement) {
            return None;
        }
        match jecs_member(state, node)? {
            ("World", "query") if !arguments.is_empty() => {
                filters.reverse();
                return Some(Chain { world: access.expression, ids: arguments, filters });
            }
            ("Query", "with") => filters.push((Filter::With, arguments)),
            ("Query", "without") => filters.push((Filter::Without, arguments)),
            // a cached query's list changes while it is walked: left to jecs' own iterator
            _ => return None,
        }
        node = skip_downwards(access.expression);
    }
}

/// The names of `for (const [e, a, b] of …)`, holes as `None`; `None` for any other initializer.
fn binding_names(initializer: P<Node>) -> Option<Vec<Option<P<Node>>>> {
    if initializer.kind() != Kind::VariableDeclarationList {
        return None;
    }
    let declarations = initializer.as_variable_declaration_list().declarations.nodes();
    let [declaration] = declarations else { return None };
    let pattern = declaration.as_variable_declaration().name;
    if pattern.kind() != Kind::ArrayBindingPattern {
        return None;
    }
    let mut names = Vec::new();
    for &element in pattern.elements() {
        if element.kind() == Kind::OmittedExpression {
            names.push(None);
            continue;
        }
        let binding = element.as_binding_element();
        if binding.dot_dot_dot_token().is_some() || binding.initializer().is_some() {
            return None;
        }
        match binding.name() {
            None => names.push(None),
            Some(name) if name.kind() == Kind::Identifier => names.push(Some(name)),
            Some(_) => return None,
        }
    }
    Some(names)
}

/// What a memoized list holds: the world, `query` ∪ the last `with`, and the last `without`.
fn memo_key(world: String, ids: &[String], filters: &[(Filter, Vec<String>)]) -> String {
    let last = |kind: Filter| filters.iter().rev().find(|(k, _)| *k == kind).map_or(&[][..], |(_, keys)| keys);
    let mut with: Vec<&String> = ids.iter().chain(last(Filter::With)).collect();
    let mut without: Vec<&String> = last(Filter::Without).iter().collect();
    for set in [&mut with, &mut without] {
        set.sort();
        set.dedup();
    }
    let join = |set: Vec<&String>| set.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",");
    format!("query {world} [{}] [{}]", join(with), join(without))
}

/// `world:query(ids):with(…):without(…):archetypes()`
fn archetypes_call(world: Expr, ids: Vec<Expr>, filters: Vec<(Filter, Vec<Expr>)>) -> Expr {
    let mut query = create::method_call(convert_to_indexable_expression(world), "query", ids);
    for (kind, args) in filters {
        query = create::method_call(query, if kind == Filter::With { "with" } else { "without" }, args);
    }
    create::method_call(query, "archetypes", Vec::new())
}

/// `if memo == nil then memo = value end`
fn memoize(memo: &TemporaryIdentifier, value: Expr) -> Stmt {
    let memo: Expr = memo.clone().into();
    create::if_stmt(
        create::binary(memo.clone(), BinaryOperator::Eq, create::nil()),
        vec![create::assignment(memo, AssignmentOperator::Assign, value)],
        ElseBody::default(),
    )
}

/// The lowered loop, or `None` when `node` is not a jecs query loop of a supported shape.
pub fn transform_query_loop(state: &mut TransformState<'_>, node: P<Node>) -> Option<Vec<Stmt>> {
    let for_of = node.as_for_in_or_of_statement();
    if for_of.await_modifier.is_some() {
        return None;
    }
    let names = binding_names(for_of.initializer)?;
    let chain = parse_chain(state, for_of.expression)?;

    // a memoized list needs the world and every term fixed once their modules have loaded
    let world_key = static_key(state, chain.world);
    let id_keys: Option<Vec<String>> = chain.ids.iter().map(|&id| static_key(state, id)).collect();
    let filter_keys: Option<Vec<(Filter, Vec<String>)>> = chain
        .filters
        .iter()
        .map(|&(kind, args)| Some((kind, args.iter().map(|&a| static_key(state, a)).collect::<Option<Vec<_>>>()?)))
        .collect();
    // the columns read: one per bound name after the entity
    let columns: Vec<usize> = (0..chain.ids.len()).filter(|&i| names.get(i + 1).is_some_and(Option::is_some)).collect();

    let mut prereqs = Prereqs::new();
    let archetypes: Expr;
    let column_keys: Vec<Expr>;
    if let (Some(world_key), Some(id_keys), Some(filter_keys)) = (world_key, id_keys, filter_keys) {
        let world = transform_expression(state, &mut prereqs, chain.world);
        let mut ids = Vec::new();
        for (&id, key) in chain.ids.iter().zip(&id_keys) {
            let value = transform_expression(state, &mut prereqs, id);
            ids.push(if is_pair_call(state, id) {
                let memo = state.ecs.memo(key.clone(), "pair");
                prereqs.push(memoize(&memo, value));
                memo.into()
            } else {
                value
            });
        }
        let mut filters = Vec::new();
        for &(kind, args) in &chain.filters {
            let values = args.iter().map(|&a| transform_expression(state, &mut prereqs, a)).collect();
            filters.push((kind, values));
        }
        let key = memo_key(world_key, &id_keys, &filter_keys);
        let version = state.ecs.memo(format!("version of {key}"), "version");
        let memo = state.ecs.memo(key, "archetypes");
        column_keys = columns.iter().map(|&i| ids[i].clone()).collect();
        // rebuilt when the world has made an archetype since: a destroyed one only ever stays empty
        let max_id = create::property(world.clone(), "max_archetype_id");
        let version: Expr = version.into();
        prereqs.push(create::if_stmt(
            create::binary(version.clone(), BinaryOperator::Ne, max_id.clone()),
            vec![
                create::assignment(
                    memo.clone().into(),
                    AssignmentOperator::Assign,
                    archetypes_call(world, ids, filters),
                ),
                create::assignment(version, AssignmentOperator::Assign, max_id),
            ],
            ElseBody::default(),
        ));
        archetypes = memo.into();
    } else {
        let mut nodes = vec![chain.world];
        nodes.extend(chain.ids);
        for &(_, args) in &chain.filters {
            nodes.extend(args);
        }
        let mut values = ensure_transform_order(state, &mut prereqs, &nodes);
        // a column key is read again for every archetype: anything the body could change is read once, up front,
        // along with everything before it so the order holds
        let last_unstable = columns.iter().rev().copied().find(|&i| !is_stable(state, nodes[i + 1], &values[i + 1]));
        if let Some(last) = last_unstable {
            for (value, &node) in values.iter_mut().zip(&nodes).take(last + 2) {
                if !is_stable(state, node, value) {
                    let name = non_empty(value_to_id_str(value), "term");
                    *value = prereqs.push_to_var(std::mem::replace(value, Expr::None), &name);
                }
            }
        }
        let mut values = values.into_iter();
        let world = values.next().expect("the world is the first value");
        let ids: Vec<Expr> = values.by_ref().take(chain.ids.len()).collect();
        let filters =
            chain.filters.iter().map(|&(kind, args)| (kind, values.by_ref().take(args.len()).collect())).collect();
        column_keys = columns.iter().map(|&i| ids[i].clone()).collect();
        archetypes = prereqs.push_to_var(archetypes_call(world, ids, filters), "archetypes");
    }

    // the loop's names, then its body
    let mut bindings = Vec::new();
    let mut entity = None;
    for (i, name) in names.iter().enumerate() {
        let Some(name) = *name else { continue };
        let id = transform_binding_name(state, name, &mut Vec::new());
        if i == 0 {
            entity = Some(id);
        } else {
            bindings.push((i - 1, id));
        }
    }
    let mut body = transform_body(state, for_of.statement);
    let broken = temp_id("broken");
    let breaks = redirect_breaks(&mut body, &broken);

    let archetype: Expr = temp_id("archetype").into();
    let entities: Expr = temp_id("entities").into();
    let row: Expr = temp_id("row").into();
    // named after the component, or after the name its values bind
    let column_ids: Vec<TemporaryIdentifier> = column_keys
        .iter()
        .zip(&columns)
        .map(|(key, &i)| temp_id(non_empty(value_to_id_str(key), names[i + 1].map_or("column", |n| n.text()))))
        .collect();

    let mut rows = Vec::new();
    let first = create::computed_index(entities.clone(), row.clone());
    let end = vec![create::break_stmt()];
    match entity {
        Some(entity) => {
            rows.push(create::local(entity.clone(), Some(first)));
            rows.push(create::if_stmt(
                create::binary(entity.into(), BinaryOperator::Eq, create::nil()),
                end,
                ElseBody::default(),
            ));
        }
        None => rows.push(create::if_stmt(
            create::binary(first, BinaryOperator::Eq, create::nil()),
            end,
            ElseBody::default(),
        )),
    }
    if !bindings.is_empty() {
        let (ids, values): (Vec<AnyIdentifier>, Vec<Expr>) = bindings
            .into_iter()
            .map(|(term, id)| {
                let value = match columns.iter().position(|&c| c == term) {
                    Some(c) => create::computed_index(column_ids[c].clone().into(), row.clone()),
                    None => create::nil(),
                };
                (id, value)
            })
            .unzip();
        rows.push(create::locals(ids, values));
    }
    rows.extend(body);

    let mut walk = vec![create::local(as_identifier(&entities), Some(create::property(archetype.clone(), "entities")))];
    if !column_ids.is_empty() {
        let columns_map = temp_id("columns");
        walk.push(create::local(columns_map.clone(), Some(create::property(archetype.clone(), "columns_map"))));
        let values =
            column_keys.into_iter().map(|key| create::computed_index(columns_map.clone().into(), key)).collect();
        walk.push(create::locals(column_ids.into_iter().map(Into::into).collect(), values));
    }
    if breaks {
        walk.push(create::local(broken.clone(), Some(create::bool(false))));
    }
    walk.push(create::numeric_for(
        as_identifier(&row),
        create::unary(UnaryOperator::Len, entities),
        create::number(1.0),
        Some(create::number(-1.0)),
        rows,
    ));
    if breaks {
        walk.push(create::if_stmt(broken.into(), vec![create::break_stmt()], ElseBody::default()));
    }

    let mut block = prereqs.statements;
    let scoped = block.iter().any(|s| matches!(s, Stmt::VariableDeclaration(_)));
    block.push(create::for_in(vec![temp_id("").into(), as_identifier(&archetype)], archetypes, walk));
    Some(if scoped { vec![create::do_stmt(block)] } else { block })
}

fn as_identifier(expression: &Expr) -> AnyIdentifier {
    AnyIdentifier::try_from(expression.clone()).expect("a temporary identifier")
}

fn non_empty(name: String, fallback: &str) -> String {
    if name.is_empty() { fallback.to_owned() } else { name }
}

/// A value the loop body cannot change: a literal, or a name bound by `const` (or an import).
fn is_stable(state: &mut TransformState<'_>, node: P<Node>, value: &Expr) -> bool {
    if value.is_simple_primitive() {
        return true;
    }
    let node = skip_downwards(node);
    value.is_any_identifier()
        && node.kind() == Kind::Identifier
        && get_original_symbol_of_node(state.checker, node).is_some_and(|s| !is_symbol_mutable(state, s))
}

/// Turns every `break` that leaves the loop body (not one inside a nested loop or function) into
/// `broken = true; break`. Whether any was found.
fn redirect_breaks(statements: &mut Vec<Stmt>, flag: &TemporaryIdentifier) -> bool {
    let mut found = false;
    let mut i = 0;
    while i < statements.len() {
        match &mut statements[i] {
            Stmt::Break => {
                statements
                    .insert(i, create::assignment(flag.clone().into(), AssignmentOperator::Assign, create::bool(true)));
                found = true;
                i += 1;
            }
            Stmt::Do(block) => found |= redirect_breaks(&mut block.statements, flag),
            Stmt::If(branch) => {
                let mut branch = &mut **branch;
                loop {
                    found |= redirect_breaks(&mut branch.statements, flag);
                    match &mut branch.else_body {
                        ElseBody::If(next) => branch = &mut **next,
                        ElseBody::Block(statements) => {
                            found |= redirect_breaks(statements, flag);
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    found
}

// --- world.get ---

/// `world.get(e, C)` as a call of the file's get helper, or `None` for any other call.
pub fn transform_world_get(
    state: &mut TransformState<'_>,
    prereqs: &mut Prereqs,
    node: P<Node>,
    base: &Expr,
    name: &str,
    arguments: &[P<Node>],
) -> Option<Expr> {
    if name != "get" || arguments.len() != 2 || arguments.iter().any(|a| a.kind() == Kind::SpreadElement) {
        return None;
    }
    if jecs_member(state, node)? != ("World", "get") {
        return None;
    }
    let mut args_prereqs = Prereqs::new();
    let args = ensure_transform_order(state, &mut args_prereqs, arguments);
    let mut world = base.clone();
    if !effects_commute(&get_effects(&world), &get_effects_of_statements(&args_prereqs.statements)) {
        world = prereqs.push_to_var(world, "world");
    }
    prereqs.push_list(args_prereqs.statements);
    let helper = state.ecs.get.get_or_insert_with(|| temp_id("get")).clone();
    let mut call_args = vec![world];
    call_args.extend(args);
    Some(create::call(helper.into(), call_args))
}

#[cfg(test)]
mod tests {
    use luau::{ElseBody, Stmt, create};

    use super::redirect_breaks;
    use crate::classes::prereqs::temp_id;

    #[test]
    fn only_breaks_that_leave_the_body_set_the_flag() {
        let flag = temp_id("broken");
        let set = || create::assignment(flag.clone().into(), luau::AssignmentOperator::Assign, create::bool(true));
        let nested = || create::while_stmt(create::bool(true), vec![create::break_stmt()]);
        let mut body = vec![
            nested(),
            create::function_declaration(true, create::id("f"), Vec::new(), false, vec![nested()]),
            create::if_stmt(
                create::bool(true),
                vec![nested()],
                ElseBody::If(Box::new(luau::IfStatement {
                    condition: create::bool(false),
                    statements: vec![create::do_stmt(vec![create::break_stmt()])],
                    else_body: ElseBody::Block(vec![create::break_stmt()]),
                })),
            ),
        ];
        assert!(redirect_breaks(&mut body, &flag));
        assert_eq!(body[0], nested());
        let Stmt::If(branch) = &body[2] else { panic!("an if") };
        assert_eq!(branch.statements, vec![nested()]);
        let ElseBody::If(elseif) = &branch.else_body else { panic!("an elseif") };
        assert_eq!(elseif.statements, vec![create::do_stmt(vec![set(), create::break_stmt()])]);
        assert_eq!(elseif.else_body, ElseBody::Block(vec![set(), create::break_stmt()]));
        assert!(!redirect_breaks(&mut vec![nested()], &flag));
    }
}
