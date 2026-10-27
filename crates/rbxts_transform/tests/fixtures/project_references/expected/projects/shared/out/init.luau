-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local exports = {}
local _src = TS.import(script, game:GetService("ReplicatedStorage"), "common")
local add = _src.add
local Counter = _src.Counter
local _src_1 = TS.import(script, game:GetService("ReplicatedStorage"), "common")
exports.Counter = _src_1.Counter
exports.total = _src_1.total
exports.common = TS.import(script, game:GetService("ReplicatedStorage"), "common")
local function accumulate(values, callback)
	for _, value in values do
		callback(add(value))
	end
end
local NamedCounter
do
	local super = Counter
	NamedCounter = setmetatable({}, {
		__tostring = function()
			return "NamedCounter"
		end,
		__index = super,
	})
	NamedCounter.__index = NamedCounter
	function NamedCounter.new(...)
		local self = setmetatable({}, NamedCounter)
		return self:constructor(...) or self
	end
	function NamedCounter:constructor(...)
		super.constructor(self, ...)
		self.name = "shared"
	end
end
exports.accumulate = accumulate
exports.NamedCounter = NamedCounter
return exports
