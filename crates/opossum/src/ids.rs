//! The id modules: `opossum/<set>` for each asset set and `opossum/resources`, as Luau source and TypeScript types,
//! rendered from the lock alone, so a build never reads an asset's bytes. Types are plain `string`/`number`, never
//! literals: a new id then changes only the source, never the types.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use luau::{is_valid_identifier, luau_string};

use crate::assets;
use crate::config::{self, Config, Environment};
use crate::diag::{Diag, Severity};
use crate::editor::{self, Module};
use crate::lock::{Asset, Lock};

/// The project's virtual modules for a build in `environment` (None without `[environments]`): `opossum/<set>` for
/// each asset set from the files discovery `found` and their records in `lock`, `opossum/resources`, and the suite
/// types. With the warnings that say where the ids are not yet what the files and the config hold: `asset.*`,
/// `lock.merged`, `lock.conflict` and `resource.notProvisioned`.
pub fn modules(
    cfg: &Config,
    environment: Option<&Environment>,
    lock: &Lock,
    found: &[assets::Asset],
) -> (Vec<Module>, Vec<Diag>) {
    let root = &cfg.root;
    let mut diags = lock.merge_warnings(root);
    diags.extend(lock.conflicts(root, Severity::Warning));
    let mut modules = Vec::new();
    for (i, set) in cfg.assets.iter().enumerate() {
        let mut values = Vec::new();
        for a in found.iter().filter(|a| a.set == i) {
            let (value, warning) = value(root, &a.file, lock.assets.get(&a.file));
            diags.extend(warning);
            values.push((a.key.clone(), value));
        }
        let (luau, types) = render_set(&set.name, &values);
        modules.push(Module { name: set.name.clone(), types, luau: Some(luau) });
    }
    if let Some(env) = environment {
        let records = lock.universes.get(&env.universe);
        let file = root.join(config::FILE);
        let mut ids = |kind: &str, keys: Vec<&str>| -> Vec<(String, Option<u64>)> {
            let mut out = Vec::new();
            for key in keys {
                let resource = format!("{kind}/{key}");
                let conflict = lock.conflicts.contains_key(&(env.universe, resource.clone()));
                let id = records.and_then(|r| r.get(&resource)).and_then(|r| r.id).filter(|_| !conflict);
                if id.is_none() && !conflict {
                    let message = format!(
                        "{resource} has no id in universe {} yet, so opossum/resources leaves it out; run `opossum \
                         provision`",
                        env.universe
                    );
                    diags.push(Diag::new("resource.notProvisioned", Severity::Warning, Some(&file), message));
                }
                out.push((key.to_owned(), id));
            }
            out
        };
        let passes = ids("pass", cfg.passes.iter().map(|p| p.key.as_str()).collect());
        let products = ids("product", cfg.products.iter().map(|p| p.key.as_str()).collect());
        let badges = ids("badge", cfg.badges.iter().map(|b| b.key.as_str()).collect());
        let thumbnails = cfg.experience.iter().flat_map(|e| e.thumbnails.iter().map(|t| t.key.as_str())).collect();
        let thumbnails = ids("thumbnail", thumbnails);
        let (luau, types) = render_resources(&Resources {
            environment: &env.name,
            universe: env.universe,
            places: &env.places,
            passes: &passes,
            products: &products,
            badges: &badges,
            thumbnails: &thumbnails,
        });
        modules.push(Module { name: "resources".to_owned(), types, luau: Some(luau) });
    }
    modules.push(editor::test_module());
    (modules, diags)
}

/// What a build puts in its set's module for `file`: its id from the lock, or `""` before its first upload, with the
/// warning that says why it is not yet what the files hold.
pub fn value(root: &Path, file: &str, record: Option<&Asset>) -> (String, Option<Diag>) {
    let warn = |code: &str, message: String| Some(Diag::new(code, Severity::Warning, Some(&root.join(file)), message));
    match record {
        Some(Asset { id: Some(id), next, .. }) => {
            let pending = next.as_ref().and_then(|_| {
                warn("asset.pending", "an upload of this file is pending; `opossum upload` finishes it".into())
            });
            (format!("rbxassetid://{id}"), pending)
        }
        _ => (
            String::new(),
            warn("asset.notUploaded", "not uploaded yet, so its id is \"\"; run `opossum upload`".into()),
        ),
    }
}

enum Node<'a> {
    Leaf(&'a str),
    Dir(BTreeMap<&'a str, Node<'a>>),
}

/// The module `opossum/<set>` for its files' `(key, value)`s: Luau source, then types. Nested by path segment, keys
/// in bytewise order. Discovery has refused keys that clash with a directory; here a clash keeps the first.
pub fn render_set(set: &str, values: &[(String, String)]) -> (String, String) {
    let mut tree = BTreeMap::new();
    for (key, value) in values {
        let mut dir = &mut tree;
        let mut segments = key.split('/').peekable();
        while let Some(segment) = segments.next() {
            if segments.peek().is_none() {
                dir.entry(segment).or_insert(Node::Leaf(value));
                break;
            }
            match dir.entry(segment).or_insert_with(|| Node::Dir(BTreeMap::new())) {
                Node::Dir(next) => dir = next,
                Node::Leaf(_) => break,
            }
        }
    }
    let header = format!("opossum/{set}: ids of asset set \"{set}\" in opossum.lock");
    let mut luau = format!("--!strict\n-- {header}\nreturn ");
    luau_table(&mut luau, &tree, 0);
    luau.push('\n');
    let mut ts = format!("// {header}\ndeclare const ids: ");
    ts_type(&mut ts, &tree, 0);
    ts.push_str("\nexport = ids\n");
    (luau, ts)
}

fn luau_table(out: &mut String, dir: &BTreeMap<&str, Node>, depth: usize) {
    if dir.is_empty() {
        return out.push_str("table.freeze({})");
    }
    out.push_str("table.freeze({\n");
    for (key, node) in dir {
        let _ = write!(out, "{}{} = ", "\t".repeat(depth + 1), luau_key(key));
        match node {
            Node::Leaf(value) => out.push_str(&luau_string(value)),
            Node::Dir(dir) => luau_table(out, dir, depth + 1),
        }
        out.push_str(",\n");
    }
    let _ = write!(out, "{}}})", "\t".repeat(depth));
}

fn ts_type(out: &mut String, dir: &BTreeMap<&str, Node>, depth: usize) {
    if dir.is_empty() {
        return out.push_str("{}");
    }
    out.push_str("{\n");
    for (key, node) in dir {
        let _ = write!(out, "{}readonly {}: ", "\t".repeat(depth + 1), ts_key(key));
        match node {
            Node::Leaf(_) => out.push_str("string"),
            Node::Dir(dir) => ts_type(out, dir, depth + 1),
        }
        out.push('\n');
    }
    let _ = write!(out, "{}}}", "\t".repeat(depth));
}

/// Bare when Luau reads it as a name, else `["…"]`.
fn luau_key(key: &str) -> String {
    if is_valid_identifier(key) { key.to_owned() } else { format!("[{}]", luau_string(key)) }
}

/// Bare when it is an ASCII identifier (TypeScript takes keywords as property names), else quoted; `luau_string`'s
/// escapes are TypeScript's too.
fn ts_key(key: &str) -> String {
    let bare = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if bare { key.to_owned() } else { luau_string(key) }
}

/// The build environment's ids. Each resource map lists every declared key with its id, `None` while it is not
/// provisioned (or a merge left two records of it): such a key is left out of the values and optional in the type.
pub struct Resources<'a> {
    pub environment: &'a str,
    pub universe: u64,
    pub places: &'a [(String, u64)],
    pub passes: &'a [(String, Option<u64>)],
    pub products: &'a [(String, Option<u64>)],
    pub badges: &'a [(String, Option<u64>)],
    pub thumbnails: &'a [(String, Option<u64>)],
}

/// The module `opossum/resources`: Luau source, then types.
pub fn render_resources(r: &Resources) -> (String, String) {
    let header = format!("opossum/resources: ids of environment {} in opossum.lock", luau_string(r.environment));
    let places: Vec<(String, Option<u64>)> = r.places.iter().map(|(k, id)| (k.clone(), Some(*id))).collect();
    let maps = [
        ("places", &places[..]),
        ("passes", r.passes),
        ("products", r.products),
        ("badges", r.badges),
        ("thumbnails", r.thumbnails),
    ];
    let mut luau = format!("--!strict\n-- {header}\nreturn table.freeze({{\n");
    let _ = writeln!(luau, "\tenvironment = {},", luau_string(r.environment));
    let _ = writeln!(luau, "\tuniverseId = {},", r.universe);
    let mut ts = format!("// {header}\ndeclare const resources: {{\n\treadonly environment: string\n");
    ts.push_str("\treadonly universeId: number\n");
    for (name, map) in maps {
        let mut map: Vec<&(String, Option<u64>)> = map.iter().collect();
        map.sort_by(|a, b| a.0.cmp(&b.0));
        let values: Vec<String> =
            map.iter().filter_map(|(k, id)| id.map(|id| format!("{} = {id}", luau_key(k)))).collect();
        let _ = match values.is_empty() {
            true => writeln!(luau, "\t{name} = table.freeze({{}}),"),
            false => writeln!(luau, "\t{name} = table.freeze({{ {} }}),", values.join(", ")),
        };
        let types: Vec<String> = map
            .iter()
            .map(|(k, id)| format!("readonly {}{}: number", ts_key(k), if id.is_some() { "" } else { "?" }))
            .collect();
        let _ = match types.is_empty() {
            true => writeln!(ts, "\treadonly {name}: {{}}"),
            false => writeln!(ts, "\treadonly {name}: {{ {} }}", types.join("; ")),
        };
    }
    luau.push_str("})\n");
    ts.push_str("}\nexport = resources\n");
    (luau, ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_set_renders_nested_and_sorted_in_both_languages() {
        // tessera's nested.luau fixture, unsorted on the way in
        let (luau, ts) = render_set(
            "assets",
            &values(&[
                ("materials/wood-planks/basecolor", "rbxassetid://300"),
                ("icons/lucide/activity", "rbxassetid://200"),
                ("icons/lucide/a-arrow-down", "rbxassetid://100"),
                ("end", ""),
            ]),
        );
        assert_eq!(
            luau,
            "--!strict
-- opossum/assets: ids of asset set \"assets\" in opossum.lock
return table.freeze({
\t[\"end\"] = \"\",
\ticons = table.freeze({
\t\tlucide = table.freeze({
\t\t\t[\"a-arrow-down\"] = \"rbxassetid://100\",
\t\t\tactivity = \"rbxassetid://200\",
\t\t}),
\t}),
\tmaterials = table.freeze({
\t\t[\"wood-planks\"] = table.freeze({
\t\t\tbasecolor = \"rbxassetid://300\",
\t\t}),
\t}),
})
"
        );
        assert_eq!(
            ts,
            "// opossum/assets: ids of asset set \"assets\" in opossum.lock
declare const ids: {
\treadonly end: string
\treadonly icons: {
\t\treadonly lucide: {
\t\t\treadonly \"a-arrow-down\": string
\t\t\treadonly activity: string
\t\t}
\t}
\treadonly materials: {
\t\treadonly \"wood-planks\": {
\t\t\treadonly basecolor: string
\t\t}
\t}
}
export = ids
"
        );
        let (luau, ts) = render_set("ui-icons", &[]);
        assert!(luau.ends_with("return table.freeze({})\n") && ts.contains("declare const ids: {}\nexport = ids\n"));
    }

    #[test]
    fn any_file_name_gives_a_module_that_compiles() {
        let keys = ["zero\u{200B}width", "quote\"back\\slash", "tab\there", "bell\u{7}", "über", "1st", "local"];
        let pairs: Vec<(&str, &str)> = keys.iter().map(|k| (*k, "rbxassetid://1")).collect();
        let (luau, ts) = render_set("ui-icons", &values(&pairs));
        luau::gate::compile(&luau, 2, 1).unwrap();
        let root = std::env::temp_dir().join(format!("opossum-ids-{}", std::process::id()));
        let root = root.to_string_lossy().into_owned();
        let module = lower::program::module_file(&root, "ui-icons");
        let user = format!("{root}/src/user.ts");
        let reads: Vec<String> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| format!("export const _{i}: string = ids[{}];", luau_string(k)))
            .collect();
        let overlay = vec![
            (module, ts),
            (user.clone(), format!("import ids from \"opossum/ui-icons\";\n{}\n", reads.join("\n"))),
        ];
        let diags = std::thread::Builder::new()
            .stack_size(lower::STACK_SIZE)
            .spawn(move || {
                let load = lower::LoadOptions { root, files: Vec::new(), checkers: 1, flags: Vec::new(), overlay };
                let project = lower::load(&load, None).unwrap();
                let file = project.program.source_files().iter().copied().find(|f| f.file_name() == user).unwrap();
                let diags = project.check(&tsrs_compiler::Context::background(), &[file]);
                diags.iter().filter(|d| d.file().is_some()).map(|d| d.localize()).collect::<Vec<_>>()
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn an_unprovisioned_resource_is_optional_in_the_type_and_absent_from_the_values() {
        let s = |k: &str| k.to_owned();
        let (luau, ts) = render_resources(&Resources {
            environment: "development",
            universe: 15302144394,
            places: &[(s("main"), 91804762848182)],
            passes: &[(s("vip.gold"), None), (s("vip"), Some(123))],
            products: &[],
            badges: &[(s("welcome"), None)],
            thumbnails: &[],
        });
        assert_eq!(
            luau,
            "--!strict
-- opossum/resources: ids of environment \"development\" in opossum.lock
return table.freeze({
\tenvironment = \"development\",
\tuniverseId = 15302144394,
\tplaces = table.freeze({ main = 91804762848182 }),
\tpasses = table.freeze({ vip = 123 }),
\tproducts = table.freeze({}),
\tbadges = table.freeze({}),
\tthumbnails = table.freeze({}),
})
"
        );
        assert_eq!(
            ts,
            "// opossum/resources: ids of environment \"development\" in opossum.lock
declare const resources: {
\treadonly environment: string
\treadonly universeId: number
\treadonly places: { readonly main: number }
\treadonly passes: { readonly vip: number; readonly \"vip.gold\"?: number }
\treadonly products: {}
\treadonly badges: { readonly welcome?: number }
\treadonly thumbnails: {}
}
export = resources
"
        );
        luau::gate::compile(&luau, 2, 1).unwrap();
    }
}
