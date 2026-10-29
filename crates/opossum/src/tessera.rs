//! Tessera's formats, read once by `opossum migrate` and by nothing else: `tessera.config.ts` as literal data (never
//! evaluated), `tessera.lock` (gzipped JSON behind a magic), and tessera's input fingerprint, which says whether a
//! recorded id still holds a file as it is now. This file goes once no project is left on tessera.

use std::io::Read as _;

use cloud::Creator;
use media::{ImageOptions, Preset};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};
use tsrs_ast::{Kind, Node, SourceFileParseOptions};
use tsrs_core::P;
use tsrs_core::ScriptKind;

/// A literal of tessera.config.ts and the byte offset it starts at, for a refusal's position.
pub struct Lit {
    pub at: u32,
    pub value: Value,
}

pub enum Value {
    Str(String),
    /// As written: ids, prices and costs are parsed where their field is known.
    Num(String),
    Bool(bool),
    List(Vec<Lit>),
    /// In source order.
    Object(Vec<(String, Lit)>),
}

const EVALUATE: &str = "opossum does not evaluate TypeScript; move this field by hand";

/// The object `export default` hands tessera, directly or through `defineConfig(...)`. Only string, number and
/// boolean literals, `as const`, and arrays and objects of those are read; anything else is refused at its offset.
pub fn config(text: &str) -> Result<Lit, (u32, String)> {
    let opts = SourceFileParseOptions { file_name: "/tessera.config.ts".into(), ..Default::default() };
    let file = tsrs_parser::parse_source_file(opts, text, ScriptKind::TS);
    if let Some(d) = file.diagnostics().first() {
        return Err((d.pos().unwrap_or(0), d.localize()));
    }
    let exported = file
        .as_node()
        .statements()
        .iter()
        .copied()
        .find(|s| s.kind() == Kind::ExportAssignment && !s.as_export_assignment().is_export_equals);
    let Some(exported) = exported.and_then(|s| s.expression()) else {
        return Err((0, "tessera.config.ts has no `export default`".into()));
    };
    let mut config = exported;
    if config.kind() == Kind::CallExpression
        && config.expression().is_some_and(|f| f.kind() == Kind::Identifier && f.text() == "defineConfig")
        && let [arg] = config.arguments()
    {
        config = *arg;
    }
    literal(text, config)
}

/// A parsed node's bytes in `text`, its leading trivia left out.
pub fn span(text: &str, node: P<Node>) -> (usize, usize) {
    let pos = tsrs_scanner::skip_trivia(text, node.pos().source_offset().unwrap_or(0));
    (pos as usize, node.end().source_offset().unwrap_or(pos) as usize)
}

fn literal(text: &str, node: P<Node>) -> Result<Lit, (u32, String)> {
    let at = span(text, node).0 as u32;
    let value = match node.kind() {
        Kind::ParenthesizedExpression | Kind::SatisfiesExpression => {
            return literal(text, node.expression().unwrap());
        }
        Kind::AsExpression => {
            let (start, end) = span(text, node.type_node().unwrap());
            if &text[start..end] != "const" {
                return Err((at, EVALUATE.to_owned()));
            }
            return literal(text, node.expression().unwrap());
        }
        Kind::StringLiteral | Kind::NoSubstitutionTemplateLiteral => Value::Str(node.text().to_owned()),
        Kind::NumericLiteral => Value::Num(node.text().to_owned()),
        Kind::TrueKeyword => Value::Bool(true),
        Kind::FalseKeyword => Value::Bool(false),
        Kind::ArrayLiteralExpression => {
            Value::List(node.elements().iter().map(|&e| literal(text, e)).collect::<Result<_, _>>()?)
        }
        Kind::ObjectLiteralExpression => {
            let mut fields = Vec::new();
            for &p in node.properties() {
                let name = p
                    .name()
                    .filter(|n| matches!(n.kind(), Kind::Identifier | Kind::StringLiteral | Kind::NumericLiteral));
                match (p.kind(), name) {
                    (Kind::PropertyAssignment, Some(name)) => {
                        fields.push((name.text().to_owned(), literal(text, p.initializer().unwrap())?));
                    }
                    _ => return Err((span(text, p).0 as u32, EVALUATE.to_owned())),
                }
            }
            Value::Object(fields)
        }
        _ => return Err((at, EVALUATE.to_owned())),
    };
    Ok(Lit { at, value })
}

/// What tessera recorded for one uploaded asset.
pub struct Record {
    /// `<set>/<logical path>`, the logical path without its extension.
    pub key: String,
    pub id: u64,
    /// The input fingerprint of what `id` holds.
    pub fingerprint: String,
    /// Uploaded as an Animation (its set's kind was `animation`).
    pub animation: bool,
    /// Who uploaded it; None for a record from before tessera stored that.
    pub creator: Option<Creator>,
}

const MAGIC: &[u8] = b"TESSERA\0L\x01";

/// The asset records of a `tessera.lock`, or every reason it cannot be migrated as it is: an upload or create still
/// in flight, a provisioned resource, anything that is not a finished asset upload.
pub fn lock(bytes: &[u8]) -> Result<Vec<Record>, Vec<String>> {
    let json = decode(bytes).map_err(|e| vec![e])?;
    let mut records = Vec::new();
    let mut refusals = Vec::new();
    for file in json["files"].as_array().into_iter().flatten() {
        let global = file["scope"]["kind"] == "global";
        for r in file["resources"].as_array().into_iter().flatten() {
            let (kind, key) = (r["kind"].as_str().unwrap_or_default(), r["key"].as_str().unwrap_or_default());
            if !global {
                let scope = file["scope"]["name"].as_str().unwrap_or_default();
                refusals.push(format!(
                    "environment {scope} holds {kind} {key}: migrate supports locks without provisioned resources; \
                     provision them again with opossum or bind their ids with `opossum adopt`"
                ));
                continue;
            }
            if kind != "asset" {
                refusals.push(format!("{kind} {key} is not an asset; finish or remove it with tessera first"));
                continue;
            }
            if r["pendingOperations"].as_array().is_some_and(|p| !p.is_empty()) {
                refusals.push(format!("{key} has an upload in flight; finish or remove it with tessera first"));
                continue;
            }
            let fields = &r["applied"]["fields"];
            let id = r["remoteIds"]["asset"].as_str().and_then(|id| id.parse().ok()).filter(|&id| id > 0);
            let fingerprint = fields["inputFingerprint"].as_str().filter(|f| !f.is_empty());
            let creator = match (fields["creatorKind"].as_str(), fields["creatorId"].as_str().map(str::parse)) {
                (None, None) => Ok(None),
                (Some("user"), Some(Ok(id))) => Ok(Some(Creator::User(id))),
                (Some("group"), Some(Ok(id))) => Ok(Some(Creator::Group(id))),
                _ => Err(()),
            };
            match (id, fingerprint, creator, key.split_once('/')) {
                (Some(id), Some(fingerprint), Ok(creator), Some((_, logical))) if !logical.is_empty() => {
                    let animation = fields["kind"] == "animation";
                    records.push(Record {
                        key: key.to_owned(),
                        id,
                        fingerprint: fingerprint.to_owned(),
                        animation,
                        creator,
                    });
                }
                _ => refusals.push(format!("{key} is not a finished upload; remove it with tessera first")),
            }
        }
    }
    if refusals.is_empty() { Ok(records) } else { Err(refusals) }
}

fn decode(bytes: &[u8]) -> Result<Json, String> {
    let gz = bytes.strip_prefix(MAGIC).ok_or("tessera.lock does not start with tessera's header")?;
    let mut text = String::new();
    flate2::read::MultiGzDecoder::new(gz)
        .read_to_string(&mut text)
        .map_err(|e| format!("tessera.lock does not decompress: {e}"))?;
    let json: Json = serde_json::from_str(&text).map_err(|e| format!("tessera.lock is not JSON: {e}"))?;
    if json["version"] != 1 || !json["files"].is_array() {
        return Err("tessera.lock is not version 1 with a list of files".into());
    }
    Ok(json)
}

/// Tessera's input fingerprint of a file with these bytes and lowercase extension in a set with these image options:
/// sha256 over the source's digest and its transform, which for an image names tessera's processor and the options.
/// Only files opossum discovers are fingerprinted, so only its extensions arrive.
pub fn fingerprint(bytes: &[u8], ext: &str, image: ImageOptions) -> String {
    let transform = match ext {
        ".png" | ".svg" | ".jpg" | ".jpeg" => format!("image-v1;source={ext};options={}", options(image)),
        _ => format!("passthrough-v2;extension={ext};mime={}", mime(ext)),
    };
    let mut h = Sha256::new();
    h.update(b"tessera-asset-input-v1\0");
    h.update(Sha256::digest(bytes));
    h.update([0]);
    h.update(transform.as_bytes());
    hex(&h.finalize())
}

/// sha256 of the JSON Go's encoder wrote for tessera 0.2.0's image processor (built with go1.26.5) and these options
/// normalized as tessera normalized them: a preset's box filled in, contained, transparent edge, scale 1.
fn options(image: ImageOptions) -> String {
    let size = match image.preset.size() {
        Some((w, h)) => format!(r#""width":{w},"height":{h},"#),
        None => String::new(),
    };
    let bleed = if image.bleed { r#""alphaBleed":true,"# } else { "" };
    let json = format!(
        r#"{{"version":"image-v3;oksvg=be6e8873101c;rasterx=2ab79fcdd4ef;ximage=0.44.0;png=stdlib","go":"go1.26.5","transform":{{"preset":"{}",{size}"fit":"contain","svgScale":1,{bleed}"edge":"transparent"}}}}"#,
        image.preset.name()
    );
    hex(&Sha256::digest(json.as_bytes()))
}

/// Tessera 0.2.0's MIME for the passthrough extensions opossum discovers, which its fingerprint names. Frozen with
/// tessera: opossum's own upload types may change, these never do, or every migrated id would read as changed.
fn mime(ext: &str) -> &'static str {
    match ext {
        ".ogg" => "audio/ogg",
        ".mp3" => "audio/mpeg",
        ".wav" => "audio/wav",
        ".flac" => "audio/flac",
        ".glb" => "model/gltf-binary",
        ".mp4" => "video/mp4",
        ".mov" => "video/mov",
        ".rbxm" | ".rbxmx" => "model/x-rbxm",
        _ => "application/octet-stream",
    }
}

/// Whether a tessera image `width` × `height` is what opossum's `preset` makes: the preset's own box, or none.
pub fn same_box(preset: Preset, width: u64, height: u64) -> bool {
    match preset.size() {
        Some((w, h)) => (width, height) == (0, 0) || (width, height) == (u64::from(w), u64::from(h)),
        None => (width, height) == (0, 0),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_is_tessera_s_to_the_byte() {
        // the options hash tessera wrote into a project's lock for { preset: 'original', bleed: true }
        assert_eq!(options(ImageOptions::DEFAULT), "741311f1dd59e719e3e14fd04b16f27606025c7989d724fdb2c443a57d68b997");
        // the rest hand-computed with python's hashlib over the same formula
        let square = ImageOptions { preset: Preset::Square512, bleed: false };
        assert_eq!(options(square), "6a69c1646ce473c9ceb906a73127e82ac49011e4b8a157254d2265b6346908cb");
        let png = fingerprint(b"png bytes", ".png", ImageOptions::DEFAULT);
        assert_eq!(png, "2e406efc045abf0ffc6c4ef703960b0768ed78d7a540be19dd998aabcfe4f1e2");
        let rbxmx = fingerprint(b"<roblox/>", ".rbxmx", ImageOptions::DEFAULT);
        assert_eq!(rbxmx, "57b9f64f6dd4b99a3198ed289d3c189f93be9157b13b1ba13e10ed93829cad85");
    }
}
