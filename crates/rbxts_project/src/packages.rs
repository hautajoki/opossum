//! package.json reading and module resolution for plugins: which package declares a file (Flamework's
//! `declaring_package`), and where a module specifier resolves from a file (`resolve_module`).

use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tsrs_core::{CompilerOptions, P, ResolutionMode, tspath};
use tsrs_module::{DefaultResolver, ResolutionHost, ResolverOptions, new_resolver};

use crate::nodepath;

/// A package.json.
#[derive(Debug)]
pub struct PackageInfo {
    /// The directory holding package.json (Node spelling).
    pub dir: String,
    /// Its `name`, when that is a string.
    pub name: Option<String>,
    /// Its `version`, when that is a string.
    pub version: Option<String>,
    /// The whole file; `Null` when it is not valid JSON.
    pub json: Value,
}

/// Nearest-package.json lookups, cached per directory. Shared between threads.
#[derive(Debug, Default)]
pub struct PackageJsonCache {
    by_dir: Mutex<HashMap<String, Option<Arc<PackageInfo>>>>,
}

impl PackageJsonCache {
    /// The package.json in exactly `dir`, if there is one.
    pub fn read(&self, dir: &str) -> Option<Arc<PackageInfo>> {
        let dir = nodepath::normalize(dir);
        if let Some(cached) = self.by_dir.lock().unwrap().get(&dir.to_lowercase()) {
            return cached.clone();
        }
        let path = nodepath::join(&[&dir, "package.json"]);
        let info = fs::read_to_string(&path).ok().map(|text| {
            let json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
            let field = |k: &str| json.get(k).and_then(Value::as_str).map(str::to_owned);
            Arc::new(PackageInfo { dir: dir.clone(), name: field("name"), version: field("version"), json })
        });
        self.by_dir.lock().unwrap().insert(dir.to_lowercase(), info.clone());
        info
    }

    /// The nearest package.json at or above the directory of `file_name`.
    pub fn package_of(&self, file_name: &str) -> Option<Arc<PackageInfo>> {
        let mut dir = nodepath::normalize(nodepath::dirname(file_name));
        loop {
            if let Some(info) = self.read(&dir) {
                return Some(info);
            }
            let parent = nodepath::dirname(&dir).to_owned();
            if parent == dir {
                return None;
            }
            dir = parent;
        }
    }
}

struct OsHost {
    cwd: String,
}

impl OsHost {
    /// The host for `cwd`: one per directory for the process (tsrs's resolution host must be `'static`, and this
    /// one keeps no state, so every build's resolvers share it instead of leaking one each).
    fn interned(cwd: &str) -> &'static OsHost {
        static HOSTS: std::sync::Mutex<Option<std::collections::HashMap<String, &'static OsHost>>> =
            std::sync::Mutex::new(None);
        let mut hosts = HOSTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        hosts
            .get_or_insert_with(std::collections::HashMap::new)
            .entry(cwd.to_owned())
            .or_insert_with(|| Box::leak(Box::new(OsHost { cwd: cwd.to_owned() })))
    }
}

impl ResolutionHost for OsHost {
    fn fs(&self) -> &dyn tsrs_vfs::FS {
        tsrs_vfs::osvfs::fs()
    }

    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

/// Where a specifier resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModuleInfo {
    /// The resolved file (TypeScript spelling, forward slashes).
    pub file_name: String,
    /// Resolved through node_modules.
    pub is_external_library_import: bool,
    /// The package's name from its package.json, when resolved into a package.
    pub package_name: Option<String>,
}

/// A TypeScript module resolver with the program's options, for specifiers the program itself did not resolve.
pub struct ModuleResolver {
    resolver: DefaultResolver,
}

impl ModuleResolver {
    /// `cwd` is the directory relative options resolve against (the program's current directory).
    pub fn new(options: P<CompilerOptions>, cwd: &str) -> Self {
        let host = OsHost::interned(&tspath::normalize_path(cwd));
        ModuleResolver { resolver: new_resolver(ResolverOptions::new(host, options)) }
    }

    /// Resolves `specifier` as an import in `containing_file`.
    pub fn resolve(&self, specifier: &str, containing_file: &str, mode: ResolutionMode) -> Option<ResolvedModuleInfo> {
        let (resolved, _) = self.resolver.resolve_module_name(specifier, containing_file, mode, None).ok()?;
        resolved.is_resolved().then(|| ResolvedModuleInfo {
            file_name: resolved.resolved_file_name.to_owned(),
            is_external_library_import: resolved.is_external_library_import,
            package_name: (!resolved.package_id.name.is_empty()).then(|| resolved.package_id.name.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_package_json() {
        let base = std::env::temp_dir().join(format!("rbxts-packages-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("node_modules/@scope/pkg/out/sub")).unwrap();
        fs::write(base.join("package.json"), r#"{"name":"root","version":"1.0.0"}"#).unwrap();
        fs::write(
            base.join("node_modules/@scope/pkg/package.json"),
            "\u{feff}{\"name\":\"@scope/pkg\",\"version\":\"2.0.0\"}",
        )
        .unwrap();
        let cache = PackageJsonCache::default();
        let file = base.join("node_modules/@scope/pkg/out/sub/index.d.ts");
        let info = cache.package_of(&file.to_string_lossy().replace('\\', "/")).unwrap();
        assert_eq!(info.name.as_deref(), Some("@scope/pkg"));
        assert_eq!(info.version.as_deref(), Some("2.0.0"));
        let root = cache.package_of(&base.join("src/a.ts").to_string_lossy()).unwrap();
        assert_eq!(root.name.as_deref(), Some("root"));
        let _ = fs::remove_dir_all(&base);
    }
}
