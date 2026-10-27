-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local _basics = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "basics")
local Point = _basics.Point
local useClasses = _basics.useClasses
local useDecorators = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "decorators").useDecorators
local DefaultClass = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "defaultClass").default
local useExpressions = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "expressions").useExpressions
local _inheritance = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "inheritance")
local NamedDefaultClass = _inheritance.default
local useInheritance = _inheritance.useInheritance
local useMore = _inheritance.useMore
local _members = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "members")
local makeLocal = _members.makeLocal
local useMembers = _members.useMembers
local NamedDefault
do
	NamedDefault = setmetatable({}, {
		__tostring = function()
			return "NamedDefault"
		end,
	})
	NamedDefault.__index = NamedDefault
	function NamedDefault.new(...)
		local self = setmetatable({}, NamedDefault)
		return self:constructor(...) or self
	end
	function NamedDefault:constructor()
	end
	NamedDefault.value = Point.dimensions
end
print(Point, DefaultClass, NamedDefault)
useClasses()
useDecorators()
useExpressions()
useInheritance()
useMore()
useMembers()
print(makeLocal(), NamedDefaultClass)
return {
	NamedDefault = NamedDefault,
}
