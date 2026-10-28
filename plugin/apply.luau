--!native
--!strict
-- The apply worker's body: decodes one binary frame (crates/sync/SYNC.md, "Frames") and applies its ops to the
-- DataModel. It owns the id map, the stamps and the record of the plugin's own writes that watch.luau reads.

local ChangeHistoryService = game:GetService("ChangeHistoryService")
local ReplicatedStorage = game:GetService("ReplicatedStorage")
local ScriptEditorService = game:GetService("ScriptEditorService")
local SerializationService = game:GetService("SerializationService")

local NIL = 0xFFFFFFFF
-- `.Source` refuses sources this long; UpdateSourceAsync takes them.
local SOURCE_LIMIT = 200000
local SYNC = "opossum: sync"
local RESYNC = "opossum: resync"
-- How long a frame waits for its UpdateSourceAsync and ApplyMesh calls before failing the ones still running.
local JOIN_TIMEOUT = 20
-- Containers below a service that are frames of reference themselves.
local NESTED: { [string]: true } = { StarterPlayerScripts = true, StarterCharacterScripts = true, Terrain = true }

export type Role = "edit" | "server" | "client"
export type Failure = { number | string }
type Hydrate = { id: number, name: string, class: string, kids: { Hydrate } }
type Node = { id: number, props: { string }, kids: { Node } }
-- An async write the frame joins: whom its failure is acked against.
type Job = { id: number, tag: number, at: Instance }

local M = {
	role = "edit" :: Role,
	-- The last generation applied in full; nil after a dropped frame, when the next generation is taken as is.
	applied = 0 :: number?,
	by_id = {} :: { [number]: Instance },
	id_of = {} :: { [Instance]: number },
	-- Roots something other than the plugin changed: never stamped until a verify reconciles them.
	dirty = setmetatable({}, { __mode = "k" }) :: { [Instance]: true },
	-- The current generation's `[id, tag, message]` failures, acked and cleared by the caller.
	failed = {} :: { Failure },
	-- Set when the last generation inserted, moved or removed anything, so the watched roots may have changed.
	structural = false,
	-- True while a frame writes synchronously: every ItemChanged then is the plugin's own (it fires synchronously).
	writing = false,
	-- Set when an edit frame applied without a recording: its writes ride the next waypoint anyone makes, so an
	-- undo may revert them.
	unrecorded = false,
}

-- The plugin's structural writes, by the token of the generation that made them, so the deferred Descendant*
-- handlers recognise them: `own_tree` holds instances inserted, moved or removed (their descendants count too),
-- `own_item` instances an async write (UpdateSourceAsync, ApplyMesh) changes after the synchronous apply.
local own_tree = setmetatable({}, { __mode = "k" }) :: { [Instance]: number }
local own_item = setmetatable({}, { __mode = "k" }) :: { [Instance]: number }
local token = 0
-- True between a generation's first part and its end.
local in_gen = false
local gen_now = 0
-- A generation dropped part way: its remaining parts are dropped too (the verify is already sent).
local skip: number? = nil

-- Per-generation state.
local cleared: { [Instance]: true } = {}
local bad_roots: { [Instance]: true } = {}
local poisoned = false
-- Ids whose `bind` found nothing: ops on them are skipped silently (the server re-inserts the subtree).
local unbound: { [number]: true } = {}
-- Instances mapped this generation, and unpaired live children of reconciled parents (each with that parent).
local claimed: { [Instance]: true } = {}
local strays: { { inst: Instance, parent: Instance } } = {}
local authoritative = false
local running: { [Job]: true } = {}
-- Blob roots an async write still reads (a carrier may sit deep in a reconciled blob): destroyed once the
-- generation has joined its jobs, never by the job itself, which may finish before the reconcile reads the rest.
local held: { [Instance]: true } = {}
local stamps: { { root: Instance, hash: string } } = {}
local reloads: { Instance } = {}
-- The recording the frame part in progress writes under, if any.
local recording: string? = nil

-- Decoder state for the frame being applied.
local buf = buffer.create(0)
local pos = 0
local roots: { Instance } = {}
local next_root = 1
local blob_error: string? = nil

local function u8(): number
	local v = buffer.readu8(buf, pos)
	pos += 1
	return v
end

local function u16(): number
	local v = buffer.readu16(buf, pos)
	pos += 2
	return v
end

local function u32(): number
	local v = buffer.readu32(buf, pos)
	pos += 4
	return v
end

local function str(): string
	local n = u32()
	local s = buffer.readstring(buf, pos, n)
	pos += n
	return s
end

-- `text` with every invalid UTF-8 byte replaced, so it can travel in a text frame.
function M.clean(text: string): string
	if utf8.len(text) then return text end
	local out = {}
	local at = 1
	while true do
		local ok, bad = utf8.len(text, at)
		if ok then
			table.insert(out, string.sub(text, at))
			return table.concat(out)
		end
		local b = bad :: number
		table.insert(out, string.sub(text, at, b - 1))
		table.insert(out, "\u{FFFD}")
		at = b + 1
	end
end

local function is_frame(inst: Instance): boolean
	local parent = inst.Parent
	return parent == game or (parent ~= nil and parent.Parent == game and NESTED[inst.ClassName] == true)
end

-- The root `inst` sits in (or is): the child of a frame of reference that carries an `opossum` attribute.
function M.root_of(inst: Instance?): Instance?
	while inst do
		local parent = inst.Parent
		if parent == nil or parent == game then return nil end
		if is_frame(parent) then return if inst:GetAttribute("opossum") ~= nil then inst else nil end
		inst = parent
	end
	return nil
end

-- Every root, by its `/`-joined path from the DataModel (a service by its class name).
function M.roots(): { [string]: Instance }
	local found = {}
	local function scan(frame: Instance, path: string)
		for _, child in frame:GetChildren() do
			if child:GetAttribute("opossum") ~= nil then
				found[`{path}/{child.Name}`] = child
			elseif NESTED[child.ClassName] then
				scan(child, `{path}/{child.Name}`)
			end
		end
	end
	for _, service in game:GetChildren() do
		pcall(scan, service, service.ClassName)
	end
	return found
end

-- True when the plugin inserted, moved or removed `inst` or an ancestor of it in a generation not yet drained.
function M.owns(inst: Instance): boolean
	local at: Instance? = inst
	while at do
		if own_tree[at] then return true end
		at = at.Parent
	end
	return false
end

-- True when `inst` is the plugin's own write: an async one still running, or a structural one.
function M.owns_item(inst: Instance): boolean
	return own_item[inst] ~= nil or M.owns(inst)
end

local function map(id: number, inst: Instance)
	local old = M.by_id[id]
	if old and old ~= inst then M.id_of[old] = nil end
	local prev = M.id_of[inst]
	if prev and prev ~= id then M.by_id[prev] = nil end
	M.by_id[id] = inst
	M.id_of[inst] = id
	claimed[inst] = true
end

function M.unmap(inst: Instance)
	local id = M.id_of[inst]
	if id then
		M.id_of[inst] = nil
		M.by_id[id] = nil
	end
end

-- Forgets the plugin's own writes up to now once the deferred handlers queued so far have run.
local function drain()
	local upto = token
	task.defer(function()
		for inst, t in own_tree do
			if t <= upto then own_tree[inst] = nil end
		end
		for inst, t in own_item do
			if t <= upto then own_item[inst] = nil end
		end
	end)
end

local function commit()
	if recording then
		ChangeHistoryService:FinishRecording(recording, Enum.FinishRecordingOperation.Commit)
		recording = nil
	end
end

-- Forgets the generation in progress: its failures, pending stamps, strays, async writes and reloads.
function M.abort()
	M.writing = false
	commit()
	in_gen = false
	table.clear(M.failed)
	table.clear(cleared)
	table.clear(bad_roots)
	poisoned = false
	table.clear(unbound)
	table.clear(claimed)
	authoritative = false
	table.clear(strays)
	table.clear(running)
	table.clear(held)
	table.clear(stamps)
	table.clear(reloads)
	drain()
end

-- Gives up on the generation in progress after a frame of it failed: its remaining parts are dropped, and the
-- frame that answers the caller's verify is taken whatever its base.
function M.drop()
	M.applied = nil
	skip = gen_now
	M.abort()
end

-- Drops the id map and the generation in progress; the next frame starts a new session.
function M.reset()
	M.abort()
	table.clear(M.by_id)
	table.clear(M.id_of)
	M.applied = 0
	skip = nil
end

-- Records a failure. `at` is the instance it concerns (or the parent it was going under), whose root then stays
-- unstamped; nil when unknown, which leaves every root of the generation unstamped.
local function fail(id: number, tag: number, message: string, at: Instance?)
	table.insert(M.failed, { id, tag, M.clean(message) })
	if at then
		local root = M.root_of(at)
		if root then bad_roots[root] = true end
	else
		poisoned = true
	end
end

-- A failure because `what` is not mapped; nothing at all when `what`'s bind found nothing this generation.
local function fail_unmapped(id: number, tag: number, what: number, at: Instance?)
	if not unbound[what] then fail(id, tag, `id {what} is not mapped`, at) end
end

-- Clears the stamp of `inst`'s root before the generation's first write under it.
local function touch(inst: Instance)
	if M.role ~= "edit" then return end
	local root = M.root_of(inst)
	if root and not cleared[root] then
		cleared[root] = true
		if root:GetAttribute("opossum") ~= "" then root:SetAttribute("opossum", "") end
	end
end

local creatable: { [string]: boolean } = {}

-- Exempt children are never paired unless a node names them, never counted and never strays.
local function exempt(inst: Instance): boolean
	if not inst.Archivable then return true end
	local class = inst.ClassName
	local ok = creatable[class]
	if ok == nil then
		local made
		ok, made = pcall(Instance.new, class, nil)
		if ok then (made :: Instance):Destroy() end
		creatable[class] = ok
	end
	return not ok
end

-- The child of frame of reference `frame` that is the root `name` (of `class`, any when ""): one carrying the
-- `opossum` attribute. A service's other children are the user's.
local function find_root(frame: Instance, name: string, class: string): Instance?
	local first = frame:FindFirstChild(name)
	if not first then return nil end
	local function fits(child: Instance): boolean
		return child.Name == name and (class == "" or child.ClassName == class) and child:GetAttribute("opossum") ~= nil
	end
	if fits(first) then return first end
	for _, child in frame:GetChildren() do
		if fits(child) then return child end
	end
	return nil
end

-- Pairs blob roots with the live children of frame of reference `frame` by (name, class): a stamped child first,
-- else an unstamped non-exempt one, which is adopted (left by `opossum build`, an older sync, or made by hand).
-- Returns each blob's partner (false when none) and the stamped, unmapped children named like a blob that pair
-- with none: a duplicate, or a root whose class changed. The user's other children are never looked at.
local function pair_roots(frame: Instance, blobs: { Instance }): ({ Instance | false }, { Instance })
	local named: { [string]: true } = {}
	for _, blob in blobs do
		named[blob.Name] = true
	end
	local stamped: { [string]: { Instance } } = {}
	local loose: { [string]: { Instance } } = {}
	local candidates: { Instance } = {}
	for _, child in frame:GetChildren() do
		if not named[child.Name] then continue end
		local key = `{child.Name}\0{child.ClassName}`
		local into = if child:GetAttribute("opossum") ~= nil then stamped elseif exempt(child) then nil else loose
		if into then
			local group = into[key]
			if group then
				table.insert(group, child)
			else
				into[key] = { child }
			end
		end
		if into == stamped and not M.id_of[child] then table.insert(candidates, child) end
	end
	local taken: { [Instance]: true } = {}
	local partners: { Instance | false } = table.create(#blobs, false :: Instance | false)
	local function free(group: { Instance }?): Instance?
		for _, child in group or {} do
			if not taken[child] then return child end
		end
		return nil
	end
	for i, blob in blobs do
		local key = `{blob.Name}\0{blob.ClassName}`
		local child = free(stamped[key]) or free(loose[key])
		if child then
			partners[i] = child
			taken[child] = true
		end
	end
	local rest = {}
	for _, child in candidates do
		if not taken[child] then table.insert(rest, child) end
	end
	return partners, rest
end

-- Pairs `parent`'s live children with `keys` (`name\0class`, in order) by (name, class, ordinal). Returns each key's
-- partner (false when none) and the paired-able live children left over.
local function pair(parent: Instance, keys: { string }): ({ Instance | false }, { Instance })
	local named: { [string]: true } = {}
	for _, key in keys do
		named[key] = true
	end
	local groups: { [string]: { Instance } } = {}
	local live = parent:GetChildren()
	for _, child in live do
		local key = `{child.Name}\0{child.ClassName}`
		if named[key] or not exempt(child) then
			local group = groups[key]
			if group then
				table.insert(group, child)
			else
				groups[key] = { child }
			end
		end
	end
	local used: { [string]: number } = {}
	local taken: { [Instance]: true } = {}
	local partners: { Instance | false } = table.create(#keys, false :: Instance | false)
	for i, key in keys do
		local n = (used[key] or 0) + 1
		used[key] = n
		local group = groups[key]
		local child = group and group[n]
		if child then
			partners[i] = child
			taken[child] = true
		end
	end
	local rest = {}
	for _, child in live do
		local group = groups[`{child.Name}\0{child.ClassName}`]
		if group and not taken[child] then table.insert(rest, child) end
	end
	return partners, rest
end

local function read_hydrate(): Hydrate
	local id = u32()
	local name = str()
	local class = str()
	local n = u32()
	local kids: { Hydrate } = {}
	for i = 1, n do
		kids[i] = read_hydrate()
	end
	return { id = id, name = name, class = class, kids = kids }
end

local function read_node(): Node
	local id = u32()
	local props: { string } = {}
	for i = 1, u16() do
		props[i] = str()
	end
	local kids: { Node } = {}
	for i = 1, u32() do
		kids[i] = read_node()
	end
	return { id = id, props = props, kids = kids }
end

-- The next blob root, or the reason there is none.
local function take(): (Instance?, string?)
	if blob_error then
		next_root += 1
		return nil, blob_error
	end
	local inst = roots[next_root]
	next_root += 1
	return inst, if inst then nil else "the blob has no root left for this op"
end

-- Runs `work` as an async write the generation joins before its stamps; it returns its failure messages, which
-- are acked against `job` unless the join already gave up on it.
local function spawn_job(job: Job, work: () -> { string })
	running[job] = true
	own_item[job.at] = token
	task.spawn(function()
		local errors = work()
		if not running[job] then return end
		running[job] = nil
		for _, message in errors do
			fail(job.id, job.tag, message, job.at)
		end
	end)
end

-- Waits for the generation's async writes, failing whatever is still running after JOIN_TIMEOUT.
local function join()
	local deadline = os.clock() + JOIN_TIMEOUT
	while next(running) and os.clock() < deadline do
		task.wait()
	end
	for job in running do
		fail(job.id, job.tag, `timed out after {JOIN_TIMEOUT} s`, job.at)
	end
	table.clear(running)
end

local function set_source(script: ModuleScript, source: string, id: number, tag: number)
	if script.Source == source then return end
	if #source >= SOURCE_LIMIT or ScriptEditorService:FindScriptDocument(script) then
		spawn_job({ id = id, tag = tag, at = script }, function()
			local ok, err = pcall(ScriptEditorService.UpdateSourceAsync, ScriptEditorService, script, function()
				return source
			end)
			return if ok then {} else { `Source: {err}` }
		end)
	else
		script.Source = source
	end
end

-- A property value from a carrier: the blob instance itself (kind 0) or a Folder's attribute (kind 1).
local function get(carrier: Instance, kind: number, prop: string): unknown
	return if kind == 1 then carrier:GetAttribute(prop) else (carrier :: any)[prop]
end

-- Writes one property `name` from `carrier` onto `live`; see SYNC.md, "copy".
local function write(live: Instance, carrier: Instance, kind: number, name: string, id: number)
	if name == "__mesh" then
		(live :: MeshPart):ApplyMesh(carrier :: MeshPart)
	elseif name == "__csg" then
		(live :: PartOperation):SubstituteGeometry(carrier)
	elseif name == "__pivot" then
		local pivot = get(carrier, kind, "WorldPivot") :: CFrame
		if (live :: Model).WorldPivot ~= pivot then (live :: Model).WorldPivot = pivot end
	elseif name == "Scale" then
		local scale = if kind == 1 then carrier:GetAttribute("Scale") :: number else (carrier :: Model):GetScale()
		if (live :: Model):GetScale() ~= scale then (live :: Model):ScaleTo(scale) end
	elseif name == "__materialColors" then
		for material, color in carrier:GetAttributes() do
			local item = Enum.Material:FromName(material)
			if item and typeof(color) == "Color3" then (live :: Terrain):SetMaterialColor(item, color) end
		end
	elseif name == "Source" then
		set_source(live :: ModuleScript, get(carrier, kind, "Source") :: string, id, 4)
	elseif name == "Attributes" then
		local from = if kind == 1 then carrier:FindFirstChild("Attributes") :: Instance else carrier
		local want = from:GetAttributes()
		for key, value in want do
			if key ~= "opossum" and live:GetAttribute(key) ~= value then live:SetAttribute(key, value) end
		end
		for key in live:GetAttributes() do
			if want[key] == nil and key ~= "opossum" then live:SetAttribute(key, nil) end
		end
	elseif name == "Tags" then
		local from = if kind == 1 then carrier:FindFirstChild("Attributes") :: Instance else carrier
		local want: { [string]: true } = {}
		for _, tag_name in from:GetTags() do
			want[tag_name] = true
			live:AddTag(tag_name)
		end
		for _, tag_name in live:GetTags() do
			if not want[tag_name] then live:RemoveTag(tag_name) end
		end
	else
		local value = get(carrier, kind, name)
		if (live :: any)[name] ~= value then (live :: any)[name] = value end
	end
end

-- `__mesh`/`__csg` first, then the rest, each under its own pcall; returns the failures.
local function write_all(live: Instance, carrier: Instance, kind: number, names: { string }, id: number): { string }
	local errors = {}
	for pass = 1, 2 do
		for _, name in names do
			if (name == "__mesh" or name == "__csg") == (pass == 1) then
				local ok, err = pcall(write, live, carrier, kind, name, id)
				if not ok then table.insert(errors, `{name}: {err}`) end
			end
		end
	end
	return errors
end

-- The one property writer. ApplyMesh waits for its asset, so a copy with `__mesh` runs as a job the frame joins,
-- keeping the mesh-first order.
local function copy(live: Instance, carrier: Instance, kind: number, names: { string }, id: number)
	if table.find(names, "__mesh") then
		local top = carrier
		while top.Parent do
			top = top.Parent
		end
		held[top] = true
		spawn_job({ id = id, tag = 4, at = live }, function()
			return write_all(live, carrier, kind, names, id)
		end)
	else
		for _, message in write_all(live, carrier, kind, names, id) do
			fail(id, 4, message, live)
		end
	end
end

-- Maps a new blob subtree to its node records, preorder. A node whose child count differs is failed and destroyed.
-- Returns false when `inst` itself does not match.
local function claim(inst: Instance, node: Node, at: Instance): boolean
	local kids = inst:GetChildren()
	if #kids ~= #node.kids then return false end
	map(node.id, inst)
	for i, kid in kids do
		if not claim(kid, node.kids[i], at) then
			fail(node.kids[i].id, 3, `{kid.Name}: the blob has {#kid:GetChildren()} children, the tree {#node.kids[i].kids}`, at)
			kid:Destroy()
		end
	end
	return true
end

-- Parents a new blob subtree in. A new root (directly under a frame of reference) carries the `opossum`
-- attribute from the start, so it stays a root whatever happens to the rest of the generation, and stays
-- unstamped when part of it failed.
local function insert(parent: Instance, inst: Instance, node: Node)
	local before = #M.failed
	if not claim(inst, node, parent) then
		return fail(node.id, 3, `{inst.Name}: the blob has {#inst:GetChildren()} children, the tree {#node.kids}`, parent)
	end
	if is_frame(parent) then
		inst:SetAttribute("opossum", "")
		if #M.failed > before then bad_roots[inst] = true end
	end
	own_tree[inst] = token
	inst.Parent = parent
end

local reconcile_children: (parent: Instance, blobs: { Instance }, nodes: { Node }) -> ()

-- A verify copies Attributes and Tags whether or not the node sets any: the blob instance holds exactly the
-- tree's, so someone else's additions go.
local function with_attributes(names: { string }): { string }
	local out = table.clone(names)
	if not table.find(out, "Attributes") then table.insert(out, "Attributes") end
	if not table.find(out, "Tags") then table.insert(out, "Tags") end
	return out
end

local function reconcile(live: Instance, blob: Instance, node: Node)
	local kids = blob:GetChildren()
	if #kids ~= #node.kids then
		fail(node.id, 3, `{blob.Name}: the blob has {#kids} children, the tree {#node.kids}`, live)
		return
	end
	map(node.id, live)
	copy(live, blob, 0, with_attributes(node.props), node.id)
	reconcile_children(live, kids, node.kids)
end

-- Under a frame of reference the blobs are roots: each paired root carries the `opossum` attribute before its first
-- write and is no longer dirty, since the reconcile makes it exactly the tree.
function reconcile_children(parent: Instance, blobs: { Instance }, nodes: { Node })
	local frame = is_frame(parent)
	local partners, rest
	if frame then
		partners, rest = pair_roots(parent, blobs)
	else
		local keys: { string } = {}
		for i, blob in blobs do
			keys[i] = `{blob.Name}\0{blob.ClassName}`
		end
		partners, rest = pair(parent, keys)
	end
	for i, blob in blobs do
		local live = partners[i]
		if not live then
			insert(parent, blob, nodes[i])
			continue
		end
		if frame then
			if live:GetAttribute("opossum") == nil then
				print(`opossum: adopting {live:GetFullName()}`)
				live:SetAttribute("opossum", "")
			end
			M.dirty[live] = nil
			touch(live)
		end
		reconcile(live, blob, nodes[i])
	end
	-- Mapped or not: an unpaired child no node claims by the end of the generation is not in the tree. One a later
	-- op moves or removes has left `parent` by then.
	for _, inst in rest do
		table.insert(strays, { inst = inst, parent = parent })
	end
end

-- Removes (authoritative, under a recording of their own so Ctrl+Z gives them back) or reports the strays the
-- generation's reconciles found. A stray that cannot be removed leaves its root unstamped.
local function settle_strays(record: boolean)
	for _, stray in strays do
		local inst = stray.inst
		if claimed[inst] or inst.Parent ~= stray.parent then continue end
		if authoritative and record then
			print(`opossum: removed {inst:GetFullName()}`)
			own_tree[inst] = token
			inst.Parent = nil
			M.unmap(inst)
			for _, below in inst:GetDescendants() do
				M.unmap(below)
			end
		else
			print(`opossum: kept {inst:GetFullName()}, which the files do not have (Resync removes it)`)
			local root = M.root_of(inst)
			if root then bad_roots[root] = true end
		end
	end
	table.clear(strays)
end

local function op_bind()
	local id, parent_id, class, name = u32(), u32(), str(), str()
	local inst: Instance?
	if parent_id == NIL then
		local ok, service = pcall(game.GetService, game, class)
		inst = if ok then service else nil
	else
		local parent = M.by_id[parent_id]
		if not parent then
			unbound[id] = true
			return fail_unmapped(id, 1, parent_id)
		end
		if NESTED[class] or not is_frame(parent) then
			-- a nested frame of reference, by class and name
			for _, child in parent:GetChildren() do
				if child.Name == name and (class == "" or child.ClassName == class) then
					inst = child
					break
				end
			end
		else
			-- a root (an orphan, by name only): never anything but a stamped instance
			inst = find_root(parent, name, class)
		end
	end
	if inst then
		map(id, inst)
	else
		unbound[id] = true
		table.insert(M.failed, { id, 1, M.clean(`no {class} {name}`) })
	end
end

local function hydrate(live: Instance, node: Hydrate, strict: boolean, mapped: { Instance }): string?
	map(node.id, live)
	table.insert(mapped, live)
	local kids = node.kids
	local keys: { string } = {}
	for i, kid in kids do
		keys[i] = `{kid.name}\0{kid.class}`
	end
	local partners, rest = pair(live, keys)
	if strict and rest[1] then return `{rest[1]:GetFullName()} is not in the tree` end
	for i, kid in kids do
		local partner = partners[i]
		if partner then
			local err = hydrate(partner, kid, strict, mapped)
			if err then return err end
		elseif strict then
			return `{live:GetFullName()}.{kid.name} ({kid.class}) is missing`
		else
			fail(kid.id, 2, `{live:GetFullName()}.{kid.name} ({kid.class}) is missing`, live)
		end
	end
	return nil
end

local function op_hydrate()
	local parent_id, n = u32(), u32()
	local nodes: { Hydrate } = {}
	for i = 1, n do
		nodes[i] = read_hydrate()
	end
	local parent = M.by_id[parent_id]
	if not parent then
		for _, node in nodes do
			fail_unmapped(node.id, 2, parent_id)
		end
		return
	end
	-- Edit compares counts below a root; the play server takes extra live children as runtime state.
	local strict = M.role == "edit"
	local partners: { Instance | false }
	if is_frame(parent) then
		-- roots, by their stamp: a service's other children are the user's
		partners = table.create(n, false :: Instance | false)
		for i, node in nodes do
			local root = find_root(parent, node.name, node.class)
			if root then partners[i] = root end
		end
	else
		local keys: { string } = {}
		for i, node in nodes do
			keys[i] = `{node.name}\0{node.class}`
		end
		local rest
		partners, rest = pair(parent, keys)
		if strict and rest[1] then
			for _, node in nodes do
				fail(node.id, 2, `{rest[1]:GetFullName()} is not in the tree`, parent)
			end
			return
		end
	end
	for i, node in nodes do
		local live = partners[i]
		if not live then
			fail(node.id, 2, `{parent:GetFullName()}.{node.name} ({node.class}) is missing`, parent)
			continue
		end
		local mapped = {}
		local err = hydrate(live, node, strict, mapped)
		if err then
			for _, inst in mapped do
				M.unmap(inst)
			end
			fail(node.id, 2, err, live)
		end
	end
end

local function op_children()
	local parent_id, flags, n = u32(), u8(), u32()
	local nodes: { Node } = {}
	for i = 1, n do
		nodes[i] = read_node()
	end
	local blobs: { Instance } = {}
	local missing: string? = nil
	for i = 1, n do
		local blob, err = take()
		blobs[i] = blob
		missing = missing or err
	end
	local parent = M.by_id[parent_id]
	if not parent then
		for _, node in nodes do
			fail_unmapped(node.id, 3, parent_id)
		end
		return
	elseif missing then
		for _, node in nodes do
			fail(node.id, 3, missing, parent)
		end
		return
	end
	M.structural = true
	touch(parent)
	if bit32.btest(flags, 1) then
		authoritative = authoritative or (bit32.btest(flags, 2) and M.role == "edit")
		reconcile_children(parent, blobs, nodes)
	else
		for i, blob in blobs do
			insert(parent, blob, nodes[i])
		end
	end
end

local function op_props()
	local id, kind, n = u32(), u8(), u16()
	local names: { string } = {}
	for i = 1, n do
		names[i] = str()
	end
	local carrier, err = take()
	local live = M.by_id[id]
	if not live then
		return fail_unmapped(id, 4, id)
	elseif not carrier then
		return fail(id, 4, err :: string, live)
	end
	touch(live)
	copy(live, carrier, kind, names, id)
end

local function op_source()
	local id, source = u32(), str()
	local live = M.by_id[id]
	if not live then return fail_unmapped(id, 5, id) end
	touch(live)
	local ok, err = pcall(set_source, live :: ModuleScript, source, id, 5)
	if not ok then fail(id, 5, `Source: {err}`, live) end
end

local function op_move()
	local id, parent_id, name = u32(), u32(), str()
	local live, parent = M.by_id[id], M.by_id[parent_id]
	if not live or not parent then return fail(id, 6, `id {if live then parent_id else id} is not mapped`, live) end
	M.structural = true
	touch(live)
	touch(parent)
	own_tree[live] = token
	local ok, err = pcall(function()
		-- A parent that is still inside `live` (a parent/child swap) steps out first; its own move places it.
		if parent:IsDescendantOf(live) then
			own_tree[parent] = token
			parent.Parent = nil
		end
		if live.Name ~= name then live.Name = name end
		if live.Parent ~= parent then live.Parent = parent end
	end)
	if not ok then fail(id, 6, tostring(err), live) end
end

local function op_ref()
	local id, prop, target_id = u32(), str(), u32()
	local live = M.by_id[id]
	local target = if target_id == NIL then nil else M.by_id[target_id]
	if not live or (target_id ~= NIL and not target) then return fail_unmapped(id, 7, if live then target_id else id, live) end
	touch(live)
	local ok, err = pcall(function()
		if (live :: any)[prop] ~= target then (live :: any)[prop] = target end
	end)
	if not ok then fail(id, 7, `{prop}: {err}`, live) end
end

local function op_remove()
	local id = u32()
	local live = M.by_id[id]
	if not live then return end
	M.structural = true
	touch(live)
	own_tree[live] = token
	local gone = live:GetDescendants()
	for _, inst in gone do
		own_tree[inst] = token
	end
	local ok, err = pcall(live.Destroy, live)
	if not ok then return fail(id, 8, tostring(err), live) end
	M.unmap(live)
	for _, inst in gone do
		M.unmap(inst)
	end
end

local function op_stamp()
	local id, hash = u32(), str()
	local root = M.by_id[id]
	if not root then
		if not unbound[id] then table.insert(M.failed, { id, 9, `id {id} is not mapped` }) end
	elseif M.role == "edit" then
		table.insert(stamps, { root = root, hash = hash })
	end
end

local function op_swap()
	local id = u32()
	local new, err = take()
	local old = M.by_id[id]
	if not old then
		return fail_unmapped(id, 10, id)
	elseif not new then
		return fail(id, 10, err :: string, old)
	end
	M.structural = true
	local ok, swap_err = pcall(function()
		own_tree[old] = token
		own_tree[new] = token
		local parent = old.Parent
		-- a swapped root stays a root
		local stamp = old:GetAttribute("opossum")
		if stamp ~= nil then new:SetAttribute("opossum", stamp) end
		old.Name ..= " (replaced)"
		for _, child in old:GetChildren() do
			child.Parent = new
		end
		new.Parent = parent
		map(id, new)
		old:Destroy()
	end)
	if not ok then fail(id, 10, tostring(swap_err), old) end
end

local function op_reload()
	for _ = 1, u32() do
		local id = u32()
		local script = M.by_id[id]
		if script then
			table.insert(reloads, script)
		else
			fail_unmapped(id, 11, id)
		end
	end
end

local OPS = table.freeze { op_bind, op_hydrate, op_children, op_props, op_source, op_move, op_ref, op_remove, op_stamp, op_swap, op_reload }

-- Writes the reload list for the game's dev runtime, then starts the swapped Scripts once its handlers have run.
local function reload(gen: number)
	local names: { string } = {}
	for i, script in reloads do
		names[i] = script:GetFullName()
	end
	local config = ReplicatedStorage:FindFirstChild("OpossumReload")
	if not config then
		local made = Instance.new("Configuration")
		made.Name = "OpossumReload"
		made.Parent = ReplicatedStorage
		config = made
	end
	(config :: Instance):SetAttribute("reload", `{gen}|{table.concat(names, ",")}`)
	local scripts = table.clone(reloads)
	task.defer(function()
		for _, script in scripts do
			(script :: Script).Enabled = true
		end
	end)
	table.clear(reloads)
end

-- In edit, opens the recording the next writes go under; nil (applied unrecorded) when another recording is open.
local function record(name: string): string?
	if M.role ~= "edit" then return nil end
	local id = ChangeHistoryService:TryBeginRecording(name)
	if not id then M.unrecorded = true end
	return id
end

local function write_stamps()
	for _, stamp in stamps do
		local root = stamp.root
		if not poisoned and not bad_roots[root] and not M.dirty[root] and root.Parent then root:SetAttribute("opossum", stamp.hash) end
	end
end

-- Ends a generation: joins its async writes, settles its strays and writes its stamps. Without async writes or
-- strays the stamps go into the frame's own recording, so a leaf edit is one waypoint.
local function finish(gen: number)
	if next(running) == nil and #strays == 0 then
		write_stamps()
		M.writing = false
		commit()
	else
		M.writing = false
		commit()
		join()
		-- async writes that landed during the join ride this waypoint, unless someone made one meanwhile
		local removing = authoritative and #strays > 0
		recording = record(if removing then RESYNC else SYNC)
		M.writing = true
		settle_strays(recording ~= nil)
		write_stamps()
		M.writing = false
		commit()
	end
	for root in held do
		if root.Parent == nil then root:Destroy() end
	end
	if #reloads > 0 then reload(gen) end
	local failed = M.failed
	M.failed = {}
	M.abort()
	M.failed = failed
	M.applied = gen
end

-- Applies one frame. "dropped": its base is not the last generation applied (answer with verify); "skipped": a
-- part of a generation already dropped (nothing to answer); "part": a leading part of a split generation;
-- "done": the generation is applied and `M.failed` holds its failures.
function M.frame(data: string): "dropped" | "skipped" | "part" | "done"
	buf = buffer.fromstring(data)
	pos = 0
	local gen, base, last = u32(), u32(), u8()
	if gen == skip then return "skipped" end
	if M.applied ~= nil and base ~= M.applied then
		gen_now = gen
		M.drop()
		return "dropped"
	end
	if not in_gen then
		in_gen = true
		token += 1
		gen_now = gen
	end
	recording = record(SYNC)
	M.writing = true
	local blob_len = u32()
	roots, next_root, blob_error = {}, 1, nil
	if blob_len > 0 then
		local blob = buffer.create(blob_len)
		buffer.copy(blob, 0, buf, pos, blob_len)
		pos += blob_len
		local ok, result = pcall(SerializationService.DeserializeInstancesAsync, SerializationService, blob)
		if ok then
			roots = result
		else
			blob_error = `the blob does not load: {result}`
		end
	end
	for _ = 1, u32() do
		local op = OPS[u8()]
		if not op then error("unknown op") end
		op()
	end
	for _, root in roots do
		if root.Parent == nil and not held[root] then root:Destroy() end
	end
	if last == 0 then
		M.writing = false
		commit()
		return "part"
	end
	finish(gen)
	return "done"
end

return M
