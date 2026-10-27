//! What one file's transform records for its plugins (in `TransformState::plugin_file`). Empty, and never touched,
//! when the build has no plugins.

use std::cell::{OnceCell, RefCell};

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_luau::{AnyIdentifier, Expr, Stmt};
use rbxts_plugin::FileInfo;
use rbxts_plugin::erased::ErasedPart;
use tsrs_ast::{Node, SourceFile};
use tsrs_core::P;

/// A top-level statement of the file and where its output sits in the file's statement list.
#[derive(Clone, Copy, Debug)]
pub struct RootRange {
    pub node: P<Node>,
    /// The statement's own Luau (not its leading comments or root insertions).
    pub start: usize,
    pub end: usize,
    /// The diagnostics its transform reported, as indices into the file's diagnostics.
    pub diagnostics_start: usize,
    pub diagnostics_end: usize,
    /// The `TS` uses its transform added.
    pub runtime_lib_uses: u32,
}

/// Elision decisions for one re-transform of an import declaration (see `dispatch::end_file`).
#[derive(Debug, Default)]
pub struct ImportOverrides {
    /// Aliases elided: a plugin removed their value uses and nothing in the output refers to them.
    pub elided: FxHashSet<P<Node>>,
    /// Aliases kept though the checker saw no value use (`mark_import_used`, a reused import).
    pub forced: FxHashSet<P<Node>>,
}

/// One import made by `import_named`.
#[derive(Debug)]
pub struct PluginImport {
    pub specifier: String,
    pub export: String,
    /// The local the import declares, which the plugin was given.
    pub local: AnyIdentifier,
    /// The `TS.import` call it indexes, until the imports are placed.
    pub import: Option<Expr>,
    /// Where it was asked for: the source position of the node whose hook first asked for it (the file's start
    /// for `begin_file` / `end_file`), then the request order. Imports are placed as Flamework's TS transformer
    /// adds them in its walk: one declaration per path, in the order its first import was asked for, listing its
    /// imports in the order they were asked for.
    pub order: (u32, usize),
}

/// One `hoist_spillable` local: the local and the hint (or name) it was given.
#[derive(Clone, Debug)]
pub struct SpillableHoist {
    pub local: SpillableLocal,
    pub hint: String,
}

/// The local of a `hoist_spillable` hoist.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SpillableLocal {
    /// A temporary (`hoist_spillable`), by id.
    Temporary(u32),
    /// A plain name (`hoist_spillable_named`), capture-safe: nothing else in the file has it.
    Named(String),
}

/// The lengths of what a hook, or a hook nested in it, can leave behind besides its value: the prereq scope it
/// writes to, root insertions (hoists), header statements and plugin imports. A declined node must leave the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectMark {
    pub prereqs: usize,
    pub root_inserts: usize,
    pub header: usize,
    pub imports: usize,
    /// Late keys used and imports kept alive (`late_string`, `mark_import_used`).
    pub late_keys: usize,
    pub forced_imports: usize,
    /// Statements for before a TS statement (`insert_before_statement`), and runtime-library members used
    /// (`runtime_member`): counters that only grow.
    pub before_statement: usize,
    pub runtime_members: usize,
}

impl EffectMark {
    /// What grew since `earlier`, as the end of a sentence "declined … after …"; `None` when nothing did.
    pub fn growth_since(&self, earlier: &EffectMark) -> Option<&'static str> {
        if self.prereqs > earlier.prereqs {
            Some("emitting prerequisites (they would run twice)")
        } else if self.root_inserts > earlier.root_inserts {
            Some(
                "leaving statements before the root statement behind (a hoist or an insertion, its own or a nested \
                 hook's)",
            )
        } else if self.header > earlier.header {
            Some("leaving header statements behind (its own or a nested hook's)")
        } else if self.imports > earlier.imports {
            Some("leaving a plugin import behind (its own or a nested hook's)")
        } else if self.late_keys > earlier.late_keys {
            Some("using a late string (its own or a nested hook's)")
        } else if self.forced_imports > earlier.forced_imports {
            Some("keeping an import alive (mark_import_used or a reused import, its own or a nested hook's)")
        } else if self.before_statement > earlier.before_statement {
            Some(
                "leaving statements before its TS statement behind (insert_before_statement, its own or a nested \
                 hook's)",
            )
        } else if self.runtime_members > earlier.runtime_members {
            Some("using the runtime library (runtime_member, its own or a nested hook's)")
        } else {
            None
        }
    }
}

impl PluginFileState {
    /// The `insert_before_statement` statements for `statement`, in TypeScript's visit order, as Flamework's
    /// `prereq`s are made: by the source position of the outermost running hook's node inside the statement when
    /// each was inserted (roblox-ts lowers a `for` head's incrementor before its condition, and a class's methods
    /// before its static fields, where TypeScript visits in source order), and in insertion order under one such
    /// hook, whose plugin chose that order as its TS transform's code does.
    pub fn take_before_statement(&mut self, statement: P<Node>) -> Option<Vec<Stmt>> {
        let mut insertions = self.before_statement.remove(&statement)?;
        insertions.sort_by_key(|&(position, _)| position);
        Some(insertions.into_iter().flat_map(|(_, statements)| statements).collect())
    }

    /// The position `insert_before_statement` orders an insertion before `statement` by: the start of the outermost
    /// running hook's node inside `statement` (`anchor`, the inserting hook's, when none is further out).
    pub fn before_statement_position(&self, statement: P<Node>, anchor: P<Node>) -> u32 {
        let inside = |node: P<Node>| {
            let mut current = Some(node);
            while let Some(n) = current {
                if n == statement {
                    return true;
                }
                current = n.parent();
            }
            false
        };
        let node = self.frames.iter().filter_map(|frame| frame.node).find(|&node| inside(node)).unwrap_or(anchor);
        crate::util::traversal::get_start(node)
    }
}

/// One running hook on the frame stack ([`PluginFileState::frames`]). Pushed by every hook call and popped after
/// it, so a nested hook (reached through the outer hook's `compile_*` calls) never touches the outer hook's state.
#[derive(Clone, Copy, Debug, Default)]
pub struct HookFrame {
    /// The hook's node.
    pub node: Option<P<Node>>,
    /// What the hook did that its node can no longer be declined after: `compile_expr` of the node itself,
    /// `compile_call`, a hoist, a root insertion, a header, an import, a receiver compiled inside a capture.
    pub lowered: Option<&'static str>,
    /// `compile_call` ran.
    pub called: bool,
    /// Hooks (claim or class hooks) that ran while this one ran, through its `compile_*` calls: declining after
    /// them would run them again.
    pub nested_hooks: u32,
    /// Of those, the ones the lazy `compile_receiver` ran: the receiver is kept for reuse, so they do not run
    /// again.
    pub receiver_nested_hooks: u32,
    /// The hook called a primitive that changes compiler state (any lowering, placement, import, late string,
    /// temporary, name approval or diagnostic), not only a query. `RBXTS_PLUGIN_CLAIMS=all` relies on it.
    pub touched: bool,
    /// The hook compiled its own node: a second time is an internal error (it would be lowered twice).
    pub self_compiled: bool,
    /// The hook's own node was lowered as a statement (an assignment or update whose value is unused), and the hook
    /// got `nil` for its value.
    pub statement_form: bool,
}

/// The per-file plugin state.
#[derive(Default)]
pub struct PluginFileState {
    pub info: Option<FileInfo>,
    /// Per plugin: `begin_file` succeeded (a plugin whose `begin_file` failed gets no claims in the file).
    pub begun: Vec<bool>,
    /// Per plugin: the file part `end_file` returned.
    pub parts: Vec<Option<ErasedPart>>,
    /// The late keys the file's Luau uses: (plugin, namespaced key), in first-use order.
    pub late_keys: Vec<(usize, String)>,
    /// What the file's hooks read through `Cx::env` and `Cx::record_glob`, kept with its parts.
    pub inputs: crate::build::plugin_inputs::FileInputs,
    late_seen: FxHashSet<String>,
    /// `header` statements, in call order.
    pub header: Vec<Stmt>,
    /// `import_named` imports, in request order.
    pub imports: Vec<PluginImport>,
    pub(crate) import_names: FxHashSet<String>,
    /// Plain names `hoist_spillable_named` and the `import_named` fallback chose (`name_1`, …): taken for the file.
    pub(crate) plain_names: FxHashSet<String>,
    identifier_names: OnceCell<FxHashSet<&'static str>>,
    /// The file's identifier texts, sorted (`file_identifier_texts`).
    identifier_texts: OnceCell<std::rc::Rc<[&'static str]>>,
    /// The id of the first temporary made in the current top-level statement (`hoist` refuses a value using a later
    /// one, unless it is in `top_level_temps`).
    pub root_temp_start: u32,
    /// Temporaries the seam declared at the file's top level before the current statement: hoists, plugin imports,
    /// header and root-insertion locals.
    pub top_level_temps: FxHashSet<u32>,
    /// `insert_before_statement` statements, by the TS statement they go before, each insertion with the position
    /// it is ordered by ([`PluginFileState::take_before_statement`]); taken when that statement's list is lowered.
    pub before_statement: FxHashMap<P<Node>, Vec<(u32, Vec<Stmt>)>>,
    /// How many statements `insert_before_statement` was given in all (for the decline rule).
    pub before_statement_count: usize,
    /// `comment` texts by the TS statement around the hook's node: one leading the statement's Luau goes above its
    /// leading comments.
    pub statement_comments: FxHashMap<P<Node>, Vec<String>>,
    /// `runtime_member` uses.
    pub runtime_members: usize,
    /// What `classify_symbol` reported while this file was transformed (refused primitives' internal errors, at the
    /// symbol's declaration): the build reports each once, apart from the files (`backend::run_classify`).
    pub classify_diagnostics: Vec<P<tsrs_ast::Diagnostic>>,
    /// The diagnostics plugins reported for the file's list (each once): the build drops one identical to another
    /// diagnostic of the build (`compile::drop_plugin_duplicates`).
    pub plugin_diagnostics: Vec<P<tsrs_ast::Diagnostic>>,
    /// The internal errors of plugins' misuse of the seam in the file's list (`plugin_internal_error`): a plugin's
    /// errors, as stock reports a failing transformer's, for `compile::first_error_cut`.
    pub plugin_failures: Vec<P<tsrs_ast::Diagnostic>>,
    /// The number of Luau comments of the file's header comment (TypeScript's detached comments), which the first
    /// top-level statement emitted ahead of its root insertions; plugin imports go after them.
    pub detached_comments: usize,
    pub(crate) import_cache: FxHashMap<(String, String), Expr>,
    /// Statements for before the current top-level statement (`insert_before_root_statement`, `hoist`).
    pub root_inserts: Vec<Stmt>,
    /// The file's top-level statements, as transformed.
    pub root_ranges: Vec<RootRange>,
    /// `request_import_elision` aliases, in request order.
    pub elision_requests: Vec<P<Node>>,
    /// `mark_import_used` aliases, and user imports `import_named` reused.
    pub forced_imports: FxHashSet<P<Node>>,
    /// Import aliases the compiler itself compiled a reference to.
    pub compiled_aliases: FxHashSet<P<Node>>,
    /// Set while an import declaration is transformed again with elision decisions.
    pub import_overrides: Option<ImportOverrides>,
    /// Calls already offered to the plugins (a chain call asked before its callee was compiled).
    pub offered: FxHashSet<P<Node>>,
    /// Nodes being offered right now, one set per offer kind: a dispatch of one of them while its offer runs (a
    /// hook compiling its own node, or an ancestor of it) is not offered to that kind again. The kinds are
    /// separate, so an expression hook compiling its own call still lets the call plugins claim that call.
    /// Calls, `new` and decorators:
    pub in_offer: FxHashSet<P<Node>>,
    /// The expression and statement fallbacks:
    pub in_fallback_offer: FxHashSet<P<Node>>,
    /// The class hook:
    pub in_class_offer: FxHashSet<P<Node>>,
    /// The assignment hook:
    pub in_assign_offer: FxHashSet<P<Node>>,
    /// Expressions every expression hook declined: the hooks are asked about a node once, so a later lowering of
    /// it (a call hook compiling its own call, or roblox-ts lowering a node twice) is roblox-ts's, as their answer
    /// said.
    pub fallback_declined: FxHashSet<P<Node>>,
    /// Assignments every assignment hook declined (asked once, as above).
    pub assign_declined: FxHashSet<P<Node>>,
    /// Calls, `new` and decorators every call plugin declined (asked once, as above).
    pub call_declined: FxHashSet<P<Node>>,
    /// Chain links being offered to the expression hooks after their base was compiled (inside an optional
    /// chain): a hook compiling its own node lowers it on that base. Innermost last.
    pub(crate) eager_offers: Vec<(P<Node>, super::backend::EagerLowering)>,
    /// Receivers of non-method optional calls (`o.f?.()`) that `compile_receiver` bound to a local, by call node:
    /// the chain lowering declares the local before it reads the callee from it.
    pub optional_receivers: FxHashMap<P<Node>, (Expr, Stmt)>,
    /// The `property_key` answers, by access node: one answer per access, however often it is lowered. `None` when
    /// every plugin declined (or a hook failed).
    pub key_answers: FxHashMap<P<Node>, Option<String>>,
    /// Locals each function body declared at its top level so far, by body node (`locals().function`).
    pub function_locals: FxHashMap<P<Node>, u32>,
    /// `hoist_spillable` locals, in hoist order: spilled into one table when the main chunk passes Luau's limit.
    pub spillable: Vec<SpillableHoist>,
    /// A plugin hook emitted something into the file (a claimed value, a class hook's statements, or any primitive
    /// that changes compiler state): the file is checked against Luau's limits (`spill`).
    pub emitted: bool,
    /// Statements inserted into the file's statement list after its top-level statements were lowered, as
    /// (index, count) in the order they were inserted: plugin imports, the header, `local exports`, the
    /// `Compiled with` comment and `local TS`. They move `root_ranges` to the final list.
    pub front_inserts: Vec<(usize, usize)>,
    /// A plugin emitted into the file: Luau's compiler checks it once rendered (`spill::decide_all`).
    pub(crate) load_check: Option<super::spill::LoadCheck>,
    /// `local` names declared at the top level of the main chunk so far (statements done, root insertions,
    /// imports, header).
    pub main_chunk_locals: u32,
    /// The hooks running now, innermost last.
    pub frames: Vec<HookFrame>,
    /// Names `is_usable_local_name` approved: the plugin may declare them anywhere, so no seam temporary or
    /// plugin import takes them.
    approved_names: RefCell<FxHashSet<String>>,
    /// The hints seam temporaries were given: their names `_hint` / `_hint_<n>` are taken for later approvals.
    seam_bases: FxHashSet<String>,
}

impl PluginFileState {
    /// The state of a file with `plugins` plugins.
    pub fn new(info: FileInfo, plugins: usize) -> PluginFileState {
        PluginFileState {
            info: Some(info),
            begun: vec![true; plugins],
            parts: (0..plugins).map(|_| None).collect(),
            ..PluginFileState::default()
        }
    }

    /// Every identifier text in the file (made on first use), as TypeScript 5's printer counts them
    /// (`rbxts_plugin::ts5_file_identifiers`: JSDoc identifiers and the literals its parser interns included): a name
    /// none of them has cannot shadow, or be shadowed by, anything the file declares or refers to, and is the name
    /// TypeScript's printer would give a TS transformer's unique name.
    pub fn identifier_names(&self, file: P<SourceFile>) -> &FxHashSet<&'static str> {
        self.identifier_names.get_or_init(|| self.identifier_texts(file).iter().copied().collect())
    }

    /// [`PluginFileState::identifier_names`], sorted (`file_identifier_texts`).
    pub fn identifier_texts(&self, file: P<SourceFile>) -> std::rc::Rc<[&'static str]> {
        self.identifier_texts.get_or_init(|| rbxts_plugin::ts5_file_identifiers(file).into()).clone()
    }

    /// TypeScript's optimistic unique name (`makeUniqueName` with `Optimistic`) for a plain local: `base` when it is
    /// not taken ([`PluginFileState::name_taken`]) and not a `luau.globals` name, else the first free `base_1`,
    /// `base_2`, … (`base1`, … when `base` ends in `_`). The name is recorded as taken. `base` is a valid identifier.
    pub fn unique_plain_name(&mut self, file: P<SourceFile>, base: &str) -> String {
        let free =
            |state: &Self, name: &str| !rbxts_luau::is_reserved_identifier(name) && !state.name_taken(file, name);
        let name = if free(self, base) {
            base.to_owned()
        } else {
            let stem = if base.ends_with('_') { base.to_owned() } else { format!("{base}_") };
            (1u32..).map(|i| format!("{stem}{i}")).find(|name| free(self, name)).expect("a free name")
        };
        self.plain_names.insert(name.clone());
        name
    }

    /// The effect lengths now, with the prereq scope at `prereqs` statements.
    pub fn effect_mark(&self, prereqs: usize) -> EffectMark {
        EffectMark {
            prereqs,
            root_inserts: self.root_inserts.len(),
            header: self.header.len(),
            imports: self.imports.len(),
            late_keys: self.late_keys.len(),
            forced_imports: self.forced_imports.len(),
            before_statement: self.before_statement_count,
            runtime_members: self.runtime_members,
        }
    }

    /// The hint for a temporary identifier a plugin asks for (`temp_id`, `hoist`, `push_to_var`, the
    /// `import_named` fallback). The renderer names a temporary `_hint`, `_hint_1`, … against its declaring
    /// scope and the scopes around it only, which is enough for roblox-ts's own temporaries but not for these: a
    /// hoist or an import is declared at the top of the file and may be used in any nested function, where a user
    /// local of that name would capture it. So the hint is changed (`hint2`, `hint3`, …) until no identifier of the
    /// file is `_hint` or `_hint_<n>`.
    ///
    /// Names `is_usable_local_name` approved and plugin import names count too, so a name the plugin may declare
    /// in a nested scope never captures a seam temporary; the hint chosen is recorded, so a later approval cannot
    /// take its names either.
    pub fn seam_hint(&mut self, file: P<SourceFile>, hint: &str) -> String {
        let hint = if hint.is_empty() { "seam" } else { hint };
        let base = {
            let names = self.identifier_names(file);
            let approved = self.approved_names.borrow();
            let taken = |base: &str| {
                let exact = format!("_{base}");
                let numbered = format!("_{base}_");
                let matches = |name: &str| {
                    name == exact
                        || name
                            .strip_prefix(numbered.as_str())
                            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
                };
                names.iter().any(|n| matches(n))
                    || approved.iter().any(|n| matches(n))
                    || self.import_names.iter().any(|n| matches(n))
                    || self.plain_names.iter().any(|n| matches(n))
            };
            if taken(hint) {
                (2..).map(|i| format!("{hint}{i}")).find(|candidate| !taken(candidate)).expect("a free name")
            } else {
                hint.to_owned()
            }
        };
        self.seam_bases.insert(base.clone());
        base
    }

    /// `name` is a seam temporary's name: `_base` or `_base_<n>` for a recorded hint.
    fn is_seam_name(&self, name: &str) -> bool {
        let Some(rest) = name.strip_prefix('_') else { return false };
        self.seam_bases.iter().any(|base| {
            rest.strip_prefix(base.as_str()).is_some_and(|tail| {
                tail.is_empty()
                    || tail
                        .strip_prefix('_')
                        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            })
        })
    }

    /// The number of names approved so far (an approval changes later naming, so it counts as touching state).
    pub fn approved_count(&self) -> usize {
        self.approved_names.borrow().len()
    }

    /// `name` is used by the file's source, a plugin import, an approved name or a seam temporary.
    pub fn name_taken(&self, file: P<SourceFile>, name: &str) -> bool {
        self.identifier_names(file).contains(name)
            || self.import_names.contains(name)
            || self.plain_names.contains(name)
            || self.approved_names.borrow().contains(name)
            || self.is_seam_name(name)
    }

    /// `is_usable_local_name`: a valid identifier, not a `luau.globals` name, and not taken
    /// ([`PluginFileState::name_taken`]). An approved name is recorded, so later seam temporaries and plugin
    /// imports avoid it.
    pub fn approve_name(&self, file: P<SourceFile>, name: &str) -> bool {
        let usable = rbxts_luau::is_valid_identifier(name)
            && !rbxts_luau::is_reserved_identifier(name)
            && !self.name_taken(file, name);
        if usable {
            self.approved_names.borrow_mut().insert(name.to_owned());
        }
        usable
    }

    /// Records a late key the file's Luau uses.
    pub fn use_late_key(&mut self, plugin: usize, key: String) {
        if self.late_seen.insert(key.clone()) {
            self.late_keys.push((plugin, key));
        }
    }
}

/// The number of `local` names a statement declares in the scope it is in.
pub fn declared_locals(statement: &Stmt) -> u32 {
    match statement {
        Stmt::VariableDeclaration(declaration) => declaration.left.len() as u32,
        Stmt::FunctionDeclaration(declaration) => u32::from(declaration.localize),
        _ => 0,
    }
}
