-- Compiled with roblox-ts v3.0.0
local function clamp(value, min, max)
	return math.clamp(value, min, max)
end
local Vector
do
	Vector = setmetatable({}, {
		__tostring = function()
			return "Vector"
		end,
	})
	Vector.__index = Vector
	function Vector.new(...)
		local self = setmetatable({}, Vector)
		return self:constructor(...) or self
	end
	function Vector:constructor(x, y)
		self.x = x
		self.y = y
	end
	function Vector:add(other)
		return Vector.new(self.x + other.x, self.y + other.y)
	end
end
return {
	clamp = clamp,
	Vector = Vector,
}
