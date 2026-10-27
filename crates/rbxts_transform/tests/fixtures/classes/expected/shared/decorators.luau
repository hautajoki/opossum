-- Compiled with roblox-ts v3.0.0
-- experimentalDecorators: class, method, property, parameter and static decorators
local mutableDecorator, classDecorator, Stacked, ConstructorParameters, Mutable, ComputedKeys, DecoratedWithConstructor, Expression
local function useDecorators()
	mutableDecorator = classDecorator
	print(Stacked, ConstructorParameters, Mutable, ComputedKeys, DecoratedWithConstructor, Expression)
end
function classDecorator(target)
	print("class", target)
end
local function classFactory(name)
	print("factory", name)
	return function(target)
		print("class decorator", name, target)
	end
end
local function methodDecorator(target, key, descriptor)
	print("method", target, key, descriptor)
end
local function methodFactory(order)
	return function(target, key, descriptor)
		print("method decorator", order, target, key, descriptor)
	end
end
local function propertyDecorator(target, key)
	print("property", target, key)
end
local function parameterDecorator(target, key, index)
	print("parameter", target, key, index)
end
mutableDecorator = function(target)
	print("mutable", target)
end
local Simple
do
	Simple = setmetatable({}, {
		__tostring = function()
			return "Simple"
		end,
	})
	Simple.__index = Simple
	function Simple.new(...)
		local self = setmetatable({}, Simple)
		return self:constructor(...) or self
	end
	function Simple:constructor()
	end
	Simple = classDecorator(Simple) or Simple
end
do
	Stacked = setmetatable({}, {
		__tostring = function()
			return "Stacked"
		end,
	})
	Stacked.__index = Stacked
	function Stacked.new(...)
		local self = setmetatable({}, Stacked)
		return self:constructor(...) or self
	end
	function Stacked:constructor()
		self.field = 1
	end
	function Stacked:method()
	end
	function Stacked:withParameters(a)
	end
	function Stacked:staticMethod()
	end
	function Stacked:plain()
	end
	Stacked.staticField = 2
	propertyDecorator(Stacked, "field")
	local _descriptor = methodDecorator(Stacked, "method", {
		value = Stacked.method,
	})
	if _descriptor then
		Stacked.method = _descriptor.value
	end
	local _decorator = methodFactory(1)
	local _decorator_1 = methodFactory(2)
	parameterDecorator(Stacked, "withParameters", 0)
	local _descriptor_1 = _decorator_1(Stacked, "withParameters", {
		value = Stacked.withParameters,
	})
	if _descriptor_1 then
		Stacked.withParameters = _descriptor_1.value
	end
	local _descriptor_2 = _decorator(Stacked, "withParameters", {
		value = Stacked.withParameters,
	})
	if _descriptor_2 then
		Stacked.withParameters = _descriptor_2.value
	end
	propertyDecorator(Stacked, "staticField")
	local _descriptor_3 = methodDecorator(Stacked, "staticMethod", {
		value = Stacked.staticMethod,
	})
	if _descriptor_3 then
		Stacked.staticMethod = _descriptor_3.value
	end
	local _decorator_2 = classFactory("first")
	local _decorator_3 = classFactory("second")
	Stacked = classDecorator(Stacked) or Stacked
	Stacked = _decorator_3(Stacked) or Stacked
	Stacked = _decorator_2(Stacked) or Stacked
end
do
	ConstructorParameters = setmetatable({}, {
		__tostring = function()
			return "ConstructorParameters"
		end,
	})
	ConstructorParameters.__index = ConstructorParameters
	function ConstructorParameters.new(...)
		local self = setmetatable({}, ConstructorParameters)
		return self:constructor(...) or self
	end
	function ConstructorParameters:constructor(first, second)
	end
	function ConstructorParameters:method(a, b, c)
	end
	parameterDecorator(ConstructorParameters, "method", 2)
	parameterDecorator(ConstructorParameters, "method", 0)
	parameterDecorator(ConstructorParameters, nil, 1)
	parameterDecorator(ConstructorParameters, nil, 0)
end
do
	Mutable = setmetatable({}, {
		__tostring = function()
			return "Mutable"
		end,
	})
	Mutable.__index = Mutable
	function Mutable.new(...)
		local self = setmetatable({}, Mutable)
		return self:constructor(...) or self
	end
	function Mutable:constructor()
	end
	local _decorator = mutableDecorator
	Mutable = mutableDecorator(Mutable) or Mutable
	Mutable = _decorator(Mutable) or Mutable
end
local key = "dynamic"
do
	ComputedKeys = setmetatable({}, {
		__tostring = function()
			return "ComputedKeys"
		end,
	})
	ComputedKeys.__index = ComputedKeys
	function ComputedKeys.new(...)
		local self = setmetatable({}, ComputedKeys)
		return self:constructor(...) or self
	end
	function ComputedKeys:constructor()
	end
	local _key = key
	ComputedKeys[_key] = function(self) end
	ComputedKeys["literal key"] = function(self) end
	local _descriptor = methodDecorator(ComputedKeys, _key, {
		value = ComputedKeys[_key],
	})
	if _descriptor then
		ComputedKeys[_key] = _descriptor.value
	end
	local _descriptor_1 = methodDecorator(ComputedKeys, "literal key", {
		value = ComputedKeys["literal key"],
	})
	if _descriptor_1 then
		ComputedKeys["literal key"] = _descriptor_1.value
	end
end
do
	DecoratedWithConstructor = setmetatable({}, {
		__tostring = function()
			return "DecoratedWithConstructor"
		end,
	})
	DecoratedWithConstructor.__index = DecoratedWithConstructor
	function DecoratedWithConstructor.new(...)
		local self = setmetatable({}, DecoratedWithConstructor)
		return self:constructor(...) or self
	end
	function DecoratedWithConstructor:constructor(value)
	end
	parameterDecorator(DecoratedWithConstructor, nil, 0)
	DecoratedWithConstructor = classDecorator(DecoratedWithConstructor) or DecoratedWithConstructor
end
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
	end
end
Expression = _class
return {
	useDecorators = useDecorators,
	Simple = Simple,
}
