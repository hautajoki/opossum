//! `opossum dev`'s texture pack capture (DESIGN.md, Texture packs). Studio bakes a pack for every SurfaceAppearance
//! and MaterialVariant whose maps it sees, within ~20 s; a script cannot read one, but `SerializeInstancesAsync`
//! carries it. So a thread of its own, woken by the edit DataModel's hello and acks and every [`TICK`] otherwise,
//! finds the pack keys the place uses that opossum.lock has no `packs.` line for, has the edit plugin serialize one
//! instance of each, and saves each pack it finds there as a lock line. It never runs during a playtest and never
//! takes dev's lifecycle lock, so play, stop and pull are never refused because of it.
//!
//! The capture writes the lock beside the user's own commands, so it never waits on them and never leaves them a
//! `lock.recovered`: with the remote guard held elsewhere it keeps what it found for the next tick, and a lock changed
//! on disk under it is read again then ([`lock::save_or_drop`]). dev's own watcher sees nothing in a pack line.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use luau::luau_string;
use serde_json::json;
use sync::Realm;

use crate::event::{Event, Out};
use crate::guard::{Cache, RemoteGuard};
use crate::lock;

/// How often it looks while nothing wakes it.
const TICK: Duration = Duration::from_secs(10);
/// How long a key stays missing before its maps are set again on one instance (once per session), which makes
/// Studio retry a bake it missed.
const NUDGE_AFTER: Duration = Duration::from_secs(60);
/// The serialize exec's time limit.
const EXEC_WAIT: Duration = Duration::from_secs(30);

/// Walks the edit DataModel for the wanted keys (`WANTED`, spliced in), picks one instance of each, sets the maps of
/// the picks in `NUDGE` to none and back, and answers the picks serialized, as base64 ("" for none). The nudge runs in
/// a recording of its own, cancelled, so it never leaves an undo step: in a thread of its own that begins it once the
/// exec's recording ended, frame by frame for up to 300 frames (measured: the exec's own recording takes a nudge as an
/// undo step, and a recording begun while it is open, as it is until the serialize returns, is refused).
const SOURCE: &str = r#"local ChangeHistoryService = game:GetService("ChangeHistoryService")
local MAPS = { "ColorMapContent", "NormalMapContent", "RoughnessMapContent", "MetalnessMapContent" }
local picked, nudged = {}, {}
for _, inst in game:GetDescendants() do
	local class = inst.ClassName
	if class == "SurfaceAppearance" or class == "MaterialVariant" then
		local key = class
		for _, map in MAPS do
			key ..= "|" .. ((inst :: any)[map].Uri or "")
		end
		if WANTED[key] then
			WANTED[key] = nil
			table.insert(picked, inst)
			if NUDGE[key] then table.insert(nudged, inst) end
		end
	end
end
if #nudged > 0 then
	task.spawn(function()
		local recording = nil
		for _ = 1, 300 do
			task.wait()
			recording = ChangeHistoryService:TryBeginRecording("opossum: packs")
			if recording then break end
		end
		if not recording then return end
		for _, inst in nudged do
			for _, map in MAPS do
				local content = (inst :: any)[map]
				;(inst :: any)[map] = Content.none
				;(inst :: any)[map] = content
			end
		end
		ChangeHistoryService:FinishRecording(recording, Enum.FinishRecordingOperation.Cancel)
	end)
end
if #picked == 0 then return "" end
local data = game:GetService("SerializationService"):SerializeInstancesAsync(picked)
return buffer.tostring(game:GetService("EncodingService"):Base64Encode(data))"#;

/// The surfaces of dev's last tree, which it sets after every reshape.
pub type Surfaces = Arc<Mutex<Vec<place::Surface>>>;

/// One key the lock has no pack for: its class and the files using it.
type Missing = BTreeMap<String, (&'static str, BTreeSet<PathBuf>)>;

pub struct Capture {
    server: sync::Server,
    surfaces: Surfaces,
    out: Out,
    root: PathBuf,
    cache: Cache,
    /// When each missing key was first seen missing.
    since: HashMap<String, Instant>,
    /// Keys whose maps were set again, and of those the ones said still unbaked: each once per session.
    nudged: HashSet<String>,
    reported: HashSet<String>,
    /// Packs found and not saved yet: the guard was held, or the lock changed on disk.
    found: BTreeMap<String, u64>,
    /// The last error said, so a broken capture says it once, not every tick.
    error: Option<String>,
}

impl Capture {
    pub fn new(server: sync::Server, surfaces: Surfaces, out: Out, root: PathBuf, cache: Cache) -> Capture {
        let (since, nudged, reported) = Default::default();
        Capture { server, surfaces, out, root, cache, since, nudged, reported, found: BTreeMap::new(), error: None }
    }

    /// Captures on every wake-up and every [`TICK`], until dev drops the sender.
    pub fn run(mut self, wakes: &Receiver<()>) {
        loop {
            match wakes.recv_timeout(TICK) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while wakes.try_recv().is_ok() {}
            self.tick();
        }
    }

    fn tick(&mut self) {
        let realms = self.server.realms();
        if !realms.contains(&Realm::Edit) || realms.iter().any(|r| matches!(r, Realm::Server | Realm::Client)) {
            return;
        }
        let lock = match lock::read(&self.root) {
            Ok((lock, _)) => lock,
            Err(e) => return self.fail(e),
        };
        let mut missing = Missing::new();
        for s in self.surfaces.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if s.waiting || s.packed || lock.packs.contains_key(&s.key) || self.found.contains_key(&s.key) {
                continue;
            }
            missing.entry(s.key.clone()).or_insert((s.class, BTreeSet::new())).1.insert(s.file.clone());
        }
        let now = Instant::now();
        self.since.retain(|key, _| missing.contains_key(key));
        for key in missing.keys() {
            self.since.entry(key.clone()).or_insert(now);
        }
        let age = |key: &String| now.duration_since(self.since[key]);
        // nudged on an earlier tick and still missing
        let unbaked: Vec<&String> =
            missing.keys().filter(|k| self.nudged.contains(*k) && self.reported.insert((*k).clone())).collect();
        let after = unbaked.iter().map(|k| age(k).as_secs()).max().unwrap_or(0);
        let unbaked: Vec<(&'static str, Vec<PathBuf>)> =
            unbaked.iter().map(|k| (missing[*k].0, missing[*k].1.iter().cloned().collect())).collect();
        let nudge: Vec<&String> =
            missing.keys().filter(|k| age(k) >= NUDGE_AFTER && self.nudged.insert((*k).clone())).collect();
        if !missing.is_empty() {
            match self.serialize(&missing, &nudge) {
                Ok(packs) => {
                    self.found.extend(packs.into_iter().filter(|(key, _)| missing.contains_key(key)));
                    self.error = None;
                }
                Err(e) => self.fail(e),
            }
        }
        let baked = self.save();
        if baked + nudge.len() + unbaked.len() > 0 {
            self.out.emit(&Event::Packs { baked, nudged: nudge.len(), unbaked: &unbaked, after });
        }
    }

    /// The packs the edit DataModel holds for `missing`, setting the maps of `nudge` again.
    fn serialize(&self, missing: &Missing, nudge: &[&String]) -> Result<Vec<(String, u64)>, String> {
        let table = |keys: &mut dyn Iterator<Item = &String>| {
            let mut t = String::from("{");
            for key in keys {
                let _ = write!(t, " [{}] = true,", luau_string(key));
            }
            t + " }"
        };
        let source = format!(
            "local WANTED = {}\nlocal NUDGE = {}\n{SOURCE}",
            table(&mut missing.keys()),
            table(&mut nudge.iter().copied())
        );
        let message = json!({ "t": "exec", "name": "packs", "source": source, "limit": crate::dev::PULL_LIMIT,
                              "now": true });
        let ran = self.server.exec(Realm::Edit, message, EXEC_WAIT, false)?;
        let result = &ran.result;
        if result["ok"] != true {
            return Err(format!("Studio did not serialize its packs: {}", result["error"].as_str().unwrap_or("")));
        }
        let text = result["values"][0].as_str().unwrap_or("");
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let bytes = crate::dev::base64(text).ok_or("the plugin's packs answer is not base64")?;
        place::texture_packs(&bytes).map_err(|e| format!("opossum cannot read the packs Studio serialized: {e}"))
    }

    /// Saves what was found into the lock as it is now, when no other opossum holds the guard and no recovered state
    /// waits to be folded back; the number saved.
    fn save(&mut self) -> usize {
        if self.found.is_empty() || lock::recovered(&self.root).is_some() {
            return 0;
        }
        let guard = match RemoteGuard::try_acquire(&self.cache) {
            Ok(Some(guard)) => guard,
            Ok(None) => return 0,
            Err(e) => {
                self.fail(e);
                return 0;
            }
        };
        let mut lock = match lock::read(&self.root) {
            Ok((lock, _)) => lock,
            Err(e) => {
                self.fail(e);
                return 0;
            }
        };
        for (key, &id) in &self.found {
            let pack = lock.packs.entry(key.clone()).or_insert(id);
            *pack = (*pack).max(id);
        }
        match lock::save_or_drop(&mut lock, &self.root, &guard) {
            Ok(true) => std::mem::take(&mut self.found).len(),
            Ok(false) => 0,
            Err(d) => {
                self.fail(d.message);
                0
            }
        }
    }

    fn fail(&mut self, error: String) {
        if self.error.as_ref() != Some(&error) {
            self.out.emit(&Event::Error { message: &format!("packs: {error}"), fatal: false });
            self.error = Some(error);
        }
    }
}
