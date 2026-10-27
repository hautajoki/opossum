//! `stamp`: a tiny plugin written against the API, as a user would.
//!
//! It rewrites calls to functions whose JSDoc has a `@stamp` tag:
//! - `@stamp` → a string literal `"src/file.ts:line:column"` (UTF-16 column, 1-based);
//! - `@stamp build` → a late string, one key per call site, resolved in `finish` to `"<prefix>#k/n"` where `k`
//!   numbers the sites over all files in program order. Output is therefore independent of which checker made
//!   which file.
//!
//! Arguments are an error; `new` and decorator calls are declined (roblox-ts lowers them); an unknown mode is a
//! warning at each call, which is then declined (not a warning from `classify_symbol`, whose count would depend on
//! the checker count). Late keys use the file's path, never its `FileIndex`.
//!
//! The prefix comes from the tsconfig entry's `prefix`, overridden by the `STAMP_PREFIX` environment variable.
//! The `k/n` values change when sites are added anywhere, which the late-string contract allows (the compiler
//! re-renders cached files); a real plugin keeps committed values stable instead. A persistent build counter lives in the plugin state and in
//! `.stamp-state`; finish writes `stamp-report.json`.

use std::cell::RefCell;
use std::sync::Arc;

use rbxts_plugin::ts::{P, Symbol};
use rbxts_plugin::{
    BuildKind, CallKind, CallSite, CheckerInfo, Claim, Cx, Fatal, FileIndex, FinishCx, Interest,
    LuauExpr, PResult, Plugin, PluginInfo, PluginInstance, SetupCx, StateOrigin, SymbolRole, Tag,
    text_literal,
};

pub const HERE: Tag = Tag(0);
pub const BUILD: Tag = Tag(1);
pub const UNKNOWN: Tag = Tag(2);
pub const STATE_FILE: &str = ".stamp-state";
pub const REPORT_FILE: &str = "stamp-report.json";

pub struct Stamp;

pub struct StampShared {
    pub prefix: String,
}

/// Persistent across builds: how many builds have finished.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StampState {
    pub builds: u32,
}

/// One file's late keys, in the order they were handed out.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StampPart {
    pub build_keys: Vec<String>,
}

pub struct StampInstance {
    keys: RefCell<Vec<String>>,
}

impl Plugin for Stamp {
    type Shared = StampShared;
    type State = StampState;
    type FilePart = StampPart;
    type Instance = StampInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo {
            name: "stamp",
            version: "0.1.0",
            tsconfig_transform: Some("rbxts-transform-stamp"),
        }
    }

    fn setup(
        &self,
        cx: &mut SetupCx<'_>,
        state: &mut StampState,
    ) -> Result<Arc<StampShared>, Fatal> {
        let mut prefix = match cx.tsconfig_entry().and_then(|entry| entry.get("prefix")) {
            None => "build ".to_owned(),
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) => {
                return Err(
                    Fatal::new(format!("stamp: `prefix` must be a string, got {other}"))
                        .with_hint("Set it in tsconfig.json under compilerOptions.plugins."),
                );
            }
        };
        if let Some(from_env) = cx.env("STAMP_PREFIX") {
            prefix = from_env;
        }
        // a new process reuses what the last build persisted
        if cx.state_origin() == StateOrigin::Fresh && cx.build_kind() == BuildKind::Incremental {
            let bytes = cx
                .read_persisted(STATE_FILE)
                .map_err(|e| Fatal::new(format!("stamp: {e}")))?;
            if let Some(bytes) = bytes {
                let text = String::from_utf8_lossy(&bytes);
                state.builds = text
                    .trim()
                    .parse()
                    .map_err(|_| Fatal::new("stamp: corrupt state file"))?;
            }
        }
        cx.info(&format!(
            "stamp: prefix {prefix:?}, {} earlier builds",
            state.builds
        ));
        Ok(Arc::new(StampShared { prefix }))
    }

    fn instance(&self, _shared: Arc<StampShared>, _checker: CheckerInfo) -> StampInstance {
        StampInstance {
            keys: RefCell::new(Vec::new()),
        }
    }

    fn finish(
        &self,
        shared: &StampShared,
        state: &mut StampState,
        parts: &[(FileIndex, &StampPart)],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        let total = parts
            .iter()
            .map(|(_, part)| part.build_keys.len())
            .sum::<usize>();
        let mut k = 0;
        for (_, part) in parts {
            for key in &part.build_keys {
                k += 1;
                cx.resolve_late(key, &format!("{}#{k}/{total}", shared.prefix));
            }
        }
        state.builds += 1;
        cx.write_artifact(
            REPORT_FILE,
            format!("{{\"sites\":{total},\"builds\":{}}}\n", state.builds).into_bytes(),
        );
        cx.write_state_file(STATE_FILE, state.builds.to_string().into_bytes());
        Ok(())
    }
}

impl PluginInstance for StampInstance {
    type FilePart = StampPart;

    fn interest(&self, interest: &mut Interest) {
        interest.calls_by_symbol();
    }

    fn classify_symbol(&self, cx: &mut Cx<'_>, symbol: P<Symbol>, role: SymbolRole) -> Option<Tag> {
        if role != SymbolRole::Callee {
            return None;
        }
        let tag = cx
            .symbol_jsdoc_tags(symbol)
            .into_iter()
            .find(|t| t.name == "stamp")?;
        Some(match tag.comment.as_str() {
            "" => HERE,
            "build" => BUILD,
            _ => UNKNOWN,
        })
    }

    fn begin_file(&self, _cx: &mut Cx<'_>) -> PResult<()> {
        self.keys.borrow_mut().clear();
        Ok(())
    }

    fn call(&self, cx: &mut Cx<'_>, call: &CallSite, tag: Tag) -> PResult<Claim<LuauExpr>> {
        if matches!(call.kind, CallKind::New | CallKind::Decorator) {
            return Ok(Claim::Decline);
        }
        if tag == UNKNOWN {
            let name = cx.source_text(call.callee);
            cx.warning(
                call.node,
                format!("`{name}` has an unknown @stamp mode; the call is left as it is"),
            );
            return Ok(Claim::Decline);
        }
        if !call.args.is_empty() {
            let name = cx.source_text(call.callee);
            return Err(cx.error(call.node, format!("`{name}` takes no arguments")));
        }
        if tag == BUILD {
            let file = cx.file().relative_path.clone();
            let key = {
                let mut keys = self.keys.borrow_mut();
                let key = format!("site:{file}:{}", keys.len());
                keys.push(key.clone());
                key
            };
            return Ok(Claim::Take(cx.late_string(&key)));
        }
        let pos = cx.utf16_pos(call.node);
        let label = format!(
            "{}:{}:{}",
            cx.file().relative_path,
            pos.line + 1,
            pos.character + 1
        );
        Ok(Claim::Take(text_literal(&label)))
    }

    fn end_file(&self, _cx: &mut Cx<'_>) -> PResult<StampPart> {
        Ok(StampPart {
            build_keys: std::mem::take(&mut *self.keys.borrow_mut()),
        })
    }
}
