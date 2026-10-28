//! Properties files: every `*.toml` directly under `place/` sets properties on services and other instances, as
//! tables keyed by instance path.
//!
//! ```toml
//! # place/workspace.toml
//! [Workspace]
//! StreamingEnabled = false
//!
//! ["Workspace/Terrain"]
//! ClassName = "Terrain"         # only needed to create or retype an instance
//! WaterColor = [12, 84, 91]
//! MaterialColors = { Asphalt = [112, 112, 108] }
//! ```
//!
//! Values take the same shorthand as model.json properties. Files are read in name order, and a path belongs to
//! one file: setting it in a second is `place.pathClash`. Entries apply parents first, so a file may set or make
//! children of an instance another file makes, whatever the files are called.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::Diag;

#[derive(Debug, Clone, PartialEq)]
pub struct PlaceEntry {
    /// `/`-separated from the DataModel: `Lighting`, `Workspace/Terrain`.
    pub path: String,
    pub class: Option<String>,
    pub props: Vec<(String, Value)>,
    /// The properties file it was read from, which its diagnostics name.
    pub file: PathBuf,
}

/// Every properties file of a `place/` and the entries they hold: files in name order, entries by path depth, then in
/// file order.
#[derive(Default)]
pub struct Properties {
    pub files: Vec<PathBuf>,
    pub entries: Vec<PlaceEntry>,
}

/// Whether `name`, an entry directly under `place/`, is a properties file.
pub fn is_properties_file(name: &str) -> bool {
    name.ends_with(".toml") && !name.starts_with('.')
}

/// Reads the properties files of `dir` (a project's `place/`). A file that does not parse sets nothing; a path set
/// in an earlier file is skipped.
pub fn read_properties(dir: &Path, diags: &mut Vec<Diag>) -> Properties {
    let mut props = Properties::default();
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| is_properties_file(name))
        .collect();
    names.sort();
    for name in names {
        let file = dir.join(&name);
        if !file.is_file() {
            continue;
        }
        let read = fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|text| {
            let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
            entries_from_toml(&table, &file)
        });
        match read {
            Ok(entries) => {
                for entry in entries {
                    match props.entries.iter().find(|e| e.path == entry.path) {
                        Some(first) => {
                            let first = first.file.file_name().unwrap_or_default().to_string_lossy();
                            let message = format!("{} is already set in place/{first}", entry.path);
                            diags.push(Diag::error("place.pathClash", &file, message));
                        }
                        None => props.entries.push(entry),
                    }
                }
            }
            Err(e) => diags.push(Diag::error("place.badProperties", &file, e)),
        }
        props.files.push(file);
    }
    props.entries.sort_by_key(|e| e.path.matches('/').count());
    props
}

/// The entries of a properties file's tables, or of opossum.toml's former `[place]` table.
pub fn entries_from_toml(table: &toml::Table, file: &Path) -> Result<Vec<PlaceEntry>, String> {
    let mut entries = Vec::new();
    for (path, props) in table {
        let toml::Value::Table(props) = props else {
            return Err(format!("{path} must be a table of properties"));
        };
        let mut entry = PlaceEntry { path: path.clone(), class: None, props: Vec::new(), file: file.to_path_buf() };
        for (name, value) in props {
            let json = serde_json::to_value(value).map_err(|e| format!("{path}.{name}: {e}"))?;
            if name == "ClassName" {
                let class = json.as_str().ok_or(format!("{path}.ClassName must be a string"))?;
                entry.class = Some(class.to_string());
            } else {
                entry.props.push((name.clone(), json));
            }
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// Renders entries as the tables of a properties file.
pub(crate) fn entries_to_toml(entries: &[&PlaceEntry]) -> Result<String, String> {
    let mut doc = toml::Table::new();
    for entry in entries {
        let mut props = toml::Table::new();
        if let Some(class) = &entry.class {
            props.insert("ClassName".into(), toml::Value::String(class.clone()));
        }
        for (name, json) in &entry.props {
            let value = toml::Value::try_from(json).map_err(|e| format!("{}.{name}: {e}", entry.path))?;
            props.insert(name.clone(), value);
        }
        doc.insert(entry.path.clone(), toml::Value::Table(props));
    }
    toml::to_string(&doc).map_err(|e| e.to_string())
}
