//! The contracts the compiler enforces on plugins, checked with the `rogue` plugin against the mock compiler:
//! unresolved late strings, `Err` without an error, phase-refused primitives, diagnostics from
//! `classify_symbol`, and `explain_checker_diagnostic`.

mod support;

use rbxts_plugin::erased::erase;
use rbxts_plugin::ts::Category;
use rbxts_plugin::{CheckerInfo, Cx};
use support::mock::{MockCompiler, MockCx, MockSetup, Phase, fixture, source};
use support::rogue::{Mode, Rogue};

const GOOD: &[&str] = &["main.ts", "other.ts"];

fn errors(diagnostics: &[String]) -> Vec<&str> {
    diagnostics
        .iter()
        .filter(|d| d.starts_with("error"))
        .map(String::as_str)
        .collect()
}

#[test]
fn an_unresolved_late_string_fails_the_build() {
    let plugin = erase(Rogue(Mode::UnresolvedKey));
    let mut compiler = MockCompiler::new();
    let error = compiler
        .build(&*plugin, &mut MockSetup::clean(fixture()), GOOD, 1)
        .err()
        .expect("the key check fails");
    assert_eq!(
        error,
        "plugin `rogue` left the late string `never` unresolved"
    );
    assert!(compiler.committed.is_none() && compiler.kept.is_none());
}

#[test]
fn err_without_a_new_error_is_an_internal_error() {
    let plugin = erase(Rogue(Mode::ErrWithoutError));
    let out = MockCompiler::new()
        .build(&*plugin, &mut MockSetup::clean(fixture()), &["main.ts"], 1)
        .unwrap();
    // the first call records "first" and keeps its Abort; the next returns it again with nothing new recorded
    assert_eq!(
        errors(&out.diagnostics)[..2],
        [
            "error main.ts: first",
            "error main.ts: internal: plugin `rogue` returned Err without reporting an error",
        ]
    );
    assert!(!out.committed);
}

#[test]
fn lowering_primitives_are_refused_in_end_file() {
    let plugin = erase(Rogue(Mode::CompileInEndFile));
    let out = MockCompiler::new()
        .build(&*plugin, &mut MockSetup::clean(fixture()), &["other.ts"], 1)
        .unwrap();
    assert_eq!(
        errors(&out.diagnostics),
        vec!["error other.ts: internal: plugin `rogue` called compile_expr in File"]
    );
    assert!(!out.committed);
}

#[test]
fn diagnostics_from_classify_symbol_are_refused() {
    let plugin = erase(Rogue(Mode::WarnInClassify));
    let one = MockCompiler::new()
        .build(&*plugin, &mut MockSetup::clean(fixture()), GOOD, 1)
        .unwrap();
    assert!(
        errors(&one.diagnostics).iter().all(|d| d.contains(
            "internal: plugin `rogue` reported a diagnostic from classify_symbol: noisy"
        )),
        "{:?}",
        one.diagnostics
    );
    assert!(!one.committed);
    // why it is refused: the number of classifications follows the checker count
    let two = MockCompiler::new()
        .build(&*plugin, &mut MockSetup::clean(fixture()), GOOD, 2)
        .unwrap();
    assert_ne!(
        errors(&one.diagnostics).len(),
        errors(&two.diagnostics).len()
    );
}

#[test]
fn explain_checker_diagnostic_rewrites_a_checker_message() {
    let program = fixture();
    let (index, file) = source(program, "main.ts");
    let plugin = erase(Rogue(Mode::UnresolvedKey));
    let shared = plugin
        .setup(
            &mut rbxts_plugin::SetupCx::new(&mut MockSetup::clean(program)),
            &mut plugin.default_state(),
        )
        .unwrap();
    let instance = plugin.instance(&shared, CheckerInfo { index: 0, count: 1 });
    let mut backend = MockCx::new(program, index, file);
    backend.phase = Phase::Explain;
    let message = tsrs_diagnostics::new_ad_hoc_message(
        "Namespace 'Networking' has no exported member 'removed'.",
    );
    let checker_diagnostic =
        tsrs_ast::new_diagnostic(Some(file), None::<tsrs_core::TextRange>, message, &[]);
    let other = tsrs_ast::new_diagnostic(
        Some(file),
        None::<tsrs_core::TextRange>,
        tsrs_diagnostics::new_ad_hoc_message("unrelated"),
        &[],
    );

    let mut cx = Cx::new(&mut backend);
    let explained = instance
        .explain_checker_diagnostic(&mut cx, checker_diagnostic)
        .expect("explained");
    assert_eq!(explained.category, Category::Error);
    assert_eq!(
        explained.message,
        "src/main.ts: explained: Namespace 'Networking' has no exported member 'removed'."
    );
    assert_eq!(explained.hint.as_deref(), Some("Rebuild the package."));
    assert!(
        instance
            .explain_checker_diagnostic(&mut cx, other)
            .is_none()
    );

    // and in that phase, lowering is refused
    assert!(cx.compile_expr(file.as_node()).is_err());
    assert_eq!(
        backend.errors(),
        vec!["internal: plugin `plugin` called compile_expr in Explain"]
    );
}

#[test]
fn import_elision_requests_follow_the_phase_table() {
    let program = fixture();
    let (index, file) = source(program, "main.ts");
    let alias = file.as_node().statements()[0];
    let mut backend = MockCx::new(program, index, file);
    for phase in [Phase::Claim, Phase::File, Phase::Classify] {
        backend.phase = phase;
        Cx::new(&mut backend).request_import_elision(alias);
    }
    assert_eq!(backend.elision_requests.len(), 2);
    assert_eq!(
        backend.errors(),
        vec!["internal: plugin `plugin` called request_import_elision in Classify"]
    );
}
