//! `identity`: claims nodes and lowers them as roblox-ts would. Its output must equal a build without plugins: a
//! check of the lowering primitives and of the claim points on a whole corpus (`rbxts-bench build --plugin
//! identity-expr --parity …`).
//!
//! - [`IdentityMode::Expr`] / [`IdentityMode::Call`]: every call, `new` and decorator by symbol, lowered with
//!   `compile_expr(call.node)` or `compile_call(Callee::Original, all Original)`. Calls of the compiler-types
//!   (roblox-ts's macros) and of `@rbxts/types` (where some of them are declared) are declined, as `compile_call`
//!   cannot pass argument slots to a macro.
//! - [`IdentityMode::Key`]: every property and element access through `property_key`, answered with its key (which
//!   leaves the access as written), computed keys (`AccessSite::key` `None`) declined. The markers are every property
//!   name the program's accesses use, collected in `setup`, so every access whose receiver has the member it reads is
//!   offered.
//! - [`IdentityMode::Assign`]: every assignment, update and `delete` of a property through the `assignment` hook,
//!   lowered with `compile_expr(site.node)`.
//! - [`IdentityMode::Fallback`]: the expression fallback for most expression kinds, each lowered with
//!   `compile_expr(node)`; binary operators with `compile_binary(op, left, right)` and object literals with
//!   `compile_object_literal(node, no edits)`.

use std::sync::Arc;

use rbxts_plugin::ts::{Kind, Node, P, Symbol};
use rbxts_plugin::{
    AccessSite, Arg, AssignSite, CallKind, CallSite, Callee, CheckerInfo, Claim, Cx, Fatal, FileIndex, FinishCx,
    Interest, LuauExpr, ObjectEdits, PResult, Plugin, PluginInfo, PluginInstance, SetupCx, SymbolRole, Tag,
};

/// How `identity` lowers what it claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityMode {
    /// Calls: `compile_expr(call.node)` (a decorator: `compile_expr` of its expression).
    Expr,
    /// Calls: `compile_call(Callee::Original, every argument Original)` (a decorator without a call: `compile_expr`).
    Call,
    /// Accesses: `property_key` answers the written key.
    Key,
    /// Assignments: `compile_expr(site.node)`.
    Assign,
    /// The expression fallback: `compile_expr(node)`, `compile_binary`, `compile_object_literal`.
    Fallback,
}

/// The `identity` plugin, lowering what it claims in one [`IdentityMode`].
pub struct Identity(pub IdentityMode);

/// What setup decided: the mode and its markers.
pub struct IdentityShared {
    mode: IdentityMode,
    /// [`IdentityMode::Key`]: the markers (every accessed property name).
    markers: Vec<&'static str>,
}

/// The per-checker instance.
pub struct IdentityInstance(Arc<IdentityShared>);

/// The kinds [`IdentityMode::Fallback`] claims.
const FALLBACK_KINDS: &[Kind] = &[
    Kind::CallExpression,
    Kind::NewExpression,
    Kind::PropertyAccessExpression,
    Kind::ElementAccessExpression,
    Kind::BinaryExpression,
    Kind::PrefixUnaryExpression,
    Kind::PostfixUnaryExpression,
    Kind::DeleteExpression,
    Kind::ObjectLiteralExpression,
    Kind::ArrayLiteralExpression,
    Kind::ConditionalExpression,
    Kind::ParenthesizedExpression,
    Kind::TemplateExpression,
    Kind::TaggedTemplateExpression,
    Kind::AwaitExpression,
    Kind::VoidExpression,
    Kind::Identifier,
    Kind::ThisKeyword,
    Kind::ArrowFunction,
    Kind::FunctionExpression,
    Kind::AsExpression,
    Kind::NonNullExpression,
    Kind::SpreadElement,
    Kind::StringLiteral,
    Kind::NumericLiteral,
];

/// Every property name the program's accesses use (`obj.name`, `obj["name"]`).
pub(crate) fn accessed_names(cx: &SetupCx<'_>) -> Vec<&'static str> {
    fn walk(node: P<Node>, out: &mut rustc_hash::FxHashSet<&'static str>) {
        match node.kind() {
            Kind::PropertyAccessExpression => {
                out.insert(node.as_property_access_expression().name.text());
            }
            Kind::ElementAccessExpression => {
                let argument = node.as_element_access_expression().argument_expression;
                if matches!(argument.kind(), Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral) {
                    out.insert(argument.text());
                }
            }
            _ => {}
        }
        node.for_each_child(&mut |child| {
            walk(child, out);
            false
        });
    }
    let mut names = rustc_hash::FxHashSet::default();
    for &file in cx.program().source_files() {
        if !file.get().is_declaration_file() {
            walk(file.as_node(), &mut names);
        }
    }
    let mut names: Vec<&'static str> = names.into_iter().collect();
    names.sort_unstable();
    names
}

impl Plugin for Identity {
    type Shared = IdentityShared;
    type State = ();
    type FilePart = ();
    type Instance = IdentityInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo { name: "identity", version: "0.1.0", tsconfig_transform: None }
    }

    fn setup(&self, cx: &mut SetupCx<'_>, _: &mut ()) -> Result<Arc<IdentityShared>, Fatal> {
        let markers = if self.0 == IdentityMode::Key { accessed_names(cx) } else { Vec::new() };
        Ok(Arc::new(IdentityShared { mode: self.0, markers }))
    }

    fn instance(&self, shared: Arc<IdentityShared>, _: CheckerInfo) -> IdentityInstance {
        IdentityInstance(shared)
    }

    fn finish(
        &self,
        _: &IdentityShared,
        _: &mut (),
        _: &[(FileIndex, &())],
        _: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        Ok(())
    }
}

impl IdentityInstance {
    fn mode(&self) -> IdentityMode {
        self.0.mode
    }
}

impl PluginInstance for IdentityInstance {
    type FilePart = ();

    fn interest(&self, interest: &mut Interest) {
        match self.mode() {
            IdentityMode::Expr | IdentityMode::Call => interest.calls_by_symbol(),
            IdentityMode::Key => {
                for &marker in &self.0.markers {
                    interest.accesses_by_marker(marker, Tag(0));
                }
            }
            IdentityMode::Assign => interest.assignments_by_receiver_symbol(),
            IdentityMode::Fallback => {
                for &kind in FALLBACK_KINDS {
                    interest.expressions(kind, Tag(0));
                }
            }
        }
    }

    fn classify_symbol(&self, cx: &mut Cx<'_>, symbol: P<Symbol>, role: SymbolRole) -> Option<Tag> {
        match role {
            SymbolRole::AssignReceiver => Some(Tag(0)),
            SymbolRole::Callee => {
                // roblox-ts's macros are declared in these two packages (`String.gmatch`, `Vector3.mul`, …)
                let macros = matches!(
                    cx.declaring_package(symbol).and_then(|p| p.name.clone()).as_deref(),
                    Some("@rbxts/compiler-types" | "@rbxts/types")
                );
                (!macros).then_some(Tag(0))
            }
        }
    }

    fn call(&self, cx: &mut Cx<'_>, call: &CallSite, _: Tag) -> PResult<Claim<LuauExpr>> {
        let value = if call.kind == CallKind::Decorator {
            let expression = call.node.expression().expect("a decorator has an expression");
            if self.mode() == IdentityMode::Call && expression.kind() == Kind::CallExpression {
                cx.compile_call(call, Callee::Original, call.args.iter().map(|&a| Arg::Original(a)).collect())?
            } else {
                cx.compile_expr(expression)?
            }
        } else if self.mode() == IdentityMode::Expr {
            cx.compile_expr(call.node)?
        } else {
            cx.compile_call(call, Callee::Original, call.args.iter().map(|&a| Arg::Original(a)).collect())?
        };
        Ok(Claim::Take(value))
    }

    fn property_key(&self, _: &mut Cx<'_>, access: &AccessSite, _: Tag) -> PResult<Claim<String>> {
        // a computed key passes through: it can only be declined
        Ok(match access.key {
            Some(key) => Claim::Take(key.to_owned()),
            None => Claim::Decline,
        })
    }

    fn assignment(&self, cx: &mut Cx<'_>, site: &AssignSite, _: Tag) -> PResult<Claim<LuauExpr>> {
        Ok(Claim::Take(cx.compile_expr(site.node)?))
    }

    fn expression(&self, cx: &mut Cx<'_>, node: P<Node>, _: Tag) -> PResult<Claim<LuauExpr>> {
        let value = match node.kind() {
            Kind::BinaryExpression => {
                let binary = node.as_binary_expression();
                let op = binary.operator_token.kind();
                let plain = !tsrs_ast::is_assignment_operator(op)
                    && !matches!(op, Kind::EqualsEqualsToken | Kind::ExclamationEqualsToken);
                if plain { cx.compile_binary(op, binary.left, binary.right())? } else { cx.compile_expr(node)? }
            }
            Kind::ObjectLiteralExpression => cx.compile_object_literal(node, ObjectEdits::default())?,
            _ => cx.compile_expr(node)?,
        };
        Ok(Claim::Take(value))
    }

    fn end_file(&self, _: &mut Cx<'_>) -> PResult<()> {
        Ok(())
    }
}
