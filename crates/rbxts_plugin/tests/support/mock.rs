//! A mock compiler: the three backends, implemented just enough to run a plugin's logic without rbxts_transform,
//! plus a small driver ([`MockCompiler`]) that walks a file's calls the way the compiler's dispatch would and keeps
//! state across builds the way DESIGN §14 says.
//!
//! What is real: the program (parsed and bound by tsrs from `tests/fixture`), nodes, binder symbols, `getStart()`
//! positions, the Luau renderer. What is mocked: callee resolution (by name among the fixture's declarations, no
//! checker), JSDoc (read from the comment text before a declaration), `compile_*` (literals and identifiers only),
//! and the commit (queued writes are only recorded).
//!
//! The contracts the real compiler must keep, and the mock does:
//! - phases: [`MockCx`] refuses what [`CxBackend`]'s phase table refuses, with an internal error;
//! - a hook's `Err` without a new error diagnostic is an internal error naming the plugin;
//! - a `Shared` is kept only with the committed state it was made from;
//! - every late key a file uses must be resolved in `finish`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use rbxts_luau::{LateStrings, TempIds, create};
use rbxts_plugin::erased::{DynInstance, DynPlugin, ErasedPart, ErasedShared, ErasedState};
use rbxts_plugin::ts::{
    Category, Checker, CompilerOptions, Diagnostic, Kind, Node, P, Program, SourceFile, Symbol,
};
use rbxts_plugin::{
    Aborted, Arg, BinOp, BuildKind, CallKind, CallSite, Callee, ChangeSet, CheckerInfo, Claim, Cx,
    CxBackend, FileIndex, FileInfo, FinishBackend, FinishCx, Interest, JsDocTag, LocalCounts,
    LuauExpr, LuauIdent, LuauStmt, ObjectEdits, PluginDiagnostic, RebuildPlan, SetupBackend,
    SetupCx, StateOrigin, SymbolRole, Tag, Utf16Pos,
};
use rbxts_project::{
    FileRojoInfo, NetworkType, PackageInfo, ProjectServices, RbxType, ResolvedModuleInfo,
};
use serde_json::{Map, Value};

/// The fixture program, loaded once per test binary.
pub fn fixture() -> &'static Program {
    static PROGRAM: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let addr = *PROGRAM.get_or_init(|| {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixture");
        let project = rbxts_project::load(&rbxts_project::LoadOptions {
            tsconfig: format!("{dir}/tsconfig.json"),
            cwd: dir.to_owned(),
            checkers: 1,
            lazy_dts: false,
        regions: false,
        })
        .unwrap_or_else(|_| panic!("the fixture tsconfig loads"));
        project.program as *const Program as usize
    });
    // SAFETY: `load` returns a `&'static Program`; the address is only stored to keep the static `Sync`.
    unsafe { &*(addr as *const Program) }
}

/// A fixture source file by base name.
pub fn source(program: &'static Program, base: &str) -> (FileIndex, P<SourceFile>) {
    let (i, &file) = program
        .source_files()
        .iter()
        .enumerate()
        .find(|(_, f)| f.get().file_name().ends_with(&format!("/{base}")))
        .unwrap_or_else(|| panic!("fixture file {base}"));
    (FileIndex(i as u32), file)
}

/// The function declarations of `stamp.d.ts` by name: the mock's callee resolution.
pub fn declared_functions(program: &'static Program) -> HashMap<String, P<Symbol>> {
    let (_, file) = source(program, "stamp.d.ts");
    let mut out = HashMap::new();
    for &statement in file.as_node().statements() {
        if statement.kind() == Kind::FunctionDeclaration
            && let Some(symbol) = statement.symbol()
        {
            out.insert(symbol.name().to_owned(), symbol);
        }
    }
    out
}

/// Every call and `new` expression of a file, in source order (pre-order walk).
pub fn calls_of(file: P<SourceFile>) -> Vec<P<Node>> {
    fn walk(node: P<Node>, out: &mut Vec<P<Node>>) {
        if matches!(node.kind(), Kind::CallExpression | Kind::NewExpression) {
            out.push(node);
        }
        node.for_each_child(&mut |child| {
            walk(child, out);
            false
        });
    }
    let mut out = Vec::new();
    for &statement in file.as_node().statements() {
        walk(statement, &mut out);
    }
    out
}

fn start_of(node: P<Node>, file: P<SourceFile>) -> u32 {
    tsrs_scanner::get_token_pos_of_node(node, file, false)
}

fn end_of(node: P<Node>) -> u32 {
    node.end().source_offset().expect("a parsed node")
}

/// Line and character (UTF-16) of a byte offset, with TypeScript's `computeLineStarts` line breaks: CR, LF, CRLF
/// (one break), U+2028 and U+2029.
pub fn line_and_character(text: &str, offset: usize) -> (u32, u32) {
    let mut line = 0;
    let mut line_start = 0;
    let mut chars = text[..offset].char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '\r' if chars.peek().map(|&(_, n)| n) == Some('\n') => {}
            '\r' | '\n' | '\u{2028}' | '\u{2029}' => {
                line += 1;
                line_start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    (line, text[line_start..offset].encode_utf16().count() as u32)
}

// --- Cx ---

/// Which hook a [`MockCx`] serves, for the phase table on [`CxBackend`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Claim hooks and `class`: everything allowed.
    Claim,
    /// `begin_file` / `end_file`: no lowering primitives.
    File,
    /// `classify_symbol`: facts only, no diagnostics.
    Classify,
    /// `explain_checker_diagnostic`: facts and reports.
    Explain,
}

pub struct MockCx {
    pub program: &'static Program,
    pub plugin: &'static str,
    pub file: FileInfo,
    pub phase: Phase,
    pub diagnostics: Vec<PluginDiagnostic>,
    /// Late-string keys handed out, in order (one plugin, so no namespacing).
    pub late_keys: Vec<String>,
    /// The prereq scopes; the first is the expression's own.
    pub scopes: Vec<Vec<LuauStmt>>,
    pub hoisted: Vec<LuauStmt>,
    pub elision_requests: Vec<P<Node>>,
    temp_ids: TempIds,
}

impl MockCx {
    pub fn new(program: &'static Program, index: FileIndex, file: P<SourceFile>) -> MockCx {
        let path = file.get().file_name().to_owned();
        let relative_path = path
            .split_once("/fixture/")
            .map_or(path.as_str(), |(_, rest)| rest)
            .to_owned();
        let rojo = FileRojoInfo {
            out_path: path.replace("/src/", "/out/").replace(".ts", ".luau"),
            rbx_path: None,
            rbx_type: RbxType::ModuleScript,
            network_type: NetworkType::Unknown,
            is_isolated: false,
            in_replicated_first: false,
        };
        MockCx {
            program,
            plugin: "plugin",
            file: FileInfo {
                source: file,
                index,
                path,
                relative_path,
                rojo,
            },
            phase: Phase::Claim,
            diagnostics: Vec::new(),
            late_keys: Vec::new(),
            scopes: vec![Vec::new()],
            hoisted: Vec::new(),
            elision_requests: Vec::new(),
            temp_ids: TempIds::new(),
        }
    }

    fn internal(&mut self, node: P<Node>, what: &str) -> Aborted {
        self.diagnostics
            .push(PluginDiagnostic::error(node, format!("internal: {what}")));
        Aborted
    }

    fn root(&self) -> P<Node> {
        self.file.source.as_node()
    }

    /// The phase table: `lowering` primitives only in `Claim`, file-level ones not in `Classify` / `Explain`.
    fn refused(&mut self, what: &str, lowering: bool) -> bool {
        let allowed = match self.phase {
            Phase::Claim => true,
            Phase::File => !lowering,
            Phase::Classify | Phase::Explain => false,
        };
        if !allowed {
            let message = format!("plugin `{}` called {what} in {:?}", self.plugin, self.phase);
            let root = self.root();
            self.internal(root, &message);
        }
        !allowed
    }

    /// Error messages recorded, in order.
    pub fn errors(&self) -> Vec<&str> {
        self.messages(Category::Error)
    }

    /// Warning messages recorded, in order.
    pub fn warnings(&self) -> Vec<&str> {
        self.messages(Category::Warning)
    }

    fn messages(&self, category: Category) -> Vec<&str> {
        self.diagnostics
            .iter()
            .filter(|d| d.category == category)
            .map(|d| d.message.as_str())
            .collect()
    }
}

impl CxBackend for MockCx {
    fn program(&self) -> &'static Program {
        self.program
    }

    fn options(&self) -> P<CompilerOptions> {
        self.program.options()
    }

    fn file(&self) -> &FileInfo {
        &self.file
    }

    fn checker(&mut self) -> &mut Checker {
        panic!("the mock has no checker")
    }

    fn project(&self) -> &ProjectServices {
        panic!("the mock has no project services")
    }

    fn resolve_module(&self, _: &str, _: P<SourceFile>) -> Option<ResolvedModuleInfo> {
        None
    }

    fn declaring_package(&self, _: P<Symbol>) -> Option<Arc<PackageInfo>> {
        None
    }

    fn compile_expr(&mut self, node: P<Node>) -> Result<LuauExpr, Aborted> {
        if self.refused("compile_expr", true) {
            return Err(Aborted);
        }
        match node.kind() {
            Kind::NumericLiteral => Ok(create::number_text(node.text())),
            Kind::StringLiteral => Ok(rbxts_plugin::text_literal(node.text())),
            Kind::Identifier => Ok(create::id(node.text())),
            _ => Err(self.internal(node, "the mock compiles literals and identifiers only")),
        }
    }

    fn compile_in_order(&mut self, nodes: &[P<Node>]) -> Result<Vec<LuauExpr>, Aborted> {
        nodes.iter().map(|&n| self.compile_expr(n)).collect()
    }

    fn compile_receiver(&mut self, call: &CallSite) -> Result<LuauExpr, Aborted> {
        Err(self.internal(call.node, "compile_receiver is not mocked"))
    }

    fn compile_call(
        &mut self,
        call: &CallSite,
        _: Callee,
        _: Vec<Arg>,
    ) -> Result<LuauExpr, Aborted> {
        Err(self.internal(call.node, "compile_call is not mocked"))
    }

    fn compile_object_literal(
        &mut self,
        node: P<Node>,
        _: ObjectEdits,
    ) -> Result<LuauExpr, Aborted> {
        Err(self.internal(node, "compile_object_literal is not mocked"))
    }

    fn compile_binary(&mut self, _: BinOp, left: P<Node>, _: P<Node>) -> Result<LuauExpr, Aborted> {
        Err(self.internal(left, "compile_binary is not mocked"))
    }

    fn prereq(&mut self, stmt: LuauStmt) {
        if !self.refused("prereq", true) {
            self.scopes.last_mut().expect("a scope").push(stmt);
        }
    }

    fn push_to_var(&mut self, value: LuauExpr, hint: &str) -> LuauExpr {
        if self.refused("push_to_var", true) {
            return create::nil();
        }
        let id = self.temp_ids.temp_id(hint);
        self.prereq(create::local(id.clone(), Some(value)));
        id.into()
    }

    fn begin_capture(&mut self) {
        // refused captures still push a scope, so begin and end stay paired
        self.refused("capture", true);
        self.scopes.push(Vec::new());
    }

    fn end_capture(&mut self) -> Vec<LuauStmt> {
        assert!(self.scopes.len() > 1, "end_capture without begin_capture");
        self.scopes.pop().expect("a scope")
    }

    fn insert_before_root_statement(&mut self, stmts: Vec<LuauStmt>) {
        if !self.refused("insert_before_root_statement", true) {
            self.hoisted.extend(stmts);
        }
    }

    fn header(&mut self, stmts: Vec<LuauStmt>) {
        if !self.refused("header", false) {
            self.hoisted.extend(stmts);
        }
    }

    fn hoist(&mut self, value: LuauExpr, hint: &str) -> LuauExpr {
        if self.refused("hoist", true) {
            return create::nil();
        }
        let id = self.temp_ids.temp_id(hint);
        self.hoisted.push(create::local(id.clone(), Some(value)));
        id.into()
    }

    fn hoist_spillable(&mut self, value: LuauExpr, hint: &str) -> LuauExpr {
        self.hoist(value, hint)
    }

    fn import_named(&mut self, specifier: &str, export: &str) -> Result<LuauExpr, Aborted> {
        if self.refused("import_named", false) {
            return Err(Aborted);
        }
        Ok(create::property(
            create::id(specifier.replace(['/', '@', '-'], "_")),
            export,
        ))
    }

    fn mark_import_used(&mut self, _: P<Node>) {
        self.refused("mark_import_used", false);
    }

    fn request_import_elision(&mut self, alias_declaration: P<Node>) {
        if !self.refused("request_import_elision", false) {
            self.elision_requests.push(alias_declaration);
        }
    }

    fn temp_id(&mut self, hint: &str) -> LuauIdent {
        self.refused("temp_id", false);
        self.temp_ids.temp_id(hint)
    }

    fn is_usable_local_name(&self, name: &str) -> bool {
        rbxts_luau::is_valid_identifier(name)
    }

    fn locals(&self) -> LocalCounts {
        LocalCounts::default()
    }

    fn late_string(&mut self, key: &str) -> LuauExpr {
        if self.refused("late_string", false) {
            return create::nil();
        }
        self.late_keys.push(key.to_owned());
        create::late_string(key)
    }

    fn comment(&mut self, text: &str) {
        self.prereq(create::comment(text));
    }

    fn utf16_pos(&self, node: P<Node>) -> Utf16Pos {
        let text = self.file.source.get().text();
        let start = start_of(node, self.file.source) as usize;
        let (line, character) = line_and_character(text, start);
        Utf16Pos {
            offset: text[..start].encode_utf16().count() as u32,
            line,
            character,
        }
    }

    fn utf16_width(&self, node: P<Node>) -> u32 {
        self.source_text(node).encode_utf16().count() as u32
    }

    fn source_text(&self, node: P<Node>) -> &'static str {
        let file = tsrs_ast::get_source_file_of_node(node).unwrap_or(self.file.source);
        &file.get().text()[start_of(node, file) as usize..end_of(node) as usize]
    }

    /// The `@name comment` lines of the JSDoc comment right before the declaration.
    fn jsdoc_tags(&mut self, declaration: P<Node>) -> Rc<[JsDocTag]> {
        let file = tsrs_ast::get_source_file_of_node(declaration).expect("a declaration in a file");
        let text = file.get().text();
        let start = declaration.pos().source_offset().unwrap() as usize;
        let trivia = &text[start..start_of(declaration, file) as usize];
        let Some(open) = trivia.rfind("/**") else {
            return Rc::from([]);
        };
        let body = trivia[open + 3..].split("*/").next().unwrap_or("");
        let tags: Vec<JsDocTag> = body
            .lines()
            .map(|line| line.trim().trim_start_matches('*').trim())
            .filter_map(|line| line.strip_prefix('@'))
            .map(|tag| {
                let (name, comment) = tag.split_once(' ').unwrap_or((tag, ""));
                JsDocTag {
                    node: declaration,
                    name: Box::leak(name.to_owned().into_boxed_str()),
                    comment: comment.trim().to_owned(),
                    links: Vec::new(),
                }
            })
            .collect();
        Rc::from(tags)
    }

    fn file_diagnostics(&self) -> &[P<Diagnostic>] {
        &[]
    }

    fn report(&mut self, diagnostic: PluginDiagnostic) {
        if self.phase == Phase::Classify {
            let message = format!(
                "plugin `{}` reported a diagnostic from classify_symbol: {}",
                self.plugin, diagnostic.message
            );
            self.internal(diagnostic.node, &message);
            return;
        }
        self.diagnostics.push(diagnostic);
    }
}

// --- SetupCx ---

pub struct MockSetup {
    pub program: &'static Program,
    pub entry: Option<Map<String, Value>>,
    pub env: HashMap<String, String>,
    pub kind: BuildKind,
    pub origin: StateOrigin,
    pub changes: Option<ChangeSet>,
    /// A build input the plugin read changed since the last build: setup must run again.
    pub input_changed: bool,
    pub transforms_all: bool,
    /// Files the plugin persisted (by path).
    pub persisted: HashMap<PathBuf, Vec<u8>>,
    /// Inputs recorded: env names and input paths.
    pub inputs: Vec<String>,
    pub messages: Vec<String>,
}

impl MockSetup {
    pub fn clean(program: &'static Program) -> MockSetup {
        MockSetup {
            program,
            entry: None,
            env: HashMap::new(),
            kind: BuildKind::Clean,
            origin: StateOrigin::Fresh,
            changes: None,
            input_changed: false,
            transforms_all: true,
            persisted: HashMap::new(),
            inputs: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// The next build is watch rebuild `generation`.
    pub fn watch(&mut self, generation: u64) {
        self.kind = BuildKind::WatchRebuild { generation };
        self.changes = Some(ChangeSet::default());
    }
}

impl SetupBackend for MockSetup {
    fn program(&self) -> &'static Program {
        self.program
    }

    fn options(&self) -> P<CompilerOptions> {
        self.program.options()
    }

    fn project(&self) -> &ProjectServices {
        panic!("the mock has no project services")
    }

    fn tsconfig_entry(&self) -> Option<&Map<String, Value>> {
        self.entry.as_ref()
    }

    fn resolve_module(&self, specifier: &str, _: &str) -> Option<ResolvedModuleInfo> {
        (specifier == "@rbxts/t").then(|| ResolvedModuleInfo {
            file_name: "/node_modules/@rbxts/t/lib/ts.d.ts".into(),
            is_external_library_import: true,
            package_name: Some("@rbxts/t".into()),
        })
    }

    fn read_input(&mut self, path: &Path) -> std::io::Result<Option<String>> {
        self.inputs.push(format!("file:{}", path.display()));
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn env(&mut self, name: &str) -> Option<String> {
        self.inputs.push(format!("env:{name}"));
        self.env.get(name).cloned()
    }

    fn build_kind(&self) -> BuildKind {
        self.kind
    }

    fn transforms_all_files(&self) -> bool {
        self.transforms_all
    }

    fn changes(&self) -> Option<&ChangeSet> {
        self.changes.as_ref()
    }

    fn state_origin(&self) -> StateOrigin {
        self.origin
    }

    fn read_persisted(&mut self, path: &Path) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.persisted.get(path).cloned())
    }

    fn info(&mut self, message: &str) {
        self.messages.push(format!("info: {message}"));
    }

    fn warn(&mut self, message: &str, _: Option<&str>) {
        self.messages.push(format!("warn: {message}"));
    }
}

// --- FinishCx ---

#[derive(Default)]
pub struct MockFinish {
    pub transforms_all: bool,
    pub late: LateStrings,
    pub resolved: Vec<String>,
    pub artifacts: Vec<(PathBuf, Vec<u8>)>,
    pub state_files: Vec<(PathBuf, Vec<u8>)>,
    pub removed: Vec<PathBuf>,
    pub watched: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

impl MockFinish {
    /// The compiler's key check: every key a current file uses has a value.
    pub fn check_keys<'k>(
        &self,
        plugin: &str,
        used: impl IntoIterator<Item = &'k String>,
    ) -> Result<(), String> {
        for key in used {
            if self.late.get(key).is_none() {
                return Err(format!(
                    "plugin `{plugin}` left the late string `{key}` unresolved"
                ));
            }
        }
        Ok(())
    }
}

impl FinishBackend for MockFinish {
    fn project(&self) -> &ProjectServices {
        panic!("the mock has no project services")
    }

    fn build_kind(&self) -> BuildKind {
        BuildKind::Clean
    }

    fn transforms_all_files(&self) -> bool {
        self.transforms_all
    }

    fn resolve_late(&mut self, key: &str, escaped: String) {
        self.resolved.push(key.to_owned());
        self.late.insert(key, escaped);
    }

    fn write_artifact(&mut self, path: &Path, bytes: Vec<u8>) {
        self.artifacts.push((path.to_owned(), bytes));
    }

    fn write_state_file(&mut self, path: &Path, bytes: Vec<u8>) {
        self.state_files.push((path.to_owned(), bytes));
    }

    fn remove_artifact(&mut self, path: &Path) {
        self.removed.push(path.to_owned());
    }

    fn watch_dir(&mut self, path: &Path) {
        self.watched.push(path.to_owned());
    }

    fn warn_at(&mut self, file: FileIndex, pos: Utf16Pos, message: &str, _: Option<&str>) {
        self.warnings.push(format!(
            "{}:{}:{}: {message}",
            file.0, pos.line, pos.character
        ));
    }

    fn info(&mut self, _: &str) {}
}

// --- a miniature compiler driver over the erased API ---

/// What happened at one call site.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Claimed; the rendered Luau value (late strings filled in after finish).
    Luau(String),
    /// Declined, or no plugin interest: roblox-ts would lower it.
    Lowered,
    /// The hook returned `Err(Abort)`: the compiler emits `nil`.
    Aborted,
}

pub struct BuildOutput {
    /// (file base name, call source text, outcome), in program order.
    pub sites: Vec<(String, String, Outcome)>,
    /// `"error file: message"` / `"warning file: message"`, file by file in program order.
    pub diagnostics: Vec<String>,
    pub finish: MockFinish,
    /// The build's working state: committed only when the build committed.
    pub state: ErasedState,
    pub shared: ErasedShared,
    pub setup_ran: bool,
    pub committed: bool,
    pub setup_messages: Vec<String>,
}

/// The compiler across builds: the committed state and the kept `Shared` (DESIGN §14, `Plugin` docs).
#[derive(Default)]
pub struct MockCompiler {
    pub committed: Option<ErasedState>,
    pub kept: Option<ErasedShared>,
    pub rebuild_start_calls: u32,
}

impl MockCompiler {
    pub fn new() -> MockCompiler {
        MockCompiler::default()
    }

    /// One build of the fixture files named in `files`, in program order, spread over `checkers` instances
    /// (file `i` of the build on instance `i % checkers`). Setup unless a kept `Shared` survives
    /// `rebuild_start`; finish, key check and render when no hook reported an error; commit on success. `Err`
    /// carries a fatal error's message or the key check's failure.
    pub fn build(
        &mut self,
        plugin: &dyn DynPlugin,
        setup: &mut MockSetup,
        files: &[&str],
        checkers: usize,
    ) -> Result<BuildOutput, String> {
        let result = self.build_inner(plugin, setup, files, checkers);
        match &result {
            // commit: the working state becomes the committed one, and the Shared is kept with it
            Ok(out) if out.committed => {
                self.committed = Some(out.state.clone());
                self.kept = Some(out.shared.clone());
            }
            // no commit: the committed state stays. A Shared this build made was never kept (setup clears the
            // kept one first); a Shared kept from the last committed build stays.
            Ok(_) | Err(_) => {}
        }
        result
    }

    fn build_inner(
        &mut self,
        plugin: &dyn DynPlugin,
        setup: &mut MockSetup,
        files: &[&str],
        checkers: usize,
    ) -> Result<BuildOutput, String> {
        let program = setup.program;
        let name = plugin.info().name;
        let functions = declared_functions(program);
        // the working copy: the committed state stays untouched if this build fails
        let mut state = self
            .committed
            .clone()
            .unwrap_or_else(|| plugin.default_state());

        // rebuild_start only on a watch rebuild with a kept Shared and no changed input
        let mut shared = None;
        if let (BuildKind::WatchRebuild { .. }, Some(kept), false) =
            (setup.kind, &self.kept, setup.input_changed)
        {
            self.rebuild_start_calls += 1;
            let changes = setup.changes.clone().unwrap_or_default();
            if plugin.rebuild_start(kept, &state, &changes) == RebuildPlan::KeepShared {
                shared = Some(kept.clone());
            }
        }
        let setup_ran = shared.is_none();
        let shared = match shared {
            Some(shared) => shared,
            None => {
                self.kept = None;
                setup.transforms_all = true;
                plugin
                    .setup(&mut SetupCx::new(setup), &mut state)
                    .map_err(|f| f.to_string())?
            }
        };

        let instances: Vec<(Box<dyn DynInstance>, Interest)> = (0..checkers)
            .map(|index| {
                let instance = plugin.instance(
                    &shared,
                    CheckerInfo {
                        index,
                        count: checkers,
                    },
                );
                let mut interest = Interest::new();
                instance.interest(&mut interest);
                (instance, interest)
            })
            .collect();
        let mut memos: Vec<ClassifyMemo> = (0..checkers).map(|_| HashMap::new()).collect();

        let mut parts: Vec<(FileIndex, ErasedPart)> = Vec::new();
        let mut pending: Vec<(String, String, Result<Option<LuauExpr>, ()>)> = Vec::new();
        let mut diagnostics = Vec::new();
        let mut used_keys = Vec::new();
        let mut nth = 0;
        for (i, &file) in program.source_files().iter().enumerate() {
            let base = file
                .get()
                .file_name()
                .rsplit('/')
                .next()
                .unwrap()
                .to_owned();
            if file.is_declaration_file() || !files.contains(&base.as_str()) {
                continue;
            }
            let (instance, interest) = &instances[nth % checkers];
            let memo = &mut memos[nth % checkers];
            nth += 1;
            let index = FileIndex(i as u32);
            let mut backend = MockCx::new(program, index, file);
            backend.plugin = name;

            backend.phase = Phase::File;
            let begun = guarded(&mut backend, |cx| instance.begin_file(cx));
            for call in calls_of(file) {
                let callee = call.expression().expect("a callee");
                let text = backend.source_text(call).to_owned();
                let symbol = (callee.kind() == Kind::Identifier)
                    .then(|| functions.get(callee.text()).copied())
                    .flatten();
                let tag = match symbol {
                    Some(symbol) if interest.wants_calls_by_symbol() => {
                        *memo.entry((symbol, SymbolRole::Callee)).or_insert_with(|| {
                            backend.phase = Phase::Classify;
                            let mut cx = Cx::new(&mut backend);
                            instance.classify_symbol(&mut cx, symbol, SymbolRole::Callee)
                        })
                    }
                    _ => None,
                };
                let (Some(tag), Ok(())) = (tag, &begun) else {
                    pending.push((base.clone(), text, Ok(None)));
                    continue;
                };
                let site = CallSite {
                    node: call,
                    kind: if call.kind() == Kind::NewExpression {
                        CallKind::New
                    } else {
                        CallKind::Call
                    },
                    callee,
                    args: call.arguments(),
                    symbol,
                    receiver: None,
                    in_optional_chain: false,
                    value_used: true,
                };
                backend.phase = Phase::Claim;
                let outcome = match guarded(&mut backend, |cx| instance.call(cx, &site, tag)) {
                    Ok(Claim::Take(expr)) => Ok(Some(expr)),
                    Ok(Claim::Decline) => Ok(None),
                    Err(()) => Err(()),
                };
                pending.push((base.clone(), text, outcome));
            }
            backend.phase = Phase::File;
            if let Ok(part) = guarded(&mut backend, |cx| instance.end_file(cx)) {
                parts.push((index, part));
            }
            used_keys.append(&mut backend.late_keys);
            diagnostics.extend(backend.diagnostics.iter().map(|d| {
                let category = if d.category == Category::Error {
                    "error"
                } else {
                    "warning"
                };
                format!("{category} {base}: {}", d.message)
            }));
        }

        let mut finish = MockFinish {
            transforms_all: setup_ran,
            ..MockFinish::default()
        };
        let has_errors = diagnostics.iter().any(|d| d.starts_with("error"));
        if !has_errors {
            let refs: Vec<(FileIndex, &ErasedPart)> = parts.iter().map(|(i, p)| (*i, p)).collect();
            plugin
                .finish(&shared, &mut state, &refs, &mut FinishCx::new(&mut finish))
                .map_err(|f| f.to_string())?;
            finish.check_keys(name, &used_keys)?;
        }
        let sites = pending
            .into_iter()
            .map(|(file, text, outcome)| {
                let outcome = match outcome {
                    Ok(Some(expr)) if !has_errors => {
                        Outcome::Luau(rbxts_luau::render_expr(&expr, &finish.late))
                    }
                    Ok(Some(_)) => Outcome::Luau("<not rendered: the build has errors>".to_owned()),
                    Ok(None) => Outcome::Lowered,
                    Err(()) => Outcome::Aborted,
                };
                (file, text, outcome)
            })
            .collect();
        Ok(BuildOutput {
            sites,
            diagnostics,
            finish,
            state,
            shared,
            setup_ran,
            committed: !has_errors,
            setup_messages: std::mem::take(&mut setup.messages),
        })
    }
}

/// The per-checker `(symbol, role)` classification memo.
type ClassifyMemo = HashMap<(P<Symbol>, SymbolRole), Option<Tag>>;

/// Runs a hook; an `Err` without a new error diagnostic becomes an internal error naming the plugin.
fn guarded<T>(
    backend: &mut MockCx,
    hook: impl FnOnce(&mut Cx<'_>) -> Result<T, rbxts_plugin::Abort>,
) -> Result<T, ()> {
    let errors_before = backend.errors().len();
    let result = hook(&mut Cx::new(backend));
    if result.is_err() && backend.errors().len() == errors_before {
        let message = format!(
            "internal: plugin `{}` returned Err without reporting an error",
            backend.plugin
        );
        let root = backend.root();
        backend
            .diagnostics
            .push(PluginDiagnostic::error(root, message));
    }
    result.map_err(|_| ())
}
