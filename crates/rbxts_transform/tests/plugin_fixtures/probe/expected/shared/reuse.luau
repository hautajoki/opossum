-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local _runtime = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "runtime")
local impl = _runtime.impl
local other = _runtime.other
local value = other
local swapped = impl("two", 2)
return {
	value = value,
	swapped = swapped,
}
