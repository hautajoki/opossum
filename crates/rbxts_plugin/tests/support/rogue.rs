//! `rogue`: a plugin that breaks one contract at a time, so the tests can check that the compiler (here the mock)
//! catches each one. It also explains checker diagnostics, which `stamp` does not use.

use std::cell::RefCell;
use std::sync::Arc;

use rbxts_plugin::ts::{Diagnostic, P, Symbol};
use rbxts_plugin::{
    Abort, CallSite, CheckerInfo, Claim, Cx, Fatal, FileIndex, FinishCx, Interest, LuauExpr,
    PResult, Plugin, PluginDiagnostic, PluginInfo, PluginInstance, SetupCx, SymbolRole, Tag,
};

/// The contract a [`Rogue`] breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Hands out a late string and never resolves it.
    UnresolvedKey,
    /// Keeps the `Abort` of its first error and returns it again from the next call, recording nothing new.
    ErrWithoutError,
    /// Calls `compile_expr` from `end_file`.
    CompileInEndFile,
    /// Warns from `classify_symbol`.
    WarnInClassify,
}

pub struct Rogue(pub Mode);

pub struct RogueInstance {
    mode: Mode,
    saved: RefCell<Option<Abort>>,
}

impl Plugin for Rogue {
    type Shared = Mode;
    type State = ();
    type FilePart = ();
    type Instance = RogueInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "rogue",
            version: "0.0.0",
            tsconfig_transform: None,
        }
    }

    fn setup(&self, _: &mut SetupCx<'_>, _: &mut ()) -> Result<Arc<Mode>, Fatal> {
        Ok(Arc::new(self.0))
    }

    fn instance(&self, shared: Arc<Mode>, _: CheckerInfo) -> RogueInstance {
        RogueInstance {
            mode: *shared,
            saved: RefCell::new(None),
        }
    }

    fn finish(
        &self,
        _: &Mode,
        _: &mut (),
        _: &[(FileIndex, &())],
        _: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        Ok(())
    }
}

impl PluginInstance for RogueInstance {
    type FilePart = ();

    fn interest(&self, interest: &mut Interest) {
        interest.calls_by_symbol();
    }

    fn classify_symbol(&self, cx: &mut Cx<'_>, symbol: P<Symbol>, _: SymbolRole) -> Option<Tag> {
        if self.mode == Mode::WarnInClassify {
            cx.warning(symbol.declarations()[0], "noisy");
        }
        Some(Tag(0))
    }

    fn call(&self, cx: &mut Cx<'_>, call: &CallSite, _: Tag) -> PResult<Claim<LuauExpr>> {
        match self.mode {
            Mode::UnresolvedKey => Ok(Claim::Take(cx.late_string("never"))),
            Mode::ErrWithoutError => {
                let saved = self.saved.borrow_mut().take();
                match saved {
                    Some(abort) => Err(abort),
                    None => {
                        let abort = cx.error(call.node, "first");
                        *self.saved.borrow_mut() = Some(abort);
                        Ok(Claim::Decline)
                    }
                }
            }
            Mode::CompileInEndFile | Mode::WarnInClassify => Ok(Claim::Decline),
        }
    }

    fn end_file(&self, cx: &mut Cx<'_>) -> PResult<()> {
        if self.mode == Mode::CompileInEndFile {
            let root = cx.file().source.as_node();
            cx.compile_expr(root)?;
        }
        Ok(())
    }

    fn explain_checker_diagnostic(
        &self,
        cx: &mut Cx<'_>,
        diagnostic: P<Diagnostic>,
    ) -> Option<PluginDiagnostic> {
        // `localize()`, not `message_text()`, which is empty for diagnostics made from a message
        let text = diagnostic.localize();
        text.contains("removed").then(|| {
            let file = cx.file().relative_path.clone();
            PluginDiagnostic::error(
                cx.file().source.as_node(),
                format!("{file}: explained: {text}"),
            )
            .with_hint("Rebuild the package.")
        })
    }
}
