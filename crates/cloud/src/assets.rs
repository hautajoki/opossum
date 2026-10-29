//! Assets: create, the operation it returns, and reading an asset's content back.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use serde_json::{Value, json};

use crate::{BODY_LIMIT, Call, Client, Error, ErrorKind, Part, absolute, backoff, code, id, multipart};

/// Roblox's limit on an asset file.
pub const MAX_ASSET: usize = 20_000_000;
/// How long [`Client::wait_operation`] polls one operation before it gives the file back as pending.
pub const OPERATION_DEADLINE: Duration = Duration::from_secs(600);
/// How long [`Client::asset_content`] retries asset delivery's 404: a model may still be processing right after its
/// operation is done.
const DELIVERY_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    Image,
    Audio,
    Video,
    Model,
    Animation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Creator {
    User(u64),
    Group(u64),
}

/// One file to upload. `ext` is the output's extension with its dot (`.png` for a processed SVG), which names the file part `asset<ext>`.
pub struct Upload<'a> {
    pub kind: AssetKind,
    /// Its display name on Roblox, cut to fit by [`display_name`].
    pub name: &'a str,
    /// Its description on Roblox.
    pub description: &'a str,
    pub creator: Creator,
    pub ext: &'static str,
    pub mime: &'static str,
    pub bytes: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    /// `operations/<id>`, validated, so polling it can never steer the key elsewhere.
    pub path: String,
    pub done: bool,
    /// Done with an error: Roblox's code (leniently decoded) and message. Terminal.
    pub error: Option<(String, String)>,
    pub asset_id: Option<u64>,
    /// `moderationResult.moderationState`: `Approved`, `Reviewing` or `Rejected` (a rejected id must never be bound).
    pub moderation: Option<String>,
}

impl Operation {
    /// Moderation rejected the asset, however Roblox spells the state (`Rejected`, `MODERATION_STATE_REJECTED`).
    pub fn rejected(&self) -> bool {
        self.moderated("REJECTED")
    }

    /// Moderation is still reviewing the asset.
    pub fn reviewing(&self) -> bool {
        self.moderated("REVIEWING")
    }

    fn moderated(&self, state: &str) -> bool {
        self.moderation.as_deref().is_some_and(|m| m.to_ascii_uppercase().ends_with(state))
    }
}

impl AssetKind {
    pub fn name(self) -> &'static str {
        match self {
            AssetKind::Image => "Image",
            AssetKind::Audio => "Audio",
            AssetKind::Video => "Video",
            AssetKind::Model => "Model",
            AssetKind::Animation => "Animation",
        }
    }
}

const CREATE: &str = "create asset";
const OPERATION: &str = "get asset operation";
const DELIVERY: &str = "get asset location";
const CONTENT: &str = "get asset content";

impl Client {
    /// Creates an asset. Not idempotent: the caller saves its intent first, and an ambiguous error keeps it.
    pub fn create_asset(&self, u: &Upload) -> Result<Operation, Error> {
        let request = json!({
            "assetType": u.kind.name(),
            "displayName": display_name(u.name),
            "description": u.description,
            "creationContext": {"creator": creator(u.creator), "expectedPrice": 0},
        })
        .to_string();
        let filename = format!("asset{}", u.ext);
        let parts = [
            Part { name: "request", filename: "", mime: "", bytes: request.as_bytes() },
            Part { name: "fileContent", filename: &filename, mime: u.mime, bytes: u.bytes },
        ];
        let (mime, body) = multipart(&parts);
        let call = Call {
            op: CREATE,
            method: "POST",
            path: "assets/v1/assets".into(),
            body: Some((&mime, &body)),
            create: true,
            long: true,
        };
        let (Creator::User(n) | Creator::Group(n)) = u.creator;
        let invalid = if u.bytes.is_empty() || u.bytes.len() > MAX_ASSET {
            Some(format!("the file is {} bytes; Roblox takes 1 to {MAX_ASSET}", u.bytes.len()))
        } else if n == 0 {
            Some("the creator id must be positive".into())
        } else if !u
            .ext
            .strip_prefix('.')
            .is_some_and(|e| !e.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        {
            Some(format!("{:?} is not a file extension", u.ext))
        } else {
            None
        };
        if let Some(message) = invalid {
            return Err(self.error(&call, ErrorKind::Local, message));
        }
        let created = self.json(&call).map_err(|mut e| {
            if e.kind == ErrorKind::Status(429) && u.kind == AssetKind::Audio {
                e.message += " (Roblox limits audio uploads per account, and the quota depends on the account)";
            }
            e
        })?;
        let path = created["path"].as_str().unwrap_or("");
        if !operation_path(path) {
            return Err(self.error(
                &call,
                ErrorKind::Decode,
                format!("the operation path {path:?} is not operations/<id>"),
            ));
        }
        Ok(decode(path, &created))
    }

    /// One look at an operation. A 404 means it expired (operations do not live forever): the caller no longer knows
    /// whether its asset exists.
    pub fn operation(&self, path: &str) -> Result<Operation, Error> {
        self.poll_operation(path, &mut Duration::default())
    }

    /// Polls an operation every 1 s doubling to 5 s until it is done, `deadline` passes or `stop` is set, and returns
    /// it either way: one that is not `done` is still pending and can be resumed later. Time slept on `Retry-After`
    /// moves the deadline back, so a busy Roblox does not turn waits into give-ups.
    pub fn wait_operation(&self, path: &str, deadline: Instant, stop: &AtomicBool) -> Result<Operation, Error> {
        let mut deadline = deadline;
        let mut interval = Duration::from_secs(1);
        loop {
            let mut waited = Duration::ZERO;
            let op = self.poll_operation(path, &mut waited)?;
            deadline += waited;
            if op.done || stop.load(Ordering::SeqCst) || self.now() + interval > deadline {
                return Ok(op);
            }
            self.sleep(interval);
            interval = (interval * 2).min(Duration::from_secs(5));
        }
    }

    fn poll_operation(&self, path: &str, waited: &mut Duration) -> Result<Operation, Error> {
        let call = Call {
            op: OPERATION,
            method: "GET",
            path: format!("assets/v1/{path}"),
            body: None,
            create: false,
            long: false,
        };
        if !operation_path(path) {
            return Err(self.error(&call, ErrorKind::Local, format!("{path:?} is not operations/<id>")));
        }
        let body = self.retrying(&call, waited)?;
        let v = self.decode(&call, &body)?;
        let got = v["path"].as_str().filter(|p| !p.is_empty()).unwrap_or(path);
        if got != path {
            return Err(self.error(&call, ErrorKind::Decode, format!("asked for {path} and got {got}")));
        }
        Ok(decode(path, &v))
    }

    /// An asset's content, as asset delivery hands it out (a Model's is the import's rbxm): its `location`, asked for
    /// with the key and retrying a 404 for [`DELIVERY_WAIT`], then fetched without it and gunzipped when it is gzip
    /// (the client never asks for gzip, but a CDN may send it anyway).
    pub fn asset_content(&self, id: u64) -> Result<Vec<u8>, Error> {
        let call = Call {
            op: DELIVERY,
            method: "GET",
            path: format!("asset-delivery-api/v1/assetId/{id}"),
            body: None,
            create: false,
            long: false,
        };
        let mut deadline = self.now() + DELIVERY_WAIT;
        let mut attempt = 0;
        let body = loop {
            let mut waited = Duration::ZERO;
            let got = self.retrying(&call, &mut waited);
            deadline += waited;
            match got {
                Err(e) if e.kind == ErrorKind::Status(404) && self.now() + backoff(attempt) <= deadline => {
                    self.sleep(backoff(attempt));
                    attempt += 1;
                }
                got => break got?,
            }
        };
        let location = self.decode(&call, &body)?["location"].as_str().unwrap_or("").to_string();
        if !absolute(&location) {
            return Err(self.error(&call, ErrorKind::Decode, format!("the location {location:?} is not a URL")));
        }
        let call = Call { op: CONTENT, method: "GET", path: location, body: None, create: false, long: false };
        let body = self.send(&call)?;
        if !body.starts_with(&[0x1f, 0x8b]) {
            return Ok(body);
        }
        let mut out = Vec::new();
        match GzDecoder::new(&body[..]).take(BODY_LIMIT + 1).read_to_end(&mut out) {
            Ok(n) if n as u64 <= BODY_LIMIT => Ok(out),
            Ok(_) => Err(self.error(&call, ErrorKind::Decode, format!("gunzipped past {BODY_LIMIT} bytes"))),
            Err(e) => Err(self.error(&call, ErrorKind::Decode, format!("gunzip: {e}"))),
        }
    }
}

fn decode(path: &str, v: &Value) -> Operation {
    let error = &v["error"];
    let error = (!error.is_null()).then(|| (code(&error["code"]), error["message"].as_str().unwrap_or("").to_string()));
    let response = &v["response"];
    Operation {
        path: path.into(),
        done: v["done"].as_bool().unwrap_or(false),
        error,
        asset_id: id(&response["assetId"]),
        moderation: response["moderationResult"]["moderationState"].as_str().map(String::from),
    }
}

fn creator(c: Creator) -> Value {
    match c {
        Creator::User(id) => json!({"userId": id.to_string()}),
        Creator::Group(id) => json!({"groupId": id.to_string()}),
    }
}

/// A name as Roblox takes it: its last 50 characters, so a long path keeps the part that tells it apart, `asset` if
/// blank.
pub fn display_name(name: &str) -> String {
    let chars: Vec<char> = name.trim().chars().collect();
    let name: String = chars[chars.len().saturating_sub(50)..].iter().collect();
    if name.trim().is_empty() { "asset".into() } else { name.trim().into() }
}

/// `operations/<id>`: one segment with no query, fragment or whitespace.
fn operation_path(path: &str) -> bool {
    let Some(id) = path.strip_prefix("operations/") else { return false };
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains(['/', '?', '#', '\\'])
        && !id.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{Mock, Reply};
    use crate::{Clock, Fake};
    use std::sync::Arc;

    fn client(base: &str) -> (Client, Arc<Fake>) {
        let fake = Fake::new();
        (Client::new("test-key", base, Clock::Fake(fake.clone())).unwrap(), fake)
    }

    const PNG: Upload = Upload {
        kind: AssetKind::Image,
        name: "ui/icon",
        description: "assets/ui/icon.png",
        creator: Creator::Group(7654321),
        ext: ".png",
        mime: "image/png",
        bytes: b"PNG bytes",
    };

    #[test]
    fn a_create_sends_exactly_these_bytes() {
        let mock = Mock::start();
        mock.on("POST", "assets/v1/assets", Reply::json(200, &json!({"path": "operations/abc", "done": false})));
        let (c, _) = client(&mock.base);
        let op = c.create_asset(&PNG).unwrap();
        assert_eq!((op.path.as_str(), op.done), ("operations/abc", false));

        let r = &mock.requests()[0];
        let mime = r.header("content-type").unwrap();
        let boundary = mime.strip_prefix("multipart/form-data; boundary=opossum-").unwrap();
        assert!(boundary.len() == 32 && boundary.bytes().all(|b| b.is_ascii_hexdigit()));
        let b = format!("opossum-{boundary}");
        let request = r#"{"assetType":"Image","displayName":"ui/icon","description":"assets/ui/icon.png","creationContext":{"creator":{"groupId":"7654321"},"expectedPrice":0}}"#;
        let want = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"request\"\r\n\r\n{request}\r\n--{b}\r\nContent-Disposition: form-data; name=\"fileContent\"; filename=\"asset.png\"\r\nContent-Type: image/png\r\n\r\nPNG bytes\r\n--{b}--\r\n"
        );
        assert_eq!(String::from_utf8_lossy(&r.body), want);
        assert_eq!(r.header("content-length"), Some(want.len().to_string().as_str()));
        assert_eq!(r.header("transfer-encoding"), None);
        assert_eq!(r.header("x-api-key"), Some("test-key"));
        assert_eq!(r.header("accept"), Some("application/json"));
    }

    /// many files may be named color.png: a name cut to Roblox's 50 characters keeps the end, which tells them apart
    #[test]
    fn a_long_display_name_keeps_its_end() {
        let key = format!("decals/{}/color", "x".repeat(60));
        assert_eq!(display_name(&key), format!("{}/color", "x".repeat(44)));
        assert_eq!(display_name("  "), "asset");
    }

    #[test]
    fn an_operation_with_a_string_error_code_is_terminal() {
        // 2026-10-24: Roblox answered a glb's operation with {"code":"Unknown"} where its schema says integer, and a
        // strict decode wedged every later upload until the lock was edited by hand
        let mock = Mock::start();
        let body =
            json!({"path": "operations/a", "done": true, "error": {"code": "Unknown", "message": "Unknown Error"}});
        mock.on("GET", "assets/v1/operations/a", Reply::json(200, &body));
        let (c, _) = client(&mock.base);
        let op = c.operation("operations/a").unwrap();
        assert!(op.done);
        assert_eq!(op.error, Some(("Unknown".into(), "Unknown Error".into())));
        assert_eq!(op.asset_id, None);
    }

    #[test]
    fn operations_decode_ids_either_way_and_keep_moderation() {
        let mock = Mock::start();
        let done = |id: Value, state: &str| json!({"done": true, "response": {"assetId": id, "moderationResult": {"moderationState": state}}});
        mock.on("GET", "assets/v1/operations/a", Reply::json(200, &done(json!("2205400862"), "Approved")));
        mock.on("GET", "assets/v1/operations/a", Reply::json(200, &done(json!(27), "Rejected")));
        mock.on("GET", "assets/v1/operations/a", Reply::json(200, &json!({"path": "operations/b", "done": true})));
        mock.on("GET", "assets/v1/operations/a", Reply::json(404, &json!({"code": "NOT_FOUND"})));
        let (c, _) = client(&mock.base);
        let op = c.operation("operations/a").unwrap();
        assert_eq!((op.asset_id, op.moderation.as_deref()), (Some(2_205_400_862), Some("Approved")));
        let op = c.operation("operations/a").unwrap();
        assert_eq!((op.asset_id, op.rejected(), op.reviewing()), (Some(27), true, false));
        let op = |state: &str| Operation { moderation: Some(state.into()), ..op.clone() };
        assert!(
            op("MODERATION_STATE_REJECTED").rejected() && op("reviewing").reviewing() && !op("Approved").rejected()
        );
        assert_eq!(c.operation("operations/a").unwrap_err().kind, ErrorKind::Decode);
        assert_eq!(c.operation("operations/a").unwrap_err().diag(), "cloud.notFound");
    }

    #[test]
    fn waiting_backs_off_to_five_seconds_and_gives_up_pending_at_the_deadline() {
        let mock = Mock::start();
        let pending = json!({"path": "operations/a", "done": false});
        for _ in 0..5 {
            mock.on("GET", "assets/v1/operations/a", Reply::json(200, &pending));
        }
        mock.on("GET", "assets/v1/operations/a", Reply::json(429, &json!({})).header("Retry-After", "30"));
        mock.on("GET", "assets/v1/operations/a", Reply::json(200, &pending));
        mock.on("GET", "assets/v1/operations/a", Reply::json(200, &pending));
        let (c, fake) = client(&mock.base);
        let op = c.wait_operation("operations/a", c.now() + Duration::from_secs(20), &AtomicBool::new(false)).unwrap();
        assert!(!op.done);
        let secs: Vec<u64> = fake.slept().iter().map(Duration::as_secs).collect();
        // 1+2+4+5+5 = 17 s polling, then 30 s of Retry-After that does not count, then one more 5 s would pass 20 s
        assert_eq!(secs, [1, 2, 4, 5, 5, 30]);
        assert_eq!(mock.requests().len(), 7);
        // a stop gives it back after one look, with nothing slept: its intent is saved, so it resumes next run
        mock.on("GET", "assets/v1/operations/a", Reply::json(200, &pending));
        let op = c.wait_operation("operations/a", c.now() + Duration::from_secs(600), &AtomicBool::new(true)).unwrap();
        assert!(!op.done && fake.slept().len() == 6 && mock.requests().len() == 8);
    }

    #[test]
    fn content_comes_back_as_sent_or_gunzipped() {
        let mock = Mock::start();
        let location = json!({"location": format!("{}/cdn/a", mock.base)});
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, b"<roblox!zipped").unwrap();
        for body in [b"<roblox!plain".to_vec(), gz.finish().unwrap()] {
            mock.on("GET", "asset-delivery-api/v1/assetId/7", Reply::json(200, &location));
            mock.on("GET", "cdn/a", Reply::bytes(200, body));
        }
        let (c, _) = client(&mock.base);
        assert_eq!(c.asset_content(7).unwrap(), b"<roblox!plain");
        assert_eq!(c.asset_content(7).unwrap(), b"<roblox!zipped");
    }

    /// Right after its operation is done a model may still be processing, and delivery answers 404 until it is not.
    #[test]
    fn a_delivery_404_is_retried_for_30_s() {
        let mock = Mock::start();
        for _ in 0..3 {
            mock.on("GET", "asset-delivery-api", Reply::json(404, &json!({})));
        }
        mock.on("GET", "asset-delivery-api", Reply::json(200, &json!({"location": format!("{}/cdn/a", mock.base)})));
        mock.on("GET", "cdn/a", Reply::bytes(200, b"model".to_vec()));
        let (c, fake) = client(&mock.base);
        assert_eq!(c.asset_content(7).unwrap(), b"model");
        assert_eq!(fake.slept(), [Duration::from_millis(250), Duration::from_millis(500), Duration::from_secs(1)]);

        // one that never clears gives up before 30 s have passed (0.25 + 0.5 + 1 + 14 × 2)
        let (c, fake) = client(&mock.base);
        assert_eq!(c.asset_content(8).unwrap_err().diag(), "cloud.notFound");
        assert_eq!(fake.slept().iter().sum::<Duration>(), Duration::from_millis(29_750));
    }

    #[test]
    fn invalid_uploads_and_paths_send_nothing() {
        let mock = Mock::start();
        let (c, _) = client(&mock.base);
        let bad = [
            Upload { bytes: b"", ..PNG },
            Upload { creator: Creator::User(0), ..PNG },
            Upload { ext: "png", ..PNG },
            Upload { ext: ".p/g", ..PNG },
        ];
        for u in &bad {
            assert_eq!(c.create_asset(u).unwrap_err().kind, ErrorKind::Local);
        }
        for path in
            ["operations/", "operations/a/b", "operations/..", "operations/a?x", "../operations/a", "operations/a b"]
        {
            assert_eq!(c.operation(path).unwrap_err().kind, ErrorKind::Local, "{path}");
        }
        assert!(mock.requests().is_empty());
    }
}
