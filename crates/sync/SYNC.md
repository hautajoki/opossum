# Sync

`opossum dev` keeps Studio equal to the files: every instance opossum owns, every property, attribute, tag, ref,
mesh and union, every script source, while never touching what the user built in Studio. One WebSocket per
DataModel, one binary frame per generation, a plugin with no per-type value codec. Numbers quoted here were measured
in Studio 0.745 (2026-10-28).

## Model

**Tree.** The dev session owns one long-lived `Tree`: a slab of immutable nodes indexed by `Id` (`u32`, never reused
within a server session), `Arc<Node { parent, name, class, props, refs, source, children, hash }>`. It changes by
deltas, never by a rebuild: a re-rendered module sets one node's source; a `place/` file event re-reads that one entry
and pairs ids inside its subtree; a rescan adds and removes module nodes. Hashes are recomputed up the dirty
ancestors only, and each generation records the ids it changed, so a leaf edit costs a few µs. `Place`/`WeakDom` is
built only for `opossum build` and for blobs; the sourcemap and log index are rebuilt only on structural deltas.

- `props`: the node's explicit properties under the names rbx_binary serializes (`Disabled`, never `Enabled`, which
  does not serialize: translating it would switch every template on). Values are what the plugin will write:
  ContentId → Content migrations happen when the node is built. `Attributes` and `Tags` are props.
- `refs`: ref-valued properties, as target ids.
- `source`: a script's `Source`, an `Arc<str>` shared with the compiler output.
- `hash`: xxh3-128 over class, props sorted by name in opossum's own byte encoding (floats by bits, SharedString by its
  hash), refs as the path from holder to target in `(name, class, ordinal)` steps (never an id), source, then each
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
  the tree with this hash" (32 hex), or is `""` when the root is dirty. An instance with the attribute that is no
  longer a root (its route or folder went away) is removed on connect.

**Stamps.** The edit plugin clears the stamp of every root a frame touches before its first write, and writes the
new hash only after the frame, if no op under that root failed and every `UpdateSourceAsync` has returned. It clears a
stamp the moment anything else changes the root, recognising its own writes by identity, not timing (handlers are
deferred and run after the apply):
- `DescendantAdded`/`DescendantRemoving` under the root, ignoring instances the plugin itself inserted or removed in
  the last frame (a weak set drained by a `task.defer` queued after the frame);
- `game.ItemChanged` for a mapped instance, if it fires for plugin-visible writes (measure; otherwise
  `GetPropertyChangedSignal("Source")` per synced script), ignoring the plugin's own writes the same way;
- `ScriptEditorService.TextDocumentDidChange` for a script under the root, ignoring the plugin's own updates;
- `ChangeHistoryService.OnRecordingFinished` (not the plugin's own recording) when the selection is under a root;
  `OnUndo`/`OnRedo`: every stamp.

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
- The handshake is refused unless `Host` is `127.0.0.1`/`localhost` with our port and `Origin` is absent or the value
  Studio sends (measure it). Pages in a browser can open local WebSockets; CORS does not apply. Log text has control
  characters stripped before it is printed.
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
container, by `/`-joined path from the DataModel, with its stamp.

### Down

The first message is JSON text: `{"t":"hello","project","place","unpublished","session","hotReload"}`. The plugin
closes the socket unless `place == game.PlaceId`, or `PlaceId == 0` and `unpublished`; otherwise it drops its id map
and queue, waits for any apply in flight, and answers with its own `hello`. Every later server message is a binary
**frame**.

## Frames

Little-endian; a string is `u32 length, bytes`.

```
u32 gen, u32 base_gen, u8 last
u32 blob_len, blob_len bytes        rbxm (rbx_binary) or nothing
u32 op_count, ops...
```

The plugin queues frames and one worker applies them, never two at once. A frame whose `base_gen` is not the last
generation it applied is dropped and answered with `verify` (a lost frame never desyncs silently). The worker acks
after a frame with `last = 1`; the server sends nothing else to that subscriber until then.

The blob holds every instance the frame needs from the engine's own reader, as roots in the order the ops consume
them. `SerializationService:DeserializeInstancesAsync` reads it in one call (it never yields; 40k parts ~100 ms;
~0.16 ms per call, so one blob per frame). rbx_binary writes roots in the order given and children in order, and the
engine keeps both. Refs whose target is inside the same blob travel in it; refs to anything else are `ref` ops (a ref
out of the blob reads back nil, and the engine drops a joint whose part is outside the blob; measure that a joint with
a nil part survives).

| tag | op | fields | meaning |
|---|---|---|---|
| 1 | `bind` | `id, parent, str class, str name` | a frame of reference: `GetService(class)` when `parent` is `0xFFFFFFFF`, else the child of `parent` named `name` with class `class` |
| 2 | `hydrate` | `parent, u32 n, n × node` | map existing children of `parent` to ids, no writes |
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

**Order within a frame**: `bind`, `hydrate`, `children` (parent first), `move` (ascending depth in the new tree; the
plugin detaches a node first when its new parent is currently its descendant), `swap`, `props`, `source`, `ref`,
`remove`, `stamp`, `reload`. A move can target a parent created this frame, refs see every instance, and a removed
parent's surviving children have moved out first.

**Failures.** An op whose `id`, `parent` or `target` is unmapped fails and writes nothing (`remove` of an unmapped id
succeeds). If the blob throws, every op that consumes it fails. `copy` `pcall`s each property and fails only that
property. Each failure is acked as `[id, tag, message]`:
- a failed `children` node leaves the subscriber's known tree with its blob-made descendants;
- a failed `move` restores the known parent and name;
- a failed `props`, `source` or `ref` marks those values unknown, so they are sent in full;
- a failed `remove` keeps the node.
The server sends the repair at once. A node that fails again with unchanged content is parked and printed once until
its content changes or Resync; a blob that throws makes the server re-send each failed subtree in its own frame once,
so the bad one is isolated and parked. Failures print in the dev terminal against the node's source file.

**Big frames.** The server splits a frame above ~10k new instances or 16 MB into parts that share `gen` (only the last
has `last = 1`). Leading parts carry only `bind`, `hydrate` and `children`; everything else rides the last part. An
oversized subtree is split at depth: its shell first, then its children as further inserts. The worker yields one
`task.wait()` between parts.

### `children`

- `flags & 1` **reconcile**: pair the parent's live children with the blob roots by `(name, class, ordinal)`,
  skipping exempt live children (`Archivable = false`, non-creatable classes) unless a node names them.
  - A pair: `copy` the listed properties, then recurse into the children.
  - A blob root with no live partner: parent the blob instance itself in. Its whole subtree is new.
  - A live child with no partner, below a root (never directly below a frame of reference): a stray. It is removed
    with `flags & 2` (edit only), otherwise left and reported.
- Without `flags & 1` (**insert**): parent every blob root in, no pairing. A duplicate `(name, class)` group is always
  inserted whole, in tree order, so ordinals stay true.

Ids are assigned by walking each blob root against its node record, preorder. A child-count mismatch fails that node
and nothing of it is parented.

Removals of strays run inside one ChangeHistory recording ("opossum: resync") with `Parent = nil`, so Ctrl+Z gives
back anything a verify took; if `TryBeginRecording` returns nil the frame waits for the next Heartbeat, never applied
unrecorded. Every removed path is printed in the dev terminal. Nothing else records: a sync is not the user's edit, and
unrecorded writes make no undo waypoint (measured).

### `copy`

`copy(live, carrier, names)` is the one property writer, used by `props` and by reconcile. It runs `__mesh`/`__csg`
first, then the rest:
- `__mesh`: `live:ApplyMesh(carrier)`, sent when any of MeshId/MeshContent, InitialSize, PhysicsData,
  PhysicalConfigData, AeroMeshData, Unscaled*, VertexCount, HasSkinnedMesh, HasJointOffset, JointOffset,
  CollisionFidelity, RenderFidelity, FluidFidelityInternal changed. `MeshId` cannot be written by a plugin.
- `__csg`: `live:SubstituteGeometry(carrier)` for PartOperations, when MeshData, ChildData, AssetId, PhysicalConfigData
  or InitialSize changed.
- `__pivot`: `live.WorldPivot = carrier.WorldPivot`. `Scale`: `live:ScaleTo(carrier:GetScale())`.
- `__materialColors` (Terrain): `SetMaterialColor` per material, from the carrier Folder's Color3 attributes.
- `Source`: set if different; `ScriptEditorService:UpdateSourceAsync` when the script is open in an editor or the
  source is 200 000 characters or more (`.Source` refuses those). Those run last, in parallel, and the frame joins them.
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
non-creatable class it is build-only and said once per property in the terminal, as are properties a carrier cannot
represent (Content, Faces, Int64, Vector3int16, BinaryString on a frame of reference).

## Sessions

**Connect.** Per root in the plugin's `hello`:
- a stamp equal to the current root hash or one in the generation ring: `hydrate` from that generation's tree, then
  patch to the current tree in the same frame. Connect builds one known tree: a node whose id it already holds
  arrives by `move`, never twice;
- otherwise a **verify**: `props` for the root, then `children(root, reconcile | authoritative, root's children)`,
  every node's properties listed and carried by its blob instance. The reconcile parent is the root itself, never a
  frame of reference;
- a root in the tree that the `hello` lacks: `children` insert under its frame of reference;
- a stamped instance that is no longer a root: `remove`, printed.

Hydrate pairs by `(name, class, ordinal)` among non-exempt children. In edit, a hydrate that does not match (a count
or name differs: something changed that no signal caught) fails the root and the server answers with a verify. A
verify of a whole project (1,110 modules, 13.6 MB of source, 867 place instances) is one frame; a clean reconnect is a
hydrate, no writes. The Resync button verifies every root.

**Publish.** Per subscriber, the writer diffs the known tree against the current one over the ids the generations
changed: new id → `children` insert of its topmost new ancestor's subtree, stopping at old ids, which `move`; parent
or name changed → `move`; props changed → `props` with the changed names; source changed → `source`; gone → `remove`
of the topmost. Per changed root, a `stamp`. The known tree becomes the current one.

**Lost instances.** When something other than the plugin removes a mapped instance (the user deletes a synced module),
the plugin unmaps it and its descendants and sends `lost`; the server drops them from the known tree, so the next
frame puts them back. The files are the truth.

## Playtests

Plugins run in all three Play Solo DataModels; each connects with its role (`IsEdit()` is edit; otherwise `IsServer()`
is server, which includes Run mode, and the rest is client). Ids are `u32`; `0xFFFFFFFF` is never an id.
- **edit** keeps syncing straight through a playtest: `IsRunning()` is false there in Play Solo, and its writes
  persist after Stop.
- **server** always connects, to forward the play server's output. Only with `hotReload` does its `hello` list roots
  and does it receive frames. It hydrates from the stamps it inherited, tolerantly: extra live
  children are runtime state, a node that does not pair is dropped from that subscriber's tree and logged. It never
  verifies, never prunes, never stamps, and receives only changes made after its hydrate.
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
finds through `GetDescendants`) is re-required only by such a runtime. Unmeasured, to check first:
`DeserializeInstancesAsync` from the play-server plugin; the fallback is `Instance.new` + `.Source`.

## Budgets

| | target |
|---|---|
| server publish after a leaf edit on a large project | ≤ 2 ms (a few µs of tree work) |
| frame for one changed source | one `source` op, no blob |
| plugin apply of a leaf edit | < 1 ms, or one frame when the script is open in an editor |
| clean reconnect (every root stamped) | hydrate only, < 20 ms for a project |
| full verify of a project | one frame, < 300 ms |
