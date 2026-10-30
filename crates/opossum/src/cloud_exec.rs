//! Exact-version execution on Open Cloud. `exec --cloud` and `test --cloud` build the place
//! here, save it once as a place version and run one Luau task on exactly that version, so what runs is the build that
//! was checked and is mapped. A cache every worktree shares remembers which version holds which build's bytes, so an
//! unchanged build saves nothing. `exec --head` runs on the current head instead, with no build and no save.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cloud::{Client, ErrorKind, Task};

use crate::build::Session;
use crate::config::Config;
use crate::diag::Diag;
use crate::editor;
use crate::guard::Cache;

/// The builds a place's cache remembers, most recent first: two branches or worktrees taking turns on one place reuse
/// their versions instead of saving one per switch.
const REMEMBERED: usize = 16;
/// How long a task's logs may take once it ended: a task that ran to its timeout still gets its error logs.
const LOGS: Duration = Duration::from_secs(30);

/// A place of an environment: where a cloud run runs.
pub struct Placed {
    pub environment: String,
    pub place: String,
    pub universe: u64,
    pub id: u64,
    /// The place's `profile`, the one `exec --cloud` builds it with.
    pub profile: Option<String>,
}

impl Placed {
    /// `development/main`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.environment, self.place)
    }
}

/// Why a cloud run stopped short: a stable code, the error, and whether the script may have run (exit 1) or certainly
/// never did (exit 2).
pub struct Failure {
    pub code: &'static str,
    pub error: String,
    pub ran: bool,
}

/// A task that started: the version it ran on (named by its path, also at the head), whether this run saved that
/// version, the task as it ended (or as it stood at the deadline), what it logged, and its milliseconds from start.
pub struct Ran {
    pub version: u64,
    pub saved: bool,
    pub task: Task,
    pub logs: Vec<String>,
    pub ms: f64,
}

/// The environment `--environment` names (else the default) and its place `--place` names (else the root place).
pub fn resolve(cfg: &Config, environment: Option<&str>, place: Option<&str>) -> Result<Placed, String> {
    let env = cfg.environment(environment)?;
    let root = cfg.places.iter().find(|p| p.root).map_or("main", |p| p.name.as_str());
    let name = place.unwrap_or(root);
    let Some(&(_, id)) = env.places.iter().find(|(p, _)| p == name) else {
        let names: Vec<&str> = env.places.iter().map(|(p, _)| p.as_str()).collect();
        return Err(format!("environment `{}` has no place `{name}`; it has: {}", env.name, names.join(", ")));
    };
    let profile = cfg.places.iter().find(|p| p.name == name).and_then(|p| p.profile.clone());
    Ok(Placed { environment: env.name.clone(), place: name.to_owned(), universe: env.universe, id, profile })
}

/// Builds the project as `build` does, with `profile` and `environment`'s ids, into rbxl bytes with texture packs. A
/// build with errors prints them and is refused, so it never reaches Roblox.
pub fn build(cfg: Config, profile: Option<&str>, environment: &str) -> Result<(Session, Vec<u8>), Failure> {
    let refused = |code, error| Failure { code, error, ran: false };
    let mut session = Session::new(cfg, profile, Some(environment)).map_err(|e| refused("config", e))?;
    editor::write(&session.cfg.root, &session.cfg.ts, &session.modules).map_err(|e| refused("build", e))?;
    session.build().map_err(|e| refused("build", e))?;
    let (rbxl, _) = rbxl(&mut session)?;
    Ok((session, rbxl))
}

/// A built session's place as rbxl bytes with texture packs, and what the packs covered. Refused when the build has
/// errors (printed on stderr) or the place is over Roblox's limit.
pub fn rbxl(session: &mut Session) -> Result<(Vec<u8>, place::PackStats), Failure> {
    let refused = |code, error| Failure { code, error, ran: false };
    let tree = session.tree(false);
    let root = &session.cfg.root;
    let errors: Vec<Diag> = session.diagnostics().into_iter().filter(Diag::is_error).collect();
    if !errors.is_empty() {
        for d in &errors {
            eprintln!("{}", d.human(root));
        }
        return Err(refused("build", "the project does not compile, so nothing was sent to Open Cloud".into()));
    }
    let mut rbxl = Vec::new();
    let stats = place::write_rbxl(&mut tree.to_place(), &session.packs, &mut rbxl).map_err(|e| refused("build", e))?;
    if rbxl.len() > cloud::MAX_PLACE {
        let error = format!("the place is {} MiB, over Roblox's 100 MiB", rbxl.len() >> 20);
        return Err(refused("place.tooLarge", error));
    }
    Ok((rbxl, stats))
}

/// Runs `script` on `at` for at most `timeout`: on the saved version holding `rbxl` (the one the cache names, or one
/// saved now, `note` added to saying so), or on the current head for None. Progress goes to stderr.
pub fn run(
    client: &Client,
    cache: &Cache,
    at: &Placed,
    rbxl: Option<&[u8]>,
    note: &str,
    script: &str,
    timeout: Duration,
) -> Result<Ran, Failure> {
    let file = versions(cache, client, at);
    let hash = rbxl.map(|bytes| blake3::hash(bytes).to_hex().to_string());
    let (mut version, mut saved) = (None, false);
    if let (Some(bytes), Some(hash)) = (rbxl, &hash) {
        let (v, s) = saved_version(client, &file, at, hash, bytes, note)?;
        (version, saved) = (Some(v), s);
    }
    let start = Instant::now();
    let task = match client.start_task(at.universe, at.id, version, script, timeout) {
        Ok(task) => task,
        // the cached version is gone from Roblox: forget it, save the build again and start there
        Err(e) if e.kind == ErrorKind::Status(404) && version.is_some() && !saved => {
            let (Some(bytes), Some(hash)) = (rbxl, &hash) else { unreachable!("a version comes from a build") };
            warn(edit(&file, |lines| lines.retain(|(h, _)| h != hash)));
            let (v, _) = saved_version(client, &file, at, hash, bytes, note)?;
            (version, saved) = (Some(v), true);
            client.start_task(at.universe, at.id, version, script, timeout).map_err(|e| unstarted(&e))?
        }
        Err(e) => return Err(unstarted(&e)),
    };
    let deadline = client.now() + timeout + Duration::from_secs(30);
    let task = client
        .wait_task(&task.path, deadline, &mut |state| eprintln!("execution {state}"))
        .map_err(|e| Failure { code: e.diag(), error: format!("{e}; the task may still be running"), ran: true })?;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let logs = client.task_logs(&task.path, client.now() + LOGS).unwrap_or_else(|e| {
        eprintln!("opossum: warning: the task's logs could not be read: {e}");
        Vec::new()
    });
    Ok(Ran { version: task.version, saved, task, logs, ms })
}

/// A task that did not start: certainly not when Roblox refused it, else it may have.
fn unstarted(e: &cloud::Error) -> Failure {
    if e.definite() {
        return Failure { code: e.diag(), error: e.to_string(), ran: false };
    }
    Failure { code: "cloud.http", error: format!("{e}; the task may have started"), ran: true }
}

/// The version holding the build hashed `hash`: the one the cache names, else `rbxl` saved now. Either way it becomes
/// the cache's most recent line. Returns it and whether it was saved now.
fn saved_version(
    client: &Client,
    file: &Path,
    at: &Placed,
    hash: &str,
    rbxl: &[u8],
    note: &str,
) -> Result<(u64, bool), Failure> {
    let cached = read(file).into_iter().find(|(h, _)| h == hash).map(|(_, v)| v);
    let (version, saved) = match cached {
        Some(version) => {
            eprintln!("reusing {} v{version}", at.label());
            (version, false)
        }
        None => {
            eprintln!("saving {} as a place version on Roblox…{note}", at.label());
            let saved = client.save_place(at.universe, at.id, rbxl, false);
            (saved.map_err(|e| Failure { code: e.diag(), error: e.to_string(), ran: false })?, true)
        }
    };
    remember(file, hash, version);
    Ok((version, saved))
}

/// The cache file of `at`'s saved versions on the Open Cloud `client` talks to, shared by every worktree on the
/// machine: a mock's versions never stand for Roblox's.
pub fn versions(cache: &Cache, client: &Client, at: &Placed) -> PathBuf {
    let host = client.base().split_once("://").map_or(client.base(), |(_, host)| host).replace(':', "-");
    cache.dir.join("versions").join(host).join(format!("{}-{}", at.universe, at.id))
}

/// Makes `version` the cache's most recent line for the build hashed `hash`: a save (`deploy`'s too) that a cloud run
/// of the same bytes then reuses.
pub fn remember(file: &Path, hash: &str, version: u64) {
    warn(edit(file, |lines| {
        lines.retain(|(h, _)| h != hash);
        lines.insert(0, (hash.to_owned(), version));
    }));
}

/// A place's cache file: `<hash> <version>` lines, most recent first. Lines it cannot read are skipped.
fn read(file: &Path) -> Vec<(String, u64)> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let line = |l: &str| l.split_once(' ').and_then(|(h, v)| Some((h.to_owned(), v.parse().ok()?)));
    text.lines().filter_map(line).collect()
}

/// Rewrites a place's cache file through `change`, keeping the most recent [`REMEMBERED`] lines; written aside and
/// renamed over, so a reader in another worktree never sees half of it.
fn edit(file: &Path, change: impl FnOnce(&mut Vec<(String, u64)>)) -> Result<(), String> {
    let mut lines = read(file);
    change(&mut lines);
    lines.truncate(REMEMBERED);
    let text: String = lines.iter().map(|(h, v)| format!("{h} {v}\n")).collect();
    let io = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
    let dir = file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let mut temp = PathBuf::from(file);
    temp.set_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(|e| io(&temp, e))?;
    std::fs::rename(&temp, file).map_err(|e| io(file, e))
}

/// The cache only saves requests: failing to write it is a warning.
fn warn(result: Result<(), String>) {
    if let Err(e) = result {
        eprintln!("opossum: warning: the saved-version cache: {e}");
    }
}

#[cfg(test)]
mod tests {
    use cloud::mock::{Mock, Reply, auto};
    use cloud::{Clock, Fake};
    use serde_json::json;

    use super::*;

    fn at() -> Placed {
        Placed { environment: "dev".into(), place: "main".into(), universe: 1, id: 2, profile: None }
    }

    fn cache(name: &str) -> Cache {
        let dir = std::env::temp_dir().join(format!("opossum-cloud-exec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Cache { dir }
    }

    /// What the mock was sent, as `METHOD path`.
    fn sent(mock: &Mock) -> Vec<String> {
        mock.requests().iter().map(|r| format!("{} {}", r.method, r.path)).collect()
    }

    const TASKS: &str = "luau-execution-session-tasks";

    #[test]
    fn a_build_is_saved_once_alternating_builds_reuse_theirs_and_a_lost_version_is_saved_again() {
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let client = Client::new("test-key", &mock.base, Clock::Fake(Fake::new())).unwrap();
        let cache = cache("reuse");
        let go = |rbxl: &[u8]| {
            let ran = run(&client, &cache, &at(), Some(rbxl), "", "return 1", Duration::from_secs(5)).ok().unwrap();
            (ran.version, ran.saved, ran.task.state)
        };
        assert_eq!(go(b"a"), (1, true, "COMPLETE".into()));
        assert_eq!(go(b"b"), (2, true, "COMPLETE".into()));
        // two branches taking turns save nothing more
        assert_eq!((go(b"a").0, go(b"b").0, go(b"a").1), (1, 2, false));
        let saves = |mock: &Mock| sent(mock).iter().filter(|r| r.ends_with("/versions")).count();
        assert_eq!(saves(&mock), 2);

        // Roblox no longer has v1: one 404, one save, the task at the new version
        let before = mock.requests().len();
        mock.on("POST", "cloud/v2/universes/1/places/2/versions/1/", Reply::json(404, &json!({})));
        assert_eq!(go(b"a"), (3, true, "COMPLETE".into()));
        assert_eq!(
            sent(&mock)[before..before + 3],
            [
                format!("POST cloud/v2/universes/1/places/2/versions/1/{TASKS}"),
                "POST universes/v1/1/places/2/versions".into(),
                format!("POST cloud/v2/universes/1/places/2/versions/3/{TASKS}"),
            ]
        );
        let hash = |b: &[u8]| blake3::hash(b).to_hex().to_string();
        let file = versions(&cache, &client, &at());
        assert!(file.ends_with(format!("127.0.0.1-{}/1-2", mock.base.rsplit(':').next().unwrap())));
        assert_eq!(read(&file), [(hash(b"a"), 3), (hash(b"b"), 2)]);
        let _ = std::fs::remove_dir_all(&cache.dir);
    }

    #[test]
    fn the_head_saves_nothing_names_the_version_that_ran_and_an_ambiguous_start_may_have_run() {
        let mock = Mock::bind(0, Some(auto())).unwrap();
        let client = Client::new("test-key", &mock.base, Clock::Fake(Fake::new())).unwrap();
        let cache = cache("head");
        client.save_place(1, 2, b"published", true).unwrap();
        let ran = run(&client, &cache, &at(), None, "", "return 1", Duration::from_secs(5)).ok().unwrap();
        assert_eq!((ran.version, ran.saved), (1, false));
        assert_eq!(sent(&mock)[1], format!("POST cloud/v2/universes/1/places/2/{TASKS}"));
        assert!(!cache.dir.exists());

        // a 503 on the create is never retried: the task may have started
        mock.on("POST", "cloud/v2/", Reply::json(503, &json!({})));
        let failure = run(&client, &cache, &at(), None, "", "return 1", Duration::from_secs(5)).err().unwrap();
        assert_eq!((failure.code, failure.ran), ("cloud.http", true));
        // a refused one never ran
        mock.on("POST", "cloud/v2/", Reply::json(403, &json!({})));
        let failure = run(&client, &cache, &at(), None, "", "return 1", Duration::from_secs(5)).err().unwrap();
        assert_eq!((failure.code, failure.ran), ("cloud.auth", false));
        assert_eq!(sent(&mock).iter().filter(|r| r.ends_with(TASKS)).count(), 3);
    }
}
