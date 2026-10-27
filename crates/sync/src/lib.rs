//! Studio sync: an HTTP long-poll server (std::net, a thread per connection) that streams instance-tree patches to
//! the opossum Studio plugin, and the plugin itself, built into an .rbxm by `plugin_rbxm`.
//!
//! The binary publishes whole snapshots; the server diffs each against the previous generation and keeps a bounded
//! log of patches, so a plugin that is a few generations behind gets the composed ops and one that is too far behind
//! (or new) gets the full snapshot. Sources travel separately, by content hash.

mod install;
mod server;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::net::TcpListener;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

pub use install::{PLUGIN_SOURCE, install_plugin, plugin_rbxm};

pub const DEFAULT_PORT: u16 = 34872;
/// Ports tried from `Config::port` upward when it is taken.
const PORT_TRIES: u16 = 16;
/// Patches kept; a plugin further behind than this gets a full snapshot.
const LOG_LEN: usize = 64;

/// One instance below a service. `class` is any Roblox class name; scripts carry their `source`.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub class: String,
    pub source: Option<Arc<str>>,
    pub props: Vec<(String, Prop)>,
    /// Project-relative source path (`features/doors/server/door.ts`, or a directory for folders); `only=` filters
    /// match it as well as the instance path.
    pub file: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Bool(bool),
    Number(f64),
    String(String),
    /// `Enum.<type>.<item>`, e.g. `Enum("RunContext", "Client")`.
    Enum(String, String),
}

/// Instance path (`ServerScriptService/server/features/doors`, `/`-separated, starting at a service) → node. Every
/// instance below the service level belongs in it, folders included.
pub type Snapshot = BTreeMap<String, Node>;

pub struct Config {
    pub project: String,
    /// The place id the plugin auto-connects to.
    pub place: u64,
    /// Lets unpublished places (`game.PlaceId == 0`) connect too.
    pub unpublished: bool,
    /// Instance paths opossum owns outright: on a full snapshot the plugin removes anything under them that the
    /// snapshot lacks.
    pub roots: Vec<String>,
    /// First port tried; 0 binds an ephemeral one.
    pub port: u16,
    /// Receives Studio output posted by the plugin.
    pub log: Option<Sender<LogLine>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    Output,
    Info,
    Warning,
    Error,
}

/// Which Studio DataModel printed a line: the edit one, or a playtest's server or client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Realm {
    Edit,
    Server,
    Client,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub kind: LogKind,
    pub realm: Realm,
    pub text: String,
}

/// A running server. Clones share it; it serves for as long as the process lives.
#[derive(Clone)]
pub struct Server {
    shared: Arc<Shared>,
    port: u16,
}

impl Server {
    /// Binds 127.0.0.1 (`config.port`, or the next free one of the 15 after it) and serves in the background.
    pub fn start(config: Config) -> io::Result<Server> {
        let listener = bind(config.port)?;
        let port = listener.local_addr()?.port();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let shared = Arc::new(Shared {
            config,
            session: format!("{:016x}", nanos ^ u64::from(std::process::id())),
            port,
            state: Mutex::new(State {
                generation: 0,
                tree: Tree::new(),
                log: VecDeque::new(),
                blobs: HashMap::new(),
            }),
            changed: Condvar::new(),
        });
        let serving = shared.clone();
        thread::Builder::new()
            .name("sync".into())
            .spawn(move || server::serve(listener, serving))?;
        Ok(Server { shared, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Makes `snapshot` the current generation and wakes waiting plugins. Returns the generation, which stays the
    /// same when nothing changed.
    pub fn publish(&self, snapshot: Snapshot) -> u64 {
        let mut tree = Tree::new();
        let mut blobs = HashMap::new();
        for (path, node) in snapshot {
            let hash = node.source.map(|source| {
                let hash = xxhash_rust::xxh3::xxh3_128(source.as_bytes());
                blobs.insert(hash, source);
                hash
            });
            let entry = Entry {
                class: node.class,
                hash,
                props: node.props,
                file: node.file,
            };
            tree.insert(path.into(), Arc::new(entry));
        }

        let mut state = self.shared.lock();
        let ops = diff(&state.tree, &tree);
        if ops.is_empty() {
            return state.generation;
        }
        // The previous generation's sources stay one more publish, for plugins still fetching them.
        let held = &mut *state;
        for entry in held.tree.values() {
            if let Some(hash) = entry.hash
                && !blobs.contains_key(&hash)
                && let Some(source) = held.blobs.remove(&hash)
            {
                blobs.insert(hash, source);
            }
        }
        state.generation += 1;
        let generation = state.generation;
        if state.log.len() == LOG_LEN {
            state.log.pop_front();
        }
        state.log.push_back(Patch { generation, ops });
        state.tree = tree;
        state.blobs = blobs;
        drop(state);
        self.shared.changed.notify_all();
        generation
    }
}

fn bind(port: u16) -> io::Result<TcpListener> {
    if port == 0 {
        return TcpListener::bind(("127.0.0.1", 0));
    }
    let mut last = None;
    for port in port..port.saturating_add(PORT_TRIES) {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => return Ok(listener),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::from(io::ErrorKind::AddrInUse)))
}

type Tree = BTreeMap<Arc<str>, Arc<Entry>>;

#[derive(Debug, PartialEq)]
struct Entry {
    class: String,
    hash: Option<u128>,
    props: Vec<(String, Prop)>,
    file: Option<String>,
}

/// A del keeps the removed entry so `only=` can still match its file.
#[derive(Clone)]
enum Op {
    Set(Arc<Entry>),
    Del(Arc<Entry>),
}

/// The ops that turn generation - 1 into `generation`, keyed by path.
struct Patch {
    generation: u64,
    ops: BTreeMap<Arc<str>, Op>,
}

struct State {
    generation: u64,
    tree: Tree,
    log: VecDeque<Patch>,
    blobs: HashMap<u128, Arc<str>>,
}

struct Shared {
    config: Config,
    /// Changes every start, so a plugin can tell a restarted server's generations from the ones it applied.
    session: String,
    port: u16,
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn diff(old: &Tree, new: &Tree) -> BTreeMap<Arc<str>, Op> {
    let mut ops = BTreeMap::new();
    for (path, entry) in new {
        if old.get(path).is_none_or(|o| o != entry) {
            ops.insert(path.clone(), Op::Set(entry.clone()));
        }
    }
    for (path, entry) in old {
        if !new.contains_key(path) {
            ops.insert(path.clone(), Op::Del(entry.clone()));
        }
    }
    ops
}
