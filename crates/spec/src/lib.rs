//! Tests and benches (hexagon's successor): the Luau runtime specs run on, the bundle that runs them in a VM, Studio or
//! Open Cloud, the local runner, and the report's aggregation, policy and printing.
//!
//! - [`bundle()`]: one script holding the runtime (`runtime/*.luau`) and one run of the suites a host lists.
//! - [`local`]: local runs, one fresh VM per `*.local.spec.luau` file, with global focus and per-file timeouts.
//! - [`report`]: [`aggregate`] and [`policy`], the exit rule every target shares.
//! - [`print()`]: the reporter.

mod bundle;
pub mod local;
mod print;
pub mod report;

pub use bundle::{CHUNK, Host, Mode, Request, Suite, bundle};
pub use local::{LocalOptions, run_local};
pub use print::print;
pub use report::{aggregate, policy};
