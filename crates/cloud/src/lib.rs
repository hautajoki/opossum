//! The Open Cloud client: HTTP and Roblox's wire formats for assets, place versions, Luau execution and resources,
//! with its retry, timeout and error rules, plus the mock server tests run against. It knows nothing about project
//! files, the lock or config; bodies are bytes the caller supplies.
//!
//! Every endpoint is a [`Call`] through [`Client::send`]: one place holds the retry table, the timeouts, the body caps
//! and the error mapping, so an endpoint is its validation, its body and its decode.

// An Error is a few strings on the failure path only, next to a network round trip; boxing it buys nothing.
#![allow(clippy::result_large_err)]

mod assets;
mod execution;
pub mod mock;
mod places;
mod resources;

pub use assets::{AssetKind, Creator, MAX_ASSET, OPERATION_DEADLINE, Operation, Upload};
pub use execution::{MAX_SCRIPT, MAX_TASK_TIMEOUT, Task};
pub use places::MAX_PLACE;
pub use resources::{BadgeChange, Image, MAX_THUMBNAILS, NewBadge, Sale};

use std::fmt;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use ureq::http;

/// Attempts per request, and the backoff between them: 250 ms doubling, capped at 2 s, no jitter.
const ATTEMPTS: u32 = 4;
const BACKOFF: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(2);
/// The longest `Retry-After` honoured. A longer one fails as `cloud.rateLimit` instead of sleeping for an hour.
const RETRY_AFTER_MAX: Duration = Duration::from_secs(60);
/// Response bodies read for a decode, and for an error's code and message.
const BODY_LIMIT: u64 = 8 << 20;
const ERROR_LIMIT: u64 = 64 << 10;
/// The place save's op name: its 409 means the place is open in Studio.
const SAVE_PLACE: &str = "save place";

/// Roblox's Open Cloud: the only base a client sends to, besides the mock on this machine ([`loopback`]).
pub const ROBLOX: &str = "https://apis.roblox.com";

/// Time for retries and polls. A `Fake` sleeps by moving its own clock forward, so tests record every wait and never
/// block; one is shared by every thread of a test.
pub enum Clock {
    Real,
    Fake(Arc<Fake>),
}

pub struct Fake {
    start: Instant,
    offset: Mutex<Duration>,
    slept: Mutex<Vec<Duration>>,
}

impl Fake {
    pub fn new() -> Arc<Fake> {
        Arc::new(Fake { start: Instant::now(), offset: Mutex::new(Duration::ZERO), slept: Mutex::new(Vec::new()) })
    }

    /// Every sleep so far, in order.
    pub fn slept(&self) -> Vec<Duration> {
        self.slept.lock().unwrap().clone()
    }
}

impl Clock {
    pub fn now(&self) -> Instant {
        match self {
            Clock::Real => Instant::now(),
            Clock::Fake(fake) => fake.start + *fake.offset.lock().unwrap(),
        }
    }

    pub fn sleep(&self, d: Duration) {
        match self {
            Clock::Real => std::thread::sleep(d),
            Clock::Fake(fake) => {
                *fake.offset.lock().unwrap() += d;
                fake.slept.lock().unwrap().push(d);
            }
        }
    }
}

/// Per-phase limits on every request, so no request hangs on a dead connection. Sending a body gets `send` plus a
/// second per MiB; uploads wait `recv_long` for Roblox's answer, everything else `recv`. Tests shorten them to see a
/// hung server quickly.
pub struct Timeouts {
    pub connect: Duration,
    pub send: Duration,
    pub recv: Duration,
    pub recv_long: Duration,
    pub body: Duration,
}

pub struct Client {
    base: String,
    key: String,
    /// GETs and idempotent writes reuse warm connections; creates always open their own (see [`ErrorKind::Connect`]).
    pooled: ureq::Agent,
    fresh: ureq::Agent,
    clock: Clock,
    pub timeouts: Timeouts,
}

#[derive(Debug)]
pub struct Error {
    pub op: &'static str,
    pub method: &'static str,
    pub url: String,
    pub kind: ErrorKind,
    /// Roblox's error code, decoded leniently: a string, a number's decimal text, or any other JSON as its text.
    pub code: String,
    pub message: String,
    pub request_id: String,
    pub retry_after: Option<Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Our own validation of a URL, header, body or path: nothing was sent.
    Local,
    /// Connection setup failed (resolve, connect, TLS) before any request byte. Creates go out on fresh connections
    /// only, so this never means "an idle keep-alive connection was reaped mid-request".
    Connect,
    /// Roblox answered with this status (a 3xx too: redirects are never followed).
    Status(u16),
    /// The exchange broke after the request may have reached Roblox: a timeout, a reset, a truncated answer.
    Transport,
    /// A 2xx whose body is too big, not UTF-8 or not the JSON expected.
    Decode,
}

/// One HTTP exchange as the retry loop sees it. `path` is the resource under the base URL, with its query and no
/// leading `/`. A `create` is not idempotent: it goes out on a fresh connection and is never resent once a byte may
/// have reached Roblox. `long` waits [`Timeouts::recv_long`] for the answer (uploads Roblox processes before replying).
pub(crate) struct Call<'a> {
    pub op: &'static str,
    pub method: &'static str,
    pub path: String,
    /// Content-Type and bytes; `None` sends no body.
    pub body: Option<(&'a str, &'a [u8])>,
    pub create: bool,
    pub long: bool,
}

impl Client {
    /// The only constructor. `base` is [`ROBLOX`] or a [`loopback`] mock, so a client can talk to nothing else.
    pub fn new(key: &str, base: &str, clock: Clock) -> Result<Client, String> {
        let key = key.trim();
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("the Open Cloud API key is empty or holds characters an HTTP header cannot carry".into());
        }
        if base != ROBLOX && !loopback(base) {
            return Err(format!("the Open Cloud base URL {base:?} is not {ROBLOX} or a loopback mock"));
        }
        let timeouts = Timeouts {
            connect: Duration::from_secs(10),
            send: Duration::from_secs(60),
            recv: Duration::from_secs(30),
            recv_long: Duration::from_secs(120),
            body: Duration::from_secs(60),
        };
        Ok(Client { base: base.into(), key: key.into(), pooled: agent(true), fresh: agent(false), clock, timeouts })
    }

    pub fn now(&self) -> Instant {
        self.clock.now()
    }

    /// Where this client sends: [`ROBLOX`] or a loopback mock.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn sleep(&self, d: Duration) {
        self.clock.sleep(d)
    }

    /// Sends `call` under the retry table (DESIGN.md, Cloud) and returns the 2xx body. Connection setup failures and
    /// 429s are retried for every request, since neither was applied; 408, 5xx and broken exchanges only when the
    /// request is not a create.
    pub(crate) fn send(&self, call: &Call) -> Result<Vec<u8>, Error> {
        self.retrying(call, &mut Duration::default())
    }

    /// [`Client::send`], decoding the body as JSON.
    pub(crate) fn json(&self, call: &Call) -> Result<Value, Error> {
        let body = self.send(call)?;
        self.decode(call, &body)
    }

    pub(crate) fn decode(&self, call: &Call, body: &[u8]) -> Result<Value, Error> {
        serde_json::from_slice(body).map_err(|e| self.error(call, ErrorKind::Decode, format!("decode response: {e}")))
    }

    /// [`Client::send`], adding to `waited` the time slept on `Retry-After`, which pollers leave out of their deadlines.
    pub(crate) fn retrying(&self, call: &Call, waited: &mut Duration) -> Result<Vec<u8>, Error> {
        let mut attempt = 0;
        loop {
            let err = match self.once(call) {
                Ok(body) => return Ok(body),
                Err(err) => err,
            };
            let retry = match err.kind {
                ErrorKind::Connect | ErrorKind::Status(429) => true,
                ErrorKind::Status(408 | 500..=599) | ErrorKind::Transport => !call.create,
                _ => false,
            };
            attempt += 1;
            if !retry || attempt == ATTEMPTS {
                return Err(err);
            }
            let wait = match err.retry_after {
                Some(d) if d > RETRY_AFTER_MAX => return Err(err),
                Some(d) => {
                    *waited += d;
                    d
                }
                None => (BACKOFF * (1 << (attempt - 1))).min(BACKOFF_MAX),
            };
            self.clock.sleep(wait);
        }
    }

    /// An error for `call` that is not Roblox's: a validation failure (`Local`, made before anything is sent) or a
    /// body that does not say what it must (`Decode`).
    pub(crate) fn error(&self, call: &Call, kind: ErrorKind, message: String) -> Error {
        Error {
            op: call.op,
            method: call.method,
            url: format!("{}/{}", self.base, call.path),
            kind,
            code: String::new(),
            message,
            request_id: String::new(),
            retry_after: None,
        }
    }

    fn once(&self, call: &Call) -> Result<Vec<u8>, Error> {
        let url = format!("{}/{}", self.base, call.path);
        let agent = if call.create { &self.fresh } else { &self.pooled };
        let t = &self.timeouts;
        let len = call.body.map_or(0, |(_, bytes)| bytes.len() as u64);
        let limits =
            [t.connect, t.send + Duration::from_secs(len >> 20), if call.long { t.recv_long } else { t.recv }, t.body];
        let request = http::Request::builder().method(call.method).uri(&url).header("x-api-key", &self.key);
        let sent = match call.body {
            None => request.body(()).map(|r| exchange(agent, r, limits)),
            Some((mime, bytes)) => request.header("content-type", mime).body(bytes).map(|r| exchange(agent, r, limits)),
        };
        let mut response = match sent {
            Err(e) => return Err(self.error(call, ErrorKind::Local, e.to_string())),
            Ok(Err(e)) => return Err(self.error(call, kind(&e), e.to_string())),
            Ok(Ok(response)) => response,
        };
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return response
                .body_mut()
                .with_config()
                .limit(BODY_LIMIT)
                .read_to_vec()
                .map_err(|e| self.error(call, kind(&e), format!("reading the response: {e}")));
        }
        let header = |name: &str| {
            let value = response.headers().get(name).and_then(|v| v.to_str().ok());
            value.unwrap_or("").trim().to_string()
        };
        let request_id =
            ["x-request-id", "roblox-request-id", "x-correlation-id"].into_iter().map(&header).find(|v| !v.is_empty());
        let retry_after = retry_after(&header("retry-after"), SystemTime::now());
        let (code, message) = if (300..400).contains(&status) {
            (String::new(), format!("redirected to {:?}, which opossum never follows", header("location")))
        } else {
            let mut body = Vec::new();
            let _ = response.body_mut().as_reader().take(ERROR_LIMIT).read_to_end(&mut body);
            api_error(&body)
        };
        Err(Error {
            op: call.op,
            method: call.method,
            url,
            kind: ErrorKind::Status(status),
            code,
            message,
            request_id: request_id.unwrap_or_default(),
            retry_after,
        })
    }
}

/// ureq's defaults contradict this client (it follows redirects, turns 4xx/5xx into body-less errors, reads proxy
/// variables and caps bodies at 10 MB), so the agent is configured here and nowhere else.
fn agent(pool: bool) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .proxy(None)
        .max_idle_connections(if pool { 8 } else { 0 })
        .user_agent(concat!("opossum/", env!("CARGO_PKG_VERSION")))
        .accept("application/json")
        .build()
        .into()
}

/// One request with a limit on every phase: resolve and connect (`connect` each), send the request and its body
/// (`send` each), receive the response and its body.
fn exchange<S: ureq::AsSendBody>(
    agent: &ureq::Agent,
    request: http::Request<S>,
    [connect, send, recv, body]: [Duration; 4],
) -> Result<http::Response<ureq::Body>, ureq::Error> {
    let request = agent
        .configure_request(request)
        .timeout_resolve(Some(connect))
        .timeout_connect(Some(connect))
        .timeout_send_request(Some(send))
        .timeout_send_body(Some(send))
        .timeout_recv_response(Some(recv))
        .timeout_recv_body(Some(body))
        .build();
    agent.run(request)
}

/// Which side of "was anything sent" a ureq error is on (DESIGN.md, Cloud: errors and retries).
fn kind(e: &ureq::Error) -> ErrorKind {
    use std::io::ErrorKind as Io;
    use ureq::{Error as E, Timeout as T};
    match e {
        E::HostNotFound | E::ConnectionFailed | E::Timeout(T::Resolve | T::Connect) | E::Tls(_) | E::Rustls(_) => {
            ErrorKind::Connect
        }
        // ureq reports a socket's errors from every phase as `Io`, so only kinds a connect alone can produce say nothing
        // was sent: an unreachable host or network also ends an established connection whose retransmits run out
        E::Io(io) if matches!(io.kind(), Io::ConnectionRefused | Io::AddrNotAvailable) => ErrorKind::Connect,
        E::BodyExceedsLimit(_) => ErrorKind::Decode,
        E::Http(_) | E::BadUri(_) | E::RequireHttpsOnly(_) => ErrorKind::Local,
        _ => ErrorKind::Transport,
    }
}

/// `http://127.0.0.1:<port>` or `http://localhost:<port>`, nothing more: where the mock listens.
pub fn loopback(base: &str) -> bool {
    let port = base.strip_prefix("http://127.0.0.1:").or_else(|| base.strip_prefix("http://localhost:"));
    port.is_some_and(|port| port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p > 0))
}

impl Error {
    /// True when the request was certainly not applied, so an intent saved before it can be cleared: it was never
    /// sent, or Roblox refused it with a status that means it did nothing. A 408, 5xx, other 4xx, a broken exchange or
    /// an unreadable 2xx leave a create ambiguous.
    pub fn definite(&self) -> bool {
        match self.kind {
            ErrorKind::Local | ErrorKind::Connect => true,
            ErrorKind::Status(s) => matches!(s, 400 | 401 | 403 | 404 | 409 | 413 | 415 | 422 | 429),
            ErrorKind::Transport | ErrorKind::Decode => false,
        }
    }

    /// The stable diagnostic code commands report.
    pub fn diag(&self) -> &'static str {
        match self.kind {
            ErrorKind::Status(401 | 403) => "cloud.auth",
            ErrorKind::Status(404) => "cloud.notFound",
            ErrorKind::Status(409) if self.op == SAVE_PLACE => "cloud.studioOpen",
            ErrorKind::Status(429) => "cloud.rateLimit",
            ErrorKind::Status(300..=399) => "cloud.redirect",
            _ => "cloud.http",
        }
    }

    fn hint(&self) -> String {
        let ErrorKind::Status(status) = self.kind else { return String::new() };
        let text = match status {
            400 => "check the resource ids, request fields and uploaded file format",
            401 => "the API key is invalid or expired",
            403 => "the API key lacks a scope for this request, or permission on this creator or universe",
            404 => "check that the universe, place, version and task ids are correct",
            409 if self.op == SAVE_PLACE => {
                "the place is open in Studio (Roblox answered 409 Server is busy); close it there and retry"
            }
            409 => "the request conflicts with the resource's current state",
            429 => match self.retry_after {
                Some(d) if d > RETRY_AFTER_MAX => {
                    return format!(
                        "Roblox asked to wait {} s, longer than opossum waits; try again later",
                        d.as_secs()
                    );
                }
                _ => "the Open Cloud rate or concurrency limit was reached",
            },
            500.. => "Roblox Open Cloud encountered a server error",
            _ => "",
        };
        text.into()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "open cloud {}: {} {}", self.op, self.method, self.url)?;
        match self.kind {
            ErrorKind::Status(status) => write!(f, " returned {status}")?,
            _ => f.write_str(" failed")?,
        }
        if !self.code.is_empty() {
            write!(f, " ({})", self.code)?;
        }
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        let hint = self.hint();
        if !hint.is_empty() {
            write!(f, "; {hint}")?;
        }
        if !self.request_id.is_empty() {
            write!(f, " [request {}]", self.request_id)?;
        }
        Ok(())
    }
}

/// Roblox's code and message from an error body, first of `{"error":{code,message}}`, `{"errors":[{…}]}` and a
/// top-level `{code,message}`; a body that says neither is the message as it is.
fn api_error(body: &[u8]) -> (String, String) {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let said = |o: &Value| o.get("code").is_some() || o.get("message").is_some();
    let o = if said(&v["error"]) {
        &v["error"]
    } else if v["errors"].as_array().is_some_and(|a| !a.is_empty()) {
        &v["errors"][0]
    } else {
        &v
    };
    let message = o["message"].as_str().filter(|m| !m.is_empty()).unwrap_or(text);
    (code(&o["code"]), message.into())
}

/// A code Roblox sent as a string, a number or anything else (an operation once answered `{"code":"Unknown"}` where
/// its schema says integer, and a strict decode of it wedged every later upload).
pub(crate) fn code(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

/// A positive id Roblox sent as a JSON number or a string (protobuf JSON spells int64 as strings).
pub(crate) fn id(v: &Value) -> Option<u64> {
    let id = v.as_u64().or_else(|| v.as_str()?.trim().parse().ok())?;
    (id > 0).then_some(id)
}

/// `Retry-After` as seconds or an HTTP date; unparseable is absent.
fn retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = http_date(value)?;
    let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(at.saturating_sub(now)))
}

/// An IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`, the only form RFC 9110 lets servers send) as unix seconds.
fn http_date(s: &str) -> Option<u64> {
    let [_, day, month, year, time, "GMT"] = s.split_ascii_whitespace().collect::<Vec<_>>()[..] else { return None };
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|m| *m == month)? as i64
        + 1;
    let (day, year): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let mut hms = time.split(':').map(|n| n.parse::<i64>().ok());
    let (h, m, sec) = (hms.next()??, hms.next()??, hms.next()??);
    // days from the civil date (Howard Hinnant's algorithm)
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + m * 60 + sec).ok()
}

/// One form-data part: a text field when `filename` is empty, else a file with its MIME.
pub(crate) struct Part<'a> {
    pub name: &'a str,
    pub filename: &'a str,
    pub mime: &'a str,
    pub bytes: &'a [u8],
}

/// A multipart/form-data body and its Content-Type, built in memory so the length is exact and nothing is chunked.
/// The boundary hashes the content, so the bytes are deterministic and no part can contain it in practice. Names and
/// filenames are ours (`asset.png`), never a user's, so nothing needs quoting.
pub(crate) fn multipart(parts: &[Part]) -> (String, Vec<u8>) {
    let mut hash = blake3::Hasher::new();
    for part in parts {
        hash.update(part.name.as_bytes());
        hash.update(part.bytes);
    }
    let boundary = format!("opossum-{}", &hash.finalize().to_hex()[..32]);
    let mut body = Vec::with_capacity(parts.iter().map(|p| p.bytes.len() + 160).sum::<usize>() + 64);
    for part in parts {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"", part.name).as_bytes(),
        );
        if !part.filename.is_empty() {
            body.extend_from_slice(
                format!("; filename=\"{}\"\r\nContent-Type: {}", part.filename, part.mime).as_bytes(),
            );
        }
        body.extend_from_slice(b"\r\n\r\n");
        body.extend_from_slice(part.bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{Mock, Reply};
    use serde_json::json;

    fn client(base: &str) -> (Client, Arc<Fake>) {
        let fake = Fake::new();
        (Client::new("test-key", base, Clock::Fake(fake.clone())).unwrap(), fake)
    }

    fn get(path: &str) -> Call<'static> {
        Call { op: "get thing", method: "GET", path: path.into(), body: None, create: false, long: false }
    }

    fn create(path: &str) -> Call<'static> {
        Call {
            op: "create thing",
            method: "POST",
            path: path.into(),
            body: Some(("application/json", b"{}")),
            create: true,
            long: true,
        }
    }

    #[test]
    fn retries_follow_retry_after_then_back_off() {
        let mock = Mock::start();
        mock.on("GET", "x", Reply::json(429, &json!({})).header("Retry-After", "3"));
        mock.on("GET", "x", Reply::json(503, &json!({})));
        mock.on("GET", "x", Reply::json(200, &json!({"ok": true})));
        let (c, fake) = client(&mock.base);
        let mut waited = Duration::ZERO;
        let body = c.retrying(&get("x"), &mut waited).unwrap();
        assert_eq!(body, br#"{"ok":true}"#);
        assert_eq!(fake.slept(), [Duration::from_secs(3), Duration::from_millis(500)]);
        assert_eq!(waited, Duration::from_secs(3));
    }

    #[test]
    fn a_retry_after_past_a_minute_fails_without_sleeping() {
        let mock = Mock::start();
        mock.on("POST", "x", Reply::json(429, &json!({})).header("Retry-After", "3600"));
        let (c, fake) = client(&mock.base);
        let err = c.send(&create("x")).unwrap_err();
        assert_eq!(err.diag(), "cloud.rateLimit");
        assert!(err.definite());
        assert!(fake.slept().is_empty());
        assert!(err.to_string().contains("asked to wait 3600 s"), "{err}");
    }

    #[test]
    fn a_create_is_retried_only_when_it_was_certainly_not_applied() {
        let mock = Mock::start();
        mock.on("POST", "x", Reply::json(429, &json!({})));
        mock.on("POST", "x", Reply::json(503, &json!({})));
        let (c, fake) = client(&mock.base);
        let err = c.send(&create("x")).unwrap_err();
        assert_eq!((err.kind, err.definite()), (ErrorKind::Status(503), false));
        assert_eq!(fake.slept(), [Duration::from_millis(250)]);

        mock.on("POST", "y", Reply::Reset);
        let err = c.send(&create("y")).unwrap_err();
        assert_eq!((err.kind, err.definite()), (ErrorKind::Transport, false));
        mock.on("POST", "y", Reply::json(408, &json!({})));
        assert!(!c.send(&create("y")).unwrap_err().definite());
        assert_eq!(mock.requests().len(), 4);

        let (c, fake) = client(&crate::mock::refused());
        let err = c.send(&create("x")).unwrap_err();
        assert_eq!((err.kind, err.definite()), (ErrorKind::Connect, true));
        assert_eq!(fake.slept(), [Duration::from_millis(250), Duration::from_millis(500), Duration::from_millis(1000)]);
    }

    /// An unreachable host can end an established connection after a create went out, so it never says "not sent".
    #[test]
    fn only_errors_a_connect_alone_produces_mean_nothing_was_sent() {
        let io = |k: std::io::ErrorKind| kind(&ureq::Error::Io(k.into()));
        assert_eq!(io(std::io::ErrorKind::ConnectionRefused), ErrorKind::Connect);
        assert_eq!(io(std::io::ErrorKind::HostUnreachable), ErrorKind::Transport);
        assert_eq!(io(std::io::ErrorKind::NetworkUnreachable), ErrorKind::Transport);
    }

    #[test]
    fn a_get_survives_a_reset() {
        let mock = Mock::start();
        mock.on("GET", "x", Reply::Reset);
        mock.on("GET", "x", Reply::json(200, &json!(1)));
        let (c, fake) = client(&mock.base);
        assert_eq!(c.json(&get("x")).unwrap(), json!(1));
        assert_eq!(fake.slept(), [Duration::from_millis(250)]);
    }

    #[test]
    fn a_hung_create_is_ambiguous_after_the_receive_timeout() {
        let mock = Mock::start();
        mock.on("POST", "x", Reply::Hang);
        let (mut c, fake) = client(&mock.base);
        c.timeouts.recv_long = Duration::from_millis(200);
        let err = c.send(&create("x")).unwrap_err();
        assert_eq!((err.kind, err.definite(), err.diag()), (ErrorKind::Transport, false, "cloud.http"));
        assert!(fake.slept().is_empty());
    }

    #[test]
    fn error_bodies_parse_in_every_shape_roblox_uses() {
        let cases = [
            (json!({"error": {"code": "INTERNAL", "message": "try later"}}), "INTERNAL", "try later"),
            (json!({"errors": [{"code": 7, "message": "first"}, {"code": 8}]}), "7", "first"),
            (json!({"code": "NOT_FOUND", "message": "no task"}), "NOT_FOUND", "no task"),
            (json!({"code": {"odd": 1}}), "{\"odd\":1}", "{\"code\":{\"odd\":1}}"),
        ];
        for (body, code, message) in cases {
            assert_eq!(api_error(body.to_string().as_bytes()), (code.to_string(), message.to_string()));
        }
        assert_eq!(api_error(b" Server is busy \n"), (String::new(), "Server is busy".into()));

        let mock = Mock::start();
        let body = json!({"error": {"code": "INTERNAL", "message": "try later"}});
        mock.on("POST", "x", Reply::json(500, &body).header("roblox-request-id", "request-7"));
        let (c, _) = client(&mock.base);
        let err = c.send(&create("x")).unwrap_err();
        let want = format!(
            "open cloud create thing: POST {}/x returned 500 (INTERNAL): try later; Roblox Open Cloud encountered a server error [request request-7]",
            mock.base
        );
        assert_eq!(err.to_string(), want);
    }

    #[test]
    fn a_redirect_is_an_error_and_its_target_never_sees_the_key() {
        let target = Mock::start();
        let mock = Mock::start();
        mock.on("GET", "x", Reply::json(302, &json!({})).header("Location", &format!("{}/x", target.base)));
        let (c, _) = client(&mock.base);
        let err = c.send(&get("x")).unwrap_err();
        assert_eq!(err.diag(), "cloud.redirect");
        assert!(target.requests().is_empty());
    }

    #[test]
    fn a_response_past_the_cap_is_a_decode_error() {
        let mock = Mock::start();
        mock.on("GET", "x", Reply::Status { status: 200, headers: vec![], body: vec![b' '; (8 << 20) + 1] });
        let (c, _) = client(&mock.base);
        assert_eq!(c.send(&get("x")).unwrap_err().kind, ErrorKind::Decode);
    }

    #[test]
    fn a_client_talks_to_roblox_or_a_loopback_mock_and_nothing_else() {
        for base in [ROBLOX, "http://127.0.0.1:4100", "http://localhost:80"] {
            assert!(Client::new("k", base, Clock::Real).is_ok(), "{base}");
        }
        for base in [
            "http://apis.roblox.com",
            "https://apis.roblox.com.evil.com",
            "http://127.0.0.1.evil.com:80",
            "http://127.0.0.1:80/x",
            "http://localhost:0",
            "https://h",
            "",
        ] {
            assert!(Client::new("k", base, Clock::Real).is_err(), "{base}");
        }
        assert!(Client::new(" \n", ROBLOX, Clock::Real).is_err());
        assert!(Client::new("a\r\nX: y", ROBLOX, Clock::Real).is_err());
    }

    #[test]
    fn http_dates_are_unix_seconds() {
        assert_eq!(http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784_111_777));
        let at = UNIX_EPOCH + Duration::from_secs(784_111_770);
        assert_eq!(retry_after("Sun, 06 Nov 1994 08:49:37 GMT", at), Some(Duration::from_secs(7)));
    }
}
