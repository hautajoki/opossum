//! Assets: create, the FBX content update, and the operation each returns.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{Call, Client, Error, ErrorKind, Part, code, id, multipart};

/// Roblox's limit on an asset file.
pub const MAX_ASSET: usize = 20_000_000;
/// How long [`Client::wait_operation`] polls one operation before it gives the file back as pending.
pub const OPERATION_DEADLINE: Duration = Duration::from_secs(600);

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

/// One file to upload. `name` is the display name before Roblox's rules (at most 50 characters, `asset` if blank);
/// `ext` is the output's extension with its dot (`.png` for a processed SVG), which names the file part `asset<ext>`.
pub struct Upload<'a> {
    pub kind: AssetKind,
    pub name: &'a str,
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
const UPDATE: &str = "update asset";
const OPERATION: &str = "get asset operation";

impl Client {
    /// Creates an asset. Not idempotent: the caller saves its intent first, and an ambiguous error keeps it.
    pub fn create_asset(&self, u: &Upload) -> Result<Operation, Error> {
        let request = json!({
            "assetType": u.kind.name(),
            "displayName": display_name(u.name),
            "description": "Uploaded by opossum",
            "creationContext": {"creator": creator(u.creator), "expectedPrice": 0},
        });
        self.upload(CREATE, "POST", "assets/v1/assets".into(), &request, u)
    }

    /// Replaces an FBX model's content in place (the only asset Roblox lets change). The caller splits off a new asset
    /// instead when another file shares `id`, so one file's change never silently changes another's.
    pub fn update_asset(&self, id: u64, u: &Upload) -> Result<Operation, Error> {
        let request = json!({
            "assetType": "Model",
            "assetId": id.to_string(),
            "creationContext": {"creator": creator(u.creator), "expectedPrice": 0},
        });
        let path = format!("assets/v1/assets/{id}");
        if id == 0 || u.kind != AssetKind::Model || u.ext != ".fbx" {
            let message =
                format!("only an FBX model's content can be updated (asset {id}, a {} {})", u.kind.name(), u.ext);
            let call = Call { op: UPDATE, method: "PATCH", path, body: None, create: true, long: true };
            return Err(self.error(&call, ErrorKind::Local, message));
        }
        self.upload(UPDATE, "PATCH", path, &request, u)
    }

    fn upload(
        &self,
        op: &'static str,
        method: &'static str,
        path: String,
        request: &Value,
        u: &Upload,
    ) -> Result<Operation, Error> {
        let request = request.to_string();
        let filename = format!("asset{}", u.ext);
        let parts = [
            Part { name: "request", filename: "", mime: "", bytes: request.as_bytes() },
            Part { name: "fileContent", filename: &filename, mime: u.mime, bytes: u.bytes },
        ];
        let (mime, body) = multipart(&parts);
        let call = Call { op, method, path, body: Some((&mime, &body)), create: true, long: true };
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

    /// Polls an operation every 1 s doubling to 5 s until it is done or `deadline` passes, and returns it either way:
    /// one that is not `done` is still pending and can be resumed later. Time slept on `Retry-After` moves the
    /// deadline back, so a busy Roblox does not turn waits into give-ups.
    pub fn wait_operation(&self, path: &str, deadline: Instant) -> Result<Operation, Error> {
        let mut deadline = deadline;
        let mut interval = Duration::from_secs(1);
        loop {
            let mut waited = Duration::ZERO;
            let op = self.poll_operation(path, &mut waited)?;
            deadline += waited;
            if op.done || self.now() + interval > deadline {
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

/// The file stem as Roblox takes it: at most 50 characters, `asset` if blank.
fn display_name(name: &str) -> String {
    let name: String = name.trim().chars().take(50).collect();
    if name.trim().is_empty() { "asset".into() } else { name }
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
        name: "icon",
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
        let request = r#"{"assetType":"Image","displayName":"icon","description":"Uploaded by opossum","creationContext":{"creator":{"groupId":"7654321"},"expectedPrice":0}}"#;
        let want = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"request\"\r\n\r\n{request}\r\n--{b}\r\nContent-Disposition: form-data; name=\"fileContent\"; filename=\"asset.png\"\r\nContent-Type: image/png\r\n\r\nPNG bytes\r\n--{b}--\r\n"
        );
        assert_eq!(String::from_utf8_lossy(&r.body), want);
        assert_eq!(r.header("content-length"), Some(want.len().to_string().as_str()));
        assert_eq!(r.header("transfer-encoding"), None);
        assert_eq!(r.header("x-api-key"), Some("test-key"));
        assert_eq!(r.header("accept"), Some("application/json"));
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
        assert_eq!((op.asset_id, op.moderation.as_deref()), (Some(27), Some("Rejected")));
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
        let op = c.wait_operation("operations/a", c.now() + Duration::from_secs(20)).unwrap();
        assert!(!op.done);
        let secs: Vec<u64> = fake.slept().iter().map(Duration::as_secs).collect();
        // 1+2+4+5+5 = 17 s polling, then 30 s of Retry-After that does not count, then one more 5 s would pass 20 s
        assert_eq!(secs, [1, 2, 4, 5, 5, 30]);
        assert_eq!(mock.requests().len(), 7);
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
        assert_eq!(c.update_asset(9001, &PNG).unwrap_err().kind, ErrorKind::Local);
        for path in
            ["operations/", "operations/a/b", "operations/..", "operations/a?x", "../operations/a", "operations/a b"]
        {
            assert_eq!(c.operation(path).unwrap_err().kind, ErrorKind::Local, "{path}");
        }
        assert!(mock.requests().is_empty());
    }

    #[test]
    fn an_fbx_update_patches_its_asset() {
        let mock = Mock::start();
        mock.on(
            "PATCH",
            "assets/v1/assets/9001",
            Reply::json(200, &json!({"path": "operations/u", "done": true, "response": {"assetId": "9001"}})),
        );
        let (c, _) = client(&mock.base);
        let fbx = Upload { kind: AssetKind::Model, ext: ".fbx", mime: "model/fbx", creator: Creator::User(1), ..PNG };
        assert_eq!(c.update_asset(9001, &fbx).unwrap().asset_id, Some(9001));
        let r = &mock.requests()[0];
        let body = String::from_utf8_lossy(&r.body);
        assert!(body.contains(
            r#"{"assetType":"Model","assetId":"9001","creationContext":{"creator":{"userId":"1"},"expectedPrice":0}}"#
        ));
        assert!(body.contains("filename=\"asset.fbx\""));
    }
}
