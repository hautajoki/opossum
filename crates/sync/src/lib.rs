//! Studio sync (SYNC.md): one WebSocket per DataModel, one binary frame per generation, and the plugin itself, built
//! into an .rbxm by `plugin_rbxm`.
//!
//! The dev session publishes each generation's [`place::Tree`]; every socket's writer diffs what its DataModel holds
//! against the newest tree when its last frame is acked, so a backlog collapses into one frame and a stalled Studio
//! never holds up a publish.

mod frame;
mod install;
mod server;
mod sub;
mod ws;

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use place::Tree;

pub use frame::{BIND, CHILDREN, HYDRATE, MOVE, PROPS, REF, RELOAD, REMOVE, SOURCE, STAMP, SWAP};
pub use install::{PLUGIN_SOURCE, install_plugin, plugin_rbxm};
pub use sub::hex;

pub const DEFAULT_PORT: u16 = 34900;
/// Ports tried from `Config::port` upward when it is taken.
const PORT_TRIES: u16 = 16;

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
    /// Receives Studio output and the server's own notices.
    pub log: Option<Sender<LogLine>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    Output,
    Warning,
    Error,
}

/// Who printed a line: a Studio DataModel (the edit one, or a playtest's server or client), or the sync itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Realm {
    Edit,
    Server,
    Client,
    Sync,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub kind: LogKind,
    pub realm: Realm,
    pub text: String,
}

/// A running server; it serves for as long as the process lives.
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
        let shared = Arc::new(server::Shared::new(info, session, port, config.log));
        let serving = shared.clone();
        thread::Builder::new().name("sync".into()).spawn(move || server::serve(listener, serving))?;
        Ok(Server { shared })
    }

    pub fn port(&self) -> u16 {
        self.shared.port
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
