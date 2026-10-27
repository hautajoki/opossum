//! HTTP/1.1 over std::net: one request per connection (`Connection: close`), a thread per connection.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::{Entry, LogKind, LogLine, Op, Prop, Realm, Shared};

/// How long `/patch` waits for a newer generation before answering with no ops.
const POLL: Duration = Duration::from_secs(25);
const MAX_HEAD: usize = 16 << 10;
const MAX_BODY: usize = 8 << 20;

pub(crate) fn serve(listener: TcpListener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = shared.clone();
        let _ = thread::Builder::new()
            .name("sync-conn".into())
            .spawn(move || handle(stream, &shared));
    }
}

struct Request {
    method: String,
    path: String,
    query: String,
    host: Option<String>,
    origin: Option<String>,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    kind: &'static str,
    body: Vec<u8>,
}

const JSON: &str = "application/json";
const TEXT: &str = "text/plain; charset=utf-8";

fn handle(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let res = match read_request(&mut stream) {
        Some(req) if !local(&req, shared.port) => {
            text(403, "opossum only answers 127.0.0.1 and localhost")
        }
        Some(req) => route(&req, shared),
        None => text(400, "bad request"),
    };
    let reason = match res.status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "",
    };
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        res.status,
        res.kind,
        res.body.len()
    );
    let _ = stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&res.body));
}

fn route(req: &Request, shared: &Shared) -> Response {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/hello") => hello(shared),
        ("GET", "/patch") => patch(req, shared),
        ("GET", path) if path.starts_with("/blob/") => blob(&path[6..], shared),
        ("POST", "/blobs") => blobs(&req.body, shared),
        ("POST", "/log") => log(&req.body, shared),
        _ => text(404, "not found"),
    }
}

fn hello(shared: &Shared) -> Response {
    let generation = shared.lock().generation;
    let config = &shared.config;
    json(json!({
        "project": config.project,
        "place": config.place,
        "unpublished": config.unpublished,
        "roots": config.roots,
        "session": shared.session,
        "generation": generation,
    }))
}

/// `?since=G&session=S[&only=a,b]`: the ops from G to the current generation, waiting up to `POLL` while G is
/// current or nothing was published yet. A `since` of 0, from another session or older than the log is answered
/// with the full snapshot.
fn patch(req: &Request, shared: &Shared) -> Response {
    let mut since = 0;
    let mut stale = false;
    let mut only = Vec::new();
    for pair in req.query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "since" => since = value.parse().unwrap_or(0),
            "session" => stale = value != shared.session,
            "only" => only.extend(
                decode(value)
                    .split(',')
                    .map(|p| p.trim_matches('/').to_string())
                    .filter(|p| !p.is_empty()),
            ),
            _ => {}
        }
    }

    let deadline = Instant::now() + POLL;
    let mut state = shared.lock();
    while state.generation == 0 || (state.generation == since && !stale) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        state = shared
            .changed
            .wait_timeout(state, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }

    let oldest = state
        .log
        .front()
        .map_or(state.generation, |p| p.generation - 1);
    let full = stale || since == 0 || since > state.generation || since < oldest;
    let mut ops = Vec::new();
    if full {
        let mut keep = BTreeSet::new();
        for (path, entry) in &state.tree {
            if matches(&only, path, entry) {
                // Ancestors come along so a scoped snapshot can still build its parents.
                let mut at = path.as_ref();
                while keep.insert(at) {
                    let Some(slash) = at.rfind('/') else { break };
                    at = &at[..slash];
                }
            }
        }
        for path in keep {
            if let Some(entry) = state.tree.get(path) {
                ops.push(set(path, entry));
            }
        }
    } else {
        let mut merged = std::collections::BTreeMap::new();
        for patch in state.log.iter().filter(|p| p.generation > since) {
            for (path, op) in &patch.ops {
                merged.insert(path.clone(), op.clone());
            }
        }
        // Sets parent-first (a parent's path sorts before its children's), dels children-first.
        for (path, op) in &merged {
            if let Op::Set(entry) = op
                && matches(&only, path, entry)
            {
                ops.push(set(path, entry));
            }
        }
        for (path, op) in merged.iter().rev() {
            if let Op::Del(entry) = op
                && matches(&only, path, entry)
            {
                ops.push(json!({ "op": "del", "path": path.as_ref() }));
            }
        }
    }
    let generation = state.generation;
    drop(state);
    json(json!({ "session": shared.session, "generation": generation, "full": full, "ops": ops }))
}

fn matches(only: &[String], path: &str, entry: &Entry) -> bool {
    only.is_empty()
        || only.iter().any(|prefix| {
            under(path, prefix) || entry.file.as_deref().is_some_and(|f| under(f, prefix))
        })
}

/// `path` is `prefix` or below it, component-wise (`a/bc` is not under `a/b`).
fn under(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn set(path: &str, entry: &Entry) -> Value {
    let mut props = Map::new();
    for (name, prop) in &entry.props {
        let value = match prop {
            Prop::Bool(b) => json!(b),
            Prop::Number(n) => json!(n),
            Prop::String(s) => json!(s),
            Prop::Enum(ty, item) => json!({ "enum": ty, "item": item }),
        };
        props.insert(name.clone(), value);
    }
    json!({
        "op": "set",
        "path": path,
        "class": entry.class,
        "hash": entry.hash.map(|h| format!("{h:032x}")),
        "props": props,
    })
}

fn blob(hex: &str, shared: &Shared) -> Response {
    match parse_hash(hex).and_then(|h| shared.lock().blobs.get(&h).cloned()) {
        Some(source) => Response {
            status: 200,
            kind: TEXT,
            body: source.as_bytes().to_vec(),
        },
        None => text(404, "no such blob"),
    }
}

/// `["<hash>", ...]` → `{"<hash>": source, ...}`; hashes no longer held are left out.
fn blobs(body: &[u8], shared: &Shared) -> Response {
    let Ok(Value::Array(hashes)) = serde_json::from_slice::<Value>(body) else {
        return text(400, "expected a JSON array of hashes");
    };
    let sources: Vec<(&str, Arc<str>)> = {
        let state = shared.lock();
        hashes
            .iter()
            .filter_map(|h| {
                let hex = h.as_str()?;
                Some((hex, state.blobs.get(&parse_hash(hex)?)?.clone()))
            })
            .collect()
    };
    // Written by hand so megabytes of source are escaped once instead of copied into a `Value` first.
    let mut out = String::from("{");
    for (i, (hex, source)) in sources.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&Value::from(*hex).to_string());
        out.push(':');
        out.push_str(&serde_json::to_string(source.as_ref()).unwrap_or_default());
    }
    out.push('}');
    Response {
        status: 200,
        kind: JSON,
        body: out.into_bytes(),
    }
}

/// `[{"kind": "MessageError", "realm": "server", "text": "..."}, ...]`, the plugin's batched LogService output.
fn log(body: &[u8], shared: &Shared) -> Response {
    let Ok(Value::Array(lines)) = serde_json::from_slice::<Value>(body) else {
        return text(400, "expected a JSON array of log lines");
    };
    let Some(sink) = &shared.config.log else {
        return text(200, "");
    };
    for line in &lines {
        let kind = match line["kind"].as_str() {
            Some("MessageInfo") => LogKind::Info,
            Some("MessageWarning") => LogKind::Warning,
            Some("MessageError") => LogKind::Error,
            _ => LogKind::Output,
        };
        let realm = match line["realm"].as_str() {
            Some("server") => Realm::Server,
            Some("client") => Realm::Client,
            _ => Realm::Edit,
        };
        let text = line["text"].as_str().unwrap_or_default().to_string();
        let _ = sink.send(LogLine { kind, realm, text });
    }
    text(200, "")
}

fn parse_hash(hex: &str) -> Option<u128> {
    if hex.len() != 32 {
        return None;
    }
    u128::from_str_radix(hex, 16).ok()
}

fn json(value: Value) -> Response {
    Response {
        status: 200,
        kind: JSON,
        body: value.to_string().into_bytes(),
    }
}

fn text(status: u16, body: &str) -> Response {
    Response {
        status,
        kind: TEXT,
        body: body.as_bytes().to_vec(),
    }
}

/// DNS rebinding guard: a page on evil.com resolving to 127.0.0.1 still sends `Host: evil.com`, and a browser's
/// cross-site request carries its `Origin`. Studio sends neither.
fn local(req: &Request, port: u16) -> bool {
    let host_ok = |host: &str| {
        let (name, p) = host
            .rsplit_once(':')
            .map_or((host, None), |(n, p)| (n, Some(p)));
        matches!(name, "127.0.0.1" | "localhost") && p.is_none_or(|p| p.parse() == Ok(port))
    };
    req.host.as_deref().is_some_and(host_ok)
        && req
            .origin
            .as_deref()
            .is_none_or(|o| o.strip_prefix("http://").is_some_and(host_ok))
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_len = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_len]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let (mut host, mut origin, mut length) = (None, None, 0usize);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => host = Some(value.to_string()),
            "origin" => origin = Some(value.to_string()),
            "content-length" => length = value.parse().ok()?,
            _ => {}
        }
    }
    if length > MAX_BODY {
        return None;
    }
    let mut body = buf[head_len + 4..].to_vec();
    if body.len() < length {
        let start = body.len();
        body.resize(length, 0);
        stream.read_exact(&mut body[start..]).ok()?;
    }
    body.truncate(length);
    Some(Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        host,
        origin,
        body,
    })
}

/// Percent-decoding for query values (`HttpService:UrlEncode` output); `+` is a space.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
