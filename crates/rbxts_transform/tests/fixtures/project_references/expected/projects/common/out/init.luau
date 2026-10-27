-- Compiled with roblox-ts v3.0.0
local exports = {}
local function add(value)
	exports.total += value
	return exports.total
end
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
	function Counter:constructor()
		self.value = 0
	end
	function Counter:increment()
		self.value += 1
		return self.value
	end
end
exports.total = 0
exports.add = add
exports.Counter = Counter
return exports
