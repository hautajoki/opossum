//! `probe-inputs`: a plugin that uses every build input and persistence method of DESIGN §14, for the compiler's
//! watch, incremental and commit tests. It claims nothing; what it reads shows in a header of the files that ask
//! for it and in its artifacts. Paths are relative to the project directory.
//!
//! - **setup:** `SetupCx::env("PROBE_SETUP")` and `SetupCx::record_glob(setup-glob, *.txt)`.
//! - **files** containing the text `probe-inputs` (marked): `Cx::env("PROBE_FILE")` and the late string `marked`,
//!   in a header comment and a local; their part says so. One containing `probe-glob` also records
//!   `Cx::record_glob(file-glob, *.txt)` (in the header and the part too). A file containing
//!   `probe-warn` gets a warning at its start (the plugin's diagnostic source, `@probe/inputs`), one containing
//!   `probe-own-source` a warning naming its own source (`@probe/own`).
//! - **finish:** `record_glob(globs/finish, *.json)`, `read_dir(finish-dir)`, `exists(finish-flag)` and
//!   `read_file(finish-file.txt)` go into the artifact `probe-inputs.txt` with the parts and the setup facts; each
//!   file of `finish-dir` gets an artifact `probe-out/deep/<name>.out` (so removing them empties two directories);
//!   each line `remove <dir>` of `finish-file.txt` is a `remove_empty_dir(<dir>)`. `marked` resolves to the number
//!   of marked files, so marking a file changes the value in every other one.
//! - **parts** are persistable: `PIP1`, then the fields, little-endian, `u32`-length strings.
//!
//! The globs are flat: a file directly in the root whose name ends with the extension, sorted by name.

use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

use rbxts_luau::create;
use rbxts_plugin::{
    CheckerInfo, Cx, Fatal, FileIndex, FinishCx, GlobMatcher, Interest, PResult, Plugin, PluginDiagnostic, PluginInfo,
    PluginInstance, SetupCx,
};

/// The plugin.
pub struct InputsProbe;

/// What setup read.
pub struct InputsShared {
    root: String,
    setup_env: Option<String>,
    setup_globs: Vec<String>,
}

/// One file's part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputsPart {
    /// The file's path.
    pub path: String,
    /// The file has a marked call (its output depends on the inputs).
    pub marked: bool,
    /// The environment variable's value the file read.
    pub env: Option<String>,
    /// The glob matches the file read.
    pub globs: Vec<String>,
}

/// The per-checker instance.
pub struct InputsInstance {
    shared: Arc<InputsShared>,
    part: RefCell<InputsPart>,
}

/// A flat glob: the files directly in the root whose name ends with the extension, sorted by name.
struct Ext(&'static str);

impl GlobMatcher for Ext {
    fn expand(&self, root: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(self.0))
            .collect();
        names.sort();
        names
    }

    fn affected_by(&self, relative: &str) -> bool {
        !relative.contains('/') && relative.ends_with(self.0)
    }
}

fn join(root: &str, relative: &str) -> String {
    format!("{}/{relative}", root.trim_end_matches(['/', '\\']))
}

impl Plugin for InputsProbe {
    type Shared = InputsShared;
    type State = ();
    type FilePart = InputsPart;
    type Instance = InputsInstance;

    fn info(&self) -> PluginInfo {
        PluginInfo { name: "probe-inputs", version: "0.1.0", tsconfig_transform: None }
    }

    fn diagnostic_source(&self) -> Option<&'static str> {
        Some("@probe/inputs")
    }

    fn setup(&self, cx: &mut SetupCx<'_>, _: &mut ()) -> Result<Arc<InputsShared>, Fatal> {
        let root = cx.project().data.project_path.replace('\\', "/");
        let setup_env = cx.env("PROBE_SETUP");
        let setup_globs = cx.record_glob(join(&root, "setup-glob"), "*.txt", Ext(".txt"));
        Ok(Arc::new(InputsShared { root, setup_env, setup_globs }))
    }

    fn instance(&self, shared: Arc<InputsShared>, _: CheckerInfo) -> InputsInstance {
        InputsInstance { shared, part: RefCell::default() }
    }

    fn finish(
        &self,
        shared: &InputsShared,
        _: &mut (),
        parts: &[(FileIndex, &InputsPart)],
        cx: &mut FinishCx<'_>,
    ) -> Result<(), Fatal> {
        let marked = parts.iter().filter(|(_, part)| part.marked).count();
        cx.resolve_late("marked", &format!("{marked} marked"));
        let root = &shared.root;
        let globs = cx.record_glob(join(root, "globs/finish"), "*.json", Ext(".json"));
        let dir = cx.read_dir(join(root, "finish-dir"));
        let flag = cx.exists(join(root, "finish-flag"));
        let file = cx.read_file(join(root, "finish-file.txt")).map_err(|e| Fatal::new(e.to_string()))?;
        let mut report = format!(
            "setup env {:?}, setup glob {:?}\nfinish glob {globs:?}\nfinish dir {:?}\nflag {flag}\nfile {file:?}\n",
            shared.setup_env,
            shared.setup_globs,
            dir.as_ref().map(|entries| entries.iter().map(|e| format!("{}:{:?}", e.name, e.kind)).collect::<Vec<_>>()),
        );
        for (_, part) in parts.iter().filter(|(_, part)| part.marked) {
            report.push_str(&format!("{} env {:?} glob {:?}\n", part.path, part.env, part.globs));
        }
        cx.write_artifact("probe-inputs.txt", report.into_bytes());
        for entry in dir.into_iter().flatten().filter(|e| e.kind == rbxts_plugin::EntryKind::File) {
            cx.write_artifact(format!("probe-out/deep/{}.out", entry.name), entry.name.clone().into_bytes());
        }
        for line in file.as_deref().unwrap_or_default().lines() {
            if let Some(dir) = line.trim().strip_prefix("remove ") {
                cx.remove_empty_dir(dir)?;
            }
        }
        Ok(())
    }

    fn encode_file_part(&self, part: &InputsPart) -> Option<Vec<u8>> {
        let mut out = b"PIP1".to_vec();
        let string = |out: &mut Vec<u8>, text: &str| {
            out.extend((text.len() as u32).to_le_bytes());
            out.extend(text.as_bytes());
        };
        string(&mut out, &part.path);
        out.push(u8::from(part.marked));
        match &part.env {
            Some(env) => {
                out.push(1);
                string(&mut out, env);
            }
            None => out.push(0),
        }
        out.extend((part.globs.len() as u32).to_le_bytes());
        for glob in &part.globs {
            string(&mut out, glob);
        }
        Some(out)
    }

    fn decode_file_part(&self, bytes: &[u8]) -> Option<InputsPart> {
        let mut reader = Reader { rest: bytes.strip_prefix(b"PIP1")? };
        let path = reader.string()?;
        let marked = match reader.byte()? {
            0 => false,
            1 => true,
            _ => return None,
        };
        let env = match reader.byte()? {
            0 => None,
            1 => Some(reader.string()?),
            _ => return None,
        };
        let count = reader.u32()?;
        let globs = (0..count).map(|_| reader.string()).collect::<Option<Vec<_>>>()?;
        reader.rest.is_empty().then_some(InputsPart { path, marked, env, globs })
    }
}

/// Reads the part format (also `probe`'s).
pub(crate) struct Reader<'a> {
    pub(crate) rest: &'a [u8],
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (head, tail) = self.rest.split_at_checked(n)?;
        self.rest = tail;
        Some(head)
    }

    pub(crate) fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> Option<usize> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?) as usize)
    }

    pub(crate) fn string(&mut self) -> Option<String> {
        let n = self.u32()?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

impl PluginInstance for InputsInstance {
    type FilePart = InputsPart;

    fn interest(&self, _: &mut Interest) {}

    fn begin_file(&self, cx: &mut Cx<'_>) -> PResult<()> {
        let source = cx.file().source;
        let text = source.text();
        let mut part = InputsPart { path: cx.file().relative_path.clone(), ..InputsPart::default() };
        if text.contains("probe-inputs") {
            part.marked = true;
            part.env = cx.env("PROBE_FILE");
            if text.contains("probe-glob") {
                part.globs = cx.record_glob(join(&self.shared.root, "file-glob"), "*.txt", Ext(".txt"));
            }
            let header = format!(" probe-inputs: env {:?}, glob {:?}", part.env, part.globs);
            let marked = cx.late_string("marked");
            let id = cx.temp_id("probeInputs");
            cx.header([create::comment(header), create::local(id, Some(marked))]);
        }
        if text.contains("probe-warn") {
            cx.warning(source.as_node(), "probe-inputs warning");
        }
        if text.contains("probe-own-source") {
            cx.report(PluginDiagnostic::warning(source.as_node(), "probe-inputs own source").with_source("@probe/own"));
        }
        *self.part.borrow_mut() = part;
        Ok(())
    }

    fn end_file(&self, _: &mut Cx<'_>) -> PResult<InputsPart> {
        Ok(self.part.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_round_trip_and_bad_bytes_are_refused() {
        let probe = InputsProbe;
        for part in [
            InputsPart::default(),
            InputsPart {
                path: "src/a.ts".into(),
                marked: true,
                env: Some("x".into()),
                globs: vec!["a.txt".into(), "b.txt".into()],
            },
        ] {
            let bytes = probe.encode_file_part(&part).unwrap();
            assert_eq!(probe.decode_file_part(&bytes), Some(part));
            for cut in 0..bytes.len() {
                assert_eq!(probe.decode_file_part(&bytes[..cut]), None);
            }
            let mut extra = bytes.clone();
            extra.push(0);
            assert_eq!(probe.decode_file_part(&extra), None);
        }
        assert_eq!(probe.decode_file_part(b"PIP2"), None);
    }
}
