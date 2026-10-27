//! Which files a build transforms (DESIGN §14 "Invalidation", census §6.3).
//!
//! Every program file gets a [`FileFacts`]: its text version, the files it references, the modules it augments and
//! whether it affects the global scope. Against the facts of the last committed build:
//! - **roblox-ts's rule:** the seeds are the files that are new, whose version changed, or whose set of referenced
//!   files changed (a deleted file's importers included), and the closure is everything that reaches a seed through
//!   the reversed references, transitively (`getChangedFilePaths`, with no signature pruning). References are every
//!   resolved import (type-only imports, `export … from` and `import()` types included), triple-slash references,
//!   type reference directives and the file's module augmentations;
//! - **TypeScript's global rule:** a seed, deleted file or file the closure reaches that affects the global scope
//!   (a script file, a `declare global`, a global `.d.ts`) or declares an ambient module invalidates every file
//!   (roblox-ts misses this, census S1). Reached files count because a global declaration whose types come from a
//!   changed module changes every file's view of the globals (TypeScript's `handleDtsMayChangeOfGlobalScope`);
//! - **case-only renames:** a file whose path key is unchanged but whose spelling changed is a seed (its output's
//!   name changes);
//! - **augmentation edges:** a file in the closure that augments module X brings in every file that reaches X (census
//!   S2), so an edit that only changes an augmentation's types re-transforms X's importers.
//!
//! `assumeChangesOnlyAffectDirectDependencies` is refused at setup (census S3).

use std::collections::VecDeque;
use std::hash::Hasher;

use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use tsrs_ast::SourceFile;
use tsrs_compiler::Program;
use tsrs_core::P;
use tsrs_core::tspath;

/// A file's text version: two independent 64-bit hashes and the length, so that an accidental collision is out of
/// reach (a collision would leave an output stale).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// The version of `text`.
    pub fn of(text: &str) -> Version {
        Version::of_bytes(text.as_bytes())
    }

    /// The version of a file's bytes (a plugin state file need not be UTF-8).
    pub fn of_bytes(bytes: &[u8]) -> Version {
        // FNV-1a, 64-bit
        let mut fnv: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            fnv ^= u64::from(b);
            fnv = fnv.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut fx = FxHasher::default();
        fx.write(bytes);
        Version(fnv, fx.finish(), bytes.len() as u64)
    }

    /// Hex text, for the incremental state file.
    pub fn to_hex(self) -> String {
        format!("{:016x}{:016x}{:x}", self.0, self.1, self.2)
    }

    /// Parses [`Version::to_hex`].
    pub fn from_hex(text: &str) -> Option<Version> {
        if text.len() < 33 || !text.is_ascii() {
            return None;
        }
        Some(Version(
            u64::from_str_radix(&text[..16], 16).ok()?,
            u64::from_str_radix(&text[16..32], 16).ok()?,
            u64::from_str_radix(&text[32..], 16).ok()?,
        ))
    }
}

/// What the invalidation rules need to know of one program file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    /// The file name as the program spells it.
    pub file_name: String,
    /// The version of its text.
    pub version: Version,
    /// The keys ([`FileFacts`] map keys: the program's `Path`s) of the files it references, sorted; unresolved
    /// module names as `?name`, so that a name that starts resolving changes the set.
    pub references: Vec<String>,
    /// The keys of the modules its module augmentations resolve to, sorted.
    pub augments: Vec<String>,
    /// It affects the global scope (TypeScript's `isFileAffectingGlobalScope`) or declares ambient modules.
    pub global: bool,
}

/// TypeScript's `isFileAffectingGlobalScope` (tsrs `file_affects_global_scope`, which is not public), widened to
/// files that declare an ambient module: their consumers resolve to the ambient module, not to the file, so no
/// reference names it.
fn affects_global_scope(file: P<SourceFile>) -> bool {
    if !file.ambient_module_names().is_empty() {
        return true;
    }
    if file
        .module_augmentations()
        .iter()
        .any(|&augmentation| augmentation.parent().is_some_and(tsrs_ast::is_global_scope_augmentation))
    {
        return true;
    }
    if tsrs_ast::is_external_or_common_js_module(file) || tsrs_ast::is_json_source_file(file) {
        return false;
    }
    file.statements().nodes().iter().any(|&stmt| !tsrs_ast::is_module_with_string_literal_name(stmt))
}

/// The program file a module resolution names: for a source of a referenced project (TypeScript project references),
/// the declaration it emits, which the program holds in its place (TypeScript's output redirect), else the resolved
/// file itself. `redirects`: the program has project references (else nothing is looked up).
pub(crate) fn program_file_of<'a>(program: &Program, redirects: bool, resolved: &'a str) -> std::borrow::Cow<'a, str> {
    if redirects {
        let redirect = program.get_parse_file_redirect(resolved);
        if !redirect.is_empty() {
            return std::borrow::Cow::Owned(redirect);
        }
    }
    std::borrow::Cow::Borrowed(resolved)
}

/// A path as a key of the facts map: the program's canonical `Path` spelling.
pub fn path_key(program: &Program, file_name: &str) -> String {
    tspath::to_path(file_name, program.get_current_directory(), program.use_case_sensitive_file_names()).to_string()
}

/// The facts of every program file, in program order, keyed by path. `versions` caches versions by file object
/// across the programs of one process (an unchanged file is the same object, so its text is not hashed again).
pub fn program_facts(
    program: &'static Program,
    versions: &mut FxHashMap<P<SourceFile>, Version>,
) -> Vec<(String, FileFacts)> {
    let resolved_modules = program.get_resolved_modules();
    let type_references = program.get_resolved_type_reference_directives();
    let case_sensitive = program.use_case_sensitive_file_names();
    let cwd = program.get_current_directory();
    let redirects = !program.get_resolved_project_references().is_empty();
    let key = |name: &str| tspath::to_path(&program_file_of(program, redirects, name), cwd, case_sensitive).to_string();
    let mut facts = Vec::with_capacity(program.source_files().len());
    for &file in program.source_files() {
        let version = *versions.entry(file).or_insert_with(|| Version::of(file.text()));
        let mut references = Vec::new();
        let mut augments = Vec::new();
        let augmentation_names: FxHashSet<&str> = file
            .module_augmentations()
            .iter()
            .filter(|&&name| tsrs_ast::is_string_literal(name))
            .map(|name| name.text())
            .collect();
        if let Some(resolutions) = resolved_modules.get(file.path()) {
            for (cache_key, resolved) in resolutions {
                let reference = if resolved.is_resolved() {
                    key(resolved.resolved_file_name)
                } else {
                    format!("?{}", cache_key.name)
                };
                if augmentation_names.contains(cache_key.name) && resolved.is_resolved() {
                    augments.push(reference.clone());
                }
                references.push(reference);
            }
        }
        let directory = tspath::get_directory_path(file.file_name());
        for reference in file.referenced_files() {
            references.push(tspath::to_path(&reference.file_name, &directory, case_sensitive).to_string());
        }
        if let Some(directives) = type_references.get(file.path()) {
            for (cache_key, resolved) in directives {
                references.push(if resolved.resolved_file_name.is_empty() {
                    format!("?type:{}", cache_key.name)
                } else {
                    key(resolved.resolved_file_name)
                });
            }
        }
        references.sort_unstable();
        references.dedup();
        augments.sort_unstable();
        augments.dedup();
        facts.push((
            file.path().to_string(),
            FileFacts {
                file_name: file.file_name().to_owned(),
                version,
                references,
                augments,
                global: affects_global_scope(file),
            },
        ));
    }
    facts
}

/// What changed between two sets of facts, and what that invalidates.
#[derive(Debug, Default)]
pub struct Invalidation {
    /// Every file is affected (a global-scope rule fired).
    pub all: bool,
    /// The affected files' keys (empty when `all`).
    pub affected: FxHashSet<String>,
    /// File names: changed text, added, deleted (program spelling; deleted ones from the old facts).
    pub changed: Vec<String>,
    /// Files added to the program.
    pub added: Vec<String>,
    /// Files gone from the program.
    pub deleted: Vec<String>,
}

/// Applies the invalidation rules to the old (committed) and new facts. `facts_of` reads the facts of an old entry.
pub fn invalidate<T>(
    old: &FxHashMap<String, T>,
    facts_of: impl Fn(&T) -> &FileFacts,
    new: &[(String, FileFacts)],
) -> Invalidation {
    let mut result = Invalidation::default();
    let mut seeds: Vec<&str> = Vec::new();
    let new_keys: FxHashSet<&str> = new.iter().map(|(k, _)| k.as_str()).collect();
    for (key, facts) in new {
        match old.get(key).map(&facts_of) {
            None => {
                result.added.push(facts.file_name.clone());
                seeds.push(key);
                result.all |= facts.global;
            }
            Some(previous) => {
                let text_changed = previous.version != facts.version;
                // a case-only rename keeps the path key but changes the output's name
                let renamed = previous.file_name != facts.file_name;
                if renamed {
                    result.deleted.push(previous.file_name.clone());
                    result.added.push(facts.file_name.clone());
                } else if text_changed {
                    result.changed.push(facts.file_name.clone());
                }
                if text_changed
                    || renamed
                    || previous.references != facts.references
                    || previous.augments != facts.augments
                {
                    seeds.push(key);
                    result.all |= facts.global || previous.global;
                }
            }
        }
    }
    let mut deleted_augments: Vec<&str> = Vec::new();
    for (key, entry) in old {
        let facts = facts_of(entry);
        if !new_keys.contains(key.as_str()) {
            result.deleted.push(facts.file_name.clone());
            result.all |= facts.global;
            deleted_augments.extend(facts.augments.iter().map(String::as_str));
        }
    }
    result.changed.sort();
    result.added.sort();
    result.deleted.sort();
    if result.all {
        return result;
    }

    // the reversed references, plus the augmentation edges of the old and the new facts
    let mut importers: FxHashMap<&str, Vec<&str>> = FxHashMap::default();
    for (key, facts) in new {
        for reference in &facts.references {
            importers.entry(reference.as_str()).or_default().push(key);
        }
    }
    let mut augments: FxHashMap<&str, Vec<&str>> = FxHashMap::default();
    for (key, facts) in new {
        augments.entry(key).or_default().extend(facts.augments.iter().map(String::as_str));
        if let Some(previous) = old.get(key).map(&facts_of) {
            augments.entry(key).or_default().extend(previous.augments.iter().map(String::as_str));
        }
    }

    // TypeScript's global rule applies to every file the walk reaches (`handleDtsMayChangeOfGlobalScope`): a global
    // script or `declare global` whose types come from a changed file changes every file's view of the globals
    let new_global: FxHashSet<&str> = new.iter().filter(|(_, f)| f.global).map(|(k, _)| k.as_str()).collect();
    let mut queue: VecDeque<&str> = seeds.iter().copied().chain(deleted_augments).collect();
    let mut seen: FxHashSet<&str> = FxHashSet::default();
    while let Some(key) = queue.pop_front() {
        if !seen.insert(key) {
            continue;
        }
        if new_global.contains(key) || old.get(key).map(&facts_of).is_some_and(|f| f.global) {
            result.all = true;
            result.affected.clear();
            return result;
        }
        if let Some(files) = importers.get(key) {
            queue.extend(files.iter().copied());
        }
        if let Some(targets) = augments.get(key) {
            queue.extend(targets.iter().copied());
        }
    }
    result.affected = seen.into_iter().filter(|k| new_keys.contains(k)).map(str::to_owned).collect();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(name: &str, version: u64, references: &[&str], augments: &[&str], global: bool) -> (String, FileFacts) {
        (
            name.to_owned(),
            FileFacts {
                file_name: name.to_owned(),
                version: Version(version, version, 0),
                references: references.iter().map(|s| s.to_string()).collect(),
                augments: augments.iter().map(|s| s.to_string()).collect(),
                global,
            },
        )
    }

    fn affected(inv: &Invalidation) -> Vec<&str> {
        let mut v: Vec<&str> = inv.affected.iter().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn the_closure_follows_reversed_references_transitively() {
        // D → B → C, E unrelated
        let old: FxHashMap<String, FileFacts> = [
            facts("c", 1, &[], &[], false),
            facts("b", 1, &["c"], &[], false),
            facts("d", 1, &["b"], &[], false),
            facts("e", 1, &[], &[], false),
        ]
        .into_iter()
        .collect();
        let new = vec![
            facts("c", 2, &[], &[], false),
            facts("b", 1, &["c"], &[], false),
            facts("d", 1, &["b"], &[], false),
            facts("e", 1, &[], &[], false),
        ];
        let inv = invalidate(&old, |f| f, &new);
        assert!(!inv.all);
        assert_eq!(affected(&inv), ["b", "c", "d"]);
        assert_eq!(inv.changed, ["c"]);
    }

    #[test]
    fn global_files_and_augmentations() {
        let old: FxHashMap<String, FileFacts> = [
            facts("g", 1, &[], &[], true),
            facts("b", 1, &[], &[], false),
            facts("m", 1, &["b"], &["b"], false),
            facts("d", 1, &["b"], &[], false),
            facts("x", 1, &[], &[], false),
        ]
        .into_iter()
        .collect();
        // a global file changed: everything
        let mut new: Vec<(String, FileFacts)> = old.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        new.iter_mut().find(|(k, _)| k == "g").unwrap().1.version = Version(2, 2, 0);
        assert!(invalidate(&old, |f| f, &new).all);
        // the augmentation in m changed: b and d too, not x
        let mut new: Vec<(String, FileFacts)> = old.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        new.iter_mut().find(|(k, _)| k == "m").unwrap().1.version = Version(2, 2, 0);
        let inv = invalidate(&old, |f| f, &new);
        assert!(!inv.all);
        assert_eq!(affected(&inv), ["b", "d", "m"]);
        // a deleted importer: only the files that referenced it changed (none here), the deletion is listed
        let new: Vec<(String, FileFacts)> =
            old.iter().filter(|(k, _)| k.as_str() != "d").map(|(k, v)| (k.clone(), v.clone())).collect();
        let inv = invalidate(&old, |f| f, &new);
        assert_eq!(inv.deleted, ["d"]);
        assert!(inv.affected.is_empty());
    }

    #[test]
    fn a_global_file_reached_through_the_closure_invalidates_everything() {
        // gl (global) references inner2 through an import() type; useHop uses gl's interface without an import
        let old: FxHashMap<String, FileFacts> = [
            facts("inner2", 1, &[], &[], false),
            facts("gl", 1, &["inner2"], &[], true),
            facts("useHop", 1, &[], &[], false),
        ]
        .into_iter()
        .collect();
        let mut new: Vec<(String, FileFacts)> = old.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        new.iter_mut().find(|(k, _)| k == "inner2").unwrap().1.version = Version(2, 2, 0);
        assert!(invalidate(&old, |f| f, &new).all);
    }

    #[test]
    fn a_case_only_rename_is_a_seed() {
        let old: FxHashMap<String, FileFacts> = [facts("usehub", 1, &[], &[], false)].into_iter().collect();
        let mut new: Vec<(String, FileFacts)> = old.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        new[0].1.file_name = "UseHub".into();
        let inv = invalidate(&old, |f| f, &new);
        assert_eq!(affected(&inv), ["usehub"]);
        assert_eq!(
            (inv.deleted.as_slice(), inv.added.as_slice()),
            (&["usehub".to_owned()][..], &["UseHub".to_owned()][..])
        );
    }

    #[test]
    fn versions_round_trip() {
        let v = Version::of("local x = 1");
        assert_eq!(Version::from_hex(&v.to_hex()), Some(v));
        assert_ne!(Version::of("a"), Version::of("b"));
    }
}
