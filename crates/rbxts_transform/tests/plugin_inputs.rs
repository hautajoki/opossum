//! The plugins' recorded build inputs and file-part persistence (DESIGN §14), with the `probe-inputs` test plugin
//! (`rbxts_test_plugins::inputs`) on copies of `tests/watch_fixtures/basic`:
//! - watch: a finish glob, listing, existence check and file read answering differently run `finish` again (and an
//!   equal answer is no change); a file's glob transforms that file again; a setup glob runs setup again;
//!   `remove_empty_dir` (and its refusals) and the removal of the directories stale artifacts leave empty;
//!   diagnostic sources;
//! - incremental builds in new sessions (as in a new process): parts come from the incremental state, so only the
//!   changed files are transformed; a cached file whose late string changes value is transformed in a second run;
//!   a setup glob answering differently transforms every file;
//! - the W1 harness with `probe-inputs` (`inputs.script.json`), as watch rebuilds and as incremental builds: the
//!   output equals a seeded clean build after every step.
//!
//! Like `tests/watch.rs`, the fixture links a `node_modules` (`RBXTS_RS_FIXTURE_NODE_MODULES`, else the roblox-ts
//! tests project's); without one the tests are skipped.

#![cfg(windows)]

use std::path::{Path, PathBuf};

use rbxts_plugin::BuildKind;
use rbxts_plugin::erased::DynPlugin;
use rbxts_project::{ProjectOptionLayer, STACK_SIZE};
use rbxts_transform::build::parity::{self, ParityOptions};
use rbxts_transform::{BuildSession, CompileOptions, CompileReport, PluginOptions};

fn node_modules() -> Option<PathBuf> {
    let path = rbxts_test_support::node_modules().unwrap_or_default();
    path.join("@rbxts").join("compiler-types").is_dir().then_some(path)
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/watch_fixtures")
}

fn big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new().stack_size(STACK_SIZE).spawn_scoped(scope, f).unwrap().join().unwrap()
    })
}

fn plugins() -> Vec<Box<dyn DynPlugin>> {
    vec![rbxts_test_plugins::by_name("probe-inputs").unwrap()]
}

/// A copy of the basic fixture with a linked node_modules and the probe's directories; removed on drop.
struct Project(PathBuf);

impl Project {
    fn new(label: &str, incremental: bool) -> Option<Project> {
        let Some(node_modules) = node_modules() else {
            rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
            return None;
        };
        let root = std::env::temp_dir().join(format!("rbxts-rs-inputs-{label}-{}", std::process::id()));
        parity::remove_tree(&root);
        parity::copy_tree(&fixtures().join("basic"), &root).unwrap();
        parity::junction(&root.join("node_modules"), &node_modules).unwrap();
        let project = Project(root);
        for dir in ["setup-glob", "file-glob", "globs/finish", "finish-dir"] {
            std::fs::create_dir_all(project.0.join(dir)).unwrap();
        }
        project.write("src/shared/marked.ts", "// probe-inputs\nexport const marked = 1;\n");
        project.write("src/shared/globbed.ts", "// probe-inputs probe-glob\nexport const globbed = 1;\n");
        if incremental {
            let tsconfig = project.read("tsconfig.json").unwrap();
            project.write(
                "tsconfig.json",
                &tsconfig.replacen("\"compilerOptions\": {", "\"compilerOptions\": {\n\t\t\"incremental\": true,", 1),
            );
        }
        Some(project)
    }

    fn path(&self, relative: &str) -> String {
        self.0.join(relative).to_string_lossy().replace('\\', "/")
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(relative)).ok()
    }

    fn exists(&self, relative: &str) -> bool {
        self.0.join(relative).exists()
    }

    fn session<'p>(&self, options: &'p PluginOptions<'p>, watch: bool) -> BuildSession<'p> {
        BuildSession::new(
            CompileOptions {
                tsconfig: "tsconfig.json".into(),
                cwd: self.0.to_string_lossy().into_owned(),
                project_options: ProjectOptionLayer::default(),
                out_dir_override: None,
                write_partial: false,
                no_write: false,
                checkers: None,
            },
            options,
            watch,
        )
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        parity::remove_tree(&self.0);
    }
}

/// The relative names of the files a build transformed.
fn transformed(report: &CompileReport) -> Vec<String> {
    let mut names: Vec<String> =
        report.files.iter().map(|f| f.file_name.rsplit('/').next().unwrap_or_default().to_owned()).collect();
    names.sort();
    names
}

fn ok(report: &CompileReport) {
    assert!(!report.has_errors(), "the build failed: {:?}", report.fatal);
}

#[test]
fn watch_rebuilds_follow_the_recorded_inputs() {
    let Some(project) = Project::new("watch", false) else { return };
    let plugins = plugins();
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let mut session = project.session(&options, true);
        let (report, summary) = session.build(&[]);
        ok(&report);
        assert_eq!(summary.kind, Some(BuildKind::Clean));
        let artifact = || project.read("probe-inputs.txt").unwrap_or_default();
        assert!(artifact().contains("finish glob []"), "{}", artifact());
        let marked = project.read("out/shared/marked.luau").unwrap();
        assert!(marked.contains("probe-inputs: env None, glob []") && marked.contains("\"2 marked\""), "{marked}");
        assert!(project.read("out/shared/globbed.luau").unwrap().contains("glob []"));
        let changed = |session: &BuildSession<'_>, relative: &str| {
            session.recorded_inputs_changed(Some(&[project.path(relative)]))
        };

        // a finish glob: a file it does not match changes nothing; one it matches runs finish, and only finish
        project.write("globs/finish/notes.md", "x");
        assert!(!changed(&session, "globs/finish/notes.md"));
        assert!(!session.recorded_inputs_changed(None));
        project.write("globs/finish/a.json", "{}");
        assert!(changed(&session, "globs/finish/a.json"));
        let (report, _) = session.build(&[project.path("globs/finish/a.json")]);
        ok(&report);
        assert!(report.files.is_empty(), "no file is transformed: {:?}", transformed(&report));
        assert!(artifact().contains("finish glob [\"a.json\"]"), "{}", artifact());
        assert!(!session.recorded_inputs_changed(None), "the answers settled");

        // a listing: an entry added writes an artifact two directories down, its removal removes both directories
        project.write("finish-dir/x.txt", "x");
        assert!(changed(&session, "finish-dir/x.txt"));
        ok(&session.build(&[project.path("finish-dir/x.txt")]).0);
        assert_eq!(project.read("probe-out/deep/x.txt.out").as_deref(), Some("x.txt"));
        std::fs::remove_file(project.0.join("finish-dir/x.txt")).unwrap();
        assert!(changed(&session, "finish-dir/x.txt"));
        let (report, _) = session.build(&[project.path("finish-dir/x.txt")]);
        ok(&report);
        assert!(!project.exists("probe-out/deep/x.txt.out"));
        assert!(
            !project.exists("probe-out"),
            "the directories the stale artifact emptied go: {:?}",
            report.artifacts_deleted
        );
        assert!(project.exists("src") && project.exists("finish-dir"));

        // an existence check
        project.write("finish-flag", "");
        assert!(changed(&session, "finish-flag"));
        ok(&session.build(&[project.path("finish-flag")]).0);
        assert!(artifact().contains("flag true"));

        // remove_empty_dir: an empty directory goes at commit; one with a file, a link and its (empty) target stay
        std::fs::create_dir_all(project.0.join("leftover")).unwrap();
        std::fs::create_dir_all(project.0.join("full")).unwrap();
        project.write("full/keep.txt", "k");
        std::fs::create_dir_all(project.0.join("target")).unwrap();
        parity::junction(&project.0.join("linked"), &project.0.join("target")).unwrap();
        let outside = std::env::temp_dir().join(format!("rbxts-rs-inputs-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        parity::junction(&project.0.join("outside"), &outside).unwrap();
        project.write("finish-file.txt", "remove leftover\nremove full\nremove linked\nremove missing\n");
        assert!(changed(&session, "finish-file.txt"));
        ok(&session.build(&[project.path("finish-file.txt")]).0);
        assert!(!project.exists("leftover"));
        assert!(project.exists("full/keep.txt") && project.exists("linked") && project.exists("target"));
        // refused: a source root, a directory inside it, the project directory, outside it (a link leading there
        // too), an output directory
        for refused in ["src", "src/shared", ".", "..", "outside", "out/shared"] {
            project.write("finish-file.txt", &format!("remove {refused}\n"));
            let (report, _) = session.build(&[project.path("finish-file.txt")]);
            assert!(
                report.fatal.iter().any(|f| f.starts_with("plugin `probe-inputs`: remove_empty_dir:")),
                "{refused}: {:?}",
                report.fatal
            );
        }
        assert!(project.exists("src/shared/marked.ts") && project.exists("out/shared/marked.luau"));
        project.write("finish-file.txt", "");
        ok(&session.build(&[project.path("finish-file.txt")]).0);
        // (`remove_dir` removes a junction itself)
        std::fs::remove_dir(project.0.join("linked")).unwrap();
        std::fs::remove_dir(project.0.join("outside")).unwrap();
        assert!(outside.is_dir(), "a link's target is never touched");
        parity::remove_tree(&outside);

        // a file's glob: a match added transforms that file again, and only it; a file it does not match is nothing
        project.write("file-glob/b.md", "x");
        assert!(!changed(&session, "file-glob/b.md"));
        project.write("file-glob/b.txt", "x");
        assert!(changed(&session, "file-glob/b.txt"));
        let (report, _) = session.build(&[project.path("file-glob/b.txt")]);
        ok(&report);
        assert_eq!(transformed(&report), ["globbed.ts"]);
        assert!(project.read("out/shared/globbed.luau").unwrap().contains("glob [\"b.txt\"]"));

        // a setup glob: setup again, every file, its root named as a changed build input
        project.write("setup-glob/s.txt", "x");
        assert!(changed(&session, "setup-glob/s.txt"));
        let (report, summary) = session.build(&[project.path("setup-glob/s.txt")]);
        ok(&report);
        assert!(summary.all);
        assert!(summary.changes.config_inputs.iter().any(|p| p.ends_with("/setup-glob")), "{:?}", summary.changes);
        assert!(artifact().contains("setup glob [\"s.txt\"]"));

        // a third marked file: the late string changes value in the cached ones, which are rendered again
        project.write("src/shared/marked3.ts", "// probe-inputs\nexport const marked3 = 1;\n");
        let (report, summary) = session.build(&[project.path("src/shared/marked3.ts")]);
        ok(&report);
        assert_eq!(transformed(&report), ["marked3.ts"]);
        assert!(summary.rerendered.iter().any(|p| p.ends_with("marked.luau")), "{:?}", summary.rerendered);
        assert!(project.read("out/shared/marked.luau").unwrap().contains("\"3 marked\""));

        // diagnostic sources: the plugin's, and a diagnostic's own
        project.write("src/shared/warned.ts", "// probe-warn probe-own-source\nexport const warned = 1;\n");
        let (report, _) = session.build(&[project.path("src/shared/warned.ts")]);
        ok(&report);
        let sources: Vec<Option<&str>> = report
            .files
            .iter()
            .flat_map(|f| &f.diagnostics)
            .map(|&d| rbxts_transform::plugins::diagnostic_source(d))
            .collect();
        assert_eq!(sources, [Some("@probe/inputs"), Some("@probe/own")]);
    });
}

#[test]
fn incremental_builds_bring_the_encoded_parts() {
    let Some(project) = Project::new("incremental", true) else { return };
    let plugins = plugins();
    big_stack(|| {
        let options =
            PluginOptions { plugins: &plugins, ignore_unknown_tsconfig_plugins: true, ..PluginOptions::default() };
        let build = |touched: &[String]| project.session(&options, false).build(touched);
        let (report, summary) = build(&[]);
        ok(&report);
        assert_eq!(summary.kind, Some(BuildKind::Clean));
        let all = report.files.len();

        // nothing changed: only the file with a glob is transformed (its matcher is not persisted)
        let (report, summary) = build(&[]);
        ok(&report);
        assert_eq!(summary.kind, Some(BuildKind::Incremental));
        assert!(!summary.all);
        assert_eq!(transformed(&report), ["globbed.ts"]);

        // a leaf edited: it (and the glob's file)
        project.write("src/shared/d.ts", &format!("{}\n// edited\n", project.read("src/shared/d.ts").unwrap()));
        let (report, _) = build(&[]);
        ok(&report);
        assert_eq!(transformed(&report), ["d.ts", "globbed.ts"]);
        assert!(project.read("probe-inputs.txt").unwrap().contains("src/shared/marked.ts env None"));

        // a new marked file changes the late string of the cached marked.ts, whose Luau no process kept: the build
        // starts over once, transforming it
        project.write("src/shared/marked2.ts", "// probe-inputs\nexport const marked2 = 1;\n");
        let (report, _) = build(&[]);
        ok(&report);
        assert_eq!(transformed(&report), ["globbed.ts", "marked.ts", "marked2.ts"]);
        assert!(project.read("out/shared/marked.luau").unwrap().contains("\"3 marked\""));

        // a setup glob answering differently: every file
        project.write("setup-glob/s.txt", "x");
        let (report, summary) = build(&[]);
        ok(&report);
        assert!(summary.all);
        assert_eq!(report.files.len(), all + 1);

        // a part the plugin cannot read any more (the bytes in the state edited): transformed again
        let state_path = project.path("tsconfig.rbxts-rs.buildinfo");
        let state = std::fs::read_to_string(&state_path).expect("the incremental state");
        let state = state.replacen("\"parts\":[\"UElQM", "\"parts\":[\"WElQM", 1);
        std::fs::write(&state_path, state).unwrap();
        let (report, _) = build(&[]);
        ok(&report);
        assert_eq!(report.files.len(), 2, "globbed.ts and the file whose part was spoiled: {:?}", transformed(&report));
    });
}

/// The W1 harness with `probe-inputs`: watch rebuilds and incremental builds equal seeded clean builds.
#[test]
fn the_harness_with_recorded_inputs_equals_clean_builds() {
    for incremental in [false, true] {
        let Some(node_modules) = node_modules() else {
            rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
            return;
        };
        let script =
            parity::parse_script(&std::fs::read_to_string(fixtures().join("inputs.script.json")).unwrap()).unwrap();
        let scratch =
            std::env::temp_dir().join(format!("rbxts-rs-inputs-harness-{incremental}-{}", std::process::id()));
        let plugins = plugins();
        let results = big_stack(|| {
            let options = ParityOptions {
                source: fixtures().join("basic"),
                tsconfig: "tsconfig.json".into(),
                copy: [
                    "src",
                    "tsconfig.json",
                    "default.project.json",
                    "package.json",
                    "setup-glob",
                    "file-glob",
                    "globs",
                    "finish-dir",
                    "finish-flag",
                    "finish-file.txt",
                    "leftover",
                ]
                .map(str::to_owned)
                .to_vec(),
                scratch: scratch.clone(),
                project_options: ProjectOptionLayer::default(),
                plugins: &plugins,
                ignore_unknown_plugins: true,
                node_modules: Some(node_modules),
                node_modules_mode: parity::NodeModules::LinkPackages,
                incremental,
                keep_clean_copies: false,
                check_clean: true,
                memory: None,
                checkers: None,
            };
            parity::run(&options, &script)
        })
        .unwrap();
        parity::remove_tree(&scratch);
        for (index, result) in results.iter().enumerate() {
            assert!(
                result.problems.is_empty(),
                "incremental {incremental}, step {index} `{}`: {:#?}",
                result.name,
                result.problems
            );
        }
        assert_eq!(results.len(), script.steps.len() + 1, "every step ran");
    }
}
