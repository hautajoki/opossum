//! [`PluginHost`]: the plugins of one checker (DESIGN §6, §11.1).

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustc_hash::{FxHashMap, FxHashSet};

use rbxts_plugin::erased::{DynInstance, ErasedShared};
use rbxts_plugin::{CheckerInfo, ClassFilter, FileIndex, Interest, JsDocTag, ReceiverInterest, SymbolRole, Tag};
use rbxts_project::{ModuleResolver, PackageInfo, ProjectServices, ResolvedModuleInfo};
use tsrs_ast::{Diagnostic, Kind, Node, SourceFile, Symbol};
use tsrs_checker::Checker;
use tsrs_compiler::Program;
use tsrs_core::P;

use super::HookKind;
use super::facts::{self, FileLines};
use super::lifecycle::ActivePlugin;
use crate::build::plugin_inputs::EnvSnapshot;

/// Where `specifier` resolves from `from_file` (an absolute path; the file need not exist): the program's own
/// resolution when a program file imports it from there, else a resolver with the program's options.
pub(crate) fn resolve_module_from(
    program: &'static Program,
    resolver: &OnceCell<ModuleResolver>,
    specifier: &str,
    from_file: &str,
) -> Option<ResolvedModuleInfo> {
    if let Some(from) = program.get_source_file(from_file) {
        let mode = program.get_default_resolution_mode_for_file(from);
        if let Some(resolved) = program.get_resolved_module(from, specifier, mode).filter(|r| r.is_resolved()) {
            return Some(ResolvedModuleInfo {
                file_name: resolved.resolved_file_name.to_owned(),
                is_external_library_import: resolved.is_external_library_import,
                package_name: (!resolved.package_id.name.is_empty()).then(|| resolved.package_id.name.to_owned()),
            });
        }
    }
    resolver.get_or_init(|| ModuleResolver::new(program.options(), program.get_current_directory())).resolve(
        specifier,
        from_file,
        tsrs_core::RESOLUTION_MODE_NONE,
    )
}

/// The `(symbol, role)` classification memo of one instance.
pub(crate) type ClassifyMemo = FxHashMap<(P<Symbol>, SymbolRole), Option<Tag>>;

/// One plugin's instance on this checker, with what its `Interest` asked for.
pub(crate) struct HostPlugin {
    pub name: &'static str,
    pub instance: Box<dyn DynInstance>,
    pub calls_by_symbol: bool,
    pub receivers: Vec<ReceiverInterest>,
    pub callee_markers: Vec<(&'static str, Tag)>,
    pub class_filter: Option<ClassFilter>,
    /// `assignments_by_receiver_symbol` was registered.
    pub assignments: bool,
    /// The `(symbol, role)` classification memo: `classify_symbol` is asked at most once for each.
    pub memo: RefCell<ClassifyMemo>,
}

/// A frame of the hook stack: a plugin hook (`Some(plugin)`) or compiler code a hook called (`None`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub plugin: Option<usize>,
    pub hook: HookKind,
    start: Instant,
    /// Time spent in frames nested in this one.
    nested: Duration,
}

/// The time and number of calls of one plugin's hooks of one kind, without the compiler work they asked for
/// (`compile_*`) and without other plugins' hooks nested in them.
#[derive(Clone, Copy, Debug, Default)]
pub struct HookTime {
    pub calls: u64,
    pub time: Duration,
}

/// The plugins of one checker: the instances, the dispatch tables built from their `Interest`s, and the caches
/// shared by every file this checker transforms. Not `Send`: it lives on its checker's thread.
pub struct PluginHost {
    pub(crate) program: &'static Program,
    pub(crate) plugins: Vec<HostPlugin>,
    /// Each plugin's `diagnostic_source()`.
    pub(crate) sources: Vec<Option<&'static str>>,
    /// The build's environment snapshot (`Cx::env`; `SetupCx::env` reads the same).
    pub(crate) env: Arc<EnvSnapshot>,
    /// Any call interest at all: the call claim points return at once without one.
    pub(crate) any_calls: bool,
    /// Some plugin classifies callee symbols.
    pub(crate) any_symbol: bool,
    /// Every method name of a receiver interest, for a quick test.
    pub(crate) receiver_methods: FxHashSet<&'static str>,
    pub(crate) any_classes: bool,
    /// Every `accesses_by_marker` registration: (plugin, marker, tag), in plugin then registration order.
    pub(crate) access_markers: Vec<(usize, &'static str, Tag)>,
    /// The same by marker name, for receivers checked against many markers (indices into `access_markers`).
    pub(crate) access_marker_index: FxHashMap<&'static str, Vec<usize>>,
    /// Some plugin registered `assignments_by_receiver_symbol`.
    pub(crate) any_assignments: bool,
    pub(crate) expressions: FxHashMap<Kind, Vec<(usize, Tag)>>,
    pub(crate) statements: FxHashMap<Kind, Vec<(usize, Tag)>>,
    /// `RBXTS_PLUGIN_CLAIMS=all`: ask every matching plugin, and report a double claim.
    pub(crate) claims_all: bool,
    /// The checker's diagnostics of each file (all of them warnings or suggestions: an error stops the build first).
    pub(crate) file_diagnostics: FxHashMap<P<SourceFile>, Vec<P<Diagnostic>>>,
    /// Each program file's index in the build's transform order (`build::transform_order`).
    pub(crate) file_indices: FxHashMap<P<SourceFile>, FileIndex>,
    lines: RefCell<FxHashMap<P<SourceFile>, Rc<FileLines>>>,
    jsdoc: RefCell<FxHashMap<P<Node>, Rc<[JsDocTag]>>>,
    resolver: OnceCell<ModuleResolver>,
    pub(crate) frames: RefCell<Vec<Frame>>,
    pub(crate) times: RefCell<Vec<[HookTime; HookKind::ALL.len()]>>,
}

impl PluginHost {
    /// Makes the instances of checker `checker` (on this thread) and their dispatch tables. `shared[i]` belongs to
    /// `active[i]`.
    pub(crate) fn new(
        program: &'static Program,
        active: &[ActivePlugin<'_>],
        shared: &[ErasedShared],
        checker: CheckerInfo,
        file_diagnostics: FxHashMap<P<SourceFile>, Vec<P<Diagnostic>>>,
        file_indices: FxHashMap<P<SourceFile>, FileIndex>,
        claims_all: bool,
    ) -> PluginHost {
        let mut host = PluginHost {
            program,
            plugins: Vec::with_capacity(active.len()),
            sources: active.iter().map(|plugin| plugin.plugin.diagnostic_source()).collect(),
            env: Arc::default(),
            any_calls: false,
            any_symbol: false,
            receiver_methods: FxHashSet::default(),
            any_classes: false,
            access_markers: Vec::new(),
            access_marker_index: FxHashMap::default(),
            any_assignments: false,
            expressions: FxHashMap::default(),
            statements: FxHashMap::default(),
            claims_all,
            file_diagnostics,
            file_indices,
            lines: RefCell::default(),
            jsdoc: RefCell::default(),
            resolver: OnceCell::new(),
            frames: RefCell::default(),
            times: RefCell::new(vec![[HookTime::default(); HookKind::ALL.len()]; active.len()]),
        };
        for (index, (plugin, shared)) in active.iter().zip(shared).enumerate() {
            let instance = plugin.plugin.instance(shared, checker);
            let mut interest = Interest::new();
            instance.interest(&mut interest);
            for &(marker, tag) in interest.access_markers() {
                host.access_marker_index.entry(marker).or_default().push(host.access_markers.len());
                host.access_markers.push((index, marker, tag));
            }
            host.any_assignments |= interest.wants_assignments_by_receiver_symbol();
            for &(kind, tag) in interest.expression_kinds() {
                host.expressions.entry(kind).or_default().push((index, tag));
            }
            for &(kind, tag) in interest.statement_kinds() {
                host.statements.entry(kind).or_default().push((index, tag));
            }
            host.receiver_methods.extend(interest.receiver_calls().iter().flat_map(|r| r.methods.iter().copied()));
            host.any_symbol |= interest.wants_calls_by_symbol();
            host.any_calls |= interest.wants_calls_by_symbol()
                || !interest.receiver_calls().is_empty()
                || !interest.callee_markers().is_empty();
            host.any_classes |= interest.class_filter().is_some();
            host.plugins.push(HostPlugin {
                name: plugin.info.name,
                instance,
                calls_by_symbol: interest.wants_calls_by_symbol(),
                receivers: interest.receiver_calls().to_vec(),
                callee_markers: interest.callee_markers().to_vec(),
                class_filter: interest.class_filter(),
                assignments: interest.wants_assignments_by_receiver_symbol(),
                memo: RefCell::default(),
            });
        }
        host
    }

    /// The number of plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// No plugins.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// A plugin's name.
    pub fn name(&self, plugin: usize) -> &'static str {
        self.plugins[plugin].name
    }

    // --- hook frames: timings and panic attribution ---

    pub(crate) fn enter(&self, plugin: Option<usize>, hook: HookKind) {
        self.frames.borrow_mut().push(Frame { plugin, hook, start: Instant::now(), nested: Duration::ZERO });
    }

    pub(crate) fn exit(&self) {
        let mut frames = self.frames.borrow_mut();
        let frame = frames.pop().expect("a hook frame");
        let elapsed = frame.start.elapsed();
        if let Some(parent) = frames.last_mut() {
            parent.nested += elapsed;
        }
        if let Some(plugin) = frame.plugin {
            let time = &mut self.times.borrow_mut()[plugin][frame.hook.index()];
            time.calls += 1;
            time.time += elapsed.saturating_sub(frame.nested);
        }
    }

    /// Adds the time of a hook run outside the transform (setup, finish, explain).
    pub(crate) fn add_time(
        times: &mut [[HookTime; HookKind::ALL.len()]],
        plugin: usize,
        hook: HookKind,
        time: Duration,
    ) {
        let slot = &mut times[plugin][hook.index()];
        slot.calls += 1;
        slot.time += time;
    }

    /// The plugin whose hook was running when a panic unwound out of the transform: the innermost frame, when it
    /// is a plugin's (a panic in compiler code that a hook called is the compiler's). Clears the stack.
    pub(crate) fn take_panic_frame(&self) -> Option<(usize, HookKind)> {
        let mut frames = self.frames.borrow_mut();
        let innermost = frames.last().copied();
        frames.clear();
        innermost.and_then(|f| f.plugin.map(|p| (p, f.hook)))
    }

    // --- source facts ---

    pub(crate) fn lines(&self, file: P<SourceFile>) -> Rc<FileLines> {
        if let Some(lines) = self.lines.borrow().get(&file) {
            return Rc::clone(lines);
        }
        let lines = Rc::new(FileLines::new(file));
        self.lines.borrow_mut().insert(file, Rc::clone(&lines));
        lines
    }

    /// The JSDoc tags of a declaration, cached for this checker.
    pub(crate) fn jsdoc_tags(&self, checker: &mut Checker, declaration: P<Node>) -> Rc<[JsDocTag]> {
        if let Some(tags) = self.jsdoc.borrow().get(&declaration) {
            return Rc::clone(tags);
        }
        let tags: Rc<[JsDocTag]> = facts::jsdoc_tags(checker, declaration).into();
        self.jsdoc.borrow_mut().insert(declaration, Rc::clone(&tags));
        tags
    }

    /// Where `specifier` resolves from `from_file`: the program's resolution when a program file imports it, else a
    /// resolver with the program's options.
    pub(crate) fn resolve_module(&self, specifier: &str, from_file: &str) -> Option<ResolvedModuleInfo> {
        resolve_module_from(self.program, &self.resolver, specifier, from_file)
    }

    /// The package.json of the package declaring `symbol` (its first declaration's file).
    pub(crate) fn declaring_package(services: &ProjectServices, symbol: P<Symbol>) -> Option<Arc<PackageInfo>> {
        let declaration = symbol.declarations().first().copied()?;
        let file = tsrs_ast::get_source_file_of_node(declaration)?;
        services.packages.package_of(file.get().file_name())
    }
}
