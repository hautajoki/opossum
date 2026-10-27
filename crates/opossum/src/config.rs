//! `opossum.toml`: read into a plain struct, every key checked. Paths in it are relative to the file.

use std::path::{Path, PathBuf};

pub const FILE: &str = "opossum.toml";

pub struct Config {
    /// The directory holding `opossum.toml`, canonical.
    pub root: PathBuf,
    pub name: String,
    /// The place id the Studio plugin auto-connects to; `None` lets unpublished places connect.
    pub place: Option<u64>,
    /// Route key → `Service/Folder/...`, in file order.
    pub routes: Vec<(String, String)>,
    /// `[native] files`: globs relative to the root.
    pub native: Vec<String>,
    /// `[ts]`: strictness flags by their tsconfig names.
    pub ts: Vec<(String, bool)>,
    pub profiles: Vec<Profile>,
    /// `[place]`: service and instance properties.
    pub place_props: place::PlaceConfig,
}

pub struct Profile {
    pub name: String,
    /// Globs relative to the root; matching source files are left out of the build.
    pub exclude: Vec<String>,
}

impl Config {
    pub fn src(&self) -> PathBuf {
        self.root.join("src")
    }

    pub fn profile(&self, name: &str) -> Result<&Profile, String> {
        self.profiles.iter().find(|p| p.name == name).ok_or_else(|| format!("{FILE}: no [profile.{name}]"))
    }
}

/// The `opossum.toml` in `dir` or the nearest directory above it.
pub fn find(dir: &Path) -> Result<PathBuf, String> {
    let mut at = Some(dir);
    while let Some(d) = at {
        let file = d.join(FILE);
        if file.is_file() {
            return Ok(file);
        }
        at = d.parent();
    }
    Err(format!("no {FILE} in {} or above it", dir.display()))
}

pub fn load(file: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let dir = file.parent().unwrap_or(Path::new("."));
    let root = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    parse(&text, root).map_err(|e| format!("{}: {e}", file.display()))
}

fn parse(text: &str, root: PathBuf) -> Result<Config, String> {
    let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut config = Config {
        root,
        name: String::new(),
        place: None,
        routes: Vec::new(),
        native: Vec::new(),
        ts: Vec::new(),
        profiles: Vec::new(),
        place_props: Vec::new(),
    };
    for (key, value) in &doc {
        match key.as_str() {
            "name" => config.name = string(value, key)?,
            "placeId" => {
                let id = value.as_integer().filter(|&id| id >= 0).ok_or("`placeId` must be a place id")?;
                config.place = Some(id as u64);
            }
            "routes" => {
                for (k, v) in table(value, key)? {
                    config.routes.push((k.clone(), string(v, &format!("routes.{k}"))?));
                }
            }
            "native" => {
                for (k, v) in table(value, key)? {
                    match k.as_str() {
                        "files" => config.native = strings(v, "native.files")?,
                        _ => return Err(unknown(&format!("native.{k}"))),
                    }
                }
            }
            "ts" => {
                for (k, v) in table(value, key)? {
                    let flag = v.as_bool().ok_or_else(|| format!("`ts.{k}` must be true or false"))?;
                    config.ts.push((k.clone(), flag));
                }
            }
            "profile" => {
                for (name, v) in table(value, key)? {
                    let mut profile = Profile { name: name.clone(), exclude: Vec::new() };
                    for (k, v) in table(v, &format!("profile.{name}"))? {
                        match k.as_str() {
                            "exclude" => profile.exclude = strings(v, &format!("profile.{name}.exclude"))?,
                            _ => return Err(unknown(&format!("profile.{name}.{k}"))),
                        }
                    }
                    config.profiles.push(profile);
                }
            }
            "place" => config.place_props = place::config_from_toml(table(value, key)?)?,
            _ => return Err(unknown(key)),
        }
    }
    if config.name.is_empty() {
        return Err("`name` is missing".into());
    }
    if config.routes.is_empty() {
        return Err("[routes] declares no route".into());
    }
    // the TypeScript flag names are lower's to check: fail here rather than at the first build
    lower::compiler_options("/", &config.ts)?;
    Ok(config)
}

fn unknown(path: &str) -> String {
    format!("unknown key `{path}`")
}

fn table<'a>(value: &'a toml::Value, path: &str) -> Result<&'a toml::Table, String> {
    value.as_table().ok_or_else(|| format!("`{path}` must be a table"))
}

fn string(value: &toml::Value, path: &str) -> Result<String, String> {
    value.as_str().map(str::to_owned).ok_or_else(|| format!("`{path}` must be a string"))
}

fn strings(value: &toml::Value, path: &str) -> Result<Vec<String>, String> {
    let list = value.as_array().ok_or_else(|| format!("`{path}` must be a list of strings"))?;
    list.iter().map(|v| string(v, path)).collect()
}
