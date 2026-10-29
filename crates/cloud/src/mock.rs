//! A fake Open Cloud on loopback, std only, for tests: a listener on `127.0.0.1:0` and one thread per connection, every
//! answer with `Connection: close` (a kept-alive connection would hold its thread). A test queues [`Reply`]s per method
//! and path prefix; each request takes the first queued reply it matches, and is recorded with its method, path, raw
//! query, headers and body. An in-flight gauge keeps its peak, so tests can pin concurrency limits.

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

struct Shared {
    /// Method, path prefix and reply, in the order queued.
    queue: Mutex<Vec<(String, String, Reply)>>,
    requests: Mutex<Vec<Request>>,
    flight: AtomicUsize,
    peak: AtomicUsize,
    stop: AtomicBool,
}

pub struct Mock {
    /// `http://127.0.0.1:<port>`, what a [`crate::Client`] is pointed at.
    pub base: String,
    addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Mock {
    pub fn start() -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock open cloud: bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("mock open cloud: local address");
        let shared = Arc::new(Shared {
            queue: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
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
        Mock { base: format!("http://{addr}"), addr, shared }
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
    shared.requests.lock().unwrap().push(request.clone());
    let flight = shared.flight.fetch_add(1, Ordering::SeqCst) + 1;
    shared.peak.fetch_max(flight, Ordering::SeqCst);
    let reply = {
        let mut queue = shared.queue.lock().unwrap();
        let at = queue.iter().position(|(m, p, _)| *m == request.method && request.path.starts_with(p.as_str()));
        at.map(|i| queue.remove(i).2)
    };
    let reply = reply.unwrap_or_else(|| {
        let message = format!("mock: nothing queued for {} {}", request.method, request.path);
        Reply::json(404, &json!({"error": {"code": "NOT_FOUND", "message": message}}))
    });
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
