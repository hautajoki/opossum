//! The compiler's incremental state on disk (DESIGN §14), for an `incremental` or `composite` project: what the
//! next build in a new process needs to transform only the changed closure.
//!
//! It sits beside roblox-ts's build info, as `<base>.rbxts-rs.buildinfo` (roblox-ts's `<base>.rbxtsc.tsbuildinfo`
//! has another format), and is written last in the commit (after the plugins' state files), so a crash never
//! leaves it newer than what it describes. It holds the compiler's identity, the plugins' names and versions, the
//! salt (options, configs, Rojo files), each program file's facts, and the artifacts the build wrote. A different
//! compiler binary or plugin version, or an unreadable file, makes the next build a clean one.
//!
//! With plugins it also holds, per project file, what the next process needs to skip it ([`FileRecord`]): each
//! plugin's file part as the plugin encoded it (`Plugin::encode_file_part`, base64), the late strings its Luau
//! uses with their values, and what its hooks read (`Cx::env` values, `Cx::record_glob` answers); and what the
//! plugins' `setup` read ([`DiskInput`]), and the plugin state files the build left ([`StateFile`]). A file whose parts are all there and decode, whose environment reads
//! the same and which recorded no glob brings its parts to `finish` untransformed (with the plugins' states
//! starting fresh, as the API says).
//!
//! Every project file also keeps its last transform's diagnostics ([`DiskDiagnostic`]: warnings, since a file with an
//! error never commits), which a new process reports again for the files it does not transform, so an incremental
//! build reports what a clean build would. And the build's outputs keep their size and modification time
//! ([`super::outputs::Stamp`]): one changed or deleted by hand since makes its source be transformed again.

use std::path::Path;

use rustc_hash::FxHashMap;
use serde_json::{Map, Value, json};

use super::graph::{FileFacts, Version};
use super::outputs::Stamp;
use super::plugin_inputs::DiskInput;

/// The state file's format version.
const FORMAT: u64 = 3;

/// A transform diagnostic of a project file, as the incremental state keeps it: one of roblox-ts's or a plugin's
/// (an ad hoc message, without a code of its own), with its related information.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskDiagnostic {
    /// The file it is at, when not the file that reported it (a plugin diagnostic at a node of another file).
    pub file: Option<String>,
    /// The span in the file (UTF-8 byte offsets).
    pub pos: u32,
    /// Where the span ends.
    pub end: u32,
    /// `tsrs_diagnostics::Category` as a number: 0 warning, 1 error, 2 suggestion, 3 message.
    pub category: u8,
    /// The message text.
    pub text: String,
    /// A plugin diagnostic's source (the reporter's code slot).
    pub source: Option<String>,
    /// A plugin reported it (`compile::drop_plugin_duplicates` keeps one of each in the build).
    pub plugin: bool,
    /// (file, pos, end, category, text) of each related information (file `None`: none).
    pub related: Vec<(Option<String>, u32, u32, u8, String)>,
}

impl DiskDiagnostic {
    fn to_json(&self) -> Value {
        let related: Vec<Value> =
            self.related.iter().map(|(file, pos, end, category, text)| json!([file, pos, end, category, text])).collect();
        json!([self.file, self.pos, self.end, self.category, self.text, self.source, self.plugin, related])
    }

    fn from_json(value: &Value) -> Option<DiskDiagnostic> {
        let number = |v: &Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
        Some(DiskDiagnostic {
            file: optional_string(value.get(0)?)?,
            pos: number(value.get(1)?)?,
            end: number(value.get(2)?)?,
            category: u8::try_from(value.get(3)?.as_u64()?).ok()?,
            text: value.get(4)?.as_str()?.to_owned(),
            source: optional_string(value.get(5)?)?,
            plugin: value.get(6)?.as_bool()?,
            related: value
                .get(7)?
                .as_array()?
                .iter()
                .map(|r| {
                    Some((
                        optional_string(r.get(0)?)?,
                        number(r.get(1)?)?,
                        number(r.get(2)?)?,
                        u8::try_from(r.get(3)?.as_u64()?).ok()?,
                        r.get(4)?.as_str()?.to_owned(),
                    ))
                })
                .collect::<Option<_>>()?,
        })
    }
}

/// What the plugins keep of one project file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileRecord {
    /// Per active plugin: the part's bytes, `None` when the plugin could not encode it (empty: no parts).
    pub parts: Vec<Option<Vec<u8>>>,
    /// (plugin, namespaced key, value) of each late string its Luau uses.
    pub late: Vec<(usize, String, String)>,
    /// (plugin, name, value) of its `Cx::env` reads.
    pub env: Vec<(usize, String, Option<String>)>,
    /// (plugin, root, pattern, answer) of its `Cx::record_glob` calls.
    pub globs: Vec<(usize, String, String, Vec<String>)>,
    /// Its last transform's diagnostics.
    pub diagnostics: Vec<DiskDiagnostic>,
    /// One of them could not be kept (a diagnostic with a code of its own): the next process transforms it again.
    pub no_replay: bool,
}

/// The incremental state of the last committed build.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskState {
    /// The compiler binary's identity ([`compiler_identity`]).
    pub compiler: String,
    /// (name, version) of each active plugin, in registration order.
    pub plugins: Vec<(String, String)>,
    /// The salt: a change of it transforms every file (the compiler binary, options, tsconfig and Rojo texts, the plugin list and tsconfig entries).
    pub salt: Version,
    /// By path key.
    pub files: FxHashMap<String, FileFacts>,
    /// Plugin artifacts the build wrote (absolute paths).
    pub artifacts: Vec<String>,
    /// By path key: what the plugins keep of each project file.
    pub records: FxHashMap<String, FileRecord>,
    /// What the plugins' `setup` read (sorted, each once).
    pub setup_inputs: Vec<DiskInput>,
    /// The plugin state files the build left, with their bytes' versions (written before this state).
    pub state_files: Vec<StateFile>,
    /// By path key: the build's outputs as it left them.
    pub outputs: Vec<(String, Stamp)>,
}

/// A plugin state file a committed build left on disk (written, or already holding those bytes). Before the next
/// build, one that is missing or changed makes its plugin start from a fresh state, and every file is transformed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateFile {
    /// The plugin's name.
    pub plugin: String,
    /// The absolute path.
    pub path: String,
    /// The version of its bytes ([`Version::of_bytes`]).
    pub version: Version,
}

impl StateFile {
    /// Whether the file still holds what the commit left: not missing, not changed (an unreadable file counts as
    /// changed).
    pub fn intact(&self) -> bool {
        std::fs::read(&self.path).is_ok_and(|bytes| Version::of_bytes(&bytes) == self.version)
    }

    fn to_json(&self) -> Value {
        json!([self.plugin, self.path, self.version.to_hex()])
    }

    fn from_json(value: &Value) -> Option<StateFile> {
        Some(StateFile {
            plugin: value.get(0)?.as_str()?.to_owned(),
            path: value.get(1)?.as_str()?.to_owned(),
            version: Version::from_hex(value.get(2)?.as_str()?)?,
        })
    }
}

/// The compiler's identity: the running executable's path, size and modification time, plus the roblox-ts version
/// it follows. Rebuilding the compiler changes it, so outputs of an older compiler are never kept.
pub fn compiler_identity() -> String {
    let exe = std::env::current_exe().ok();
    let meta = exe.as_ref().and_then(|e| std::fs::metadata(e).ok());
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!(
        "rbxts-rs {} (roblox-ts {}) {} {} {modified}",
        env!("CARGO_PKG_VERSION"),
        crate::shared::constants::COMPILER_VERSION,
        exe.map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
        meta.map_or(0, |m| m.len()),
    )
}

/// Where the state of a project with roblox-ts build info at `build_info` goes.
pub fn state_path(build_info: &str) -> String {
    let base = build_info.strip_suffix(".rbxtsc.tsbuildinfo").or_else(|| build_info.strip_suffix(".tsbuildinfo"));
    format!("{}.rbxts-rs.buildinfo", base.unwrap_or(build_info))
}

fn strings(value: &Value) -> Option<Vec<String>> {
    value.as_array()?.iter().map(|v| v.as_str().map(str::to_owned)).collect()
}

fn optional_string(value: &Value) -> Option<Option<String>> {
    match value {
        Value::Null => Some(None),
        Value::String(s) => Some(Some(s.clone())),
        _ => None,
    }
}

fn index(value: &Value) -> Option<usize> {
    usize::try_from(value.as_u64()?).ok()
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// [`base64_encode`]'s inverse; `None` for anything else.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.len() / 4;
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let padding = chunk.iter().rev().take_while(|&&b| b == b'=').count();
        if padding > 2 || (padding > 0 && index + 1 != chunks) {
            return None;
        }
        let mut n = 0u32;
        for (i, &b) in chunk[..4 - padding].iter().enumerate() {
            let value = BASE64.iter().position(|&c| c == b)? as u32;
            n |= value << (18 - 6 * i);
        }
        for i in 0..3 - padding {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

impl FileRecord {
    fn is_empty(&self) -> bool {
        self.parts.is_empty()
            && self.late.is_empty()
            && self.env.is_empty()
            && self.globs.is_empty()
            && self.diagnostics.is_empty()
            && !self.no_replay
    }

    fn read(file: &Value) -> Option<FileRecord> {
        let list = |name: &str| file.get(name).map_or(Some(&[][..]), |v| v.as_array().map(Vec::as_slice));
        Some(FileRecord {
            parts: list("parts")?
                .iter()
                .map(|p| match p {
                    Value::Null => Some(None),
                    Value::String(s) => base64_decode(s).map(Some),
                    _ => None,
                })
                .collect::<Option<_>>()?,
            late: list("late")?
                .iter()
                .map(|l| Some((index(l.get(0)?)?, l.get(1)?.as_str()?.to_owned(), l.get(2)?.as_str()?.to_owned())))
                .collect::<Option<_>>()?,
            env: list("env")?
                .iter()
                .map(|e| Some((index(e.get(0)?)?, e.get(1)?.as_str()?.to_owned(), optional_string(e.get(2)?)?)))
                .collect::<Option<_>>()?,
            globs: list("globs")?
                .iter()
                .map(|g| {
                    Some((
                        index(g.get(0)?)?,
                        g.get(1)?.as_str()?.to_owned(),
                        g.get(2)?.as_str()?.to_owned(),
                        strings(g.get(3)?)?,
                    ))
                })
                .collect::<Option<_>>()?,
            diagnostics: list("diag")?.iter().map(DiskDiagnostic::from_json).collect::<Option<_>>()?,
            no_replay: file.get("noReplay").is_some_and(|v| v == &Value::Bool(true)),
        })
    }

    fn write(&self, file: &mut Map<String, Value>) {
        if !self.parts.is_empty() {
            let parts = self.parts.iter().map(|p| p.as_deref().map_or(Value::Null, |b| json!(base64_encode(b))));
            file.insert("parts".into(), Value::Array(parts.collect()));
        }
        if !self.late.is_empty() {
            file.insert("late".into(), json!(self.late.iter().map(|(p, k, v)| json!([p, k, v])).collect::<Vec<_>>()));
        }
        if !self.env.is_empty() {
            file.insert("env".into(), json!(self.env.iter().map(|(p, n, v)| json!([p, n, v])).collect::<Vec<_>>()));
        }
        if !self.globs.is_empty() {
            let globs = self.globs.iter().map(|(p, root, pattern, answer)| json!([p, root, pattern, answer]));
            file.insert("globs".into(), Value::Array(globs.collect()));
        }
        if !self.diagnostics.is_empty() {
            file.insert("diag".into(), Value::Array(self.diagnostics.iter().map(DiskDiagnostic::to_json).collect()));
        }
        if self.no_replay {
            file.insert("noReplay".into(), Value::Bool(true));
        }
    }
}

impl DiskState {
    /// Reads the state; `None` when it is missing, unreadable or of another format.
    pub fn read(path: &str) -> Option<DiskState> {
        let text = std::fs::read_to_string(path).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        if value.get("format")?.as_u64()? != FORMAT {
            return None;
        }
        let plugins = value
            .get("plugins")?
            .as_array()?
            .iter()
            .map(|p| Some((p.get(0)?.as_str()?.to_owned(), p.get(1)?.as_str()?.to_owned())))
            .collect::<Option<Vec<_>>>()?;
        let mut files = FxHashMap::default();
        let mut records = FxHashMap::default();
        for (key, file) in value.get("files")?.as_object()? {
            let record = FileRecord::read(file)?;
            if !record.is_empty() {
                records.insert(key.clone(), record);
            }
            files.insert(
                key.clone(),
                FileFacts {
                    file_name: file.get("name")?.as_str()?.to_owned(),
                    version: Version::from_hex(file.get("v")?.as_str()?)?,
                    references: strings(file.get("refs")?)?,
                    augments: strings(file.get("aug")?)?,
                    global: file.get("global")?.as_bool()?,
                },
            );
        }
        Some(DiskState {
            compiler: value.get("compiler")?.as_str()?.to_owned(),
            plugins,
            salt: Version::from_hex(value.get("salt")?.as_str()?)?,
            files,
            artifacts: strings(value.get("artifacts")?)?,
            records,
            setup_inputs: match value.get("setup") {
                Some(inputs) => inputs.as_array()?.iter().map(DiskInput::from_json).collect::<Option<_>>()?,
                None => Vec::new(),
            },
            // optional: a state written before it was recorded checks no state file
            state_files: match value.get("stateFiles") {
                Some(files) => files.as_array()?.iter().map(StateFile::from_json).collect::<Option<_>>()?,
                None => Vec::new(),
            },
            outputs: match value.get("outputs") {
                Some(outputs) => outputs
                    .as_array()?
                    .iter()
                    .map(|o| {
                        let hash = u64::from_str_radix(o.get(2)?.as_str()?, 16).ok()?;
                        let stamp = Stamp { size: o.get(1)?.as_u64()?, hash, modified: 0 };
                        Some((o.get(0)?.as_str()?.to_owned(), stamp))
                    })
                    .collect::<Option<_>>()?,
                None => Vec::new(),
            },
        })
    }

    /// The state as the file's bytes (files sorted, so equal states give equal bytes).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut keys: Vec<&String> = self.files.keys().collect();
        keys.sort();
        let mut files = Map::new();
        for key in keys {
            let f = &self.files[key];
            let mut file = json!({
                "name": f.file_name,
                "v": f.version.to_hex(),
                "refs": f.references,
                "aug": f.augments,
                "global": f.global,
            });
            if let (Some(record), Some(object)) = (self.records.get(key), file.as_object_mut()) {
                record.write(object);
            }
            files.insert(key.clone(), file);
        }
        let mut value = json!({
            "format": FORMAT,
            "compiler": self.compiler,
            "plugins": self.plugins.iter().map(|(n, v)| json!([n, v])).collect::<Vec<_>>(),
            "salt": self.salt.to_hex(),
            "files": Value::Object(files),
            "artifacts": self.artifacts,
        });
        if !self.setup_inputs.is_empty()
            && let Some(object) = value.as_object_mut()
        {
            object.insert("setup".into(), Value::Array(self.setup_inputs.iter().map(DiskInput::to_json).collect()));
        }
        if !self.state_files.is_empty()
            && let Some(object) = value.as_object_mut()
        {
            object.insert("stateFiles".into(), Value::Array(self.state_files.iter().map(StateFile::to_json).collect()));
        }
        if !self.outputs.is_empty()
            && let Some(object) = value.as_object_mut()
        {
            let mut outputs: Vec<&(String, Stamp)> = self.outputs.iter().collect();
            outputs.sort_by(|a, b| a.0.cmp(&b.0));
            // the hash as hex text: 64 bits overflow JSON's safe integers
            let outputs = outputs.iter().map(|(key, stamp)| json!([key, stamp.size, format!("{:016x}", stamp.hash)]));
            object.insert("outputs".into(), Value::Array(outputs.collect()));
        }
        let mut bytes = serde_json::to_vec(&value).unwrap_or_default();
        bytes.push(b'\n');
        bytes
    }

    /// Writes the state (atomically, only if changed).
    pub fn write(&self, path: &str) -> std::io::Result<bool> {
        crate::plugins::lifecycle::write_if_changed(Path::new(path), &self.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_round_trips() {
        let mut files = FxHashMap::default();
        files.insert(
            "e:/p/src/a.ts".to_owned(),
            FileFacts {
                file_name: "E:/p/src/a.ts".into(),
                version: Version::of("x"),
                references: vec!["e:/p/src/b.ts".into(), "?missing".into()],
                augments: vec![],
                global: false,
            },
        );
        let mut records = FxHashMap::default();
        records.insert(
            "e:/p/src/a.ts".to_owned(),
            FileRecord {
                parts: vec![Some(vec![0, 1, 2, 255]), None],
                late: vec![(0, "stamp\u{0}k".into(), "v".into())],
                env: vec![(1, "X".into(), None), (0, "Y".into(), Some("y".into()))],
                globs: vec![(0, "E:/p/assets".into(), "*.txt".into(), vec!["b.txt".into(), "a.txt".into()])],
                diagnostics: vec![
                    DiskDiagnostic {
                        file: Some("E:/p/src/c.d.ts".into()),
                        pos: 3,
                        end: 9,
                        category: 0,
                        text: "a warning\nwith a hint".into(),
                        source: Some("@probe/inputs".into()),
                        plugin: true,
                        related: vec![(Some("E:/p/src/b.ts".into()), 0, 4, 3, "declared here".into()), (None, 0, 0, 3, "x".into())],
                    },
                    DiskDiagnostic {
                        file: None,
                        pos: 0,
                        end: 1,
                        category: 0,
                        text: "roblox-ts's".into(),
                        source: None,
                        plugin: false,
                        related: vec![],
                    },
                ],
                no_replay: false,
            },
        );
        records.insert("e:/p/src/b.ts".to_owned(), FileRecord { no_replay: true, ..FileRecord::default() });
        let b = FileFacts { file_name: "E:/p/src/b.ts".into(), version: Version::of("b"), ..files["e:/p/src/a.ts"].clone() };
        files.insert("e:/p/src/b.ts".to_owned(), b);
        let state = DiskState {
            compiler: "c".into(),
            plugins: vec![("stamp".into(), "0.1.0".into())],
            salt: Version::of("salt"),
            files,
            artifacts: vec!["E:/p/out/x.json".into()],
            records,
            setup_inputs: vec![DiskInput::Env { plugin: "stamp".into(), name: "STAMP_PREFIX".into(), value: None }],
            state_files: vec![StateFile {
                plugin: "stamp".into(),
                path: "E:/p/.stamp-state".into(),
                version: Version::of_bytes(&[0xff, b'1']),
            }],
            outputs: vec![("e:/p/out/a.luau".into(), Stamp { size: 12, hash: 0xfedc_ba98_7654_3210, modified: 0 })],
        };
        let dir = std::env::temp_dir().join(format!("rbxts-rs-state-{}", std::process::id()));
        let path = dir.join("tsconfig.rbxts-rs.buildinfo");
        let path = path.to_string_lossy().into_owned();
        assert!(state.write(&path).unwrap());
        assert!(!state.write(&path).unwrap());
        assert_eq!(DiskState::read(&path), Some(state));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(state_path("E:/p/out/tsconfig.rbxtsc.tsbuildinfo"), "E:/p/out/tsconfig.rbxts-rs.buildinfo");
    }

    #[test]
    fn base64_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0, 255, 128, 7]] {
            let text = base64_encode(bytes);
            assert_eq!(base64_decode(&text).as_deref(), Some(bytes), "{text}");
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_decode("Zm8"), None);
        assert_eq!(base64_decode("Z=8="), None);
        assert_eq!(base64_decode("Zm8=Zm8="), None);
    }
}
