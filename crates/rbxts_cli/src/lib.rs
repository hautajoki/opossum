//! An `rbxtsc`-compatible command line (roblox-ts's `CLI/commands/build.ts`) over `rbxts_transform`, with
//! compiled-in plugins (DESIGN §11.6):
//!
//! ```no_run
//! fn main() -> std::process::ExitCode {
//!     rbxts_cli::Cli::new()
//!         // .plugin(flamework_rs::Flamework::new())
//!         .run()
//! }
//! ```
//!
//! [`Cli::run`] parses roblox-ts's `build` arguments (`-p`, `--verbose`, `--rojo`, `--includePath`, `--type`,
//! `--optimizedLoops`, `--allowCommentDirectives`, `--noInclude`, `--logTruthyChanges`, `--writeOnlyChanged`,
//! `--writeTransformedFiles`, `--luau`) as yargs does, builds on a thread with a 512 MiB stack, and returns
//! roblox-ts's exit codes: 0 when the build succeeded, 1 otherwise. Output follows roblox-ts's CLI: on stdout, every
//! diagnostic as TypeScript's pretty reporter prints it (fatal errors and "Unable to find tsconfig.json!" as roblox-ts
//! text diagnostics), and argument errors as yargs's message alone. `-w` / `--watch` runs watch mode ([`watch`],
//! DESIGN §14), with `--usePolling` and `--events json` (machine-readable build events on stdout).
//!
//! rbxts-rs's own options besides `--timings` and `--events`: `--checkers <n>`, the number of type checkers, each
//! checking and transforming its share of the files on a thread of its own (default
//! [`rbxts_transform::DEFAULT_CHECKERS`], or `RBXTS_CHECKERS`; an invalid `RBXTS_CHECKERS` is a warning). The output
//! is the same for any count, but for one known rare case: with several checkers tsrs (like TypeScript 7) can give a
//! different diagnostic when a base constraint is cut short by the instantiation-depth limit, depending on which
//! checker resolved it first (tsrs `notes/open-history-dependence.md`).
//!
//! `RBXTS_PLUGIN_CLAIMS=all` asks every matching plugin for each claimable node and reports double claims.
#![warn(missing_docs)]

mod pretty;
pub mod watch;
pub mod yargs;

use std::ffi::OsString;
use std::process::ExitCode;

use rbxts_plugin::Plugin;
use rbxts_plugin::erased::{DynPlugin, erase};
use rbxts_project::{ProjectOptionLayer, ProjectType, STACK_SIZE, nodepath};
use rbxts_transform::shared::diagnostics::text_diagnostic;
use rbxts_transform::{CompileOptions, CompileReport, PluginOptions, compile_project_with};
use tsrs_ast::Diagnostic;
use tsrs_compiler::sort_and_deduplicate_diagnostics;
use tsrs_core::P;
use tsrs_diagnostics::Category;

const USAGE: &str = "\
rbxtsc build [options]

Build a project

Options:
  -p, --project                 project path                      [string] [default: \".\"]
      --verbose                 enable verbose logs                            [boolean]
      --noInclude               do not copy include files                      [boolean]
      --logTruthyChanges        logs changes to truthiness evaluation from Lua truthiness rules
                                                                               [boolean]
      --type                    override project type   [choices: \"game\", \"model\", \"package\"]
  -i, --includePath             folder to copy runtime files to                 [string]
      --rojo                    manually select Rojo project file               [string]
  -w, --watch                   enable watch mode                              [boolean]
      --usePolling              use polling for watch mode                     [boolean]
      --events                  machine-readable watch events on stdout   [choices: \"json\"]
      --luau                    emit files with .luau extension                [boolean]
      --timings                 print plugin hook timings                      [boolean]
      --checkers                type checkers (one thread each)    [number] [default: 4]
  -h, --help                    show help
  -v, --version                 show version number";

/// The command line, with the plugins compiled into this binary.
#[derive(Default)]
pub struct Cli {
    plugins: Vec<Box<dyn DynPlugin>>,
}

/// Parsed `build` arguments.
#[derive(Debug, Default, PartialEq)]
pub struct BuildArgs {
    /// `-p` / `--project`: the tsconfig, or the directory holding it.
    pub project: String,
    /// `--verbose`: roblox-ts's verbose log lines, and rbxts-rs's file counts.
    pub verbose: bool,
    /// `--timings`: the time each build phase took.
    pub timings: bool,
    /// `-w` / `--watch`.
    pub watch: bool,
    /// `--usePolling`: poll instead of file system notifications.
    pub use_polling: bool,
    /// `--events json`.
    pub events_json: bool,
    /// `--checkers <n>`.
    pub checkers: Option<usize>,
    /// The project options the command line sets (`--type`, `--includePath`, `--rojo`, …).
    pub options: ProjectOptionLayer,
}

/// What the arguments ask for.
#[derive(Debug, PartialEq)]
pub enum Command {
    /// `rbxtsc build` (or no command): build the project.
    Build(BuildArgs),
    /// `--help`: print the usage.
    Help,
    /// `--version`: print the version.
    Version,
}

/// A path option as roblox-ts reads it: `path.resolve` of a string; an empty string or `false` (`--no-includePath`)
/// means the default; anything else makes roblox-ts throw.
fn path_option(name: &str, value: &yargs::Value, cwd: &str) -> Result<String, String> {
    match value {
        yargs::Value::Str(path) if path.is_empty() => Ok(String::new()),
        yargs::Value::Str(path) => Ok(nodepath::resolve(&[cwd, path])),
        yargs::Value::Bool(false) => Ok(String::new()),
        other => Err(not_a_path(name, other)),
    }
}

/// rbxts-rs: roblox-ts crashes on a path option that is not one string (`path.resolve` throws a TypeError, exit 1);
/// rbxtsc says so instead.
fn not_a_path(name: &str, value: &yargs::Value) -> String {
    format!("--{name} must be given once, as a path (got {}); roblox-ts stops with a TypeError here", value.to_json())
}

/// rbxts-rs: `--checkers <n>`, a whole number from 1 to 256.
fn checkers_option(value: &yargs::Value) -> Result<usize, String> {
    match value {
        yargs::Value::Num(n) if n.fract() == 0.0 && (1.0..=256.0).contains(n) => Ok(*n as usize),
        other => Err(format!("--checkers must be a whole number from 1 to 256 (got {})", other.to_json())),
    }
}

/// Parses roblox-ts's `build` arguments as roblox-ts's yargs does ([`yargs`]) and reads them as roblox-ts does:
/// paths are resolved against `cwd` (`path.resolve`), an empty `--includePath` or `--rojo` means the default, and a
/// repeated `--type` of valid choices is neither a project nor a package (roblox-ts then compares an array), as a model.
/// Errors carry yargs's message.
pub fn parse_args(args: impl IntoIterator<Item = String>, cwd: &str) -> Result<Command, String> {
    let args: Vec<String> = args.into_iter().collect();
    let options = match yargs::parse(&args) {
        yargs::Parsed::Help => return Ok(Command::Help),
        yargs::Parsed::Version => return Ok(Command::Version),
        yargs::Parsed::Fail(message) => return Err(message),
        yargs::Parsed::Options(options) => options,
    };
    let mut out = BuildArgs { project: ".".to_owned(), ..BuildArgs::default() };
    for (name, value) in &options {
        // a boolean is truthy unless false (an object from `--verbose.x` is truthy too)
        let flag = !matches!(value, yargs::Value::Bool(false));
        match name.as_str() {
            "project" => match value {
                yargs::Value::Str(project) => out.project = project.clone(),
                other => return Err(not_a_path("project", other)),
            },
            "includePath" => out.options.include_path = Some(path_option("includePath", value, cwd)?),
            "rojo" => out.options.rojo = Some(path_option("rojo", value, cwd)?),
            "type" => {
                out.options.project_type = Some(match value {
                    yargs::Value::Str(t) => ProjectType::parse(t).expect("yargs checked the choices"),
                    _ => ProjectType::Model,
                })
            }
            "verbose" => out.verbose = flag,
            "timings" => out.timings = flag,
            "noInclude" => out.options.no_include = Some(flag),
            "logTruthyChanges" => out.options.log_truthy_changes = Some(flag),
            "writeOnlyChanged" => out.options.write_only_changed = Some(flag),
            "writeTransformedFiles" => out.options.write_transformed_files = Some(flag),
            "optimizedLoops" => out.options.optimized_loops = Some(flag),
            "allowCommentDirectives" => out.options.allow_comment_directives = Some(flag),
            "luau" => out.options.luau = Some(flag),
            "watch" => out.watch = flag,
            "usePolling" => out.use_polling = flag,
            "checkers" => out.checkers = Some(checkers_option(value)?),
            "events" => match value {
                yargs::Value::Str(events) if events == "json" => out.events_json = true,
                other => {
                    return Err(format!(
                        "Invalid values:\n  Argument: events, Given: {}, Choices: \"json\"",
                        other.to_json()
                    ));
                }
            },
            _ => {}
        }
    }
    Ok(Command::Build(out))
}

impl Cli {
    /// A command line with no plugins.
    pub fn new() -> Cli {
        Cli::default()
    }

    /// Registers a plugin. Plugins are asked in registration order; a plugin with a `tsconfig_transform` is on only
    /// for projects whose tsconfig `plugins` list that transform.
    pub fn plugin<P: Plugin>(mut self, plugin: P) -> Cli {
        self.plugins.push(erase(plugin));
        self
    }

    /// Registers an already erased plugin.
    pub fn erased_plugin(mut self, plugin: Box<dyn DynPlugin>) -> Cli {
        self.plugins.push(plugin);
        self
    }

    /// Runs with the process's arguments.
    pub fn run(self) -> ExitCode {
        let args: Vec<String> =
            std::env::args_os().skip(1).map(|a: OsString| a.to_string_lossy().into_owned()).collect();
        ExitCode::from(self.run_with(args))
    }

    /// Runs with these arguments (without the program name) and returns the exit status: 0 or 1.
    pub fn run_with(self, args: Vec<String>) -> u8 {
        let thread =
            std::thread::Builder::new().name("rbxtsc".into()).stack_size(STACK_SIZE).spawn(move || self.main(args));
        match thread.map(|t| t.join()) {
            Ok(Ok(status)) => status,
            Ok(Err(_)) => {
                eprintln!("rbxtsc: the compiler crashed (a panic outside the per-file transform)");
                1
            }
            Err(e) => {
                eprintln!("rbxtsc: could not start the build thread: {e}");
                1
            }
        }
    }

    fn main(self, args: Vec<String>) -> u8 {
        let cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
        let build = match parse_args(args, &cwd) {
            Ok(Command::Build(build)) => build,
            Ok(Command::Help) => {
                println!("{USAGE}");
                return 0;
            }
            Ok(Command::Version) => {
                println!("{}", rbxts_transform::shared::constants::COMPILER_VERSION);
                return 0;
            }
            Err(message) => {
                // yargs's `.fail()` in roblox-ts's CLI: the message alone, on stdout
                println!("{message}");
                return 1;
            }
        };
        let tsconfig = match rbxts_project::setup::find_tsconfig_path(&build.project, &cwd) {
            Ok(path) => path,
            Err(_) => {
                // a `CLIError`, logged with `LogService.writeLine`: the formatted diagnostic, then "\n"
                let error = text_diagnostic("Unable to find tsconfig.json!", Category::Error);
                println!("{}", format_diagnostic_logged(error, &cwd));
                return 1;
            }
        };
        let options = CompileOptions {
            tsconfig,
            cwd: cwd.clone(),
            project_options: build.options.clone(),
            out_dir_override: None,
            write_partial: false,
            no_write: false,
            checkers: build.checkers,
        };
        if build.watch {
            return watch::run(&self.plugins, &build, options, &cwd);
        }
        let plugin_options = PluginOptions { plugins: &self.plugins, ..PluginOptions::default() };
        let report = compile_project_with(&options, &plugin_options);
        if report.thrown {
            // `DiagnosticError.log`: `formatDiagnostics` of them all, then `LogService.writeLine`'s line feed
            let text: String = report.diagnostics.iter().map(|d| format_diagnostic_logged(*d, &cwd)).collect();
            println!("{text}");
        } else {
            print_report(&report, &cwd, &build);
        }
        u8::from(report.has_errors())
    }
}

/// `ts.sys.newLine`.
const NEW_LINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// One diagnostic as roblox-ts's `ts.createDiagnosticReporter(ts.sys, true)` prints it: TypeScript 5's
/// `formatDiagnosticsWithColorAndContext` ([`pretty`]), ending with a line end, with the OS's line ends and `ts.sys`'s
/// file name rules (case-insensitive on Windows). rbxts-rs: roblox-ts's own diagnostics, ad hoc messages here, show
/// roblox-ts's code (`TS roblox-ts`, from `createTextDiagnostic` and `createDiagnosticWithLocation`) instead of tsrs's
/// `TS-1`; a plugin diagnostic with a source (`Plugin::diagnostic_source`, `PluginDiagnostic::source`) shows that
/// source instead (`TS @flamework-experimental/transformer`).
pub fn format_diagnostic(diagnostic: P<Diagnostic>, cwd: &str) -> String {
    format_with(diagnostic, cwd, cfg!(not(windows)))
}

/// One diagnostic as roblox-ts's `formatDiagnostics` formats it (`DiagnosticError`, `CLIError`): as
/// [`format_diagnostic`], with file names compared as they are spelled.
pub fn format_diagnostic_logged(diagnostic: P<Diagnostic>, cwd: &str) -> String {
    format_with(diagnostic, cwd, true)
}

fn format_with(diagnostic: P<Diagnostic>, cwd: &str, case_sensitive: bool) -> String {
    let code = |diagnostic: P<Diagnostic>| {
        if diagnostic.code() == -1 {
            // a plugin diagnostic with a source shows it in the code slot, as the TS transformers' do (`code: " <name>"`)
            format!(" {}", rbxts_transform::plugins::diagnostic_source(diagnostic).unwrap_or("roblox-ts"))
        } else {
            diagnostic.code().to_string()
        }
    };
    pretty::format_diagnostic(diagnostic, &pretty::Format { cwd, case_sensitive, new_line: NEW_LINE, code: &code })
}

/// Prints the build's outcome: warnings, plugin messages (`--verbose`), diagnostics with TypeScript's pretty
/// reporter, fatal errors, and the plugin timings (`--timings`).
pub fn print_report(report: &CompileReport, cwd: &str, build: &BuildArgs) {
    for warning in &report.warnings {
        println!("\u{1b}[33m{warning}\u{1b}[0m");
    }
    if build.verbose {
        for message in &report.plugin_messages {
            println!("{message}");
        }
    }
    let mut diagnostics = report.diagnostics.clone();
    for file in &report.files {
        diagnostics.extend(file.diagnostics.iter().copied());
    }
    // roblox-ts's build returns `ts.sortAndDeduplicateDiagnostics(diagnostics)`: by file, position, code and message,
    // whatever order the files were checked and transformed in
    // the project graph's errors (thrown before any build) come in the order found: roblox-ts's watcher reports a
    // `DiagnosticError`'s diagnostics as they are
    let diagnostics = if report.thrown { diagnostics } else { sort_and_deduplicate_diagnostics(&diagnostics) };
    // roblox-ts reports its fatal errors (`ProjectError`, `emitResultFailure`) as text diagnostics too
    let fatal = report.fatal.iter().map(|message| text_diagnostic(message, Category::Error));
    for diagnostic in diagnostics.iter().copied().chain(fatal) {
        // `ts.createDiagnosticReporter(ts.sys, true)`: the formatted diagnostic, then a line end
        print!("{}{NEW_LINE}", format_diagnostic(diagnostic, cwd));
    }
    if build.verbose {
        let errors = diagnostics.iter().filter(|d| d.category() == Category::Error).count();
        let written = report.files.iter().filter(|f| f.written).count();
        println!("{} files, {written} written, {errors} errors", report.files.len());
    }
    if build.timings {
        print!("{}", format_plugin_timings(report));
    }
}

/// The plugin hook timings as a table: plugin, hook, calls, milliseconds (hooks that never ran are left out).
pub fn format_plugin_timings(report: &CompileReport) -> String {
    let mut out = String::new();
    for plugin in &report.plugin_timings {
        out.push_str(&format!("plugin {}\n", plugin.name));
        for (hook, time) in &plugin.hooks {
            if time.calls > 0 {
                out.push_str(&format!(
                    "  {:<28} {:>8} calls {:>10.2} ms\n",
                    hook.label(),
                    time.calls,
                    time.time.as_secs_f64() * 1e3
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        parse_args(args.iter().map(|s| s.to_string()), "E:/work")
    }

    #[test]
    fn build_arguments() {
        let Command::Build(build) = parse(&[
            "build",
            "-p",
            "game",
            "--verbose",
            "--optimizedLoops",
            "--allowCommentDirectives=true",
            "--type",
            "package",
            "-i",
            "include",
            "--rojo=default.project.json",
            "--luau=false",
        ])
        .unwrap() else {
            panic!("a build")
        };
        assert_eq!(build.project, "game");
        assert!(build.verbose);
        assert_eq!(build.options.optimized_loops, Some(true));
        assert_eq!(build.options.allow_comment_directives, Some(true));
        assert_eq!(build.options.project_type, Some(ProjectType::Package));
        assert_eq!(build.options.include_path.as_deref(), Some(nodepath::resolve(&["E:/work", "include"]).as_str()));
        assert_eq!(
            build.options.rojo.as_deref(),
            Some(nodepath::resolve(&["E:/work", "default.project.json"]).as_str())
        );
        assert_eq!(build.options.luau, Some(false));
    }

    #[test]
    fn defaults_help_and_errors() {
        assert_eq!(parse(&[]).unwrap(), Command::Build(BuildArgs { project: ".".into(), ..BuildArgs::default() }));
        assert_eq!(parse(&["--help"]).unwrap(), Command::Help);
        assert_eq!(parse(&["-v"]).unwrap(), Command::Version);
        let Command::Build(watch) = parse(&["-w", "--usePolling", "--events", "json"]).unwrap() else { panic!() };
        assert!(watch.watch && watch.use_polling && watch.events_json);
        assert!(parse(&["--events", "xml"]).unwrap_err().contains("Choices"));
        assert!(parse(&["-w", "false"]).is_ok_and(|c| matches!(c, Command::Build(b) if !b.watch)));
        assert!(parse(&["--type", "place"]).unwrap_err().contains("Given: \"place\", Choices"));
        assert!(parse(&["--type"]).unwrap_err().contains("Given: true, Choices"));
        assert_eq!(parse(&["--bogus=1"]).unwrap_err(), "Unknown argument: bogus");
        assert_eq!(parse(&["-x"]).unwrap_err(), "Unknown argument: x");
        assert_eq!(parse(&["build", "build"]).unwrap_err(), "Unknown argument: build");
    }

    /// yargs's value rules, as roblox-ts's CLI parses and reads them (checked against roblox-ts's own yargs).
    #[test]
    fn yargs_values() {
        let build = |args: &[&str]| match parse(args).unwrap() {
            Command::Build(build) => build,
            other => panic!("a build, not {other:?}"),
        };
        // a string option without a value takes its default (`-p`: ".") or is empty
        assert_eq!(build(&["-p"]).project, ".");
        let b = build(&["-p", "--noInclude"]);
        assert_eq!((b.project.as_str(), b.options.no_include), (".", Some(true)));
        // an empty include path or Rojo file, or a negated one, means the default
        assert_eq!(build(&["-i"]).options.include_path.as_deref(), Some(""));
        assert_eq!(build(&["--includePath="]).options.include_path.as_deref(), Some(""));
        assert_eq!(build(&["--no-i"]).options.include_path.as_deref(), Some(""));
        assert_eq!(build(&["--rojo", "--verbose"]).options.rojo.as_deref(), Some(""));
        // booleans: a following true/false, `=value` (anything but "true" is false), `--no-<name>`
        assert_eq!(build(&["--noInclude", "false"]).options.no_include, Some(false));
        assert_eq!(build(&["--noInclude=0"]).options.no_include, Some(false));
        assert_eq!(build(&["--no-noInclude"]).options.no_include, Some(false));
        assert_eq!(build(&["--luau=yes"]).options.luau, Some(false));
        assert!(!build(&["--verbose=maybe"]).verbose);
        assert!(build(&["--verbose", "true"]).verbose);
        // kebab-case forms of the options
        assert_eq!(build(&["--optimized-loops"]).options.optimized_loops, Some(true));
        let b = build(&["--include-path", "include"]);
        assert_eq!(b.options.include_path.as_deref(), Some(nodepath::resolve(&["E:/work", "include"]).as_str()));
        assert_eq!(build(&["--no-no-include"]).options.no_include, Some(false));
        // a repeated --type of valid choices is neither a project nor a package
        assert_eq!(build(&["--type", "game", "--type", "game"]).options.project_type, Some(ProjectType::Model));
        // yargs's messages
        assert_eq!(parse(&["--no-include"]).unwrap_err(), "Unknown argument: include");
        assert_eq!(parse(&["--foo-bar"]).unwrap_err(), "Unknown arguments: foo-bar, fooBar");
        assert_eq!(parse(&["--verbose", "1"]).unwrap_err(), "Unknown argument: 1");
        assert!(parse(&["--usePolling"]).unwrap_err().starts_with("Missing dependent arguments:"));
        // rbxts-rs's --checkers: a whole number from 1 to 256, in any of yargs's spellings
        assert_eq!(build(&["--checkers", "8"]).checkers, Some(8));
        assert_eq!(build(&["--checkers=2"]).checkers, Some(2));
        assert_eq!(build(&[]).checkers, None);
        for bad in [
            &["--checkers"][..],
            &["--checkers", "0"],
            &["--checkers", "1.5"],
            &["--checkers", "many"],
            &["--checkers=257"],
        ] {
            assert!(parse(bad).unwrap_err().starts_with("--checkers must be a whole number"), "{bad:?}");
        }
        assert!(USAGE.contains(&format!("[number] [default: {}]", rbxts_transform::DEFAULT_CHECKERS)));
        assert!(parse(&["-p", "a", "-p", "b"]).unwrap_err().contains("TypeError"));
        assert_eq!(parse(&["x", "help"]).unwrap(), Command::Help);
    }
}
