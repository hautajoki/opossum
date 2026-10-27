-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local b = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "b").b
local c = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "c").c
local a = b + c
return {
	a = a,
}
