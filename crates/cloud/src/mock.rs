//! A fake Open Cloud on loopback, std only: a listener on `127.0.0.1` and one thread per connection, every answer with
//! `Connection: close` (a kept-alive connection would hold its thread). A test queues [`Reply`]s per method and path
//! prefix; each request takes the first queued reply it matches, and is recorded with its method, path, raw query,
//! headers and body. An in-flight gauge keeps its peak, so tests can pin concurrency limits.
//!
//! [`serve`] runs it as a server for end-to-end checks of the real CLI (`examples/mock.rs`), answering with [`auto`]:
//! a stateful stand-in for every endpoint opossum calls.

use std::collections::HashMap;
use std::fmt::Write as _;
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

    /// A body that is not JSON, such as an asset's content.
    pub fn bytes(status: u16, body: Vec<u8>) -> Reply {
        Reply::Status { status, headers: vec![("Content-Type".into(), "application/octet-stream".into())], body }
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
        let seen = seen.iter().map(|r| {
            let body = blake3::hash(&r.body).to_hex().to_string();
            json!({"method": r.method, "path": r.path, "query": r.query, "bytes": r.body.len(), "blake3": body})
        });
        let seen: Vec<Value> = seen.collect();
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

/// Serves [`auto`] on `127.0.0.1:port` until the process ends, printing its base URL on stdout, every task completing
/// with `results` (what a script returned, so a canned spec report checks `test --cloud`). `GET <base>/_mock/requests`
/// lists every request it saw with its body's length and BLAKE3, so a check can count what a command sent and see
/// which bytes a place save carried.
pub fn serve(port: u16, results: Vec<Value>) -> Result<(), String> {
    let state = Mutex::new(Auto { results, ..Auto::default() });
    let mock = Mock::bind(port, Some(Box::new(move |r| state.lock().unwrap().reply(r))))?;
    println!("{}", mock.base);
    loop {
        thread::park();
    }
}

/// A stateful stand-in for the endpoints opossum calls. Asset creates answer with an operation that is done at its
/// first poll (new ids count up from 1000001); a glb's id is also given a delivery `location` on this server, which
/// serves the import Roblox would make of it ([`import`]), gzipped as the CDN sends it; place saves count versions up per place; tasks run at the version asked
/// for, or the place's last saved one, and complete at their first poll with no results and no logs; pass, product,
/// badge and thumbnail creates answer with ids counting up from 2000001, and every other resource call succeeds.
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
    /// A created glb's id → its JSON chunk.
    models: HashMap<u64, Value>,
    /// What every task returns.
    results: Vec<Value>,
}

impl Auto {
    fn reply(&mut self, r: &Request) -> Reply {
        let segments: Vec<&str> = r.path.split('/').collect();
        match (r.method.as_str(), &segments[..]) {
            ("POST", ["assets", "v1", "assets"]) => {
                self.next += 1;
                let id = 1_000_000 + self.next;
                if let Some(gltf) = form_part(r, "fileContent").and_then(glb_json) {
                    self.models.insert(id, gltf);
                }
                self.operation(id)
            }
            ("GET", ["asset-delivery-api", "v1", "assetId", id]) => match id.parse() {
                Ok(id) if self.models.contains_key(&id) => {
                    let host = r.header("host").unwrap_or("127.0.0.1");
                    Reply::json(200, &json!({"location": format!("http://{host}/_mock/content/{id}")}))
                }
                _ => not_found(r),
            },
            ("GET", ["_mock", "content", id]) => match id.parse().ok().and_then(|id| Some((id, self.models.get(&id)?)))
            {
                Some((id, gltf)) => {
                    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                    gz.write_all(import(id, gltf).as_bytes()).expect("gzip into memory");
                    Reply::bytes(200, gz.finish().expect("gzip into memory"))
                }
                None => not_found(r),
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
                &json!({"path": &r.path["cloud/v2/".len()..], "state": "COMPLETE", "output": {"results": self.results}}),
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

/// The bytes of form-data part `name` in a body [`crate::multipart`] built.
fn form_part<'a>(r: &'a Request, name: &str) -> Option<&'a [u8]> {
    let boundary = r.header("content-type")?.split_once("boundary=")?.1;
    let body = &r.body[..];
    let at = find(body, format!("name=\"{name}\"").as_bytes())?;
    let start = at + find(&body[at..], b"\r\n\r\n")? + 4;
    let len = find(&body[start..], format!("\r\n--{boundary}").as_bytes())?;
    Some(&body[start..start + len])
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w[0] == needle[0] && w == needle)
}

/// A glb's JSON chunk, or `None` for anything that is not a glb.
fn glb_json(bytes: &[u8]) -> Option<Value> {
    let len = u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?) as usize;
    if bytes.get(..4)? != b"glTF" || bytes.get(16..20)? != b"JSON" {
        return None;
    }
    serde_json::from_slice(bytes.get(20..20 + len)?).ok()
}

/// The rbxmx Roblox's import makes of the glb whose JSON is `gltf`, uploaded as asset `id`, in the shape of a real one
/// (a real one, loaded in Studio): a Model holding the node tree as `<name>_Node` Models, a mesh node's own MeshPart
/// named after it inside its `_Node`, with `MeshContent`, `InitialSize` (the POSITION extent times the node's scale)
/// and, when its material has textures, a SurfaceAppearance with a map per channel. Ids are made up from `id`: meshes
/// `id + 10^12 + node`, maps `id + 2·10^12 + 4·material + channel`, so parts sharing a material share maps as they do.
fn import(id: u64, gltf: &Value) -> String {
    let nodes = gltf["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    let scene = &gltf["scenes"][gltf["scene"].as_u64().unwrap_or(0) as usize]["nodes"];
    let roots: Vec<usize> = match scene.as_array() {
        Some(roots) => roots.iter().filter_map(Value::as_u64).map(|n| n as usize).collect(),
        None => {
            let child =
                |i: usize| nodes.iter().any(|n| n["children"].as_array().is_some_and(|c| c.contains(&json!(i))));
            (0..nodes.len()).filter(|&i| !child(i)).collect()
        }
    };
    let mut xml = String::from("<roblox version=\"4\">\n");
    let mut refs = 0;
    open(&mut xml, &mut refs, "Model", "Model");
    xml += "</Properties>\n";
    for root in roots {
        node(&mut xml, &mut refs, id, gltf, root);
    }
    xml += "</Item>\n</roblox>\n";
    xml
}

fn node(xml: &mut String, refs: &mut usize, id: u64, gltf: &Value, i: usize) {
    let n = &gltf["nodes"][i];
    let name = n["name"].as_str().unwrap_or("Node");
    open(xml, refs, "Model", &format!("{name}_Node"));
    *xml += "</Properties>\n";
    if let Some(mesh) = n["mesh"].as_u64() {
        let primitive = &gltf["meshes"][mesh as usize]["primitives"][0];
        let position = &gltf["accessors"][primitive["attributes"]["POSITION"].as_u64().unwrap_or(u64::MAX) as usize];
        let axis = |k: usize| {
            let at = |v: &Value| v[k].as_f64().unwrap_or(0.0);
            ((at(&position["max"]) - at(&position["min"])) * n["scale"][k].as_f64().unwrap_or(1.0).abs()) as f32
        };
        open(xml, refs, "MeshPart", name);
        let mesh = id + 1_000_000_000_000 + i as u64;
        let _ = writeln!(xml, "<Content name=\"MeshContent\"><uri>rbxassetid://{mesh}</uri></Content>");
        let (x, y, z) = (axis(0), axis(1), axis(2));
        let _ = writeln!(xml, "<Vector3 name=\"InitialSize\"><X>{x}</X><Y>{y}</Y><Z>{z}</Z></Vector3>\n</Properties>");
        if let Some(material) = primitive["material"].as_u64() {
            let m = &gltf["materials"][material as usize];
            let pbr = &m["pbrMetallicRoughness"];
            let maps = [
                ("ColorMapContent", &pbr["baseColorTexture"]),
                ("NormalMapContent", &m["normalTexture"]),
                ("RoughnessMapContent", &pbr["metallicRoughnessTexture"]),
                ("MetalnessMapContent", &pbr["metallicRoughnessTexture"]),
            ];
            if maps.iter().any(|(_, t)| !t.is_null()) {
                open(xml, refs, "SurfaceAppearance", "SurfaceAppearance");
                for (channel, (property, _)) in maps.iter().enumerate().filter(|(_, (_, t))| !t.is_null()) {
                    let map = id + 2_000_000_000_000 + 4 * material + channel as u64;
                    let _ = writeln!(xml, "<Content name=\"{property}\"><uri>rbxassetid://{map}</uri></Content>");
                }
                *xml += "</Properties>\n</Item>\n";
            }
        }
        *xml += "</Item>\n";
    }
    for child in n["children"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
        node(xml, refs, id, gltf, child as usize);
    }
    *xml += "</Item>\n";
}

/// Opens an `<Item>` and its `<Properties>` with its Name; the caller adds properties and closes both.
fn open(xml: &mut String, refs: &mut usize, class: &str, name: &str) {
    let _ = write!(
        xml,
        "<Item class=\"{class}\" referent=\"RBX{refs}\">\n<Properties>\n<string name=\"Name\">{name}</string>\n"
    );
    *refs += 1;
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
            description: "a.png",
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

    /// A glb as Blender exports it, but only the JSON chunk the mock reads.
    fn glb(gltf: &Value) -> Vec<u8> {
        let mut json = gltf.to_string().into_bytes();
        json.resize(json.len().div_ceil(4) * 4, b' ');
        let mut out = b"glTF".to_vec();
        for n in [2, 20 + json.len() as u32, json.len() as u32] {
            out.extend(n.to_le_bytes());
        }
        out.extend(b"JSON");
        out.extend(json);
        out
    }

    #[test]
    fn a_created_glb_reads_back_as_its_import_without_the_key() {
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let c = Client::new("test-key", &mock.base, Clock::Real).unwrap();
        let gltf = json!({
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": "hull", "mesh": 0, "children": [1]}, {"name": "Lens", "mesh": 1, "scale": [2, 2, 2]}],
            "meshes": [
                {"name": "hull", "primitives": [{"attributes": {"POSITION": 0}, "material": 0}]},
                {"name": "Lens", "primitives": [{"attributes": {"POSITION": 1}, "material": 0}]},
            ],
            "accessors": [{"min": [-1, -2, -3], "max": [1, 2, 3]}, {"min": [0, 0, 0], "max": [0.25, 0.5, 0.125]}],
            "materials": [{
                "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicRoughnessTexture": {"index": 1}},
                "normalTexture": {"index": 2},
            }],
        });
        let bytes = glb(&gltf);
        let model = Upload {
            kind: AssetKind::Model,
            name: "lantern",
            description: "lantern.glb",
            creator: Creator::User(1),
            ext: ".glb",
            mime: "model/gltf-binary",
            bytes: &bytes,
        };
        let op = c.create_asset(&model).unwrap();
        let id = c.wait_operation(&op.path, c.now(), &std::sync::atomic::AtomicBool::new(false)).unwrap().asset_id;
        let xml = String::from_utf8(c.asset_content(id.unwrap()).unwrap()).unwrap();

        // Lens_Node sits in hull_Node after hull's MeshPart, and the two parts share their one material's four maps
        let ids: Vec<&str> = xml.split("rbxassetid://").skip(1).map(|s| &s[..s.find('<').unwrap()]).collect();
        let maps = ["2000001000001", "2000001000002", "2000001000003", "2000001000004"];
        assert_eq!(ids, [&["1000001000001"][..], &maps, &["1000001000002"], &maps].concat());
        let at = |s: &str| xml.find(s).unwrap();
        assert!(at("hull_Node") < at(">hull<") && at(">hull<") < at("Lens_Node") && at("Lens_Node") < at(">Lens<"));
        assert_eq!(xml.matches("class=\"MeshPart\"").count(), 2);
        assert!(xml.contains("<X>2</X><Y>4</Y><Z>6</Z>") && xml.contains("<X>0.5</X><Y>1</Y><Z>0.25</Z>"));

        let requests = mock.requests();
        let sent = |path: &str| requests.iter().find(|r| r.path.starts_with(path)).unwrap().header("x-api-key");
        assert_eq!(sent("asset-delivery-api/"), Some("test-key"));
        assert_eq!(sent("_mock/content/"), None);
    }
}
