//! Sockets: an acceptor thread, then per socket a reader (the plugin's JSON) and a writer that builds its subscriber's
//! frame when the last one is acked. `publish` only swaps the tree under the lock and wakes the writers.

use std::collections::VecDeque;
use std::io::BufReader;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

use place::{Id, Tree};
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::{Value, json};

use crate::sub::{Context, Role, Sub};
use crate::{LogKind, LogLine, Realm, ws};

/// Generations kept for hydrating a reconnect whose stamps are a little behind.
pub const RING: usize = 16;

pub struct Info {
    pub project: String,
    pub place: u64,
    pub unpublished: bool,
    pub hot_reload: bool,
}

pub enum Event {
    Hello(Role, Vec<(String, String)>),
    /// The generation applied, and its failures.
    Ack(u32, Vec<(Id, u8, String)>),
    Lost(Vec<Id>),
    /// The plugin's stamped roots, as in its hello, and whether an undo or redo asked (quiet).
    Verify(Vec<(String, String)>, bool),
    Pong(Vec<u8>),
}

#[derive(Default)]
struct Inbox {
    events: Vec<Event>,
    closed: bool,
}

pub struct State {
    /// What a socket is told at its hello; [`State::close`] makes every plugin connect again after a change.
    pub info: Info,
    pub generation: u32,
    pub tree: Tree,
    /// The last generations' trees, oldest first; the current one is last.
    pub ring: VecDeque<Tree>,
    inboxes: FxHashMap<u64, Inbox>,
    next: u64,
}

pub struct Shared {
    pub session: String,
    pub port: u16,
    pub log: Option<Sender<LogLine>>,
    pub state: Mutex<State>,
    pub wake: Condvar,
    noticed: Mutex<FxHashSet<String>>,
}

impl State {
    /// Closes every socket: each plugin connects again and checks the place in the server's new hello.
    pub fn close(&mut self) {
        for inbox in self.inboxes.values_mut() {
            inbox.closed = true;
        }
    }
}

impl Shared {
    pub fn new(info: Info, session: String, port: u16, log: Option<Sender<LogLine>>) -> Shared {
        let state = State {
            info,
            generation: 0,
            tree: Tree::new(),
            ring: VecDeque::new(),
            inboxes: FxHashMap::default(),
            next: 0,
        };
        Shared { session, port, log, state: Mutex::new(state), wake: Condvar::new(), noticed: Mutex::default() }
    }

    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn say(&self, kind: LogKind, text: String) {
        match &self.log {
            Some(log) => {
                let _ = log.send(LogLine { kind, realm: Realm::Sync, text });
            }
            None => eprintln!("opossum: {text}"),
        }
    }

    /// Says `text` once for the life of the server.
    fn notice(&self, text: String) {
        if self.noticed.lock().unwrap_or_else(|e| e.into_inner()).insert(text.clone()) {
            self.say(LogKind::Warning, text);
        }
    }

    fn push(&self, conn: u64, event: Event) {
        if let Some(inbox) = self.lock().inboxes.get_mut(&conn) {
            inbox.events.push(event);
        }
        self.wake.notify_all();
    }
}

pub fn serve(listener: TcpListener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = shared.clone();
        let _ = thread::Builder::new().name("sync-socket".into()).spawn(move || socket(stream, &shared));
    }
}

fn socket(mut stream: TcpStream, shared: &Arc<Shared>) {
    let _ = stream.set_nodelay(true);
    let reader = match ws::accept(&mut stream, shared.port) {
        Ok(reader) => reader,
        Err(reason) => {
            if reason.starts_with("refused") {
                shared.notice(reason);
            }
            return;
        }
    };
    // the inbox exists from the hello on, so a reconfigure after it closes this socket too
    let (hello, conn) = {
        let mut state = shared.lock();
        let info = &state.info;
        let hello = json!({
            "t": "hello",
            "project": info.project,
            "place": info.place,
            "unpublished": info.unpublished,
            "session": shared.session,
            "hotReload": info.hot_reload,
        });
        state.next += 1;
        let conn = state.next;
        state.inboxes.insert(conn, Inbox::default());
        (hello, conn)
    };
    if ws::write(&mut stream, ws::TEXT, hello.to_string().as_bytes()).is_err() {
        shared.lock().inboxes.remove(&conn);
        return;
    }
    let writer = stream.try_clone().ok().and_then(|mut out| {
        let shared = shared.clone();
        let run = move || {
            // a panic says so and closes the socket, so the plugin reconnects instead of waiting on a dead writer
            if let Err(panic) = panic::catch_unwind(AssertUnwindSafe(|| write_loop(&mut out, &shared, conn))) {
                let text = panic.downcast_ref::<&str>().copied().or(panic.downcast_ref::<String>().map(String::as_str));
                shared.say(LogKind::Error, format!("the sync writer failed: {}", text.unwrap_or("a panic")));
            }
            // unblocks the reader
            let _ = out.shutdown(Shutdown::Both);
        };
        thread::Builder::new().name("sync-writer".into()).spawn(run).ok()
    });
    if writer.is_some() {
        read_loop(reader, shared, conn);
    }
    if let Some(inbox) = shared.lock().inboxes.get_mut(&conn) {
        inbox.closed = true;
    }
    shared.wake.notify_all();
    let _ = stream.shutdown(Shutdown::Both);
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    shared.lock().inboxes.remove(&conn);
}

fn read_loop(mut reader: BufReader<TcpStream>, shared: &Shared, conn: u64) {
    let mut realm = Realm::Edit;
    loop {
        let Ok((op, payload)) = ws::read(&mut reader) else { return };
        match op {
            ws::PING => shared.push(conn, Event::Pong(payload)),
            ws::CLOSE => return,
            ws::TEXT => {
                let Ok(message) = serde_json::from_slice::<Value>(&payload) else { continue };
                match message["t"].as_str() {
                    Some("hello") => {
                        let role = match message["role"].as_str() {
                            Some("server") => Role::Server,
                            Some("client") => Role::Client,
                            _ => Role::Edit,
                        };
                        realm = match role {
                            Role::Edit => Realm::Edit,
                            Role::Server => Realm::Server,
                            Role::Client => Realm::Client,
                        };
                        shared.push(conn, Event::Hello(role, roots(&message)));
                    }
                    Some("ack") => {
                        let failed = message["failed"].as_array().map_or_else(Vec::new, |list| {
                            list.iter()
                                .filter_map(|f| {
                                    let id = u32::try_from(f[0].as_u64()?).ok()?;
                                    let tag = u8::try_from(f[1].as_u64()?).ok()?;
                                    Some((id, tag, clean(f[2].as_str().unwrap_or(""))))
                                })
                                .collect()
                        });
                        // an ack without a generation matches no frame
                        let generation = message["gen"].as_u64().and_then(|g| u32::try_from(g).ok()).unwrap_or(0);
                        shared.push(conn, Event::Ack(generation, failed));
                    }
                    Some("lost") => {
                        let ids = message["ids"].as_array().map_or_else(Vec::new, |ids| {
                            ids.iter().filter_map(|i| u32::try_from(i.as_u64()?).ok()).collect()
                        });
                        shared.push(conn, Event::Lost(ids));
                    }
                    Some("verify") => {
                        shared.push(conn, Event::Verify(roots(&message), message["quiet"].as_bool().unwrap_or(false)));
                    }
                    Some("log") => {
                        let Some(log) = &shared.log else { continue };
                        for line in message["lines"].as_array().into_iter().flatten() {
                            let kind = match line[0].as_str() {
                                Some("warning") => LogKind::Warning,
                                Some("error") => LogKind::Error,
                                _ => LogKind::Output,
                            };
                            let text = clean(line[1].as_str().unwrap_or(""));
                            let _ = log.send(LogLine { kind, realm, text });
                        }
                    }
                    _ => {}
                }
            }
            // upstream is text only
            _ => return,
        }
    }
}

/// A hello's or verify's `roots`: stamp by path.
fn roots(message: &Value) -> Vec<(String, String)> {
    message["roots"].as_object().map_or_else(Vec::new, |roots| {
        roots.iter().map(|(path, stamp)| (path.clone(), stamp.as_str().unwrap_or("").to_owned())).collect()
    })
}

/// Log and failure text from Studio with control characters other than newlines and tabs removed, so it cannot
/// drive the terminal.
pub fn clean(text: &str) -> String {
    text.chars().filter(|&c| !c.is_control() || c == '\n' || c == '\t').collect()
}

/// Runs until the socket closes; the caller shuts the stream down after it returns.
fn write_loop(out: &mut TcpStream, shared: &Shared, conn: u64) {
    let mut sub = Sub::new();
    let mut seen = 0;
    loop {
        let (events, tree, ring, generation, hot_reload) = {
            let mut state = shared.lock();
            loop {
                // nothing is built before the first publish: an empty tree would remove every root
                let ready = state.generation > 0;
                let generation = state.generation;
                let Some(inbox) = state.inboxes.get_mut(&conn) else { return };
                if inbox.closed {
                    return;
                }
                if ready && (!inbox.events.is_empty() || (sub.sent.is_none() && generation > seen)) {
                    break;
                }
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            let events = std::mem::take(&mut state.inboxes.get_mut(&conn).expect("checked above").events);
            let ring: Vec<Tree> = state.ring.iter().cloned().collect();
            (events, state.tree.clone(), ring, state.generation, state.info.hot_reload)
        };
        let notice = |text: String| shared.notice(text);
        let say = |text: String| shared.say(LogKind::Error, text);
        let cx = Context { cur: &tree, ring: &ring, hot_reload, notice: &notice, say: &say };
        for event in events {
            match event {
                Event::Hello(role, roots) => sub.hello(role, roots, hot_reload),
                Event::Ack(generation, failed) => {
                    sub.ack(&cx, generation, &failed);
                }
                Event::Lost(ids) => sub.lost(&tree, &ids),
                Event::Verify(roots, quiet) => sub.resync(&tree, &roots, quiet),
                Event::Pong(payload) => {
                    if ws::write(out, ws::PONG, &payload).is_err() {
                        return;
                    }
                }
            }
        }
        if sub.sent.is_some() {
            continue;
        }
        seen = generation;
        let Some(parts) = sub.frame(&cx, generation.max(sub.base + 1)) else { continue };
        for part in parts {
            if ws::write(out, ws::BINARY, &part).is_err() {
                return;
            }
        }
    }
}
