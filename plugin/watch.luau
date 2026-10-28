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
		apply.left[root] = ""
		if root:GetAttribute("opossum") ~= "" then root:SetAttribute("opossum", "") end
	end
end

-- Roots something dirtied since the last waypoint anyone recorded or the last undo or redo: dirtied during a step,
-- they mean the step itself changed synced content (it fires the same signals), so the server verifies.
local stirred = setmetatable({}, { __mode = "k" }) :: { [Instance]: true }

local function stir(root: Instance?)
	if root then
		stirred[root] = true
		dirty(root)
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
			if not apply.owns(inst) then stir(root) end
		end),
		root.DescendantRemoving:Connect(function(inst)
			if not apply.owns(inst) then
				stir(root)
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

-- Watches the roots the plugin found since the last look and lets go of the ones that are no longer roots (edit
-- only); called after a generation that changed structure. The watched set is every root the plugin knows.
function M.refresh()
	if not edit then return end
	for _, root in apply.found do
		if not watched[root] and apply.root_of(root) == root then watch(root) end
	end
	table.clear(apply.found)
	for root in watched do
		if apply.root_of(root) ~= root then unwatch(root) end
	end
end

-- Watches every root in `roots` (a full apply.roots() scan, at start and on connect) and refreshes.
function M.scan(roots: { { path: string, root: Instance } })
	if not edit then return end
	for _, r in roots do
		table.insert(apply.found, r.root)
		local stamp = r.root:GetAttribute("opossum")
		apply.left[r.root] = if type(stamp) == "string" then stamp else nil
	end
	M.refresh()
end

-- Starts the DataModel-wide listeners for `role`; `sender` sends one upstream message.
function M.start(role: apply.Role, sender: (message: string) -> ())
	send = sender
	edit = role == "edit"
	if not edit then return end
	-- Fires synchronously for every property write, the plugin's own and the command bar's included (measured,
	-- ~0.3 µs an event), so apply.writing covers the plugin's synchronous writes and whatever they derive (ScaleTo,
	-- welded parts). Attribute and tag changes do not fire it. (Missing from the type definitions.)
	-- It fires for every part a drag moves, so it bails out early: once a frame mapped the place, an unmapped
	-- instance under a root is new (DescendantAdded dirtied its root), a kept stray (its root stays unstamped) or
	-- exempt, and a dirty root stays dirty (stepped() puts back a clear an undo reverted).
	((game :: any).ItemChanged :: RBXScriptSignal<Instance, string>):Connect(function(inst, prop)
		if apply.writing or (apply.applied ~= 0 and not apply.id_of[inst]) or not is_edit(inst, prop) then return end
		local root = apply.root_of(inst)
		if root and not apply.dirty[root] and not apply.owns_item(inst) then stir(root) end
	end)
	ScriptEditorService.TextDocumentDidChange:Connect(function(document)
		local script = document:GetScript()
		if script and not apply.owns_item(script) then stir(apply.root_of(script)) end
	end)
	ChangeHistoryService.OnRecordingFinished:Connect(function(name)
		table.clear(stirred)
		if string.sub(name, 1, 8) ~= "opossum:" then
			for _, inst in Selection:Get() do
				dirty(apply.root_of(inst))
			end
		end
	end)
	-- Each frame is its own waypoint, so undoing someone's edit reverts only theirs, and the signals above see it.
	-- A step may also revert the plugin's unrecorded clear of a dirty root's stamp (it rode that waypoint), so
	-- every dirty root is cleared again. Stepping over an opossum waypoint makes every root dirty: "opossum:
	-- resync" gives back the strays a verify took (kept until the next Resync), "opossum: sync" reverts synced
	-- content, so the server verifies. So it does after a step that changed a clean root (unrecorded writes rode
	-- that waypoint, however deep it was: the signals above saw it, or it put back another stamp than the plugin
	-- left), or any step while the last unrecorded writes have not been verified since. That verify is quiet: the
	-- frame answering it applies unrecorded, so the next Ctrl+Z goes on to the waypoint below instead of undoing
	-- the verify.
	local function stepped(name: string)
		local reverted = next(stirred) ~= nil
		table.clear(stirred)
		for root in watched do
			local left = apply.left[root]
			if left ~= nil and not apply.dirty[root] and root:GetAttribute("opossum") ~= left then
				reverted = true
				break
			end
		end
		for root in apply.dirty do
			dirty(root)
		end
		local sync = name == "opossum: sync" or apply.unrecorded or reverted
		if not sync and name ~= "opossum: resync" then return end
		for root in watched do
			local stamp = root:GetAttribute("opossum")
			if sync and type(stamp) == "string" and stamp ~= "" then apply.before[root] = stamp end
			dirty(root)
		end
		if sync then
			apply.unrecorded = false
			-- the verify pairs every root again, so what the step removed is never reported lost (and a lost the
			-- server took first would only be answered before the verify)
			table.clear(lost)
			send(apply.verify(true))
		end
	end
	ChangeHistoryService.OnUndo:Connect(stepped)
	ChangeHistoryService.OnRedo:Connect(stepped)
	M.scan(apply.roots())
end

return table.freeze(M)
