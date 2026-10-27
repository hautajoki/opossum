//! Luau's limits (200 local registers, 255 registers and 200 upvalues per function, its nesting depth), checked on
//! every file a plugin emitted anything into (DESIGN §9: the compiler owns the check for all code, its own output
//! included). Files no plugin emitted into, and builds without plugins, are left as roblox-ts leaves them.
//!
//! Every file a plugin emitted into is compiled by Luau 0.740 once rendered: whether a file is within the limits is
//! decided by Luau's own compiler, not predicted, since whether a `local` takes a register depends on Luau's constant
//! folding, dead-branch elimination and inlining, which vary with the optimisation and debug levels and between
//! releases. The compiler is [`rbxts_luau_check`] (luau0-src 0.22.0+luau740, pinned exactly in that crate's
//! `Cargo.toml`; a bump can move a file across a limit).
//! 1. **Spill** (transform time): a cheap pre-count of the main chunk (every local name live at once, enclosing
//!    blocks and loop variables included, and the registers an expression holds on top) decides whether to spill the
//!    locals plugins hoisted with `hoist_spillable` into one table local: past 200 locals or 255 registers (an
//!    unneeded spill is harmless). The file is then marked for the load check ([`LoadCheck`]).
//! 2. **Load check** (after rendering, files in parallel: [`decide_all`]): Luau compiles the rendered file, its
//!    `--!optimize` directive honoured as in Roblox ([`decide`]). Refused at optimisation level 1 with debug level 1
//!    (Roblox's default when testing in Studio) or at level 2 (live games): an error quoting Luau's message, at the TS
//!    statement whose Luau holds the line Luau names (for a function, the main chunk's statement that contains it).
//!    Else refused at `--!optimize 0` or debug level 2: a warning, the file loads only with Luau's constant folding.
//!    Else it is clean.

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_luau::{
    AnyIdentifier, BinaryOperator, ElseBody, Expr, InterpolatedStringPart, Stmt, TableField, create,
};
use rbxts_luau_check::{CompileError, LUAU_VERSION};
use tsrs_ast::{Diagnostic, Node, SourceFile, new_diagnostic};
use tsrs_core::{P, TextRange};
use tsrs_diagnostics::{Category, new_ad_hoc_message};

use super::file_state::SpillableLocal;
use super::luau_walk::{Visit, walk_statements};
use crate::classes::prereqs::temp_id;
use crate::classes::transform_state::TransformState;
use crate::shared::diagnostics::{internal_error, node_range};

/// The most local registers Luau allows in one function, the main chunk included.
pub const LUAU_MAX_LOCALS: u32 = 200;

/// The most registers Luau allows in one function: locals and the values an expression holds.
pub const LUAU_MAX_REGISTERS: u32 = 255;

/// The main chunk's pre-count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pressure {
    /// The most local names live at once.
    locals: u32,
    /// The most local names live at once plus the registers an expression evaluated there holds.
    registers: u32,
}

impl Pressure {
    fn note(&mut self, active: u32, extra: u32) {
        self.locals = self.locals.max(active);
        self.registers = self.registers.max(active + extra);
    }
}

/// The registers evaluating `e` holds at once (an estimate: one per value held; a function is one, its body is its
/// own).
fn expr_registers(e: &Expr) -> u32 {
    match e {
        Expr::ComputedIndex(c) => expr_registers(&c.expression).max(1 + expr_registers(&c.index)),
        Expr::PropertyAccess(p) => expr_registers(&p.expression),
        Expr::Call(c) => expr_registers(&c.expression).max(list_registers(c.args.iter(), 1)),
        Expr::MethodCall(m) => expr_registers(&m.expression).max(list_registers(m.args.iter(), 2)),
        Expr::Parenthesized(p) => expr_registers(&p.expression),
        // Luau evaluates a whole `..` chain into consecutive registers (rendered without parentheses, it is one chain)
        Expr::Binary(b) if b.operator == BinaryOperator::Concat => {
            let mut operands = Vec::new();
            concat_operands(e, &mut operands);
            list_registers(operands.into_iter(), 0)
        }
        Expr::Binary(b) => expr_registers(&b.left).max(1 + expr_registers(&b.right)),
        Expr::Unary(u) => expr_registers(&u.expression),
        Expr::If(i) => {
            expr_registers(&i.condition).max(expr_registers(&i.expression)).max(expr_registers(&i.alternative))
        }
        Expr::InterpolatedString(s) => list_registers(
            s.parts.iter().filter_map(|part| match part {
                InterpolatedStringPart::Expression(e) => Some(e),
                InterpolatedStringPart::Text(_) => None,
            }),
            1,
        ),
        Expr::Array(items) | Expr::Set(items) => list_registers(items.iter(), 1),
        Expr::Map(fields) => {
            fields.iter().map(|f| 1 + expr_registers(&f.index).max(1 + expr_registers(&f.value))).max().unwrap_or(1)
        }
        Expr::MixedTable(fields) => {
            let mut most = 1;
            let mut held = 1;
            for field in fields {
                match field {
                    TableField::Value(e) => {
                        most = most.max(held + expr_registers(e));
                        held += 1;
                    }
                    TableField::Field(f) => {
                        most = most.max(held + expr_registers(&f.index).max(1 + expr_registers(&f.value)));
                    }
                }
            }
            most
        }
        Expr::Identifier(_)
        | Expr::TemporaryIdentifier(_)
        | Expr::Function(_)
        | Expr::None
        | Expr::Nil
        | Expr::False
        | Expr::True
        | Expr::Number(_)
        | Expr::String(_)
        | Expr::LateString(_)
        | Expr::VarArgs => 1,
    }
}

/// The operands of a `..` chain, in order (not through parentheses, which Luau keeps).
fn concat_operands<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match e {
        Expr::Binary(b) if b.operator == BinaryOperator::Concat => {
            concat_operands(&b.left, out);
            concat_operands(&b.right, out);
        }
        e => out.push(e),
    }
}

/// Values evaluated into consecutive registers after `held` others (call arguments, table items, a statement's
/// values): the most registers at once.
fn list_registers<'a>(values: impl Iterator<Item = &'a Expr>, held: u32) -> u32 {
    values.enumerate().map(|(i, value)| held + i as u32 + expr_registers(value)).max().unwrap_or(held)
}

/// A block's statements on top of `active` live locals; returns the locals live at its end.
fn block_pressure(statements: &[Stmt], start: u32, most: &mut Pressure) -> u32 {
    let mut active = start;
    for statement in statements {
        active += statement_pressure(statement, active, most);
        most.note(active, 0);
    }
    active
}

/// What a statement declares in its block, on top of `active` live locals.
fn statement_pressure(statement: &Stmt, active: u32, most: &mut Pressure) -> u32 {
    match statement {
        Stmt::Assignment(a) => {
            let targets = a.left.iter().map(expr_registers).max().unwrap_or(0);
            most.note(active, list_registers(a.right.iter(), 0) + targets);
            0
        }
        Stmt::Break | Stmt::Continue | Stmt::Comment(_) => 0,
        Stmt::Call(e) => {
            most.note(active, expr_registers(e));
            0
        }
        Stmt::Do(block) => {
            block_pressure(&block.statements, active, most);
            0
        }
        Stmt::While(w) => {
            most.note(active, expr_registers(&w.condition));
            block_pressure(&w.statements, active, most);
            0
        }
        Stmt::Repeat(r) => {
            // the condition sees the block's locals
            let inner = block_pressure(&r.statements, active, most);
            most.note(inner, expr_registers(&r.condition));
            0
        }
        Stmt::If(i) => {
            let mut current = Some(i.as_ref());
            while let Some(i) = current {
                most.note(active, expr_registers(&i.condition));
                block_pressure(&i.statements, active, most);
                current = match &i.else_body {
                    ElseBody::If(next) => Some(next.as_ref()),
                    ElseBody::Block(statements) => {
                        block_pressure(statements, active, most);
                        None
                    }
                };
            }
            0
        }
        Stmt::NumericFor(f) => {
            let mut registers = expr_registers(&f.start).max(1 + expr_registers(&f.end));
            if let Some(step) = &f.step {
                registers = registers.max(2 + expr_registers(step));
            }
            // the loop's three internal registers, then its variable
            most.note(active, registers.max(3));
            block_pressure(&f.statements, active + 4, most);
            0
        }
        Stmt::For(f) => {
            most.note(active, expr_registers(&f.expression).max(3));
            block_pressure(&f.statements, active + 3 + f.ids.len() as u32, most);
            0
        }
        Stmt::FunctionDeclaration(f) => {
            let declared = u32::from(f.localize);
            most.note(active + declared, 1 + if f.localize { 0 } else { expr_registers(&f.name) });
            declared
        }
        Stmt::MethodDeclaration(m) => {
            most.note(active, expr_registers(&m.expression) + 1);
            0
        }
        Stmt::VariableDeclaration(declaration) => {
            let declared = declaration.left.len() as u32;
            most.note(active, list_registers(declaration.right.iter(), 0).max(declared));
            declared
        }
        Stmt::Return(r) => {
            most.note(active, list_registers(r.expressions.iter(), 0));
            0
        }
    }
}

/// The main chunk's pre-count.
fn main_pressure(statements: &[Stmt]) -> Pressure {
    let mut most = Pressure::default();
    block_pressure(statements, 0, &mut most);
    most
}

/// The level a `--!optimize N` directive among the file's leading comments sets, if it has one.
fn optimize_directive(statements: &[Stmt]) -> Option<u32> {
    statements
        .iter()
        .map_while(|s| match s {
            Stmt::Comment(text) => Some(text),
            _ => None,
        })
        .find_map(|text| text.strip_prefix("!optimize").map(|level| level.trim().parse().unwrap_or(1)))
}

/// A file a plugin emitted into, for the load check: where its TS statements' Luau ends in the final statement list
/// (for each, the index past its last statement, and the statement's text range), and its `--!optimize` directive's
/// level. Text ranges rather than nodes: a watch build keeps the mark with a cached file's Luau (its late strings may
/// change value and decide whether it loads), and the file's text, not its node objects, is what stays the same.
#[derive(Clone, Debug)]
pub(crate) struct LoadCheck {
    statement_ends: Vec<(usize, TextRange)>,
    directive: Option<u32>,
}

impl LoadCheck {
    /// The text of the TS statement whose Luau holds the final statement `index` (the file's start, before the first).
    fn anchor(&self, index: usize) -> TextRange {
        self.statement_ends.iter().find(|(end, _)| *end > index).map_or(TextRange::new(0, 0), |(_, range)| *range)
    }

    /// The file has a `--!optimize` directive (the load check then compiles at every level it needs).
    pub(crate) fn has_directive(&self) -> bool {
        self.directive.is_some()
    }
}

/// Checks a complete file a plugin emitted anything into (`plugin_file.emitted`): spills the `hoist_spillable`
/// locals when the main chunk may pass Luau's limits, and marks the file for the load check
/// (`plugin_file.load_check`). Files no plugin emitted into are left as roblox-ts leaves them.
pub(crate) fn check_main_chunk(state: &mut TransformState<'_>, statements: &mut Vec<Stmt>) {
    if !state.plugin_file.emitted {
        return;
    }
    let pressure = main_pressure(statements);
    let mut spilled_at = None;
    if (pressure.locals > LUAU_MAX_LOCALS || pressure.registers > LUAU_MAX_REGISTERS)
        && !state.plugin_file.spillable.is_empty()
    {
        spilled_at = spill(state, statements);
    }
    state.plugin_file.load_check = Some(LoadCheck {
        statement_ends: statement_ends(state, spilled_at),
        directive: optimize_directive(statements),
    });
}

/// Where Luau refused a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    error: CompileError,
    optimization_level: u8,
    debug_level: u8,
}

/// What Luau's compiler says about a rendered file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Loads,
    /// It loads at optimisation levels 1 and 2 at debug level 1, but not at `--!optimize 0` or debug level 2: only
    /// with Luau's constant folding.
    FoldingOnly(Refusal),
    /// It fails at optimisation level 1 or 2 at debug level 1: in Studio or in live games.
    Fails(Refusal),
    /// The check itself panicked (a compiler bug): the file's internal error.
    Panicked(String),
}

/// Compiles `source` as Roblox loads it. Without a `--!optimize` directive, a file that loads at level 0 (debug 1, the
/// level that folds nothing) and at level 2 (debug 1, inlining) loads at level 1 with debug levels 1 and 2 too (every
/// probe of the limit validations bears this out), so a clean file takes two compiles. Otherwise, or when one of them
/// fails, the levels Roblox runs are compiled first, then the ones that fold nothing.
pub(crate) fn decide(source: &str, directive: bool) -> Verdict {
    let at = |optimization_level, debug_level| {
        rbxts_luau_check::compile(source, optimization_level, debug_level).err().map(|error| Refusal {
            error,
            optimization_level,
            debug_level,
        })
    };
    let mut unfolded = None;
    if !directive {
        let level_0 = at(0, 1);
        if level_0.is_none() {
            return match at(2, 1) {
                None => Verdict::Loads,
                Some(level_2) => Verdict::Fails(at(1, 1).unwrap_or(level_2)),
            };
        }
        unfolded = Some(level_0);
    }
    if let Some(refusal) = at(1, 1).or_else(|| at(2, 1)) {
        return Verdict::Fails(refusal);
    }
    match unfolded.unwrap_or_else(|| at(0, 1)).or_else(|| at(1, 2)) {
        Some(refusal) => Verdict::FoldingOnly(refusal),
        None => Verdict::Loads,
    }
}

/// The stack each load-check thread gets: Luau's compiler recurses up to its nesting limit (1000 levels), which took
/// at most about 1.1 MiB of stack on the deepest shapes it accepts or refuses (`rbxts_luau_check`'s `stack` example,
/// dev and measure profiles alike); 8 MiB leaves a wide margin.
const LOAD_CHECK_STACK: usize = 8 << 20;

/// [`decide`], with a panic as the file's [`Verdict::Panicked`] rather than the build's crash.
fn decide_caught(source: &str, directive: bool) -> Verdict {
    crate::compile::quietly(|| decide(source, directive))
        .unwrap_or_else(|panic| Verdict::Panicked(crate::compile::panic_message(&*panic)))
}

/// [`decide`] for each `(source, directive)`, in parallel on up to one thread per core. Luau's compiler is reentrant.
pub(crate) fn decide_all(jobs: &[(&str, bool)]) -> Vec<Verdict> {
    let threads = std::thread::available_parallelism().map_or(1, usize::from).min(jobs.len());
    if threads <= 1 {
        return jobs.iter().map(|&(source, directive)| decide_caught(source, directive)).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let work = || {
        let mut done = Vec::new();
        loop {
            let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(&(source, directive)) = jobs.get(index) else { return done };
            done.push((index, decide_caught(source, directive)));
        }
    };
    let mut verdicts: Vec<Option<Verdict>> = vec![None; jobs.len()];
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .filter_map(|_| std::thread::Builder::new().stack_size(LOAD_CHECK_STACK).spawn_scoped(scope, work).ok())
            .collect();
        // if no thread could be made, this one (the transform's, with its large stack) does the work
        let mut results = if workers.is_empty() { work() } else { Vec::new() };
        // (a worker's own panic, outside `decide_caught`, leaves its files unchecked: each is Panicked below)
        for worker in workers {
            if let Ok(done) = worker.join() {
                results.extend(done);
            }
        }
        for (index, verdict) in results {
            verdicts[index] = Some(verdict);
        }
    });
    verdicts
        .into_iter()
        .map(|verdict| verdict.unwrap_or_else(|| Verdict::Panicked("a load-check thread stopped".to_owned())))
        .collect()
}

/// The diagnostic for a rendered file's verdict: an error when Luau refuses it where Roblox runs it, a warning when it
/// loads only with Luau's constant folding. `lines` is the line each final statement starts on; `file` is the TS
/// file (of the program being built: for a cached file, the same text the mark was made from).
pub(crate) fn load_check_diagnostic(
    check: &LoadCheck,
    verdict: &Verdict,
    lines: &[u32],
    file: P<SourceFile>,
) -> Option<P<Diagnostic>> {
    let refusal = match verdict {
        Verdict::Loads => return None,
        Verdict::Panicked(message) => {
            return Some(internal_error(file.as_node(), &format!("the Luau load check panicked: {message}")));
        }
        Verdict::Fails(refusal) | Verdict::FoldingOnly(refusal) => refusal,
    };
    let index = lines.partition_point(|&line| line <= refusal.error.line).saturating_sub(1);
    let anchor = check.anchor(index);
    let luau_says = format!("\"{}\" (output line {})", refusal.error.message, refusal.error.line);
    let level = |optimization_level: u8| match check.directive {
        Some(directive) => format!("optimisation level {directive} (the file's --!optimize directive)"),
        None => format!("optimisation level {optimization_level}"),
    };
    let (category, message) = match verdict {
        Verdict::Fails(refusal) => {
            let r#where = if refusal.optimization_level == 1 {
                "Roblox's default when testing in Studio"
            } else {
                "what live games run"
            };
            (
                Category::Error,
                format!(
                    "Luau {LUAU_VERSION} refuses this file at {} and debug level 1, {where}: {luau_says}. It would \
                     build, then fail to load.",
                    level(refusal.optimization_level)
                ),
            )
        }
        _ => {
            // a directive sets the optimisation level, so only the debug level can make the file fail
            let fails_at = if check.directive.is_some() { "debug level 2" } else { "--!optimize 0 or debug level 2" };
            (
                Category::Warning,
                format!(
                    "This file loads only with Luau's constant folding: Luau {LUAU_VERSION} refuses it at {} and \
                     debug level {}: {luau_says}. It fails to load at {fails_at}.",
                    level(refusal.optimization_level),
                    refusal.debug_level
                ),
            )
        }
    };
    let diagnostic = new_diagnostic(Some(file), anchor, new_ad_hoc_message(&message), &[]);
    if category != Category::Error {
        diagnostic.set_category(category);
    }
    Some(diagnostic)
}

/// For each top-level TS statement, the index past its Luau in the final statement list: the root ranges, moved by
/// the statements inserted in front of them since (`plugin_file.front_inserts`) and by the spill table.
fn statement_ends(state: &TransformState<'_>, spilled_at: Option<usize>) -> Vec<(usize, TextRange)> {
    let shift = |mut i: usize| {
        for &(at, count) in &state.plugin_file.front_inserts {
            if i >= at {
                i += count;
            }
        }
        if let Some(at) = spilled_at
            && i >= at
        {
            i += 1;
        }
        i
    };
    state.plugin_file.root_ranges.iter().map(|range| (shift(range.end), node_range(range.node).1)).collect()
}

/// The spilled hoists' table and fields.
struct Spill {
    table: Expr,
    fields: FxHashMap<SpillableLocal, String>,
}

impl Spill {
    /// The field of a spilled local, if `id` is one.
    fn field(&self, id: &AnyIdentifier) -> Option<&String> {
        match id {
            AnyIdentifier::Temporary(t) => self.fields.get(&SpillableLocal::Temporary(t.id)),
            AnyIdentifier::Identifier(i) => self.named_field(&i.name),
        }
    }

    fn named_field(&self, name: &str) -> Option<&String> {
        // (named hoists are rare: skip the String allocation when there are none)
        if !self.fields.keys().any(|k| matches!(k, SpillableLocal::Named(_))) {
            return None;
        }
        self.fields.get(&SpillableLocal::Named(name.to_owned()))
    }
}

/// Moves every `hoist_spillable` local whose declaration is a statement of the main chunk into one table local:
/// `local _spill = {}` where the first was declared, `_spill.<hint> = value` for each, and every use of one
/// becomes `_spill.<hint>`. Returns where the table's declaration went.
fn spill(state: &mut TransformState<'_>, statements: &mut Vec<Stmt>) -> Option<usize> {
    let spillable: FxHashMap<SpillableLocal, String> =
        state.plugin_file.spillable.iter().map(|hoist| (hoist.local.clone(), hoist.hint.clone())).collect();
    // the declarations, in file order
    let declarations: Vec<(usize, SpillableLocal)> = statements
        .iter()
        .enumerate()
        .filter_map(|(i, statement)| match statement {
            Stmt::VariableDeclaration(declaration) => match declaration.left.as_slice() {
                [id] if declaration.right.len() == 1 => {
                    let local = match id {
                        AnyIdentifier::Temporary(t) => SpillableLocal::Temporary(t.id),
                        AnyIdentifier::Identifier(i) => SpillableLocal::Named(i.name.clone()),
                    };
                    spillable.contains_key(&local).then_some((i, local))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    let &(first, _) = declarations.first()?;
    let table_id = temp_id(state.plugin_file.seam_hint(state.source_file, "spill"));
    let table = Expr::TemporaryIdentifier(table_id.clone());
    let mut fields = FxHashMap::default();
    let mut taken = FxHashSet::default();
    for (_, id) in &declarations {
        let hint = &spillable[id];
        let base = if rbxts_luau::is_valid_identifier(hint) { hint.clone() } else { "field".to_owned() };
        let field = (1..)
            .map(|n| if n == 1 { base.clone() } else { format!("{base}_{n}") })
            .find(|name| !taken.contains(name))
            .expect("a free field name");
        taken.insert(field.clone());
        fields.insert(id.clone(), field);
    }
    let spill = Spill { table, fields };
    for (i, id) in &declarations {
        let i = *i;
        let Stmt::VariableDeclaration(declaration) = &mut statements[i] else { unreachable!("found above") };
        let value = declaration.right.pop().expect("one value");
        statements[i] = create::assignment(
            create::property(spill.table.clone(), spill.fields[id].clone()),
            rbxts_luau::AssignmentOperator::Assign,
            value,
        );
    }
    let mut misplaced = false;
    walk_statements(statements, &mut |visit| match visit {
        Visit::Declared(ids) => misplaced |= ids.iter().any(|id| spill.field(id).is_some()),
        Visit::LocalFunction(name) => {
            misplaced |= match name {
                Expr::TemporaryIdentifier(t) => spill.fields.contains_key(&SpillableLocal::Temporary(t.id)),
                Expr::Identifier(i) => spill.named_field(&i.name).is_some(),
                _ => false,
            }
        }
        Visit::Expr(expr) => {
            let field = match &*expr {
                Expr::TemporaryIdentifier(t) => spill.fields.get(&SpillableLocal::Temporary(t.id)),
                Expr::Identifier(i) => spill.named_field(&i.name),
                _ => None,
            };
            if let Some(field) = field {
                *expr = create::property(spill.table.clone(), field.clone());
            }
        }
    });
    // the table first: `local _spill = {}` before the first field assignment
    statements.insert(first, create::local(table_id, Some(create::map([]))));
    if misplaced {
        let node: P<Node> = state.source_file.as_node();
        state.plugin_internal_error(
            node,
            "a hoist_spillable local was declared again by plugin code (as a parameter, loop variable or local), so \
             it cannot be spilled into a table",
        );
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use rbxts_luau::BinaryOperator as B;

    use super::*;

    fn f() -> Expr {
        create::call(create::id("f"), vec![])
    }

    /// `local <prefix>0 = value` … `local <prefix><count - 1> = value`
    fn locals(prefix: &str, count: usize, value: &dyn Fn() -> Expr) -> Vec<Stmt> {
        (0..count).map(|i| create::local(create::any_id(format!("{prefix}{i}")), Some(value()))).collect()
    }

    fn chain(count: usize, operator: B) -> Expr {
        (1..count).fold(create::id("x0"), |e, i| create::binary(e, operator, create::id(format!("x{i}"))))
    }

    #[test]
    fn the_pre_count_counts_every_name_and_whole_concat_chains() {
        // constants count: whether Luau folds them is the load check's question
        assert_eq!(main_pressure(&locals("x", 195, &|| create::number(1.0))).locals, 195);
        // nested blocks and loop variables add to the enclosing locals; function bodies are their own
        let mut statements = locals("x", 150, &f);
        let body = locals("y", 30, &f);
        statements.push(create::numeric_for(create::any_id("i"), create::number(1.0), create::number(3.0), None, body));
        statements.push(create::function_declaration(true, create::id("g"), vec![], false, locals("z", 100, &f)));
        let pressure = main_pressure(&statements);
        assert_eq!(pressure.locals, 150 + 4 + 30);
        // a call's arguments, and a `..` chain, are held at once; a `+` chain is not
        let mut statements = locals("x", 150, &f);
        statements.push(create::call_statement(create::call(create::id("print"), vec![chain(98, B::Concat)])));
        assert_eq!(main_pressure(&statements).registers, 150 + 1 + 98);
        let mut statements = locals("x", 150, &f);
        statements.push(create::call_statement(create::call(create::id("print"), vec![chain(98, B::Add)])));
        assert_eq!(main_pressure(&statements).registers, 150 + 1 + 2);
    }

    /// `count` lines of `local x<i> = <value>`.
    fn lua(count: usize, value: &str) -> String {
        (0..count).map(|i| format!("local x{i} = {value}\n")).collect()
    }

    fn refused_at(verdict: &Verdict) -> Option<(u8, u8, &str)> {
        match verdict {
            Verdict::Loads | Verdict::Panicked(_) => None,
            Verdict::Fails(r) | Verdict::FoldingOnly(r) => {
                Some((r.optimization_level, r.debug_level, r.error.message.as_str()))
            }
        }
    }

    #[test]
    fn luau_decides() {
        assert_eq!(decide(&lua(200, "f()"), false), Verdict::Loads);
        assert_eq!(decide(&lua(200, "f()"), true), Verdict::Loads);
        // 201 calls: refused wherever it runs
        let verdict = decide(&lua(201, "f()"), false);
        assert!(matches!(verdict, Verdict::Fails(_)), "{verdict:?}");
        let (o, d, message) = refused_at(&verdict).unwrap();
        assert_eq!((o, d), (1, 1));
        assert!(message.starts_with("Out of local registers"), "{message}");
        // 201 constants: folded at optimisation level 1+ and debug level 1 only
        let verdict = decide(&lua(201, "1"), false);
        assert!(matches!(verdict, Verdict::FoldingOnly(_)), "{verdict:?}");
        assert_eq!(refused_at(&verdict).map(|(o, d, _)| (o, d)), Some((0, 1)));
        // dead branches: skipped from level 1
        let dead = format!("{}if false then\n{}end\n", lua(195, "f()"), lua(10, "f()"));
        assert!(matches!(decide(&dead, false), Verdict::FoldingOnly(_)), "{:?}", decide(&dead, false));
        // the file's directive wins; with `--!optimize 2` level 0 is never compiled, so debug level 2 is asked
        assert!(matches!(decide(&format!("--!optimize 0\n{}", lua(201, "1")), true), Verdict::Fails(_)));
        let verdict = decide(&format!("--!optimize 2\n{}", lua(201, "1")), true);
        assert_eq!(refused_at(&verdict).map(|(o, d, _)| (o, d)), Some((1, 2)), "{verdict:?}");
        // upvalues and nesting depth are Luau's to judge too
        let names = (0..201).map(|i| format!("x{i}")).collect::<Vec<_>>().join(", ");
        let verdict = decide(&format!("{}local g = function() print({names}) end\n", lua(201, "f()")), false);
        assert!(refused_at(&verdict).unwrap().2.contains("upvalue"), "{verdict:?}");
        let elseifs = (0..1005).map(|i| format!("elseif y == {i} then\n\tprint({i})\n")).collect::<String>();
        let verdict = decide(&format!("if y then\n\tprint(0)\n{elseifs}end\n"), false);
        assert!(refused_at(&verdict).unwrap().2.starts_with("Exceeded allowed recursion depth"), "{verdict:?}");
    }

    #[test]
    fn files_are_decided_in_parallel_as_one_by_one() {
        let sources: Vec<String> = (0..40)
            .map(|i| match i % 4 {
                0 => lua(201, "f()"),
                1 => lua(201, "1"),
                2 => lua(150, "f()"),
                _ => format!("--!optimize 2\n{}", lua(201, "1")),
            })
            .collect();
        let jobs: Vec<(&str, bool)> = sources.iter().map(|s| (s.as_str(), s.starts_with("--!"))).collect();
        let one_by_one: Vec<Verdict> = jobs.iter().map(|&(source, directive)| decide(source, directive)).collect();
        assert_eq!(decide_all(&jobs), one_by_one);
        assert_eq!(decide_all(&[]), Vec::new());
    }
}
