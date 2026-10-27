use crate::{Code, Module, Realm, Routes};

/// Whether code in `importer` may require a module in `target`: shared code is visible to both sides, and code
/// on one side sees only its own side. `Some(Code::Boundary)` when the import crosses a realm boundary. Type-only
/// imports never become requires, so the compiler checks only value imports.
pub fn boundary(importer: Realm, target: Realm) -> Option<Code> {
    (target != Realm::Shared && target != importer).then_some(Code::Boundary)
}

/// The require string that reaches `target` from `importer`. Within one route target it is relative (`./x`,
/// `../x`, `@self/x` into an init module's own children); across targets it is `@game/Service/...`, except into a
/// client target, whose code runs from a copy in PlayerScripts and so is reachable only relatively from the same
/// target. `Err(Code::Boundary)` when the import is not allowed.
pub fn require(routes: &Routes, importer: &Module, target: &Module) -> Result<String, Code> {
    if let Some(code) = boundary(importer.realm, target.realm) {
        return Err(code);
    }
    if routes.get(importer.route).target == routes.get(target.route).target {
        return Ok(relative(&importer.instance, &target.instance));
    }
    if target.realm == Realm::Client {
        return Err(Code::Boundary);
    }
    Ok(format!("@game/{}", target.instance.join("/")))
}

/// Luau's string require over instances: `./` is the requiring module's parent, each further `../` one level
/// up, and `@self/` the module itself.
fn relative(from: &[String], to: &[String]) -> String {
    if to.len() > from.len() && to.starts_with(from) {
        return format!("@self/{}", to[from.len()..].join("/"));
    }
    let base = &from[..from.len() - 1];
    let mut common = base.iter().zip(to).take_while(|(a, b)| a == b).count();
    if common == to.len() {
        // The target is the parent folder module or one above it: name it from its own parent.
        common -= 1;
    }
    let ups = base.len() - common;
    let up = if ups == 0 { "./".to_owned() } else { "../".repeat(ups) };
    format!("{up}{}", to[common..].join("/"))
}
