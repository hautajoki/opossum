--!strict
-- opossum's Studio plugin. It finds a running `opossum dev` on localhost whose place matches this one and connects
-- with no clicks, applies its instance patches (long-polling /patch, sources from /blobs) and forwards Output to it.
-- Only the edit DataModel syncs; a playtest's server and client copies of the plugin forward their output.

local HttpService = game:GetService("HttpService")
local LogService = game:GetService("LogService")
local RunService = game:GetService("RunService")
local ScriptEditorService = game:GetService("ScriptEditorService")

local FIRST_PORT = 34900
local PORTS = 16
-- `Source` assignment fails above this; longer sources go through ScriptEditorService.
local SOURCE_LIMIT = 199999
local BLOB_BATCH = 200

type Hello = { project: string, place: number, unpublished: boolean, roots: { string }, session: string }
type Op = { op: string, path: string, class: string, hash: string?, props: { [string]: any } }
type Patch = { session: string, generation: number, full: boolean, ops: { Op } }

local EDIT = RunService:IsEdit()
local REALM = if EDIT then "edit" elseif RunService:IsClient() then "client" else "server"

local enabled = plugin:GetSetting("opossumEnabled") ~= false
-- Comma-separated path or file prefixes (`features/doors`) to sync only part of the project; nil syncs it all.
local only: string? = plugin:GetSetting("opossumOnly")
local server: string? = nil
local roots: { string } = {}
-- What this plugin made, by path: the only instances it deletes outside the roots.
local created: { [string]: Instance } = {}
-- Source hash last written per path, so a reconnect does not refetch unchanged sources.
local hashes: { [string]: string } = {}

local function request(method: "GET" | "POST", url: string, body: string?): string?
	local ok, res = pcall(function()
		return HttpService:RequestAsync({
			Url = url,
			Method = method,
			Body = body,
		})
	end)
	return if ok and res.Success then res.Body else nil
end

local function decode(body: string?): any
	if not body then
		return nil
	end
	local ok, value = pcall(HttpService.JSONDecode, HttpService, body)
	return if ok then value else nil
end

local function discover(): (string?, Hello?)
	for port = FIRST_PORT, FIRST_PORT + PORTS - 1 do
		local url = `http://127.0.0.1:{port}`
		local info = decode(request("GET", url .. "/hello"))
		if
			type(info) == "table"
			and info.session
			and (info.place == game.PlaceId or (game.PlaceId == 0 and info.unpublished))
		then
			return url, info
		end
	end
	return nil, nil
end

local function split(path: string): (string?, string)
	local parent, name = string.match(path, "^(.*)/([^/]+)$")
	return parent, name or path
end

local function find(path: string): Instance?
	local names = string.split(path, "/")
	local ok, service = pcall(game.GetService, game, names[1])
	local inst: Instance? = if ok then service else nil
	for i = 2, #names do
		inst = inst and inst:FindFirstChild(names[i])
	end
	return inst
end

local function owned(path: string): boolean
	if created[path] then
		return true
	end
	for _, root in roots do
		if path == root or string.sub(path, 1, #root + 1) == root .. "/" then
			return true
		end
	end
	return false
end

local function forget(path: string)
	local prefix = path .. "/"
	local function drop(map: { [string]: any })
		for p in map do
			if p == path or string.sub(p, 1, #prefix) == prefix then
				map[p] = nil
			end
		end
	end
	drop(created)
	drop(hashes)
end

-- The instance at `path`, with missing parents made as Folders (the snapshot names them, but a scoped sync may not).
local function ensure(path: string): Instance?
	local inst = find(path)
	local parentPath, name = split(path)
	if inst or not parentPath then
		return inst
	end
	local parent = ensure(parentPath)
	if not parent then
		return nil
	end
	local folder = Instance.new("Folder")
	folder.Name = name
	folder.Parent = parent
	created[path] = folder
	return folder
end

local function set(op: Op, sources: { [string]: string })
	local parentPath, name = split(op.path)
	local parent = parentPath and ensure(parentPath)
	if not parent then
		error("no parent")
	end
	local old = parent:FindFirstChild(name)
	if old and old.ClassName ~= op.class and not owned(op.path) then
		error(`it is a {old.ClassName} opossum did not create`)
	end
	local inst: Instance
	if old and old.ClassName == op.class then
		inst = old
	else
		inst = Instance.new(op.class :: any)
		inst.Name = name
		if old then
			for _, child in old:GetChildren() do
				child.Parent = inst
			end
			old:Destroy()
		end
		inst.Parent = parent
		created[op.path] = inst
		hashes[op.path] = nil
	end
	for key, value in op.props do
		(inst :: any)[key] = if type(value) == "table" then (Enum :: any)[value.enum][value.item] else value
	end
	local hash = op.hash
	if hash and hashes[op.path] ~= hash then
		local source = sources[hash]
		if #source > SOURCE_LIMIT then
			ScriptEditorService:UpdateSourceAsync(inst :: LuaSourceContainer, function()
				return source
			end)
		else
			(inst :: any).Source = source
		end
		hashes[op.path] = hash
	end
end

local function del(path: string)
	if owned(path) then
		local inst = find(path)
		if inst then
			inst:Destroy()
		end
		forget(path)
	end
end

-- Sources the ops need that are not already in place, or nil when the server no longer has one of them.
local function fetch(url: string, ops: { Op }): { [string]: string }?
	local need, seen = {}, {}
	for _, op in ops do
		local hash = op.hash
		if hash and not seen[hash] then
			local inst = find(op.path)
			if hashes[op.path] ~= hash or not inst or inst.ClassName ~= op.class then
				seen[hash] = true
				table.insert(need, hash)
			end
		end
	end
	local sources = {}
	for i = 1, #need, BLOB_BATCH do
		local batch = table.move(need, i, math.min(i + BLOB_BATCH - 1, #need), 1, {})
		local got = decode(request("POST", url .. "/blobs", HttpService:JSONEncode(batch)))
		if type(got) ~= "table" then
			return nil
		end
		for hash, source in got do
			sources[hash] = source
		end
	end
	for _, hash in need do
		if not sources[hash] then
			return nil
		end
	end
	return sources
end

-- A full snapshot replaces everything opossum owns: what the roots hold and what this plugin made.
local function prune(ops: { Op })
	local keep: { [string]: true } = {}
	for _, op in ops do
		local path: string? = op.path
		while path and not keep[path] do
			keep[path] = true
			path = split(path)
		end
	end
	local function walk(inst: Instance, path: string)
		for _, child in inst:GetChildren() do
			local childPath = `{path}/{child.Name}`
			if keep[childPath] then
				walk(child, childPath)
			else
				child:Destroy()
			end
		end
	end
	for _, root in roots do
		local inst = find(root)
		if inst then
			walk(inst, root)
		end
	end
	for path, inst in created do
		if not keep[path] then
			inst:Destroy()
			forget(path)
		end
	end
end

local function apply(url: string, patch: Patch): boolean
	local sources = fetch(url, patch.ops)
	if not sources then
		return false
	end
	if patch.full and not only then
		prune(patch.ops)
	end
	for _, op in patch.ops do
		local ok, err = pcall(function()
			if op.op == "set" then
				set(op, sources)
			else
				del(op.path)
			end
		end)
		if not ok then
			warn(`opossum: {op.op} {op.path}: {err}`)
		end
	end
	return true
end

-- Applies patches until the server goes away or sync is turned off. During a playtest nothing is applied; the
-- server composes what arrives meanwhile into the next patch.
local function sync(url: string, session: string)
	local generation = 0
	while enabled and server == url do
		if RunService:IsRunning() then
			task.wait(0.5)
			continue
		end
		local query = `{url}/patch?since={generation}&session={session}`
		if only then
			query ..= "&only=" .. HttpService:UrlEncode(only)
		end
		local patch: Patch? = decode(request("GET", query))
		if not patch or not enabled then
			return
		end
		-- A patch that lands after a playtest started is left for the poll after it ends, which returns it again.
		if RunService:IsRunning() then
			continue
		end
		if apply(url, patch) then
			generation = patch.generation
			session = patch.session
		else
			task.wait(1)
		end
	end
end

local button: PluginToolbarButton? = nil
local function show(connected: boolean)
	if button then
		button:SetActive(enabled and connected)
	end
end

if EDIT then
	local b = plugin:CreateToolbar("opossum"):CreateButton("opossum", "Sync with opossum dev", "")
	b.ClickableWhenViewportHidden = true
	b.Click:Connect(function()
		enabled = not enabled
		plugin:SetSetting("opossumEnabled", enabled)
		print(`opossum: sync {if enabled then "on" else "off"}`)
		show(server ~= nil)
	end)
	button = b
end

local pending: { { kind: string, realm: string, text: string } } = {}
LogService.MessageOut:Connect(function(text, kind)
	if server and #pending < 1000 then
		table.insert(pending, { kind = kind.Name, realm = REALM, text = text })
	end
end)

task.spawn(function()
	while true do
		task.wait(0.5)
		local url = server
		if url and #pending > 0 then
			local batch = pending
			pending = {}
			if not request("POST", url .. "/log", HttpService:JSONEncode(batch)) and server == url then
				server = nil
			end
		end
	end
end)

while true do
	if enabled and not server then
		local url, info = discover()
		if url and info then
			server, roots = url, info.roots
			show(true)
			if EDIT then
				print(`opossum: syncing {info.project} from {url}`)
				sync(url, info.session)
				server = nil
				show(false)
				print("opossum: disconnected")
			end
		end
	end
	task.wait(3)
end
