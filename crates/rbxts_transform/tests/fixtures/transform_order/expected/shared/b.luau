-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local d = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "d").d
local b = d + 1
return {
	b = b,
}
