//! One socket end to end, with tungstenite as the plugin: the handshake, the hello exchange, an adopting verify, an
//! edit, a repair after a failed ack, and a late ack after a verify.

use std::net::TcpStream;
use std::sync::Arc;

use place::{ScriptKind, Source, Tree};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

type Socket = WebSocket<TcpStream>;

fn tree(source: &str) -> Tree {
    let mut fresh = Tree::new();
    fresh.set_script("ReplicatedStorage/shared/a", ScriptKind::Module, Source::new(Arc::from(source)), None).unwrap();
    let mut tree = Tree::new();
    tree.update(fresh);
    tree
}

fn connect(port: u16, origin: Option<&str>) -> Result<Socket, tungstenite::Error> {
    let mut request = format!("ws://127.0.0.1:{port}/").into_client_request()?;
    if let Some(origin) = origin {
        request.headers_mut().insert("Origin", origin.parse().unwrap());
    }
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    tungstenite::client(request, stream).map(|(socket, _)| socket).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => e,
        tungstenite::HandshakeError::Interrupted(_) => unreachable!("a blocking stream"),
    })
}

/// A decoded frame: the header, the blob's root classes, and each op as its tag and fields.
#[derive(Debug)]
struct Frame {
    generation: u32,
    base: u32,
    last: bool,
    blob: Vec<String>,
    ops: Vec<(u8, Vec<Field>)>,
}

#[derive(Debug, PartialEq)]
enum Field {
    U(u32),
    S(String),
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        head
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take(2).try_into().unwrap())
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn str(&mut self) -> String {
        let n = self.u32() as usize;
        String::from_utf8(self.take(n).to_vec()).unwrap()
    }
    fn node(&mut self, out: &mut Vec<Field>, children: bool) {
        out.push(Field::U(self.u32()));
        if children {
            for _ in 0..self.u16() {
                out.push(Field::S(self.str()));
            }
        } else {
            out.push(Field::S(self.str()));
            out.push(Field::S(self.str()));
        }
        let n = self.u32();
        for _ in 0..n {
            self.node(out, children);
        }
    }
}

fn decode(bytes: &[u8]) -> Frame {
    let mut r = Reader(bytes);
    let (generation, base, last) = (r.u32(), r.u32(), r.u8() == 1);
    let len = r.u32() as usize;
    let blob = r.take(len).to_vec();
    let blob = if blob.is_empty() {
        Vec::new()
    } else {
        let dom = rbx_binary::from_reader(blob.as_slice()).unwrap();
        dom.root().children().iter().map(|&c| dom.get_by_ref(c).unwrap().class.to_string()).collect()
    };
    let mut ops = Vec::new();
    for _ in 0..r.u32() {
        let tag = r.u8();
        let mut f = Vec::new();
        match tag {
            sync::BIND => f.extend([Field::U(r.u32()), Field::U(r.u32()), Field::S(r.str()), Field::S(r.str())]),
            sync::HYDRATE | sync::CHILDREN => {
                f.push(Field::U(r.u32()));
                if tag == sync::CHILDREN {
                    f.push(Field::U(u32::from(r.u8())));
                }
                for _ in 0..r.u32() {
                    r.node(&mut f, tag == sync::CHILDREN);
                }
            }
            sync::PROPS => {
                f.extend([Field::U(r.u32()), Field::U(u32::from(r.u8()))]);
                for _ in 0..r.u16() {
                    f.push(Field::S(r.str()));
                }
            }
            sync::SOURCE | sync::STAMP => f.extend([Field::U(r.u32()), Field::S(r.str())]),
            sync::MOVE | sync::REF => f.extend([Field::U(r.u32()), Field::S(r.str()), Field::U(r.u32())]),
            sync::REMOVE | sync::SWAP => f.push(Field::U(r.u32())),
            sync::RELOAD => {
                for _ in 0..r.u32() {
                    f.push(Field::U(r.u32()));
                }
            }
            _ => panic!("unknown tag {tag}"),
        }
        ops.push((tag, f));
    }
    assert!(r.0.is_empty(), "trailing bytes");
    Frame { generation, base, last, blob, ops }
}

fn frame(socket: &mut Socket) -> Frame {
    match socket.read().unwrap() {
        Message::Binary(bytes) => decode(&bytes),
        other => panic!("expected a frame, got {other:?}"),
    }
}

fn send(socket: &mut Socket, json: serde_json::Value) {
    socket.send(Message::text(json.to_string())).unwrap();
}

fn tags(frame: &Frame) -> Vec<u8> {
    frame.ops.iter().map(|(t, _)| *t).collect()
}

#[test]
fn socket_session() {
    let server = sync::Server::start(sync::Config {
        project: "test".into(),
        place: 0,
        unpublished: true,
        hot_reload: false,
        port: 0,
        log: None,
    })
    .unwrap();
    let port = server.port();
    let mut cur = tree("return 1");

    // a browser page names its origin
    match connect(port, Some("http://evil.example")) {
        Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 403),
        other => panic!("expected a 403, got {:?}", other.map(|_| ())),
    }

    let mut socket = connect(port, None).unwrap();
    let hello: serde_json::Value = match socket.read().unwrap() {
        Message::Text(text) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected the hello, got {other:?}"),
    };
    assert_eq!((hello["t"].as_str(), hello["unpublished"].as_bool()), (Some("hello"), Some(true)));

    // a place made by `opossum build` carries no stamps, so the hello lists no root
    let rs = cur.find("ReplicatedStorage").unwrap();
    let shared = cur.find("ReplicatedStorage/shared").unwrap();
    let a = cur.find("ReplicatedStorage/shared/a").unwrap();
    send(&mut socket, serde_json::json!({"t": "hello", "role": "edit", "place": 0, "roots": {}}));
    // the plugin connected before the first build: nothing is sent until it is published, as the empty tree would
    // remove every stamped root
    std::thread::sleep(std::time::Duration::from_millis(50));
    server.publish(cur.clone());
    let verify = frame(&mut socket);
    assert!(verify.last);
    // the root is verified under its frame of reference, which adopts the unstamped `shared` the build left there
    // rather than inserting a second one beside it
    assert_eq!(tags(&verify), [sync::BIND, sync::CHILDREN, sync::STAMP]);
    let (_, children) = &verify.ops[1];
    // reconcile + authoritative under ReplicatedStorage: `shared` (no properties), then `a` with its Source
    assert_eq!(children[..4], [Field::U(rs), Field::U(3), Field::U(shared), Field::U(a)]);
    assert!(children.contains(&Field::S("Source".into())));
    assert_eq!(verify.blob, ["Folder"]);
    let stamp = &verify.ops[2].1;
    assert_eq!(stamp[1], Field::S(sync::hex(cur.get(shared).unwrap().hash)));
    send(&mut socket, serde_json::json!({"t": "ack", "gen": verify.generation, "failed": []}));

    // a leaf edit: one source op, and the root's new stamp
    cur.set_sources(vec![(a, Source::new(Arc::from("return 2")))]);
    let generation = server.publish(cur.clone());
    let edit = frame(&mut socket);
    assert_eq!((edit.generation, edit.base), (generation, verify.generation));
    assert!(edit.blob.is_empty());
    assert_eq!(tags(&edit), [sync::SOURCE, sync::STAMP]);
    assert_eq!(edit.ops[0].1, [Field::U(a), Field::S("return 2".into())]);

    // the source failed to apply: the server sends it again at once
    send(&mut socket, serde_json::json!({"t": "ack", "gen": edit.generation, "failed": [[a, sync::SOURCE, "boom"]]}));
    let repair = frame(&mut socket);
    assert_eq!(repair.base, edit.generation);
    assert_eq!(tags(&repair), [sync::SOURCE, sync::STAMP]);
    assert_eq!(repair.ops[0].1, [Field::U(a), Field::S("return 2".into())]);

    // Resync while the repair is still applying: the wait ends and the root is verified
    send(&mut socket, serde_json::json!({"t": "verify"}));
    let reverify = frame(&mut socket);
    assert_eq!(reverify.base, repair.generation);
    assert!(tags(&reverify).contains(&sync::CHILDREN));

    // the repair's ack arrives late: it is not the verify's, which covers it, so it repairs nothing
    send(&mut socket, serde_json::json!({"t": "ack", "gen": repair.generation, "failed": [[a, sync::SOURCE, "late"]]}));
    send(&mut socket, serde_json::json!({"t": "ack", "gen": reverify.generation, "failed": []}));
    cur.set_sources(vec![(a, Source::new(Arc::from("return 3")))]);
    server.publish(cur.clone());
    let next = frame(&mut socket);
    assert_eq!(next.base, reverify.generation);
    assert_eq!(next.ops[0].1, [Field::U(a), Field::S("return 3".into())]);
}
