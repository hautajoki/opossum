//! The TypeScript → Luau transform: a port of roblox-ts master's `src/TSTransformer` (DESIGN §9b), plus the build
//! pipeline of `compileFiles` ([`compile`]).
//!
//! The module layout follows roblox-ts's folders, one Rust module per TypeScript file, and ported functions keep
//! their TypeScript names in snake_case, so the two can be read side by side. Deliberate differences are marked
//! `// rbxts-rs:`.
//!
//! - [`classes`]: [`TransformState`] (one file), [`MultiTransformState`] (the build), [`Prereqs`], the
//!   [`MacroManager`], the diagnostics of a file.
//! - [`nodes`]: `transform_expression` / `transform_statement` dispatch and the node transforms.
//! - [`macros`]: the macro tables (names only in T1; each transform is `None` until ported).
//! - [`util`]: helpers, including the evaluation-order analysis (`util::evaluation`).
//! - [`shared`]: roblox-ts's diagnostics table and constants.
//! - [`build`]: clean, incremental and watch builds ([`build::BuildSession`], DESIGN §14).
//! - [`plugins`]: the compiler side of the plugin seam (`rbxts_plugin`'s backends, dispatch, build lifecycle).
//! - [`project`]: roblox-ts's project build steps around the transform (the include copy).
//!
//! A file with errors is not written; the build carries on and reports every file.
//!
//! [`TransformState`]: classes::transform_state::TransformState
//! [`MultiTransformState`]: classes::multi_transform_state::MultiTransformState
//! [`Prereqs`]: classes::prereqs::Prereqs
//! [`MacroManager`]: classes::macro_manager::MacroManager
#![warn(missing_docs)]

pub mod build;
pub mod classes;
pub mod compile;
pub mod macros;
pub mod nodes;
pub mod plugins;
pub mod project;
pub mod shared;
pub mod util;

pub use build::{BuildSession, BuildSummary};
pub use compile::{
    CompileOptions, CompileReport, DEFAULT_CHECKERS, FileOutcome, MAX_CHECKERS, ProjectOutcome, Timings, compile_project,
    compile_project_with, default_checkers, env_checkers,
};
pub use plugins::{PluginOptions, PluginTimings};
