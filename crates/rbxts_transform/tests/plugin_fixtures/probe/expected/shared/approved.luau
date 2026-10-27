-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local impl_1 = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "runtime").impl
local _hoisted2 = 2
-- names the plugin declared after is_usable_local_name approved them are not taken by seam temporaries or imports
local function f()
	local _hoisted = 99
	local a = 1
	return a + _hoisted2
end
local function g()
	local impl = 99
	local b = 1
	return b + impl_1("s", 3)
end
return {
	f = f,
	g = g,
}
