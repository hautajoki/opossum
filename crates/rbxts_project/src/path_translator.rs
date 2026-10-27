//! Port of `@roblox-ts/path-translator` 1.1.1: maps source paths to output paths and back.

use crate::nodepath;

const TS_EXT: &str = ".ts";
const TSX_EXT: &str = ".tsx";
const D_EXT: &str = ".d";
const DTS_EXT: &str = ".d.ts";
const TRANSFORMED_EXT: &str = ".transformed";
const LUA_EXT: &str = ".lua";
const LUAU_EXT: &str = ".luau";
const INDEX_NAME: &str = "index";
const INIT_NAME: &str = "init";

/// A path split as the package splits it: the directory, the name up to the first dot, then every `.ext`.
struct PathInfo {
    dir_name: String,
    file_name: String,
    exts: Vec<String>,
}

impl PathInfo {
    fn from(file_path: &str) -> PathInfo {
        let dir_name = nodepath::dirname(file_path).to_owned();
        // `filePath.slice(dirName.length + sep.length)`: one character after the directory, even when the directory
        // is a root that already ends with a separator (the package's behaviour).
        let rest = file_path.get(dir_name.len()..).unwrap_or("");
        let rest = rest.char_indices().nth(1).map_or("", |(i, _)| &rest[i..]);
        let mut parts = rest.split('.');
        let file_name = parts.next().unwrap_or("").to_owned();
        let exts = parts.map(|p| format!(".{p}")).collect();
        PathInfo { dir_name, file_name, exts }
    }

    fn exts_peek(&self, depth: usize) -> Option<&str> {
        self.exts.len().checked_sub(depth + 1).map(|i| self.exts[i].as_str())
    }

    fn join(&self) -> String {
        let mut name = self.file_name.clone();
        for ext in &self.exts {
            name.push_str(ext);
        }
        nodepath::join(&[&self.dir_name, &name])
    }
}

/// Maps between a project's source tree (`root_dir`) and its output tree (`out_dir`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathTranslator {
    /// The source root: the common ancestor of the common source directory (`rootDir`, or TypeScript's computed one) and the `rootDirs`.
    pub root_dir: String,
    /// The output root: `outDir`.
    pub out_dir: String,
    /// Where the TypeScript build info is written, if anywhere (recorded, not used by the mappings).
    pub build_info_output_path: Option<String>,
    /// Declaration files are emitted, so `.d.ts` outputs map back to inputs too.
    pub declaration: bool,
    /// `.luau` outputs rather than `.lua`.
    pub use_luau_extension: bool,
}

impl PathTranslator {
    /// A translator for these roots. `build_info_output_path` is roblox-ts's own build info of an incremental build (`<tsBuildInfoFile>.rbxtsc.tsbuildinfo`).
    pub fn new(
        root_dir: String,
        out_dir: String,
        build_info_output_path: Option<String>,
        declaration: bool,
        use_luau_extension: bool,
    ) -> PathTranslator {
        PathTranslator { root_dir, out_dir, build_info_output_path, declaration, use_luau_extension }
    }

    fn luau_ext(&self) -> &'static str {
        if self.use_luau_extension { LUAU_EXT } else { LUA_EXT }
    }

    fn relative(from: &str, to: &str, info: &PathInfo) -> String {
        nodepath::join(&[to, &nodepath::relative(from, &info.join())])
    }

    fn to_out(&self, info: &PathInfo) -> String {
        Self::relative(&self.root_dir, &self.out_dir, info)
    }

    fn is_ts_not_dts(info: &PathInfo) -> bool {
        matches!(info.exts_peek(0), Some(TS_EXT | TSX_EXT)) && info.exts_peek(1) != Some(D_EXT)
    }

    /// `getOutputPath`: `.ts(x)` (not `.d.ts`) → `.lua(u)`, `index` → `init`, `rootDir/*` → `outDir/*`.
    pub fn get_output_path(&self, file_path: &str) -> String {
        let mut info = PathInfo::from(file_path);
        if Self::is_ts_not_dts(&info) {
            info.exts.pop();
            if info.file_name == INDEX_NAME {
                info.file_name = INIT_NAME.into();
            }
            info.exts.push(self.luau_ext().into());
        }
        self.to_out(&info)
    }

    /// `getOutputDeclarationPath`: `.ts(x)` (not `.d.ts`) → `.d.ts`, `rootDir/*` → `outDir/*`.
    pub fn get_output_declaration_path(&self, file_path: &str) -> String {
        let mut info = PathInfo::from(file_path);
        if Self::is_ts_not_dts(&info) {
            info.exts.pop();
            info.exts.push(DTS_EXT.into());
        }
        self.to_out(&info)
    }

    /// `getOutputTransformedPath`: `.ts(x)` → `.transformed.ts(x)` (before `.d` for declarations).
    pub fn get_output_transformed_path(&self, file_path: &str) -> String {
        let mut info = PathInfo::from(file_path);
        let len = info.exts.len();
        // `splice(length - n, 0, x)` with a negative start counts from the end and clamps at 0.
        let at = if info.exts_peek(1) == Some(D_EXT) { len.saturating_sub(2) } else { len.saturating_sub(1) };
        info.exts.insert(at, TRANSFORMED_EXT.into());
        self.to_out(&info)
    }

    /// `getInputPaths`: the source files an output may come from, most likely first (`.ts`, `.tsx`, then
    /// `index.*` for `init.*`, then declaration sources, then the output path itself mapped back).
    pub fn get_input_paths(&self, file_path: &str) -> Vec<String> {
        let back = |info: &PathInfo| Self::relative(&self.out_dir, &self.root_dir, info);
        let mut possible = Vec::new();
        let mut info = PathInfo::from(file_path);

        // index.*.lua(u) cannot come from a .ts file
        if info.exts_peek(0) == Some(self.luau_ext()) && info.file_name != INDEX_NAME {
            info.exts.pop();
            for ext in [TS_EXT, TSX_EXT] {
                info.exts.push(ext.into());
                possible.push(back(&info));
                info.exts.pop();
            }
            if info.file_name == INIT_NAME {
                let original = std::mem::replace(&mut info.file_name, INDEX_NAME.into());
                for ext in [TS_EXT, TSX_EXT] {
                    info.exts.push(ext.into());
                    possible.push(back(&info));
                    info.exts.pop();
                }
                info.file_name = original;
            }
            info.exts.push(self.luau_ext().into());
        }

        if self.declaration && matches!(info.exts_peek(0), Some(TS_EXT | TSX_EXT)) && info.exts_peek(1) == Some(D_EXT) {
            let ts_ext = info.exts.pop().unwrap_or_default();
            info.exts.pop();
            for ext in [TS_EXT, TSX_EXT] {
                info.exts.push(ext.into());
                possible.push(back(&info));
                info.exts.pop();
            }
            info.exts.push(D_EXT.into());
            info.exts.push(ts_ext);
        }

        possible.push(back(&info));
        possible
    }

    /// `getImportPath`: the (possibly virtual) output path an import of `file_path` resolves to, for Rojo lookups.
    /// `.d.ts(x)` and `.ts(x)` → `.lua(u)`, `index` → `init`. Inside node_modules the output sits next to the
    /// source; elsewhere it is mapped into `outDir`.
    pub fn get_import_path(&self, file_path: &str, is_node_module: bool) -> String {
        let mut info = PathInfo::from(file_path);
        if matches!(info.exts_peek(0), Some(TS_EXT | TSX_EXT)) {
            info.exts.pop();
            if info.exts_peek(0) == Some(D_EXT) {
                info.exts.pop();
            }
            if info.file_name == INDEX_NAME {
                info.file_name = INIT_NAME.into();
            }
            info.exts.push(self.luau_ext().into());
        }
        if is_node_module { info.join() } else { self.to_out(&info) }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn translator(declaration: bool, luau: bool) -> PathTranslator {
        PathTranslator::new("E:\\p\\src\\".into(), "E:\\p\\out".into(), None, declaration, luau)
    }

    #[test]
    fn output_paths() {
        let t = translator(true, true);
        assert_eq!(t.get_output_path("E:/p/src/a/index.ts"), "E:\\p\\out\\a\\init.luau");
        assert_eq!(t.get_output_path("E:/p/src/a/b.server.tsx"), "E:\\p\\out\\a\\b.server.luau");
        assert_eq!(t.get_output_path("E:/p/src/a/b.d.ts"), "E:\\p\\out\\a\\b.d.ts");
        assert_eq!(t.get_output_path("E:/p/src/data.json"), "E:\\p\\out\\data.json");
        assert_eq!(t.get_output_declaration_path("E:/p/src/a/index.ts"), "E:\\p\\out\\a\\index.d.ts");
        assert_eq!(t.get_output_transformed_path("E:/p/src/a.ts"), "E:\\p\\out\\a.transformed.ts");
        assert_eq!(t.get_output_transformed_path("E:/p/src/a.d.ts"), "E:\\p\\out\\a.transformed.d.ts");
        assert_eq!(translator(false, false).get_output_path("E:/p/src/x.ts"), "E:\\p\\out\\x.lua");
    }

    #[test]
    fn input_paths() {
        let t = translator(true, true);
        assert_eq!(
            t.get_input_paths("E:\\p\\out\\a\\init.luau"),
            [
                "E:\\p\\src\\a\\init.ts",
                "E:\\p\\src\\a\\init.tsx",
                "E:\\p\\src\\a\\index.ts",
                "E:\\p\\src\\a\\index.tsx",
                "E:\\p\\src\\a\\init.luau"
            ]
        );
        assert_eq!(
            t.get_input_paths("E:\\p\\out\\b.d.ts"),
            ["E:\\p\\src\\b.ts", "E:\\p\\src\\b.tsx", "E:\\p\\src\\b.d.ts"]
        );
        assert_eq!(translator(false, true).get_input_paths("E:\\p\\out\\b.d.ts"), ["E:\\p\\src\\b.d.ts"]);
        assert_eq!(t.get_input_paths("E:\\p\\out\\index.luau"), ["E:\\p\\src\\index.luau"]);
    }

    #[test]
    fn import_paths() {
        let t = translator(false, true);
        assert_eq!(t.get_import_path("E:/p/src/a/index.d.ts", false), "E:\\p\\out\\a\\init.luau");
        assert_eq!(
            t.get_import_path("E:/p/node_modules/@rbxts/x/out/index.d.ts", true),
            "E:\\p\\node_modules\\@rbxts\\x\\out\\init.luau"
        );
    }
}
