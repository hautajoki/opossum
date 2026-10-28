--!strict
-- opossum's Studio plugin (crates/sync/SYNC.md). In every DataModel it finds a running `opossum dev` on one
-- WebSocket and forwards Output to it, and runs the Luau `opossum exec` sends; the edit DataModel, and the play server
-- with hot reload, also apply its frames, one at a time, and ack them.

local HttpService = game:GetService("HttpService")
local LogService = game:GetService("LogService")
local RunService = game:GetService("RunService")

local apply = require("@self/apply")
local exec = require("@self/exec")
local watch = require("@self/watch")

local FIRST_PORT = 34900
local PORTS = 16
local HELLO_TIMEOUT = 2
local MAX_LINES = 1000

type Hello = { t: string, project: string, place: number, unpublished: boolean, session: string, hotReload: boolean }

local ROLE: apply.Role = if RunService:IsEdit() then "edit" elseif RunService:IsServer() then "server" else "client"
apply.role = ROLE

local socket: WebStreamClient? = nil
-- The live socket's signal connections: a client with connected handlers is never collected, even closed.
local links: { RBXScriptConnection } = {}
local project = ""
-- Bumped per socket, so an apply that outlives its socket never acks into the next one.
local epoch = 0
-- Frames and execs waiting for the worker, in order, each with the os.clock() it arrived at.
type Item = { data: string, at: number, exec: exec.Request? }
local queue: { Item } = {}
-- When the generation being applied got its first part, or the worker got to it after an exec it queued behind: its
-- ack carries the milliseconds since.
local started: number? = nil
-- When the worker last finished an exec.
local freed = 0
local busy = false
local status: PluginToolbarButton? = nil

local function send(message: string)
	if socket then pcall(socket.Send, socket, message) end
end

local function show()
	if status then status:SetActive(socket ~= nil) end
end

-- Runs queued frames and execs in order until the queue is empty or the socket changes.
local function drain(mine: number)
	while queue[1] and epoch == mine do
		local item = table.remove(queue, 1) :: Item
		local request = item.exec
		if request then
			local ok, result = pcall(exec.run, request)
			freed = os.clock()
			if epoch == mine then send(if ok then result else exec.failed(request, result)) end
			continue
		end
		local since = started or math.max(item.at, freed)
		local ok, result = pcall(apply.frame, item.data)
		if epoch ~= mine then break end
		started = if ok and result == "part" then since else nil
		if not ok then
			-- the rest of its generation is dropped too, so this verify is the generation's only answer
			warn(`opossum: a frame failed to apply: {result}`)
			apply.drop()
			send(apply.verify(apply.quiet))
		elseif result == "dropped" then
			send(apply.verify(apply.quiet))
		elseif result == "part" then
			task.wait()
		elseif result == "done" then
			local ms = (os.clock() - since) * 1000
			send(HttpService:JSONEncode { t = "ack", gen = apply.applied, failed = apply.failed, ms = ms })
			table.clear(apply.failed)
			if apply.structural then
				apply.structural = false
				watch.refresh()
			end
		end
	end
end

-- The single worker. Whatever throws, it stops being busy, so the next message starts it again.
local function work()
	busy = true
	local ok, problem = pcall(drain, epoch)
	busy = false
	if not ok then warn(`opossum: the worker stopped: {problem}`) end
end

local function hello(server: Hello): string
	local found = apply.roots()
	watch.scan(found)
	local roots = if ROLE == "edit" or (ROLE == "server" and server.hotReload) then apply.listing(found) else "{}"
	return `\{"t":"hello","role":"{ROLE}","place":{game.PlaceId},"roots":{roots}}`
end

local lines: { { string } } = {}
local KINDS = {
	[Enum.MessageType.MessageWarning] = "warning",
	[Enum.MessageType.MessageError] = "error",
}

local function flush()
	if #lines > 0 then
		local ok, body = pcall(HttpService.JSONEncode, HttpService, { t = "log", lines = lines })
		if ok then send(body) end
		lines = {}
	end
end

-- Closes `client` and disconnects its handlers, so it can be collected.
local function release(client: WebStreamClient, connections: { RBXScriptConnection })
	pcall(client.Close, client)
	for _, connection in connections do
		connection:Disconnect()
	end
end

local function drop(client: WebStreamClient)
	if socket == client then
		release(client, links)
		links = {}
		socket = nil
		epoch += 1
		table.clear(queue)
		started = nil
		show()
		print(`opossum: disconnected from {project}`)
	end
end

-- Tries one port: "live" when connected, "next" when nothing usable answers there, "busy" when no client can be
-- created now (Studio allows a few per DataModel and other plugins hold some).
local function connect(port: number): "live" | "next" | "busy"
	local options: { [string]: any } = { Url = `ws://127.0.0.1:{port}/` }
	local ok, client = pcall(HttpService.CreateWebStreamClient, HttpService, Enum.WebStreamClientType.WebSocket, options)
	if not ok then return "busy" end
	local state: "pending" | "hello" | "bad" | "live" = "pending"
	local closed = false
	local server: Hello? = nil
	local connections: { RBXScriptConnection } = {}
	connections[1] = client.MessageReceived:Connect(function(message)
		if state == "live" then
			if socket ~= client then return end
			local item: Item = { data = message, at = os.clock() }
			-- generation 0, which no frame has, leads a JSON message
			if string.sub(message, 1, 4) == "\0\0\0\0" then
				local ok, value = pcall(HttpService.JSONDecode, HttpService, string.sub(message, 5))
				if not ok or type(value) ~= "table" or value.t ~= "exec" then return end
				item.exec = value :: exec.Request
			end
			table.insert(queue, item)
			if not busy then task.spawn(work) end
		elseif state == "pending" then
			local decoded, value = pcall(HttpService.JSONDecode, HttpService, message)
			local info = if decoded and type(value) == "table" then value :: Hello else nil
			if info and info.t == "hello" and (info.place == game.PlaceId or (game.PlaceId == 0 and info.unpublished == true)) then
				server = info
				state = "hello"
			else
				state = "bad"
			end
		end
	end)
	local function gone()
		closed = true
		if state == "live" then
			drop(client)
		elseif state == "pending" then
			state = "bad"
		end
	end
	table.insert(connections, client.Error:Connect(gone))
	table.insert(connections, client.Closed:Connect(gone))
	local deadline = os.clock() + HELLO_TIMEOUT
	while state == "pending" and os.clock() < deadline do
		task.wait()
	end
	if state ~= "hello" or not server then
		release(client, connections)
		return "next"
	end
	epoch += 1
	table.clear(queue)
	started = nil
	-- an apply in flight ends within its join timeout
	while busy do
		task.wait()
	end
	if closed then
		release(client, connections)
		return "next"
	end
	apply.reset()
	socket = client
	links = connections
	project = server.project
	state = "live"
	send(hello(server))
	show()
	print(`opossum: syncing {project} ({ROLE}) from port {port}`)
	flush()
	return "live"
end

local function output(text: string, kind: Enum.MessageType)
	if (socket or ROLE ~= "edit") and #lines < MAX_LINES then
		table.insert(lines, { KINDS[kind] or "output", apply.clean(text) })
		if #lines == 1 and socket then task.defer(flush) end
	end
end

-- A playtest's scripts start before the plugin, so there the output so far, and anything printed before the socket
-- opens, waits for it (up to MAX_LINES).
if ROLE ~= "edit" then
	for _, entry in LogService:GetLogHistory() do
		output(entry.message, entry.messageType)
	end
end
LogService.MessageOut:Connect(output)

if ROLE == "edit" then
	local toolbar = plugin:CreateToolbar("opossum")
	local button = toolbar:CreateButton("opossum", "opossum dev: connection status", "")
	button.ClickableWhenViewportHidden = true
	button.Click:Connect(function()
		show()
		print(
			if socket
				then `opossum: syncing {project}, generation {apply.applied}`
				else `opossum: looking for opossum dev on ports {FIRST_PORT}-{FIRST_PORT + PORTS - 1}`
		)
	end)
	status = button
	local resync = toolbar:CreateButton("Resync", "Make every synced root exactly the files (Ctrl+Z gives back removals)", "")
	resync.ClickableWhenViewportHidden = true
	resync.Click:Connect(function()
		resync:SetActive(false)
		if socket then send(apply.verify()) end
	end)
end

watch.start(ROLE, send)

while true do
	if not socket then
		for port = FIRST_PORT, FIRST_PORT + PORTS - 1 do
			if connect(port) ~= "next" then break end
		end
	end
	task.wait(3)
end
