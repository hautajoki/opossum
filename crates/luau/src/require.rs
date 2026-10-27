//! Where a module lives in the DataModel, and how one module's `require` names another.
//!
//! Spelling, from the requiring module's path and the target's:
//! - inside one route root (`ServerScriptService/server`, …) → relative: `./x` names a sibling, each `../` goes one
//!   instance up, and an init module (a folder that is a module) names its own children `@self/x`;
//! - across roots → `@game/<Service>/...`;
//! - never `@game/StarterPlayer/...`: client code runs from a copy under PlayerScripts, so a client module is only
//!   reachable relatively from its own root.

/// A module's instance path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModulePath {
    /// Instance names from the service down: `["ServerScriptService", "server", "features", "x"]`.
    pub names: Vec<String>,
    /// How many leading names form the route root (`ServerScriptService/server` → 2).
    pub root: usize,
    /// The module is a folder module (`init.luau`, `index.ts`): its children are under it.
    pub init: bool,
}

impl ModulePath {
    fn in_root_of(&self, other: &ModulePath) -> bool {
        self.root == other.root
            && self.root <= self.names.len().min(other.names.len())
            && self.names[..self.root] == other.names[..other.root]
    }
}

/// The string `from` requires `to` with; `None` when `to` cannot be named from `from` (a client module from outside
/// its root).
pub fn spell(from: &ModulePath, to: &ModulePath) -> Option<String> {
    if from.in_root_of(to) {
        return Some(relative(from, to));
    }
    if to.names.first().is_some_and(|s| s == "StarterPlayer") {
        return None;
    }
    Some(format!("@game/{}", to.names.join("/")))
}

fn relative(from: &ModulePath, to: &ModulePath) -> String {
    let own = &from.names[..];
    if from.init && to.names.len() > own.len() && to.names.starts_with(own) {
        return format!("@self/{}", to.names[own.len()..].join("/"));
    }
    // `./` is the requiring module's parent; at least the target's own name stays in the path
    let base = &own[..own.len().saturating_sub(1)];
    let common = base.iter().zip(&to.names).take_while(|(a, b)| a == b).count().min(to.names.len().saturating_sub(1));
    let ups = base.len() - common;
    let rest = to.names[common..].join("/");
    if ups == 0 { format!("./{rest}") } else { format!("{}{rest}", "../".repeat(ups)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, root: usize, init: bool) -> ModulePath {
        ModulePath { names: path.split('/').map(str::to_owned).collect(), root, init }
    }

    #[test]
    fn spellings() {
        let server = |p: &str| at(&format!("ServerScriptService/server/{p}"), 2, false);
        let client = |p: &str| at(&format!("StarterPlayer/StarterPlayerScripts/client/{p}"), 3, false);
        let shared = |p: &str| at(&format!("ReplicatedStorage/shared/{p}"), 2, false);
        assert_eq!(spell(&server("a/b"), &server("a/c")).unwrap(), "./c");
        assert_eq!(spell(&server("a/b"), &server("d/e")).unwrap(), "../d/e");
        assert_eq!(spell(&server("a/b/c"), &server("x")).unwrap(), "../../x");
        // a folder module from its child, and an init module's children
        assert_eq!(spell(&server("a/b"), &server("a")).unwrap(), "../a");
        let init = at("ServerScriptService/server/a", 2, true);
        assert_eq!(spell(&init, &server("a/b")).unwrap(), "@self/b");
        assert_eq!(spell(&init, &server("c")).unwrap(), "./c");
        assert_eq!(spell(&server("a"), &shared("x/y")).unwrap(), "@game/ReplicatedStorage/shared/x/y");
        assert_eq!(spell(&client("a/b"), &client("c")).unwrap(), "../c");
        assert_eq!(spell(&shared("a"), &client("c")), None);
    }
}
