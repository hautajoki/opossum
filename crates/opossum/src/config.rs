//! `opossum.toml`: read into a plain struct, every key checked. Paths in it are relative to the file.

use std::path::{Path, PathBuf};

pub use cloud::Creator;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use media::{ImageOptions, Preset};

pub const FILE: &str = "opossum.toml";

pub struct Config {
    /// The directory holding `opossum.toml`, canonical.
    pub root: PathBuf,
    pub name: String,
    /// The place id the Studio plugin auto-connects to: `placeId`, or the default environment's root place. `None`
    /// lets unpublished places connect.
    pub place: Option<u64>,
    /// Owner of uploaded assets and payer of badge fees; set whenever there are asset sets or badges.
    pub creator: Option<Creator>,
    /// `[assets]`, in file order.
    pub assets: Vec<AssetSet>,
    /// `[places]`, in file order: one `main` when absent. Exactly one is the root.
    pub places: Vec<Place>,
    pub environments: Vec<Environment>,
    /// `defaultEnvironment`, or the only environment.
    pub default_environment: Option<String>,
    pub passes: Vec<Sale>,
    pub products: Vec<Sale>,
    pub badges: Vec<Badge>,
    pub experience: Option<Experience>,
    /// Route key → `Service/Folder/...`, in file order.
    pub routes: Vec<(String, String)>,
    /// `[native] files`: globs relative to the root.
    pub native: Vec<String>,
    /// `[ts]`: strictness flags by their tsconfig names.
    pub ts: Vec<(String, bool)>,
    pub profiles: Vec<Profile>,
    /// `[dev] hotReload`: a running playtest's server swaps the modules an edit changed.
    pub hot_reload: bool,
}

pub struct Profile {
    pub name: String,
    /// Globs relative to the root; matching source files are left out of the build.
    pub exclude: Vec<String>,
}

/// An `[assets]` entry: the files under `root` whose ids the module `opossum/<name>` holds. A set whose root lies
/// inside another's owns that subtree.
pub struct AssetSet {
    pub name: String,
    /// Relative to the project, `/`-separated and normalized: `place/assets`.
    pub root: String,
    pub image: ImageOptions,
    /// Globs on a file's path under `root`; None takes every file.
    pub include: Option<GlobSet>,
    pub exclude: GlobSet,
}

impl AssetSet {
    /// Whether the set takes the file at `rel`, its `/`-separated path under the root.
    pub fn selects(&self, rel: &str) -> bool {
        self.include.as_ref().is_none_or(|g| g.is_match(rel)) && !self.exclude.is_match(rel)
    }
}

/// A logical place, `[places.<name>]`.
pub struct Place {
    pub name: String,
    pub root: bool,
    /// The build profile `deploy` and cloud runs build this place with.
    pub profile: Option<String>,
}

/// `[environments.<name>]`: a universe and the ids of the places in it.
pub struct Environment {
    pub name: String,
    pub universe: u64,
    /// Logical place → place id, in file order; always holds the root place.
    pub places: Vec<(String, u64)>,
}

/// A `[passes.<key>]` or `[products.<key>]` entry. An absent optional field leaves Roblox's value alone.
pub struct Sale {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub price: Option<u64>,
    pub for_sale: Option<bool>,
    pub managed_pricing: Option<bool>,
    /// Relative to the project, as written.
    pub icon: Option<String>,
}

pub struct Badge {
    pub key: String,
    pub name: String,
    pub description: String,
    /// Relative to the project, as written.
    pub icon: String,
    pub enabled: bool,
    /// The exact Robux fee a create may accept.
    pub expected_cost: u64,
}

/// `[experience]`: only the fields present are managed. Every thumbnail not listed is deleted.
pub struct Experience {
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    /// In carousel order.
    pub thumbnails: Vec<Thumbnail>,
}

pub struct Thumbnail {
    pub key: String,
    pub image: String,
    pub alt_text: Option<String>,
}

impl Config {
    pub fn src(&self) -> PathBuf {
        self.root.join("src")
    }

    pub fn profile(&self, name: &str) -> Result<&Profile, String> {
        self.profiles.iter().find(|p| p.name == name).ok_or_else(|| format!("{FILE}: no [profile.{name}]"))
    }

    /// The environment `--environment` names, else the default one.
    pub fn environment(&self, name: Option<&str>) -> Result<&Environment, String> {
        let names = self.environments.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ");
        let Some(name) = name.or(self.default_environment.as_deref()) else {
            return Err(format!("{FILE} declares no [environments]"));
        };
        let found = self.environments.iter().find(|e| e.name == name);
        found.ok_or_else(|| format!("environment `{name}` is not declared; choose one of: {names}"))
    }

    /// The environment whose ids a build's `opossum/resources` holds: `--environment`, else the default one; None
    /// without `[environments]` and a flag.
    pub fn build_environment(&self, name: Option<&str>) -> Result<Option<&Environment>, String> {
        match name {
            None if self.environments.is_empty() => Ok(None),
            name => self.environment(name).map(Some),
        }
    }

    /// Every asset set's root, which the place reader leaves out of `place/`.
    pub fn asset_roots(&self) -> Vec<PathBuf> {
        self.assets.iter().map(|s| self.root.join(&s.root)).collect()
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
        hot_reload: false,
        creator: None,
        assets: Vec::new(),
        places: Vec::new(),
        environments: Vec::new(),
        default_environment: None,
        passes: Vec::new(),
        products: Vec::new(),
        badges: Vec::new(),
        experience: None,
    };
    for (key, value) in &doc {
        match key.as_str() {
            "name" => config.name = string(value, key)?,
            "placeId" => {
                let id = value.as_integer().filter(|&id| id >= 0).ok_or("`placeId` must be a place id")?;
                config.place = Some(id as u64);
            }
            "creator" => config.creator = Some(creator(value)?),
            "defaultEnvironment" => config.default_environment = Some(string(value, key)?),
            "assets" => {
                for (name, v) in table(value, key)? {
                    let set = asset_set(&config.root, name, v)?;
                    if let Some(other) = config.assets.iter().find(|s| s.root == set.root) {
                        return Err(format!("`assets.{}` and `assets.{name}` have the same root", other.name));
                    }
                    config.assets.push(set);
                }
            }
            "places" => {
                for (name, v) in table(value, key)? {
                    config.places.push(place(name, v)?);
                }
                if config.places.is_empty() {
                    return Err("[places] declares no place".into());
                }
            }
            "environments" => {
                for (name, v) in table(value, key)? {
                    config.environments.push(environment(name, v)?);
                }
            }
            "passes" => config.passes = sales(value, key)?,
            "products" => config.products = sales(value, key)?,
            "badges" => {
                for (k, v) in table(value, key)? {
                    config.badges.push(badge(k, v)?);
                }
            }
            "experience" => config.experience = Some(experience(value)?),
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
                    config.ts.push((k.clone(), flag(v, &format!("ts.{k}"))?));
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
            "place" => return Err(MOVED.into()),
            "dev" => {
                for (k, v) in table(value, key)? {
                    match k.as_str() {
                        "hotReload" => config.hot_reload = flag(v, "dev.hotReload")?,
                        _ => return Err(unknown(&format!("dev.{k}"))),
                    }
                }
            }
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
    link(&mut config)?;
    Ok(config)
}

/// The rules between tables, which may come in any order: places and their profiles, environments and their places,
/// the default environment, and what needs a creator or an environment.
fn link(config: &mut Config) -> Result<(), String> {
    if config.places.is_empty() {
        config.places.push(Place { name: "main".into(), root: true, profile: None });
    }
    if let [only] = config.places.as_mut_slice() {
        only.root = true;
    }
    let roots: Vec<&Place> = config.places.iter().filter(|p| p.root).collect();
    let [root] = roots[..] else { return Err("[places] must mark exactly one place `root = true`".into()) };
    let root = root.name.clone();
    for p in &config.places {
        if let Some(profile) = &p.profile
            && !config.profiles.iter().any(|q| &q.name == profile)
        {
            return Err(format!("`places.{}.profile` names no [profile.{profile}]", p.name));
        }
    }
    for e in &config.environments {
        for (name, _) in &e.places {
            if !config.places.iter().any(|p| &p.name == name) {
                return Err(format!("`environments.{}.places.{name}` is not a declared place", e.name));
            }
        }
        if !e.places.iter().any(|(name, _)| name == &root) {
            return Err(format!("`environments.{}.places` must map the root place `{root}`", e.name));
        }
    }
    let names = config.environments.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ");
    match (&config.default_environment, config.environments.as_slice()) {
        (Some(_), []) => return Err("`defaultEnvironment` needs [environments]".into()),
        (Some(name), envs) if !envs.iter().any(|e| &e.name == name) => {
            return Err(format!("`defaultEnvironment` must name one of: {names}"));
        }
        (Some(_), _) | (None, []) => {}
        (None, [only]) => config.default_environment = Some(only.name.clone()),
        (None, _) => return Err(format!("`defaultEnvironment` must name one of: {names}")),
    }
    if !config.environments.is_empty() {
        if config.place.is_some() {
            let why = "the default environment's root place is the Studio place";
            return Err(format!("`placeId` cannot be used with [environments]; {why}"));
        }
        let env = config.environment(None)?;
        config.place = env.places.iter().find(|(name, _)| name == &root).map(|&(_, id)| id);
    }
    let resources = [
        ("[passes]", !config.passes.is_empty()),
        ("[products]", !config.products.is_empty()),
        ("[badges]", !config.badges.is_empty()),
        ("[experience]", config.experience.is_some()),
    ];
    if let Some((table, _)) = resources.iter().find(|(_, used)| *used)
        && config.environments.is_empty()
    {
        return Err(format!("{table} needs [environments]"));
    }
    if config.creator.is_none() && (!config.assets.is_empty() || !config.badges.is_empty()) {
        return Err("`creator` is required by [assets] and [badges]".into());
    }
    Ok(())
}

fn creator(value: &toml::Value) -> Result<Creator, String> {
    const RULE: &str = "`creator` must be { user = <id> } or { group = <id> }";
    let entries: Vec<_> = value.as_table().ok_or(RULE)?.iter().collect();
    let [(kind, id)] = entries[..] else { return Err(RULE.into()) };
    let id = id.as_integer().filter(|&id| id > 0).ok_or(RULE)? as u64;
    match kind.as_str() {
        "user" => Ok(Creator::User(id)),
        "group" => Ok(Creator::Group(id)),
        _ => Err(RULE.into()),
    }
}

/// `name = "root"`, or `name = { root, image = { preset, bleed }, include, exclude }`.
fn asset_set(project: &Path, name: &str, value: &toml::Value) -> Result<AssetSet, String> {
    let path = format!("assets.{name}");
    if !is_name(name, "_-") || name == "test" || name == "resources" {
        let rule = "[A-Za-z][A-Za-z0-9_-]*, and not test or resources";
        return Err(format!("`{path}`: a set is the module opossum/{name}, so its name must match {rule}"));
    }
    let mut set = AssetSet {
        name: name.to_owned(),
        root: String::new(),
        image: ImageOptions::DEFAULT,
        include: None,
        exclude: GlobSet::empty(),
    };
    // the key the root was written at, for its error
    let root_key = match value {
        toml::Value::String(root) => {
            set.root = root.clone();
            path.clone()
        }
        toml::Value::Table(t) => {
            for (k, v) in t {
                let at = format!("{path}.{k}");
                match k.as_str() {
                    "root" => set.root = string(v, &at)?,
                    "image" => {
                        for (k, v) in table(v, &at)? {
                            let at = format!("{at}.{k}");
                            match k.as_str() {
                                "preset" => {
                                    let names = Preset::ALL.map(Preset::name).join(", ");
                                    let preset = v.as_str().and_then(Preset::parse);
                                    set.image.preset =
                                        preset.ok_or_else(|| format!("`{at}` must be one of: {names}"))?;
                                }
                                "bleed" => set.image.bleed = flag(v, &at)?,
                                _ => return Err(unknown(&at)),
                            }
                        }
                    }
                    "include" => {
                        let globs = strings(v, &at)?;
                        if globs.is_empty() {
                            return Err(format!("`{at}` must list at least one glob; leave it out to take every file"));
                        }
                        set.include = Some(glob_set(&globs, &at)?);
                    }
                    "exclude" => set.exclude = glob_set(&strings(v, &at)?, &at)?,
                    _ => return Err(unknown(&at)),
                }
            }
            if !t.contains_key("root") {
                return Err(format!("`{path}.root` is missing"));
            }
            format!("{path}.root")
        }
        _ => return Err(format!("`{path}` must be a directory or a table with `root`")),
    };
    set.root = normalize(&set.root)
        .filter(|r| !r.is_empty() && project.join(r).is_dir())
        .filter(|r| !["src", ".opossum"].iter().any(|d| r == d || r.starts_with(&format!("{d}/"))))
        .ok_or_else(|| format!("`{root_key}` must be a directory inside the project, outside src/ and .opossum/"))?;
    Ok(set)
}

/// `path` with `.`, empty segments and `..` resolved; None when it leaves the project.
fn normalize(path: &str) -> Option<String> {
    if path.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Git-style globs on `/`-separated relative paths: `*` stays within a segment and `**` spans them.
pub(crate) fn glob_set(patterns: &[String], path: &str) -> Result<GlobSet, String> {
    let mut set = GlobSetBuilder::new();
    for p in patterns {
        if p.is_empty() || p.starts_with('/') || p.split('/').any(|s| s == "." || s == "..") {
            return Err(format!("`{path}`: `{p}` must be a path pattern under the set's root"));
        }
        set.add(GlobBuilder::new(p).literal_separator(true).build().map_err(|e| format!("`{path}`: `{p}`: {e}"))?);
    }
    set.build().map_err(|e| format!("`{path}`: {e}"))
}

fn place(name: &str, value: &toml::Value) -> Result<Place, String> {
    if !is_name(name, "_-") {
        return Err(format!("`places.{name}`: a place name must match [A-Za-z][A-Za-z0-9_-]*"));
    }
    let mut place = Place { name: name.to_owned(), root: false, profile: None };
    for (k, v) in table(value, &format!("places.{name}"))? {
        let at = format!("places.{name}.{k}");
        match k.as_str() {
            "root" => place.root = flag(v, &at)?,
            "profile" => place.profile = Some(string(v, &at)?),
            _ => return Err(unknown(&at)),
        }
    }
    Ok(place)
}

fn environment(name: &str, value: &toml::Value) -> Result<Environment, String> {
    let path = format!("environments.{name}");
    if !is_name(name, "_-") {
        return Err(format!("`{path}`: an environment name must match [A-Za-z][A-Za-z0-9_-]*"));
    }
    let mut env = Environment { name: name.to_owned(), universe: 0, places: Vec::new() };
    for (k, v) in table(value, &path)? {
        let at = format!("{path}.{k}");
        match k.as_str() {
            "universeId" => env.universe = id(v, &at)?,
            "places" => {
                for (p, v) in table(v, &at)? {
                    let id = id(v, &format!("{at}.{p}"))?;
                    if let Some((q, _)) = env.places.iter().find(|&&(_, other)| other == id) {
                        return Err(format!("`{at}` maps `{q}` and `{p}` to the same place id"));
                    }
                    env.places.push((p.clone(), id));
                }
            }
            _ => return Err(unknown(&at)),
        }
    }
    if env.universe == 0 {
        return Err(format!("`{path}.universeId` is missing"));
    }
    Ok(env)
}

/// `[passes]` or `[products]`.
fn sales(value: &toml::Value, group: &str) -> Result<Vec<Sale>, String> {
    table(value, group)?.iter().map(|(key, v)| sale(&format!("{group}.{key}"), key, v)).collect()
}

fn sale(path: &str, key: &str, value: &toml::Value) -> Result<Sale, String> {
    resource_key(path, key)?;
    let mut sale = Sale {
        key: key.to_owned(),
        name: String::new(),
        description: None,
        price: None,
        for_sale: None,
        managed_pricing: None,
        icon: None,
    };
    for (k, v) in table(value, path)? {
        let at = format!("{path}.{k}");
        match k.as_str() {
            "name" => sale.name = text(v, &at)?,
            "description" => sale.description = Some(string(v, &at)?),
            "price" => {
                let price = v.as_integer().filter(|p| (1..=1_000_000_000).contains(p));
                sale.price = Some(price.ok_or_else(|| format!("`{at}` must be 1 to 1000000000 Robux"))? as u64);
            }
            "forSale" => sale.for_sale = Some(flag(v, &at)?),
            "managedPricing" => sale.managed_pricing = Some(flag(v, &at)?),
            "icon" => sale.icon = Some(text(v, &at)?),
            _ => return Err(unknown(&at)),
        }
    }
    if sale.name.is_empty() {
        return Err(format!("`{path}.name` is missing"));
    }
    Ok(sale)
}

fn badge(key: &str, value: &toml::Value) -> Result<Badge, String> {
    let path = format!("badges.{key}");
    resource_key(&path, key)?;
    let mut badge = Badge {
        key: key.to_owned(),
        name: String::new(),
        description: String::new(),
        icon: String::new(),
        enabled: true,
        expected_cost: 0,
    };
    for (k, v) in table(value, &path)? {
        let at = format!("{path}.{k}");
        match k.as_str() {
            "name" => badge.name = text(v, &at)?,
            "description" => badge.description = text(v, &at)?,
            "icon" => badge.icon = text(v, &at)?,
            "enabled" => badge.enabled = flag(v, &at)?,
            "expectedCost" => {
                let cost = v.as_integer().filter(|&c| c >= 0);
                badge.expected_cost = cost.ok_or_else(|| format!("`{at}` must be a Robux amount, 0 or more"))? as u64;
            }
            _ => return Err(unknown(&at)),
        }
    }
    for (field, value) in [("name", &badge.name), ("description", &badge.description), ("icon", &badge.icon)] {
        if value.is_empty() {
            return Err(format!("`{path}.{field}` is missing"));
        }
    }
    Ok(badge)
}

fn experience(value: &toml::Value) -> Result<Experience, String> {
    let mut exp = Experience { name: None, description: None, icon: None, thumbnails: Vec::new() };
    for (k, v) in table(value, "experience")? {
        let at = format!("experience.{k}");
        match k.as_str() {
            "name" => exp.name = Some(text(v, &at)?),
            "description" => exp.description = Some(string(v, &at)?),
            "icon" => exp.icon = Some(text(v, &at)?),
            "thumbnails" => {
                let list = v.as_array().ok_or_else(|| format!("`{at}` must be [[experience.thumbnails]] tables"))?;
                if list.len() > 10 {
                    return Err(format!("`{at}` holds {} thumbnails; Roblox shows at most 10", list.len()));
                }
                for (i, v) in list.iter().enumerate() {
                    exp.thumbnails.push(thumbnail(&format!("{at}[{i}]"), v)?);
                    let key = &exp.thumbnails[i].key;
                    if exp.thumbnails[..i].iter().any(|t| &t.key == key) {
                        return Err(format!("`{at}` lists the key `{key}` twice"));
                    }
                }
            }
            _ => return Err(unknown(&at)),
        }
    }
    if exp.name.is_none() && exp.description.is_none() && exp.icon.is_none() && exp.thumbnails.is_empty() {
        return Err("[experience] must set at least one of name, description, icon or thumbnails".into());
    }
    Ok(exp)
}

fn thumbnail(path: &str, value: &toml::Value) -> Result<Thumbnail, String> {
    let mut thumb = Thumbnail { key: String::new(), image: String::new(), alt_text: None };
    for (k, v) in table(value, path)? {
        let at = format!("{path}.{k}");
        match k.as_str() {
            "key" => {
                thumb.key = string(v, &at)?;
                resource_key(&at, &thumb.key)?;
            }
            "image" => thumb.image = text(v, &at)?,
            "altText" => thumb.alt_text = Some(string(v, &at)?),
            _ => return Err(unknown(&at)),
        }
    }
    for (field, value) in [("key", &thumb.key), ("image", &thumb.image)] {
        if value.is_empty() {
            return Err(format!("`{path}.{field}` is missing"));
        }
    }
    Ok(thumb)
}

/// Whether `s` is an ASCII letter followed by letters, digits and `extra`.
fn is_name(s: &str, extra: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

fn resource_key(path: &str, key: &str) -> Result<(), String> {
    if is_name(key, "_.-") {
        return Ok(());
    }
    Err(format!("`{path}`: `{key}` must match [A-Za-z][A-Za-z0-9_.-]*"))
}

fn flag(value: &toml::Value, path: &str) -> Result<bool, String> {
    value.as_bool().ok_or_else(|| format!("`{path}` must be true or false"))
}

fn id(value: &toml::Value, path: &str) -> Result<u64, String> {
    let id = value.as_integer().filter(|&id| id > 0);
    id.map(|id| id as u64).ok_or_else(|| format!("`{path}` must be a positive id"))
}

/// A string with something in it besides whitespace.
fn text(value: &toml::Value, path: &str) -> Result<String, String> {
    let s = string(value, path)?;
    if s.trim().is_empty() {
        return Err(format!("`{path}` must not be blank"));
    }
    Ok(s)
}

const MOVED: &str = "[place] moved to properties files, place/*.toml; `opossum migrate` moves it there";

/// opossum.toml's text without its `[place]` sections, and the table they held; None when it has none. Every other
/// byte stays, comments included: the comments and blank lines a cut section ends with stay when they lead into a
/// section that stays, as its own. Only the blank lines a cut at the end of the file leaves trailing go. Err when
/// `[place]` is spelled in a way a section cut does not remove.
pub fn cut_place(text: &str) -> Result<Option<(String, toml::Table)>, String> {
    let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let Some(place) = doc.get("place") else { return Ok(None) };
    let place = table(place, "place")?.clone();
    let mut out = String::with_capacity(text.len());
    let mut cutting = false;
    // the comment and blank lines since the cut section's last key
    let mut held: Vec<&str> = Vec::new();
    for line in text.split_inclusive('\n') {
        // a header line parses on its own as the table it opens
        if line.trim_start().starts_with('[')
            && let Ok(header) = line.parse::<toml::Table>()
        {
            let place = header.contains_key("place");
            if cutting && !place {
                // the blank lines before them only set the cut section apart
                out.extend(held.iter().copied().skip_while(|l| l.trim().is_empty()));
            }
            held.clear();
            cutting = place;
        }
        let bare = line.trim();
        if !cutting {
            out.push_str(line);
        } else if bare.is_empty() || bare.starts_with('#') {
            held.push(line);
        } else {
            held.clear();
        }
    }
    if cutting {
        // the blank line that set the cut section apart now ends the file
        while out.ends_with("\n\n") {
            out.pop();
        }
    }
    let mut rest = doc;
    rest.remove("place");
    if out.parse::<toml::Table>().ok() != Some(rest) {
        return Err("[place] is not only whole sections; move it to place/*.toml by hand".into());
    }
    Ok(Some((out, place)))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table's rules, each case on top of a minimal valid file in a project holding `place/assets/animations`
    /// and `src/`: the error it gives, or "" when it loads.
    #[test]
    fn every_table_checks_its_keys_and_values() {
        let root = std::env::temp_dir().join(format!("opossum-config-{}", std::process::id()));
        std::fs::create_dir_all(root.join("place/assets/animations")).unwrap();
        std::fs::create_dir_all(root.join("src/art")).unwrap();
        let env = "[environments.dev]\nuniverseId = 1\nplaces = { main = 2 }\n";
        let user = "creator = { user = 1 }\n";
        let cases: &[(&str, &str)] = &[
            // creator
            ("creator = { user = 1 }", ""),
            ("creator = { group = 0 }", "`creator` must be { user = <id> } or { group = <id> }"),
            ("creator = { user = 1, group = 2 }", "`creator` must be"),
            ("creator = { team = 1 }", "`creator` must be"),
            ("[assets]\nassets = \"place/assets\"", "`creator` is required by [assets] and [badges]"),
            // assets: nested roots, same roots, where a root may be
            ("[assets]\nassets = \"place/assets\"\nanimations = \"place/assets/animations\"", "+creator"),
            (
                "[assets]\na = \"place/assets\"\nb = \"./place//assets/\"",
                "`assets.a` and `assets.b` have the same root",
            ),
            ("[assets]\na = \"place/assets/../assets\"", "+creator"),
            ("[assets]\na = \"place/missing\"", "`assets.a` must be a directory inside the project"),
            ("[assets]\na = \"../x\"", "`assets.a` must be a directory inside the project"),
            ("[assets]\na = \".\"", "`assets.a` must be a directory inside the project"),
            ("[assets]\na = \"src/art\"", "`assets.a` must be a directory inside the project"),
            ("[assets]\n\"a.b\" = \"place/assets\"", "`assets.a.b`: a set is the module"),
            ("[assets]\ntest = \"place/assets\"", "`assets.test`: a set is the module"),
            ("[assets]\na = 1", "`assets.a` must be a directory or a table with `root`"),
            (
                "[assets.a]\nroot = \"place/assets\"\nimage = { preset = \"badge-icon\", bleed = false }\ninclude = [\"**/*.png\"]\nexclude = [\"x/**\"]",
                "+creator",
            ),
            ("[assets.a]\ninclude = [\"*\"]", "`assets.a.root` is missing"),
            ("[assets.a]\nroot = \"place/assets\"\nkind = \"animation\"", "unknown key `assets.a.kind`"),
            (
                "[assets.a]\nroot = \"place/assets\"\nimage = { preset = \"huge\" }",
                "`assets.a.image.preset` must be one of: original, square-512,",
            ),
            ("[assets.a]\nroot = \"place/assets\"\nimage = { edge = \"repeat\" }", "unknown key `assets.a.image.edge`"),
            ("[assets.a]\nroot = \"place/assets\"\ninclude = []", "`assets.a.include` must list at least one glob"),
            (
                "[assets.a]\nroot = \"place/assets\"\nexclude = [\"../x\"]",
                "`assets.a.exclude`: `../x` must be a path pattern",
            ),
            ("[assets.a]\nroot = \"place/assets\"\nexclude = [\"[\"]", "`assets.a.exclude`: `[`:"),
            // places
            ("[places.main]\nprofile = \"release\"\n[profile.release]", ""),
            ("[places.main]\nprofile = \"release\"", "`places.main.profile` names no [profile.release]"),
            ("[places.a]\n[places.b]", "[places] must mark exactly one place `root = true`"),
            ("[places.a]\nroot = true\n[places.b]\nroot = true", "[places] must mark exactly one place"),
            ("[places.a]\nroot = true\n[places.b]", ""),
            ("[places.a]\nartifact = \"x.rbxl\"", "unknown key `places.a.artifact`"),
            ("[places.\"a.b\"]", "`places.a.b`: a place name must match"),
            ("[places]", "[places] declares no place"),
            // environments and the default
            (env, ""),
            ("[environments.dev]\nplaces = { main = 2 }", "`environments.dev.universeId` is missing"),
            (
                "[environments.dev]\nuniverseId = 0\nplaces = { main = 2 }",
                "`environments.dev.universeId` must be a positive id",
            ),
            ("[environments.dev]\nuniverseId = 1", "`environments.dev.places` must map the root place `main`"),
            (
                "[environments.dev]\nuniverseId = 1\nplaces = { other = 2 }",
                "`environments.dev.places.other` is not a declared place",
            ),
            (
                "[places.a]\nroot = true\n[places.b]\n[environments.dev]\nuniverseId = 1\nplaces = { b = 2 }",
                "must map the root place `a`",
            ),
            (
                "[places.a]\nroot = true\n[places.b]\n[environments.dev]\nuniverseId = 1\nplaces = { a = 2, b = 2 }",
                "`environments.dev.places` maps `a` and `b` to the same place id",
            ),
            (
                "[environments.dev]\nuniverseId = 1\nplaces = { main = 2 }\nrootPlaceId = 2",
                "unknown key `environments.dev.rootPlaceId`",
            ),
            ("defaultEnvironment = \"dev\"", "`defaultEnvironment` needs [environments]"),
            (
                "defaultEnvironment = \"prod\"\n[environments.dev]\nuniverseId = 1\nplaces = { main = 2 }",
                "`defaultEnvironment` must name one of: dev",
            ),
            (
                "[environments.a]\nuniverseId = 1\nplaces = { main = 2 }\n[environments.b]\nuniverseId = 3\nplaces = { main = 4 }",
                "`defaultEnvironment` must name one of: a, b",
            ),
            (
                "defaultEnvironment = \"b\"\n[environments.a]\nuniverseId = 1\nplaces = { main = 2 }\n[environments.b]\nuniverseId = 3\nplaces = { main = 4 }",
                "",
            ),
            (
                "placeId = 2\n[environments.dev]\nuniverseId = 1\nplaces = { main = 2 }",
                "`placeId` cannot be used with [environments]",
            ),
            // passes and products
            ("[passes.vip]\nname = \"VIP\"", "[passes] needs [environments]"),
            (
                "[passes.vip]\nname = \"VIP\"\ndescription = \"\"\nprice = 1000000000\nforSale = true\nmanagedPricing = false\nicon = \"vip.png\"",
                "+env",
            ),
            ("[products.\"coins-100.x\"]\nname = \"Coins\"", "+env"),
            ("[products.\"1coins\"]\nname = \"Coins\"", "`products.1coins`: `1coins` must match"),
            ("[passes.vip]\nname = \"  \"", "`passes.vip.name` must not be blank"),
            ("[passes.vip]\nprice = 5", "`passes.vip.name` is missing"),
            ("[passes.vip]\nname = \"VIP\"\nprice = 0", "`passes.vip.price` must be 1 to 1000000000 Robux"),
            ("[products.c]\nname = \"C\"\nprice = 1000000001", "`products.c.price` must be 1 to"),
            ("[passes.vip]\nname = \"VIP\"\nicon = \"\"", "`passes.vip.icon` must not be blank"),
            ("[passes.vip]\nname = \"VIP\"\nisForSale = true", "unknown key `passes.vip.isForSale`"),
            // badges
            (
                &format!("[badges.w]\nname = \"W\"\ndescription = \"d\"\nicon = \"w.png\"\n{env}"),
                "`creator` is required by",
            ),
            (
                "[badges.w]\nname = \"W\"\ndescription = \"d\"\nicon = \"w.png\"\nenabled = false\nexpectedCost = 100",
                "+creator+env",
            ),
            ("[badges.w]\nname = \"W\"\nicon = \"w.png\"", "`badges.w.description` is missing"),
            (
                "[badges.w]\nname = \"W\"\ndescription = \" \"\nicon = \"w.png\"",
                "`badges.w.description` must not be blank",
            ),
            ("[badges.w]\nname = \"W\"\ndescription = \"d\"", "`badges.w.icon` is missing"),
            (
                "[badges.w]\nname = \"W\"\ndescription = \"d\"\nicon = \"w.png\"\nexpectedCost = -1",
                "`badges.w.expectedCost` must be a Robux amount",
            ),
            // experience
            ("[experience]\ndescription = \"\"", "+env"),
            ("[experience]", "[experience] must set at least one of"),
            ("[experience]\nname = \"\"", "`experience.name` must not be blank"),
            ("[experience]\nthumbnailOrder = []", "unknown key `experience.thumbnailOrder`"),
            (
                "[[experience.thumbnails]]\nkey = \"a\"\nimage = \"a.png\"\naltText = \"\"\n[[experience.thumbnails]]\nkey = \"b\"\nimage = \"b.png\"",
                "+env",
            ),
            (
                "[[experience.thumbnails]]\nkey = \"a\"\nimage = \"a.png\"\n[[experience.thumbnails]]\nkey = \"a\"\nimage = \"b.png\"",
                "`experience.thumbnails` lists the key `a` twice",
            ),
            ("[[experience.thumbnails]]\nkey = \"a\"", "`experience.thumbnails[0].image` is missing"),
            ("[[experience.thumbnails]]\nimage = \"a.png\"", "`experience.thumbnails[0].key` is missing"),
            (
                "[experience]\nthumbnails = \"a.png\"",
                "`experience.thumbnails` must be [[experience.thumbnails]] tables",
            ),
            (
                &"[[experience.thumbnails]]\nkey = \"k\"\nimage = \"a.png\"\n".repeat(11),
                "holds 11 thumbnails; Roblox shows at most 10",
            ),
        ];
        let mut failed = Vec::new();
        for &(case, want) in cases {
            // "+creator", "+env" or both: loads once given those
            let (user, env, want) = match want.starts_with('+') {
                true => {
                    (if want.contains("creator") { user } else { "" }, if want.contains("env") { env } else { "" }, "")
                }
                false => ("", "", want),
            };
            // top-level keys go before the first table header
            let (top, tables) = match case.find('[') {
                Some(at) => case.split_at(at),
                None => (case, ""),
            };
            let routes = "[routes]\nshared = \"ReplicatedStorage/shared\"";
            let text = format!("name = \"x\"\n{user}{top}\n{routes}\n{tables}\n{env}");
            let got = match parse(&text, root.clone()) {
                Ok(_) => String::new(),
                Err(e) => e,
            };
            if want.is_empty() != got.is_empty() || !got.contains(want) {
                failed.push(format!("{case:?}\n  want {want:?}\n  got  {got:?}"));
            }
        }
        std::fs::remove_dir_all(&root).unwrap();
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    #[test]
    fn environments_set_the_studio_place_and_nested_sets_own_their_subtree() {
        let root = std::env::temp_dir().join(format!("opossum-config-env-{}", std::process::id()));
        std::fs::create_dir_all(root.join("place/assets/animations")).unwrap();
        let text = r#"name = "x"
creator = { group = 7 }
defaultEnvironment = "prod"
[routes]
shared = "ReplicatedStorage/shared"
[assets]
assets = { root = "place/assets", exclude = ["**/*.txt"] }
animations = "place/assets/animations/"
[places.lobby]
[places.main]
root = true
[environments.dev]
universeId = 1
places = { main = 2 }
[environments.prod]
universeId = 3
places = { lobby = 5, main = 4 }
"#;
        let config = parse(text, root.clone());
        std::fs::remove_dir_all(&root).unwrap();
        let mut config = config.unwrap();
        assert_eq!(config.creator, Some(Creator::Group(7)));
        assert_eq!(config.place, Some(4));
        assert_eq!(config.environment(None).unwrap().universe, 3);
        assert_eq!(config.environment(Some("dev")).unwrap().universe, 1);
        assert!(config.environment(Some("qa")).is_err_and(|e| e.contains("choose one of: dev, prod")));
        let [assets, animations] = &config.assets[..] else { panic!() };
        assert_eq!((assets.root.as_str(), animations.root.as_str()), ("place/assets", "place/assets/animations"));
        assert_eq!(animations.image, ImageOptions::DEFAULT);
        // globs are git's: `*` stays in its segment
        assert!(assets.selects("a/b.png") && !assets.selects("a/b.txt") && animations.selects("x.rbxmx"));
        let star = AssetSet { include: Some(glob_set(&["*.png".into()], "t").unwrap()), ..config.assets.remove(1) };
        assert!(star.selects("a.png") && !star.selects("a/b.png"));
    }

    #[test]
    fn cutting_place_keeps_the_rest_byte_for_byte() {
        let text = r#"name = "x"  # the name

[place.Lighting]   # sky
ClockTime = 16.9

[place."Workspace/Terrain"]
# the cut takes this comment with its section
WaterColor = [1, 2, 3]

# but leaves this one, which leads into [routes]
[routes]
# shared code
shared = "ReplicatedStorage/shared"

[place]
HttpService = { HttpEnabled = true }
"#;
        let kept = r#"name = "x"  # the name

# but leaves this one, which leads into [routes]
[routes]
# shared code
shared = "ReplicatedStorage/shared"
"#;
        let (rest, place) = cut_place(text).unwrap().unwrap();
        assert_eq!(rest, kept);
        assert_eq!(place.keys().collect::<Vec<_>>(), ["Lighting", "Workspace/Terrain", "HttpService"]);
        assert!(cut_place(&rest).unwrap().is_none());
    }
}
