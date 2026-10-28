use std::fs;
use std::io;
use std::path::Path;

use rbx_dom_weak::{InstanceBuilder, WeakDom};

/// The Studio plugin's Script.
pub const PLUGIN_SOURCE: &str = include_str!("../../../plugin/opossum.server.luau");

/// The ModuleScripts under it, which it requires as `@self/<name>`.
const PLUGIN_MODULES: [(&str, &str); 2] =
    [("apply", include_str!("../../../plugin/apply.luau")), ("watch", include_str!("../../../plugin/watch.luau"))];

/// The plugin as an .rbxm: a Script named `opossum` holding `PLUGIN_SOURCE`, with its ModuleScripts as children.
pub fn plugin_rbxm() -> io::Result<Vec<u8>> {
    let mut dom = WeakDom::new(InstanceBuilder::new("DataModel"));
    let script = InstanceBuilder::new("Script")
        .with_name("opossum")
        .with_property("Source", PLUGIN_SOURCE)
        .with_children(PLUGIN_MODULES.map(|(name, source)| {
            InstanceBuilder::new("ModuleScript").with_name(name).with_property("Source", source)
        }));
    let script = dom.insert(dom.root_ref(), script);
    let mut out = Vec::new();
    rbx_binary::to_writer(&mut out, &dom, &[script]).map_err(io::Error::other)?;
    Ok(out)
}

/// Writes the plugin .rbxm to `path` (Studio's Plugins folder, typically), creating its directory.
pub fn install_plugin(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, plugin_rbxm()?)
}
