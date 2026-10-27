-- Compiled with roblox-ts v3.0.0
local exports = {}
local function pair()
	return { 1, "a" }
end
local function luaTuple()
	return 1, "a", true
end
local function hoisted()
	local h0, h1
	local function read()
		return h0 + h1
	end
	local _binding = { 1, 2 }
	h0 = _binding[1]
	h1 = _binding[2]
	return read()
end
local function sideEffect()
	print("key")
	return "name"
end
local _binding = {
	alpha = 1,
	beta = "b",
}
local alpha = _binding.alpha
local renamed = _binding.beta
local _binding_1 = pair()
exports.firstLet = _binding_1[1]
exports.secondLet = _binding_1[2]
exports.gamma = ({
	gamma = true,
}).gamma
local config = {
	name = "n",
	nested = {
		inner = 1,
		list = { 1, 2, 3 },
	},
	["key with space"] = true,
	[7] = "seven",
}
-- one named property reads the receiver once
local name = config.name
local configName = config.name
local size = config.size
if size == nil then
	size = 10
end
local whole = ({
	nested = config.nested,
}).nested
-- several properties, nested patterns, defaults, string and numeric keys
local n2 = config.name
local s2 = config.size
if s2 == nil then
	s2 = 5
end
local _binding_2 = config.nested
local inner = _binding_2.inner
local _binding_3 = _binding_2.list
local l1 = _binding_3[1]
local l3 = _binding_3[3]
if l3 == nil then
	l3 = 0
end
local spaced = config["key with space"]
local seven = config[7]
-- object rest
local restName = config.name
local _rest = {}
for _key, _value in config do
	if _key ~= "name" then
		_rest[_key] = _value
	end
end
local others = _rest
local _rest_1 = {}
for _key, _value in config do
	_rest_1[_key] = _value
end
local everything = _rest_1
local oneKey = config.name
local twoKeys = config.size
local _rest_2 = {}
local _extracted = {
	name = true,
	size = true,
}
for _key, _value in config do
	if not _extracted[_key] then
		_rest_2[_key] = _value
	end
end
local remaining = _rest_2
-- computed keys
local key = "name"
local _key = key
local viaConst = config[_key]
local _key_1 = sideEffect()
local viaCall = config[_key_1]
local _rest_3 = {}
local _extracted_1 = {
	[_key] = true,
	[_key_1] = true,
}
for _key_2, _value in config do
	if not _extracted_1[_key_2] then
		_rest_3[_key_2] = _value
	end
end
local afterComputed = _rest_3
-- empty patterns
local _ = config
local _1 = { 1, 2 }
-- array patterns over arrays and tuples
local arr = { 1, 2, 3, 4 }
local a0 = arr[1]
local a1 = arr[2]
local b1 = arr[2]
local b3 = arr[4]
local c0 = arr[1]
local cRest = table.move(arr, 2, #arr, 1, {})
local _array = {}
local _length = #_array
table.move(arr, 1, #arr, _length + 1, _array)
local e0 = _array[1]
local _binding_4 = pair()
local t0 = _binding_4[1]
local t1 = _binding_4[2]
local _binding_5, _binding_6 = pair(), arr
local p0 = _binding_5[1]
local p1 = _binding_5[2]
local q0 = _binding_6[1]
-- LuaTuple calls and array literals declare all names at once
local u0, u1, u2 = luaTuple()
local v0, _2, v2 = luaTuple()
if v2 == nil then
	v2 = false
end
local w0, w1 = 1, 2
local x0, _binding_7 = arr[1], arr
if x0 == nil then
	x0 = 3
end
if _binding_7 == nil then
	_binding_7 = { 4 }
end
local x1 = _binding_7[1]
local y0, _binding_8 = 1, config
local y1 = _binding_8.name
local _binding_9 = { 1, 2, 3 }
local z0 = _binding_9[1]
local zRest = table.move(_binding_9, 2, #_binding_9, 1, {})
print(alpha, renamed, exports.firstLet, exports.secondLet, exports.gamma, name, configName, size, whole, n2, s2, inner, l1, l3, spaced, seven)
print(restName, others, everything, oneKey, twoKeys, remaining, viaConst, viaCall, afterComputed)
print(a0, a1, b1, b3, c0, cRest, e0, t0, t1, p0, p1, q0, u0, u1, u2, v0, v2, w0, w1, x0, x1, y0, y1, z0, zRest)
exports.pair = pair
exports.luaTuple = luaTuple
exports.hoisted = hoisted
exports.sideEffect = sideEffect
exports.alpha = alpha
exports.renamed = renamed
return exports
