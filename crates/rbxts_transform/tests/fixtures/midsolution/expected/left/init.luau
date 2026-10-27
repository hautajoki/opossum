-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local make = TS.import(script, game:GetService("ReplicatedStorage"), "common").make
local twice = TS.import(script, game:GetService("ReplicatedStorage"), "common", "util", "math").twice
local function left()
	return make(twice(1))
end
return {
	left = left,
}
