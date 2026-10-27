//! `scan`: a plugin whose `Shared` depends on whether its own state file exists, as Flamework's candidate scan does
//! (it stops at the project's own `flamework.build`, and loads an ancestor's while its own is missing).
//!
//! - `setup` reads its state file [`STATE_FILE`] through `read_input` (a build input). When it is there, the answer
//!   is `own`; when it is missing, `setup` reads [`ANCESTOR_FILE`] (a build input too) and the answer is
//!   `ancestor <its text>` (`ancestor none` without one).
//! - Calls of functions whose JSDoc has a `@scan` tag become a string literal of that answer, so the outputs of
//!   the files that call one depend on it, as Flamework's ids do.
//! - `finish` writes the state file with the number of builds this state saw, new bytes at every build (as
//!   Flamework's hashed id mode writes a new salt at every fresh setup). The `Shared` never depends on those bytes.
//!
//! [`ScanReader`] (`scan-reader`) reads that state file in its own `setup`: another plugin's state file as an input.

use std::sync::Arc;

use rbxts_plugin::ts::{P, Symbol};
use rbxts_plugin::{
    CallKind, CallSite, CheckerInfo, Claim, Cx, Fatal, FileIndex, FinishCx, Interest, LuauExpr, PResult, Plugin,
    PluginInfo, PluginInstance, SetupCx, SymbolRole, Tag, text_literal,
};

/// The plugin's state file, in the project directory.
pub const STATE_FILE: &str = "scan-state.txt";
/// What `setup` reads while its state file is missing (Flamework: an ancestor's `flamework.build`).
pub const ANCESTOR_FILE: &str = "scan-ancestor.txt";

const SCAN: Tag = Tag(0);

/// The `scan` plugin.
pub struct Scan;

/// `scan`'s per-checker instance.
pub struct ScanInstance(Arc<String>);

impl Plugin for Scan {
    type Shared = String;
    type State = u32;
    type FilePart = ();
    type Instance = ScanInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo { name: "scan", version: "0.1.0", tsconfig_transform: None }
    }

    fn setup(&self, cx: &mut SetupCx<'_>, _state: &mut u32) -> Result<Arc<String>, Fatal> {
        let read = |cx: &mut SetupCx<'_>, path: &str| cx.read_input(path).map_err(|e| Fatal::new(format!("scan: {e}")));
        if read(cx, STATE_FILE)?.is_some() {
            return Ok(Arc::new("own".to_owned()));
        }
        let ancestor = read(cx, ANCESTOR_FILE)?;
        Ok(Arc::new(format!("ancestor {}", ancestor.as_deref().map_or("none", str::trim))))
    }

    fn instance(&self, shared: Arc<String>, _checker: CheckerInfo) -> ScanInstance {
        ScanInstance(shared)
    }

    fn finish(
        &self,
        _shared: &String,
        state: &mut u32,
        _parts: &[(FileIndex, &())],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        *state += 1;
        cx.write_state_file(STATE_FILE, format!("{state}\n").into_bytes());
        Ok(())
    }

    fn encode_file_part(&self, _part: &()) -> Option<Vec<u8>> {
        Some(Vec::new())
    }

    fn decode_file_part(&self, _bytes: &[u8]) -> Option<()> {
        Some(())
    }
}

impl PluginInstance for ScanInstance {
    type FilePart = ();

    fn interest(&self, interest: &mut Interest) {
        interest.calls_by_symbol();
    }

    fn classify_symbol(&self, cx: &mut Cx<'_>, symbol: P<Symbol>, role: SymbolRole) -> Option<Tag> {
        (role == SymbolRole::Callee && cx.symbol_jsdoc_tags(symbol).iter().any(|t| t.name == "scan")).then_some(SCAN)
    }

    fn call(&self, _cx: &mut Cx<'_>, call: &CallSite, _tag: Tag) -> PResult<Claim<LuauExpr>> {
        if matches!(call.kind, CallKind::New | CallKind::Decorator) || !call.args.is_empty() {
            return Ok(Claim::Decline);
        }
        Ok(Claim::Take(text_literal(&self.0)))
    }

    fn end_file(&self, _cx: &mut Cx<'_>) -> PResult<()> {
        Ok(())
    }
}

/// `scan-reader`: another plugin's state file as a setup input. Its `setup` reads `scan`'s [`STATE_FILE`] through
/// `read_input`, and calls of functions whose JSDoc has a `@scanreader` tag become a string literal of
/// `reader <its text>` (`reader none` without one). It has no state file of its own, and `scan` writes new bytes
/// at every build, so its output depends on bytes another plugin's commit changes.
pub struct ScanReader;

/// `scan-reader`'s per-checker instance.
pub struct ScanReaderInstance(Arc<String>);

const READER: Tag = Tag(0);

impl Plugin for ScanReader {
    type Shared = String;
    type State = ();
    type FilePart = ();
    type Instance = ScanReaderInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo { name: "scan-reader", version: "0.1.0", tsconfig_transform: None }
    }

    fn setup(&self, cx: &mut SetupCx<'_>, _state: &mut ()) -> Result<Arc<String>, Fatal> {
        let text = cx.read_input(STATE_FILE).map_err(|e| Fatal::new(format!("scan-reader: {e}")))?;
        Ok(Arc::new(format!("reader {}", text.as_deref().map_or("none", str::trim))))
    }

    fn instance(&self, shared: Arc<String>, _checker: CheckerInfo) -> ScanReaderInstance {
        ScanReaderInstance(shared)
    }

    fn finish(
        &self,
        _shared: &String,
        _state: &mut (),
        _parts: &[(FileIndex, &())],
        _cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        Ok(())
    }

    fn encode_file_part(&self, _part: &()) -> Option<Vec<u8>> {
        Some(Vec::new())
    }

    fn decode_file_part(&self, _bytes: &[u8]) -> Option<()> {
        Some(())
    }
}

impl PluginInstance for ScanReaderInstance {
    type FilePart = ();

    fn interest(&self, interest: &mut Interest) {
        interest.calls_by_symbol();
    }

    fn classify_symbol(&self, cx: &mut Cx<'_>, symbol: P<Symbol>, role: SymbolRole) -> Option<Tag> {
        (role == SymbolRole::Callee && cx.symbol_jsdoc_tags(symbol).iter().any(|t| t.name == "scanreader"))
            .then_some(READER)
    }

    fn call(&self, _cx: &mut Cx<'_>, call: &CallSite, _tag: Tag) -> PResult<Claim<LuauExpr>> {
        if matches!(call.kind, CallKind::New | CallKind::Decorator) || !call.args.is_empty() {
            return Ok(Claim::Decline);
        }
        Ok(Claim::Take(text_literal(&self.0)))
    }

    fn end_file(&self, _cx: &mut Cx<'_>) -> PResult<()> {
        Ok(())
    }
}
