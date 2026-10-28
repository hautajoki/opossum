# Sync

`opossum dev` keeps Studio equal to the files: every instance opossum owns, every property, attribute, tag, ref,
mesh and union, every script source, while never touching what the user built in Studio. One WebSocket per
DataModel, one binary frame per generation, a plugin with no per-type value codec. Numbers quoted here were measured
in Studio 0.745 (2026-10-28).

## Model

**Tree.** The dev session owns one long-lived `Tree`: a slab of immutable nodes indexed by `Id` (`u32`, never reused
within a server session), `Arc<Node { parent, name, class, props, refs, source, children, hash }>`. It changes by
deltas, never by a rebuild: a re-rendered module sets one node's source; a `place/` file event re-reads that entry with
its same-named siblings (an event on a service directory re-reads the whole place), re-applies `[place]`, resolves
again every ref that was unresolved or pointed into what was re-read, and pairs ids again (7–10 ms on a large project); entries
sort by directory name, so a re-read orders and hashes exactly like a fresh read; a rescan adds and removes module nodes. Hashes are recomputed up the dirty
ancestors only, and unchanged nodes keep their `Arc`, so two trees differ exactly where their pointers differ and a
leaf edit costs a few µs. `Place`/`WeakDom` is
built only for `opossum build` and for blobs; the sourcemap and log index are rebuilt only on structural deltas.

- `props`: the node's explicit properties under the names rbx_binary serializes (`Disabled`, never `Enabled`, which
  does not serialize: translating it would switch every template on). Values are what the plugin will write:
  ContentId → Content migrations happen when the node is built. `Attributes` and `Tags` are props.
- `refs`: ref-valued properties, as target ids. A ref into a re-read `place/` entry is resolved again by path.
- `source`: a script's `Source`, an `Arc<str>` shared with the compiler output.
- `hash`: xxh3-128 over class, props sorted by name in opossum's own byte encoding (floats by bits, SharedString by its
  hash), refs as the path from holder to target in `(name, class, ordinal)` steps (never an id), the source's own hash, then each
  child's `(name, hash)` in order. The node's own name and parent are left out, so a renamed or moved subtree keeps
  its hash, while a parent's hash certifies its children's names.
- Excluded from hashing and change detection, carried in blobs as is: engine metadata (`Capabilities`,
  `SourceAssetId`, `ScriptGuid`, `SerializedOverrides`, `UniqueId`, `HistoryId`, `NeedsPivotMigration`,
  `InertiaMigrated`, `SlimHash`, `ModelMesh*`, `Sandboxed`).

**Identity.** When a delta replaces a subtree, the new nodes take ids from the old ones:
1. children of a matched parent pair by `(name, class, ordinal among siblings with that name and class)`; a class
   change at one path is a new id, and the new node's children pair with the old node's children the same way;
2. then unpartnered nodes, shallowest first and in tree order, each claim a vanished node with an equal hash (lowest
   id first) together with its whole subtree, positionally; a claim that would reuse a taken id is skipped;
3. anything left gets a fresh id. An id belongs to at most one node, and claims carry whole subtrees, so moves never
   form a cycle.

A duplicate `(name, class)` group that changed size, or lost a member to a replacement, is new as a whole group, and a
member of a duplicate group never claims by hash except inside a claimed ancestor. Studio appends what it is given, so
a group that mixed kept and new members would put them in a different order than the tree's ordinals, and the next
hydrate would pair the wrong instances.

**Roots.** Services and nested containers (`StarterPlayer/StarterPlayerScripts`, `StarterCharacterScripts`,
`Workspace/Terrain`) are **frames of reference**: bound by class or name, written for properties only, never
created, removed or replaced. Their children in the tree are the **roots**: route targets (`ReplicatedStorage/shared`;
a deeper target's first folder), `ReplicatedStorage/rbxts`, every top-level entry of a `place/services/<service>`
folder, and every module a `*` route puts directly into a service.

- Inside a root, children are **authoritative**: a live instance the tree lacks is a stray and a verify removes it,
  except `Archivable = false` instances and classes `Instance.new` cannot create (the plugin learns this per class
  with a cached `pcall`).
- A service's other children are the user's and are never touched. A map built in Workspace stays.
- Each root carries an `opossum` attribute: its **stamp**. The stamp marks ownership and says "this subtree is exactly
  the tree with this hash" (32 hex), or is `""` when the root is dirty. A root the plugin inserts or adopts gets `""`
  before its first write. An instance with the attribute that is no longer a root (its route or folder went away) is
  removed on connect, and so is a stamped duplicate of a root, or a stamped instance where the root's class changed.

**Stamps.** The edit plugin clears the stamp of every root a frame touches before its first write (a move touches
both roots), and writes the new hash only after the frame, if no op under that root failed and every async write has
returned. The server sends no stamp for a root holding a parked, kept or deferred node. It clears a
stamp the moment anything else changes the root, recognising its own writes by identity, not timing (handlers are
deferred and run after the apply):
- `DescendantAdded`/`DescendantRemoving` under the root, ignoring instances the plugin itself inserted or removed in
  the last frame (a weak set drained by a `task.defer` queued after the frame);
- `game.ItemChanged` for a mapped instance, while the plugin is not writing synchronously and only for properties a
  plugin can write (cached per class from the reflection service), which leaves out derived ones (`Assembly*`, `Mass`,
  `Extents*`). It fires synchronously for every property write (~0.33 µs per event) but not for attributes or tags,
  so someone else's attribute or tag edit is caught only by a Resync;
- `ScriptEditorService.TextDocumentDidChange` for a script under the root, ignoring the plugin's own updates;
- `ChangeHistoryService.OnRecordingFinished` (not the plugin's own recording) when the selection is under a root;
- `OnUndo`/`OnRedo` of an opossum waypoint, or any undo after an unrecorded frame: every stamp, and undoing an
  "opossum: sync" waypoint also sends `verify`.

The listeners start with the plugin, not with the connection, so edits made while `opossum dev` is down are caught.

A dirty root keeps `""`, so it is never re-stamped until a verify. Stamps travel into playtests (Play Solo copies the
edit DataModel) and survive restarts. The server keeps its last 16 generations' trees, indexed by root hash.

## Transport

One WebSocket per DataModel, `ws://127.0.0.1:<port>/`, port 34900 or the next free one of 16 (Rojo owns 34872). The
plugin tries each port in turn, `Close()`ing every client that fails before the next. The server speaks first.

- Studio allows 5 `RequestAsync` in flight per process, shared by every plugin and DataModel, and the play client may
  not use it at all. Plugins get 4 WebStreamClients per DataModel and other plugins use some. So: no HTTP, one socket
  per DataModel, and a failed `CreateWebStreamClient` means "try again later".
- Down: binary frames, intact up to at least 64 MB (16 MB in ~40 ms). Up: UTF-8 text only (anything else kills the
  socket), so upstream messages are JSON and the plugin replaces invalid UTF-8 in log text before encoding.
- The handshake is refused unless `Host` is `127.0.0.1`/`localhost` with our port and there is no `Origin`: Studio
  sends none (measured), and pages in a browser, which can open local WebSockets with CORS not applying, always do.
  Log text has control characters stripped before it is printed.
- Threads: `publish` swaps an `Arc<Tree>` and notifies. Each socket has a reader and a writer thread; the writer builds
  its subscriber's frame when it is ready to send (after the previous frame's ack), diffing that subscriber's known
  tree against the current one, so a backlog collapses into one frame and a stalled Studio never blocks a publish.

### Up (JSON text)

```
{"t":"hello", "role":"edit"|"server"|"client", "place":<PlaceId>, "roots":{"<path>":"<stamp>", ...}}
{"t":"ack", "gen":G, "failed":[[id,tag,"message"], ...]}
{"t":"lost", "ids":[...]}            # mapped instances removed by someone else
{"t":"verify"}                       # the Resync button
{"t":"log", "lines":[["output"|"warning"|"error","text"], ...]}
```

The `hello` lists every instance carrying an `opossum` attribute that sits directly under a service or nested
container, by `/`-joined path from the DataModel, with its stamp. Each generation gets exactly one `ack` or one
`verify`; an ack for another generation than the one in flight is printed and otherwise ignored. The plugin `Close()`s
every socket it drops.

### Down

The first message is JSON text: `{"t":"hello","project","place","unpublished","session","hotReload"}`. The plugin
closes the socket unless `place == game.PlaceId`, or `PlaceId == 0` and `unpublished`; otherwise it drops its id map
and queue, waits for any apply in flight, and answers with its own `hello`. Every later server message is a binary
**frame**. The server sends no frame before its first publish, so a Studio that connects during the first build is
never answered from an empty tree.

## Frames

Little-endian; a string is `u32 length, bytes`.

```
u32 gen, u32 base_gen, u8 last
u32 blob_len, blob_len bytes        rbxm (rbx_binary) or nothing
u32 op_count, ops...
```

The plugin queues frames and one worker applies them, never two at once. A frame whose `base_gen` is not the last
generation it applied is dropped, together with the rest of its generation's parts, and answered with `verify` (a
lost frame never desyncs silently). An apply error partway through a part also skips the rest of the generation.
`verify` ends the server's wait for an ack, and the plugin takes the frame that
answers it whatever its `base_gen`. The worker acks after a frame with `last = 1`; the server sends nothing else to
that subscriber until then. A repair frame, sent with no new publish, has `gen = max(published, base_gen + 1)`.

The blob holds every instance the frame needs from the engine's own reader, as roots in the order the ops consume
them. `SerializationService:DeserializeInstancesAsync` reads it in one call (it never yields; 40k parts ~100 ms;
~0.16 ms per call, so one blob per frame). rbx_binary writes roots in the order given and children in order, and the
engine keeps both. Refs whose target is inside the same blob travel in it; refs to anything else are `ref` ops (a ref
out of the blob reads back nil; a Weld, Motor6D or WeldConstraint with a nil part survives the read, so the `ref` op
fills it in).

| tag | op | fields | meaning |
|---|---|---|---|
| 1 | `bind` | `id, parent, str class, str name` | a frame of reference: `GetService(class)` when `parent` is `0xFFFFFFFF`, else the child of `parent` named `name` with class `class`. With class `""`, an orphan: the stamped child of `parent` named `name`, any class |
| 2 | `hydrate` | `parent, u32 n, n × node` | map existing children of `parent` to ids, no writes; under a frame of reference only stamped children pair |
| 3 | `children` | `parent, u8 flags, u32 n, n × node` | reconcile or insert `n` blob roots under `parent` |
| 4 | `props` | `id, u8 carrier, u16 n, n × str name` | copy `name`s from the next blob root onto `id` |
| 5 | `source` | `id, str source` | a script's Source |
| 6 | `move` | `id, parent, str name` | reparent and/or rename, identity kept |
| 7 | `ref` | `id, str prop, target` | `inst[prop] = map[target]`, `0xFFFFFFFF` is nil |
| 8 | `remove` | `id` | destroy `id`; it and every mapped descendant leave the map |
| 9 | `stamp` | `id, str hash` | a root's stamp (edit only) |
| 10 | `swap` | `id` | hot reload: the next blob root replaces `id` (play server only) |
| 11 | `reload` | `u32 n, n × id` | hot reload: Scripts to start after the reload signal (play server only) |

A `hydrate` node is `id, str name, str class, u32 child_count`, then its children, preorder. A `children` node is
`id, u16 prop_count, prop_count × str name, u32 child_count`, then its children: the shape of the blob root it
describes plus the properties the server set on each.

**Order within a frame**: `bind`, `hydrate`, `children` (parent first), `move` (ascending depth in the new tree; when
the new parent is currently inside the moving node, the plugin detaches the new parent first), `swap`, `props`, `source`, `ref`,
`remove`, `stamp`, `reload`. A move can target a parent created this frame, refs see every instance, and a removed
parent's surviving children have moved out first.

**Failures.** An op whose `id`, `parent` or `target` is unmapped fails and writes nothing (`remove` of an unmapped id
succeeds), and leaves every root of the generation unstamped, since it points at no instance; an unmapped `stamp`
fails alone. If the blob throws, every op that consumes it fails. `copy` `pcall`s each property and fails only that
property, acked with tag 4 even inside a reconcile, its message starting with the property name (`Size: …`). Each
failure is acked as `[id, tag, message]`:
- a failed `children` node leaves the subscriber's known tree with its blob-made descendants;
- a failed `move` restores the known parent and name;
- a failed `props`, `source` or `ref` marks those values unknown, so they are sent in full;
- a failed `remove` (a `Destroy` that throws, acked with tag 8) keeps the node.
The server sends the repair at once. A failed reconcile node verifies its root again instead, or is left out of the known tree if it is parked. A failed
`move` whose old parent was removed in the same frame is inserted again. A node that fails again, in a later frame,
with unchanged content is parked: its
failure prints once, then once more with a parked note, until its content changes or Resync. A blob that throws makes
the server re-send each failed subtree (or verify each root) in its own frame once, so the bad one is isolated and
parked. Failures print in
the dev terminal against the nearest ancestor's source file, with the instance path.

**Big frames.** The server splits a frame above ~10k new instances or 16 MB into parts that share `gen` (only the last
has `last = 1`). Leading parts carry only `bind`, `hydrate` and `children`; everything else rides the last part. An
oversized subtree is split at depth: its shell first, then its children as further inserts. The worker yields one
`task.wait()` between parts. A verify splits at root granularity: one root is never split, and a root over the limits
gets a part of its own.

### `children`

- `flags & 1` **reconcile**: pair the parent's live children with the blob roots by `(name, class, ordinal)`,
  skipping exempt live children (`Archivable = false`, non-creatable classes) unless a node names them. At a frame of
  reference a stamped child pairs first, else (edit only) an unstamped one is adopted; the user's other children are
  never looked at.
  - A pair: `copy` the listed properties plus `Attributes` and `Tags`, then recurse into the children. Refs of
    reconciled nodes, and refs into them, always arrive as `ref` ops, never inside the blob.
  - A blob root with no live partner: parent the blob instance itself in. Its whole subtree is new.
  - A live child with no partner, below a root (never directly below a frame of reference), that is not mapped: a
    stray, whether mapped or not, unless a later op of the same generation moves or removes it. Strays are settled at
    the end of the generation and removed with `flags & 2` (edit only), otherwise left and reported. At a frame of
    reference only a stamped, unpaired child named like a reconciled root is a stray.
- Without `flags & 1` (**insert**): parent every blob root in, no pairing. A duplicate `(name, class)` group is always
  inserted whole, in tree order, so ordinals stay true.

Ids are assigned by walking each blob root against its node record, preorder. A child-count mismatch fails that node
and nothing of it is parented.

**Undo.** Unrecorded writes make no waypoint of their own but ride the next one anybody makes (measured), so a user's
Ctrl+Z would revert every sync since their last edit. In edit each frame is therefore an "opossum: sync" ChangeHistory
recording (~0.04 ms a frame): the user's Ctrl+Z reverts only their own work until they step back over a sync
waypoint. A frame with async writes or strays records twice: the sync waypoint, then the join, then a second waypoint
("opossum: sync", or "opossum: resync" with strays) holding strays, removed with `Parent = nil` so Ctrl+Z gives them
back, and stamps. Frames of reference are bound before the recording opens, so an undo never tries to unparent a
service. Measured: an empty recording leaves no waypoint, and Undo does not revert `.Source` writes. When
`TryBeginRecording` returns nil (the play server, or another recording is open) the frame applies unrecorded, strays
are kept and their root stays unstamped. Every removed stray's path is printed in the dev terminal.

### `copy`

`copy(live, carrier, names)` is the one property writer, used by `props` and by reconcile. It runs `__mesh`/`__csg`
first, then the rest:
- `__mesh`: `live:ApplyMesh(carrier)`, which waits for the asset (~1.5 s for a new one), so it runs in parallel like
  `UpdateSourceAsync` and the frame joins it (its carrier lives until the join); sent when any of MeshId/MeshContent, InitialSize, PhysicsData,
  PhysicalConfigData, AeroMeshData, Unscaled*, VertexCount, HasSkinnedMesh, HasJointOffset, JointOffset,
  CollisionFidelity, RenderFidelity, FluidFidelityInternal changed. `MeshId` cannot be written by a plugin.
- `__csg`: `live:SubstituteGeometry(carrier)` for PartOperations, when MeshData, ChildData, AssetId, PhysicalConfigData
  or InitialSize changed.
- `__pivot`: `live.WorldPivot = carrier.WorldPivot` when they differ. `Scale`: `live:ScaleTo(carrier:GetScale())`.
- `__materialColors` (Terrain): `SetMaterialColor` per material, from the carrier Folder's Color3 attributes.
- `Source`: set if different; `ScriptEditorService:UpdateSourceAsync` when the script is open in an editor or the
  source is 200 000 characters or more (`.Source` refuses those). Those run last, in parallel, and the frame joins them,
  giving up after 20 s.
- `Attributes`: make `live`'s attributes the carrier's (set changed, clear missing), never touching `opossum`.
- `Tags`: make `live`'s tags the carrier's.
- anything else: `if live[name] ~= carrier[name] then live[name] = carrier[name] end` (both are engine-normalized, so
  equal values compare equal).

`carrier = 0`: the blob root is the node alone, without children, in its own class, holding the new values;
properties it does not hold read as the class default, so a removed property resets. `carrier = 1`, for a frame of
reference or any class that cannot be serialized: a Folder whose attributes are the property values (enums as
`EnumItem`, numbers as float64), with a child Folder `Attributes` whose attributes and tags are the target's
`Attributes`/`Tags`.

The server sends a property only when it can be written: reflection scriptability `ReadWrite`, a name above, or a
property unknown to the reflection database (copied under `pcall`). Anything else that changed on a creatable class
makes the server replace the instance (new id inserted, children moved, old removed); on a frame of reference or a
non-creatable class it is build-only and said once per property in the terminal when it changes, never on connect
(`HttpEnabled`, `Technology`, `AllowCustomAnimations` are skipped silently), as are properties a carrier cannot
represent (Content, Faces, Int64, Vector3int16, BinaryString on a frame of reference), and a property removed from a
frame of reference, which carrier 1 cannot reset. A `props` op with no names is never sent.

## Sessions

**Connect.** Per root:
- a stamp in the plugin's `hello` equal to the current root hash or one in the generation ring: `hydrate` from that
  generation's tree, then patch to the current tree in the same frame. Connect builds one known tree: a node whose id
  it already holds arrives by `move`, never twice;
- otherwise, and for a root the `hello` lacks, a **verify**: `children(frame, reconcile | authoritative, [root])`,
  every node's properties listed and carried by its blob instance. Pairing at the frame finds the stamped root, or
  **adopts** an unstamped instance at the root's `(name, class)` (left by an older sync, or made by hand), or inserts
  the blob root: never a duplicate. A verify sends `remove` for mapped nodes the tree dropped, and nodes held under
  another root arrive by `move`, keeping their identity. In edit a root that appears during a session is reconciled
  the same way;
- a stamped instance that is no longer a root: `bind` with class `""` (the `hello` carries none), then `remove`,
  printed; one that is already gone is silently fine;
- a frame of reference that appears in a later generation (a service newly given `[place]` properties) is bound and
  its properties copied. A frame of reference that leaves the tree is never removed, only its former children.

A connect frame that never applied (dropped, or failed) is answered by connecting again. Hydrate pairs by
`(name, class, ordinal)` among non-exempt children. In edit, a hydrate that does not match (a count
or name differs: something changed that no signal caught) fails the root and the server answers with a verify. A
verify of a whole project (1,110 modules, 13.6 MB of source, 867 place instances) is one frame; a clean reconnect is a
hydrate, no writes. The Resync button verifies every root.

**Publish.** Per subscriber, the writer diffs the known tree against the current one over the ids the generations
changed: new id → `children` insert of its topmost new ancestor's subtree, stopping at old ids, which `move`; parent
or name changed → `move`; props changed → `props` with the changed names; source changed → `source`; gone → `remove`
of the topmost. Per changed root, a `stamp`. The known tree becomes the current one.

**Lost instances.** When something other than the plugin removes a mapped instance (the user deletes a synced module),
the edit plugin unmaps it and its descendants and sends `lost`; the server drops them from the known tree, so the next
frame puts them back. The files are the truth. The play server never sends `lost`: game code destroys and reparents
synced instances at runtime; the server ignores a `lost` from it as a backstop. After `lost`, refs from elsewhere
into the restored subtree are sent again.

## Playtests

Plugins run in all three Play Solo DataModels; each connects with its role (`IsEdit()` is edit; otherwise `IsServer()`
is server, which includes Run mode, and the rest is client). Ids are `u32`; `0xFFFFFFFF` is never an id.
- **edit** keeps syncing straight through a playtest: `IsRunning()` is false there in Play Solo, and its writes
  persist after Stop.
- **server** always connects, to forward the play server's output, including what it printed before the socket
  opened (`LogService:GetLogHistory()` plus a buffer of up to 1000 lines; the client does the same). Only with `hotReload` does its `hello` list roots
  and does it receive frames. It hydrates from the stamps it inherited, tolerantly: extra live
  children are runtime state, a node that does not pair is dropped from that subscriber's tree and logged. It never
  verifies, never prunes, never stamps, never watches, and receives only changes made after its hydrate. When it
  answers a frame with `verify`, it is sent everything again from its known tree before that frame.
- **client** only forwards logs: the play server's writes to replicated containers reach it by replication (Source,
  new instances, destroyed scripts, all measured), and client code lives in ReplicatedStorage.

**Hot reload** (`[dev] hotReload = true`). `require` caches per instance and a running Script ignores a new Source, so
in the play server a changed script is swapped, never edited: the server sends `swap id` with the whole new instance
(node alone, Scripts carried `Disabled = true`), and the plugin renames the old instance out of the way, moves its
children into the new one, parents it, points the id at it and destroys the old one. The server swaps every transitive
importer the same way, up to the Scripts, using the compiler's require graph (each module's runtime require targets,
recorded at render; `require("…")` parsed in `.luau`). After the frame the plugin writes the reload list
(`gen|full.name,...`) to the `reload` attribute of a Configuration `ReplicatedStorage.OpossumReload` it creates, which
replicates to the client, and then, in a `task.defer` so the deferred handlers run first, enables the Scripts `reload`
lists. A project's dev runtime can watch that attribute to dispose what modules created: module side effects (connections,
threads made at require time) outlive their module (measured), and a module nothing requires statically (one a loader
finds through `GetDescendants`) is re-required only by such a runtime. `DeserializeInstancesAsync` works
from the play-server plugin (measured). A ref held elsewhere to a swapped instance is not repointed.

## Budgets

| | target | measured on a large project (2026-10-28) |
|---|---|---|
| server publish after a leaf edit | ≤ 2 ms (a few µs of tree work) | 0.11–0.16 ms; 0.41 ms for a hub edit re-lowering 285 modules |
| frame for one changed source | one `source` op, no blob | one 13 KB `source` op and a `stamp` |
| plugin apply of a leaf edit | < 1 ms, or one frame when the script is open in an editor | 0.59 ms; 16.6 ms open in an editor |
| clean reconnect (every root stamped) | hydrate only, < 20 ms | 14.7 ms, 101 KB, no writes |
| full verify | one frame, < 300 ms | 130 ms, no writes; first insert 5.88 MB in 142 ms |

A file save reaches Studio in 50–250 ms, almost all of it the macOS file watcher; the sync itself takes about a
millisecond.
