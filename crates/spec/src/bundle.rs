//! One Luau script holding the spec runtime and one run of it. The same text runs in the VM (local runs), as Studio's
//! `/exec` source and as an Open Cloud task: nothing is written to disk or into the project.

use luau::luau_string;

/// The runtime's modules by name, as their string requires (`./name`) know them.
const RUNTIME: [(&str, &str); 5] = [
    ("expect", include_str!("../runtime/expect.luau")),
    ("json", include_str!("../runtime/json.luau")),
    ("quote", include_str!("../runtime/quote.luau")),
    ("runner", include_str!("../runtime/runner.luau")),
    ("spec", include_str!("../runtime/spec.luau")),
];

/// The chunk name a bundle runs under in the VM. Like `[C]`, it cannot be a project path, so a trace line starting
/// with `[` is never the project's.
pub const CHUNK: &str = "[spec]";

/// Where a bundle runs, which decides how it loads a suite.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Host {
    /// A [`luau::vm::Vm`] rooted at the project: a suite is `__load(label)`.
    Vm,
    /// Studio or Open Cloud: a suite is `require` of the instance at its path from `game`.
    Roblox,
}

/// `test` runs `.spec` suites' tests; `bench` runs `.bench` suites, their tests first as correctness guards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Tests only.
    Test,
    /// Tests, then benches.
    Bench,
}

impl Mode {
    /// The mode's name, as the runtime and reports spell it.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Test => "test",
            Mode::Bench => "bench",
        }
    }
}

/// One suite to run. `label` is its source file relative to the project (the first label of its cases, and in the VM
/// the file loaded); `path` is its instance's names from `game` on Roblox, empty in the VM.
#[derive(Clone, Debug)]
pub struct Suite {
    /// The source file relative to the project.
    pub label: String,
    /// Instance names from `game`, one per element because names may hold dots.
    pub path: Vec<String>,
}

/// One run: the suites, the mode, a case filter (a case-insensitive substring of a case's labels joined by ` / `),
/// and focus: `None` focuses when any selected case has `only`, `Some` is the host's decision for the whole run.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    /// The suites, in the order they run.
    pub suites: &'a [Suite],
    /// What runs.
    pub mode: Mode,
    /// Selects cases by label.
    pub filter: Option<&'a str>,
    /// Keeps only the `only` cases.
    pub focus: Option<bool>,
}

/// The script for `req` on `host`. It returns the run's report as JSON text.
pub fn bundle(host: Host, req: &Request) -> String {
    let mut out =
        String::from("-- opossum's spec runtime and one run of it\nlocal modules: { [string]: (any) -> any } = {}\n");
    for (name, source) in RUNTIME {
        out.push_str(&format!("modules[\"{name}\"] = function(require)\n{source}\nend\n"));
    }
    out.push_str(
        "local loaded: { [string]: any } = {}
local function runtime(name: string): any
\tif loaded[name] == nil then
\t\tloaded[name] = modules[name](function(target: string): any
\t\t\treturn runtime((string.gsub(target, \"^%./\", \"\")))
\t\tend)
\tend
\treturn loaded[name]
end
",
    );
    out.push_str(match host {
        Host::Vm => "local function load(suite: any): any\n\treturn __load(suite.label)\nend\n",
        Host::Roblox => {
            "local function load(suite: any): any
\tlocal instance = game:GetService(suite.path[1])
\tfor index = 2, #suite.path do
\t\tinstance = instance:FindFirstChild(suite.path[index])
\t\t\tor error(`{suite.label} is not in this place ({table.concat(suite.path, \".\")})`, 0)
\tend
\treturn require(instance)
end
"
        }
    });
    let wait = if host == Host::Roblox { ", task.wait" } else { "" };
    out.push_str(&format!("return runtime(\"runner\").run({}, load{wait})\n", request(req)));
    out
}

/// `req` as a Luau table literal.
fn request(req: &Request) -> String {
    let mut out = String::from("{\n\tsuites = {\n");
    for suite in req.suites {
        let path: Vec<String> = suite.path.iter().map(|name| luau_string(name)).collect();
        out.push_str(&format!("\t\t{{ label = {}, path = {{ {} }} }},\n", luau_string(&suite.label), path.join(", ")));
    }
    let focus = req.focus.map_or("nil", |focus| if focus { "true" } else { "false" });
    let filter = req.filter.map_or_else(|| "nil".to_owned(), luau_string);
    out.push_str(&format!("\t}},\n\tmode = \"{}\",\n\tfilter = {filter},\n\tfocus = {focus},\n}}", req.mode.name()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_compile_through_the_gate() {
        let suites = [
            Suite { label: "src/a.spec.ts".into(), path: vec!["ReplicatedStorage".into(), "a.spec".into()] },
            Suite { label: "src/\"b\"\n.spec.luau".into(), path: vec!["ServerScriptService".into(), "b.spec".into()] },
        ];
        let req = Request { suites: &suites, mode: Mode::Bench, filter: Some("x\"y"), focus: None };
        for host in [Host::Roblox, Host::Vm] {
            let source = bundle(host, &req);
            if let Err(error) = luau::gate::compile(&source, 2, 1) {
                panic!("{host:?}: {error:?}\n{source}");
            }
        }
    }

    #[test]
    fn the_runtime_passes_its_own_spec() {
        let suites = [Suite { label: "tests/runtime.local.spec.luau".into(), path: Vec::new() }];
        let req = Request { suites: &suites, mode: Mode::Test, filter: None, focus: None };
        let mut vm = luau::vm::Vm::new(std::path::Path::new(env!("CARGO_MANIFEST_DIR")), true).unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&vm.eval(CHUNK, &bundle(Host::Vm, &req)).unwrap()).unwrap();
        assert!(report["failures"].as_array().is_some_and(Vec::is_empty), "{:#}", report["failures"]);
        assert_eq!((&report["ok"], &report["test_pass"]), (&true.into(), &12.into()));
    }
}
