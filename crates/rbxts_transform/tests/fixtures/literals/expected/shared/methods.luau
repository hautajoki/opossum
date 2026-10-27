-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Hand-written literals fixture: methods in object literals.
local key = "k"
local methods = {
	value = 1,
	method = function(self)
		return self.value
	end,
	arrow = function()
		return 1
	end,
	fn = function(self)
		return 2
	end,
	["quoted-name"] = function(self)
		return 3
	end,
	[key] = function(self)
		return 4
	end,
	asyncMethod = TS.async(function(self)
		return 5
	end),
	gen = function(self)
		return TS.generator(function() end)
	end,
	noSelf = function()
		return 6
	end,
	withArgs = function(self, a, b, ...)
		if b == nil then
			b = 2
		end
		local rest = { ... }
		return a + b + rest[1]
	end,
}
-- once the object is a temporary, methods become declarations
local _object = table.clone(record)
setmetatable(_object, nil)
function _object:method()
	return 1
end
function _object.noSelf()
	return 2
end
_object["bad name"] = function(self)
	return 3
end
_object.later = TS.async(function(self)
	return 4
end)
_object["end"] = true
local afterSpread = _object
return {
	methods = methods,
	afterSpread = afterSpread,
}
