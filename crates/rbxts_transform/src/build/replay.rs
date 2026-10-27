//! A project file's transform diagnostics kept in the incremental state ([`DiskDiagnostic`]) and made again in a
//! new process for a file it does not transform, so an incremental build reports what a clean build would.
//!
//! Only roblox-ts's and plugins' diagnostics are kept: ad hoc messages (code -1) without a message chain. A file
//! with any other diagnostic is marked so the next process transforms it again ([`to_disk`]'s answer).

use std::sync::{Mutex, PoisonError};

use rustc_hash::FxHashMap;
use tsrs_ast::{Diagnostic, DiagnosticExt, SourceFile, new_diagnostic};
use tsrs_compiler::Program;
use tsrs_core::{P, TextRange};
use tsrs_diagnostics::{Category, new_ad_hoc_message};

use super::state::DiskDiagnostic;

fn category_number(category: Category) -> u8 {
    match category {
        Category::Warning => 0,
        Category::Error => 1,
        Category::Suggestion => 2,
        Category::Message => 3,
    }
}

fn category_of(number: u8) -> Category {
    match number {
        0 => Category::Warning,
        2 => Category::Suggestion,
        3 => Category::Message,
        _ => Category::Error,
    }
}

/// A kept diagnostic's span and text, `None` for one that can't be kept (a code of its own, a message chain).
fn plain(d: P<Diagnostic>) -> Option<(u32, u32, String)> {
    if d.code() != -1 || !d.message_chain().is_empty() {
        return None;
    }
    let loc = d.loc()?;
    let pos = loc.pos().expect_source("a transform diagnostic has a source position");
    Some((pos, pos + loc.len()?, d.localize()))
}

/// `owner`'s diagnostics as the incremental state keeps them (`plugin`: which of them plugins reported), and whether
/// one of them could not be kept.
pub fn to_disk(diagnostics: &[P<Diagnostic>], plugin: &[P<Diagnostic>], owner: P<SourceFile>) -> (Vec<DiskDiagnostic>, bool) {
    let owner = owner.get().file_name();
    let mut kept = Vec::with_capacity(diagnostics.len());
    for &d in diagnostics {
        let Some((pos, end, text)) = plain(d) else { return (Vec::new(), true) };
        let mut related = Vec::new();
        for &r in d.related_information() {
            let Some((pos, end, text)) = plain(r) else { return (Vec::new(), true) };
            related.push((r.file().map(|f| f.get().file_name().to_owned()), pos, end, category_number(r.category()), text));
        }
        kept.push(DiskDiagnostic {
            file: d.file().map(|f| f.get().file_name()).filter(|&name| name != owner).map(str::to_owned),
            pos,
            end,
            category: category_number(d.category()),
            text,
            source: crate::plugins::diagnostic_source(d).map(str::to_owned),
            plugin: plugin.contains(&d),
            related,
        });
    }
    (kept, false)
}

/// The sources of replayed plugin diagnostics, by the address of their ad hoc message (as the plugin backend keeps
/// its own diagnostics').
static SOURCES: Mutex<Option<FxHashMap<usize, &'static str>>> = Mutex::new(None);

/// The source of a diagnostic [`from_disk`] made, `None` for any other.
pub fn source(diagnostic: P<Diagnostic>) -> Option<&'static str> {
    let address = std::ptr::from_ref(diagnostic.message()?) as usize;
    SOURCES.lock().unwrap_or_else(PoisonError::into_inner).as_ref()?.get(&address).copied()
}

/// A diagnostic `file` kept, made again in `program` (one at another file, and related information, in the program's
/// files by name). `None` when the program no longer has the other file.
pub fn from_disk(kept: &DiskDiagnostic, file: P<SourceFile>, program: &Program) -> Option<P<Diagnostic>> {
    let make = |file: Option<P<SourceFile>>, pos: u32, end: u32, category: u8, text: &str| {
        let d = new_diagnostic(file, TextRange::new(pos, end), new_ad_hoc_message(text), &[]);
        if category_of(category) != Category::Error {
            d.set_category(category_of(category));
        }
        d
    };
    let file = match &kept.file {
        Some(name) => program.get_source_file(name)?,
        None => file,
    };
    let d = make(Some(file), kept.pos, kept.end, kept.category, &kept.text);
    if let Some(source) = &kept.source
        && let Some(message) = d.message()
    {
        let source: &'static str = Box::leak(source.clone().into_boxed_str());
        SOURCES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(FxHashMap::default)
            .insert(std::ptr::from_ref(message) as usize, source);
    }
    if !kept.related.is_empty() {
        // related information at a file the program no longer has: as for the diagnostic's own file, the owner is
        // transformed again rather than reporting it without its file
        let related: Vec<P<Diagnostic>> = kept
            .related
            .iter()
            .map(|(name, pos, end, category, text)| {
                let file = match name {
                    Some(name) => Some(program.get_source_file(name)?),
                    None => None,
                };
                Some(make(file, *pos, *end, *category, text))
            })
            .collect::<Option<_>>()?;
        d.set_related_info(&related);
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::from_disk;
    use crate::build::state::DiskDiagnostic;

    /// A kept diagnostic is made again only when the program has every file it names: its own and its related
    /// information's. One at a file the program no longer has makes its owner be transformed again (`None`), never a
    /// diagnostic that lost its file.
    #[test]
    fn related_information_at_a_missing_file_is_not_replayed() {
        let dir = rbxts_test_support::TempDir::new("replay-related");
        std::fs::write(dir.path().join("tsconfig.json"), r#"{ "files": ["a.ts", "b.ts"] }"#).unwrap();
        std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").unwrap();
        std::fs::write(dir.path().join("b.ts"), "export const b = 2;\n").unwrap();
        let cwd = dir.path().to_string_lossy().replace('\\', "/");
        let options =
            rbxts_project::LoadOptions { tsconfig: "tsconfig.json".into(), cwd: cwd.clone(), checkers: 1, lazy_dts: false, regions: false };
        let Ok(project) = rbxts_project::load(&options) else { panic!("the project loads") };
        let program = project.program;
        let a = program.get_source_file(&format!("{cwd}/a.ts")).expect("a.ts");
        let kept = |related_file: &str| DiskDiagnostic {
            file: None,
            pos: 0,
            end: 6,
            category: 0,
            text: "kept".into(),
            source: None,
            plugin: false,
            related: vec![(Some(format!("{cwd}/{related_file}")), 0, 6, 3, "related".into())],
        };
        let replayed = from_disk(&kept("b.ts"), a, program).expect("both files are there");
        let related = replayed.related_information();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].file().map(|f| f.get().file_name().to_owned()), Some(format!("{cwd}/b.ts")));
        assert!(from_disk(&kept("gone.ts"), a, program).is_none(), "the related file is gone");
    }
}
