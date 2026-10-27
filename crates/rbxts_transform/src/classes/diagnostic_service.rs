//! `classes/DiagnosticService.ts`: the diagnostics of one file's transform.
//!
//! rbxts-rs: roblox-ts keeps one static list for the whole build; here each file's `TransformState` owns one, and
//! the pipeline collects them in transform order. roblox-ts's `addSingleDiagnostic` is not ported: nothing in
//! roblox-ts calls it.
//!
//! `addDiagnosticWithCache` (once per compilation and symbol) does not depend on the checker count: its caches are
//! keyed by [`ReportOnce`], each file records the keys of what it reported, and the build drops a file's diagnostic
//! whose key an earlier file (transform order) reported ([`drop_reported_earlier`]).

use rustc_hash::FxHashSet;

use tsrs_ast::{CheckFlags, Diagnostic, Node, SourceFile, Symbol, SymbolFlags};
use tsrs_core::P;
use tsrs_diagnostics::Category;

/// `DiagnosticService`
#[derive(Default)]
pub struct DiagnosticService {
    /// The file's diagnostics, in report order.
    pub diagnostics: Vec<P<Diagnostic>>,
    /// rbxts-rs: the diagnostics reported once per compilation, with their keys ([`drop_reported_earlier`]).
    pub once: Vec<(ReportOnce, P<Diagnostic>)>,
}

impl DiagnosticService {
    /// `addDiagnostic`
    pub fn add_diagnostic(&mut self, diagnostic: P<Diagnostic>) {
        self.diagnostics.push(diagnostic);
    }

    /// `addDiagnosticWithCache`'s report, once its cache took `key`.
    ///
    /// rbxts-rs: an error, always (roblox-ts's three are: `noAny`, enum and namespace merging). A build drops a file's
    /// once-reported diagnostic an earlier file reported, and keeps what is left with the file: an incremental or
    /// watch build that does not transform the earlier file again reports what was kept. That is only what a clean
    /// build reports because a file with an error never commits, so nothing once-keyed is ever kept.
    pub fn add_diagnostic_once(&mut self, key: ReportOnce, diagnostic: P<Diagnostic>) {
        assert_eq!(diagnostic.category(), Category::Error, "a once-per-compilation diagnostic must be an error");
        self.once.push((key, diagnostic));
        self.add_diagnostic(diagnostic);
    }

    /// `hasErrors`
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.category() == Category::Error)
    }
}

/// rbxts-rs: which `addDiagnosticWithCache` cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OnceCache {
    /// `isReportedByNoAnyCache`
    NoAny,
    /// `isReportedByMultipleDefinitionsCache`
    MultipleDefinitions,
}

/// rbxts-rs: the key of a once-per-compilation diagnostic, the same on every checker and whatever each checker
/// checked before. A symbol with declarations is keyed by its first declaration, its name, its declaration count and
/// the check flags that say what kind of symbol a checker made ([`KIND_FLAGS`]: a mapped type's property, a reverse
/// mapped one, a union or intersection's synthetic property), whether it is the binder's symbol or one a checker made
/// (merged or instantiated: `Transient`, a different object on each checker). An instantiation and its declared
/// symbol must share a key: `instantiateSymbol` answers a property of an instantiation (`Holder<string>.value`) with
/// the declared symbol when that property's type is already resolved and has no type variables, else with a new
/// transient one (`Instantiated`, left out of the key), so which of the two a checker gives depends on what it
/// resolved before. The kind flags don't depend on history: a checker makes a mapped type's properties and a union's
/// synthetic properties anew for each such type, never from what it resolved earlier. A symbol without declarations
/// has no identity across checkers: a binder one is its own key, a transient one is reported once per file.
///
/// Against stock roblox-ts (one checker, keyed by symbol object): the same, and stock reports a union's
/// (`(Alpha | Beta).value`) or a mapped type's (`Partial<Src>.value`) property apart from its source's, as here.
/// Two distinct symbols of one kind with the same declarations still share a key here, where stock reports each:
/// `Partial<Src>.value` next to `Readonly<Src>.value`, or one union property next to another with the same
/// declarations. Only the instantiation case (`value: T` with `T = any` in two instantiations of one interface,
/// each a new symbol or the declared one) depends on stock's checker history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReportOnce {
    cache: OnceCache,
    key: OnceKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum OnceKey {
    Symbol(P<Symbol>),
    Declared(P<Node>, &'static str, CheckFlags, usize),
    InFile(P<Symbol>, P<SourceFile>),
}

/// The check flags in a [`ReportOnce`] key: those of a symbol a checker makes for a mapped type or a union or
/// intersection, which never depend on what the checker resolved before (unlike `Instantiated`).
const KIND_FLAGS: CheckFlags = CheckFlags::Mapped.union(CheckFlags::ReverseMapped).union(CheckFlags::Synthetic);

impl ReportOnce {
    /// The key of `symbol` in `cache`, reported in `file`.
    pub fn new(cache: OnceCache, symbol: P<Symbol>, file: P<SourceFile>) -> ReportOnce {
        let key = if let Some(&declaration) = symbol.declarations().first() {
            let kind = symbol.check_flags() & KIND_FLAGS;
            OnceKey::Declared(declaration, symbol.name(), kind, symbol.declarations().len())
        } else if !symbol.flags().intersects(SymbolFlags::Transient) {
            OnceKey::Symbol(symbol)
        } else {
            OnceKey::InFile(symbol, file)
        };
        ReportOnce { cache, key }
    }
}

/// rbxts-rs: `files` in transform order, each as its diagnostics and the once-per-compilation ones among them. Drops
/// each diagnostic whose key an earlier file reported (within a file, the transform's cache kept the first).
pub fn drop_reported_earlier<'a>(
    files: impl Iterator<Item = (&'a mut Vec<P<Diagnostic>>, &'a [(ReportOnce, P<Diagnostic>)])>,
) {
    let mut reported: FxHashSet<ReportOnce> = FxHashSet::default();
    for (diagnostics, once) in files {
        let dropped: FxHashSet<P<Diagnostic>> =
            once.iter().filter(|(key, _)| !reported.insert(*key)).map(|&(_, diagnostic)| diagnostic).collect();
        if !dropped.is_empty() {
            diagnostics.retain(|d| !dropped.contains(d));
        }
    }
}
