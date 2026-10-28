--!strict
-- Notices what changes synced instances besides the plugin, in the edit DataModel only: it clears the stamp of the
-- root that changed (marking it dirty until a verify), and unmaps mapped instances someone else removed and
-- reports them as lost. The play server never does either: game code changes synced instances at runtime. The
-- plugin's own writes are recognised by identity (apply.writing, apply.owns), never by timing.

local ChangeHistoryService = game:GetService("ChangeHistoryService")
local ReflectionService = game:GetService("ReflectionService")
local ScriptEditorService = game:GetService("ScriptEditorService")
local Selection = game:GetService("Selection")

local apply = require("./apply")

local watched: { [Instance]: { RBXScriptConnection } } = {}
local lost: { number } = {}
local send: (message: string) -> () = function() end
local edit = false

local M = {}

-- Per class, whether a property can be written by a plugin. Only those changes are edits: the rest are derived
-- by the engine (Mass, Assembly*, AbsoluteSize, TimeLength, ...). A property reflection does not know counts.
local writable: { [string]: { [string]: boolean } } = {}

local function is_edit(inst: Instance, prop: string): boolean
	local class = inst.ClassName
	local props = writable[class]
	if not props then
		local known: { [string]: boolean } = {}
		local ok, list = pcall(function()
			return ReflectionService:GetPropertiesOfClass(class)
		end)
		if ok and list then
			for _, p in list do
				known[p.Name] = p.Permits.Write ~= nil
			end
		end
		writable[class] = known
		props = known
	end
	return props[prop] ~= false
end

local function dirty(root: Instance?)
	if root then
		apply.dirty[root] = true
		if root:GetAttribute("opossum") ~= "" then root:SetAttribute("opossum", "") end
	end
end

local function flush()
	if #lost > 0 then
		send(`\{"t":"lost","ids":[{table.concat(lost, ",")}]}`)
		table.clear(lost)
	end
end

local function lose(inst: Instance)
	local id = apply.id_of[inst]
	if id then
		apply.unmap(inst)
		table.insert(lost, id)
		if #lost == 1 then task.defer(flush) end
	end
end

local function unwatch(root: Instance)
	for _, connection in watched[root] do
		connection:Disconnect()
	end
	watched[root] = nil
end

local function watch(root: Instance)
	watched[root] = {
		root.DescendantAdded:Connect(function(inst)
			if not apply.owns(inst) then dirty(root) end
		end),
		root.DescendantRemoving:Connect(function(inst)
			if not apply.owns(inst) then
				dirty(root)
				lose(inst)
			end
		end),
		root.AncestryChanged:Connect(function()
			if apply.owns(root) or apply.root_of(root) == root then return end
			-- Moved or deleted by someone else: it and everything mapped in it are gone from where the tree has them.
			lose(root)
			for _, inst in root:GetDescendants() do
				lose(inst)
			end
			for inst in apply.id_of do
				if not inst:IsDescendantOf(game) then lose(inst) end
			end
			unwatch(root)
		end),
	}
end

-- Watches exactly the current roots (edit only). Called at start, after connecting and after a generation that
-- changed structure.
function M.refresh()
	if not edit then return end
	local now: { [Instance]: true } = {}
	for _, root in apply.roots() do
		now[root] = true
		if not watched[root] then watch(root) end
	end
	for root in watched do
		if not now[root] then unwatch(root) end
	end
end

-- Starts the DataModel-wide listeners for `role`; `sender` sends one upstream message.
function M.start(role: apply.Role, sender: (message: string) -> ())
	send = sender
	edit = role == "edit"
	if not edit then return end
	-- Fires synchronously for every property write, the plugin's own and the command bar's included (measured,
	-- ~0.3 µs an event), so apply.writing covers the plugin's synchronous writes and whatever they derive (ScaleTo,
	-- welded parts). Attribute and tag changes do not fire it. (Missing from the type definitions.)
	((game :: any).ItemChanged :: RBXScriptSignal<Instance, string>):Connect(function(inst, prop)
		if apply.writing or not is_edit(inst, prop) then return end
		local root = apply.root_of(inst)
		if root and not apply.owns_item(inst) then dirty(root) end
	end)
	ScriptEditorService.TextDocumentDidChange:Connect(function(document)
		local script = document:GetScript()
		if script and not apply.owns_item(script) then dirty(apply.root_of(script)) end
	end)
	ChangeHistoryService.OnRecordingFinished:Connect(function(name)
		if string.sub(name, 1, 8) ~= "opossum:" then
			for _, inst in Selection:Get() do
				dirty(apply.root_of(inst))
			end
		end
	end)
	-- Each frame is its own waypoint, so undoing someone's edit reverts only theirs, and the signals above see it.
	-- Stepping over an opossum waypoint makes every root dirty: "opossum: resync" gives back the strays a verify
	-- took (kept until the next Resync), "opossum: sync" reverts synced content, so the server verifies, as it
	-- does after any step while a frame went unrecorded and rode someone else's waypoint.
	local function stepped(name: string)
		local sync = name == "opossum: sync" or apply.unrecorded
		if not sync and name ~= "opossum: resync" then return end
		for _, root in apply.roots() do
			dirty(root)
		end
		if sync then
			apply.unrecorded = false
			send(`\{"t":"verify"}`)
		end
	end
	ChangeHistoryService.OnUndo:Connect(stepped)
	ChangeHistoryService.OnRedo:Connect(stepped)
	M.refresh()
end

return table.freeze(M)
