//! Clean, incremental and watch builds (DESIGN §14): the build session and what it decides with.
//!
//! - [`session`]: [`BuildSession`], the lifecycle over one project across builds.
//! - [`checker_groups`]: the transform over the program's checkers, each file on the checker that checked it.
//! - [`graph`]: file versions, references and the invalidation rules (which files a build transforms).
//! - [`outputs`]: assets, the keep set and the clean of stale outputs (roblox-ts's `getProjectOutputs`), and the
//!   parallel write of the Luau outputs.
//! - [`state`]: the incremental state on disk; [`replay`]: the transform diagnostics it keeps, made again.
//! - [`transform_order`]: the order roblox-ts transforms a build's files in (`getChangedFilePaths`).
//! - [`plugin_inputs`]: what plugins record besides setup's file reads (environment, globs, finish reads), and the
//!   directory rules of `remove_empty_dir`.
//! - [`parity`]: the watch-parity harness's driver (DESIGN §14 W1).

pub(crate) mod checker_groups;
pub mod graph;
pub mod outputs;
pub(crate) mod parallel;
pub mod parity;
pub mod plugin_inputs;
pub(crate) mod references;
pub mod replay;
pub mod session;
pub mod state;
pub mod transform_order;

pub use session::{BuildSession, BuildSummary};
