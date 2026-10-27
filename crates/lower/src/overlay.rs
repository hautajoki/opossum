//! The file system the program reads through: the disk, plus opossum's in-memory declarations and a rewrite of
//! `@rbxts/compiler-types` so that projects pinned to older compiler-types type-check under TypeScript 7.
//!
//! tsrs resolves the global iteration types (`Iterable`, `IterableIterator`, `AsyncIterable`,
//! `AsyncIterableIterator`, `IteratorObject`) at three type parameters, as TypeScript 5.6+ declares them. Older
//! compiler-types declare them with one (`interface Iterable<T>`), which leaves the checker without an iteration
//! protocol: `for…of` over a Set or Map fails. Merging cannot add type parameters, so the declarations are rewritten
//! in place to `<T, TReturn = any, TNext = any>`, and the `IteratorObject` pair is appended when missing. The `any`
//! defaults keep the extra parameters vacuous: no new diagnostics on code that type-checked before.

use std::time::SystemTime;

use rustc_hash::FxHashMap;
use tsrs_vfs::{Entries, FS, FileInfo};

/// Declarations for the compile-time macros, injected as a root file of every program.
pub const MACROS_D_TS: &str = r#"declare const $env: {
	readonly [envName: string]: string | undefined;
} & ((name: string, fallback: string) => string) &
	((name: string) => string | undefined);

declare function $nameof(item: unknown): string;
declare function $keys<T>(): Array<Extract<keyof T, string>>;
declare function $file(path: string): any;
declare function $git(field: "sha" | "branch" | "tag"): string;
declare function $git(field: "dirty"): boolean;
declare function $git(field: "timestamp"): number;
declare function $buildTime(): string;
declare const $package: {
	readonly name: string;
	readonly version: string;
	readonly [field: string]: any;
};
"#;

const ITERATION_TYPES: [&str; 4] = ["Iterable", "IterableIterator", "AsyncIterable", "AsyncIterableIterator"];

const ITERATOR_OBJECTS: &str = r#"
interface IteratorObject<T, TReturn = any, TNext = any> extends Iterator<T, TReturn, TNext> {
	[Symbol.iterator](): IteratorObject<T, TReturn, TNext>;
}

interface AsyncIteratorObject<T, TReturn = any, TNext = any> extends AsyncIterator<T, TReturn, TNext> {
	[Symbol.asyncIterator](): AsyncIteratorObject<T, TReturn, TNext>;
}
"#;

/// The disk under a set of in-memory files (by absolute, normalized path) and the compiler-types rewrite.
pub struct Overlay {
    pub inner: Box<dyn FS>,
    pub files: FxHashMap<String, String>,
}

impl Overlay {
    fn virtual_dir(&self, path: &str) -> bool {
        let path = path.trim_end_matches('/');
        self.files.keys().any(|f| f.len() > path.len() && f.starts_with(path) && f.as_bytes()[path.len()] == b'/')
    }
}

/// The text of a compiler-types declaration file with the iteration types at three type parameters, and the
/// `IteratorObject` pair added to the file that declares `Iterable` when no file of the package has it.
pub fn rewrite_compiler_types(text: &str) -> String {
    let mut out = text.to_owned();
    let mut declares_iterable = false;
    for name in ITERATION_TYPES {
        let old = format!("interface {name}<T>");
        if out.contains(&old) {
            declares_iterable |= name == "Iterable";
            out = out.replace(&old, &format!("interface {name}<T, TReturn = any, TNext = any>"));
        }
    }
    if declares_iterable && !out.contains("interface IteratorObject<") {
        out.push_str(ITERATOR_OBJECTS);
    }
    out
}

fn is_compiler_types(path: &str) -> bool {
    path.contains("/node_modules/@rbxts/compiler-types/") && path.ends_with(".d.ts")
}

impl FS for Overlay {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        self.files.contains_key(path) || self.inner.file_exists(path)
    }

    fn read_file(&self, path: &str) -> Option<String> {
        if let Some(text) = self.files.get(path) {
            return Some(text.clone());
        }
        let text = self.inner.read_file(path)?;
        Some(if is_compiler_types(path) { rewrite_compiler_types(&text) } else { text })
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), String> {
        self.inner.write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), String> {
        self.inner.append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        self.inner.remove(path)
    }

    fn chtimes(&self, path: &str, a_time: SystemTime, m_time: SystemTime) -> Result<(), String> {
        self.inner.chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.virtual_dir(path) || self.inner.directory_exists(path)
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let mut entries = self.inner.get_accessible_entries(path);
        let dir = path.trim_end_matches('/');
        for file in self.files.keys() {
            if let Some(name) = file.strip_prefix(dir).and_then(|rest| rest.strip_prefix('/'))
                && !name.contains('/')
                && !entries.files.iter().any(|f| f == name)
            {
                entries.files.push(name.to_owned());
            }
        }
        entries
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        // in-memory files have no stat: the source cache parses them again each load (they are small)
        if self.files.contains_key(path) { None } else { self.inner.stat(path) }
    }

    fn realpath(&self, path: &str) -> String {
        if self.files.contains_key(path) { path.to_owned() } else { self.inner.realpath(path) }
    }
}

#[cfg(test)]
mod tests {
    use super::rewrite_compiler_types;

    #[test]
    fn iteration_types_get_three_parameters_once() {
        let text = "interface Iterable<T> {}\ninterface IterableIterator<T> extends Iterator<T> {}\n";
        let once = rewrite_compiler_types(text);
        assert!(once.contains("interface Iterable<T, TReturn = any, TNext = any> {}"));
        assert!(once.contains("interface IterableIterator<T, TReturn = any, TNext = any> extends Iterator<T>"));
        assert!(once.contains("interface IteratorObject<"));
        // idempotent: a rewritten (or already three-parameter) file is left alone
        assert_eq!(rewrite_compiler_types(&once), once);
    }
}
