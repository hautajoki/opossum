--!strict
-- Runs what `opossum exec` and `opossum test` send (crates/sync/SYNC.md, "Exec"): Luau as a fresh ModuleScript chunk
-- whose print and warn are its own, under a time limit, leaving nothing in the DataModel.
--
-- Studio's require caches a result per instance for the life of this VM: a Source change, rename, reparent or
-- UpdateSourceAsync leaves it returning the first result (measured), and sync writes Source in place. So each exec
-- loads every ModuleScript it requires afresh: a nil-parented copy of its current Source, placed in a mirror of its
-- ancestors' names so chunk names and tracebacks read like the original's, its first line given `script` (the
-- original) and a `require` that does the same. Nothing is cached between execs, module state included.

local ChangeHistoryService = game:GetService("ChangeHistoryService")
local HttpService = game:GetService("HttpService")
local ScriptContext = game:GetService("ScriptContext")
local ScriptEditorService = game:GetService("ScriptEditorService")

local apply = require("./apply")

-- `.Source` refuses sources this long; UpdateSourceAsync takes them.
local SOURCE_LIMIT = 200000
-- Bytes of output, of returned values, and of an error and its trace an answer carries, each: the rest is cut, so a
-- result stays far below what the socket takes in one message.
local ANSWER_LIMIT = 1000000
-- Traceback lines from the plugin itself, below the exec's own frames.
local OWN = script.Parent and script.Parent:GetFullName() or "opossum"
local LOADING = table.freeze {}

export type Request = {
	id: number,
	name: string,
	source: string,
	modules: { [string]: string }?,
	timeout: number?,
	-- seconds it may wait in the plugin's queue before it starts; past that dev no longer waits, so it never runs
	start: number?,
}

-- The copy loading now, by token: the original it stands for and the require it gets. Read by the copy's first line.
local pending: { [number]: { any } } = {}
local token = 0
_G.__opossum_exec = function(n: number): (Instance, (any) -> any)
	local entry = pending[n]
	pending[n] = nil
	return entry[1], entry[2]
end

-- The byte where the first token that is not a comment starts, or nil: directives (--!native) stay in front of it.
local function first_code(source: string): number?
	local at = 1
	while true do
		local s = string.find(source, "%S", at)
		if not s then return nil end
		if string.sub(source, s, s + 1) ~= "--" then return s end
		local level = string.match(source, "^%[(=*)%[", s + 2)
		if level then
			local close = string.find(source, `]{level}]`, s + 4 + #level, true)
			if not close then return nil end
			at = close + #level + 2
		else
			local eol = string.find(source, "\n", s, true)
			if not eol then return nil end
			at = eol + 1
		end
	end
end

-- `path` (a string require: @self/, @game/, ./ or ../, then names) from the module `from`.
local function resolve(from: Instance, path: string): Instance
	local at: Instance?, rest: string
	if string.sub(path, 1, 5) == "@self" then
		at, rest = from, string.sub(path, 6)
	elseif string.sub(path, 1, 5) == "@game" then
		at, rest = game, string.sub(path, 6)
	elseif string.sub(path, 1, 2) == "./" then
		at, rest = from.Parent, string.sub(path, 3)
	elseif string.sub(path, 1, 3) == "../" then
		at, rest = from.Parent and from.Parent.Parent, string.sub(path, 4)
	else
		error(`unsupported require path {path} from {from:GetFullName()}`, 3)
	end
	for name in string.gmatch(rest, "[^/]+") do
		if not at then break end
		if name == ".." then
			at = at.Parent
		elseif name ~= "." then
			at = at:FindFirstChild(name)
		end
	end
	if not at then error(`could not find {path} from {from:GetFullName()}`, 3) end
	return at
end

type Session = { cache: { [Instance]: any }, folders: { [string]: Instance }, tops: { Instance } }

local function set_source(module: ModuleScript, source: string)
	if #source < SOURCE_LIMIT then
		module.Source = source
	else
		ScriptEditorService:UpdateSourceAsync(module, function()
			return source
		end)
	end
end

-- tostring that cannot throw: a value whose __tostring errors is shown by its type.
local function describe(value: unknown): string
	local ok, text = pcall(tostring :: (any) -> string, value)
	return apply.clean(if ok and type(text) == "string" then text else typeof(value))
end

-- At most `room` bytes of `text`, saying how much was cut.
local function cut(text: string, room: number): string
	if #text <= room then return text end
	return `{apply.clean(string.sub(text, 1, math.max(room, 0)))}... ({#text - math.max(room, 0)} more bytes cut)`
end

local load: (session: Session, original: ModuleScript) -> any

local function bound(session: Session, from: Instance): (any) -> any
	return function(target: any): any
		if type(target) == "string" then target = resolve(from, target) end
		if typeof(target) == "Instance" and target:IsA("ModuleScript") then return load(session, target) end
		-- an asset id, or whatever require makes of it
		return (require :: any)(target)
	end
end

-- The mirror folder a copy of `original` goes in, made on first use; nil for a module without a parent.
local function folder_for(session: Session, original: Instance): Instance?
	local names: { string } = {}
	local at = original.Parent
	while at and at ~= game do
		table.insert(names, 1, at.Name)
		at = at.Parent
	end
	local parent: Instance? = nil
	for i, name in names do
		local key = table.concat(names, "\0", 1, i)
		local folder = session.folders[key]
		if not folder then
			local made = Instance.new("Folder")
			made.Name = name
			made.Parent = parent
			if not parent then table.insert(session.tops, made) end
			session.folders[key] = made
			folder = made
		end
		parent = folder
	end
	return parent
end

load = function(session: Session, original: ModuleScript): any
	local cached = session.cache[original]
	if cached == LOADING then error(`{original:GetFullName()} was required recursively`, 3) end
	if cached ~= nil then return cached end
	session.cache[original] = LOADING
	token += 1
	pending[token] = { original, bound(session, original) }
	local source = original.Source
	local line = `local script, require = _G.__opossum_exec({token}); `
	local at = first_code(source)
	source = if at then string.sub(source, 1, at - 1) .. line .. string.sub(source, at) else source .. "\n" .. line
	local copy = Instance.new("ModuleScript")
	copy.Name = original.Name
	set_source(copy, source)
	local parent = folder_for(session, original)
	if parent then
		copy.Parent = parent
	else
		table.insert(session.tops, copy)
	end
	-- a module that throws makes require say only "Requested module experienced an error while loading" (measured):
	-- ScriptContext.Error has a runtime error's own message, synchronously, and a syntax error compiles again to say it
	local failure: string? = nil
	local listen = ScriptContext.Error:Connect(function(message, stack, at)
		if at ~= copy then return end
		-- an engine error ("x is not a valid member of y") names no place; its stack does, as `Path, line 3`
		local path, n = string.match(stack, "^([^\n]-), line (%d+)")
		local placed = string.match(message, "^[%w_%.%-@]+:%d+: ") ~= nil
		failure = if path and not placed then `{path}:{n}: {message}` else message
	end)
	local ok, result = pcall(require, copy)
	listen:Disconnect()
	pending[token] = nil
	if not ok then
		session.cache[original] = nil
		if not failure and loadstring then
			local _, syntax = loadstring(source, `={copy:GetFullName()}`)
			failure = syntax
		end
		error(failure or result, 0)
	end
	session.cache[original] = result
	return result
end

-- A copy of `value` JSONEncode keeps whole: every leaf JSON has no word for (an Instance, a Vector3, a function, nan)
-- as its tostring, where JSONEncode would make it null. False for a table with a cycle, or with keys that are neither
-- all strings nor all array indices.
local function plain(value: unknown, seen: { [any]: true }): (boolean, unknown)
	local kind = type(value)
	if kind == "string" then return true, apply.clean(value :: string) end
	if kind == "boolean" then return true, value end
	if kind == "number" and value == value and math.abs(value :: number) ~= math.huge then return true, value end
	if kind ~= "table" then return true, describe(value) end
	local t = value :: { [any]: any }
	if seen[t] then return false, nil end
	seen[t] = true
	local n, strings, numbers = #t, false, false
	local copy: { [any]: any } = {}
	for k, v in next, t do
		if type(k) == "string" then
			strings = true
		elseif type(k) == "number" and k >= 1 and k <= n and k % 1 == 0 then
			numbers = true
		else
			return false, nil
		end
		local ok, inner = plain(v, seen)
		if not ok or (strings and numbers) then return false, nil end
		copy[if type(k) == "string" then apply.clean(k) else k] = inner
	end
	seen[t] = nil
	return true, copy
end

-- One returned value as JSON, at most `room` bytes of it (a value cut short becomes a string).
local function encode(value: unknown, room: number): string
	if value == nil then return "null" end
	local ok, copy = plain(value, {})
	local encoded, text = false, ""
	if ok then
		encoded, text = pcall(HttpService.JSONEncode, HttpService, copy)
	end
	if not encoded then return HttpService:JSONEncode(cut(describe(value), room)) end
	return if #text <= room then text else HttpService:JSONEncode(cut(text, room))
end

-- `trace` without the plugin's own frames.
local function trim(trace: string): string
	local lines: { string } = {}
	for line in string.gmatch(trace, "[^\n]+") do
		if string.sub(line, 1, #OWN) ~= OWN then table.insert(lines, line) end
	end
	return table.concat(lines, "\n")
end

-- The `result` message for an exec that did not run to an answer: `late` when it never started.
local function failed(request: Request, problem: unknown, late: boolean?): string
	local text = cut(describe(problem), ANSWER_LIMIT)
	return HttpService:JSONEncode { t = "result", id = request.id, ok = false, late = late == true, error = text, values = {} }
end

-- Runs one exec that arrived at os.clock() `at` and returns the upstream `result` message. Never throws.
local function run(request: Request, at: number): string
	local started = os.clock()
	if request.start and started - at > request.start then
		return failed(request, `waited {math.floor(started - at)} s behind earlier work in Studio and was not run`, true)
	end
	local output: { { string } } = {}
	local room, dropped = ANSWER_LIMIT, 0
	local function capture(kind: string): (...any) -> ()
		return function(...)
			if room <= 0 then
				dropped += 1
				return
			end
			local parts = table.pack(...)
			for i = 1, parts.n do
				parts[i] = tostring(parts[i])
			end
			local text = cut(apply.clean(table.concat(parts :: { any }, " ", 1, parts.n)), room)
			room -= #text
			table.insert(output, { kind, text })
		end
	end
	local chunk: ModuleScript? = nil
	local session: Session = { cache = {}, folders = {}, tops = {} }
	-- in edit, what the script writes is one waypoint of its own, so Ctrl+Z takes it back
	local recording = if apply.role == "edit" then ChangeHistoryService:TryBeginRecording("opossum: exec") else nil
	local result: { n: number, [number]: any }? = nil
	local thread = task.spawn(function()
		result = table.pack(xpcall(function()
			local made = Instance.new("ModuleScript")
			chunk = made
			made.Name = request.name
			-- spliced in front of the first code, on its line, so directives stay first and lines keep their numbers;
			-- print and warn are this exec's own
			local source = request.source
			local code = first_code(source)
			local open = "return function(print, warn, ...) "
			set_source(
				made,
				if code then `{string.sub(source, 1, code - 1)}{open}{string.sub(source, code)}\nend` else `{source}\n{open}\nend`
			)
			for name, text in request.modules or {} do
				local module = Instance.new("ModuleScript")
				module.Name = name
				set_source(module, text)
				module.Parent = made
			end
			return load(session, made)(capture("output"), capture("warning"))
		end, function(e: unknown)
			return { message = if type(e) == "string" then e else describe(e), trace = trim(debug.traceback(nil, 2)) }
		end))
	end)
	local deadline = started + (request.timeout or 60)
	while not result and os.clock() < deadline do
		task.wait()
	end
	if not result then pcall(task.cancel, thread) end
	if recording then ChangeHistoryService:FinishRecording(recording, Enum.FinishRecordingOperation.Commit) end
	if chunk then chunk:Destroy() end
	for _, top in session.tops do
		top:Destroy()
	end
	if dropped > 0 then table.insert(output, { "warning", `... {dropped} more lines cut` }) end
	local message = { t = "result", id = request.id, ok = false, output = output, ms = (os.clock() - started) * 1000 } :: { [string]: any }
	local values: { string } = {}
	if not result then
		message.error = `timed out after {request.timeout or 60} s`
	elseif result[1] then
		message.ok = true
		room = ANSWER_LIMIT
		for i = 2, result.n do
			local text = encode(result[i], room)
			room -= #text
			values[i - 1] = text
		end
	else
		-- a handler that threw anyway leaves a string ("error in error handling")
		local failure = result[2]
		local text, trace = describe(failure), ""
		if type(failure) == "table" then
			text, trace = failure.message, failure.trace
		end
		message.error = cut(apply.clean(text), ANSWER_LIMIT)
		message.trace = cut(apply.clean(trace), ANSWER_LIMIT)
	end
	-- values may hold nils, which a Luau array cannot: spliced in as JSON
	local body = HttpService:JSONEncode(message)
	return `{string.sub(body, 1, -2)},"values":[{table.concat(values, ",")}]}`
end

return table.freeze { run = run, failed = failed }
