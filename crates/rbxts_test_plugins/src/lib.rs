//! Test plugins for the compiler side of the plugin seam (DESIGN §11), registered by `rbxts_transform`'s plugin
//! tests and by `rbxts-bench build --plugin <name>`.
//!
//! - [`stamp`]: the `rbxts_plugin` crate's own test plugin (`crates/rbxts_plugin/tests/support/stamp.rs`, included
//!   by path so there is one source): JSDoc-tagged calls become strings or late strings, with a tsconfig entry, an
//!   env input, an artifact and a state file.
//! - [`probe`]: exercises every compiler primitive S1 offers: calls by symbol, by receiver and by callee marker,
//!   `new` and decorator claims, `compile_call` slots, prereqs, hoists, captures, imports and import elision, the
//!   class hook, the by-kind fallbacks, late strings, artifacts, and (in its failure modes) the phase guards,
//!   panics and the contracts the compiler enforces.
//! - [`identity`]: claims calls, accesses (`property_key`), assignments or most expression kinds, and lowers them as
//!   roblox-ts would (`compile_expr`, `compile_call`, `compile_binary`, `compile_object_literal`): its output must
//!   equal a build without plugins.
//! - [`decline`]: `decline-all`, offered everything (every call, access, assignment, class, expression and statement
//!   kind) and declining it all: its output must equal a build without plugins; `decline-audit` also reports what it
//!   was not offered; `inert`, active with no interest, the baseline of a plugin build.
//! - [`inputs`]: `probe-inputs`, every build input and persistence method of DESIGN §14 (environment, globs,
//!   recorded finish reads, `remove_empty_dir`, encoded file parts, a diagnostic source).
//! - [`transformer`]: claims a tsconfig transform and does nothing (the `--writeTransformedFiles` fixture).
//! - [`scan`]: a `Shared` that depends on whether the plugin's own state file exists, as Flamework's candidate scan's;
//!   `scan-reader`, one that depends on the bytes of `scan`'s state file.
#![warn(missing_docs)]

// `rbxts_plugin`'s own file, documented there
#[allow(missing_docs)]
#[path = "../../rbxts_plugin/tests/support/stamp.rs"]
pub mod stamp;

pub mod decline;
pub mod identity;
pub mod inputs;
pub mod probe;
pub mod scan;
pub mod transformer;

use rbxts_plugin::erased::{DynPlugin, erase};

/// The plugin names `by_name` knows, for usage messages.
pub const NAMES: &[&str] = &[
    "stamp",
    "probe",
    "probe-b",
    "probe-panic",
    "probe-refuse",
    "probe-unresolved",
    "probe-fail",
    "probe-err",
    "probe-artifact-outside",
    "probe-artifact-sneaky",
    "probe-setup-panic",
    "probe-finish-panic",
    "probe-classself",
    "probe-touched",
    "probe-exprcall",
    "identity-expr",
    "identity-call",
    "identity-key",
    "identity-assign",
    "identity-fallback",
    "decline-all",
    "decline-audit",
    "inert",
    "probe-inputs",
    "scan",
    "scan-reader",
    "transformer",
];

/// A test plugin by name (`rbxts-bench build --plugin <name>`).
pub fn by_name(name: &str) -> Option<Box<dyn DynPlugin>> {
    use probe::{Mode, Probe};
    Some(match name {
        "stamp" => erase(stamp::Stamp),
        "probe" => erase(Probe::new("probe", Mode::Normal)),
        "probe-b" => erase(Probe::new("probe-b", Mode::Normal)),
        "probe-panic" => erase(Probe::new("probe", Mode::Panic)),
        "probe-refuse" => erase(Probe::new("probe", Mode::Refuse)),
        "probe-unresolved" => erase(Probe::new("probe", Mode::Unresolved)),
        "probe-fail" => erase(Probe::new("probe", Mode::Fail)),
        "probe-err" => erase(Probe::new("probe", Mode::ErrWithoutError)),
        "probe-artifact-outside" => erase(Probe::new("probe", Mode::ArtifactOutside)),
        "probe-artifact-sneaky" => erase(Probe::new("probe", Mode::ArtifactSneaky)),
        "probe-setup-panic" => erase(Probe::new("probe", Mode::SetupPanic)),
        "probe-finish-panic" => erase(Probe::new("probe", Mode::FinishPanic)),
        "probe-classself" => erase(Probe::new("probe", Mode::ClassSelf)),
        "probe-touched" => erase(Probe::new("probe-touched", Mode::DeclineTouched)),
        "probe-exprcall" => erase(Probe::new("probe-exprcall", Mode::ExprCall)),
        "identity-expr" => erase(identity::Identity(identity::IdentityMode::Expr)),
        "identity-call" => erase(identity::Identity(identity::IdentityMode::Call)),
        "identity-key" => erase(identity::Identity(identity::IdentityMode::Key)),
        "identity-assign" => erase(identity::Identity(identity::IdentityMode::Assign)),
        "identity-fallback" => erase(identity::Identity(identity::IdentityMode::Fallback)),
        "decline-all" => erase(decline::DeclineAll { audit: false, inert: false }),
        "decline-audit" => erase(decline::DeclineAll { audit: true, inert: false }),
        "inert" => erase(decline::DeclineAll { audit: false, inert: true }),
        "probe-inputs" => erase(inputs::InputsProbe),
        "scan" => erase(scan::Scan),
        "scan-reader" => erase(scan::ScanReader),
        "transformer" => erase(transformer::Transformer),
        _ => return None,
    })
}
