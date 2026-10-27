-- Compiled with roblox-ts v3.0.0
-- extends, super calls, super methods, implicit constructors
local Derived, ImplicitDerived, EmptyDerived, AbstractDerived, FromCall, Implementer, SuperIndexed
local function useInheritance()
	print(Derived, ImplicitDerived, EmptyDerived, AbstractDerived, FromCall, Implementer, SuperIndexed)
end
local NestedSuper, SuperNotFirst, FromExpression, SuperMiddle
local function useMore()
	print(NestedSuper, SuperNotFirst, FromExpression, SuperMiddle)
end
local Base
do
	Base = setmetatable({}, {
		__tostring = function()
			return "Base"
		end,
	})
	Base.__index = Base
	function Base.new(...)
		local self = setmetatable({}, Base)
		return self:constructor(...) or self
	end
	function Base:constructor(id)
		self.id = id
		self.name = "base"
	end
	function Base:greet(greeting)
		return `{greeting} {self.name}`
	end
	function Base.create(id)
		return id
	end
	function Base:count()
		return 1
	end
end
do
	local super = Base
	Derived = setmetatable({}, {
		__tostring = function()
			return "Derived"
		end,
		__index = super,
	})
	Derived.__index = Derived
	function Derived.new(...)
		local self = setmetatable({}, Derived)
		return self:constructor(...) or self
	end
	function Derived:constructor(id, tag)
		print("before super")
		super.constructor(self, id)
		self.tag = tag
		self.extra = 1
		print("after super", self.extra)
	end
	function Derived:greet(greeting)
		return super.greet(self, greeting) .. "!"
	end
	function Derived:count()
		return super.count(self) + 1
	end
end
do
	local super = Derived
	ImplicitDerived = setmetatable({}, {
		__tostring = function()
			return "ImplicitDerived"
		end,
		__index = super,
	})
	ImplicitDerived.__index = ImplicitDerived
	function ImplicitDerived.new(...)
		local self = setmetatable({}, ImplicitDerived)
		return self:constructor(...) or self
	end
	function ImplicitDerived:constructor(...)
		super.constructor(self, ...)
		self.more = "x"
	end
end
do
	local super = Base
	EmptyDerived = setmetatable({}, {
		__tostring = function()
			return "EmptyDerived"
		end,
		__index = super,
	})
	EmptyDerived.__index = EmptyDerived
	function EmptyDerived.new(...)
		local self = setmetatable({}, EmptyDerived)
		return self:constructor(...) or self
	end
	function EmptyDerived:constructor(...)
		super.constructor(self, ...)
	end
end
do
	local super = Base
	AbstractDerived = setmetatable({}, {
		__tostring = function()
			return "AbstractDerived"
		end,
		__index = super,
	})
	AbstractDerived.__index = AbstractDerived
	function AbstractDerived:constructor(...)
		super.constructor(self, ...)
	end
end
local function getBase()
	return Base
end
do
	local super = getBase()
	FromCall = setmetatable({}, {
		__tostring = function()
			return "FromCall"
		end,
		__index = super,
	})
	FromCall.__index = FromCall
	function FromCall.new(...)
		local self = setmetatable({}, FromCall)
		return self:constructor(...) or self
	end
	function FromCall:constructor()
		super.constructor(self, 2)
	end
end
do
	Implementer = setmetatable({}, {
		__tostring = function()
			return "Implementer"
		end,
	})
	Implementer.__index = Implementer
	function Implementer.new(...)
		local self = setmetatable({}, Implementer)
		return self:constructor(...) or self
	end
	function Implementer:constructor()
	end
	function Implementer:greet(greeting)
		return greeting
	end
end
do
	local super = Base
	SuperIndexed = setmetatable({}, {
		__tostring = function()
			return "SuperIndexed"
		end,
		__index = super,
	})
	SuperIndexed.__index = SuperIndexed
	function SuperIndexed.new(...)
		local self = setmetatable({}, SuperIndexed)
		return self:constructor(...) or self
	end
	function SuperIndexed:constructor(...)
		super.constructor(self, ...)
	end
	function SuperIndexed:greet(greeting)
		-- comment before the super call
		return super.greet(self, greeting)
	end
end
do
	local super = Base
	NestedSuper = setmetatable({}, {
		__tostring = function()
			return "NestedSuper"
		end,
		__index = super,
	})
	NestedSuper.__index = NestedSuper
	function NestedSuper.new(...)
		local self = setmetatable({}, NestedSuper)
		return self:constructor(...) or self
	end
	function NestedSuper:constructor(flag)
		self.field = "field"
		if flag then
			super.constructor(self, 1)
		else
			super.constructor(self, 2)
		end
		print(self.field)
	end
	function NestedSuper:make()
		return super.create(5)
	end
end
do
	local super = Base
	SuperNotFirst = setmetatable({}, {
		__tostring = function()
			return "SuperNotFirst"
		end,
		__index = super,
	})
	SuperNotFirst.__index = SuperNotFirst
	function SuperNotFirst.new(...)
		local self = setmetatable({}, SuperNotFirst)
		return self:constructor(...) or self
	end
	function SuperNotFirst:constructor(label)
		local id = 3
		super.constructor(self, id)
		self.label = label
	end
end
local _class
do
	local super = Base
	_class = setmetatable({}, {
		__tostring = function()
			return "Anonymous"
		end,
		__index = super,
	})
	_class.__index = _class
	function _class.new(...)
		local self = setmetatable({}, _class)
		return self:constructor(...) or self
	end
	function _class:constructor(...)
		super.constructor(self, ...)
	end
end
local BaseExpression = _class
do
	local super = BaseExpression
	FromExpression = setmetatable({}, {
		__tostring = function()
			return "FromExpression"
		end,
		__index = super,
	})
	FromExpression.__index = FromExpression
	function FromExpression.new(...)
		local self = setmetatable({}, FromExpression)
		return self:constructor(...) or self
	end
	function FromExpression:constructor(...)
		super.constructor(self, ...)
	end
end
local NamedDefaultClass
do
	local super = Base
	NamedDefaultClass = setmetatable({}, {
		__tostring = function()
			return "NamedDefaultClass"
		end,
		__index = super,
	})
	NamedDefaultClass.__index = NamedDefaultClass
	function NamedDefaultClass.new(...)
		local self = setmetatable({}, NamedDefaultClass)
		return self:constructor(...) or self
	end
	function NamedDefaultClass:constructor()
		super.constructor(self, 4)
	end
end
do
	local super = Base
	SuperMiddle = setmetatable({}, {
		__tostring = function()
			return "SuperMiddle"
		end,
		__index = super,
	})
	SuperMiddle.__index = SuperMiddle
	function SuperMiddle.new(...)
		local self = setmetatable({}, SuperMiddle)
		return self:constructor(...) or self
	end
	function SuperMiddle:constructor()
		-- before super
		super.constructor(self, 1)
		-- trailing
		self.q = 1
		-- set up
		print("x")
		-- trailing
	end
end
return {
	useInheritance = useInheritance,
	useMore = useMore,
	default = NamedDefaultClass,
}
