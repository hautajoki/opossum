-- Compiled with roblox-ts v3.0.0
-- Function values in slots whose signature has a `this` parameter. TypeScript 5 gives a function expression or
-- object literal method without an explicit `this` the contextual signature's `this`; TypeScript 7 only when the
-- function uses `this` or has an unannotated parameter.
local function accept(callback)
	return callback
end
-- a parameterless function expression in a method slot (roblox-ts's array.spec)
local parameterless = { function(self)
	print("parameterless")
end, function(self, arg)
	print(arg)
end }
-- only annotated parameters: TypeScript 7 alone reported "expected method, got function" here
local annotated = function(self, arg)
	print(arg)
end
-- through an optional parameter, whose contextual type is a union with undefined
accept(function(self, arg)
	print(arg)
end)
accept(function(self) end)
-- object literal members against their contextual property types
local holder = {
	method = function(self, arg)
		print(arg)
	end,
	callback = function(arg)
		print(arg)
	end,
	short = function(arg)
		print(arg)
	end,
}
holder:method("x")
holder.callback("y")
holder.short("z")
-- neighbours that must not change
local arrow = function()
	return print("arrow")
end
local usesThis = function(self, arg)
	print(self, arg)
end
local explicitThis = function(self, arg)
	print(arg)
end
local voidSlot = function(arg)
	print(arg)
end
local plain = function(arg)
	print(arg)
end
return {
	accept = accept,
	parameterless = parameterless,
	annotated = annotated,
	holder = holder,
	arrow = arrow,
	usesThis = usesThis,
	explicitThis = explicitThis,
	voidSlot = voidSlot,
	plain = plain,
}
