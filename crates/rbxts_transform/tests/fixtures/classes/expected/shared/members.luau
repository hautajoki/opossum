-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- unusual members: reserved and non-identifier names, generators, declare fields, index signatures, rest parameters
local dynamicKey = "dynamic"
local Members
do
	Members = setmetatable({}, {
		__tostring = function()
			return "Members"
		end,
	})
	Members.__index = Members
	function Members.new(...)
		local self = setmetatable({}, Members)
		return self:constructor(...) or self
	end
	function Members:constructor(first, ...)
		if first == nil then
			first = 1
		end
		local rest = { ... }
		self[dynamicKey] = 1
		self["with space"] = 2
		self[3] = "three"
		print(first, rest)
	end
	Members["end"] = function(self)
		return "end"
	end
	Members["not an identifier"] = function(self)
		return 1
	end
	function Members:generator()
		return TS.generator(function()
			return 1
		end)
	end
	Members.staticAsync = TS.async(function(self)
		return 1
	end)
	function Members:staticGenerator()
		return TS.generator(function()
			return 2
		end)
	end
	function Members.withThisVoid()
		return 3
	end
	Members["async computed"] = TS.async(function(self)
		return 1
	end)
	Members[dynamicKey] = 4
	Members.staticArrow = function()
		return Members
	end
	Members.staticFunction = function()
		return 1
	end
end
local OverloadedConstructor
do
	OverloadedConstructor = setmetatable({}, {
		__tostring = function()
			return "OverloadedConstructor"
		end,
	})
	OverloadedConstructor.__index = OverloadedConstructor
	function OverloadedConstructor.new(...)
		local self = setmetatable({}, OverloadedConstructor)
		return self:constructor(...) or self
	end
	function OverloadedConstructor:constructor(a)
		print(a)
	end
end
local function makeLocal()
	local InFunction
	do
		InFunction = setmetatable({}, {
			__tostring = function()
				return "InFunction"
			end,
		})
		InFunction.__index = InFunction
		function InFunction.new(...)
			local self = setmetatable({}, InFunction)
			return self:constructor(...) or self
		end
		function InFunction:constructor()
		end
		InFunction.x = 1
	end
	do
		local InBlock
		do
			local super = InFunction
			InBlock = setmetatable({}, {
				__tostring = function()
					return "InBlock"
				end,
				__index = super,
			})
			InBlock.__index = InBlock
			function InBlock.new(...)
				local self = setmetatable({}, InBlock)
				return self:constructor(...) or self
			end
			function InBlock:constructor(...)
				super.constructor(self, ...)
				self.y = InFunction.x
			end
		end
		return InBlock
	end
end
local function useMembers()
	print(Members, OverloadedConstructor)
end
return {
	makeLocal = makeLocal,
	useMembers = useMembers,
}
