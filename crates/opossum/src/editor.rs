//! `.opossum/`: what the editor reads. `tsconfig.json` carries the compiler options lowering uses (the project's own
//! tsconfig is `{ "extends": "./.opossum/tsconfig.json" }`), `opossum.d.ts` and `vector.d.ts` the declarations
//! lowering injects.

use std::path::{Path, PathBuf};

use serde_json::json;

pub const DIR: &str = ".opossum";

pub fn dir(root: &Path) -> PathBuf {
    root.join(DIR)
}

/// Writes `tsconfig.json` and the injected declarations, leaving files whose text is already right alone.
pub fn write(root: &Path, flags: &[(String, bool)]) -> Result<(), String> {
    let dir = dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    write_if_changed(&dir.join("tsconfig.json"), &tsconfig(root, flags))?;
    for (path, text) in lower::program::declarations(&root.to_string_lossy()) {
        write_if_changed(Path::new(&path), &text)?;
    }
    Ok(())
}

pub fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `lower::compiler_options` as tsconfig JSON, paths relative to `.opossum/`. Lowering types every package of the
/// `@rbxts` type root (`types: ["*"]`); editors on TypeScript 5 do not take `*`, so the packages are listed.
fn tsconfig(root: &Path, flags: &[(String, bool)]) -> String {
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
        "paths": { "*": ["../src/*"] },
        "typeRoots": ["../node_modules/@rbxts"],
        "types": types,
    });
    if let Some(map) = options.as_object_mut() {
        for (name, value) in flags {
            map.insert(name.clone(), json!(value));
        }
    }
    let config = json!({
        "compilerOptions": options,
        "include": ["../src", "opossum.d.ts", "vector.d.ts"],
    });
    let mut text = serde_json::to_string_pretty(&config).unwrap_or_default();
    text.push('\n');
    text
}
