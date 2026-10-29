//! A fake Open Cloud on loopback, std only: a listener on `127.0.0.1` and one thread per connection, every answer with
//! `Connection: close` (a kept-alive connection would hold its thread). A test queues [`Reply`]s per method and path
//! prefix; each request takes the first queued reply it matches, and is recorded with its method, path, raw query,
//! headers and body. An in-flight gauge keeps its peak, so tests can pin concurrency limits.
//!
//! [`serve`] runs it as a server for end-to-end checks of the real CLI (`examples/mock.rs`), answering with [`auto`]:
//! a stateful stand-in for every endpoint opossum calls.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// Without its leading `/`, as the client's resources are spelled (`assets/v1/assets`).
    pub path: String,
    pub query: String,
    /// Names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub enum Reply {
    Status {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Read the request, then close without answering.
    Reset,
    /// Never answer: hold the connection until the client gives up on it.
    Hang,
    /// Decide while the request is in flight: read what the client saved before sending it, or wait on a barrier.
    Run(Box<dyn FnOnce(&Request) -> Reply + Send>),
}

/// What answers a request nothing was queued for.
pub type Fallback = Box<dyn Fn(&Request) -> Reply + Send + Sync>;

struct Shared {
    /// Method, path prefix and reply, in the order queued.
    queue: Mutex<Vec<(String, String, Reply)>>,
    requests: Mutex<Vec<Request>>,
    flight: AtomicUsize,
    peak: AtomicUsize,
    stop: AtomicBool,
    /// Without one, an unqueued request is a 404.
    fallback: Option<Fallback>,
}

pub struct Mock {
    /// `http://127.0.0.1:<port>`, what a [`crate::Client`] is pointed at.
    pub base: String,
    addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Mock {
    pub fn start() -> Mock {
        Mock::bind(0, None).expect("mock open cloud")
    }

    /// Listens on `127.0.0.1:port` (0 picks one), answering what nothing queued with `fallback`.
    pub fn bind(port: u16, fallback: Option<Fallback>) -> Result<Mock, String> {
        let listen = |e| format!("mock open cloud: cannot listen on 127.0.0.1:{port}: {e}");
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(listen)?;
        let addr = listener.local_addr().map_err(listen)?;
        let shared = Arc::new(Shared {
            queue: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            fallback,
        });
        let accept = shared.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                if accept.stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok(stream) = stream {
                    let shared = accept.clone();
                    thread::spawn(move || connection(stream, &shared));
                }
            }
        });
        Ok(Mock { base: format!("http://{addr}"), addr, shared })
    }

    /// Queues `reply` for the next `method` request whose path starts with `prefix`.
    pub fn on(&self, method: &str, prefix: &str, reply: Reply) {
        self.shared.queue.lock().unwrap().push((method.into(), prefix.into(), reply));
    }

    /// Every request so far, in the order they arrived.
    pub fn requests(&self) -> Vec<Request> {
        self.shared.requests.lock().unwrap().clone()
    }

    /// The most requests that were in flight at once.
    pub fn peak(&self) -> usize {
        self.shared.peak.load(Ordering::SeqCst)
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.addr);
    }
}

/// The base URL of a port nothing listens on: connecting is refused.
pub fn refused() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock open cloud: bind 127.0.0.1:0");
    format!("http://{}", listener.local_addr().expect("mock open cloud: local address"))
}

impl Reply {
    pub fn json(status: u16, value: &Value) -> Reply {
        let headers = vec![("Content-Type".into(), "application/json".into())];
        Reply::Status { status, headers, body: value.to_string().into_bytes() }
    }

    pub fn header(mut self, name: &str, value: &str) -> Reply {
        if let Reply::Status { headers, .. } = &mut self {
            headers.push((name.into(), value.into()));
        }
        self
    }
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

fn connection(mut stream: TcpStream, shared: &Shared) {
    let Some(request) = read(&stream) else { return };
    if request.method == "GET" && request.path == "_mock/requests" {
        let seen = shared.requests.lock().unwrap();
        let seen: Vec<Value> =
            seen.iter().map(|r| json!({"method": r.method, "path": r.path, "query": r.query})).collect();
        return answer(&mut stream, Reply::json(200, &Value::Array(seen)), &request);
    }
    shared.requests.lock().unwrap().push(request.clone());
    let flight = shared.flight.fetch_add(1, Ordering::SeqCst) + 1;
    shared.peak.fetch_max(flight, Ordering::SeqCst);
    let reply = {
        let mut queue = shared.queue.lock().unwrap();
        let at = queue.iter().position(|(m, p, _)| *m == request.method && request.path.starts_with(p.as_str()));
        at.map(|i| queue.remove(i).2)
    };
    let reply = match (reply, &shared.fallback) {
        (Some(reply), _) => reply,
        (None, Some(fallback)) => fallback(&request),
        (None, None) => not_found(&request),
    };
    answer(&mut stream, reply, &request);
    shared.flight.fetch_sub(1, Ordering::SeqCst);
}

fn answer(stream: &mut TcpStream, reply: Reply, request: &Request) {
    match reply {
        Reply::Status { status, headers, body } => {
            let mut head = format!("HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
            for (name, value) in headers {
                head += &format!("{name}: {value}\r\n");
            }
            head += "\r\n";
            let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body));
        }
        Reply::Reset => {}
        Reply::Hang => {
            let mut sink = [0u8; 256];
            while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
        }
        Reply::Run(decide) => answer(stream, decide(request), request),
    }
}

/// One HTTP/1.1 request with a Content-Length body (the client never chunks).
fn read(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut words = line.split_ascii_whitespace();
    let method = words.next()?.to_string();
    let target = words.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (path, query) = (path.trim_start_matches('/').to_string(), query.to_string());
    let mut headers = Vec::new();
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let Some((name, value)) = line.trim_end().split_once(':') else { break };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let len = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    Some(Request { method, path, query, headers, body })
}

fn not_found(r: &Request) -> Reply {
    let message = format!("mock: nothing answers {} {}", r.method, r.path);
    Reply::json(404, &json!({"error": {"code": "NOT_FOUND", "message": message}}))
}

/// Serves [`auto`] on `127.0.0.1:port` until the process ends, printing its base URL on stdout. `GET
/// <base>/_mock/requests` lists every request it saw, so a check can count what a command sent.
pub fn serve(port: u16) -> Result<(), String> {
    let mock = Mock::bind(port, Some(auto()))?;
    println!("{}", mock.base);
    loop {
        thread::park();
    }
}

/// A stateful stand-in for the endpoints opossum calls. Asset creates and FBX updates answer with an operation that
/// is done at its first poll (new ids count up from 1000001); place saves count versions up per place; tasks run at
/// the version asked for, or the place's last saved one, and complete at their first poll with no results and no logs;
/// pass, product, badge and thumbnail creates answer with ids counting up from 2000001, and every other resource call
/// succeeds.
pub fn auto() -> Fallback {
    let state = Mutex::new(Auto::default());
    Box::new(move |r| state.lock().unwrap().reply(r))
}

#[derive(Default)]
struct Auto {
    next: u64,
    /// Operation path → the asset id it finishes with.
    operations: HashMap<String, u64>,
    /// `universe/place` → its last version.
    versions: HashMap<String, u64>,
}

impl Auto {
    fn reply(&mut self, r: &Request) -> Reply {
        let segments: Vec<&str> = r.path.split('/').collect();
        match (r.method.as_str(), &segments[..]) {
            ("POST", ["assets", "v1", "assets"]) => {
                self.next += 1;
                self.operation(1_000_000 + self.next)
            }
            ("PATCH", ["assets", "v1", "assets", id]) => match id.parse() {
                Ok(id) => self.operation(id),
                Err(_) => not_found(r),
            },
            ("GET", ["assets", "v1", "operations", _]) => match self.operations.get(&r.path["assets/v1/".len()..]) {
                Some(id) => Reply::json(
                    200,
                    &json!({"path": &r.path["assets/v1/".len()..], "done": true, "response": {
                        "assetId": id.to_string(), "moderationResult": {"moderationState": "Approved"}}}),
                ),
                None => not_found(r),
            },
            ("POST", ["universes", "v1", universe, "places", place, "versions"]) => {
                let version = self.versions.entry(format!("{universe}/{place}")).or_insert(0);
                *version += 1;
                Reply::json(200, &json!({"versionNumber": *version}))
            }
            (
                "POST",
                ["cloud", "v2", "universes", universe, "places", place, rest @ .., "luau-execution-session-tasks"],
            ) => {
                let version = match rest {
                    ["versions", v] => v.to_string(),
                    _ => self.versions.get(&format!("{universe}/{place}")).copied().unwrap_or(1).to_string(),
                };
                self.next += 1;
                let n = self.next;
                let path = format!(
                    "universes/{universe}/places/{place}/versions/{version}/luau-execution-sessions/{n}/tasks/{n}"
                );
                Reply::json(200, &json!({"path": path, "state": "QUEUED"}))
            }
            ("GET", ["cloud", "v2", "universes", .., "tasks", _]) => Reply::json(
                200,
                &json!({"path": &r.path["cloud/v2/".len()..], "state": "COMPLETE", "output": {"results": []}}),
            ),
            ("GET", ["cloud", "v2", "universes", .., "logs"]) => {
                Reply::json(200, &json!({"luauExecutionSessionTaskLogs": [], "nextPageToken": ""}))
            }
            ("POST", ["game-passes", "v1", "universes", _, "game-passes"]) => self.created("gamePassId"),
            ("POST", ["developer-products", "v2", "universes", _, "developer-products"]) => self.created("productId"),
            ("POST", ["legacy-badges", "v1", "universes", _, "badges"]) => self.created("id"),
            (
                "POST",
                ["legacy-game-internationalization", "v1", "game-thumbnails", "games", _, "language-codes", _, "image"],
            ) => self.created("mediaAssetId"),
            ("PATCH", ["game-passes" | "developer-products" | "legacy-badges", ..])
            | ("PATCH", ["cloud", "v2", "universes", _, "places", _])
            | ("POST", ["legacy-publish" | "legacy-game-internationalization", ..])
            | ("DELETE", ["legacy-game-internationalization", ..]) => Reply::json(200, &json!({})),
            _ => not_found(r),
        }
    }

    /// A resource create's answer: `{"<field>": <new id>}`.
    fn created(&mut self, field: &str) -> Reply {
        self.next += 1;
        Reply::json(200, &json!({ field: 2_000_000 + self.next }))
    }

    fn operation(&mut self, asset: u64) -> Reply {
        self.next += 1;
        let path = format!("operations/{}", self.next);
        self.operations.insert(path.clone(), asset);
        Reply::json(200, &json!({"path": path, "done": false}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssetKind, Call, Client, Clock, Creator, Upload};
    use std::time::Duration;

    #[test]
    fn the_server_answers_a_whole_upload_save_and_run() {
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let c = Client::new("test-key", &mock.base, Clock::Real).unwrap();
        let png = Upload {
            kind: AssetKind::Image,
            name: "a",
            creator: Creator::User(1),
            ext: ".png",
            mime: "image/png",
            bytes: b"x",
        };
        let op = c.create_asset(&png).unwrap();
        let op = c.wait_operation(&op.path, c.now(), &std::sync::atomic::AtomicBool::new(false)).unwrap();
        assert_eq!((op.done, op.asset_id), (true, Some(1_000_001)));
        assert_eq!(c.save_place(12, 34, b"rbxl", false).unwrap(), 1);
        assert_eq!(c.save_place(12, 34, b"rbxl", false).unwrap(), 2);
        let task = c.start_task(12, 34, None, "return 1", Duration::from_secs(1)).unwrap();
        let task = c.wait_task(&task.path, c.now() + Duration::from_secs(5), &mut |_| {}).unwrap();
        assert_eq!((task.state.as_str(), task.version), ("COMPLETE", 2));
        assert!(c.task_logs(&task.path, c.now()).unwrap().is_empty());

        let call =
            Call { op: "list", method: "GET", path: "_mock/requests".into(), body: None, create: false, long: false };
        let seen = c.json(&call).unwrap();
        assert_eq!(seen.as_array().unwrap().len(), 7);
        assert_eq!(seen[2]["path"], "universes/v1/12/places/34/versions");
    }
}
