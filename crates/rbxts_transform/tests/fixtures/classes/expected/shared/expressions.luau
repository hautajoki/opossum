-- Compiled with roblox-ts v3.0.0
-- class expressions, hoisting, self-references
local _class
do
	_class = setmetatable({}, {
		__tostring = function()
			return "Anonymous"
		end,
	})
	_class.__index = _class
	function _class.new(...)
		local self = setmetatable({}, _class)
		return self:constructor(...) or self
	end
	function _class:constructor()
		self.value = 1
	end
end
local Anonymous = _class
local _class_1
do
	local Inner = setmetatable({}, {
		__tostring = function()
			return "Inner"
		end,
	})
	Inner.__index = Inner
	function Inner.new(...)
		local self = setmetatable({}, Inner)
		return self:constructor(...) or self
	end
	function Inner:constructor()
	end
	function Inner:method()
		return Inner
	end
	Inner.instance = Inner
	Inner.other = Inner
	_class_1 = Inner
end
local Named = _class_1
local function takesClass(value)
	return value
end
local _class_2
do
	_class_2 = setmetatable({}, {
		__tostring = function()
			return "Anonymous"
		end,
	})
	_class_2.__index = _class_2
	function _class_2.new(...)
		local self = setmetatable({}, _class_2)
		return self:constructor(...) or self
	end
	function _class_2:constructor()
	end
	_class_2.x = 1
end
takesClass(_class_2)
local _class_3
do
	local super = Named
	local Local = setmetatable({}, {
		__tostring = function()
			return "Local"
		end,
		__index = super,
	})
	Local.__index = Local
	function Local.new(...)
		local self = setmetatable({}, Local)
		return self:constructor(...) or self
	end
	function Local:constructor(...)
		super.constructor(self, ...)
	end
	Local.y = Local
	_class_3 = Local
end
takesClass(_class_3)
local Later
local function usesLater()
	return Later
end
do
	Later = setmetatable({}, {
		__tostring = function()
			return "Later"
		end,
	})
	Later.__index = Later
	function Later.new(...)
		local self = setmetatable({}, Later)
		return self:constructor(...) or self
	end
	function Later:constructor()
	end
	Later.value = 1
end
local function useExpressions()
	print(Anonymous, Named, usesLater())
end
return {
	useExpressions = useExpressions,
}
