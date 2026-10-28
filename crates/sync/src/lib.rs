//! Studio sync (SYNC.md): one WebSocket per DataModel, one binary frame per generation, and the plugin itself, built
//! into an .rbxm by `plugin_rbxm`.
//!
//! The dev session publishes each generation's [`place::Tree`]; every socket's writer diffs what its DataModel holds
//! against the newest tree when its last frame is acked, so a backlog collapses into one frame and a stalled Studio
//! never holds up a publish. [`Server::exec`] runs Luau in a connected DataModel once it holds the newest tree.

mod frame;
mod install;
mod server;
mod sub;
mod ws;

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use place::Tree;
use serde_json::{Value, json};

pub use frame::{BIND, CHILDREN, HYDRATE, MOVE, PROPS, REF, RELOAD, REMOVE, SOURCE, STAMP, SWAP};
pub use install::{PLUGIN_SOURCE, install_plugin, plugin_rbxm};
pub use server::clean;
pub use sub::hex;

pub const DEFAULT_PORT: u16 = 34900;
/// Ports tried from `Config::port` upward when it is taken.
const PORT_TRIES: u16 = 16;
/// How long an exec may wait to start: in the writer until its DataModel holds the newest tree, then in the plugin's
/// queue behind earlier work. Past it, it never runs.
const START: Duration = Duration::from_secs(30);
/// How much longer than its start and time limit an exec's waiter gives the plugin to answer.
const ANSWER: Duration = Duration::from_secs(5);

pub struct Config {
    pub project: String,
    /// The place id the plugin connects to.
    pub place: u64,
    /// Lets unpublished places (`game.PlaceId == 0`) connect too.
    pub unpublished: bool,
    /// `[dev] hotReload`: the play server takes frames and swaps changed modules.
    pub hot_reload: bool,
    /// First port tried; 0 binds an ephemeral one.
    pub port: u16,
    /// Receives what each DataModel prints and does with its frames, and the server's own notices.
    pub reports: Sender<Report>,
    /// Receives each `POST /exec` (`opossum exec` and `test`); nobody listening answers 503.
    pub requests: Sender<Request>,
}

/// A `POST /exec`: its body as sent, and where its answer goes (HTTP status, JSON body).
pub struct Request {
    pub body: Vec<u8>,
    pub reply: Sender<(u16, String)>,
}

/// What a DataModel made of an exec.
pub struct Ran {
    /// The build it held: the newest for a DataModel that takes frames, else the newest of the last 16 whose every
    /// root it holds as stamped by edit before Play copied it (0 for none).
    pub generation: u32,
    /// False when that is not the newest build: a DataModel that takes no frames runs older code.
    pub fresh: bool,
    /// The plugin's `result` message.
    pub result: Value,
}

/// How an exec's waiter hears from the socket: the writer hands it over (or finds its DataModel too old for it),
/// then the reader brings the plugin's answer.
pub(crate) enum Reply {
    Started { generation: u32, fresh: bool },
    Stale { generation: u32, published: u32 },
    Done(Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    Output,
    Warning,
    Error,
}

impl LogKind {
    pub fn name(self) -> &'static str {
        match self {
            LogKind::Output => "output",
            LogKind::Warning => "warning",
            LogKind::Error => "error",
        }
    }
}

/// Who printed a line: a Studio DataModel (the edit one, or a playtest's server or client), or the sync itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Realm {
    Edit,
    Server,
    Client,
    Sync,
}

impl Realm {
    pub fn name(self) -> &'static str {
        match self {
            Realm::Edit => "edit",
            Realm::Server => "server",
            Realm::Client => "client",
            Realm::Sync => "sync",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub kind: LogKind,
    pub realm: Realm,
    pub text: String,
}

/// What the server tells the dev session, in order per socket. Text from Studio is already [`clean`]ed.
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    /// A DataModel's output, or the server's own notice (realm `Sync`).
    Log(LogLine),
    /// A plugin's hello: a DataModel of `place` syncs from now on.
    Connected {
        realm: Realm,
        place: u64,
    },
    /// Its socket closed.
    Disconnected {
        realm: Realm,
        place: u64,
    },
    /// The frame in flight was acked: the DataModel holds the published `generation` (a repair re-sends what failed
    /// and reports the generation it repairs again), `ms` from the frame's first part arriving in Studio to the ack.
    /// Each of its `failed` ops was reported as a [`Failure`] first, unless it was already parked.
    Applied {
        realm: Realm,
        generation: u32,
        ms: f64,
        failed: usize,
    },
    /// The plugin answered the frame in flight, which would have brought it to `generation`, with a verify instead of
    /// an ack: it dropped or could not apply the frame, or an undo (`quiet`) or Resync asked meanwhile.
    Verify {
        realm: Realm,
        generation: u32,
        quiet: bool,
    },
    Failed(Failure),
}

/// One op Studio could not apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub realm: Realm,
    /// The published generation the frame was bringing the DataModel to.
    pub generation: u32,
    pub id: u32,
    /// The op's name (`children`, `source`, ...).
    pub op: &'static str,
    pub message: String,
    /// The source file of the instance or of its nearest ancestor that has one, as the tree holds it.
    pub file: Option<PathBuf>,
    /// The instance path, when a tree still knows the id.
    pub path: Option<String>,
    /// It failed again unchanged, so it is parked until it changes or a Resync; later failures are not reported.
    pub parked: bool,
}

/// A running server; it serves for as long as the process lives.
#[derive(Clone)]
pub struct Server {
    shared: Arc<server::Shared>,
}

impl Server {
    /// Binds 127.0.0.1 (`config.port`, or the next free one of the 15 after it) and serves in the background.
    pub fn start(config: Config) -> io::Result<Server> {
        let listener = bind(config.port)?;
        let port = listener.local_addr()?.port();
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        let info = server::Info {
            project: config.project,
            place: config.place,
            unpublished: config.unpublished,
            hot_reload: config.hot_reload,
        };
        let session = format!("{:016x}", nanos ^ u64::from(std::process::id()));
        let shared = Arc::new(server::Shared::new(info, session, port, config.reports, config.requests));
        let serving = shared.clone();
        thread::Builder::new().name("sync".into()).spawn(move || server::serve(listener, serving))?;
        Ok(Server { shared })
    }

    pub fn port(&self) -> u16 {
        self.shared.port
    }

    /// The last generation published.
    pub fn generation(&self) -> u32 {
        self.shared.lock().generation
    }

    /// Makes `tree` the current generation and wakes the writers. Never touches a socket.
    pub fn publish(&self, tree: Tree) -> u32 {
        let mut state = self.shared.lock();
        state.generation += 1;
        if state.ring.len() == server::RING {
            state.ring.pop_front();
        }
        state.ring.push_back(tree.clone());
        state.tree = tree;
        let generation = state.generation;
        drop(state);
        self.shared.wake.notify_all();
        generation
    }

    /// The realms of the DataModels connected now (said hello), each once.
    pub fn realms(&self) -> Vec<Realm> {
        let mut realms: Vec<Realm> = self.shared.lock().inboxes.values().filter_map(|i| i.realm).collect();
        realms.sort_by_key(|r| *r as u8);
        realms.dedup();
        realms
    }

    /// Whether the play server takes frames (`hotReload`), so it holds every build as edit does.
    pub fn hot_reload(&self) -> bool {
        self.shared.lock().info.hot_reload
    }

    /// Sends `message` (the plugin's `exec`, its `id`, `timeout` and `start` filled in here) to the newest connected
    /// DataModel of `realm` as soon as that DataModel holds the newest published tree (SYNC.md, Exec), and waits for
    /// its result. With `fresh`, a DataModel holding older code (one that takes no frames) refuses it instead.
    ///
    /// An error means it never ran and never will: no such DataModel, it holds older code than allowed, or it could
    /// not start in time. When Studio had it but did not answer in time or disconnected, the result is the server's
    /// own, `{"ok": false, "lost": true, "error"}`: it may have run, or still be running.
    pub fn exec(&self, realm: Realm, mut message: Value, timeout: Duration, fresh: bool) -> Result<Ran, String> {
        let (tx, rx) = mpsc::channel();
        let start_by = Instant::now() + START;
        let name = realm.name();
        let id = {
            let mut state = self.shared.lock();
            let conn = state.inboxes.iter().filter(|(_, i)| i.realm == Some(realm) && !i.closed).map(|(&c, _)| c).max();
            let Some(conn) = conn else { return Err(format!("no {name} DataModel is connected")) };
            state.next += 1;
            let id = state.next;
            message["id"] = id.into();
            message["timeout"] = timeout.as_secs_f64().into();
            state.execs.insert(id, server::Waiter { conn, tx, fresh, start_by, sent: false });
            state.push(conn, server::Event::Exec(id, message));
            id
        };
        self.shared.wake.notify_all();
        let first = match rx.recv_timeout(START) {
            Ok(reply) => reply,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(format!("the {name} DataModel disconnected"));
            }
            Err(RecvTimeoutError::Timeout) => {
                // the writer hands an exec over under this lock: it did already, or it never will
                let mut state = self.shared.lock();
                match rx.try_recv() {
                    Ok(reply) => reply,
                    Err(_) => {
                        state.execs.remove(&id);
                        let wait = START.as_secs();
                        return Err(format!("the {name} DataModel did not take it within {wait} s"));
                    }
                }
            }
        };
        let (generation, fresh) = match first {
            Reply::Started { generation, fresh } => (generation, fresh),
            Reply::Stale { generation, published } => {
                return Err(format!(
                    "the {name} DataModel takes no frames and runs older code than the files (build {generation}, \
                     newest {published}): restart the playtest, or turn on hotReload for the server"
                ));
            }
            Reply::Done(_) => unreachable!("the reader answers only an exec the writer sent"),
        };
        let wait = start_by.saturating_duration_since(Instant::now()) + timeout + ANSWER;
        match rx.recv_timeout(wait) {
            // the plugin got to it only after its start ran out
            Ok(Reply::Done(result)) if result["late"] == true => {
                let error = result["error"].as_str().unwrap_or("it started too late");
                Err(format!("in the {name} DataModel, {error}"))
            }
            Ok(Reply::Done(result)) => Ok(Ran { generation, fresh, result }),
            Ok(_) => unreachable!("an exec is handed over once"),
            Err(e) => {
                self.shared.lock().execs.remove(&id);
                let error = match e {
                    RecvTimeoutError::Timeout => {
                        format!(
                            "the {name} DataModel did not answer within {} s; it may still be running",
                            wait.as_secs()
                        )
                    }
                    RecvTimeoutError::Disconnected => {
                        format!("the {name} DataModel disconnected; the code may have run")
                    }
                };
                Ok(Ran { generation, fresh, result: json!({"t": "result", "ok": false, "lost": true, "error": error}) })
            }
        }
    }

    /// A reloaded `opossum.toml`: what the next sockets are told. A plugin checks the place, and the play server
    /// learns hot reload, only at hello, so a change to either closes every socket and each plugin connects again.
    pub fn configure(&self, project: String, place: u64, unpublished: bool, hot_reload: bool) {
        let mut state = self.shared.lock();
        let old = &state.info;
        let again = old.place != place || old.unpublished != unpublished || old.hot_reload != hot_reload;
        state.info = server::Info { project, place, unpublished, hot_reload };
        if again {
            state.close();
        }
        drop(state);
        self.shared.wake.notify_all();
    }
}

fn bind(port: u16) -> io::Result<TcpListener> {
    if port == 0 {
        return TcpListener::bind(("127.0.0.1", 0));
    }
    let mut last = None;
    for port in port..port.saturating_add(PORT_TRIES) {
        // std binds with SO_REUSEADDR, which on macOS succeeds beside a wildcard listener (`*:port`) and would take
        // its loopback traffic: a port is free only when nothing answers there
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            last = Some(io::Error::from(io::ErrorKind::AddrInUse));
            continue;
        }
        match TcpListener::bind(addr) {
            Ok(listener) => return Ok(listener),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::from(io::ErrorKind::AddrInUse)))
}
