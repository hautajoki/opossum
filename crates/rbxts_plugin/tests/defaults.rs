//! The default implementations of the backend methods added for watch and incremental builds (`env`,
//! `record_glob`, `remove_empty_dir`), of the seam additions (`insert_before_statement`, `hoist_spillable_named`,
//! `runtime_member`, `file_identifier_texts`) and of `Plugin::encode_file_part` / `decode_file_part`: a compiler that
//! does not override them builds as before, and these are what it gets.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rbxts_plugin::erased::{ErasedPart, erase};
use rbxts_plugin::ts::CompilerOptions;
use rbxts_plugin::{
    BuildKind, Cx, EntryKind, FileIndex, FinishBackend, FinishCx, GlobMatcher, Plugin, PluginDiagnostic,
    SetupCx, Utf16Pos,
};
use rbxts_project::{PathTranslator, ProjectData, ProjectOptions, ProjectServices, ProjectSetup};
use support::mock::{MockCx, MockFinish, MockSetup, fixture, source};
use support::stamp::{Stamp, StampPart};

/// A matcher that counts its calls and remembers the roots it was given.
#[derive(Clone, Default)]
struct Counting {
    calls: Arc<AtomicUsize>,
    roots: Arc<Mutex<Vec<PathBuf>>>,
}

impl GlobMatcher for Counting {
    fn expand(&self, root: &Path) -> Vec<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.roots.lock().unwrap().push(root.to_owned());
        // in the matcher's own order, which the backend must keep
        vec!["src/b.ts".into(), "src/a.ts".into()]
    }
}

fn expected() -> Vec<String> {
    vec!["src/b.ts".to_owned(), "src/a.ts".to_owned()]
}

/// A variable of this process with a Unicode value, if there is one.
fn some_variable() -> Option<(String, String)> {
    std::env::vars_os().find_map(|(k, v)| {
        let (k, v) = (k.into_string().ok()?, v.into_string().ok()?);
        (!k.is_empty() && !k.contains('=')).then_some((k, v))
    })
}

#[test]
fn cx_env_reads_the_process_environment() {
    let program = fixture();
    let (index, file) = source(program, "main.ts");
    let mut backend = MockCx::new(program, index, file);
    let mut cx = Cx::new(&mut backend);
    if let Some((name, value)) = some_variable() {
        assert_eq!(cx.env(&name), Some(value));
        assert_eq!(cx.env(&name), std::env::var(&name).ok(), "the value SetupCx's compiler sees");
    }
    assert_eq!(cx.env("RBXTS_PLUGIN_DEFAULTS_SURELY_UNSET_7f3a"), None);
    // names the environment cannot hold are unset, not a panic
    assert_eq!(cx.env(""), None);
    assert_eq!(cx.env("A=B"), None);
    assert_eq!(cx.env("A\0B"), None);
    assert!(backend.diagnostics.is_empty(), "nothing is reported");
}

#[test]
fn cx_record_glob_expands_once_and_returns_the_answer_as_it_is() {
    let program = fixture();
    let (index, file) = source(program, "main.ts");
    let mut backend = MockCx::new(program, index, file);
    let matcher = Counting::default();
    let root = PathBuf::from("/project");
    let answer = Cx::new(&mut backend).record_glob(&root, "src/*.ts", matcher.clone());
    assert_eq!(answer, expected());
    assert_eq!(matcher.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*matcher.roots.lock().unwrap(), vec![root]);
    assert!(backend.diagnostics.is_empty());
}

#[test]
fn setup_record_glob_expands_once_and_records_no_input() {
    let mut backend = MockSetup::clean(fixture());
    let matcher = Counting::default();
    let answer = SetupCx::new(&mut backend).record_glob("/project", "src/**", matcher.clone());
    assert_eq!(answer, expected());
    assert_eq!(matcher.calls.load(Ordering::SeqCst), 1);
    assert!(backend.inputs.is_empty(), "the default records nothing");
}

#[test]
fn finish_record_glob_expands_once_and_watches_nothing() {
    let mut backend = MockFinish::default();
    let matcher = Counting::default();
    let answer = FinishCx::new(&mut backend).record_glob("/project", "src/**", matcher.clone());
    assert_eq!(answer, expected());
    assert_eq!(matcher.calls.load(Ordering::SeqCst), 1);
    assert!(backend.watched.is_empty(), "the default records nothing");
}

#[test]
fn closures_are_matchers_and_every_path_affects_them_by_default() {
    let matcher = |root: &Path| vec![root.to_string_lossy().into_owned()];
    assert_eq!(matcher.expand(Path::new("/r")), vec!["/r".to_owned()]);
    assert!(matcher.affected_by("anything/at/all"));
    assert!(Counting::default().affected_by(""));
}

#[test]
fn file_parts_are_not_persistable_by_default() {
    let part = StampPart { build_keys: vec!["k".into()] };
    assert_eq!(Stamp.encode_file_part(&part), None);
    assert!(Stamp.decode_file_part(b"anything").is_none());

    let erased = erase(Stamp);
    let boxed: ErasedPart = Box::new(part);
    assert_eq!(erased.encode_file_part(&boxed), None);
    assert!(erased.decode_file_part(&[]).is_none());
}

// --- remove_empty_dir through the trait, with real project services ---

/// A finish backend with real project services and the defaults for everything this test calls.
struct Services(ProjectServices);

impl FinishBackend for Services {
    fn project(&self) -> &ProjectServices {
        &self.0
    }
    fn build_kind(&self) -> BuildKind {
        BuildKind::Clean
    }
    fn transforms_all_files(&self) -> bool {
        true
    }
    fn resolve_late(&mut self, _: &str, _: String) {}
    fn write_artifact(&mut self, _: &Path, _: Vec<u8>) {}
    fn write_state_file(&mut self, _: &Path, _: Vec<u8>) {}
    fn remove_artifact(&mut self, _: &Path) {}
    fn watch_dir(&mut self, _: &Path) {}
    fn warn_at(&mut self, _: FileIndex, _: Utf16Pos, _: &str, _: Option<&str>) {}
    fn info(&mut self, _: &str) {}
}

/// `<temp>/<name>/{src,out,include}`, removed on drop.
struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let dir = std::env::temp_dir().join(format!("rbxts_plugin-defaults-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["src", "out", "include/flamework"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        Project { dir }
    }

    fn text(&self, relative: &str) -> String {
        let path = if relative.is_empty() { self.dir.clone() } else { self.dir.join(relative) };
        path.to_string_lossy().replace('\\', "/")
    }

    fn services(&self, include_path: String) -> ProjectServices {
        let options = ProjectOptions {
            include_path,
            project_type: Some(rbxts_project::ProjectType::Package),
            ..ProjectOptions::default()
        };
        let data = ProjectData {
            tsconfig_path: self.text("tsconfig.json"),
            project_path: self.text(""),
            is_package: true,
            node_modules_path: self.text("node_modules"),
            options,
            rojo_config_path: None,
            warnings: Vec::new(),
        };
        let translator = PathTranslator::new(self.text("src"), self.text("out"), None, false, false);
        let compiler_options = CompilerOptions { out_dir: self.text("out"), ..CompilerOptions::default() };
        ProjectSetup::new(data, translator, &compiler_options, &[], false)
            .expect("project services")
            .services
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn remove_empty_dir_removes_the_plugins_empty_folder_at_once() {
    let project = Project::new("remove");
    let mut backend = Services(project.services(project.text("include")));
    let mut cx = FinishCx::new(&mut backend);
    assert_eq!(cx.remove_empty_dir(project.dir.join("include/flamework")), Ok(()));
    assert!(!project.dir.join("include/flamework").exists());
    assert!(project.dir.join("include").is_dir());
    // missing now: a no-op
    assert_eq!(cx.remove_empty_dir("include/flamework"), Ok(()));
}

#[test]
fn remove_empty_dir_leaves_a_folder_with_files() {
    let project = Project::new("full");
    std::fs::write(project.dir.join("include/flamework/globs.json"), "{}").unwrap();
    let mut backend = Services(project.services(project.text("include")));
    assert_eq!(FinishCx::new(&mut backend).remove_empty_dir("include/flamework"), Ok(()));
    assert!(project.dir.join("include/flamework/globs.json").is_file());
}

#[test]
fn remove_empty_dir_refuses_with_a_fatal() {
    let project = Project::new("refuse");
    std::fs::create_dir_all(project.dir.join("out/server")).unwrap();
    let mut backend = Services(project.services(project.text("include")));
    let mut cx = FinishCx::new(&mut backend);
    for refused in ["out/server", "out", "include", "src", ".."] {
        let fatal = cx.remove_empty_dir(refused).expect_err(refused);
        assert!(fatal.message.starts_with("remove_empty_dir: "), "{}", fatal.message);
    }
    assert!(project.dir.join("out/server").is_dir());
}

#[test]
fn remove_empty_dir_defaults_an_empty_include_path_to_the_project_include() {
    let project = Project::new("default-include");
    let mut backend = Services(project.services(String::new()));
    let mut cx = FinishCx::new(&mut backend);
    assert!(cx.remove_empty_dir("include").is_err(), "the include root itself");
    assert_eq!(cx.remove_empty_dir("include/flamework"), Ok(()));
    assert!(!project.dir.join("include/flamework").exists());
}

// --- recorded finish reads: the defaults read now ---

#[test]
fn finish_reads_read_the_disk_now() {
    let project = Project::new("reads");
    std::fs::write(project.dir.join("src/b.ts"), "b").unwrap();
    std::fs::write(project.dir.join("src/A.ts"), "a").unwrap();
    std::fs::create_dir_all(project.dir.join("src/sub")).unwrap();
    let mut backend = MockFinish::default();
    let mut cx = FinishCx::new(&mut backend);

    let entries = cx.read_dir(project.dir.join("src")).expect("a directory");
    let listed: Vec<(&str, EntryKind)> = entries.iter().map(|e| (e.name.as_str(), e.kind)).collect();
    // sorted by name, byte order
    assert_eq!(listed, vec![("A.ts", EntryKind::File), ("b.ts", EntryKind::File), ("sub", EntryKind::Directory)]);
    assert_eq!(cx.read_dir(project.dir.join("src/b.ts")), None, "a file is not a directory");
    assert_eq!(cx.read_dir(project.dir.join("missing")), None);

    assert!(cx.exists(project.dir.join("src/b.ts")));
    assert!(cx.exists(project.dir.join("src/sub")));
    assert!(!cx.exists(project.dir.join("src/missing.ts")));

    assert_eq!(cx.read_file(project.dir.join("src/b.ts")).unwrap(), Some("b".to_owned()));
    assert_eq!(cx.read_file(project.dir.join("src/missing.ts")).unwrap(), None);
    assert!(backend.watched.is_empty(), "the defaults record nothing");
}

#[test]
fn finish_reads_resolve_relative_paths_against_the_project() {
    let project = Project::new("relative-reads");
    std::fs::write(project.dir.join("src/x.ts"), "x").unwrap();
    let mut backend = Services(project.services(project.text("include")));
    let mut cx = FinishCx::new(&mut backend);
    assert!(cx.exists("src/x.ts"));
    assert_eq!(cx.read_file("src/x.ts").unwrap(), Some("x".to_owned()));
    assert_eq!(cx.read_dir("src").map(|e| e.len()), Some(1));
}

// --- diagnostic sources ---

#[test]
fn diagnostics_have_no_source_unless_given() {
    assert_eq!(Stamp.diagnostic_source(), None, "the plugin default is None");
    assert_eq!(erase(Stamp).diagnostic_source(), None);
    let program = fixture();
    let (_, file) = source(program, "main.ts");
    let node = file.as_node();
    assert_eq!(PluginDiagnostic::error(node, "e").source, None);
    assert_eq!(PluginDiagnostic::warning(node, "w").source, None);
    let named = PluginDiagnostic::error(node, "e").with_source("@scope/transformer");
    assert_eq!(named.source, Some("@scope/transformer"));
}

#[test]
fn seam_additions_fall_back_to_the_older_primitives() {
    use rbxts_luau::{LateStrings, create, render_expr};
    let program = fixture();
    let (index, file) = source(program, "main.ts");
    let mut backend = MockCx::new(program, index, file);
    let mut cx = Cx::new(&mut backend);
    // insert_before_statement: a root insertion
    cx.insert_before_statement([create::comment(" before")]);
    // hoist_spillable_named: a hoist_spillable with the name as its hint
    let hoisted = cx.hoist_spillable_named(create::number(1.0), "constant_22");
    assert!(matches!(hoisted, rbxts_luau::Expr::TemporaryIdentifier(_)), "{hoisted:?}");
    // runtime_member: the expression alone
    let member = cx.runtime_member("Promise", "is");
    assert_eq!(render_expr(&member, &LateStrings::default()), "TS.Promise.is");
    // synthesized_string needs no backend
    assert_eq!(render_expr(&cx.synthesized_string("é"), &LateStrings::default()), r#""\u{00E9}""#);
    // file_identifier_texts: TypeScript 5's identifier set of the file, sorted
    let texts = cx.file_identifier_texts();
    assert!(texts.contains(&"buildId") && texts.contains(&"here"), "{texts:?}");
    assert!(texts.windows(2).all(|w| w[0] < w[1]), "sorted, no duplicates");
    assert_eq!(backend.hoisted.len(), 2, "the insertion and the hoist");
    assert!(backend.diagnostics.is_empty());
}
