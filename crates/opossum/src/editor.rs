//! `.opossum/`: what the editor reads. `tsconfig.json` carries the compiler options lowering uses (the project's own
//! tsconfig is `{ "extends": "./.opossum/tsconfig.json" }`), `opossum.d.ts` and `vector.d.ts` the declarations
//! lowering injects, and `modules/` the virtual modules `opossum/<name>`: each one's types (`<name>.d.ts`, which the
//! program reads from memory, never from here) and its source (`<name>.luau`) for luau-lsp and Luau tools that read it.

use std::path::{Path, PathBuf};

use serde_json::json;

pub const DIR: &str = ".opossum";

pub fn dir(root: &Path) -> PathBuf {
    root.join(DIR)
}

/// A virtual module `opossum/<name>`: its types and, unless it is types only, its Luau source.
pub struct Module {
    pub name: String,
    pub types: String,
    pub luau: Option<String>,
}

/// `opossum/test`: the suite types, with no runtime module.
pub fn test_module() -> Module {
    Module { name: "test".to_owned(), types: include_str!("test.d.ts").to_owned(), luau: None }
}

/// The editor's copy of a module's Luau source, which the sourcemap points luau-lsp at.
pub fn module_luau(root: &Path, name: &str) -> PathBuf {
    dir(root).join("modules").join(format!("{name}.luau"))
}

/// What every program of the project at `root` reads from memory: lowering's declarations and the modules' types.
pub fn overlay(root: &str, modules: &[Module]) -> Vec<(String, String)> {
    let mut files = lower::program::declarations(root);
    files.extend(modules.iter().map(|m| (lower::program::module_file(root, &m.name), m.types.clone())));
    files
}

/// Writes `tsconfig.json`, the injected declarations and `modules`, leaving files whose text is already right alone,
/// and removes what `modules/` holds of a module the project no longer has.
pub fn write(root: &Path, flags: &[(String, bool)], modules: &[Module]) -> Result<(), String> {
    let dir = dir(root);
    let modules_dir = dir.join("modules");
    std::fs::create_dir_all(&modules_dir).map_err(|e| format!("{}: {e}", modules_dir.display()))?;
    let overlay = overlay(&root.to_string_lossy(), modules);
    write_if_changed(&dir.join("tsconfig.json"), &tsconfig(root, flags, &overlay))?;
    for (path, text) in &overlay {
        write_if_changed(Path::new(path), text)?;
    }
    let mut keep = Vec::new();
    for m in modules {
        keep.push(format!("{}.d.ts", m.name));
        if let Some(luau) = &m.luau {
            let file = format!("{}.luau", m.name);
            write_if_changed(&modules_dir.join(&file), luau)?;
            keep.push(file);
        }
    }
    let entries = std::fs::read_dir(&modules_dir).map_err(|e| format!("{}: {e}", modules_dir.display()))?;
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(|name| keep.iter().any(|k| k == name)) {
            let path = entry.path();
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

pub fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `lower::compiler_options` as tsconfig JSON, paths relative to `.opossum/`, including `src/` and the `overlay`
/// files as the program's root files. Lowering types every package of the `@rbxts` type root (`types: ["*"]`);
/// editors on TypeScript 5 do not take `*`, so the packages are listed.
fn tsconfig(root: &Path, flags: &[(String, bool)], overlay: &[(String, String)]) -> String {
    let mut types: Vec<String> = std::fs::read_dir(root.join("node_modules/@rbxts"))
        .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    types.retain(|t| !t.starts_with('.'));
    types.sort();
    let mut options = json!({
        "module": "preserve",
        "moduleResolution": "bundler",
        "moduleDetection": "force",
        "target": "ESNext",
        "noLib": true,
        "strict": true,
        "jsx": "react",
        "jsxFactory": "React.createElement",
        "jsxFragmentFactory": "React.Fragment",
        "experimentalDecorators": true,
        "resolveJsonModule": true,
        "allowSyntheticDefaultImports": true,
        "forceConsistentCasingInFileNames": true,
        "skipLibCheck": true,
        "noEmit": true,
        "paths": { "*": ["../src/*"], "opossum/*": ["./modules/*"] },
        "typeRoots": ["../node_modules/@rbxts"],
        "types": types,
    });
    if let Some(map) = options.as_object_mut() {
        for (name, value) in flags {
            map.insert(name.clone(), json!(value));
        }
    }
    let dir = format!("{}/", dir(root).to_string_lossy());
    let mut include = vec!["../src".to_owned()];
    include.extend(overlay.iter().map(|(path, _)| path.strip_prefix(&dir).unwrap_or(path).to_owned()));
    let config = json!({ "compilerOptions": options, "include": include });
    let mut text = serde_json::to_string_pretty(&config).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use tsrs_core::tspath::get_normalized_absolute_path;

    /// The editor sees the program lowering loads: the same `paths` and, besides `src/`, the same root files.
    #[test]
    fn tsconfig_agrees_with_the_compiler_options() {
        let root = "/project";
        let overlay = super::overlay(root, &[super::test_module()]);
        let config: serde_json::Value = serde_json::from_str(&super::tsconfig(Path::new(root), &[], &overlay)).unwrap();
        let editor = "/project/.opossum";

        let options = lower::compiler_options(root, &[]).unwrap();
        let mut lowered = BTreeMap::new();
        for (pattern, targets) in options.paths.as_deref().unwrap().iter() {
            let targets = targets.as_deref().unwrap_or_default();
            let targets: Vec<String> =
                targets.iter().map(|t| get_normalized_absolute_path(t, &options.paths_base_path)).collect();
            lowered.insert(pattern.clone(), targets);
        }
        let mut edited = BTreeMap::new();
        for (pattern, targets) in config["compilerOptions"]["paths"].as_object().unwrap() {
            let targets = targets.as_array().unwrap();
            let targets: Vec<String> =
                targets.iter().map(|t| get_normalized_absolute_path(t.as_str().unwrap(), editor)).collect();
            edited.insert(pattern.clone(), targets);
        }
        assert_eq!(edited, lowered);

        let include: Vec<String> = config["include"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| get_normalized_absolute_path(p.as_str().unwrap(), editor))
            .collect();
        let mut files = vec!["/project/src".to_owned()];
        files.extend(overlay.into_iter().map(|(path, _)| path));
        assert_eq!(include, files);
    }
}
