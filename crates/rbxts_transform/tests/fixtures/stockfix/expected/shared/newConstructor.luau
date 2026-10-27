-- Compiled with roblox-ts v3.0.0
-- `new` evaluates its constructor before the arguments. roblox-ts read `C` after the arguments' prerequisites ran,
-- so `new C((C = B, "arg"))` constructed a `B`.
local A
do
	A = setmetatable({}, {
		__tostring = function()
			return "A"
		end,
	})
	A.__index = A
	function A.new(...)
		local self = setmetatable({}, A)
		return self:constructor(...) or self
	end
	function A:constructor(tag)
		self.tag = tag
		self.name = "A"
	end
end
local B
do
	B = setmetatable({}, {
		__tostring = function()
			return "B"
		end,
	})
	B.__index = B
	function B.new(...)
		local self = setmetatable({}, B)
		return self:constructor(...) or self
	end
	function B:constructor(tag)
		self.tag = tag
		self.name = "B"
	end
end
local function reassigned()
	local C = A
	local _class = C
	C = B
	local _ = C
	local made = _class.new("arg")
	return { made.name, made.tag, C.new("again").name }
end
-- nothing in the arguments changes the constructor: no capture
local function plain(tag)
	return A.new(tag).tag
end
return {
	reassigned = reassigned,
	plain = plain,
}
