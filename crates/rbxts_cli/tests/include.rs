//! The `rbxtsc` binary on copies of `tests/fixtures/include_game`: the runtime include copy (`copyInclude`) and the
//! command line's behaviour and output shape against stock roblox-ts's (exit codes, stdout).
//!
//! The expected outputs were taken from roblox-ts master (68648b0, `node out/CLI/cli.js`) on the same projects.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

const RBXTSC: &str = env!("CARGO_BIN_EXE_rbxtsc");

fn node_modules() -> Option<PathBuf> {
    let path = rbxts_test_support::node_modules().unwrap_or_default();
    if path.join("@rbxts").join("compiler-types").is_dir() {
        Some(path)
    } else {
        rbxts_test_support::skip(module_path!(), "no node_modules with the roblox-ts tests' packages");
        None
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A temporary root holding a copy of the fixture at `game`, its `node_modules` a junction to the shared one.
struct Fixture {
    root: PathBuf,
    game: PathBuf,
}

impl Fixture {
    fn new(name: &str, node_modules: &Path) -> Fixture {
        let root = std::env::temp_dir().join(format!("rbxts-rs-include-{name}-{}", std::process::id()));
        remove_tree(&root);
        let game = root.join("game");
        copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/include_game"), &game);
        // a junction needs no privileges, unlike a directory symlink; mklink wants backslashes
        let status = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(game.join("node_modules"))
            .arg(node_modules.to_string_lossy().replace('/', "\\"))
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "could not link node_modules");
        Fixture { root, game }
    }

    /// Runs `rbxtsc` in `cwd`: (exit code, stdout).
    fn run(&self, cwd: &Path, args: &[&str]) -> (i32, String) {
        let output = Command::new(RBXTSC).args(args).current_dir(cwd).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(output.stderr.is_empty(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
        (output.status.code().unwrap_or(-1), stdout)
    }

    fn build(&self, args: &[&str]) -> (i32, String) {
        self.run(&self.game, args)
    }

    /// The files under `dir` of the copy, relative, sorted.
    fn files(&self, dir: &str) -> Vec<String> {
        let mut files = Vec::new();
        let base = self.game.join(dir);
        let mut stack = vec![base.clone()];
        while let Some(directory) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else { continue };
            for entry in entries.flatten() {
                if entry.file_type().unwrap().is_dir() {
                    stack.push(entry.path());
                } else {
                    files.push(entry.path().strip_prefix(&base).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        files.sort();
        files
    }
}

/// Removes a tree, a junction inside it as a link (never its target).
fn remove_tree(root: &Path) {
    let link = root.join("game").join("node_modules");
    if std::fs::symlink_metadata(&link).is_ok() {
        let _ = std::fs::remove_dir(&link);
    }
    let _ = std::fs::remove_dir_all(root);
}

impl Drop for Fixture {
    fn drop(&mut self) {
        remove_tree(&self.root);
    }
}

fn embedded(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../rbxts_transform/include").join(name)).unwrap()
}

/// A roblox-ts text diagnostic as stock's reporter prints it.
fn text_error(message: &str) -> String {
    format!("\u{1b}[91merror\u{1b}[0m\u{1b}[90m TS roblox-ts: \u{1b}[0m{message}\r\n\r\n")
}

#[test]
fn copies_the_runtime_files_only_when_changed() {
    let Some(node_modules) = node_modules() else { return };
    let fixture = Fixture::new("default", &node_modules);
    // no roblox-ts package anywhere above the project: the embedded files
    assert_eq!(fixture.build(&[]), (0, String::new()));
    assert_eq!(fixture.files("include"), ["Promise.luau", "RuntimeLib.luau"]);
    assert_eq!(std::fs::read(fixture.game.join("include/RuntimeLib.luau")).unwrap(), embedded("RuntimeLib.luau"));
    assert_eq!(std::fs::read(fixture.game.join("include/Promise.luau")).unwrap(), embedded("Promise.luau"));
    assert_eq!(fixture.files("out"), ["server/main.server.luau", "shared/greet.luau"]);

    // unchanged files are not written again
    let runtime = fixture.game.join("include/RuntimeLib.luau");
    let before = std::fs::metadata(&runtime).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(fixture.build(&[]).0, 0);
    assert_eq!(std::fs::metadata(&runtime).unwrap().modified().unwrap(), before);

    // other files in the include folder stay; a `.lua` beside a `.luau` runtime file goes
    std::fs::write(fixture.game.join("include/RuntimeLib.lua"), "stale").unwrap();
    std::fs::write(fixture.game.join("include/keep.json"), "{}").unwrap();
    assert_eq!(fixture.build(&[]).0, 0);
    assert_eq!(fixture.files("include"), ["Promise.luau", "RuntimeLib.luau", "keep.json"]);

    // `--luau false`: `.lua` names, and the `.luau` files go
    assert_eq!(fixture.build(&["--luau", "false"]).0, 0);
    assert_eq!(fixture.files("include"), ["Promise.lua", "RuntimeLib.lua", "keep.json"]);
    assert_eq!(std::fs::read(fixture.game.join("include/RuntimeLib.lua")).unwrap(), embedded("RuntimeLib.luau"));
}

#[test]
fn takes_the_files_of_the_roblox_ts_package_the_project_resolves() {
    let Some(node_modules) = node_modules() else { return };
    let fixture = Fixture::new("package", &node_modules);
    // a roblox-ts package above the project (the project's own node_modules has none), as Node finds it
    let package = fixture.root.join("node_modules/roblox-ts");
    std::fs::create_dir_all(package.join("include/nested")).unwrap();
    std::fs::write(package.join("package.json"), r#"{ "name": "roblox-ts" }"#).unwrap();
    std::fs::write(package.join("include/RuntimeLib.lua"), "-- runtime 3.0.0\r\n").unwrap();
    std::fs::write(package.join("include/Promise.lua"), "-- promise\n").unwrap();
    std::fs::write(package.join("include/nested/extra.luau"), "-- extra\n").unwrap();

    // `-p <dir>` from elsewhere
    let game = fixture.game.to_string_lossy().into_owned();
    assert_eq!(fixture.run(&fixture.root, &["-p", &game]), (0, String::new()));
    assert_eq!(fixture.files("include"), ["Promise.lua", "RuntimeLib.lua", "nested/extra.luau"]);
    assert_eq!(std::fs::read(fixture.game.join("include/RuntimeLib.lua")).unwrap(), b"-- runtime 3.0.0\r\n");
}

#[test]
fn no_copy_for_packages_or_no_include() {
    let Some(node_modules) = node_modules() else { return };
    let fixture = Fixture::new("none", &node_modules);
    assert_eq!(fixture.build(&["--noInclude"]), (0, String::new()));
    assert!(fixture.files("include").is_empty());
    assert_eq!(fixture.files("out"), ["server/main.server.luau", "shared/greet.luau"]);

    assert_eq!(fixture.build(&["--type", "package"]), (0, String::new()));
    assert!(fixture.files("include").is_empty());

    // a scoped package.json name makes a package: no Rojo file needed, no include copy
    std::fs::write(fixture.game.join("package.json"), r#"{ "name": "@scope/include-game", "version": "1.0.0" }"#)
        .unwrap();
    std::fs::remove_file(fixture.game.join("default.project.json")).unwrap();
    std::fs::remove_dir_all(fixture.game.join("out")).unwrap();
    assert_eq!(fixture.build(&[]), (0, String::new()));
    assert!(fixture.files("include").is_empty());
    assert_eq!(fixture.files("out"), ["server/main.server.luau", "shared/greet.luau"]);

    // an explicit type wins over package.json, as in roblox-ts (a model then needs its Rojo file)
    let (code, stdout) = fixture.build(&["--type", "model"]);
    assert_eq!((code, stdout), (1, text_error("Non-package projects must have a Rojo project file!")));
    assert!(fixture.files("include").is_empty(), "a failed build writes nothing (rbxts-rs)");
}

#[test]
fn include_path_and_rojo_options() {
    let Some(node_modules) = node_modules() else { return };
    let fixture = Fixture::new("paths", &node_modules);
    // an include folder the Rojo project does not map: roblox-ts's error, and nothing written
    let (code, stdout) = fixture.build(&["-i", "runtime"]);
    assert_eq!((code, stdout), (1, text_error("Rojo project contained no data for include folder!")));
    assert!(fixture.files("runtime").is_empty() && fixture.files("out").is_empty());

    let project = std::fs::read_to_string(fixture.game.join("default.project.json")).unwrap();
    std::fs::write(
        fixture.game.join("alt.project.json"),
        project.replace("\"$path\": \"include\"", "\"$path\": \"runtime\""),
    )
    .unwrap();
    assert_eq!(fixture.build(&["-i", "runtime", "--rojo", "alt.project.json"]), (0, String::new()));
    assert_eq!(fixture.files("runtime"), ["Promise.luau", "RuntimeLib.luau"]);
    assert!(fixture.files("include").is_empty());
}

#[test]
fn errors_write_nothing_and_print_as_roblox_ts() {
    let Some(node_modules) = node_modules() else { return };
    let fixture = Fixture::new("errors", &node_modules);
    std::fs::write(fixture.game.join("src/shared/bad.ts"), "const x: number = \"nope\";\nexport = x;\n").unwrap();
    let (code, stdout) = fixture.build(&[]);
    assert_eq!(code, 1);
    // TypeScript's pretty reporter: the diagnostic, then a blank line, with CRLF line ends
    assert_eq!(
        stdout,
        "\u{1b}[96msrc/shared/bad.ts\u{1b}[0m:\u{1b}[93m1\u{1b}[0m:\u{1b}[93m7\u{1b}[0m - \u{1b}[91merror\u{1b}[0m\
         \u{1b}[90m TS2322: \u{1b}[0mType 'string' is not assignable to type 'number'.\r\n\r\n\
         \u{1b}[7m1\u{1b}[0m const x: number = \"nope\";\r\n\
         \u{1b}[7m \u{1b}[0m \u{1b}[91m      ~\u{1b}[0m\r\n\r\n"
    );
    // rbxts-rs: nothing is written by a failed build (roblox-ts copies the include folder before compiling)
    assert!(fixture.files("include").is_empty() && fixture.files("out").is_empty());
}

#[test]
fn command_line_parity() {
    let Some(node_modules) = node_modules() else { return };
    let fixture = Fixture::new("cli", &node_modules);
    let tsconfig = fixture.game.join("tsconfig.json").to_string_lossy().into_owned();
    assert_eq!(fixture.run(&fixture.root, &["-p", "game"]), (0, String::new()));
    assert_eq!(fixture.run(&fixture.root, &["build", "-p", &tsconfig]), (0, String::new()));
    assert_eq!(fixture.build(&["--optimizedLoops", "--allowCommentDirectives"]), (0, String::new()));
    // a `-p` that does not exist inside the project finds the project's tsconfig upwards, as `ts.findConfigFile`
    assert_eq!(fixture.build(&["-p", "nonexistent"]), (0, String::new()));
    // `-p` without a value is the working directory
    assert_eq!(fixture.build(&["-p"]), (0, String::new()));

    // no tsconfig.json here or above: a `CLIError`
    let unable = "\u{1b}[91merror\u{1b}[0m\u{1b}[90m TS roblox-ts: \u{1b}[0mUnable to find tsconfig.json!\r\n\n";
    let elsewhere = fixture.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    if !elsewhere.ancestors().any(|dir| dir.join("tsconfig.json").exists()) {
        let missing = elsewhere.join("nested").to_string_lossy().into_owned();
        assert_eq!(fixture.run(&elsewhere, &["-p", &missing]), (1, unable.to_owned()));
    }

    // yargs's errors: the message alone
    assert_eq!(fixture.build(&["--bogus"]), (1, "Unknown argument: bogus\n".to_owned()));
    assert_eq!(fixture.build(&["stray"]), (1, "Unknown argument: stray\n".to_owned()));
    assert_eq!(
        fixture.build(&["--type", "place"]),
        (
            1,
            "Invalid values:\n  Argument: type, Given: \"place\", Choices: \"game\", \"model\", \"package\"\n"
                .to_owned()
        )
    );
}

/// An outDir override (parity runs, the harness) with the include folder inside the outDir: every build writes it
/// under the override and the clean keeps it there, build after build; nothing reaches the project's own outDir.
#[test]
fn override_keeps_include_across_builds() {
    let Some(node_modules) = node_modules() else { return };
    let fixture = Fixture::new("override", &node_modules);
    let tsconfig = std::fs::read_to_string(fixture.game.join("tsconfig.json")).unwrap();
    std::fs::write(
        fixture.game.join("tsconfig.json"),
        tsconfig.replacen(
            "\"compilerOptions\": {",
            "\"rbxts\": { \"includePath\": \"out/include\" },\n\t\"compilerOptions\": {",
            1,
        ),
    )
    .unwrap();
    let project = std::fs::read_to_string(fixture.game.join("default.project.json")).unwrap();
    std::fs::write(
        fixture.game.join("default.project.json"),
        project.replace("\"$path\": \"include\"", "\"$path\": \"out/include\""),
    )
    .unwrap();
    let elsewhere = fixture.root.join("X");
    let cwd = fixture.game.to_string_lossy().into_owned();
    let out_dir = elsewhere.to_string_lossy().into_owned();
    for build in 1..=4 {
        let (cwd, out_dir) = (cwd.clone(), out_dir.clone());
        let report = std::thread::Builder::new()
            .stack_size(rbxts_project::STACK_SIZE)
            .spawn(move || {
                rbxts_transform::compile_project(&rbxts_transform::CompileOptions {
                    tsconfig: "tsconfig.json".into(),
                    cwd,
                    project_options: rbxts_project::ProjectOptionLayer::default(),
                    out_dir_override: Some(out_dir),
                    write_partial: false,
                    no_write: false,
                    checkers: None,
                })
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(!report.has_errors(), "build {build}: {:?}", report.fatal);
        let mut files: Vec<String> = Vec::new();
        let mut stack = vec![elsewhere.clone()];
        while let Some(directory) = stack.pop() {
            for entry in std::fs::read_dir(&directory).unwrap().flatten() {
                if entry.file_type().unwrap().is_dir() {
                    stack.push(entry.path());
                } else {
                    files.push(entry.path().strip_prefix(&elsewhere).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        files.sort();
        assert_eq!(
            files,
            ["include/Promise.luau", "include/RuntimeLib.luau", "server/main.server.luau", "shared/greet.luau"],
            "build {build}"
        );
        assert!(!fixture.game.join("out").exists(), "build {build} wrote into the project's outDir");
    }
}
