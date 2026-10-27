//! `transformer`: claims the tsconfig `plugins` entry `{ "transform": "./identity.cjs" }` and does nothing else, so a
//! project naming it builds as with stock roblox-ts and a TypeScript transformer that changes nothing: the
//! `--writeTransformedFiles` fixture (`tests/fixtures/write_transformed`), whose `identity.cjs` is that transformer.

use std::sync::Arc;

use rbxts_plugin::{
    CheckerInfo, Cx, Fatal, FileIndex, FinishCx, Interest, PResult, Plugin, PluginInfo, PluginInstance, SetupCx,
};

/// The tsconfig transform it claims.
pub const TRANSFORM: &str = "./identity.cjs";

/// The `transformer` plugin.
pub struct Transformer;

/// Its per-checker instance (no hooks).
pub struct TransformerInstance;

impl Plugin for Transformer {
    type Shared = ();
    type State = ();
    type FilePart = ();
    type Instance = TransformerInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo { name: "transformer", version: "0.1.0", tsconfig_transform: Some(TRANSFORM) }
    }

    fn setup(&self, _cx: &mut SetupCx<'_>, _state: &mut ()) -> Result<Arc<()>, Fatal> {
        Ok(Arc::new(()))
    }

    fn instance(&self, _shared: Arc<()>, _checker: CheckerInfo) -> TransformerInstance {
        TransformerInstance
    }

    fn finish(&self, _: &(), _: &mut (), _: &[(FileIndex, &())], _: &mut FinishCx<'_>) -> Result<(), Fatal> {
        Ok(())
    }
}

impl PluginInstance for TransformerInstance {
    type FilePart = ();

    fn interest(&self, _interest: &mut Interest) {}

    fn end_file(&self, _cx: &mut Cx<'_>) -> PResult<()> {
        Ok(())
    }
}
