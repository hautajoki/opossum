//! Where each file lives once routed: the caller's answer, which lowering only reads. Imports become requires of
//! the target's instance path, and their realm decides whether the importer may require it at all.

use luau::ModulePath;
use rustc_hash::FxHashMap;

/// Where a module runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Realm {
    Server,
    Client,
    Shared,
}

impl Realm {
    pub fn name(self) -> &'static str {
        match self {
            Realm::Server => "server",
            Realm::Client => "client",
            Realm::Shared => "shared",
        }
    }

    /// Whether code of this realm can require a module of `target`'s realm at run time: shared code runs on both
    /// sides, so it can only require shared modules; server and client code can require their own and shared ones.
    pub fn reaches(self, target: Realm) -> bool {
        target == Realm::Shared || target == self
    }
}

/// A routed module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub module: ModulePath,
    pub realm: Realm,
}

/// The routed location of every source and package file lowering may meet, by file name as the program spells it
/// (absolute, forward slashes). A package's declaration file (`node_modules/@rbxts/x/index.d.ts`) maps to the
/// module it describes.
#[derive(Clone, Debug, Default)]
pub struct Locator {
    pub files: FxHashMap<String, Location>,
    /// The runtime library (`TS` helpers), required by files that use one.
    pub runtime_lib: Option<Location>,
}

impl Locator {
    pub fn get(&self, file_name: &str) -> Option<&Location> {
        self.files.get(file_name)
    }
}
