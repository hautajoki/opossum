//! One socket end to end, with tungstenite as the plugin: the handshake, the hello exchange, an adopting verify, an
//! edit, a repair after a failed ack, a late ack after a verify, an exec held back until the frame in flight is
//! acked, an orphan a verify lists, and a reconfigure, with what the server reports to the dev session along the way.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use place::{ScriptKind, Source, Tree};
use sync::{Realm, Report};
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
    quiet: bool,
    blob: Vec<String>,
    ops: Vec<(u8, Vec<Field>)>,
}

#[derive(Debug, PartialEq, Clone)]
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
    let (generation, base, flags) = (r.u32(), r.u32(), r.u8());
    let (last, quiet) = (flags & 1 != 0, flags & 2 != 0);
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
    Frame { generation, base, last, quiet, blob, ops }
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

fn report(reports: &Receiver<Report>) -> Report {
    reports.recv_timeout(Duration::from_secs(5)).expect("a report")
}

fn tags(frame: &Frame) -> Vec<u8> {
    frame.ops.iter().map(|(t, _)| *t).collect()
}

#[test]
fn socket_session() {
    let (tx, reports) = std::sync::mpsc::channel();
    let (requests, _requests) = std::sync::mpsc::channel();
    let server = sync::Server::start(sync::Config {
        project: "test".into(),
        place: 0,
        unpublished: true,
        hot_reload: false,
        port: 0,
        reports: tx,
        requests,
    })
    .unwrap();
    let port = server.port();

    // a page can POST to localhost too, and always says where from
    let mut post = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!(
        "POST /exec HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://evil.example\r\nContent-Length: 2\r\n\r\n{{}}"
    );
    post.write_all(head.as_bytes()).unwrap();
    let mut answer = String::new();
    post.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 403"));
    assert!(matches!(report(&reports), Report::Log(l) if l.realm == Realm::Sync && l.text.starts_with("refused")));
    let mut cur = tree("return 1");

    // a browser page names its origin (the notice is said once)
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
    // reconcile + authoritative under ReplicatedStorage: `shared`, then `a` with its Source; a verify lists every
    // property it copies, the ones the files leave unset included, so the blob copy's defaults reset someone's edit
    assert_eq!(children[..3], [Field::U(rs), Field::U(3), Field::U(shared)]);
    let at = 3 + children[3..].iter().position(|f| *f == Field::U(a)).unwrap();
    assert!(children[3..at].contains(&Field::S("Archivable".into())));
    assert!(
        children[at..].contains(&Field::S("Source".into())) && children[at..].contains(&Field::S("Archivable".into()))
    );
    assert_eq!(verify.blob, ["Folder"]);
    let stamp = &verify.ops[2].1;
    assert_eq!(stamp[1], Field::S(sync::hex(cur.get(shared).unwrap().hash)));
    send(&mut socket, serde_json::json!({"t": "ack", "gen": verify.generation, "failed": [], "ms": 1.5}));
    assert_eq!(report(&reports), Report::Connected { realm: Realm::Edit, place: 0 });
    let applied = Report::Applied { realm: Realm::Edit, generation: verify.generation, ms: 1.5, failed: 0 };
    assert_eq!(report(&reports), applied);

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
    // the failure is located by instance path (this tree's scripts have no file), before the ack itself
    match report(&reports) {
        Report::Failed(f) => assert_eq!(
            (f.generation, f.op, f.message.as_str(), f.file, f.path.as_deref(), f.parked),
            (edit.generation, "source", "boom", None, Some("ReplicatedStorage/shared/a"), false)
        ),
        other => panic!("expected the failure, got {other:?}"),
    }
    assert!(matches!(report(&reports), Report::Applied { failed: 1, generation, .. } if generation == edit.generation));
    assert_eq!(repair.base, edit.generation);
    assert_eq!(tags(&repair), [sync::SOURCE, sync::STAMP]);
    assert_eq!(repair.ops[0].1, [Field::U(a), Field::S("return 2".into())]);

    // an undo's verify while the repair is still applying: the wait ends and the root is verified, and only the
    // frame that answers it is quiet (the repair already in flight records as usual)
    send(&mut socket, serde_json::json!({"t": "verify", "quiet": true}));
    let reverify = frame(&mut socket);
    // reports name builds, never frame numbers: the repair is numbered past the last publish but brings that build
    assert!(repair.generation > generation);
    let answered = Report::Verify { realm: Realm::Edit, generation, quiet: true };
    assert_eq!(report(&reports), answered);
    assert_eq!(reverify.base, repair.generation);
    assert!(tags(&reverify).contains(&sync::CHILDREN));
    assert!(!repair.quiet && reverify.quiet);

    // the repair's ack arrives late: it is not the verify's, which covers it, so it repairs nothing
    send(&mut socket, serde_json::json!({"t": "ack", "gen": repair.generation, "failed": [[a, sync::SOURCE, "late"]]}));
    send(&mut socket, serde_json::json!({"t": "ack", "gen": reverify.generation, "failed": []}));
    // its failure is still reported against the build it was bringing, but only the verify's ack says applied
    assert!(matches!(report(&reports), Report::Failed(f) if f.message == "late" && f.generation == generation));
    assert!(matches!(report(&reports), Report::Applied { generation: g, failed: 0, .. } if g == generation));
    cur.set_sources(vec![(a, Source::new(Arc::from("return 3")))]);
    server.publish(cur.clone());
    let next = frame(&mut socket);
    assert_eq!(next.base, reverify.generation);
    assert!(!next.quiet);
    assert_eq!(next.ops[0].1, [Field::U(a), Field::S("return 3".into())]);

    // an exec asked while that frame is in flight runs only once Studio holds it: nothing comes before the ack
    let execs = server.clone();
    let message = serde_json::json!({"t": "exec", "name": "x", "source": "return 1"});
    let exec = std::thread::spawn(move || execs.exec(Realm::Edit, message, Duration::from_secs(5)));
    socket.get_mut().set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    assert!(socket.read().is_err(), "the exec went out before the ack");
    socket.get_mut().set_read_timeout(None).unwrap();
    send(&mut socket, serde_json::json!({"t": "ack", "gen": next.generation, "failed": []}));
    // generation 0, which no frame has, leads a JSON message
    let request: serde_json::Value = match socket.read().unwrap() {
        Message::Binary(bytes) if bytes[..4] == [0; 4] => serde_json::from_slice(&bytes[4..]).unwrap(),
        other => panic!("expected the exec, got {other:?}"),
    };
    assert_eq!(request["source"], "return 1");
    send(&mut socket, serde_json::json!({"t": "result", "id": request["id"], "ok": true, "values": [1]}));
    let ran = exec.join().unwrap().unwrap();
    assert_eq!(
        (ran.generation, ran.fresh, &ran.result["values"]),
        (server.generation(), true, &serde_json::json!([1]))
    );

    // a verify lists the stamped roots: one that is no longer a root is bound by its stamp alone and removed
    let roots = serde_json::json!({"ReplicatedStorage/shared": "", "ReplicatedStorage/old": "00"});
    send(&mut socket, serde_json::json!({"t": "verify", "roots": roots}));
    let orphan = frame(&mut socket);
    let bind = orphan.ops.iter().find(|(t, f)| *t == sync::BIND && f[2] == Field::S(String::new())).unwrap();
    assert_eq!(bind.1[1..], [Field::U(rs), Field::S(String::new()), Field::S("old".into())]);
    assert!(orphan.ops.contains(&(sync::REMOVE, vec![bind.1[0].clone()])));
    send(&mut socket, serde_json::json!({"t": "ack", "gen": orphan.generation, "failed": []}));

    // the plugin checks the place only at hello: another place closes the socket, and the plugin connects again
    server.configure("test".into(), 42, false, false);
    assert!(matches!(socket.read(), Err(_) | Ok(Message::Close(_))));
    let last = std::iter::from_fn(|| reports.recv_timeout(Duration::from_secs(5)).ok())
        .find(|r| matches!(r, Report::Disconnected { .. }));
    assert_eq!(last, Some(Report::Disconnected { realm: Realm::Edit, place: 0 }));
}
