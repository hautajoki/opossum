//! The server half of RFC 6455 over a blocking `TcpStream`, small enough to split: the handshake, then a reader
//! that takes masked client frames and a writer (another handle of the same stream) that sends unmasked ones. The
//! same listener answers one plain HTTP `POST` per connection (`opossum exec`), behind the same checks.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_HEAD: usize = 16 << 10;
/// Upstream is acks, logs and exec results (the plugin cuts those to a few MB); anything bigger is not the plugin.
const MAX_MESSAGE: usize = 32 << 20;

pub const TEXT: u8 = 1;
pub const BINARY: u8 = 2;
pub const CLOSE: u8 = 8;
pub const PING: u8 = 9;
pub const PONG: u8 = 10;

/// Origins Studio sends on its WebSocket handshake. A browser page always sends its own, so any other is refused.
pub const STUDIO_ORIGINS: &[&str] = &[];

/// What a connection asked for.
pub enum Request {
    /// A WebSocket, upgraded: its reader.
    Socket(BufReader<TcpStream>),
    /// A plain HTTP POST to `path`, its body read; the caller answers it with [`respond`].
    Post { path: String, body: Vec<u8> },
}

/// Reads the request head and answers an upgrade with 101, when `Host` is this machine on `port` and `Origin` is
/// absent or Studio's; a POST passes the same checks. Anything else gets 403 or 400 with the reason. A browser
/// always sends `Origin` on a cross-site POST, and `Host` defeats DNS rebinding.
pub fn accept(stream: &mut TcpStream, port: u16) -> Result<Request, String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let (mut host, mut origin, mut key, mut upgrade, mut length) = (None, None, None, false, 0);
    let mut post = None;
    let mut size = 0;
    let mut first = true;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        size += n;
        if n == 0 || size > MAX_HEAD {
            return Err("bad request".into());
        }
        let line = line.trim_end();
        if first {
            first = false;
            if let Some(rest) = line.strip_prefix("POST ") {
                post = rest.split(' ').next().map(str::to_owned);
            } else if !line.starts_with("GET ") {
                return refuse(stream, 400, "expected a WebSocket upgrade");
            }
            continue;
        }
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim().to_owned();
        match name.trim().to_ascii_lowercase().as_str() {
            "host" => host = Some(value),
            "origin" => origin = Some(value),
            "sec-websocket-key" => key = Some(value),
            "upgrade" => upgrade = value.eq_ignore_ascii_case("websocket"),
            "content-length" => length = value.parse().unwrap_or(usize::MAX),
            _ => {}
        }
    }
    let local = host.as_deref().is_some_and(|h| {
        let (name, p) = h.rsplit_once(':').map_or((h, None), |(n, p)| (n, Some(p)));
        matches!(name, "127.0.0.1" | "localhost") && p.is_none_or(|p| p.parse() == Ok(port))
    });
    if !local {
        return refuse(stream, 403, "opossum only answers 127.0.0.1 and localhost");
    }
    if let Some(origin) = &origin
        && !STUDIO_ORIGINS.contains(&origin.as_str())
    {
        return refuse(stream, 403, &format!("refused a socket from {origin}"));
    }
    if let Some(path) = post {
        if length > MAX_MESSAGE {
            return refuse(stream, 400, "the body is too big");
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        return Ok(Request::Post { path, body });
    }
    let (Some(key), true) = (key, upgrade) else {
        return refuse(stream, 400, "expected a WebSocket upgrade");
    };
    let accept = base64(&sha1(format!("{key}{GUID}").as_bytes()));
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    Ok(Request::Socket(reader))
}

fn refuse<T>(stream: &mut TcpStream, status: u16, reason: &str) -> Result<T, String> {
    respond(stream, status, "text/plain", reason);
    Err(reason.to_owned())
}

/// Answers a request and closes the connection.
pub fn respond(stream: &mut TcpStream, status: u16, kind: &str, body: &str) {
    let text = match status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Bad Request",
    };
    let head = format!(
        "HTTP/1.1 {status} {text}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body.as_bytes()));
}

/// The next whole message (fragments joined) as (opcode, payload). Control frames come back as they arrive.
pub fn read(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut message: Option<(u8, Vec<u8>)> = None;
    loop {
        let mut head = [0u8; 2];
        r.read_exact(&mut head)?;
        let (fin, op) = (head[0] & 0x80 != 0, head[0] & 0x0f);
        let masked = head[1] & 0x80 != 0;
        let len = match head[1] & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                r.read_exact(&mut b)?;
                u64::from(u16::from_be_bytes(b))
            }
            127 => {
                let mut b = [0u8; 8];
                r.read_exact(&mut b)?;
                u64::from_be_bytes(b)
            }
            n => u64::from(n),
        };
        let pending = message.as_ref().map_or(0, |m| m.1.len());
        if !masked || len as usize > MAX_MESSAGE - pending {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad client frame"));
        }
        let mut mask = [0u8; 4];
        r.read_exact(&mut mask)?;
        let mut payload = vec![0u8; len as usize];
        r.read_exact(&mut payload)?;
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
        if op >= CLOSE {
            return Ok((op, payload));
        }
        match (&mut message, op) {
            (None, 0) => return Err(io::Error::new(io::ErrorKind::InvalidData, "continuation without a start")),
            (None, op) => message = Some((op, payload)),
            (Some(m), 0) => m.1.extend_from_slice(&payload),
            (Some(_), _) => return Err(io::Error::new(io::ErrorKind::InvalidData, "interleaved message")),
        }
        if fin {
            return Ok(message.take().expect("a message was started"));
        }
    }
}

/// One unmasked frame.
pub fn write(w: &mut impl Write, op: u8, payload: &[u8]) -> io::Result<()> {
    let mut head = Vec::with_capacity(10);
    head.push(0x80 | op);
    match payload.len() {
        n if n < 126 => head.push(n as u8),
        n if n <= 0xffff => {
            head.push(126);
            head.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            head.push(127);
            head.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    w.write_all(&head)?;
    w.write_all(payload)?;
    w.flush()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// SHA-1, for `Sec-WebSocket-Accept` only.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..20 => ((b & c) | (!b & d), 0x5A827999),
                20..40 => (b ^ c ^ d, 0x6ED9EBA1),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn accept_key_matches_rfc_6455() {
        // the example in RFC 6455 section 1.3
        let digest = super::sha1(b"dGhlIHNhbXBsZSBub25jZQ==258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        assert_eq!(super::base64(&digest), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
