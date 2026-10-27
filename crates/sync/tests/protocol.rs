use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use sync::{Config, LogKind, LogLine, Node, Prop, Realm, Server, Snapshot};

fn start(log: Option<mpsc::Sender<LogLine>>) -> Server {
    let config = Config {
        project: "test".into(),
        place: 42,
        unpublished: false,
        roots: vec!["ServerScriptService/server".into()],
        port: 0,
        log,
    };
    Server::start(config).expect("bind")
}

/// Sends one request with `host`, returns (status, body).
fn send(port: u16, host: &str, method: &str, target: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let request = format!(
        "{method} {target} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write");
    let mut out = String::new();
    stream.read_to_string(&mut out).expect("read");
    let status = out[9..12].parse().expect("status");
    let body = out
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

fn get(port: u16, target: &str) -> Value {
    let (status, body) = send(port, &format!("127.0.0.1:{port}"), "GET", target, "");
    assert_eq!(status, 200, "{target}: {body}");
    serde_json::from_str(&body).expect("json")
}

fn node(class: &str, source: Option<&str>, file: Option<&str>) -> Node {
    Node {
        class: class.into(),
        source: source.map(Arc::from),
        props: Vec::new(),
        file: file.map(String::from),
    }
}

fn first() -> Snapshot {
    let mut s = Snapshot::new();
    s.insert(
        "ServerScriptService/server".into(),
        node("Folder", None, None),
    );
    s.insert(
        "ServerScriptService/server/doors".into(),
        node("Folder", None, Some("features/doors")),
    );
    s.insert(
        "ServerScriptService/server/doors/door".into(),
        node(
            "ModuleScript",
            Some("return 1"),
            Some("features/doors/server/door.ts"),
        ),
    );
    s.insert(
        "ServerScriptService/server/old".into(),
        node("Folder", None, None),
    );
    s.insert(
        "ServerScriptService/server/old/gone".into(),
        node("ModuleScript", Some("return 0"), None),
    );
    s
}

fn second() -> Snapshot {
    let mut s = first();
    s.remove("ServerScriptService/server/old");
    s.remove("ServerScriptService/server/old/gone");
    s.get_mut("ServerScriptService/server/doors/door")
        .expect("door")
        .source = Some("return 2".into());
    let mut main = node("Script", Some("print('hi')"), None);
    main.props.push((
        "RunContext".into(),
        Prop::Enum("RunContext".into(), "Server".into()),
    ));
    s.insert("ServerScriptService/server/main".into(), main);
    s
}

fn ops(patch: &Value) -> Vec<(String, String)> {
    let list = patch["ops"].as_array().expect("ops");
    list.iter()
        .map(|op| {
            (
                op["op"].as_str().unwrap_or("").into(),
                op["path"].as_str().unwrap_or("").into(),
            )
        })
        .collect()
}

#[test]
fn patches_compose_and_scope() {
    let server = start(None);
    let port = server.port();
    assert_eq!(server.publish(first()), 1);
    assert_eq!(
        server.publish(first()),
        1,
        "an unchanged snapshot is not a generation"
    );
    assert_eq!(server.publish(second()), 2);

    let hello = get(port, "/hello");
    assert_eq!(hello["place"], 42);
    assert_eq!(hello["generation"], 2);
    let session = hello["session"].as_str().expect("session").to_string();

    let full = get(port, &format!("/patch?since=0&session={session}"));
    assert_eq!(full["full"], true);
    assert_eq!(full["ops"].as_array().map(Vec::len), Some(4));

    // Sets parent-first, dels child-first, so the plugin can apply them in order.
    let diff = get(port, &format!("/patch?since=1&session={session}"));
    assert_eq!(diff["full"], false);
    let expect = [
        ("set", "ServerScriptService/server/doors/door"),
        ("set", "ServerScriptService/server/main"),
        ("del", "ServerScriptService/server/old/gone"),
        ("del", "ServerScriptService/server/old"),
    ];
    assert_eq!(
        ops(&diff),
        expect.map(|(o, p)| (o.to_string(), p.to_string()))
    );
    assert_eq!(diff["ops"][1]["props"]["RunContext"]["item"], "Server");

    // Another session's generation 1 is not ours: full snapshot.
    assert_eq!(get(port, "/patch?since=1&session=elsewhere")["full"], true);

    // `only` matches the file path too, and a scoped full snapshot brings the ancestors along.
    let scoped = get(
        port,
        &format!("/patch?since=0&session={session}&only=features%2Fdoors"),
    );
    let paths: Vec<String> = ops(&scoped).into_iter().map(|(_, p)| p).collect();
    assert_eq!(
        paths,
        [
            "ServerScriptService/server",
            "ServerScriptService/server/doors",
            "ServerScriptService/server/doors/door"
        ]
    );

    let hash = diff["ops"][0]["hash"].as_str().expect("hash");
    let (status, source) = send(
        port,
        &format!("localhost:{port}"),
        "GET",
        &format!("/blob/{hash}"),
        "",
    );
    assert_eq!((status, source.as_str()), (200, "return 2"));
    let (status, batch) = send(
        port,
        &format!("127.0.0.1:{port}"),
        "POST",
        "/blobs",
        &format!("[\"{hash}\"]"),
    );
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&batch).expect("json")[hash],
        "return 2"
    );
}

#[test]
fn long_poll_wakes_on_publish() {
    let server = start(None);
    let port = server.port();
    server.publish(first());
    let session = get(port, "/hello")["session"]
        .as_str()
        .expect("session")
        .to_string();

    let poll = thread::spawn(move || {
        let start = Instant::now();
        (
            get(port, &format!("/patch?since=1&session={session}")),
            start.elapsed(),
        )
    });
    thread::sleep(Duration::from_millis(200));
    server.publish(second());
    let (patch, waited) = poll.join().expect("poll");
    assert_eq!(patch["generation"], 2);
    assert_eq!(patch["full"], false);
    assert!(
        waited >= Duration::from_millis(150) && waited < Duration::from_secs(5),
        "{waited:?}"
    );
}

#[test]
fn rejects_foreign_hosts_and_origins() {
    let server = start(None);
    let port = server.port();
    assert_eq!(
        send(port, &format!("evil.com:{port}"), "GET", "/hello", "").0,
        403
    );
    assert_eq!(send(port, "127.0.0.1:1", "GET", "/hello", "").0, 403);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let request =
        format!("GET /hello HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: http://evil.com\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write");
    let mut out = String::new();
    stream.read_to_string(&mut out).expect("read");
    assert!(out.starts_with("HTTP/1.1 403"), "{out}");
}

#[test]
fn log_lines_reach_the_channel() {
    let (tx, rx) = mpsc::channel();
    let server = start(Some(tx));
    let port = server.port();
    let body = r#"[{"kind":"MessageError","realm":"server","text":"boom"}]"#;
    assert_eq!(
        send(port, &format!("127.0.0.1:{port}"), "POST", "/log", body).0,
        200
    );
    let line = rx.recv_timeout(Duration::from_secs(5)).expect("log line");
    assert_eq!(
        line,
        LogLine {
            kind: LogKind::Error,
            realm: Realm::Server,
            text: "boom".into()
        }
    );
}

#[test]
fn plugin_rbxm_round_trips() {
    let bytes = sync::plugin_rbxm().expect("rbxm");
    let dom = rbx_binary::from_reader(bytes.as_slice()).expect("parse");
    let script = dom.get_by_ref(dom.root().children()[0]).expect("script");
    assert_eq!(
        (script.name.as_str(), script.class.as_str()),
        ("opossum", "Script")
    );
    let source = script.properties.get(&rbx_dom_weak::ustr("Source"));
    assert_eq!(
        source,
        Some(&rbx_dom_weak::types::Variant::String(
            sync::PLUGIN_SOURCE.into()
        ))
    );
}
