-- Runs the stockfix fixture's expected outputs (`expected/shared/<module>.luau`) with Lune and checks each call
-- against the value JavaScript gives for the TypeScript source (`runtime/<module>.luau`: the module's ambient
-- functions and its calls). Also checks that no module writes a global. Run from the fixture directory:
--     lune run runtime/check.luau
-- Prints `ok <calls>` and exits 0, or prints every failure and exits 1.
local fs = require("@lune/fs")
local luau = require("@lune/luau")
local process = require("@lune/process")

local modules = {}
local environments = {}
local ambients = {}

-- What the modules use of the runtime library: `TS.import` (below) and the try statement.
local TS = {
	TRY_RETURN = 1,
	TRY_BREAK = 2,
	TRY_CONTINUE = 3,
}

-- `TS.try` verbatim from roblox-ts's include/RuntimeLib.luau (rethrows; runs `finally` after a failed catch)
function TS.try(try, catch, finally)
	-- execute try
	local trySuccess, exitTypeOrTryError, returns = pcall(try)
	local exitType, tryError
	if trySuccess then
		exitType = exitTypeOrTryError
	else
		tryError = exitTypeOrTryError
	end

	local catchSuccess = true
	local catchError

	-- if try block failed, and catch block exists, execute catch
	if not trySuccess and catch then
		local newExitTypeOrCatchError, newReturns
		catchSuccess, newExitTypeOrCatchError, newReturns = pcall(catch, tryError)
		local newExitType
		if catchSuccess then
			newExitType = newExitTypeOrCatchError
		else
			catchError = newExitTypeOrCatchError
		end

		if newExitType then
			exitType, returns = newExitType, newReturns
		end
	end

	-- execute finally
	if finally then
		local newExitType, newReturns = finally()
		if newExitType then
			exitType, returns = newExitType, newReturns
		end
	end

	-- if exit type is a control flow, do not rethrow errors
	if exitType ~= TS.TRY_RETURN and exitType ~= TS.TRY_BREAK and exitType ~= TS.TRY_CONTINUE then
		-- if catch block threw an error, rethrow it
		if not catchSuccess then
			error(catchError, 2)
		end

		-- if try block threw an error and there was no catch block, rethrow it
		if not trySuccess and not catch then
			error(tryError, 2)
		end
	end

	return exitType, returns
end

local function loadModule(name: string)
	if modules[name] then
		return modules[name]
	end
	local source = fs.readFile(`expected/shared/{name}.luau`)
	source = source:gsub("local TS = require%b()\n", "local TS = _TS\n")
	local environment = setmetatable({}, { __index = _G })
	environment._TS = setmetatable({
		import = function(_, _, ...)
			local path = { ... }
			return loadModule(path[#path])
		end,
	}, { __index = TS })
	environment.script = {}
	environment.game = {
		GetService = function()
			return {}
		end,
	}
	for key, value in ambients[name] or {} do
		environment[key] = value
	end
	local chunk = luau.load(source, { debugName = name, environment = environment })
	environments[name] = { environment = getfenv(chunk), known = table.clone(getfenv(chunk)) }
	modules[name] = chunk()
	return modules[name]
end

local function show(value: any): string
	if type(value) == "string" then
		return string.format("%q", value)
	elseif type(value) ~= "table" then
		return tostring(value)
	end
	local parts = {}
	for k, v in value do
		table.insert(parts, `[{show(k)}] = {show(v)}`)
	end
	table.sort(parts)
	return "{ " .. table.concat(parts, ", ") .. " }"
end

local function equal(a: any, b: any): boolean
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	for k, v in a do
		if not equal(v, b[k]) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end

local failures = {}
local count = 0
local files = fs.readDir("runtime")
table.sort(files)
local suites = {}
for _, file in files do
	if file ~= "check.luau" and file:match("%.luau$") then
		local suite = luau.load(fs.readFile("runtime/" .. file), { debugName = file })()
		suites[file:gsub("%.luau$", "")] = suite
		ambients[file:gsub("%.luau$", "")] = suite.ambient
	end
end
local names = {}
for name in suites do
	table.insert(names, name)
end
table.sort(names)
for _, name in names do
	for _, case in suites[name].calls do
		count += 1
		local label = `{name}.{case.call}{case.label or "()"}`
		local ok, result = pcall(function()
			return loadModule(name)[case.call](table.unpack(case.args or {}, 1, case.argCount or #(case.args or {})))
		end)
		if not ok then
			table.insert(failures, `{label}: error {result}`)
		elseif type(case.expect) == "function" then
			if not case.expect(result) then
				table.insert(failures, `{label}: unexpected {show(result)}`)
			end
		elseif not equal(result, case.expect) then
			table.insert(failures, `{label}: got {show(result)}, JavaScript gives {show(case.expect)}`)
		end
	end
end
for name, env in environments do
	for key in env.environment do
		if env.known[key] == nil then
			table.insert(failures, `{name}: wrote the global {key}`)
		end
	end
end
if #failures > 0 then
	print(table.concat(failures, "\n"))
	process.exit(1)
end
print(`ok {count}`)
