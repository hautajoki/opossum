//! Asset sets: which files each `[assets]` root holds and under which key, what a file goes up to Roblox as, its input
//! hash, and the bytes its upload sends. Discovery reads directory entries only, so every build runs it; hashing and
//! compiling read bytes and run only in the commands that upload or plan.

use std::fs;
use std::path::Path;

use cloud::AssetKind;
use media::{Image, ImageOptions};

use crate::config::AssetSet;
use crate::diag::{Diag, Severity};

pub struct Asset {
    /// Its set's index.
    pub set: usize,
    /// Project-relative and `/`-separated, extension included: its key in the lock.
    pub file: String,
    /// The path under the set root without its last extension: its key in the module.
    pub key: String,
    /// Lowercase, with its dot.
    pub ext: String,
}

/// Every extension opossum uploads and the asset type its file goes up as (an `.rbxm`/`.rbxmx` may turn out to be an
/// Animation, see [`kind`]). `.bmp` and `.tga` are not here: convert them to PNG. A `.glb` is the only model file
/// format, since its part names are checked before upload ([`media::glb`]).
fn supported(ext: &str) -> Option<(&'static str, AssetKind)> {
    use AssetKind::*;
    let all = [
        (".png", Image),
        (".svg", Image),
        (".jpg", Image),
        (".jpeg", Image),
        (".ogg", Audio),
        (".mp3", Audio),
        (".wav", Audio),
        (".flac", Audio),
        (".mp4", Video),
        (".mov", Video),
        (".glb", Model),
        (".rbxm", Model),
        (".rbxmx", Model),
    ];
    all.into_iter().find(|&(e, _)| e == ext)
}

/// The files of every set, each set's sorted by key. Dot-files and dot-directories are skipped, as is any subtree
/// that is another set's root: the innermost root owns its files. Errors (a root that cannot be read, a file opossum
/// cannot upload, an `.fbx` or `.gltf` model, two files with one key, a key that is also a directory) are build
/// errors, and the files they name are left out.
pub fn discover(root: &Path, sets: &[AssetSet]) -> (Vec<Asset>, Vec<Diag>) {
    let mut assets = Vec::new();
    let mut diags = Vec::new();
    for (i, set) in sets.iter().enumerate() {
        let error =
            |code: &str, file: &str, message: String| Diag::new(code, Severity::Error, Some(&root.join(file)), message);
        let others: Vec<&str> = sets.iter().map(|s| s.root.as_str()).filter(|r| *r != set.root).collect();
        let mut files = Vec::new();
        walk(root, &set.root, "", &others, &mut files, &mut diags);
        let mut found: Vec<Asset> = Vec::new();
        for rel in files {
            if !set.selects(&rel) {
                continue;
            }
            let file = join(&set.root, &rel);
            let name = rel.rsplit('/').next().unwrap_or(&rel);
            let ext = name.rfind('.').map_or(String::new(), |dot| name[dot..].to_ascii_lowercase());
            if ext == ".fbx" || ext == ".gltf" {
                let message = format!("{file}: export a glb; opossum uploads models as .glb only");
                diags.push(error("asset.model", &file, message));
                continue;
            }
            if supported(&ext).is_none() {
                let what = if ext.is_empty() { "a file with no extension".to_owned() } else { ext };
                let message =
                    format!("{file}: opossum cannot upload {what}; move it out of the asset root or exclude it");
                diags.push(error("asset.unsupported", &file, message));
                continue;
            }
            let key = rel[..rel.len() - ext.len()].to_owned();
            found.push(Asset { set: i, file, key, ext });
        }
        found.sort_by(|a, b| (&a.key, &a.file).cmp(&(&b.key, &b.file)));
        let mut kept: Vec<Asset> = Vec::new();
        for a in found {
            match kept.last() {
                Some(first) if first.key == a.key => {
                    let message =
                        format!("{} and {} are both `{}` in set `{}`; rename one", first.file, a.file, a.key, set.name);
                    diags.push(error("asset.collision", &a.file, message));
                }
                _ => kept.push(a),
            }
        }
        // a key that is also a directory of keys: `decal.png` beside `decal/color.png`
        let keys: std::collections::HashSet<&str> = kept.iter().map(|a| a.key.as_str()).collect();
        let clashes: Vec<Option<String>> = kept
            .iter()
            .map(|a| {
                a.key.match_indices('/').map(|(at, _)| &a.key[..at]).find(|dir| keys.contains(dir)).map(Into::into)
            })
            .collect();
        for (a, clash) in kept.into_iter().zip(clashes) {
            match clash {
                Some(dir) => {
                    let message =
                        format!("{} is under `{dir}`, which is also a file's key in set `{}`", a.file, set.name);
                    diags.push(error("asset.treeClash", &a.file, message));
                }
                None => assets.push(a),
            }
        }
    }
    (assets, diags)
}

/// `dir/rel` as a project path, where an empty or `.` dir is the project root and an empty rel the dir itself.
pub fn join(dir: &str, rel: &str) -> String {
    match (dir, rel) {
        ("" | ".", rel) => rel.to_owned(),
        (dir, "") => dir.to_owned(),
        (dir, rel) => format!("{dir}/{rel}"),
    }
}

/// Collects the regular files under `<root>/<set_root>/<rel>`, paths relative to the set root, in bytewise order.
fn walk(root: &Path, set_root: &str, rel: &str, others: &[&str], out: &mut Vec<String>, diags: &mut Vec<Diag>) {
    let dir = join(set_root, rel);
    let unreadable = |e: std::io::Error| {
        Diag::new("asset.root", Severity::Error, Some(&root.join(&dir)), format!("{dir} cannot be read: {e}"))
    };
    let read = match fs::read_dir(root.join(&dir)) {
        Ok(read) => read,
        Err(e) => return diags.push(unreadable(e)),
    };
    let mut entries: Vec<(String, bool)> = Vec::new();
    for entry in read {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                diags.push(unreadable(e));
                continue;
            }
        };
        let Ok(name) = entry.file_name().into_string() else {
            let message = format!("{dir} holds a file whose name is not UTF-8; rename it");
            diags.push(Diag::new("asset.unsupported", Severity::Error, Some(&entry.path()), message));
            continue;
        };
        // symbolic links are skipped: file_type does not follow them
        match entry.file_type() {
            _ if name.starts_with('.') => {}
            Ok(t) if t.is_dir() || t.is_file() => entries.push((name, t.is_dir())),
            _ => {}
        }
    }
    entries.sort();
    for (name, is_dir) in entries {
        let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if !is_dir {
            out.push(rel);
        } else if !others.contains(&join(set_root, &rel).as_str()) {
            walk(root, set_root, &rel, others, out, diags);
        }
    }
}

/// A file's input hash: 32 hex characters naming its bytes, its extension and the pipeline those bytes go through
/// under its set's image options. Equal hashes mean an upload of one serves the other; a changed option re-uploads
/// exactly the files it changes.
pub fn hash(ext: &str, image: ImageOptions, bytes: &[u8]) -> String {
    let pipeline = media::pipeline(ext, image).unwrap_or_else(|| "raw".to_owned());
    let mut h = blake3::Hasher::new();
    h.update(b"opossum-asset-1\0");
    h.update(pipeline.as_bytes());
    h.update(b"\0");
    h.update(ext.as_bytes());
    h.update(b"\0");
    h.update(blake3::hash(bytes).as_bytes());
    h.finalize().as_bytes()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Every asset's input hash, read and hashed in parallel, in `assets` order; a file that cannot be read is an error
/// in its slot.
pub fn hash_all(root: &Path, sets: &[AssetSet], assets: &[Asset]) -> Vec<Result<String, String>> {
    lower::parallel::map(assets, |a| {
        let bytes = fs::read(root.join(&a.file)).map_err(|e| format!("{}: {e}", a.file))?;
        Ok(hash(&a.ext, sets[a.set].image, &bytes))
    })
}

/// What a file goes up as. Only `.rbxm`/`.rbxmx` needs its bytes: exactly one top-level KeyframeSequence or
/// CurveAnimation is an Animation (a keyframe sequence uploaded as a Model is never what anyone wants), anything else
/// a Model.
pub fn kind(ext: &str, bytes: &[u8]) -> Result<AssetKind, String> {
    let xml = match ext {
        ".rbxm" => false,
        ".rbxmx" => true,
        _ => return supported(ext).map(|(_, kind)| kind).ok_or(format!("opossum cannot upload {ext}")),
    };
    let classes = place::model_classes(bytes, xml)?;
    Ok(match &classes[..] {
        [class] if class == "KeyframeSequence" || class == "CurveAnimation" => AssetKind::Animation,
        _ => AssetKind::Model,
    })
}

/// What an upload sends for one file.
pub struct Compiled {
    pub kind: AssetKind,
    /// The output's extension (`.png` for a processed SVG) and MIME type.
    pub ext: &'static str,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// Turns a file's bytes into what its upload sends, refusing what Roblox would refuse and opossum can tell before
/// sending: the size, image dimensions, audio length, rate and channels, a model that does not decode, a glb's part
/// names. `path` names the file in the diagnostic.
pub fn compile(path: &Path, ext: &str, image: ImageOptions, bytes: Vec<u8>) -> Result<Compiled, Diag> {
    let fail = |code: &str, message: String| Diag::new(code, Severity::Error, Some(path), message);
    let Some((ext, kind)) = supported(ext) else {
        return Err(fail("asset.unsupported", format!("opossum cannot upload {ext}")));
    };
    let size = |n: usize| match n {
        0 => Err(fail("asset.tooLarge", "the file is empty; Roblox takes no empty asset".into())),
        n if n > cloud::MAX_ASSET => {
            Err(fail("asset.tooLarge", format!("{n} bytes is over Roblox's 20 MB limit for an asset")))
        }
        _ => Ok(()),
    };
    size(bytes.len())?;
    let mime = media::mime(ext).unwrap_or("application/octet-stream");
    let mut out = Compiled { kind, ext, mime, bytes };
    match kind {
        AssetKind::Image => {
            let (w, h) = match media::process(ext, &out.bytes, image).map_err(|e| fail("asset.image", e))? {
                Image::Unchanged { w, h } => (w, h),
                Image::Png { bytes, w, h } => {
                    (out.ext, out.mime, out.bytes) = (".png", "image/png", bytes);
                    size(out.bytes.len())?;
                    (w, h)
                }
            };
            if w > media::MAX_SIDE || h > media::MAX_SIDE {
                let message = format!("{w}x{h} pixels; Roblox takes images under 8000 pixels on each side");
                return Err(fail("asset.tooLarge", message));
            }
        }
        AssetKind::Audio => {
            media::inspect_audio(ext, &out.bytes).map_err(|e| fail("asset.audio", e))?;
        }
        AssetKind::Model => {
            if ext == ".glb" {
                media::glb(&out.bytes).map_err(|e| fail("asset.model", e))?;
            }
            out.kind = self::kind(ext, &out.bytes).map_err(|e| fail("asset.model", e))?;
        }
        AssetKind::Video | AssetKind::Animation => {}
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use media::Preset;

    use super::*;
    use crate::config::glob_set;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("opossum-assets-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set(name: &str, root: &str, include: &[&str], exclude: &[&str]) -> AssetSet {
        let globs = |p: &[&str]| glob_set(&p.iter().map(|s| s.to_string()).collect::<Vec<_>>(), name).unwrap();
        AssetSet {
            name: name.into(),
            root: root.into(),
            image: ImageOptions::DEFAULT,
            include: Some(globs(include)),
            exclude: globs(exclude),
        }
    }

    #[test]
    fn discovery_gives_nested_roots_their_own_files_and_reports_what_it_cannot_key() {
        let root = temp("discover");
        for file in [
            "place/assets/icons/Play.SVG",
            "place/assets/icons/.DS_Store",
            "place/assets/.cache/x.png",
            "place/assets/sfx/hit.ogg",
            "place/assets/sfx/hit.mp3",
            "place/assets/notes.txt",
            "place/assets/LICENSE",
            "place/assets/models/hull.fbx",
            "place/assets/raw/skip.png",
            "place/assets/decal.png",
            "place/assets/decal/color.png",
            "place/assets/animations/walk.rbxmx",
            "place/assets/animations/sub/run.rbxmx",
        ] {
            fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
            fs::write(root.join(file), "").unwrap();
        }
        let sets = [
            set("assets", "place/assets", &["**/*"], &["raw/**"]),
            set("animations", "place/assets/animations", &["**/*.rbxmx"], &[]),
            set("missing", "place/missing", &["**/*"], &[]),
        ];
        let (assets, diags) = discover(&root, &sets);
        let got: Vec<(usize, &str, &str, &str)> =
            assets.iter().map(|a| (a.set, a.file.as_str(), a.key.as_str(), a.ext.as_str())).collect();
        assert_eq!(
            got,
            [
                (0, "place/assets/decal.png", "decal", ".png"),
                (0, "place/assets/icons/Play.SVG", "icons/Play", ".svg"),
                (0, "place/assets/sfx/hit.mp3", "sfx/hit", ".mp3"),
                (1, "place/assets/animations/sub/run.rbxmx", "sub/run", ".rbxmx"),
                (1, "place/assets/animations/walk.rbxmx", "walk", ".rbxmx"),
            ]
        );
        let codes: Vec<(&str, String)> = diags
            .iter()
            .map(|d| (d.code.as_str(), d.file.as_ref().unwrap().strip_prefix(&root).unwrap().display().to_string()))
            .collect();
        assert_eq!(
            codes,
            [
                ("asset.unsupported", "place/assets/LICENSE".to_owned()),
                ("asset.model", "place/assets/models/hull.fbx".to_owned()),
                ("asset.unsupported", "place/assets/notes.txt".to_owned()),
                ("asset.collision", "place/assets/sfx/hit.ogg".to_owned()),
                ("asset.treeClash", "place/assets/decal/color.png".to_owned()),
                ("asset.root", "place/missing".to_owned()),
            ]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_model_file_is_an_animation_only_when_it_holds_one_keyframe_sequence() {
        let item = |class: &str, n: u32| {
            format!(
                "<Item class=\"{class}\" referent=\"RBX{n}\"><Properties><string name=\"Name\">x</string></Properties></Item>"
            )
        };
        let file = |items: &str| format!("<roblox version=\"4\">{items}</roblox>");
        let sequence = file(&item("KeyframeSequence", 0));
        assert_eq!(kind(".rbxmx", sequence.as_bytes()), Ok(AssetKind::Animation));
        assert_eq!(kind(".rbxmx", file(&item("Model", 0)).as_bytes()), Ok(AssetKind::Model));
        let two = file(&(item("KeyframeSequence", 0) + &item("KeyframeSequence", 1)));
        assert_eq!(kind(".rbxmx", two.as_bytes()), Ok(AssetKind::Model));
        assert!(kind(".rbxmx", b"<roblox").is_err());
        let compiled = compile(Path::new("walk.rbxmx"), ".rbxmx", ImageOptions::DEFAULT, sequence.into_bytes());
        assert_eq!(compiled.map(|c| (c.kind, c.mime)).ok(), Some((AssetKind::Animation, "model/x-rbxm")));
    }

    #[test]
    fn the_image_options_are_in_the_hash_of_exactly_the_files_they_change() {
        let square = ImageOptions { preset: Preset::Square512, bleed: true };
        let unbled = ImageOptions { preset: Preset::Original, bleed: false };
        let all = [ImageOptions::DEFAULT, square, unbled];
        let hashes = |ext: &str| all.map(|o| hash(ext, o, b"bytes"));
        let png = hashes(".png");
        assert!(png[0] != png[1] && png[0] != png[2] && png[1] != png[2]);
        // a JPEG goes up as it is unless a preset resizes it, and nothing else is ever processed
        let jpg = hashes(".jpg");
        assert!(jpg[0] == jpg[2] && jpg[0] != jpg[1]);
        let ogg = hashes(".ogg");
        assert!(ogg[0] == ogg[1] && ogg[0] == ogg[2]);
        // the extension is in it too: the same bytes as another type are another upload
        assert_ne!(hash(".glb", ImageOptions::DEFAULT, b"bytes"), ogg[0]);
        assert_eq!(png[0].len(), 32);
    }
}
