-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
--/ <reference path="./globals.d.ts" />
local d = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "d").d
local c = d + 2
return {
	c = c,
}
