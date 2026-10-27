-- Compiled with roblox-ts v3.0.0
local TS = _G[script]
local exports = {}
--/ <reference types="types" preserve="true" />
local Players = TS.import(script, TS.getModule(script, "@rbxts", "services")).Players
local _math = TS.import(script, script, "util", "math")
local clamp = _math.clamp
local Vector = _math.Vector
local native = TS.import(script, script, "native").native
for _k, _v in TS.import(script, script, "util", "math") or {} do
	exports[_k] = _v
end
exports.describe = TS.import(script, script, "util", "describe").describe
local function configure(options)
	local _object = {}
	local _left = "name"
	local _condition = options.name
	if _condition == nil then
		_condition = "default"
	end
	_object[_left] = _condition
	local _left_1 = "size"
	local _condition_1 = options.size
	if _condition_1 == nil then
		_condition_1 = native(1)
	end
	_object[_left_1] = _condition_1
	return _object
end
local function origin()
	return Vector.new(0, 0)
end
local function playerCount()
	return #Players:GetPlayers()
end
-- A counter with a limit.
local Counter
do
	Counter = setmetatable({}, {
		__tostring = function()
			return "Counter"
		end,
	})
	Counter.__index = Counter
	function Counter.new(...)
		local self = setmetatable({}, Counter)
		return self:constructor(...) or self
	end
	function Counter:constructor(limit)
		self.limit = limit
		self.value = 0
	end
	function Counter:increment(by)
		if by == nil then
			by = 1
		end
		self.value = clamp(self.value + by, 0, self.limit)
		return self.value
	end
	function Counter:current()
		return self.value
	end
end
local default = Counter
exports.configure = configure
exports.origin = origin
exports.playerCount = playerCount
exports.Counter = Counter
exports.default = default
return exports
