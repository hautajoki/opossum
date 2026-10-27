-- Runs the seamrest fixture's expected outputs (`expected/shared/<module>.luau`, the probe plugins' claims in place)
-- with Lune, and checks what each call returns and what it ran, in order, against the program the claims make: a
-- claimed optional call or chain link still short-circuits (nothing of it runs when the chain stops), receivers and
-- bases are evaluated once, and claimed `super` calls keep `self`. Run from the fixture directory:
--     lune run runtime/check.luau
-- Prints `ok <cases>` and exits 0, or prints every failure and exits 1.
local fs = require("@lune/fs")
local luau = require("@lune/luau")
local process = require("@lune/process")

-- what the ambient functions ran, in order
local log: { string } = {}
local function record(...: any)
	local parts = {}
	for i = 1, select("#", ...) do
		table.insert(parts, tostring((select(i, ...))))
	end
	table.insert(log, table.concat(parts, " "))
end

-- the state the ambient getters return, set per case
local state: { [string]: any } = {}
local counter = 0

local function method(name: string, result: (any, number) -> any)
	return function(self: any, value: number)
		record(name, value)
		return result(self, value)
	end
end

local ambient: { [string]: any } = {}
ambient.next = function()
	counter += 1
	record("next")
	return counter
end
for _, getter in { "getMarked", "getMaybeMethod", "getMaybeMaybeMethod", "getHolder", "getA", "getA2", "getExprs", "getAskOnce" } do
	ambient[getter] = function()
		record(getter)
		return state[getter]
	end
end
ambient.useRecv = function(receiver: any, value: number)
	record("useRecv", receiver == state.getHolder, value)
	return value + 6
end
ambient.wrap = function(value: any)
	record("wrap")
	return value
end
ambient.exprWrap = function(value: any)
	record("exprWrap")
	return value
end
ambient.takenValue = { c = "taken" }
ambient.probeAskOnce = function(value: number)
	record("probeAskOnce", value)
	return value * 2
end
ambient.probeAskOnceNew = {
	new = function(value: number)
		record("new", value)
		return { made = value }
	end,
}
ambient.probeElseIf = true
ambient.print = function(...: any)
	record("print", ...)
end
ambient.createElement = function(tag: any)
	return { tag = tag }
end
ambient.tags = { probeAccTag = function() end, plainTag = function() end }
ambient.keyedTags = { k_Comp = function() end }
-- rewritten keys: `k_a` is a field, any other key read is logged
ambient.dropKeyed = setmetatable({ k_a = 1 }, {
	__index = function(_, key)
		record("index", key)
		return "a"
	end,
})
ambient.getDropKeyed = function()
	record("getDropKeyed")
	return ambient.dropKeyed
end
ambient.keyName = function()
	record("keyName")
	return "a"
end
-- a roblox-ts class as `SeamBase` is declared, with the keys `property_key` gives its members (`k_<key>`)
local SeamBase = {}
SeamBase.__index = SeamBase
function SeamBase.constructor(self: any, value: number)
	self.value = value
end
function SeamBase.k_m(self: any, v: number)
	record("k_m", self.value, v)
	return self.value + v
end
function SeamBase.k_n(self: any, v: number, extra: string, more: any)
	record("k_n", self.value, v, extra, more)
	return self.value * v
end
function SeamBase.k_probeKeyed(self: any)
	record("k_probeKeyed", self.value)
	return self.value * 10
end
ambient.SeamBase = SeamBase

local modules = {}
local environments = {}
local function loadModule(name: string)
	if modules[name] then
		return modules[name]
	end
	local environment = setmetatable(table.clone(ambient), { __index = _G })
	local chunk = luau.load(fs.readFile(`expected/shared/{name}.luau`), { debugName = name, environment = environment })
	environments[name] = { environment = getfenv(chunk), known = table.clone(getfenv(chunk)) }
	modules[name] = chunk()
	return modules[name]
end

local function marked()
	return { probeMethod = function()
		error("the claimed call must not call probeMethod")
	end, probed = method("probed", function(_, v)
		return v * 10
	end) }
end
local function maybeMethod(present: boolean)
	local object = marked()
	if not present then
		object.probeMethod = nil
	end
	return object
end
local function holder(fn: boolean, sf: boolean)
	return {
		fn = fn and function()
			error("the claimed call must not call fn")
		end or nil,
		sf = sf and function(value: number)
			record("sf", value)
			return value + 4
		end or nil,
	}
end
local accA = { probeAccB = { c = 3 }, probeAccTakeB = { c = 4 } }
accA.plain = accA

-- module, call, arguments, state, expected result, expected log
local cases = {
	-- optional method calls: the receiver once, the claimed call only when nothing is nil
	{ "optional", "receiverOptional", {}, { getMarked = nil }, nil, { "getMarked" } },
	{ "optional", "receiverOptional", {}, { getMarked = marked() }, 10, { "getMarked", "next", "probed 1" } },
	{ "optional", "methodOptional", {}, { getMaybeMethod = maybeMethod(false) }, nil, { "getMaybeMethod" } },
	{ "optional", "methodOptional", {}, { getMaybeMethod = maybeMethod(true) }, 10, { "getMaybeMethod", "next", "probed 1" } },
	{ "optional", "bothOptional", {}, { getMaybeMaybeMethod = nil }, nil, { "getMaybeMaybeMethod" } },
	{ "optional", "bothOptional", {}, { getMaybeMaybeMethod = maybeMethod(false) }, nil, { "getMaybeMaybeMethod" } },
	{ "optional", "bothOptional", {}, { getMaybeMaybeMethod = maybeMethod(true) }, 10, { "getMaybeMaybeMethod", "next", "probed 1" } },
	-- non-method optional calls: the receiver bound once and passed to the claimed call
	{ "optional", "fnOptional", {}, { getHolder = nil }, nil, { "getHolder" } },
	{ "optional", "fnOptional", {}, { getHolder = holder(false, false) }, nil, { "getHolder" } },
	{ "optional", "fnOptional", {}, { getHolder = holder(true, false) }, 7, { "getHolder", "next", "useRecv true 1" } },
	{ "optional", "selfOptional", {}, { getHolder = holder(false, false) }, nil, { "getHolder" } },
	{ "optional", "selfOptional", {}, { getHolder = holder(false, true) }, 5, { "getHolder", "next", "sf 1", "wrap" } },
	{ "optional", "statementOptional", {}, { getMarked = nil }, nil, { "getMarked" } },
	{ "optional", "statementOptional", {}, { getMarked = marked() }, nil, { "getMarked", "next", "probed 1" } },
	-- links inside chains: before the first `?.` (lazy), or inside the nil check on their base (eager)
	{ "chains", "lazyLink", {}, { getA2 = accA }, 3, { "getA2", "exprWrap" } },
	{ "chains", "lazyLink", {}, { getA2 = {} }, nil, { "getA2", "exprWrap" } },
	{ "chains", "eagerLink", {}, { getA = nil }, nil, { "getA" } },
	{ "chains", "eagerLink", {}, { getA = accA }, 3, { "getA", "exprWrap" } },
	{ "chains", "takenLink", {}, { getA = nil }, nil, { "getA" } },
	{ "chains", "takenLink", {}, { getA = accA }, "taken", { "getA" } },
	{ "chains", "elementLink", {}, { getA = nil }, nil, { "getA" } },
	{ "chains", "elementLink", {}, { getA = accA }, 3, { "getA", "exprWrap" } },
	{ "chains", "deepLink", {}, { getA = { probeAccB = { c = 3 } } }, nil, { "getA" } },
	{ "chains", "deepLink", {}, { getA = accA }, 3, { "getA", "exprWrap" } },
	{ "chains", "callInChain", {}, { getExprs = nil }, nil, { "getExprs" } },
	{ "chains", "callInChain", {}, { getExprs = { probeExprM = method("probeExprM", function(_, v)
		return { value = v + 100 }
	end) } }, 101, { "getExprs", "next", "probeExprM 1", "exprWrap" } },
	-- asked once: the claimed calls run once
	{ "askonce", "plain", {}, {}, 2, { "next", "probeAskOnce 1", "wrap" } },
	{ "askonce", "inChain", {}, { getAskOnce = nil }, nil, { "getAskOnce" } },
	{ "askonce", "inChain", {}, { getAskOnce = { probeAskOnceM = method("probeAskOnceM", function(_, v)
		return v * 3
	end) } }, 3, { "getAskOnce", "next", "probeAskOnceM 1", "wrap" } },
	{ "askonce", "construct", {}, {}, { made = 1 }, { "next", "new 1", "wrap" } },
	-- statements: a claimed block, `else if`, `case` block and static block run the claim, not what they held
	{ "statements", "blockBody", { true }, {}, 2, { "print probe: a block of 2 statements" } },
	{ "statements", "blockBody", { false }, {}, 2, {} },
	{ "statements", "elseIf", { true }, {}, "a", {} },
	{ "statements", "elseIf", { false }, {}, "c", { "print probe: an if statement" } },
	{ "statements", "loopBody", {}, {}, 0, { "print probe: a block of 2 statements", "print probe: a block of 2 statements" } },
	{ "statements", "caseBody", { 1 }, {}, "other", { "print probe: a block of 2 statements" } },
	{ "statements", "caseBody", { 2 }, {}, "other", {} },
	-- rewritten keys: an enum member argument is dropped, a call or a read still runs, after the receiver
	{ "keys", "enumKey", {}, {}, 1, {} },
	{ "keys", "callKey", {}, {}, 1, { "getDropKeyed", "keyName" } },
	{ "keys", "readKey", {}, {}, 1, { "index k_s" } },
	{ "keys", "writeKey", {}, {}, 2, { "getDropKeyed" } },
	-- dotted JSX tags
	{ "tags", "dotted", {}, {}, { tag = ambient.tags.probeAccTag }, { "exprWrap" } },
	{ "tags", "keyedTag", {}, {}, { tag = ambient.keyedTags.k_Comp }, {} },
	{ "tags", "plainTag", {}, {}, { tag = ambient.tags.plainTag }, {} },
}

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
local function check(label: string, run: () -> any, expected: any, expectedLog: { string })
	count += 1
	table.clear(log)
	counter = 0
	local ok, result = pcall(run)
	if not ok then
		table.insert(failures, `{label}: error {result}`)
	elseif not equal(result, expected) then
		table.insert(failures, `{label}: got {show(result)}, expected {show(expected)}`)
	elseif not equal(log, expectedLog) then
		table.insert(failures, `{label}: ran {show(log)}, expected {show(expectedLog)}`)
	end
end

-- loading a module runs its top level: the claimed `declare` statement and the claimed static block
check("statements (loaded)", function()
	return loadModule("statements").WithStatic.value
end, 1, { "print probe: a declare statement", "print probe: a block of 2 statements" })

for index, case in cases do
	local name, call, args, caseState, expected, expectedLog = table.unpack(case, 1, 6)
	state = caseState
	check(`case {index}: {name}.{call}({show(args)})`, function()
		return loadModule(name)[call](table.unpack(args))
	end, expected, expectedLog)
end

-- `super` calls: `self` is the instance, through the claim (`wrap`, injected arguments) and the rewritten keys
state = {}
local function subclass()
	return loadModule("supercalls").Sub.new(5)
end
check("supercalls: Sub.new(5).value", function()
	return subclass().value
end, 5, {})
check("supercalls: m(2)", function()
	return subclass():m(2)
end, 7, { "k_m 5 2", "wrap" })
check("supercalls: n(3)", function()
	return subclass():n(3)
end, 15, { "k_n 5 3 injected nil" })
check("supercalls: n(3) without k_n", function()
	local k_n = SeamBase.k_n
	SeamBase.k_n = nil
	local ok, result = pcall(function()
		return subclass():n(3)
	end)
	SeamBase.k_n = k_n
	assert(ok, result)
	return result
end, -1, {})
check("supercalls: probeKeyed()", function()
	return subclass():probeKeyed()
end, 51, { "k_probeKeyed 5" })

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
