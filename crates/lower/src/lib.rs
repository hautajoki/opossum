//! TypeScript to Luau over the tsrs checker: load a project ([`program`]), check it, and lower each file to a Luau
//! tree ([`lower_files`]) whose requires name routed instance paths ([`locate`]). The node lowering keeps roblox-ts's
//! layout (one module per construct under [`nodes`], helpers under [`util`], the macro tables under [`macros`]).
//!
//! A file is lowered on the checker that checked it, one thread per checker ([`groups`]). Lowered trees are plain
//! owned data, rendered by the `luau` crate.

pub mod classes;
pub mod graph;
pub mod groups;
pub mod locate;
pub mod macros;
pub mod nodes;
pub mod options;
pub mod overlay;
pub mod parallel;
pub mod program;
pub mod shared;
pub mod source_cache;
pub mod util;

pub use groups::{Lowered, lower_files};
pub use locate::{Location, Locator, Realm};
pub use options::Options;
pub use program::{LoadOptions, Project, STACK_SIZE, compiler_options, load};
pub use source_cache::SourceFileCache;
