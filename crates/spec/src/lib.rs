//! Tests and benches (hexagon's successor): the Luau runtime specs run on, the bundle that runs them in a VM, Studio or
//! Open Cloud, the local runner, and the report's aggregation, policy and printing.
//!
//! - [`bundle()`]: one script holding the runtime (`runtime/*.luau`) and one run of the suites a host lists.

mod bundle;

pub use bundle::{CHUNK, Host, Mode, Request, Suite, bundle};
