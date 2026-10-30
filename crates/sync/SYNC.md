# Sync

`opossum dev` keeps Studio equal to the files: every instance opossum owns, every property, attribute, tag, ref,
mesh and union, every script source, while never touching what the user built in Studio. One WebSocket per
DataModel, one binary frame per generation, a plugin with no per-type value codec. Numbers quoted here were measured
in Studio 0.745 (2026-10-28).

## Model

**Tree.** The dev session owns one long-lived `Tree`: a slab of immutable nodes indexed by `Id` (`u32`, never reused
within a server session), `Arc<Node { parent, name, class, props, refs, source, children, hash }>`. It changes by
deltas, never by a rebuild: a re-rendered module sets one node's source; a `place/` file event re-reads that entry with
its same-named siblings (an event on a service directory, on a nested container's own directory or under a service
directory that went away or on a properties file (`place/*.toml`) re-reads the whole place; other loose top-level files
are never content), re-applies the properties files, resolves
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
  before its first write, and so does a root the plugin moves under a frame of reference; a move that nests a root
  inside another removes its attribute, and a verify clears a leftover one on nested instances. An instance with the
  attribute that is no longer a root (its route or folder went away) is removed on connect and on every edit verify,
  and so is a stamped duplicate of a root, or a stamped instance where the root's class changed.

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
- `ChangeHistoryService.OnRecordingFinished` (not the plugin's own recording) when the selection is under a root.
  Studio's undo restores the selection, so it can leave synced roots selected for the next recording;
- `OnUndo`/`OnRedo` of an opossum waypoint, any undo or redo that changed a clean root, or any step while unrecorded
  writes are unverified: every stamp, and a `verify` (see Undo). Every step also clears each dirty root's stamp again,
  since the step may have reverted the plugin's unrecorded clear.

`ItemChanged` fires for every part a drag moves, so it returns early: once a frame has mapped the place, an unmapped
instance is new (its root was dirtied by `DescendantAdded`), a kept stray or exempt, and an already dirty root stays
dirty. The plugin's own-write check is constant time once every structural write it made has been drained.

The listeners start with the plugin, not with the connection, so edits made while `opossum dev` is down are caught.
The edit plugin watches the roots it inserts, adopts, pairs or moves in; a full scan of the frames runs only at start
and at hello.

A dirty root keeps `""`, so it is never re-stamped until a verify, and the `hello` and `verify` list it as `""`
whatever its attribute says (an undo may have put an old stamp back). Stamps travel into playtests (Play Solo copies the
edit DataModel) and survive restarts. The server keeps its last 16 generations' trees, indexed by root hash.

## Transport

One WebSocket per DataModel, `ws://127.0.0.1:<port>/`, port 34900 or the next free one of 16 (Rojo owns 34872). A port is
free only when nothing answers on `127.0.0.1:port`: std binds with `SO_REUSEADDR`, which on macOS succeeds beside a
wildcard listener and would take its loopback traffic. The plugin first tries the port it last synced from (Attach,
below), then each port in turn, closing every client that fails before the next and disconnecting its handlers (a
WebStreamClient with live connections is never collected, measured), so a plugin that loses a front proxy falls back
to opossum's own port. The server speaks first. The same listener answers one plain `POST` per connection, to `/exec`
(Exec, below), `/play` or `/stop` (Playtests), `/pull` (Pull), or `/status` and `/release` (Attach).

- Studio allows 5 `RequestAsync` in flight per process, shared by every plugin and DataModel, and the play client may
  not use it at all. Plugins get 4 WebStreamClients per DataModel and other plugins use some. So: no HTTP, one socket
  per DataModel, and a failed `CreateWebStreamClient` means "try again later".
- Down: binary frames, intact up to at least 64 MB (16 MB in ~40 ms). Up: UTF-8 text only (anything else kills the
  socket), so upstream messages are JSON and the plugin replaces invalid UTF-8 in log text before encoding.
- The handshake is refused unless `Host` is `127.0.0.1`/`localhost` with our port and there is no `Origin`: Studio
  sends none (measured), and pages in a browser, which can open local WebSockets with CORS not applying, always do.
  A `POST` passes the same two checks: a page's cross-site POST always carries `Origin`, and `Host` defeats DNS
  rebinding.
  Log text has control characters stripped before it is printed.
- Threads: `publish` swaps an `Arc<Tree>` and notifies. Each socket has a reader and a writer thread; the writer builds
  its subscriber's frame when it is ready to send (after the previous frame's ack), diffing that subscriber's known
  tree against the current one, so a backlog collapses into one frame and a stalled Studio never blocks a publish. A
  writer that panics prints why and shuts the socket down, so the plugin reconnects instead of waiting on it.

### Up (JSON text)

```
{"t":"hello", "role":"edit"|"server"|"client", "place":<PlaceId>, "roots":{"<path>":"<stamp>", ...}}
{"t":"ack", "gen":G, "failed":[[id,tag,"message"], ...], "ms":M}
{"t":"lost", "ids":[...]}            # mapped instances removed by someone else
{"t":"verify", "quiet":bool, "roots":{...}}   # Resync, an undo over a sync, a dropped or failed frame
{"t":"log", "lines":[["output"|"warning"|"error","text"], ...]}
{"t":"result", "id":N, "ok":bool, "output":[["output"|"warning","text"], ...], "values":[...], "error":"...",
 "trace":"...", "ms":M, "late":bool}   # an exec's answer
```

The `hello` lists every instance carrying an `opossum` attribute that sits directly under a service or nested container,
by `/`-joined path from the DataModel, with its stamp; a path that appears twice goes out as `""`, so its root is
verified and the duplicate removed. A playtest DataModel that takes no frames lists them too: they are the stamps Play
copied from edit, and tell the server which build it holds (Exec). `verify` carries the same `roots`. Each generation
gets exactly one `ack` or one `verify`; an ack for another generation than the one in flight is reported and otherwise
ignored. An ack's `ms` is the wall time (`os.clock`) from the generation's first part arriving, or from the worker
finishing an exec it queued behind if that came later, to the ack: queueing behind other frames and every part's apply
included, an exec's run not. The plugin `Close()`s every socket it drops. A verify is `quiet` when an undo or redo asked
for it, or when the frame that failed or was dropped was itself quiet.

The server reports to the dev session (`sync::Report`), in order per socket: a hello (connected, with its realm and
place), the ack of the frame in flight (its failures one by one, then applied, with `ms` and the failure count), a
`verify` that ends the wait for one (and whether it is quiet), log lines and the server's own notices (realm `sync`),
and the socket closing (disconnected). Each names the published generation the frame brings the DataModel to, never
the frame's own number, which a repair takes past the last publish. A late ack reports only its failures.
`opossum dev` shows each report as one event.

### Down

The first message is JSON text: `{"t":"hello","project","place","unpublished","session","hotReload"}`. The plugin closes
the socket unless `place == game.PlaceId`, or `PlaceId == 0` and `unpublished`; otherwise it drops its id map and queue,
waits for any apply in flight, and answers with its own `hello`. Every later server message is a binary **frame**, or an
exec: four zero bytes (generation 0, which no frame has), then
`{"t":"exec","id","name","source","timeout","start","limit","now"}`, or the same way `{"t":"attach","port"}`
(Attach). The server sends no frame before its first publish, so
a Studio that connects during the first build is never answered from an empty tree. A config change to `placeId`,
`unpublished` or `hotReload` closes every socket, so each plugin checks the new hello; other changes keep them.

## Frames

Little-endian; a string is `u32 length, bytes`.

```
u32 gen, u32 base_gen, u8 flags          bit 0: the generation's last part; bit 1: quiet
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
| 1 | `bind` | `id, parent, str class, str name` | a frame of reference: `GetService(class)` when `parent` is `0xFFFFFFFF`, else the child of `parent` named `name` with class `class`. With class `""`, an orphan: the stamped child of `parent` named `name`, any class, removed by a later `remove` like a stray |
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
The server sends the repair at once. A failed reconcile node (`children` or `props`, a reconciled `Source` included)
verifies its root again instead; once parked it stays paired with its live instance, its root unstamped, and its root
is verified again when the node changes. A failed `move` or `remove` whose node's old parent was removed in the same
frame, or that was reported lost while the frame was in flight, is inserted again. A node that fails again, in a later
frame, with unchanged content is parked: its failure prints once, then once more with a parked note, until its content
changes or Resync. Any blob that fails to load starts isolation: the server re-sends each failed subtree (or verifies
each root) in its own frame once, so the bad one is isolated and parked. A ref whose target the subscriber will not
hold after the frame is held back, its root unstamped; refs from elsewhere into reconciled or re-inserted nodes are
sent again. Failures print in the dev terminal against the nearest ancestor's source file, with the instance path.

**Big frames.** The server splits a frame above ~10k new instances or 16 MB into parts that share `gen` (only the last
has the last bit). Leading parts carry only `bind`, `hydrate` and `children`; everything else rides the last part. An
oversized subtree is split at depth: its shell first, then its children as further inserts. The worker yields one
`task.wait()` between parts. A verify splits at root granularity: one root is never split, and a root over the limits
gets a part of its own.

### `children`

- `flags & 1` **reconcile**: pair the parent's live children with the blob roots by `(name, class, ordinal)`,
  skipping exempt live children (`Archivable = false`, non-creatable classes) unless a node names them. At a frame of
  reference a stamped child pairs first, else (edit only) an unstamped one is adopted; the user's other children are
  never looked at.
  - A pair: `copy` the listed properties plus `Attributes` and `Tags`, then recurse into the children. A reconcile
    node lists its explicit properties plus every property of its class that is canonical, serialized, not
    deprecated and writable (`ReadWrite`, or a special write kind such as Model `Scale`), less the special groups,
    `Name`, `Source`, `Attributes`, `Tags` and the refs it holds; the blob copy holds class defaults, so a verify
    resets any drift, nil refs included (about 2.6% more frame on a large project). An unset pivot is not verified. Refs of
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
back, and stamps. A service the place lacks is created in its own "opossum: services" waypoint before the frame's
recording opens (an unrecorded `GetService` would ride the frame's waypoint, and `SetEnabled(false)` wipes the user's
history); undoing it leaves the service with a harmless "Parent is locked" warning. Measured: an empty recording
leaves no waypoint unless unrecorded writes are pending (they ride any recording that commits, and a step an undo or
redo takes), and Undo does not revert `.Source` writes. When `TryBeginRecording` returns nil (the play server,
or another recording is open) the frame applies unrecorded, strays are kept and their root stays unstamped. Every
removed stray's path is printed in the dev terminal.

A verify sent because of an undo or redo is quiet: the server sets the quiet bit on every part of the generation that
answers it (never on a connect frame), and the plugin applies exactly those frames unrecorded and removes no strays,
so they leave no waypoint and the next Ctrl+Z continues to the user's waypoint below instead of landing on the verify
it just caused. A frame already in flight when the undo happened keeps recording as usual. What the step removed is
not reported lost, since the verify pairs every root again.

The quiet frame's writes ride the next waypoint anyone makes, however deep it ends up. The plugin therefore counts
what an unrecorded generation changed (any content write but Source, a mesh or union only when its MeshId or fidelity
differs, since geometry cannot be read back, and a root's stamp ending other than before) and remembers each root's
stamp as it left it. An undo or redo then verifies (quietly) only when an opossum waypoint was stepped over, when
unrecorded writes that changed something have not been verified since, or when the step itself changed a clean root:
the watch signals fired during it, or a root's stamp came back other than the plugin left it. Opening its own
"opossum: sync" recording settles the pending writes, since they ride that waypoint. So undoing a user's edit costs no
verify; the undo right after a verify does verify again (36–54 ms server, 65–84 ms apply), because the verify
rewrote the stamp the step reverted. Undoing "opossum: resync" verifies too: the strays coming back fire the signals.
Strays stay until a Resync, including a duplicate a redo brings back, which `require` may then pick. Unrecorded
writes by anyone (Studio's lazily created services, other plugins) before opossum's first waypoint ride that one.

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
  giving up after 20 s; a job the join gave up on stops writing and, if it lands later, leaves its root dirty.
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
- a stamped instance that is no longer a root: `bind` with class `""` (the `hello` carries none), then `remove` with
  `Parent = nil` inside the "opossum: resync" waypoint, so Ctrl+Z gives it back, printed (kept and reported when no
  recording opens); one that is already gone is silently fine;
- a frame of reference that appears in a later generation (a service a properties file newly sets) is bound and
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
frame puts them back. The files are the truth. A lost id the in-flight frame places anew verifies its root instead.
The play server never sends `lost`: game code destroys and reparents
synced instances at runtime; the server ignores a `lost` from it as a backstop. After `lost`, refs from elsewhere
into the restored subtree are sent again.

## Playtests

Plugins run in all three Play Solo DataModels; each connects with its role (`IsEdit()` is edit; otherwise `IsServer()`
is server, which includes Run mode, and the rest is client). Ids are `u32`; `0xFFFFFFFF` is never an id.
- **edit** keeps syncing straight through a playtest: `IsRunning()` is false there in Play Solo, and its writes
  persist after Stop.
- **server** always connects, to forward the play server's output, including what it printed before the socket
  opened (`LogService:GetLogHistory()` plus a buffer of up to 1000 lines; the client does the same). Only with
  `hotReload` does it receive frames (without, its `hello`'s roots only say which build it holds, Exec). It hydrates
  from the stamps it inherited, tolerantly: extra live children are runtime state, a node that does not pair is
  dropped from that subscriber's tree and logged. It never verifies, never prunes, never watches, and receives only changes made after its hydrate. It
  keeps the stamps it applies in memory, never in the attribute, and lists them in its `hello` and `verify`, so a
  reconnect hydrates from where it is; a failure clears its remembered stamp. When it answers a frame that threw with
  `verify`, it first destroys that generation's inserts and detaches what moved into them, then is sent everything
  again from its known tree before that frame.
- **client** only forwards logs: the play server's writes to replicated containers reach it by replication (Source,
  new instances, destroyed scripts, all measured), and client code lives in ReplicatedStorage.

**Starting and stopping** (`opossum play`, `stop`). Both are execs of a few lines dev writes itself,
so the plugin has no message for them. Play runs in edit once it holds the newest build: it refuses when
`StudioTestService.EditModeActive` is false (a playtest runs), else defers
`StudioTestService:ExecutePlayModeAsync("opossum")` (`ExecuteRunModeAsync` for `--run`), so the test starts after the
exec's recording ends; the method yields until the test does, in its own thread, never the worker. Its argument cannot
be nil ("Argument 1 missing or nil", measured). dev then waits for the server's hello (and the client's in Play Solo)
and sends the play server an empty exec, whose answer says the build it holds; the playtest runs whatever that answers.
Stop runs `StudioTestService:EndTest("opossum")` in the play server, which ends any Studio test, not only one a plugin
started (measured: Play Solo, Run, and one the Studio MCP started), as a `now` exec (Exec), so a long exec or frame
there never holds it up; its answer may be lost to the socket closing, and dev waits for every socket of the playtest to
close (it counts them: a test started by hand may have several clients). Their log lines all arrive before the close, so
the tally dev keeps of the playtest's errors and warnings (from the first of its DataModels connecting while none was)
is whole by then. StudioTestService's multiplayer test is not offered. Measured in the blank place (2026-10-29): play
returns 1.9 s after the request (0.7 s for Run), stop 0.3–0.5 s.

**Hot reload** (`[dev] hotReload = true`). `require` caches per instance and a running Script ignores a new Source, so
in the play server a changed script is swapped, never edited: the server sends `swap id` with the whole new instance
(node alone, Scripts carried `Disabled = true`), and the plugin renames the old instance out of the way, moves its
children into the new one, parents it, points the id at it and destroys the old one. The server swaps every transitive
importer the same way, up to the Scripts, using the compiler's require graph (each module's runtime require targets,
recorded at render; `require("…")` parsed in `.luau`). After every frame carrying `reload`, even one that swapped only modules, the plugin writes the reload list
(`gen|full.name,...`) to the `reload` attribute of a Configuration `ReplicatedStorage.OpossumReload` it creates, which
replicates to the client, and then, in a `task.defer` so the deferred handlers run first, enables the Scripts `reload`
lists. A project's dev runtime can watch that attribute to dispose what modules created: module side effects (connections,
threads made at require time) outlive their module (measured), and a module nothing requires statically (one a loader
finds through `GetDescendants`) is re-required only by such a runtime. `DeserializeInstancesAsync` works
from the play-server plugin (measured). A ref held elsewhere to a swapped instance is not repointed.

## Exec

`opossum exec` and `opossum test` run Luau in a connected DataModel: the CLI posts to `opossum dev`,
which makes sure its build covers every file saved before the request (its fence) and hands the server the plugin's
`exec` message for the newest DataModel of the asked realm, failing at once when none is connected.

**When.** The socket's writer sends the exec only once nothing is in flight and nothing is left to send: the DataModel
has acked the newest published tree (edit, or the play server with hot reload), so it holds every build up to the
request. A DataModel that takes no frames (the play server without hot reload, the client) gets it at once and runs
the code it started with. Which build that is comes from the stamps in its hello, which Play copied from edit: the
newest of the last 16 builds whose every root it lists with that build's hash (a client, which sees only replicated
roots, counts the roots it lists), or 0 for none. So a playtest started while edit was still applying a save holds the
build before it, however late its plugin says hello. The answer is `fresh` when that is the newest build; an exec that
asks for fresh code (`opossum test`) is refused instead of sent to a DataModel that is not. The plugin queues the exec
behind the frames that arrived before it, and the frames after it wait for it: one worker, so no frame applies while
the code runs. Its `result` goes back up the same socket. An exec with `now`, only dev's own snippets that need no build
(stop's EndTest, the packs capture below), skips both waits: the writer sends it at once, saying the build the
DataModel last acked, and the plugin runs it in a thread of its own beside the worker.

**Giving up.** An exec may wait 30 s to start, in the writer and then in the plugin's queue; `start` tells the plugin
how many seconds of that are left when it is sent. Until the writer hands it over, a waiter that gives up takes it
back, so it never runs. A plugin that gets to it after `start` ran out answers `late` without running it. Once it ran,
the waiter gives the plugin its `timeout` and 5 s more past the start; when that runs out, or the socket closes, the
answer is the server's own `{"ok": false, "lost": true}`: the code may have run, or still be running. An answer that
it never ran is only ever given when it never will.

**Running.** The chunk is a nil-parented ModuleScript named after the file, holding
`return function(...) ` spliced in front of the source's first code, on that line (directives such as `--!native`
stay first, line numbers stay the user's, a top-level `...` is legal as in any script), and `\nend` after it, loaded
as a module (below) and called, so every value it returns is answered. A source of 200,000 characters or more is set
with `UpdateSourceAsync`, which `Source` refuses. The print and warn of the chunk and of every module it requires
record into the answer's `output`, so what a spec's code prints is in `opossum test`'s output, not only in Studio's;
a thread the exec leaves running prints to Output once the answer is sent. It runs in a
thread under `xpcall`: an error answers with its message and `debug.traceback` less the plugin's own frames; returned
values are JSON, every leaf JSON has no word for (an Instance, a Vector3, a function, inf, nan) as its `tostring`,
nested ones included, and a value with a cycle or keys JSON cannot hold as its `tostring` whole. A `tostring` that
throws shows the value's type. Output, returned values, and the error and its trace are each cut at 1 MB, saying how
much was cut, so a result stays far below what one message may carry; an exec's `limit` raises the cut for returned
values (pull's, 24 MB of base64 within the 32 MB a message up may hold; a 10.7 MB message measured). After `timeout` seconds (default 60) the
thread is cancelled; a loop that never yields holds the plugin until Studio's script timeout stops it. Nothing the
code does can stop the worker: whatever escapes the run is answered as the exec's error, and the worker carries on.
The chunk and everything below are destroyed after, so nothing is left in the DataModel. In edit the run is one
"opossum: exec" ChangeHistory recording: what the script writes is a waypoint of its own that Ctrl+Z takes back (and
the stamps watch.luau keeps catch any synced instance it touched).

**Require.** Studio caches a ModuleScript's result per instance for the life of the plugin's VM: after a `Source`
write, a rename, a reparent or `UpdateSourceAsync`, `require` of the same instance returns the first result, and only
a new instance loads anew (measured in Studio 0.745, 2026-10-28). Sync writes `Source` in place, so the plugin's
`require` would run stale code from the second exec on. So the chunk and everything it requires load
through exec.luau's own `require`, which runs each module's **body**: its `Source` wrapped as
`return function(script, require, print, warn, ...) <source> end` (spliced in front of the first code line, so `--!native` stays a
directive and lines keep their numbers), compiled by requiring a copy placed in a nil-parented mirror of the
original's ancestors' names, so chunk names, errors and tracebacks read `ReplicatedStorage.shared.x:12` as the
original's would and map to files the same way. `script` is the original, `require` the loader, which resolves
instances and `@self`/`@game`/`./`/`../` paths against the original, and `print` and `warn` the exec's. A body is compiled once per `Source` and called
anew by each exec, so module state never leaks from one run into the next while compiled code is kept: Studio never
frees native code, and compiling fresh copies of every module per run ran the edit VM out of it after about a dozen
`opossum test` runs over a large project ("Memory allocation limit reached for native code generation", every module interpreted
from then on and the whole set past its 60 s limit; measured). A body that throws while loading throws through the
requires like any call, so its traceback reaches the code that failed; an engine error (`x is not a valid member of
y`) names no place, so its stack's first frame is put in front (`ReplicatedStorage.shared.x:3: x is not ...`), and a
later require of a module that failed rethrows its error, as Studio's does. A syntax error makes the copy's `require`
say only "Requested module experienced an error while loading" (measured): compiling the source again with
`loadstring` says what (plugins have it; `debug.loadmodule` is not enabled).

**Packs.** dev's texture pack capture is an exec it writes, `packs`, sent to edit with `now`
and pull's `limit`, never fenced (a fence file every 10 s would wake the watcher each time). Its source starts with two
tables spliced in, `WANTED` (the pack keys the lock lacks) and `NUDGE` (those of them missing for 60 s), then walks
`game:GetDescendants()` for SurfaceAppearances and MaterialVariants, spells each one's key from its four `…MapContent`
uris as `place::pack_key` does, picks the first instance of each wanted key, and returns `EncodingService:Base64Encode`
of `SerializationService:SerializeInstancesAsync(picks)` ("" for none), which never waits on a bake: dev reads each
pick's key and `TexturePackContent` from the bytes, so a key the Luau spells differently can only be missed, never
saved wrong. The picks of `NUDGE` have their maps set to `Content.none` and back, which makes Studio retry its bake,
in a thread the exec spawns: inside the exec's own "opossum: exec" recording that write is an undo step, and a
recording begun while it is open is refused (measured 2026-10-30), so the thread tries `TryBeginRecording("opossum:
packs")` each frame, for up to 300, until the exec's has finished, nudges, and finishes it with `Cancel`, leaving no
waypoint. The nudged instances are synced ones, so watch.luau marks their roots dirty and the next verify finds
nothing to change.

Measured on a test project in the blank place (2026-10-28): 20 of 20 "write a module's return value, immediately exec a
require of it" runs saw the new value, for a `.luau` module and for a TypeScript one, about 65 ms per run end to end
(46 ms median with nothing saved, the fence's wait); new files 85–94 ms; the play server with hot reload 20 of 20. 20
of 20 "write a module and a spec expecting its value, immediately `opossum test` it" runs passed, 155 ms each in edit
(1.26 s while every run compiled every module afresh). `opossum test` over a large project's 1,203 cases: 13.7 s in edit, steady
over 15 runs in a row, 24 s in the play server; its 176 suites that also run locally hold the same 1,107 cases.

## Pull

`opossum pull` is an exec dev writes, run in edit: it finds the instance by name from `game`, clones
it, clears the clone's `opossum` attribute, destroys in the clone what routes mount inside it (each by its names from
the instance, which dev lists: the tree's own copy comes from `src/`, so the file must not hold another) and returns its
class and `EncodingService:Base64Encode` of `SerializationService:SerializeInstancesAsync({clone})`, with a `limit` that
lets the model through. Writing the file alone would sync wrong: the root is dirty (what the user built or tuned there
cleared its stamp), but the server does not know, and the diff of the new build inserts every node it has not mapped
(what was built in Studio) beside the live instance, which the next verify removes as a stray (measured: a second copy).
So before writing into `place/`, dev calls `Server::adopt(path)`: each edit subscriber keeps the path with the hash of
the tree's node there now (0 for none), and in the first frame whose tree differs there verifies that node's root
instead of diffing it. The reconcile pairs the live instances by `(name, class, ordinal)`, as a connect does, so they
keep their identity, and removes only what the files do not hold. A build before the file is read leaves the path
waiting; a pull whose content equals the files' leaves it waiting until the next change there, which then verifies its
root (a verify is always correct, only larger). A file written outside `place/` is not adopted, and the play server,
which never verifies, is not told.

## Attach

Every dev listens on one of the 16 ports, so `opossum attach` needs no registry: it asks each port
for `/status` (`Server::status`: the project, the place it serves, the build it published, each connected DataModel's
realm, place and the build it holds, the execs waiting or running), then asks the dev holding Studio for `/release`.
That dev (`Server::release`) refuses while a play DataModel is connected or an exec waits or runs, unless forced, and
otherwise queues `attach` with the new port for every socket; the writer sends it between frames. The plugin takes a
port of its own range only: it records it as the port it wants, drops the socket (its queue with it, the apply in
flight finishing first) and connects there at once, through the usual hello, place check and connect frame; a dev that
does not answer or serves another place sends it back to the scan. Both `/status` and `/release` pass the Host and
Origin checks; neither carries a project root, since other projects ask them.

The port a plugin wants is the last it connected to, whatever dev serves it when it reconnects. The edit plugin also
writes that dev to its settings under `dev<PlaceId>` on every connect, as `{port, session}` (the session from the dev's
hello), and every plugin reads that setting when it starts: plugin settings are shared by the plugin in every DataModel
of a Studio (measured, 2026-10-29: Play Solo's server and client joined the dev edit had been moved to, not the first on
the ports), so a playtest started after an attach syncs from the same dev as edit, and so does a Studio opened later. A
plugin starting takes the saved port only from a hello of that session: once that dev exits, a dev that takes its port
is another (an unpublished place's setting is shared by every unpublished place), and the plugin scans as usual.

## Budgets

| | target | measured on a large project (2026-10-28) |
|---|---|---|
| server publish after a leaf edit | ≤ 2 ms (a few µs of tree work) | 0.11–0.16 ms; 0.41 ms for a hub edit re-lowering 285 modules |
| frame for one changed source | one `source` op, no blob | one `source` op and a `stamp` |
| plugin apply of a leaf edit | < 1 ms, or one frame when the script is open in an editor | 0.4–0.6 ms; 16.6 ms open in an editor |
| clean reconnect (every root stamped) | hydrate only, < 20 ms | 9.7 ms, 101 KB, no writes |
| full verify | < 300 ms | 6.03 MB; 42–51 ms server, 53–84 ms apply, no content writes |
| quiet verify (an undo's) | < 300 ms | 36–54 ms server, 65–84 ms apply |
| first insert | — | 6.03 MB, 2,112 instances, ~109 ms apply |

A file save reaches Studio in 50–250 ms, almost all of it the macOS file watcher; the sync itself takes about a
millisecond.
