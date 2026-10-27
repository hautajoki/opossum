//! The `[place]` table of opossum.toml: properties for services and other instances, keyed by instance path.
//!
//! ```toml
//! [place.HttpService]
//! HttpEnabled = true
//!
//! [place."Workspace/Terrain"]
//! ClassName = "Terrain"         # only needed to create or retype an instance
//! WaterColor = [12, 84, 91]
//! MaterialColors = { Asphalt = [112, 112, 108] }
//! ```
//!
//! Values take the same shorthand as model.json properties.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct PlaceEntry {
    /// `/`-separated from the DataModel: `Lighting`, `Workspace/Terrain`.
    pub path: String,
    pub class: Option<String>,
    pub props: Vec<(String, Value)>,
}

/// The entries in file order.
pub type PlaceConfig = Vec<PlaceEntry>;

/// Reads the `[place]` table (the table itself, not the whole document).
pub fn config_from_toml(table: &toml::Table) -> Result<PlaceConfig, String> {
    let mut config = Vec::new();
    for (path, props) in table {
        let toml::Value::Table(props) = props else {
            return Err(format!("[place] {path} must be a table of properties"));
        };
        let mut entry = PlaceEntry {
            path: path.clone(),
            class: None,
            props: Vec::new(),
        };
        for (name, value) in props {
            let json =
                serde_json::to_value(value).map_err(|e| format!("[place] {path}.{name}: {e}"))?;
            if name == "ClassName" {
                let class = json
                    .as_str()
                    .ok_or(format!("[place] {path}.ClassName must be a string"))?;
                entry.class = Some(class.to_string());
            } else {
                entry.props.push((name.clone(), json));
            }
        }
        config.push(entry);
    }
    Ok(config)
}

/// Renders entries as `[place."path"]` tables, ready to paste into opossum.toml.
pub fn config_to_toml(config: &PlaceConfig) -> Result<String, String> {
    let mut place = toml::Table::new();
    for entry in config {
        let mut props = toml::Table::new();
        if let Some(class) = &entry.class {
            props.insert("ClassName".into(), toml::Value::String(class.clone()));
        }
        for (name, json) in &entry.props {
            let value =
                toml::Value::try_from(json).map_err(|e| format!("{}.{name}: {e}", entry.path))?;
            props.insert(name.clone(), value);
        }
        place.insert(entry.path.clone(), toml::Value::Table(props));
    }
    let mut doc = toml::Table::new();
    doc.insert("place".into(), toml::Value::Table(place));
    toml::to_string(&doc).map_err(|e| e.to_string())
}
